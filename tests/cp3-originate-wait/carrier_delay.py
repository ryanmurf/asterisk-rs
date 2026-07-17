#!/usr/bin/env python3
"""Offline carrier that DELAYS its 200 OK, for the CP3 wait-for-answer harness.

Proves RECEIVER-SIDE that an AMI Originate does not run the dialplan app until
the far end ANSWERS: the carrier holds its 200 for --answer-delay seconds and
tags every captured datagram `phase=pre` (before it sent the 200) or
`phase=post`. A wait-for-answer-correct rustisk sends NOTHING but the INVITE in
the pre-answer window (no ACK/BYE/CANCEL, no RTP) and only after the 200 does the
app run — so the ACK, the app's DTMF RTP, and the BYE are all `phase=post`.

RED (app run immediately, before answer): the app runs and tears the unanswered
leg down before the delayed 200; the 200 is never ACKed and no post-answer BYE
appears.

--early-media (M7 follow-up, WIRE-MINOR-1): the carrier additionally sends a
183 Session Progress WITH SDP immediately (before its delayed 200), so a
pre-answer media path EXISTS — the caller knows the carrier's RTP address from
the 183. This gives the `RTP phase=pre == 0` guard independent teeth: if the
Originate wait were defeated so the app ran on the 183, the app's DTMF RTP
would arrive pre-answer and that guard alone REDs (instead of the defeat only
ever surfacing as an orphaned-200). The 183 and the 200 carry the SAME To tag
(one early dialog, confirmed by the 200).

Captures SIP on :5060 and RTP on :40000 (the SDP-answered media port).
"""

import argparse
import re
import select
import socket
import sys
import time

SIP_PORT = 5060
RTP_PORT = 40000


def log(msg):
    sys.stderr.write("[carrier_delay] " + msg + "\n")
    sys.stderr.flush()


def own_ip_toward(peer_ip):
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect((peer_ip, 15060))
        return s.getsockname()[0]
    finally:
        s.close()


def get_headers(text, name):
    out = []
    for line in text.split("\r\n"):
        if line == "":
            break
        if ":" in line:
            hn, hv = line.split(":", 1)
            if hn.strip().lower() == name.lower():
                out.append(hv.strip())
    return out


def get_header(text, name):
    v = get_headers(text, name)
    return v[0] if v else None


def cseq_of(text):
    cs = get_header(text, "CSeq")
    return cs.strip() if cs else None


def build_response(req_text, code, reason, own, to_tag=None, sdp=None):
    vias = get_headers(req_text, "Via")
    frm = get_header(req_text, "From") or ""
    to = get_header(req_text, "To") or ""
    call_id = get_header(req_text, "Call-ID") or ""
    cseq = get_header(req_text, "CSeq") or ""
    if to_tag and "tag=" not in to:
        to = to + ";tag=%s" % to_tag
    lines = ["SIP/2.0 %d %s" % (code, reason)]
    for v in vias:
        lines.append("Via: %s" % v)
    lines.append("From: %s" % frm)
    lines.append("To: %s" % to)
    lines.append("Call-ID: %s" % call_id)
    lines.append("CSeq: %s" % cseq)
    lines.append("Contact: <sip:carrier@%s:%d>" % (own, SIP_PORT))
    body = sdp or ""
    if body:
        lines.append("Content-Type: application/sdp")
    lines.append("Content-Length: %d" % len(body))
    lines.append("")
    lines.append(body)
    return ("\r\n".join(lines)).encode("utf-8")


