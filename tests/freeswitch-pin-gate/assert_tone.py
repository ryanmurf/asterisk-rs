#!/usr/bin/env python3
"""Prove that a receiver-side WAV contains the negotiated test tone."""

import math
import pathlib
import struct
import sys
import wave


def power(samples: list[float], sample_rate: int, frequency: float) -> float:
    omega = 2.0 * math.pi * frequency / sample_rate
    cosine = math.cos(omega)
    coefficient = 2.0 * cosine
    previous = 0.0
    previous_two = 0.0
    for sample in samples:
        current = sample + coefficient * previous - previous_two
        previous_two = previous
        previous = current
    return previous_two**2 + previous**2 - coefficient * previous * previous_two


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit("usage: assert_tone.py WAV FREQUENCY_HZ")

    path = pathlib.Path(sys.argv[1])
    expected = float(sys.argv[2])
    with wave.open(str(path), "rb") as recording:
        if recording.getsampwidth() != 2:
            raise SystemExit("receiver capture is not signed 16-bit PCM")
        channels = recording.getnchannels()
        sample_rate = recording.getframerate()
        frames = recording.readframes(recording.getnframes())

    values = struct.unpack(f"<{len(frames) // 2}h", frames)
    samples = [float(values[index]) for index in range(0, len(values), channels)]
    if not samples:
        raise SystemExit("receiver capture contains no samples")

    target_power = power(samples, sample_rate, expected)
    off_frequency_power = max(
        power(samples, sample_rate, expected - 170),
        power(samples, sample_rate, expected + 170),
    )
    rms = math.sqrt(sum(sample * sample for sample in samples) / len(samples))
    ratio = target_power / max(off_frequency_power, 1.0)
    if rms < 100 or ratio < 10:
        raise SystemExit(
            f"expected tone absent: rms={rms:.1f} spectral_ratio={ratio:.1f}"
        )
    print(
        f"FarCaptureToneHz={expected:.0f} RMS={rms:.1f} "
        f"SpectralRatio={ratio:.1f} Samples={len(samples)}"
    )


if __name__ == "__main__":
    main()
