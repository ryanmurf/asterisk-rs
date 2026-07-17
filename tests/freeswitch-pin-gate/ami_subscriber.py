#!/usr/bin/env python3
"""Capture AMI events for the M3 zero-hit audit until a stop file exists.

Logs in as the [harness] account, which is granted an explicit `read = all` in
manager.conf. Under the AMI least-privilege default (DENY, issues #126/#127) an
account without an explicit read grant would receive NOTHING; the harness's
positive control (observing a benign PinGate Newexten + PINGATESTATUS VarSet)
proves this subscriber is live and its read grant is effective, so audited
silence cannot masquerade as a pass.
"""

import argparse
import os
import socket
import sys


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
                # EOF before the harness signaled stop. The subscriber died
                # mid-run, so it may have MISSED later (possibly PIN-bearing)
                # events while the greps and zero-hit audit still "pass" against
                # the partial transcript — a subscriber-sees-nothing false pass.
                # Fail loudly instead: `wait "$AMI_SUBSCRIBER_PID"` in run.sh
                # then errors out rather than silently accepting truncation.
                sys.stderr.write(
                    "ami_subscriber: connection closed before stop file existed "
                    "(subscriber died early; audit transcript is truncated)\n"
                )
                sys.exit(1)
            os.write(1, chunk)


if __name__ == "__main__":
    main()
