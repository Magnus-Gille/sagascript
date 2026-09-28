#!/usr/bin/env python3
"""Repeat a tracked public PCM fixture into a >2048-frame offline smoke input."""

import argparse
import json
import os
import wave
from pathlib import Path


REPEATS = 21  # 11 s x 21 = 231 s, beyond the 2,048-encoder-frame tile gate.


def make_smoke_audio(source: Path, destination: Path) -> float:
    if not source.is_file() or source.is_symlink():
        raise ValueError("smoke source must be an existing regular file")
    with wave.open(str(source), "rb") as original:
        params = original.getparams()
        if (params.nchannels, params.sampwidth, params.framerate, params.comptype) != (
            1, 2, 16_000, "NONE"
        ):
            raise ValueError("smoke source must be 16 kHz mono PCM16")
        frames = original.readframes(original.getnframes())
        frame_count = original.getnframes()
    if frame_count != 176_000:  # The tracked JFK fixture must remain exactly 11 seconds.
        raise ValueError("unexpected smoke fixture length")
    handle = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(handle, "wb") as stream, wave.open(stream, "wb") as output:
        output.setparams(params)
        for _ in range(REPEATS):
            output.writeframesraw(frames)
        output.writeframes(b"")
    return frame_count * REPEATS / params.framerate


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    duration = make_smoke_audio(args.source, args.destination)
    print(json.dumps({"audio_seconds": duration, "expected_min_encoder_frames": 2049}))


if __name__ == "__main__":
    main()
