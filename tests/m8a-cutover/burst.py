#!/usr/bin/env python3
"""Send a BOUNDED burst of numbered datagrams from a fixed five-tuple, then
exit. Used by the RED control's honest rollback-persistence probe (part B).
Drains any reply so the conntrack flow becomes bidirectional/established."""
import argparse, socket, time


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--src-ip", required=True)
    ap.add_argument("--src-port", type=int, required=True)
    ap.add_argument("--dst-ip", required=True)
    ap.add_argument("--dst-port", type=int, required=True)
    ap.add_argument("--tag", default="X")
    ap.add_argument("--count", type=int, required=True)
    ap.add_argument("--rate-ms", type=float, default=4.0)
    ap.add_argument("--family", type=int, choices=(4, 6), default=4)
    a = ap.parse_args()
    fam = socket.AF_INET if a.family == 4 else socket.AF_INET6
    s = socket.socket(fam, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind((a.src_ip, a.src_port))
    s.settimeout(0.002)
    for i in range(1, a.count + 1):
        s.sendto(f"{a.tag} {i}".encode(), (a.dst_ip, a.dst_port))
        try:
            s.recvfrom(200)
        except OSError:
            pass
        time.sleep(a.rate_ms / 1000.0)
    s.close()


if __name__ == "__main__":
    main()
