#!/usr/bin/env python3
"""Write a pcap containing SIP UDP packets only; never capture RTP payloads."""

import argparse
import os
import socket
import struct
import time


PCAP_GLOBAL_HEADER = struct.pack("<IHHIIII", 0xA1B2C3D4, 2, 4, 0, 0, 65535, 1)


def is_sip_udp(packet: bytes) -> bool:
    if len(packet) < 42 or packet[12:14] != b"\x08\x00":
        return False
    ip_offset = 14
    ihl = (packet[ip_offset] & 0x0F) * 4
    if ihl < 20 or len(packet) < ip_offset + ihl + 8:
        return False
    if packet[ip_offset + 9] != socket.IPPROTO_UDP:
        return False
    udp_offset = ip_offset + ihl
    source, destination = struct.unpack("!HH", packet[udp_offset : udp_offset + 4])
    return source in (5060, 15060) or destination in (5060, 15060)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    parser.add_argument("--stop-file", required=True)
    args = parser.parse_args()

    packet_count = 0
    with open(args.output, "wb") as output:
        output.write(PCAP_GLOBAL_HEADER)
        with socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(3)) as capture:
            capture.settimeout(0.2)
            while not os.path.exists(args.stop_file):
                try:
                    packet = capture.recv(65535)
                except TimeoutError:
                    continue
                if not is_sip_udp(packet):
                    continue
                timestamp = time.time()
                seconds = int(timestamp)
                micros = int((timestamp - seconds) * 1_000_000)
                output.write(struct.pack("<IIII", seconds, micros, len(packet), len(packet)))
                output.write(packet)
                output.flush()
                packet_count += 1
    print(f"SIPOnlyPackets={packet_count}")


if __name__ == "__main__":
    main()
