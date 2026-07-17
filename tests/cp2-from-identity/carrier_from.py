#!/usr/bin/env python3
"""Minimal offline carrier for the CP2 from-identity harness (stdlib only).

Captures the outbound INVITE's From header RECEIVER-SIDE and answers the call
(100 -> 200 with SDP -> consumes ACK/BYE) so the leg completes cleanly. The
harness asserts the captured From carries the endpoint's configured
from_user@from_domain — never a rustisk TX log.
"""

import argparse
import re
import socket
import sys


SIP_PORT = 5060


def log(msg):
    sys.stderr.write("[carrier_from] " + msg + "\n")
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


def append_capture(path, line):
    with open(path, "a") as f:
        f.write(line + "\n")
        f.flush()
    log(line)


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
        "t=0 0\r\nm=audio 40000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n"
    ) % (own, own)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--caller", required=True)
    ap.add_argument("--capture", required=True)
    args = ap.parse_args()

    own = own_ip_toward(args.caller)
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind(("0.0.0.0", SIP_PORT))
    append_capture(args.capture, "READY own=%s" % own)

    while True:
        try:
            sock.settimeout(0.5)
            data, src = sock.recvfrom(8192)
        except socket.timeout:
            continue
        except OSError:
            break
        text = data.decode("utf-8", "replace")
        first = text.split("\r\n", 1)[0]
        if first.startswith("INVITE "):
            frm = get_header(text, "From") or "?"
            # Extract the bare URI (sip:user@domain) from the From value.
            m = re.search(r'<([^>]+)>', frm)
            uri = m.group(1) if m else frm
            append_capture(args.capture, "INVITE-FROM own=%s from_uri=%s raw_from=%s" % (own, uri, frm))
            sock.sendto(build_response(text, 100, "Trying", own), src)
            sock.sendto(build_response(text, 200, "OK", own, to_tag="cp2from200", sdp=carrier_sdp(own)), src)
        elif first.startswith("BYE "):
            append_capture(args.capture, "BYE own=%s" % own)
            sock.sendto(build_response(text, 200, "OK", own), src)
        # ACK: nothing to answer.


if __name__ == "__main__":
    main()
