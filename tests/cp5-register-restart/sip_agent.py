#!/usr/bin/env python3
"""Minimal UDP SIP agent for the CP5 container-restart REGISTER harness.

Two roles, one script (stdlib only, runs in the pinned python image):

  --role bridge    Digest-REGISTERs to rustisk advertising THIS container's IP
                   (401 -> digest -> 200), then serves: every inbound INVITE
                   datagram is captured RECEIVER-SIDE (own arrival, not a
                   rustisk TX log) to --capture, and answered 100 -> 486 so the
                   rustisk INVITE client transaction terminates cleanly.

  --role sentinel  Does NOT register. Binds the (now-vacated) address A that a
                   restarted bridge used to hold, and captures ANY INVITE that
                   still arrives there. A datagram here after the bridge moved
                   to B is a stale-route hit.

The agent derives its own routable IP by route-selecting toward the registrar
(a connected UDP socket performs selection without sending a packet), so the
harness never hardcodes the Docker-assigned address.
"""

import argparse
import hashlib
import os
import random
import re
import socket
import sys
import time

SIP_PORT = 5060


def log(msg):
    sys.stderr.write("[sip_agent] " + msg + "\n")
    sys.stderr.flush()


def own_ip_toward(registrar_ip):
    """Return the local IP the kernel would use to reach the registrar."""
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect((registrar_ip, 15060))
        return s.getsockname()[0]
    finally:
        s.close()


def rand_hex(n=16):
    return "".join(random.choice("0123456789abcdef") for _ in range(n))


def md5_hex(s):
    return hashlib.md5(s.encode("utf-8")).hexdigest()


def parse_challenge(www):
    """Parse a Digest WWW-Authenticate value into a dict of params."""
    value = www.strip()
    if value.lower().startswith("digest"):
        value = value[len("digest"):].strip()
    params = {}
    for m in re.finditer(r'(\w+)\s*=\s*(?:"([^"]*)"|([^,]+))', value):
        key = m.group(1).lower()
        params[key] = (m.group(2) if m.group(2) is not None else m.group(3)).strip()
    return params


def build_authorization(username, password, method, uri, ch):
    realm = ch.get("realm", "")
    nonce = ch.get("nonce", "")
    qop = ch.get("qop")
    algorithm = ch.get("algorithm", "MD5")
    opaque = ch.get("opaque")
    ha1 = md5_hex("%s:%s:%s" % (username, realm, password))
    ha2 = md5_hex("%s:%s" % (method, uri))
    parts = [
        'username="%s"' % username,
        'realm="%s"' % realm,
        'nonce="%s"' % nonce,
        'uri="%s"' % uri,
    ]
    if qop and "auth" in qop:
        cnonce = rand_hex()
        nc = "00000001"
        response = md5_hex("%s:%s:%s:%s:auth:%s" % (ha1, nonce, nc, cnonce, ha2))
        parts.append('response="%s"' % response)
        parts.append("algorithm=%s" % algorithm)
        parts.append("qop=auth")
        parts.append("nc=%s" % nc)
        parts.append('cnonce="%s"' % cnonce)
    else:
        response = md5_hex("%s:%s:%s" % (ha1, nonce, ha2))
        parts.append('response="%s"' % response)
        parts.append("algorithm=%s" % algorithm)
    if opaque:
        parts.append('opaque="%s"' % opaque)
    return "Digest " + ", ".join(parts)


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
    vals = get_headers(text, name)
    return vals[0] if vals else None


def build_register(aor, registrar_ip, own, call_id, cseq, authorization=None):
    reg_uri = "sip:%s" % registrar_ip
    lines = [
        "REGISTER %s SIP/2.0" % reg_uri,
        "Via: SIP/2.0/UDP %s:%d;rport;branch=z9hG4bK%s" % (own, SIP_PORT, rand_hex(12)),
        "Max-Forwards: 70",
        "From: <sip:%s@%s>;tag=%s" % (aor, registrar_ip, rand_hex(8)),
        "To: <sip:%s@%s>" % (aor, registrar_ip),
        "Call-ID: %s" % call_id,
        "CSeq: %d REGISTER" % cseq,
        "Contact: <sip:%s@%s:%d>" % (aor, own, SIP_PORT),
    ]
    if authorization:
        lines.append("Authorization: %s" % authorization)
    lines.append("Expires: 3600")
    lines.append("Content-Length: 0")
    lines.append("")
    lines.append("")
    return ("\r\n".join(lines)).encode("utf-8")


def status_of(text):
    first = text.split("\r\n", 1)[0]
    m = re.match(r"SIP/2\.0\s+(\d{3})", first)
    return int(m.group(1)) if m else None


