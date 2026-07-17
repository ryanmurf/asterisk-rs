#!/usr/bin/env python3
"""M8a stand-in listener — a receiver-side witness for the cutover proof.

Models ONE PBX holding the public UDP bind (hostIP:45070 in prod; a synthetic
high port here). Binds ON COMMAND so the driver can hand the port from one
stand-in to the other:

    SIGUSR1  -> bind the port   (this stand-in takes the port)
    SIGUSR2  -> close the socket (this stand-in releases the port)
    SIGTERM  -> exit

There is deliberately NO SO_REUSEPORT: at most one stand-in can hold the port at
a time, exactly like the single public bind. bind() uses SO_REUSEADDR only so a
just-released UDP port can be reclaimed immediately (UDP has no TIME_WAIT).

Dual-stack: binds [::]:PORT with IPV6_V6ONLY=0, so a single socket witnesses
BOTH v4-mapped and v6 datagrams — that is how one listener proves the v4 AND v6
source-drop simultaneously.

Every received datagram is appended to --out as CSV:
    recv_ts_ns,label,src,tag,seq
(one flushed line per datagram — this file is the receiver-side ground truth).

Bind/unbind transitions are appended to --status as:
    <BOUND|UNBOUND|BINDFAIL> <ts_ns>
so the driver can measure the handover window and avoid EADDRINUSE races.
"""
import argparse
import errno
import os
import select
import signal
import socket
import sys
import time

want_bound = False
stop = False


def _on_bind(_sig, _frm):
    global want_bound
    want_bound = True


def _on_unbind(_sig, _frm):
    global want_bound
    want_bound = False


def _on_term(_sig, _frm):
    global stop
    stop = True


def make_socket(port):
    s = socket.socket(socket.AF_INET6, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    # Dual-stack: receive both v6 and v4-mapped on the same port.
    s.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 0)
    s.bind(("::", port))
    s.setblocking(False)
    return s


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--label", required=True, help="FS or RUSTISK")
    ap.add_argument("--out", required=True)
    ap.add_argument("--status", required=True)
    ap.add_argument("--pidfile", default=None)
    args = ap.parse_args()

    signal.signal(signal.SIGUSR1, _on_bind)
    signal.signal(signal.SIGUSR2, _on_unbind)
    signal.signal(signal.SIGTERM, _on_term)

    if args.pidfile:
        with open(args.pidfile, "w") as f:
            f.write(str(os.getpid()) + "\n")

    out = open(args.out, "a", buffering=1)
    status = open(args.status, "a", buffering=1)

    sock = None
    while not stop:
        try:
            if want_bound and sock is None:
                try:
                    sock = make_socket(args.port)
                    status.write(f"BOUND {time.time_ns()}\n")
                    status.flush()
                except OSError as e:
                    if e.errno in (errno.EADDRINUSE,):
                        # Old holder has not released yet; retry shortly.
                        time.sleep(0.0005)
                        continue
                    status.write(f"BINDFAIL {time.time_ns()} {e.errno}\n")
                    status.flush()
                    time.sleep(0.001)
                    continue
            if not want_bound and sock is not None:
                sock.close()
                sock = None
                status.write(f"UNBOUND {time.time_ns()}\n")
                status.flush()

            if sock is None:
                time.sleep(0.0005)
                continue

            try:
                r, _, _ = select.select([sock], [], [], 0.02)
            except InterruptedError:
                continue
            if not r:
                continue
            while True:
                try:
                    data, addr = sock.recvfrom(4096)
                except BlockingIOError:
                    break
                except InterruptedError:
                    break
                except OSError:
                    break
                recv_ts = time.time_ns()
                src = f"{addr[0]}:{addr[1]}"
                parts = data.decode("ascii", "replace").split()
                tag = parts[0] if len(parts) > 0 else "?"
                seq = parts[1] if len(parts) > 1 else "?"
                out.write(f"{recv_ts},{args.label},{src},{tag},{seq}\n")
        except InterruptedError:
            continue

    if sock is not None:
        sock.close()
        status.write(f"UNBOUND {time.time_ns()}\n")
        status.flush()
    out.close()
    status.close()


if __name__ == "__main__":
    main()
