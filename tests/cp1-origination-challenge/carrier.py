#!/usr/bin/env python3
"""Offline carrier UAS for the CP1 wire-correct-challenge harness (stdlib only).

Two roles, one script, each on its OWN container IP so the harness can prove —
RECEIVER-SIDE — that in-dialog requests land on the route-set target and NOT on
the original request-URI:

  --role core     The INVITE target / signalling peer. On the first INVITE (no
                  Authorization) it CHALLENGES with 401 + WWW-Authenticate and
                  captures the ACK the caller sends for that final (RFC 3261
                  §17.1.1.3 — same branch). If no ACK arrives it RETRANSMITS the
                  401 and finally records NO-ACK (the RED for "challenge not
                  ACKed"). On the credentialed retry INVITE it validates the
                  digest and captures the branch + CSeq + auth result, then
                  answers 100 -> 200 OK with a CHANGED Contact and a
                  Record-Route, both pointing at the route target T. Everything
                  it sees is written to --capture receiver-side.

  --role target   Holds address T (the Contact / Record-Route target). It sends
                  no INVITE and challenges nothing; it only captures the 2xx ACK
                  and the in-dialog BYE that a route-set-correct caller delivers
                  here, and answers the BYE 200 so the client transaction
                  settles. A datagram here proves route-set/Contact targeting;
                  the same datagram arriving at `core` instead is the RED.

The agent derives its own routable IP by route-selecting toward the caller
(rustisk), so the harness never hardcodes the Docker-assigned address.
"""

import argparse
import hashlib
import re
import socket
import sys
import time

SIP_PORT = 5060


def log(msg):
    sys.stderr.write("[carrier] " + msg + "\n")
    sys.stderr.flush()


def md5_hex(s):
    return hashlib.md5(s.encode("utf-8")).hexdigest()


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
    vals = get_headers(text, name)
    return vals[0] if vals else None


def branch_of(text):
    via = get_header(text, "Via")
    if not via:
        return None
    for part in via.split(";"):
        part = part.strip()
        if part.startswith("branch="):
            return part[len("branch="):].strip()
    return None


def cseq_of(text):
    cs = get_header(text, "CSeq")
    return cs.strip() if cs else None


def request_uri(text):
    first = text.split("\r\n", 1)[0]
    toks = first.split(" ")
    return toks[1] if len(toks) >= 2 else "?"


def parse_authorization(value):
    value = value.strip()
    if value.lower().startswith("digest"):
        value = value[len("digest"):].strip()
    params = {}
    for m in re.finditer(r'(\w+)\s*=\s*(?:"([^"]*)"|([^,]+))', value):
        key = m.group(1).lower()
        params[key] = (m.group(2) if m.group(2) is not None else m.group(3)).strip()
    return params


def digest_valid(auth_params, method, password):
    """Recompute the digest response server-side and compare (qop=auth + RFC 2069)."""
    try:
        username = auth_params["username"]
        realm = auth_params.get("realm", "")
        nonce = auth_params["nonce"]
        uri = auth_params["uri"]
        given = auth_params["response"]
    except KeyError:
        return False
    ha1 = md5_hex("%s:%s:%s" % (username, realm, password))
    ha2 = md5_hex("%s:%s" % (method, uri))
    qop = auth_params.get("qop")
    if qop and "auth" in qop:
        cnonce = auth_params.get("cnonce")
        nc = auth_params.get("nc")
        if not cnonce or not nc:
            return False
        expected = md5_hex("%s:%s:%s:%s:auth:%s" % (ha1, nonce, nc, cnonce, ha2))
    else:
        expected = md5_hex("%s:%s:%s" % (ha1, nonce, ha2))
    return expected == given


def append_capture(path, line):
    with open(path, "a") as f:
        f.write(line + "\n")
        f.flush()
    log(line)


def build_response(req_text, code, reason, extra_headers=None, to_tag=None, sdp=None):
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
    for h in extra_headers or []:
        lines.append(h)
    body = sdp or ""
    if body:
        lines.append("Content-Type: application/sdp")
    lines.append("Content-Length: %d" % len(body))
    lines.append("")
    lines.append(body)
    return ("\r\n".join(lines)).encode("utf-8")


def carrier_sdp(own):
    """Minimal G.711u answer so the caller's outbound answer applies cleanly."""
    return (
        "v=0\r\n"
        "o=carrier 0 0 IN IP4 %s\r\n"
        "s=-\r\n"
        "c=IN IP4 %s\r\n"
        "t=0 0\r\n"
        "m=audio 40000 RTP/AVP 0\r\n"
        "a=rtpmap:0 PCMU/8000\r\n"
    ) % (own, own)


