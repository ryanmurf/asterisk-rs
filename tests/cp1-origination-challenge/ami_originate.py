#!/usr/bin/env python3
"""Trigger one rustisk outbound Dial via a single authenticated AMI Originate.

    ami_originate.py HOST PORT ENDPOINT ACTION_ID

Sends `Originate Channel: PJSIP/<ENDPOINT>` asynchronously. rustisk resolves the
endpoint's contact (live registrar binding preferred over static config) and
sends the INVITE to it. Prints the AMI response; exits nonzero if the action was
not queued.
"""

import socket
import sys


def main():
    if len(sys.argv) != 5:
        raise SystemExit("usage: ami_originate.py HOST PORT ENDPOINT ACTION_ID")
    host, port, endpoint, action_id = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4]

    login = (
        "Action: Login\r\n"
        "Username: cp1\r\n"
        "Secret: cp1-local-only\r\n"
        "\r\n"
    )
    originate = (
        "Action: Originate\r\n"
        "ActionID: %s\r\n"
        "Channel: PJSIP/%s\r\n"
        "Context: default\r\n"
        "Exten: s\r\n"
        "Priority: 1\r\n"
        "Timeout: 4000\r\n"
        "Async: true\r\n"
        "\r\n"
    ) % (action_id, endpoint)
    logoff = "Action: Logoff\r\n\r\n"

    payload = (login + originate + logoff).encode("utf-8")
    response = bytearray()
    with socket.create_connection((host, port), timeout=4) as mgr:
        mgr.settimeout(4)
        mgr.sendall(payload)
        try:
            while b"Response: Goodbye\r\n" not in response:
                chunk = mgr.recv(65536)
                if not chunk:
                    break
                response.extend(chunk)
        except socket.timeout:
            pass

    text = response.decode("utf-8", "replace")
    sys.stdout.write(text)
    # Require the Originate-specific queued message. A bare "Success" is NOT
    # sufficient: the Login reply also carries "Success", so matching it would
    # green-light a session whose Originate actually failed.
    if "successfully queued" not in text:
        sys.stderr.write("Originate not queued\n")
        sys.exit(2)


if __name__ == "__main__":
    main()
