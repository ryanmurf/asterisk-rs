#!/usr/bin/env python3
"""Deterministic UDP SIP impairment proxy for the offline FreeSWITCH gate."""

import argparse
import hashlib
import json
import os
import re
import signal
import socket
import time


def endpoint(value):
    host, port = value.rsplit(":", 1)
    return host, int(port)


def sip_meta(payload):
    text = payload.decode("latin1", errors="replace")
    first = text.splitlines()[0] if text else ""
    response = first.startswith("SIP/2.0 ")
    status = int(first.split()[1]) if response and len(first.split()) > 1 else None
    method = None if response else (first.split()[0].upper() if first else None)
    cseq = re.search(r"(?im)^CSeq:\s*(\d+)\s+([A-Z]+)", text)
    call_id = re.search(r"(?im)^Call-ID:\s*([^\r\n]+)", text)
    return {
        "first": first,
        "response": response,
        "status": status,
        "method": method,
        "cseq_number": int(cseq.group(1)) if cseq else None,
        "cseq_method": cseq.group(2).upper() if cseq else None,
        "call_id": call_id.group(1).strip() if call_id else None,
    }


def header(text, name):
    match = re.search(rf"(?im)^{re.escape(name)}:\s*([^\r\n]+)", text)
    return match.group(1).strip() if match else None


def replace_contact(payload, proxy_host, proxy_port):
    text = payload.decode("latin1", errors="replace")
    pattern = re.compile(r"(?im)^(Contact\s*:\s*.*?@)([^;>\s]+)")

    def replacement(match):
        return f"{match.group(1)}{proxy_host}:{proxy_port}"

    return pattern.sub(replacement, text).encode("latin1")


def replace_status(payload, status, reason):
    text = payload.decode("latin1", errors="replace")
    return re.sub(
        r"^SIP/2\.0\s+\d{3}\s+[^\r\n]+",
        f"SIP/2.0 {status} {reason}",
        text,
        count=1,
    ).encode("latin1")


def add_proxy_via(payload, proxy_host, proxy_port):
    text = payload.decode("latin1", errors="replace")
    first, separator, remainder = text.partition("\r\n")
    if not separator:
        return payload
    branch = hashlib.sha256(payload).hexdigest()[:20]
    via = (f"Via: SIP/2.0/UDP {proxy_host}:{proxy_port};"
           f"branch=z9hG4bKproxy{branch};rport\r\n")
    return f"{first}\r\n{via}{remainder}".encode("latin1")


def remove_proxy_via(payload):
    text = payload.decode("latin1", errors="replace")
    pattern = re.compile(r"(?im)^Via:\s*SIP/2\.0/UDP\s+[^\r\n]*branch=z9hG4bKproxy[^\r\n]*\r\n")
    return pattern.sub("", text, count=1).encode("latin1")


