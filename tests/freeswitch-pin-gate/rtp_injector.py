#!/usr/bin/env python3
"""Sniff and inject deterministic RTP inside FreeSWITCH's network namespace."""

import argparse
from collections import Counter
import json
import math
import os
import socket
import struct
import time


def checksum(data: bytes) -> int:
    if len(data) % 2:
        data += b"\0"
    words = struct.unpack(f"!{len(data) // 2}H", data)
    total = sum(words)
    while total >> 16:
        total = (total & 0xFFFF) + (total >> 16)
    return (~total) & 0xFFFF


def rtp_packet(payload_type: int, sequence: int, timestamp: int, ssrc: int, payload: bytes) -> bytes:
    return struct.pack(
        "!BBHII",
        0x80,
        payload_type & 0x7F,
        sequence & 0xFFFF,
        timestamp & 0xFFFFFFFF,
        ssrc & 0xFFFFFFFF,
    ) + payload


def raw_udp(
    source_ip: str,
    source_port: int,
    destination_ip: str,
    destination_port: int,
    payload: bytes,
    packet_id: int,
) -> None:
    udp = struct.pack("!HHHH", source_port, destination_port, 8 + len(payload), 0) + payload
    source = socket.inet_aton(source_ip)
    destination = socket.inet_aton(destination_ip)
    header = struct.pack(
        "!BBHHHBBH4s4s",
        0x45,
        0,
        20 + len(udp),
        packet_id & 0xFFFF,
        0,
        64,
        socket.IPPROTO_UDP,
        0,
        source,
        destination,
    )
    header = header[:10] + struct.pack("!H", checksum(header)) + header[12:]
    with socket.socket(socket.AF_INET, socket.SOCK_RAW, socket.IPPROTO_RAW) as raw:
        raw.setsockopt(socket.IPPROTO_IP, socket.IP_HDRINCL, 1)
        raw.sendto(header + udp, (destination_ip, destination_port))


def _read_one(packet_socket: socket.socket, args: argparse.Namespace, totals: Counter, udp_flows: Counter):
    """Read and classify a single frame. Returns a match dict, or None if the
    frame (if any) didn't match — never blocks longer than the socket's own
    recv timeout. Always updates totals/udp_flows so nothing observed during
    any phase of the sniff is lost."""
    try:
        frame = packet_socket.recv(65535)
    except TimeoutError:
        return None
    totals["frames"] += 1
    if len(frame) < 34 or struct.unpack("!H", frame[12:14])[0] != 0x0800:
        return None
    totals["ipv4"] += 1
    ip_offset = 14
    ihl = (frame[ip_offset] & 0x0F) * 4
    if len(frame) < ip_offset + ihl + 8 or frame[ip_offset + 9] != socket.IPPROTO_UDP:
        return None
    totals["udp"] += 1
    source_ip = socket.inet_ntoa(frame[ip_offset + 12 : ip_offset + 16])
    destination_ip = socket.inet_ntoa(frame[ip_offset + 16 : ip_offset + 20])
    udp_offset = ip_offset + ihl
    source_port, destination_port = struct.unpack("!HH", frame[udp_offset : udp_offset + 4])
    payload = frame[udp_offset + 8 :]
    udp_flows[f"{source_ip}:{source_port}->{destination_ip}:{destination_port}"] += 1
    if (
        source_ip != args.source_ip
        or destination_ip != args.destination_ip
        or source_port != args.source_port
        or destination_port != args.destination_port
        or len(payload) < 12
        or payload[0] >> 6 != 2
    ):
        return None
    payload_type = payload[1] & 0x7F
    sequence, timestamp, ssrc = struct.unpack("!HII", payload[2:12])
    return {
        "payload_type": payload_type,
        "sequence": sequence,
        "timestamp": timestamp,
        "ssrc": ssrc,
    }


def _flows_summary(udp_flows: Counter) -> str:
    return ", ".join(f"{flow}={count}" for flow, count in udp_flows.most_common(8))