def serve_core(sock, own, capture, route_target, realm, password, answer_delay):
    """Challenge -> capture ACK -> validate retry -> 200 with changed Contact + RR."""
    nonce = "cp1nonce" + hashlib.md5(own.encode()).hexdigest()[:8]
    # Per-Call-ID state: challenged INVITE branch + retransmit bookkeeping.
    challenged = {}   # call_id -> {"branch", "retx", "deadline", "acked"}
    answered = set()  # call_id already 200'd
    rejected = {}     # call_id -> set of branches given a non-2xx final (403)
    RETX_INTERVAL = 1.0
    MAX_RETX = 3
    while True:
        try:
            sock.settimeout(0.2)
            data, src = sock.recvfrom(8192)
        except socket.timeout:
            now = time.time()
            for cid, st in list(challenged.items()):
                if st["acked"]:
                    continue
                if now >= st["deadline"]:
                    if st["retx"] < MAX_RETX:
                        st["retx"] += 1
                        st["deadline"] = now + RETX_INTERVAL
                        sock.sendto(st["challenge_bytes"], st["src"])
                        append_capture(capture, "RETX-401 own=%s callid=%s n=%d" % (own, cid, st["retx"]))
                    else:
                        append_capture(capture, "NO-ACK own=%s callid=%s branch=%s (challenge never ACKed)" % (own, cid, st["branch"]))
                        st["acked"] = True  # stop reporting
            continue
        except OSError:
            break
        text = data.decode("utf-8", "replace")
        first = text.split("\r\n", 1)[0]
        call_id = get_header(text, "Call-ID") or "?"

        if first.startswith("INVITE "):
            auth = get_header(text, "Authorization") or get_header(text, "Proxy-Authorization")
            branch = branch_of(text)
            cseq = cseq_of(text)
            if not auth:
                # First INVITE -> challenge 401.
                append_capture(capture, "INVITE own=%s src=%s:%d branch=%s cseq=%s auth=no callid=%s" % (
                    own, src[0], src[1], branch, cseq, call_id))
                chal = ('Digest realm="%s", nonce="%s", algorithm=MD5, qop="auth"' % (realm, nonce))
                resp = build_response(text, 401, "Unauthorized",
                                      extra_headers=["WWW-Authenticate: %s" % chal],
                                      to_tag="cp1core401")
                sock.sendto(resp, src)
                challenged[call_id] = {
                    "branch": branch, "retx": 0, "deadline": time.time() + RETX_INTERVAL,
                    "acked": False, "challenge_bytes": resp, "src": src,
                }
            else:
                # Credentialed retry INVITE.
                params = parse_authorization(auth)
                valid = digest_valid(params, "INVITE", password)
                prev = challenged.get(call_id, {})
                append_capture(capture, "RETRY-INVITE own=%s src=%s:%d branch=%s cseq=%s auth=yes valid=%s prev_branch=%s callid=%s" % (
                    own, src[0], src[1], branch, cseq, "yes" if valid else "no", prev.get("branch"), call_id))
                if not valid:
                    sock.sendto(build_response(text, 403, "Forbidden", to_tag="cp1core403"), src)
                    # Remember this branch: its non-2xx ACK (same branch, RFC
                    # 3261 §17.1.1.3) correctly comes back to the core and must
                    # not be mislabeled as a 2xx-ACK route-set violation.
                    rejected.setdefault(call_id, set()).add(branch)
                    continue
                if call_id in answered:
                    continue
                answered.add(call_id)
                # 100 Trying, then 200 OK with CHANGED Contact + Record-Route at T.
                sock.sendto(build_response(text, 100, "Trying"), src)
                if answer_delay > 0:
                    time.sleep(answer_delay)
                contact = "Contact: <sip:carrier@%s:%d>" % (route_target, SIP_PORT)
                rr = "Record-Route: <sip:%s:%d;lr>" % (route_target, SIP_PORT)
                ok = build_response(text, 200, "OK", extra_headers=[rr, contact],
                                    to_tag="cp1core200", sdp=carrier_sdp(own))
                sock.sendto(ok, src)
                append_capture(capture, "SENT-200 own=%s contact=%s:%d record_route=%s:%d callid=%s" % (
                    own, route_target, SIP_PORT, route_target, SIP_PORT, call_id))
        elif first.startswith("ACK "):
            branch = branch_of(text)
            cseq = cseq_of(text)
            st = challenged.get(call_id)
            # Label by the SPECIFIC transaction the ACK belongs to (branch),
            # not a coarse challenged-or-not match — a non-2xx ACK reuses its
            # INVITE's branch (§17.1.1.3), while a 2xx ACK is a NEW transaction
            # with a fresh branch. Only the latter at the core is a route-set
            # violation (RED).
            if st and branch == st["branch"]:
                if not st.get("marked_ack"):
                    st["acked"] = True
                    st["marked_ack"] = True
                    append_capture(capture, "ACK-CHALLENGE own=%s src=%s:%d branch=%s cseq=%s callid=%s" % (
                        own, src[0], src[1], branch, cseq, call_id))
                else:
                    append_capture(capture, "ACK-CHALLENGE-RETX own=%s src=%s:%d branch=%s cseq=%s callid=%s" % (
                        own, src[0], src[1], branch, cseq, call_id))
            elif branch in rejected.get(call_id, set()):
                # The 403'd retry's ACK — hop-by-hop, correctly at the core.
                append_capture(capture, "ACK-NON2XX-AT-CORE own=%s src=%s:%d branch=%s cseq=%s callid=%s" % (
                    own, src[0], src[1], branch, cseq, call_id))
            else:
                # A 2xx ACK reaching CORE means the route set was IGNORED (RED).
                append_capture(capture, "ACK-2XX-AT-CORE own=%s src=%s:%d branch=%s cseq=%s callid=%s" % (
                    own, src[0], src[1], branch, cseq, call_id))
        elif first.startswith("BYE "):
            append_capture(capture, "BYE-AT-CORE own=%s src=%s:%d cseq=%s callid=%s" % (
                own, src[0], src[1], cseq_of(text), call_id))
            sock.sendto(build_response(text, 200, "OK"), src)
        elif first.startswith("CANCEL "):
            append_capture(capture, "CANCEL-AT-CORE own=%s src=%s:%d cseq=%s callid=%s" % (
                own, src[0], src[1], cseq_of(text), call_id))
            sock.sendto(build_response(text, 200, "OK"), src)


