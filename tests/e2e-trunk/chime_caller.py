#!/usr/bin/env python3
"""Hermetic synthetic Chime-dialect SIP UAC for the durable e2e-trunk suite.

Emulates the Chime inbound leg (per REPORT-M0a): PCMU/PT0 + telephone-event/
PT101, ptime 20 / 50 pps, no 100rel, no session timers. It:
  * places a call into rustisk's PIN gate (INVITE, offer PCMU+RFC2833);
  * enters a PIN via RFC2833 (in-band RTP) DTMF once answered;
  * emits a continuous PCMU tone (default 440 Hz) so that, once GRANTED and
    bridged, the tone reaches the far leg;
  * captures the return RTP (audio bridged back FROM the far leg) and, over a
    late measurement window (after the bridge is up), tone-detects the far
    end's distinct tone (default 660 Hz) to prove the bridge->caller direction.

Receiver-side proof only, exactly like the M0/M2 harnesses. Pure Python
stdlib — the FFT is replaced by a Goertzel filter so CI needs no numpy.
This is the hermetic sibling of the numpy M9 caller in integration/.
"""
import argparse
import json
import math
import os
import random
import socket
import struct
import sys
import threading
import time
import wave


def log(*a):
    print("[caller]", *a, file=sys.stderr, flush=True)


# ---- mulaw + RTP (helpers copied from tests/freeswitch-pin-gate/rtp_injector.py
#      lineage; identical math to the proven M9 caller) ----
def linear_to_mulaw(sample):
    bias = 0x84
    clip = 32635
    sign = 0x80 if sample < 0 else 0
    magnitude = min(abs(sample), clip) + bias
    exponent = 7
    mask = 0x4000
    while exponent > 0 and not magnitude & mask:
        exponent -= 1
        mask >>= 1
    mantissa = (magnitude >> (exponent + 3)) & 0x0F
    return (~(sign | (exponent << 4) | mantissa)) & 0xFF


_MULAW_DECODE = None


def mulaw_to_linear(u):
    global _MULAW_DECODE
    if _MULAW_DECODE is None:
        tab = []
        for i in range(256):
            u2 = ~i & 0xFF
            sign = u2 & 0x80
            exp = (u2 >> 4) & 0x07
            man = u2 & 0x0F
            mag = ((man << 3) + 0x84) << exp
            mag -= 0x84
            tab.append(-mag if sign else mag)
        _MULAW_DECODE = tab
    return _MULAW_DECODE[u & 0xFF]


def tone_pcmu(freq, packet_index, amp=10000):
    out = bytearray()
    for offset in range(160):
        absolute = packet_index * 160 + offset
        s = round(amp * math.sin(2 * math.pi * freq * absolute / 8000))
        out.append(linear_to_mulaw(s))
    return bytes(out)


def rtp_hdr(pt, seq, ts, ssrc, marker=0):
    return struct.pack(
        "!BBHII", 0x80, ((marker & 1) << 7) | (pt & 0x7F), seq & 0xFFFF, ts & 0xFFFFFFFF, ssrc & 0xFFFFFFFF
    )


# ---- Goertzel tone detector (stdlib, replaces numpy FFT) ----
def goertzel(samples, target_hz, rate=8000):
    """Normalized single-bin power at target_hz. Returns a ratio scaled so the
    numbers are comparable across runs (mag / total-energy * 1e4)."""
    n = len(samples)
    if n < 320:
        return 0.0
    k = int(0.5 + (n * target_hz) / rate)
    w = (2.0 * math.pi / n) * k
    coeff = 2.0 * math.cos(w)
    s_prev = 0.0
    s_prev2 = 0.0
    energy = 0.0
    for x in samples:
        energy += x * x
        s = x + coeff * s_prev - s_prev2
        s_prev2 = s_prev
        s_prev = s
    power = s_prev2 * s_prev2 + s_prev * s_prev - coeff * s_prev * s_prev2
    if energy <= 0:
        return 0.0
    return round(math.sqrt(max(power, 0.0)) / math.sqrt(energy) * 1e4, 2)