def do_register(sock, aor, registrar_ip, username, password, own, status_path):
    """Full digest REGISTER (401 -> digest -> 200), retrying until rustisk is up."""
    reg_uri = "sip:%s" % registrar_ip
    dst = (registrar_ip, 15060)
    deadline = time.time() + 90
    attempt = 0
    while time.time() < deadline:
        attempt += 1
        call_id = "%s-%d" % (rand_hex(10), attempt)
        # 1. unauthenticated REGISTER -> expect 401
        sock.sendto(build_register(aor, registrar_ip, own, call_id, 1), dst)
        ch = None
        t = time.time() + 2
        while time.time() < t:
            try:
                sock.settimeout(1.0)
                data, _src = sock.recvfrom(8192)
            except socket.timeout:
                continue
            text = data.decode("utf-8", "replace")
            st = status_of(text)
            if st == 401:
                www = get_header(text, "WWW-Authenticate")
                if www:
                    ch = parse_challenge(www)
                break
            if st == 200:
                # already bound (rare); treat as success
                _mark_registered(status_path, own)
                return True
        if ch is None:
            time.sleep(1.0)
            continue
        # 2. authenticated REGISTER -> expect 200
        auth = build_authorization(username, password, "REGISTER", reg_uri, ch)
        sock.sendto(build_register(aor, registrar_ip, own, call_id, 2, auth), dst)
        t = time.time() + 3
        while time.time() < t:
            try:
                sock.settimeout(1.0)
                data, _src = sock.recvfrom(8192)
            except socket.timeout:
                continue
            text = data.decode("utf-8", "replace")
            st = status_of(text)
            if st == 200:
                _mark_registered(status_path, own)
                log("REGISTERED aor=%s own=%s (attempt %d)" % (aor, own, attempt))
                return True
            if st in (401, 403):
                log("REGISTER rejected status=%s (attempt %d)" % (st, attempt))
                break
        time.sleep(1.0)
    return False


def _mark_registered(status_path, own):
    if status_path:
        with open(status_path, "a") as f:
            f.write("REGISTERED own=%s ts=%.3f\n" % (own, time.time()))
            f.flush()


def build_response(req_text, code, reason, own):
    """Build a SIP response echoing dialog-identifying headers from req_text."""
    vias = get_headers(req_text, "Via")
    frm = get_header(req_text, "From") or ""
    to = get_header(req_text, "To") or ""
    call_id = get_header(req_text, "Call-ID") or ""
    cseq = get_header(req_text, "CSeq") or ""
    if "tag=" not in to:
        to = to + ";tag=%s" % rand_hex(8)
    lines = ["SIP/2.0 %d %s" % (code, reason)]
    for v in vias:
        lines.append("Via: %s" % v)
    lines.append("From: %s" % frm)
    lines.append("To: %s" % to)
    lines.append("Call-ID: %s" % call_id)
    lines.append("CSeq: %s" % cseq)
    lines.append("Content-Length: 0")
    lines.append("")
    lines.append("")
    return ("\r\n".join(lines)).encode("utf-8")


def serve(sock, own, role, capture_path):
    """Capture inbound INVITEs receiver-side; answer 100 then 486."""
    tag = "INVITE" if role == "bridge" else "STRAY_INVITE"
    while True:
        try:
            sock.settimeout(1.0)
            data, src = sock.recvfrom(8192)
        except socket.timeout:
            continue
        except OSError:
            break
        text = data.decode("utf-8", "replace")
        first = text.split("\r\n", 1)[0]
        if first.startswith("INVITE "):
            call_id = get_header(text, "Call-ID") or "?"
            ruri = first.split(" ", 2)[1] if len(first.split(" ")) >= 2 else "?"
            line = "%s role=%s own=%s src=%s:%d ruri=%s callid=%s ts=%.3f\n" % (
                tag, role, own, src[0], src[1], ruri, call_id, time.time())
            with open(capture_path, "a") as f:
                f.write(line)
                f.flush()
            log(line.strip())
            # Answer so the client INVITE transaction terminates cleanly.
            sock.sendto(build_response(text, 100, "Trying", own), src)
            sock.sendto(build_response(text, 486, "Busy Here", own), src)
        elif first.startswith("ACK ") or first.startswith("BYE ") or first.startswith("CANCEL "):
            if first.startswith("BYE ") or first.startswith("CANCEL "):
                sock.sendto(build_response(text, 200, "OK", own), src)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--role", choices=["bridge", "sentinel"], required=True)
    ap.add_argument("--registrar", required=True, help="rustisk container IP")
    ap.add_argument("--aor", default="bridge")
    ap.add_argument("--user", default="bridge")
    ap.add_argument("--password", default="bridgepass")
    ap.add_argument("--capture", required=True)
    ap.add_argument("--status", default="")
    args = ap.parse_args()

    own = own_ip_toward(args.registrar)
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind(("0.0.0.0", SIP_PORT))
    log("role=%s own=%s registrar=%s" % (args.role, own, args.registrar))

    if args.role == "bridge":
        ok = do_register(sock, args.aor, args.registrar, args.user, args.password,
                         own, args.status)
        if not ok:
            log("FATAL: registration never succeeded")
            sys.exit(1)
    else:
        # sentinel: announce readiness on the vacated address
        _mark_registered(args.status, own) if args.status else None
        log("SENTINEL holding own=%s" % own)

    serve(sock, own, args.role, args.capture)


if __name__ == "__main__":
    main()