def carrier_sdp(own):
    return (
        "v=0\r\no=carrier 0 0 IN IP4 %s\r\ns=-\r\nc=IN IP4 %s\r\n"
        "t=0 0\r\nm=audio %d RTP/AVP 0 101\r\na=rtpmap:0 PCMU/8000\r\n"
        "a=rtpmap:101 telephone-event/8000\r\na=fmtp:101 0-16\r\n"
    ) % (own, own, RTP_PORT)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--caller", required=True)
    ap.add_argument("--capture", required=True)
    ap.add_argument("--answer-delay", type=float, default=3.0)
    ap.add_argument("--early-media", action="store_true",
                    help="send 183 Session Progress WITH SDP before the delayed 200")
    args = ap.parse_args()

    own = own_ip_toward(args.caller)
    t0 = time.time()

    # In early-media mode the 183 opens an early dialog; the 200 (and a 487 on
    # CANCEL) must carry the SAME To tag or the caller rightly treats the final
    # as a stray from a different dialog.
    tag_200 = "cp3del183" if args.early_media else "cp3del200"
    tag_487 = "cp3del183" if args.early_media else "cp3del487"

    def cap(line):
        with open(args.capture, "a") as f:
            f.write(line + "\n")
            f.flush()
        log(line)

    sip = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sip.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sip.bind(("0.0.0.0", SIP_PORT))
    rtp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    rtp.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    rtp.bind(("0.0.0.0", RTP_PORT))

    cap("READY own=%s answer_delay=%.2f rel=%.3f" % (own, args.answer_delay, 0.0))

    answered = False            # global (single-call harness)
    pending_200 = None          # (send_at, invite_text, src)

    def phase():
        return "post" if answered else "pre"

    while True:
        # Fire the delayed 200 if due.
        if pending_200 is not None and time.time() >= pending_200[0]:
            send_at, inv, src = pending_200
            pending_200 = None
            ok = build_response(inv, 200, "OK", own, to_tag=tag_200, sdp=carrier_sdp(own))
            sip.sendto(ok, src)
            answered = True
            cap("SENT-200 own=%s rel=%.3f" % (own, time.time() - t0))

        timeout = 0.2
        if pending_200 is not None:
            timeout = max(0.01, min(0.2, pending_200[0] - time.time()))
        r, _, _ = select.select([sip, rtp], [], [], timeout)
        for s in r:
            try:
                data, src = s.recvfrom(8192)
            except OSError:
                continue
            rel = time.time() - t0
            if s is rtp:
                cap("RTP phase=%s own=%s src=%s:%d bytes=%d rel=%.3f" % (
                    phase(), own, src[0], src[1], len(data), rel))
                continue
            text = data.decode("utf-8", "replace")
            first = text.split("\r\n", 1)[0]
            if first.startswith("INVITE "):
                cap("INVITE phase=%s own=%s src=%s:%d cseq=%s rel=%.3f" % (
                    phase(), own, src[0], src[1], cseq_of(text), rel))
                # 100 Trying immediately; 200 OK is DELAYED.
                sip.sendto(build_response(text, 100, "Trying", own), src)
                if args.early_media and not answered:
                    # Early media: 183 WITH SDP before the delayed 200 — the
                    # pre-answer media path now exists (RTP-guard teeth).
                    sip.sendto(build_response(text, 183, "Session Progress", own,
                                              to_tag=tag_200, sdp=carrier_sdp(own)), src)
                    cap("SENT-183 own=%s rel=%.3f" % (own, time.time() - t0))
                if pending_200 is None and not answered:
                    pending_200 = (time.time() + args.answer_delay, text, src)
            elif first.startswith("ACK "):
                cap("ACK phase=%s own=%s src=%s:%d cseq=%s rel=%.3f" % (
                    phase(), own, src[0], src[1], cseq_of(text), rel))
            elif first.startswith("BYE "):
                cap("BYE phase=%s own=%s src=%s:%d cseq=%s rel=%.3f" % (
                    phase(), own, src[0], src[1], cseq_of(text), rel))
                sip.sendto(build_response(text, 200, "OK", own), src)
            elif first.startswith("CANCEL "):
                cap("CANCEL phase=%s own=%s src=%s:%d cseq=%s rel=%.3f" % (
                    phase(), own, src[0], src[1], cseq_of(text), rel))
                sip.sendto(build_response(text, 200, "OK", own), src)
                # RFC 3261: also 487 the INVITE and cancel the pending 200.
                if pending_200 is not None:
                    _, inv, isrc = pending_200
                    pending_200 = None
                    sip.sendto(build_response(inv, 487, "Request Terminated", own, to_tag=tag_487), isrc)


if __name__ == "__main__":
    main()
