#!/usr/bin/env python3
"""Offline carrier that captures the advertised signalling host on EVERY request
of a full origination (INVITE, ACK, BYE) for the CP5 signalling-scope harness.

With `external_signaling_address` configured and the carrier outside `local_net`,
a scope-correct rustisk advertises the EXTERNAL address (not the raw `0.0.0.0`
bind) in Via/From/Contact on the core INVITE AND the in-dialog ACK/BYE. This
carrier records the host of those headers receiver-side so the harness can assert
no internal-bind leak on the origination path.
"""

import argparse
import re
import socket
import sys


SIP_PORT = 5060


def log(m):
    sys.stderr.write("[carrier_scope] " + m + "\n"); sys.stderr.flush()


def own_ip_toward(peer_ip):
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect((peer_ip, 15060)); return s.getsockname()[0]
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


def via_host(text):
    via = get_header(text, "Via") or ""
    m = re.search(r'SIP/2\.0/UDP\s+([^;\s]+)', via)
    return m.group(1) if m else "?"


def uri_host(header_value):
    if not header_value:
        return "?"
    m = re.search(r'<[a-z]+:[^@>]*@([^;>]+)', header_value)
    if m:
        return m.group(1)
    m = re.search(r'[a-z]+:[^@>]*@([^;>]+)', header_value)
    return m.group(1) if m else "?"


def cap(path, line):
    with open(path, "a") as f:
        f.write(line + "\n"); f.flush()
    log(line)


def build_response(req, code, reason, own, to_tag=None, sdp=None):
    vias = get_headers(req, "Via")
    frm = get_header(req, "From") or ""
    to = get_header(req, "To") or ""
    cid = get_header(req, "Call-ID") or ""
    cseq = get_header(req, "CSeq") or ""
    if to_tag and "tag=" not in to:
        to = to + ";tag=%s" % to_tag
    lines = ["SIP/2.0 %d %s" % (code, reason)]
    for v in vias:
        lines.append("Via: %s" % v)
    lines += ["From: %s" % frm, "To: %s" % to, "Call-ID: %s" % cid, "CSeq: %s" % cseq]
    lines.append("Contact: <sip:carrier@%s:%d>" % (own, SIP_PORT))
    body = sdp or ""
    if body:
        lines.append("Content-Type: application/sdp")
    lines.append("Content-Length: %d" % len(body))
    lines += ["", body]
    return ("\r\n".join(lines)).encode("utf-8")


def carrier_sdp(own):
    return ("v=0\r\no=carrier 0 0 IN IP4 %s\r\ns=-\r\nc=IN IP4 %s\r\n"
            "t=0 0\r\nm=audio 40000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n") % (own, own)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--caller", required=True)
    ap.add_argument("--capture", required=True)
    args = ap.parse_args()
    own = own_ip_toward(args.caller)
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("0.0.0.0", SIP_PORT))
    cap(args.capture, "READY own=%s" % own)
    while True:
        try:
            s.settimeout(0.5)
            data, src = s.recvfrom(8192)
        except socket.timeout:
            continue
        except OSError:
            break
        text = data.decode("utf-8", "replace")
        first = text.split("\r\n", 1)[0]
        method = first.split(" ", 1)[0]
        if method in ("INVITE", "ACK", "BYE"):
            cap(args.capture, "%s via_host=%s from_host=%s contact_host=%s src=%s:%d" % (
                method, via_host(text),
                uri_host(get_header(text, "From")),
                uri_host(get_header(text, "Contact")) if get_header(text, "Contact") else "-",
                src[0], src[1]))
        if method == "INVITE":
            s.sendto(build_response(text, 100, "Trying", own), src)
            s.sendto(build_response(text, 200, "OK", own, to_tag="cp5scope200", sdp=carrier_sdp(own)), src)
        elif method == "BYE":
            s.sendto(build_response(text, 200, "OK", own), src)


if __name__ == "__main__":
    main()
