#!/usr/bin/env python3
"""Synthetic Chime-dialect SIP UAC for the M9 dry-run.

Emulates the Chime inbound leg (per REPORT-M0a): PCMU/PT0 + telephone-event/PT101,
ptime 20 / 50pps, no 100rel, no session timers. Places a call into rustisk's
PIN-gate, enters a PIN via RFC2833 (in-band RTP) DTMF, emits a continuous PCMU
tone (so once GRANTED+bridged the tone reaches qa-sip), and captures the return
RTP (the bridged audio coming back FROM qa-sip). Prints receiver-side counters
and writes the decoded RX audio to a WAV for tone analysis.

Pure stdlib + numpy. No repo imports (helpers copied from rtp_injector.py).
"""
import argparse, math, os, random, socket, struct, sys, threading, time, wave
import numpy as np

def log(*a): print("[caller]", *a, file=sys.stderr, flush=True)

# ---- mulaw + RTP (copied verbatim from tests/.../rtp_injector.py) ----
def linear_to_mulaw(sample):
    bias=0x84; clip=32635
    sign=0x80 if sample<0 else 0
    magnitude=min(abs(sample),clip)+bias
    exponent=7; mask=0x4000
    while exponent>0 and not magnitude&mask:
        exponent-=1; mask>>=1
    mantissa=(magnitude>>(exponent+3))&0x0F
    return (~(sign|(exponent<<4)|mantissa))&0xFF

_MULAW_DECODE=None
def mulaw_to_linear(u):
    global _MULAW_DECODE
    if _MULAW_DECODE is None:
        tab=[]
        for i in range(256):
            u2=~i&0xFF
            sign=u2&0x80; exp=(u2>>4)&0x07; man=u2&0x0F
            mag=((man<<3)+0x84)<<exp
            mag-=0x84
            tab.append(-mag if sign else mag)
        _MULAW_DECODE=tab
    return _MULAW_DECODE[u&0xFF]

def tone_pcmu(freq, packet_index, amp=10000):
    out=bytearray()
    for offset in range(160):
        absolute=packet_index*160+offset
        s=round(amp*math.sin(2*math.pi*freq*absolute/8000))
        out.append(linear_to_mulaw(s))
    return bytes(out)

def rtp_hdr(pt, seq, ts, ssrc, marker=0):
    return struct.pack("!BBHII", 0x80, ((marker&1)<<7)|(pt&0x7F), seq&0xFFFF, ts&0xFFFFFFFF, ssrc&0xFFFFFFFF)

# ---- minimal SIP ----
def parse_sdp_pts(body):
    """Return (pcmu_pt, tev_pt, remote_ip, remote_port) from an SDP answer."""
    pcmu=0; tev=101; rip=None; rport=None
    for line in body.splitlines():
        line=line.strip()
        if line.startswith("m=audio"):
            parts=line.split()
            rport=int(parts[1])
        elif line.startswith("c=IN IP4"):
            rip=line.split()[-1]
        elif line.lower().startswith("a=rtpmap:"):
            rest=line[len("a=rtpmap:"):]
            pt,enc=rest.split(None,1)
            if enc.upper().startswith("PCMU/"): pcmu=int(pt)
            if enc.lower().startswith("telephone-event/"): tev=int(pt)
    return pcmu, tev, rip, rport