def serve_target(sock, own, capture):
    """Capture the 2xx ACK and the BYE that a route-set-correct caller delivers here."""
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
        call_id = get_header(text, "Call-ID") or "?"
        if first.startswith("ACK "):
            append_capture(capture, "ACK-2XX-AT-TARGET own=%s src=%s:%d branch=%s cseq=%s ruri=%s callid=%s" % (
                own, src[0], src[1], branch_of(text), cseq_of(text), request_uri(text), call_id))
        elif first.startswith("BYE "):
            append_capture(capture, "BYE-AT-TARGET own=%s src=%s:%d cseq=%s ruri=%s callid=%s" % (
                own, src[0], src[1], cseq_of(text), request_uri(text), call_id))
            sock.sendto(build_response(text, 200, "OK"), src)
        elif first.startswith("INVITE "):
            append_capture(capture, "STRAY-INVITE-AT-TARGET own=%s src=%s:%d callid=%s" % (
                own, src[0], src[1], call_id))
            sock.sendto(build_response(text, 200, "OK", to_tag="strayt"), src)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--role", choices=["core", "target"], required=True)
    ap.add_argument("--caller", required=True, help="rustisk container IP (for route selection)")
    ap.add_argument("--capture", required=True)
    ap.add_argument("--route-target", default="", help="core: the T address to advertise in Contact/Record-Route")
    ap.add_argument("--realm", default="carrier")
    ap.add_argument("--password", default="s3cr3t")
    ap.add_argument("--answer-delay", type=float, default=0.0)
    args = ap.parse_args()

    own = own_ip_toward(args.caller)
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind(("0.0.0.0", SIP_PORT))
    log("role=%s own=%s caller=%s route_target=%s" % (args.role, own, args.caller, args.route_target))
    # Readiness marker (first capture line) so the harness can gate on liveness.
    append_capture(args.capture, "READY role=%s own=%s" % (args.role, own))

    if args.role == "core":
        if not args.route_target:
            log("FATAL: core requires --route-target")
            sys.exit(2)
        serve_core(sock, own, args.capture, args.route_target, args.realm, args.password, args.answer_delay)
    else:
        serve_target(sock, own, args.capture)


if __name__ == "__main__":
    main()
