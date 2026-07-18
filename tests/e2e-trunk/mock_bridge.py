#!/usr/bin/env python3
"""Hermetic MOCK registered SIP endpoint that stands in for the pymumble
qa-bridge in the durable e2e-trunk suite.

It replaces the whole Mumble/voice stack with a tiny, deterministic UAS so the
trunk e2e is self-contained (no external services). It:

  1. REGISTERs to rustisk as `qa-bridge` with MD5 digest auth (realm "asterisk",
     qop=auth) so rustisk's registrar binds a dynamic AOR contact and
     `Dial(PJSIP/qa-bridge)` resolves to THIS process (proven registrar +
     dynamic-AOR routing path, see rustisk registrar.rs / channel_driver.rs).
  2. Answers the INVITE rustisk sends when a GRANTED PIN triggers the Dial
     (100 Trying -> 200 OK with a PCMU + telephone-event SDP answer).
  3. Runs bidirectional media: it TRANSMITS a distinct steady tone (default
     660 Hz) toward the bridge AND RECEIVES the caller's relayed tone,
     Goertzel-detecting the caller's tone (default 440 Hz). Distinct tones are
     used (rather than a pure echo) precisely so each RTP direction is
     independently attributable by frequency — a strictly stronger two-way
     proof than an echo, which cannot tell a relayed frame from a socket loop.

It records an `invite_count`: 0 proves the REJECTED path never Dialed the
bridge; 1 (with RX frames + a detected caller tone) proves the GRANTED path
bridged real two-way media.

Pure Python stdlib (Goertzel, not numpy) so it runs on a bare CI runner.
"""
import argparse
import hashlib
import json
import math
import os
import random
import signal
import socket
import struct
import sys
import threading
import time

_TERMINATE = threading.Event()


def log(*a):
    print("[mock-bridge]", *a, file=sys.stderr, flush=True)


# ---- mulaw + RTP + Goertzel (same lineage as chime_caller.py) ----
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


def goertzel(samples, target_hz, rate=8000):
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


def md5hex(s):
    return hashlib.md5(s.encode()).hexdigest()


def header(msg, name):
    """First header value (case-insensitive) from a raw SIP message."""
    want = name.lower() + ":"
    for line in msg.split("\r\n"):
        if line.lower().startswith(want):
            return line.split(":", 1)[1].strip()
    return None


def all_via(msg):
    return [line for line in msg.split("\r\n") if line.lower().startswith("via:")]


def parse_challenge(msg):
    """Extract realm+nonce+qop from a 401 WWW-Authenticate header."""
    wa = header(msg, "WWW-Authenticate") or ""
    fields = {}
    for part in wa[len("Digest"):].split(",") if wa.lower().startswith("digest") else []:
        if "=" in part:
            k, v = part.split("=", 1)
            fields[k.strip().lower()] = v.strip().strip('"')
    return fields.get("realm", "asterisk"), fields.get("nonce", ""), fields.get("qop", "")


def parse_sdp(body):
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


def register(sipsock, args):
    """Send REGISTER, answer the 401 digest challenge, expect 200 OK."""
    reg_domain = f"{args.reg_ip}:{args.reg_port}"
    ruri = f"sip:{reg_domain}"
    callid = f"{random.randint(0, 1 << 48):012x}@{args.src_ip}"
    fromtag = f"{random.randint(0, 1 << 32):08x}"
    contact = f"<sip:{args.username}@{args.src_ip}:{args.sip_port}>"
    cseq = 1

    def build(auth_header=None):
        branch = f"z9hG4bK{random.randint(0, 1 << 32):08x}"
        lines = [
            f"REGISTER {ruri} SIP/2.0",
            f"Via: SIP/2.0/UDP {args.src_ip}:{args.sip_port};branch={branch};rport",
            "Max-Forwards: 70",
            f"From: <sip:{args.username}@{args.reg_ip}>;tag={fromtag}",
            f"To: <sip:{args.username}@{args.reg_ip}>",
            f"Call-ID: {callid}",
            f"CSeq: {cseq} REGISTER",
            f"Contact: {contact}",
            f"Expires: {args.expires}",
        ]
        if auth_header:
            lines.append(auth_header)
        lines += ["Content-Length: 0", "", ""]
        return "\r\n".join(lines)

    sipsock.sendto(build().encode(), (args.reg_ip, args.reg_port))
    log(f"REGISTER -> {ruri} (as {args.username})")
    deadline = time.time() + 8
    while time.time() < deadline:
        try:
            data, _ = sipsock.recvfrom(65535)
        except socket.timeout:
            continue
        msg = data.decode("latin1")
        first = msg.split("\r\n", 1)[0]
        if "401" in first or "407" in first:
            realm, nonce, qop = parse_challenge(msg)
            realm = args.realm or realm
            cseq += 1
            ha1 = md5hex(f"{args.username}:{realm}:{args.password}")
            ha2 = md5hex(f"REGISTER:{ruri}")
            if "auth" in (qop or ""):
                cnonce = f"{random.randint(0, 1 << 32):08x}"
                nc = "00000001"
                resp = md5hex(f"{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}")
                auth = (
                    f'Authorization: Digest username="{args.username}", realm="{realm}", '
                    f'nonce="{nonce}", uri="{ruri}", response="{resp}", algorithm=MD5, '
                    f'qop=auth, nc={nc}, cnonce="{cnonce}"'
                )
            else:
                resp = md5hex(f"{ha1}:{nonce}:{ha2}")
                auth = (
                    f'Authorization: Digest username="{args.username}", realm="{realm}", '
                    f'nonce="{nonce}", uri="{ruri}", response="{resp}", algorithm=MD5'
                )
            sipsock.sendto(build(auth).encode(), (args.reg_ip, args.reg_port))
            log("REGISTER (authed) -> resending with digest response")
            continue
        if " 200 " in first:
            log("REGISTER: 200 OK (contact bound)")
            return True
        log("REGISTER <=", first)
    return False