def main():
    ap=argparse.ArgumentParser()
    ap.add_argument("--dst-ip", required=True)
    ap.add_argument("--dst-port", type=int, required=True)
    ap.add_argument("--src-ip", required=True)
    ap.add_argument("--sip-port", type=int, default=25002)
    ap.add_argument("--rtp-port", type=int, default=40002)
    ap.add_argument("--exten", default="9000")
    ap.add_argument("--pin", required=True)
    ap.add_argument("--tone", type=int, default=440)     # A->qa-sip tone
    ap.add_argument("--call-secs", type=float, default=12.0)
    ap.add_argument("--dtmf-start", type=float, default=1.2)  # after 200 OK
    ap.add_argument("--rx-wav", required=True)
    args=ap.parse_args()

    ssrc=random.randint(0,0x7fffffff)
    callid=f"{random.randint(0,1<<48):012x}@{args.src_ip}"
    fromtag=f"{random.randint(0,1<<32):08x}"
    branch=f"z9hG4bK{random.randint(0,1<<32):08x}"
    cseq=1

    sipsock=socket.socket(socket.AF_INET,socket.SOCK_DGRAM)
    sipsock.bind((args.src_ip,args.sip_port)); sipsock.settimeout(5)
    rtpsock=socket.socket(socket.AF_INET,socket.SOCK_DGRAM)
    rtpsock.bind((args.src_ip,args.rtp_port)); rtpsock.settimeout(0.25)

    sdp=(f"v=0\r\no=- {random.randint(1,1<<30)} 1 IN IP4 {args.src_ip}\r\ns=chime\r\n"
         f"c=IN IP4 {args.src_ip}\r\nt=0 0\r\nm=audio {args.rtp_port} RTP/AVP 0 101\r\n"
         f"a=rtpmap:0 PCMU/8000\r\na=rtpmap:101 telephone-event/8000\r\n"
         f"a=fmtp:101 0-15\r\na=ptime:20\r\na=sendrecv\r\n")
    ruri=f"sip:{args.exten}@{args.dst_ip}:{args.dst_port}"
    invite=(f"INVITE {ruri} SIP/2.0\r\n"
            f"Via: SIP/2.0/UDP {args.src_ip}:{args.sip_port};branch={branch};rport\r\n"
            f"Max-Forwards: 70\r\n"
            f"From: <sip:chime@{args.src_ip}>;tag={fromtag}\r\n"
            f"To: <{ruri}>\r\n"
            f"Call-ID: {callid}\r\nCSeq: {cseq} INVITE\r\n"
            f"Contact: <sip:chime@{args.src_ip}:{args.sip_port}>\r\n"
            f"Content-Type: application/sdp\r\nContent-Length: {len(sdp)}\r\n\r\n{sdp}")
    sipsock.sendto(invite.encode(),(args.dst_ip,args.dst_port))
    log(f"INVITE -> {ruri}")

    totag=None; ok_body=None; deadline=time.time()+8
    while time.time()<deadline:
        try: data,_=sipsock.recvfrom(65535)
        except socket.timeout:
            sipsock.sendto(invite.encode(),(args.dst_ip,args.dst_port)); continue
        msg=data.decode("latin1"); first=msg.split("\r\n",1)[0]
        log("SIP <=",first)
        if " 200 " in first or first.endswith(" 200 OK"):
            for l in msg.split("\r\n"):
                if l.lower().startswith("to:") and "tag=" in l.lower():
                    totag=l.split("tag=",1)[1].strip()
            ok_body=msg.split("\r\n\r\n",1)[1] if "\r\n\r\n" in msg else ""
            break
        # 100/180/183 -> keep waiting
    if ok_body is None:
        log("no 200 OK; aborting"); return 2

    pcmu_pt,tev_pt,rip,rport=parse_sdp_pts(ok_body)
    if not rip or rip=="0.0.0.0": rip=args.dst_ip
    log(f"answered: PCMU_PT={pcmu_pt} TEV_PT={tev_pt} remote_media={rip}:{rport}")
    # ACK
    ack=(f"ACK {ruri} SIP/2.0\r\n"
         f"Via: SIP/2.0/UDP {args.src_ip}:{args.sip_port};branch=z9hG4bK{random.randint(0,1<<32):08x};rport\r\n"
         f"Max-Forwards: 70\r\nFrom: <sip:chime@{args.src_ip}>;tag={fromtag}\r\n"
         f"To: <{ruri}>;tag={totag}\r\nCall-ID: {callid}\r\nCSeq: {cseq} ACK\r\nContent-Length: 0\r\n\r\n")
    sipsock.sendto(ack.encode(),(args.dst_ip,args.dst_port))

    # ---- media threads ----
    stop=threading.Event()
    rx_frames={"voice":0,"other":0,"bytes":0}
    rx_pcm=[]
    def rx_loop():
        while not stop.is_set():
            try: pkt,_=rtpsock.recvfrom(4096)
            except socket.timeout: continue
            if len(pkt)<12: continue
            pt=pkt[1]&0x7f; rx_frames["bytes"]+=len(pkt)
            payload=pkt[12:]
            if pt==pcmu_pt and len(payload)>0:
                rx_frames["voice"]+=1
                rx_pcm.extend(mulaw_to_linear(b) for b in payload)
            else:
                rx_frames["other"]+=1
    threading.Thread(target=rx_loop,daemon=True).start()

    seq=random.randint(0,0x7fff); ts=random.randint(0,0x7fffffff)
    t0=time.time(); dtmf_sent=False
    def send_voice_packet(idx):
        nonlocal seq,ts
        pkt=rtp_hdr(pcmu_pt,seq,ts,ssrc)+tone_pcmu(args.tone,idx)
        rtpsock.sendto(pkt,(rip,rport)); seq=(seq+1)&0xffff; ts=(ts+160)&0xffffffff
    def send_dtmf(digit):
        nonlocal seq,ts
        evmap={**{str(d):d for d in range(10)},"*":10,"#":11,"A":12,"B":13,"C":14,"D":15}
        ev=evmap[digit]; ev_ts=ts; dur=0
        # 8 event packets (~160ms), marker on first
        for i in range(8):
            dur+=160
            payload=struct.pack("!BBH",ev,(0<<7)|10,dur)  # E=0, vol=10
            pkt=rtp_hdr(tev_pt,seq,ev_ts,ssrc,marker=1 if i==0 else 0)+payload
            rtpsock.sendto(pkt,(rip,rport)); seq=(seq+1)&0xffff; time.sleep(0.02)
        # 3 end packets, E=1
        for _ in range(3):
            payload=struct.pack("!BBH",ev,(1<<7)|10,dur)
            pkt=rtp_hdr(tev_pt,seq,ev_ts,ssrc)+pkt[12:] if False else rtp_hdr(tev_pt,seq,ev_ts,ssrc)+payload
            rtpsock.sendto(pkt,(rip,rport)); seq=(seq+1)&0xffff; time.sleep(0.02)
        ts=(ts+160*11)&0xffffffff  # advance past the event duration

    idx=0
    while time.time()-t0 < args.call_secs:
        cyc=time.time()-t0
        if (not dtmf_sent) and cyc>=args.dtmf_start:
            log(f"sending PIN via RFC2833 (TEV_PT={tev_pt})")
            for d in args.pin:
                send_dtmf(d); time.sleep(0.06)   # inter-digit gap
            # terminate entry
            send_dtmf("#")
            dtmf_sent=True
            continue
        send_voice_packet(idx); idx+=1
        time.sleep(0.02)

    stop.set(); time.sleep(0.3)
    # BYE
    bye=(f"BYE {ruri} SIP/2.0\r\n"
         f"Via: SIP/2.0/UDP {args.src_ip}:{args.sip_port};branch=z9hG4bK{random.randint(0,1<<32):08x};rport\r\n"
         f"Max-Forwards: 70\r\nFrom: <sip:chime@{args.src_ip}>;tag={fromtag}\r\n"
         f"To: <{ruri}>;tag={totag}\r\nCall-ID: {callid}\r\nCSeq: {cseq+1} BYE\r\nContent-Length: 0\r\n\r\n")
    sipsock.sendto(bye.encode(),(args.dst_ip,args.dst_port))

    # write RX wav (8k mono s16)
    pcm=np.array(rx_pcm,dtype=np.int16) if rx_pcm else np.zeros(0,dtype=np.int16)
    with wave.open(args.rx_wav,"wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(8000); w.writeframes(pcm.tobytes())
    # tone detect on RX
    det={}
    if len(pcm)>=1600:
        x=pcm.astype(np.float64); x-=x.mean()
        N=len(x); f=np.fft.rfftfreq(N,1/8000); X=np.abs(np.fft.rfft(x*np.hanning(N)))
        def band(f0):
            m=(f>=f0-30)&(f<=f0+30); return float(X[m].max()) if m.any() else 0.0
        tot=float(X.sum())+1e-9
        for f0 in (440,660,350,480):
            det[f0]=round(band(f0)/tot*1e4,2)
    print(f"RESULT voice_rx={rx_frames['voice']} other_rx={rx_frames['other']} "
          f"rx_bytes={rx_frames['bytes']} rx_samples={len(pcm)} tone_ratios={det}")
    return 0

if __name__=="__main__":
    sys.exit(main())
