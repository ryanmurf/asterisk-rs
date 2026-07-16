#!/usr/bin/env python3
"""Capture authenticated default-permission AMI events until a stop file exists."""

import argparse
import os
import socket


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("host")
    parser.add_argument("port", type=int)
    parser.add_argument("--stop-file", required=True)
    args = parser.parse_args()

    request = (
        "Action: Login\r\n"
        "Username: harness\r\n"
        "Secret: pin-gate-local-only\r\n\r\n"
    ).encode("ascii")
    with socket.create_connection((args.host, args.port), timeout=2) as manager:
        manager.settimeout(0.2)
        manager.sendall(request)
        while not os.path.exists(args.stop_file):
            try:
                chunk = manager.recv(65536)
            except TimeoutError:
                continue
            if not chunk:
                break
            os.write(1, chunk)


if __name__ == "__main__":
    main()