def sniff(args: argparse.Namespace) -> None:
    totals: Counter[str] = Counter()
    udp_flows: Counter[str] = Counter()
    with socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(0x0800)) as packet_socket:
        if args.ready_file:
            with open(args.ready_file, "w", encoding="utf-8") as ready:
                ready.write("ready\n")
        packet_socket.settimeout(0.2)

        # Priming phase. The capture loop is already live at this point (the
        # ready-file above proves the raw socket is bound), but the caller's
        # flow-triggering action (an ESL round trip through a `docker exec`
        # into a FreeSWITCH container) has its own, host-load-dependent
        # startup latency that has nothing to do with RTP transit time. The
        # original implementation started its match deadline the instant the
        # socket opened, so that setup latency was silently deducted from the
        # assertion window — occasionally consuming all of it before
        # FreeSWITCH ever emitted a packet, which read as "no matching RTP
        # packet observed" even though RTP was about to flow (or had just
        # started) fine. Keep reading and classifying every frame during this
        # phase — so an early packet is never missed — but don't start the
        # real timeout clock until the caller confirms (via go_file) that it
        # has actually issued the flow-triggering action. If a match arrives
        # during priming, that's fine too: treat it as an immediate go.
        match = None
        if args.go_file:
            priming_deadline = time.monotonic() + args.priming_timeout
            while match is None and not os.path.exists(args.go_file):
                if time.monotonic() > priming_deadline:
                    raise SystemExit(
                        "go-file was never observed within the priming budget "
                        f"({args.priming_timeout}s) — the flow-triggering action "
                        "was never confirmed issued: "
                        f"frames={totals['frames']} ipv4={totals['ipv4']} "
                        f"udp={totals['udp']} flows=[{_flows_summary(udp_flows)}]"
                    )
                match = _read_one(packet_socket, args, totals, udp_flows)

        deadline = time.monotonic() + args.timeout
        while match is None and time.monotonic() < deadline:
            match = _read_one(packet_socket, args, totals, udp_flows)

        if match is None:
            raise SystemExit(
                "no matching RTP packet observed: "
                f"frames={totals['frames']} ipv4={totals['ipv4']} udp={totals['udp']} "
                f"flows=[{_flows_summary(udp_flows)}]"
            )
        print(json.dumps(match, sort_keys=True))


def inject(args: argparse.Namespace) -> None:
    payload = bytes([0xFF]) * 160
    valid = rtp_packet(args.payload_type, args.sequence, args.timestamp, args.ssrc, payload)
    if args.kind == "wrong-source":
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
            udp.bind((args.source_ip, 0))
            actual_port = udp.getsockname()[1]
            if actual_port == args.source_port:
                raise SystemExit("ephemeral wrong-source port unexpectedly matched negotiated port")
            udp.sendto(valid, (args.destination_ip, args.destination_port))
        print(f"Injected=wrong-source SourcePort={actual_port}")
        return

    if args.kind == "wrong-pt":
        hostile = rtp_packet(18, args.sequence, args.timestamp, args.ssrc, payload)
    elif args.kind == "malformed":
        hostile = b"\x80\x00\x00"
    elif args.kind == "unstable-ssrc":
        hostile = rtp_packet(
            args.payload_type,
            args.sequence,
            args.timestamp,
            args.ssrc ^ 0xA5A5A5A5,
            payload,
        )
    else:
        raise SystemExit(f"unsupported injection kind: {args.kind}")
    raw_udp(
        args.source_ip,
        args.source_port,
        args.destination_ip,
        args.destination_port,
        hostile,
        args.sequence,
    )
    print(f"Injected={args.kind} SourcePort={args.source_port}")


def linear_to_mulaw(sample: int) -> int:
    bias = 0x84
    clip = 32635
    sign = 0x80 if sample < 0 else 0
    magnitude = min(abs(sample), clip) + bias
    exponent = 7
    mask = 0x4000
    while exponent > 0 and not magnitude & mask:
        exponent -= 1
        mask >>= 1
    mantissa = (magnitude >> (exponent + 3)) & 0x0F
    return (~(sign | (exponent << 4) | mantissa)) & 0xFF


