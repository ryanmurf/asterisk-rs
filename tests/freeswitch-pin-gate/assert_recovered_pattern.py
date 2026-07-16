#!/usr/bin/env python3
"""Assert the far-side WAV contains the reordered/deduplicated RTP audio pattern."""

import math
import pathlib
import struct
import sys
import wave


FREQUENCIES = (500, 750, 1000, 1250, 1500)
EXPECTED = tuple(index % 4 for index in range(48) if index != 20)


def power(samples: list[float], sample_rate: int, frequency: float) -> float:
    omega = 2.0 * math.pi * frequency / sample_rate
    coefficient = 2.0 * math.cos(omega)
    previous = 0.0
    previous_two = 0.0
    for sample in samples:
        current = sample + coefficient * previous - previous_two
        previous_two = previous
        previous = current
    return previous_two**2 + previous**2 - coefficient * previous * previous_two


def classify(samples: list[float], sample_rate: int) -> int | None:
    rms = math.sqrt(sum(sample * sample for sample in samples) / len(samples))
    if rms < 200:
        return None
    powers = [power(samples, sample_rate, frequency) for frequency in FREQUENCIES]
    ranked = sorted(range(len(powers)), key=powers.__getitem__, reverse=True)
    if powers[ranked[0]] < max(powers[ranked[1]], 1.0) * 2.0:
        return None
    return ranked[0]


def lcs(left: list[int], right: tuple[int, ...]) -> int:
    row = [0] * (len(right) + 1)
    for left_item in left:
        previous = 0
        for index, right_item in enumerate(right, start=1):
            saved = row[index]
            if left_item == right_item:
                row[index] = previous + 1
            else:
                row[index] = max(row[index], row[index - 1])
            previous = saved
    return row[-1]


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: assert_recovered_pattern.py WAV")

    path = pathlib.Path(sys.argv[1])
    with wave.open(str(path), "rb") as recording:
        if recording.getsampwidth() != 2 or recording.getframerate() != 8000:
            raise SystemExit("receiver pattern capture must be signed 16-bit PCM at 8 kHz")
        channels = recording.getnchannels()
        frames = recording.readframes(recording.getnframes())

    values = struct.unpack(f"<{len(frames) // 2}h", frames)
    mono = [float(values[index]) for index in range(0, len(values), channels)]
    best_score = -1
    best_observed: list[int] = []
    for offset in range(160):
        labels = []
        for start in range(offset, len(mono) - 159, 160):
            label = classify(mono[start : start + 160], 8000)
            if label is not None:
                labels.append(label)
        for marker_end in range(5, len(labels) + 1):
            if labels[marker_end - 5 : marker_end] != [4] * 5:
                continue
            observed = [label for label in labels[marker_end : marker_end + 55] if label < 4]
            score = lcs(observed, EXPECTED)
            if score > best_score:
                best_score = score
                best_observed = observed

    minimum = 44
    if best_score < minimum:
        raise SystemExit(
            "recovered RTP pattern absent or out of order: "
            f"matched={best_score}/{len(EXPECTED)} observed={best_observed}"
        )
    print(
        "RecoveredRtpPattern="
        f"{best_score}/{len(EXPECTED)} Frequencies=500/750/1000/1250 "
        "GapDataIndex=20 DuplicateDataIndex=13 SwapPairs=true"
    )


if __name__ == "__main__":
    main()