class Proxy:
    def __init__(self, args):
        self.listen = endpoint(args.listen)
        self.fs = endpoint(args.freeswitch)
        self.rustisk = endpoint(args.rustisk)
        self.proxy_host = args.proxy_host
        self.control_path = args.control
        self.state_path = args.state
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.bind(self.listen)
        self.sock.settimeout(0.05)
        self.running = True
        self.control_mtime = None
        self.generation = 0
        self.mode = "none"
        self.mode_seen = {}
        self.held_200 = None
        self.held_released = False
        self.saved_replay_invite = None
        self.replay_sent = False
        self.last_inbound_invite = None
        self.last_inbound_200 = None
        self.state = {"generation": 0, "mode": "none", "events": [], "counters": {}}

    def write_state(self):
        tmp = f"{self.state_path}.tmp"
        with open(tmp, "w", encoding="utf-8") as output:
            json.dump(self.state, output, sort_keys=True)
        os.replace(tmp, self.state_path)

    def bump(self, name, amount=1):
        counters = self.state["counters"]
        counters[name] = counters.get(name, 0) + amount

    def record(self, direction, meta, payload, action):
        event = {
            "direction": direction,
            "first": meta["first"],
            "status": meta["status"],
            "method": meta["method"],
            "cseq_method": meta["cseq_method"],
            "call_id": meta["call_id"],
            "sha256": hashlib.sha256(payload).hexdigest(),
            "action": action,
        }
        self.state["events"].append(event)
        self.state["events"] = self.state["events"][-800:]
        self.bump("packets")
        if meta["response"]:
            self.bump(f"{direction}_response_{meta['status']}_{meta['cseq_method']}")
        elif meta["method"]:
            self.bump(f"{direction}_request_{meta['method']}")

    def load_control(self):
        try:
            mtime = os.stat(self.control_path).st_mtime_ns
        except FileNotFoundError:
            return
        if mtime == self.control_mtime:
            return
        with open(self.control_path, encoding="utf-8") as source:
            control = json.load(source)
        self.control_mtime = mtime
        self.generation = int(control.get("generation", self.generation + 1))
        self.mode = control.get("mode", "none")
        self.mode_seen = {}
        self.held_200 = None
        self.held_released = False
        self.saved_replay_invite = None
        self.replay_sent = False
        self.state = {
            "generation": self.generation,
            "mode": self.mode,
            "events": [],
            "counters": {},
        }
        if control.get("inject_forged"):
            self.inject_forged_dialog_requests()
        self.write_state()

    def inject_forged_dialog_requests(self):
        if not self.last_inbound_invite or not self.last_inbound_200:
            self.state["inject_error"] = "dialog not captured"
            return
        invite_text = self.last_inbound_invite.decode("latin1", errors="replace")
        answer_text = self.last_inbound_200.decode("latin1", errors="replace")
        call_id = header(invite_text, "Call-ID")
        from_value = header(invite_text, "From")
        to_value = header(answer_text, "To")
        cseq_value = header(invite_text, "CSeq")
        if not all((call_id, from_value, to_value, cseq_value)):
            self.state["inject_error"] = "captured dialog missing headers"
            return
        invite_cseq = int(cseq_value.split()[0])
        forged = [
            (f"forged-{call_id}", from_value, to_value, invite_cseq + 1),
            (call_id, re.sub(r";tag=[^;>\s]+", ";tag=forged-tag", from_value), to_value,
             invite_cseq + 2),
            (call_id, from_value, to_value, invite_cseq),
        ]
        for index, (variant_call_id, variant_from, variant_to, cseq) in enumerate(forged, 1):
            request = (
                f"BYE sip:rustisk@{self.rustisk[0]}:{self.rustisk[1]} SIP/2.0\r\n"
                f"Via: SIP/2.0/UDP {self.proxy_host}:{self.listen[1]};"
                f"branch=z9hG4bKforged{self.generation}{index};rport\r\n"
                f"Max-Forwards: 70\r\n"
                f"From: {variant_from}\r\n"
                f"To: {variant_to}\r\n"
                f"Call-ID: {variant_call_id}\r\n"
                f"CSeq: {cseq} BYE\r\n"
                "Content-Length: 0\r\n\r\n"
            ).encode("latin1")
            self.sock.sendto(request, self.rustisk)
            self.record("injected_to_rustisk", sip_meta(request), request, "inject-forged")
            self.bump("forged_injected")

    def process(self, payload, source):
        if source[0] == self.fs[0]:
            direction = "fs_to_rustisk"
            destination = self.rustisk
        elif source[0] == self.rustisk[0]:
            direction = "rustisk_to_fs"
            destination = self.fs
        else:
            self.bump("unknown_source")
            self.write_state()
            return

        original_meta = sip_meta(payload)
        forwarded = replace_contact(payload, self.proxy_host, self.listen[1])
        if original_meta["response"]:
            forwarded = remove_proxy_via(forwarded)
        elif original_meta["method"]:
            forwarded = add_proxy_via(forwarded, self.proxy_host, self.listen[1])
        action = "forward"

        if direction == "fs_to_rustisk" and original_meta["method"] == "INVITE":
            self.last_inbound_invite = forwarded
            if self.mode == "replay_invite_after_200" and self.saved_replay_invite is None:
                self.saved_replay_invite = forwarded
        if (direction == "rustisk_to_fs" and original_meta["status"] == 200
                and original_meta["cseq_method"] == "INVITE"):
            self.last_inbound_200 = payload

        if (self.mode == "drop_first_invite_200"
                and direction == "fs_to_rustisk"
                and original_meta["status"] == 200
                and original_meta["cseq_method"] == "INVITE"):
            seen = self.mode_seen.get("invite_200", 0)
            self.mode_seen["invite_200"] = seen + 1
            if seen == 0:
                action = "drop"
        elif (self.mode == "drop_all_ack"
                and direction == "fs_to_rustisk"
                and original_meta["method"] == "ACK"):
            action = "drop"
        elif (self.mode == "drop_first_bye"
                and direction == "rustisk_to_fs"
                and original_meta["method"] == "BYE"):
            seen = self.mode_seen.get("bye", 0)
            self.mode_seen["bye"] = seen + 1
            if seen == 0:
                action = "drop"
        elif (self.mode == "hold_invite_200_until_cancel"
                and direction == "fs_to_rustisk"
                and original_meta["status"] == 200
                and original_meta["cseq_method"] == "INVITE"
                and not self.held_released):
            if self.held_200 is None:
                self.held_200 = (forwarded, destination)
            action = "hold"
        elif (self.mode == "rewrite_bye_final_481"
                and direction == "fs_to_rustisk"
                and original_meta["status"] == 200
                and original_meta["cseq_method"] == "BYE"
                and not self.mode_seen.get("bye_final")):
            self.mode_seen["bye_final"] = 1
            forwarded = replace_status(forwarded, 481, "Call/Transaction Does Not Exist")
            action = "rewrite-481"
        elif (self.mode == "drop_all_bye_final"
                and direction == "fs_to_rustisk"
                and original_meta["status"] is not None
                and original_meta["status"] >= 200
                and original_meta["cseq_method"] == "BYE"):
            action = "drop"

        self.record(direction, original_meta, forwarded, action)
        if action not in ("drop", "hold"):
            self.sock.sendto(forwarded, destination)

        if (self.mode == "hold_invite_200_until_cancel"
                and direction == "rustisk_to_fs"
                and original_meta["method"] == "CANCEL"
                and self.held_200 is not None
                and not self.held_released):
            held, held_destination = self.held_200
            self.sock.sendto(held, held_destination)
            self.held_released = True
            self.bump("held_200_released_after_cancel")

        if (self.mode == "replay_invite_after_200"
                and direction == "rustisk_to_fs"
                and original_meta["status"] == 200
                and original_meta["cseq_method"] == "INVITE"
                and self.saved_replay_invite is not None
                and not self.replay_sent):
            for _ in range(2):
                self.sock.sendto(self.saved_replay_invite, self.rustisk)
                replay_meta = sip_meta(self.saved_replay_invite)
                self.record("replayed_to_rustisk", replay_meta,
                            self.saved_replay_invite, "replay-after-200")
            self.replay_sent = True
            self.bump("late_invite_replays", 2)
        self.write_state()

    def run(self):
        while self.running:
            self.load_control()
            try:
                payload, source = self.sock.recvfrom(65535)
            except socket.timeout:
                continue
            self.process(payload, source)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--listen", required=True)
    parser.add_argument("--freeswitch", required=True)
    parser.add_argument("--rustisk", required=True)
    parser.add_argument("--proxy-host", required=True)
    parser.add_argument("--control", required=True)
    parser.add_argument("--state", required=True)
    args = parser.parse_args()
    proxy = Proxy(args)
    signal.signal(signal.SIGTERM, lambda *_: setattr(proxy, "running", False))
    signal.signal(signal.SIGINT, lambda *_: setattr(proxy, "running", False))
    proxy.run()


if __name__ == "__main__":
    main()
