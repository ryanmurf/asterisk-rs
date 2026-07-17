#!/usr/bin/env python3
"""Low-level AMI transport for bin/call.sh (the migrated operational command).

Subcommands (all read the AMI secret from a FILE, never argv/env value):

  list      Print active channels, one per line:
              CHANNEL\tEXTENSION\tCALLERIDNUM\tSTATE
  originate Place one outbound call (Async). Exits 0 only if AMI queued it.
  hangup    Hang up ONE named channel.

The AMI secret path (a mounted k8s Secret in production) is passed with
--secret-file; the value is read here and never printed. This replaces the ESL
password handling in the FreeSWITCH-era bin/call.sh.
"""

import argparse
import socket
import sys
import uuid


def read_secret(path):
    with open(path, "r") as f:
        return f.read().strip()


def ami_txn(host, port, username, secret, action_lines, terminator):
    """Login, send one action, read until `terminator` appears, logoff. Returns text."""
    login = "Action: Login\r\nUsername: %s\r\nSecret: %s\r\n\r\n" % (username, secret)
    action = "".join("%s\r\n" % l for l in action_lines) + "\r\n"
    logoff = "Action: Logoff\r\n\r\n"
    buf = bytearray()
    with socket.create_connection((host, port), timeout=6) as s:
        s.settimeout(6)
        s.sendall((login + action + logoff).encode("utf-8"))
        try:
            while terminator.encode("utf-8") not in buf and b"Response: Goodbye\r\n" not in buf:
                chunk = s.recv(65536)
                if not chunk:
                    break
                buf.extend(chunk)
        except socket.timeout:
            pass
    return buf.decode("utf-8", "replace")


def parse_events(text):
    """Split an AMI stream into a list of dict blocks (keyed by header name)."""
    blocks = []
    for raw in text.split("\r\n\r\n"):
        raw = raw.strip("\r\n")
        if not raw:
            continue
        d = {}
        for line in raw.split("\r\n"):
            if ":" in line:
                k, v = line.split(":", 1)
                d[k.strip()] = v.strip()
        if d:
            blocks.append(d)
    return blocks


def cmd_list(args):
    secret = read_secret(args.secret_file)
    aid = uuid.uuid4().hex[:8]
    text = ami_txn(args.host, args.port, args.username, secret,
                   ["Action: CoreShowChannels", "ActionID: %s" % aid],
                   "CoreShowChannelsComplete")
    for b in parse_events(text):
        if b.get("Event") == "CoreShowChannel":
            sys.stdout.write("%s\t%s\t%s\t%s\n" % (
                b.get("Channel", ""),
                b.get("Extension", ""),
                b.get("CallerIDNum", ""),
                b.get("ChannelStateDesc", ""),
            ))
    return 0


def cmd_originate(args):
    secret = read_secret(args.secret_file)
    aid = uuid.uuid4().hex[:8]
    lines = [
        "Action: Originate",
        "ActionID: %s" % aid,
        "Channel: %s" % args.channel,
        "Context: %s" % args.context,
        "Exten: %s" % args.exten,
        "Priority: %d" % args.priority,
        "Timeout: %d" % args.timeout,
        "Async: true",
    ]
    if args.callerid:
        lines.append("CallerID: %s" % args.callerid)
    text = ami_txn(args.host, args.port, args.username, secret, lines, "successfully queued")
    sys.stdout.write(text)
    # The Login reply also carries "Success"; require the Originate-specific
    # queued message so a failed Originate cannot false-pass.
    return 0 if "successfully queued" in text else 2


def cmd_hangup(args):
    secret = read_secret(args.secret_file)
    aid = uuid.uuid4().hex[:8]
    text = ami_txn(args.host, args.port, args.username, secret,
                   ["Action: Hangup", "ActionID: %s" % aid, "Channel: %s" % args.channel],
                   "Response:")
    sys.stdout.write(text)
    return 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=5038)
    ap.add_argument("--username", default="operator")
    ap.add_argument("--secret-file", required=True)
    sub = ap.add_subparsers(dest="cmd", required=True)

    sub.add_parser("list")

    o = sub.add_parser("originate")
    o.add_argument("--channel", required=True)
    o.add_argument("--context", default="default")
    o.add_argument("--exten", default="s")
    o.add_argument("--priority", type=int, default=1)
    o.add_argument("--callerid", default="")
    o.add_argument("--timeout", type=int, default=30000)

    h = sub.add_parser("hangup")
    h.add_argument("--channel", required=True)

    args = ap.parse_args()
    if args.cmd == "list":
        sys.exit(cmd_list(args))
    if args.cmd == "originate":
        sys.exit(cmd_originate(args))
    if args.cmd == "hangup":
        sys.exit(cmd_hangup(args))


if __name__ == "__main__":
    main()
