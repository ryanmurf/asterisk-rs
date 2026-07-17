#!/usr/bin/env python3
"""M8a numbered-datagram sender — keeps one fixed UDP five-tuple HOT.

Emits datagrams "<TAG> <SEQ> <SENT_TS_NS>" with SEQ = 1,2,3,... and NO gaps,
from a FIXED source (--src-ip/--src-port) to a FIXED destination
(--dst-ip/--dst-port), at a fixed inter-datagram interval (--rate-ms), until
SIGTERM. Binding the source address+port pins the five-tuple so the switch and
the rollback are exercised against the SAME primed tuple (proof requirement C2),
never a fresh one.

--family 4 uses AF_INET; 6 uses AF_INET6. A v6 sender is used for the untrusted
v6 source-drop leg.
"""
import argparse
import signal
import socket
import sys
import time

stop = False


def _on_term(_sig, _frm):
    global stop
    stop = True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--src-ip", required=True)
    ap.add_argument("--src-port", type=int, required=True)
    ap.add_argument("--dst-ip", required=True)
    ap.add_argument("--dst-port", type=int, required=True)
    ap.add_argument("--tag", required=True)
    ap.add_argument("--rate-ms", type=float, default=2.0)
    ap.add_argument("--family", type=int, choices=(4, 6), default=4)
    ap.add_argument("--seqfile", default=None,
                    help="write the highest seq sent so far (for the driver)")
    args = ap.parse_args()

    signal.signal(signal.SIGTERM, _on_term)
    signal.signal(signal.SIGINT, _on_term)

    fam = socket.AF_INET if args.family == 4 else socket.AF_INET6
    s = socket.socket(fam, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind((args.src_ip, args.src_port))
    dst = (args.dst_ip, args.dst_port)

    seqfile = open(args.seqfile, "w", buffering=1) if args.seqfile else None
    interval = args.rate_ms / 1000.0
    seq = 0
    next_t = time.monotonic()
    while not stop:
        seq += 1
        payload = f"{args.tag} {seq} {time.time_ns()}".encode("ascii")
        try:
            s.sendto(payload, dst)
        except OSError:
            pass
        if seqfile is not None and (seq % 20 == 0):
            seqfile.seek(0)
            seqfile.write(str(seq))
            seqfile.truncate()
        next_t += interval
        sleep = next_t - time.monotonic()
        if sleep > 0:
            time.sleep(sleep)
        else:
            next_t = time.monotonic()
    s.close()


if __name__ == "__main__":
    main()
