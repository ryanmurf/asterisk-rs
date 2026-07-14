#!/usr/bin/env python3
"""Generate deterministic 8 kHz mono signed-16 WAV fixtures."""

import math
import pathlib
import struct
import sys
import wave


SAMPLE_RATE = 8_000


def write_tone(path: pathlib.Path, frequency: int, duration: float) -> None:
    sample_count = round(SAMPLE_RATE * duration)
    with wave.open(str(path), "wb") as output:
        output.setnchannels(1)
        output.setsampwidth(2)
        output.setframerate(SAMPLE_RATE)
        samples = (
            struct.pack(
                "<h",
                round(8_000 * math.sin(2 * math.pi * frequency * index / SAMPLE_RATE)),
            )
            for index in range(sample_count)
        )
        output.writeframes(b"".join(samples))


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: generate_wavs.py OUTPUT_DIRECTORY")
    output_dir = pathlib.Path(sys.argv[1])
    output_dir.mkdir(parents=True, exist_ok=True)
    write_tone(output_dir / "pin-prompt.wav", 440, 2.0)
    write_tone(output_dir / "granted.wav", 880, 1.0)
    write_tone(output_dir / "rejected.wav", 220, 1.0)


if __name__ == "__main__":
    main()