def parse_sdp_pts(body):
    """Return (pcmu_pt, tev_pt, remote_ip, remote_port) from an SDP answer."""
    pcmu = 0
    tev = 101
    rip = None
    rport = None
    for line in body.splitlines():
        line = line.strip()
        if line.startswith("m=audio"):
            rport = int(line.split()[1])
        elif line.startswith("c=IN IP4"):
            rip = line.split()[-1]
        elif line.lower().startswith("a=rtpmap:"):
            pt, enc = line[len("a=rtpmap:"):].split(None, 1)
            if enc.upper().startswith("PCMU/"):
                pcmu = int(pt)
            if enc.lower().startswith("telephone-event/"):
                tev = int(pt)
    return pcmu, tev, rip, rport


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dst-ip", required=True)
    ap.add_argument("--dst-port", type=int, required=True)
    ap.add_argument("--src-ip", required=True)
    ap.add_argument("--sip-port", type=int, default=35062)
    ap.add_argument("--rtp-port", type=int, default=36200)
    ap.add_argument("--exten", default="9000")
    ap.add_argument("--pin", required=True)
    ap.add_argument("--tone", type=int, default=440)  # A -> far-end tone
    ap.add_argument("--detect", type=int, default=660)  # far-end -> A tone we expect back
    ap.add_argument("--call-secs", type=float, default=14.0)
    ap.add_argument("--dtmf-start", type=float, default=1.5)  # after 200 OK
    ap.add_argument("--measure-start", type=float, default=6.0)  # accumulate RX after bridge is up
    ap.add_argument("--rx-wav", required=True)
    ap.add_argument("--result-file", required=True)
    args = ap.parse_args()

    ssrc = random.randint(0, 0x7FFFFFFF)
    callid = f"{random.randint(0, 1 << 48):012x}@{args.src_ip}"
    fromtag = f"{random.randint(0, 1 << 32):08x}"
    branch = f"z9hG4bK{random.randint(0, 1 << 32):08x}"
    cseq = 1

    sipsock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sipsock.bind((args.src_ip, args.sip_port))
    sipsock.settimeout(5)
    rtpsock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    rtpsock.bind((args.src_ip, args.rtp_port))
    rtpsock.settimeout(0.25)

    sdp = (
        f"v=0\r\no=- {random.randint(1, 1 << 30)} 1 IN IP4 {args.src_ip}\r\ns=chime\r\n"
        f"c=IN IP4 {args.src_ip}\r\nt=0 0\r\nm=audio {args.rtp_port} RTP/AVP 0 101\r\n"
        f"a=rtpmap:0 PCMU/8000\r\na=rtpmap:101 telephone-event/8000\r\n"
        f"a=fmtp:101 0-15\r\na=ptime:20\r\na=sendrecv\r\n"
    )
    ruri = f"sip:{args.exten}@{args.dst_ip}:{args.dst_port}"
    invite = (
        f"INVITE {ruri} SIP/2.0\r\n"
        f"Via: SIP/2.0/UDP {args.src_ip}:{args.sip_port};branch={branch};rport\r\n"
        f"Max-Forwards: 70\r\n"
        f"From: <sip:chime@{args.src_ip}>;tag={fromtag}\r\n"
        f"To: <{ruri}>\r\n"
        f"Call-ID: {callid}\r\nCSeq: {cseq} INVITE\r\n"
        f"Contact: <sip:chime@{args.src_ip}:{args.sip_port}>\r\n"
        f"Content-Type: application/sdp\r\nContent-Length: {len(sdp)}\r\n\r\n{sdp}"
    )
    sipsock.sendto(invite.encode(), (args.dst_ip, args.dst_port))
    log(f"INVITE -> {ruri}")

    totag = None
    ok_body = None
    deadline = time.time() + 8
    while time.time() < deadline:
        try:
            data, _ = sipsock.recvfrom(65535)
        except socket.timeout:
            sipsock.sendto(invite.encode(), (args.dst_ip, args.dst_port))
            continue
        msg = data.decode("latin1")
        first = msg.split("\r\n", 1)[0]
        log("SIP <=", first)
        if " 200 " in first:
            for l in msg.split("\r\n"):
                if l.lower().startswith("to:") and "tag=" in l.lower():
                    totag = l.split("tag=", 1)[1].strip()
            ok_body = msg.split("\r\n\r\n", 1)[1] if "\r\n\r\n" in msg else ""
            break
        # 100/180/183 -> keep waiting
    if ok_body is None:
        log("no 200 OK; aborting")
        _write_result(args, {"error": "no_200_ok", "voice_rx": 0}, exit_code=2)
        return 2

    pcmu_pt, tev_pt, rip, rport = parse_sdp_pts(ok_body)
    if not rip or rip == "0.0.0.0":
        rip = args.dst_ip
    log(f"answered: PCMU_PT={pcmu_pt} TEV_PT={tev_pt} remote_media={rip}:{rport}")
    ack = (
        f"ACK {ruri} SIP/2.0\r\n"
        f"Via: SIP/2.0/UDP {args.src_ip}:{args.sip_port};branch=z9hG4bK{random.randint(0, 1 << 32):08x};rport\r\n"
        f"Max-Forwards: 70\r\nFrom: <sip:chime@{args.src_ip}>;tag={fromtag}\r\n"
        f"To: <{ruri}>;tag={totag}\r\nCall-ID: {callid}\r\nCSeq: {cseq} ACK\r\nContent-Length: 0\r\n\r\n"
    )
    sipsock.sendto(ack.encode(), (args.dst_ip, args.dst_port))

    # ---- media threads ----
    stop = threading.Event()
    rx_frames = {"voice": 0, "other": 0, "bytes": 0}
    rx_pcm = []  # accumulated ONLY during the measurement window (post-bridge)
    measure_open = threading.Event()

    def rx_loop():
        while not stop.is_set():
            try:
                pkt, _ = rtpsock.recvfrom(4096)
            except socket.timeout:
                continue
            if len(pkt) < 12:
                continue
            pt = pkt[1] & 0x7F
            rx_frames["bytes"] += len(pkt)
            payload = pkt[12:]
            if pt == pcmu_pt and len(payload) > 0:
                rx_frames["voice"] += 1
                if measure_open.is_set():
                    rx_pcm.extend(mulaw_to_linear(b) for b in payload)
            else:
                rx_frames["other"] += 1

    threading.Thread(target=rx_loop, daemon=True).start()

    seq = random.randint(0, 0x7FFF)
    ts = random.randint(0, 0x7FFFFFFF)
    t0 = time.time()
    dtmf_sent = False

    def send_voice_packet(idx):
        nonlocal seq, ts
        pkt = rtp_hdr(pcmu_pt, seq, ts, ssrc) + tone_pcmu(args.tone, idx)
        rtpsock.sendto(pkt, (rip, rport))
        seq = (seq + 1) & 0xFFFF
        ts = (ts + 160) & 0xFFFFFFFF

    def send_dtmf(digit):
        nonlocal seq, ts
        evmap = {**{str(d): d for d in range(10)}, "*": 10, "#": 11, "A": 12, "B": 13, "C": 14, "D": 15}
        ev = evmap[digit]
        ev_ts = ts
        dur = 0
        for i in range(8):  # 8 event packets (~160 ms), marker on first
            dur += 160
            payload = struct.pack("!BBH", ev, (0 << 7) | 10, dur)
            pkt = rtp_hdr(tev_pt, seq, ev_ts, ssrc, marker=1 if i == 0 else 0) + payload
            rtpsock.sendto(pkt, (rip, rport))
            seq = (seq + 1) & 0xFFFF
            time.sleep(0.02)
        for _ in range(3):  # 3 end packets, E=1
            payload = struct.pack("!BBH", ev, (1 << 7) | 10, dur)
            pkt = rtp_hdr(tev_pt, seq, ev_ts, ssrc) + payload
            rtpsock.sendto(pkt, (rip, rport))
            seq = (seq + 1) & 0xFFFF
            time.sleep(0.02)
        ts = (ts + 160 * 11) & 0xFFFFFFFF

    idx = 0
    while time.time() - t0 < args.call_secs:
        cyc = time.time() - t0
        if (not dtmf_sent) and cyc >= args.dtmf_start:
            log(f"sending PIN via RFC2833 (TEV_PT={tev_pt})")
            for d in args.pin:
                send_dtmf(d)
                time.sleep(0.06)  # inter-digit gap
            send_dtmf("#")  # terminate entry
            dtmf_sent = True
            continue
        if (not measure_open.is_set()) and cyc >= args.measure_start:
            measure_open.set()
            log(f"measurement window open at t+{cyc:.1f}s")
        send_voice_packet(idx)
        idx += 1
        time.sleep(0.02)

    stop.set()
    time.sleep(0.3)
    bye = (
        f"BYE {ruri} SIP/2.0\r\n"
        f"Via: SIP/2.0/UDP {args.src_ip}:{args.sip_port};branch=z9hG4bK{random.randint(0, 1 << 32):08x};rport\r\n"
        f"Max-Forwards: 70\r\nFrom: <sip:chime@{args.src_ip}>;tag={fromtag}\r\n"
        f"To: <{ruri}>;tag={totag}\r\nCall-ID: {callid}\r\nCSeq: {cseq + 1} BYE\r\nContent-Length: 0\r\n\r\n"
    )
    sipsock.sendto(bye.encode(), (args.dst_ip, args.dst_port))

    # write RX wav (8k mono s16) from the measurement window
    with wave.open(args.rx_wav, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(8000)
        w.writeframes(struct.pack(f"<{len(rx_pcm)}h", *(max(-32768, min(32767, int(s))) for s in rx_pcm)))

    det = {}
    if len(rx_pcm) >= 1600:
        mean = sum(rx_pcm) / len(rx_pcm)
        centered = [s - mean for s in rx_pcm]
        for f0 in (args.detect, args.tone, 350, 880):
            det[f0] = goertzel(centered, f0)

    result = {
        "voice_rx": rx_frames["voice"],
        "other_rx": rx_frames["other"],
        "rx_bytes": rx_frames["bytes"],
        "rx_samples": len(rx_pcm),
        "tone_ratios": det,
        "detect_hz": args.detect,
    }
    _write_result(args, result, exit_code=0)
    print(
        f"RESULT voice_rx={rx_frames['voice']} other_rx={rx_frames['other']} "
        f"rx_bytes={rx_frames['bytes']} rx_samples={len(rx_pcm)} tone_ratios={det}"
    )
    return 0


def _write_result(args, result, exit_code):
    tmp = args.result_file + ".tmp"
    with open(tmp, "w") as f:
        json.dump(result, f)
    os.replace(tmp, args.result_file)


if __name__ == "__main__":
    sys.exit(main())