def respond(sipsock, dst, status, msg, extra_headers=None, body="", to_tag=None):
    """Build a response echoing dialog headers from the request `msg`."""
    lines = [f"SIP/2.0 {status}"]
    lines += all_via(msg)
    frm = header(msg, "From")
    to = header(msg, "To")
    if to_tag and "tag=" not in (to or "").lower():
        to = f"{to};tag={to_tag}"
    lines.append(f"From: {frm}")
    lines.append(f"To: {to}")
    lines.append(f"Call-ID: {header(msg, 'Call-ID')}")
    lines.append(f"CSeq: {header(msg, 'CSeq')}")
    if extra_headers:
        lines += extra_headers
    if body:
        lines.append("Content-Type: application/sdp")
        lines.append(f"Content-Length: {len(body)}")
        lines += ["", body]
    else:
        lines += ["Content-Length: 0", "", ""]
    sipsock.sendto(("\r\n".join(lines)).encode(), dst)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--reg-ip", required=True)  # rustisk SIP IP
    ap.add_argument("--reg-port", type=int, required=True)  # rustisk SIP port
    ap.add_argument("--src-ip", required=True)  # this mock's IP
    ap.add_argument("--sip-port", type=int, default=35064)
    ap.add_argument("--rtp-port", type=int, default=36300)
    ap.add_argument("--username", default="qa-bridge")
    ap.add_argument("--password", required=True)
    ap.add_argument("--realm", default="asterisk")
    ap.add_argument("--expires", type=int, default=300)
    ap.add_argument("--tx-tone", type=int, default=660)  # distinct far-end tone
    ap.add_argument("--detect", type=int, default=440)  # caller tone we expect to receive
    ap.add_argument("--run-secs", type=float, default=40.0)
    ap.add_argument("--ready-file", required=True)
    ap.add_argument("--result-file", required=True)
    args = ap.parse_args()

    # SIGTERM (from run.sh, e.g. to end the REJECTED case where no INVITE ever
    # arrives) finalizes gracefully: break the loop, write the result file.
    signal.signal(signal.SIGTERM, lambda *_: _TERMINATE.set())

    sipsock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sipsock.bind((args.src_ip, args.sip_port))
    sipsock.settimeout(0.25)
    rtpsock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    rtpsock.bind((args.src_ip, args.rtp_port))
    rtpsock.settimeout(0.25)

    if not register(sipsock, args):
        _write(args, {"registered": False, "invite_count": 0, "rx_voice": 0}, exit_code=3)
        return 3
    open(args.ready_file, "w").write("registered\n")

    state = {"invite_count": 0, "rx_voice": 0, "rx_bytes": 0}
    seen_callids = set()
    stop_media = threading.Event()
    got_bye = threading.Event()
    rx_pcm = []
    media_started = threading.Event()
    remote = {"ip": None, "port": None, "pcmu_pt": 0}

    def rx_loop():
        while not stop_media.is_set():
            try:
                pkt, _ = rtpsock.recvfrom(4096)
            except socket.timeout:
                continue
            if len(pkt) < 12:
                continue
            pt = pkt[1] & 0x7F
            state["rx_bytes"] += len(pkt)
            payload = pkt[12:]
            if pt == remote["pcmu_pt"] and payload:
                state["rx_voice"] += 1
                rx_pcm.extend(mulaw_to_linear(b) for b in payload)

    def tx_loop():
        seq = random.randint(0, 0x7FFF)
        ts = random.randint(0, 0x7FFFFFFF)
        ssrc = random.randint(0, 0x7FFFFFFF)
        idx = 0
        media_started.wait()
        while not stop_media.is_set():
            if remote["ip"]:
                pkt = rtp_hdr(remote["pcmu_pt"], seq, ts, ssrc) + tone_pcmu(args.tx_tone, idx)
                try:
                    rtpsock.sendto(pkt, (remote["ip"], remote["port"]))
                except OSError:
                    pass
                seq = (seq + 1) & 0xFFFF
                ts = (ts + 160) & 0xFFFFFFFF
                idx += 1
            time.sleep(0.02)

    threading.Thread(target=rx_loop, daemon=True).start()
    threading.Thread(target=tx_loop, daemon=True).start()

    to_tag = f"{random.randint(0, 1 << 32):08x}"
    deadline = time.time() + args.run_secs
    while time.time() < deadline and not _TERMINATE.is_set():
        try:
            data, addr = sipsock.recvfrom(65535)
        except socket.timeout:
            if got_bye.is_set():
                break
            continue
        msg = data.decode("latin1")
        first = msg.split("\r\n", 1)[0]
        method = first.split(" ", 1)[0].upper()
        if method == "INVITE":
            callid = header(msg, "Call-ID")
            body = msg.split("\r\n\r\n", 1)[1] if "\r\n\r\n" in msg else ""
            pcmu_pt, tev_pt, rip, rport = parse_sdp(body)
            remote["ip"], remote["port"], remote["pcmu_pt"] = rip, rport, pcmu_pt
            if callid not in seen_callids:
                seen_callids.add(callid)
                state["invite_count"] += 1
                log(f"INVITE #{state['invite_count']} from {addr} remote_media={rip}:{rport} PCMU_PT={pcmu_pt}")
            respond(sipsock, addr, "100 Trying", msg)
            answer = (
                f"v=0\r\no=- {random.randint(1, 1 << 30)} 1 IN IP4 {args.src_ip}\r\ns=mock-bridge\r\n"
                f"c=IN IP4 {args.src_ip}\r\nt=0 0\r\nm=audio {args.rtp_port} RTP/AVP {pcmu_pt} {tev_pt}\r\n"
                f"a=rtpmap:{pcmu_pt} PCMU/8000\r\na=rtpmap:{tev_pt} telephone-event/8000\r\n"
                f"a=fmtp:{tev_pt} 0-15\r\na=ptime:20\r\na=sendrecv\r\n"
            )
            respond(
                sipsock,
                addr,
                "200 OK",
                msg,
                extra_headers=[f"Contact: <sip:{args.username}@{args.src_ip}:{args.sip_port}>"],
                body=answer,
                to_tag=to_tag,
            )
            media_started.set()
        elif method == "ACK":
            media_started.set()
        elif method == "BYE":
            respond(sipsock, addr, "200 OK", msg)
            log("BYE received -> 200 OK; draining")
            got_bye.set()
            # brief drain so trailing RTP is counted, then exit the loop
            time.sleep(0.4)
            break
        elif method in ("OPTIONS", "INFO", "UPDATE"):
            respond(sipsock, addr, "200 OK", msg)

    stop_media.set()
    time.sleep(0.2)

    det = {}
    if len(rx_pcm) >= 1600:
        mean = sum(rx_pcm) / len(rx_pcm)
        centered = [s - mean for s in rx_pcm]
        for f0 in (args.detect, args.tx_tone, 350, 880):
            det[f0] = goertzel(centered, f0)

    result = {
        "registered": True,
        "invite_count": state["invite_count"],
        "rx_voice": state["rx_voice"],
        "rx_bytes": state["rx_bytes"],
        "rx_samples": len(rx_pcm),
        "tone_ratios": det,
        "detect_hz": args.detect,
    }
    _write(args, result, exit_code=0)
    print(
        f"RESULT registered=True invite_count={state['invite_count']} rx_voice={state['rx_voice']} "
        f"rx_bytes={state['rx_bytes']} rx_samples={len(rx_pcm)} tone_ratios={det}"
    )
    return 0


def _write(args, result, exit_code):
    tmp = args.result_file + ".tmp"
    with open(tmp, "w") as f:
        json.dump(result, f)
    os.replace(tmp, args.result_file)


if __name__ == "__main__":
    sys.exit(main())
