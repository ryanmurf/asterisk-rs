#!/usr/bin/env python3
"""Run one authenticated AMI exchange without delaying after Logoff."""

import socket
import sys


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit("usage: ami_client.py HOST PORT")

    payload = sys.stdin.buffer.read()
    response = bytearray()
    with socket.create_connection((sys.argv[1], int(sys.argv[2])), timeout=2) as manager:
        manager.settimeout(2)
        manager.sendall(payload)
        while b"Response: Goodbye\r\n" not in response:
            chunk = manager.recv(65536)
            if not chunk:
                break
            response.extend(chunk)

    sys.stdout.buffer.write(response)


if __name__ == "__main__":
    main()
