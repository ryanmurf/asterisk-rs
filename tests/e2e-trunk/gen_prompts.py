#!/usr/bin/env python3
"""Generate the deterministic 8 kHz mono signed-16 WAV prompt fixtures the
hermetic dialplan references (PinGate prompt + granted/rejected playback).

Pure stdlib. Tones are chosen so NONE of them collide with the media-proof
tones used by the e2e (caller TX 440 Hz, mock TX 660 Hz are detected on the
FAR side only; the prompts play on the near/A-leg before the bridge exists,
so they can share 440 without contaminating the far-side assertions — but we
still keep granted/rejected distinct for eyeballing captures)."""
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
        raise SystemExit("usage: gen_prompts.py OUTPUT_DIRECTORY")
    out = pathlib.Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    write_tone(out / "pin-prompt.wav", 350, 2.0)
    write_tone(out / "granted.wav", 880, 0.8)
    write_tone(out / "rejected.wav", 220, 0.8)
    print(f"wrote prompt fixtures to {out}")


if __name__ == "__main__":
    main()