def tone_payload(frequency: int, packet_index: int) -> bytes:
    samples = []
    for offset in range(160):
        absolute = packet_index * 160 + offset
        sample = round(10000 * math.sin(2 * math.pi * frequency * absolute / 8000))
        samples.append(linear_to_mulaw(sample))
    return bytes(samples)


def pattern(args: argparse.Namespace) -> None:
    marker_packets = 5
    data_packets = 48
    gap_index = 20
    duplicate_index = 13
    frequencies = (500, 750, 1000, 1250)
    packets = []
    for index in range(marker_packets + data_packets):
        data_index = index - marker_packets
        if data_index == gap_index:
            continue
        frequency = 1500 if data_index < 0 else frequencies[data_index % len(frequencies)]
        sequence = args.sequence + index
        timestamp = args.timestamp + index * 160
        packet = rtp_packet(
            args.payload_type,
            sequence,
            timestamp,
            args.ssrc,
            tone_payload(frequency, index),
        )
        packets.append((index, packet, sequence))

    marker = packets[:marker_packets]
    data = packets[marker_packets:]
    ordered = marker[:]
    cursor = 0
    while cursor < len(data):
        if cursor + 1 < len(data):
            ordered.extend([data[cursor + 1], data[cursor]])
            cursor += 2
        else:
            ordered.append(data[cursor])
            cursor += 1

    packet_id = 1000
    for index, packet, sequence in ordered:
        raw_udp(
            args.source_ip,
            args.source_port,
            args.destination_ip,
            args.destination_port,
            packet,
            packet_id,
        )
        packet_id += 1
        if index == marker_packets + duplicate_index:
            raw_udp(
                args.source_ip,
                args.source_port,
                args.destination_ip,
                args.destination_port,
                packet,
                packet_id,
            )
            packet_id += 1
        time.sleep(0.012)

    print(
        "PatternInjected="
        f"Packets{marker_packets + data_packets - 1} "
        f"GapDataIndex={gap_index} DuplicateDataIndex={duplicate_index} "
        f"SwapPairs=true StartSequence={args.sequence}"
    )


def common_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--source-ip", required=True)
    parser.add_argument("--source-port", required=True, type=int)
    parser.add_argument("--destination-ip", required=True)
    parser.add_argument("--destination-port", required=True, type=int)


def main() -> None:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)

    sniff_parser = commands.add_parser("sniff")
    common_arguments(sniff_parser)
    sniff_parser.add_argument("--timeout", type=float, default=5.0)
    sniff_parser.add_argument("--ready-file")
    sniff_parser.add_argument(
        "--go-file",
        help=(
            "Path the caller creates once it has confirmed the flow-triggering "
            "action was actually issued (e.g. after a synchronous ESL command "
            "returns). --timeout is measured from this point, not from socket "
            "open, so caller-side setup latency can't eat into the match window. "
            "If omitted, --timeout is measured from socket open as before."
        ),
    )
    sniff_parser.add_argument(
        "--priming-timeout",
        type=float,
        default=20.0,
        help="Bounded fallback: how long to wait for --go-file to appear before giving up.",
    )
    sniff_parser.set_defaults(function=sniff)

    inject_parser = commands.add_parser("inject")
    common_arguments(inject_parser)
    inject_parser.add_argument(
        "--kind",
        choices=("wrong-source", "wrong-pt", "malformed", "unstable-ssrc"),
        required=True,
    )
    inject_parser.add_argument("--payload-type", required=True, type=int)
    inject_parser.add_argument("--sequence", required=True, type=int)
    inject_parser.add_argument("--timestamp", required=True, type=int)
    inject_parser.add_argument("--ssrc", required=True, type=int)
    inject_parser.set_defaults(function=inject)

    pattern_parser = commands.add_parser("pattern")
    common_arguments(pattern_parser)
    pattern_parser.add_argument("--payload-type", required=True, type=int)
    pattern_parser.add_argument("--sequence", required=True, type=int)
    pattern_parser.add_argument("--timestamp", required=True, type=int)
    pattern_parser.add_argument("--ssrc", required=True, type=int)
    pattern_parser.set_defaults(function=pattern)

    args = parser.parse_args()
    args.function(args)


if __name__ == "__main__":
    main()
