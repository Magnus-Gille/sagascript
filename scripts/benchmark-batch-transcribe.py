#!/usr/bin/env python3
"""Integration benchmark for one-load batch transcription.

Build the lean release CLI first, then compare three standalone invocations
with one three-file batch. The transcript text must be equivalent and the batch
must be at least 1.5x faster for the benchmark to pass.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import tempfile
import time
from pathlib import Path


def run(command: list[str]) -> tuple[float, object, str]:
    started = time.perf_counter()
    completed = subprocess.run(command, text=True, capture_output=True, check=False)
    elapsed = time.perf_counter() - started
    if completed.returncode != 0:
        raise SystemExit(
            f"command failed ({completed.returncode}): {' '.join(command)}\n"
            f"stderr:\n{completed.stderr}"
        )
    return elapsed, json.loads(completed.stdout), completed.stderr


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--binary",
        type=Path,
        default=Path("src-tauri/target/release/sagascript"),
    )
    parser.add_argument(
        "--fixture", type=Path, default=Path("test-audio/norwegian-short-3s.mp3")
    )
    parser.add_argument("--model", default="nb-whisper-tiny")
    parser.add_argument("--minimum-speedup", type=float, default=1.5)
    args = parser.parse_args()

    binary = args.binary.resolve()
    fixture = args.fixture.resolve()
    if not binary.is_file():
        raise SystemExit(f"binary not found: {binary}")
    if not fixture.is_file():
        raise SystemExit(f"fixture not found: {fixture}")

    common = [
        "--language",
        "no",
        "--model",
        args.model,
        "--beam",
        "0",
        "--json",
    ]
    with tempfile.TemporaryDirectory(prefix="sagascript-batch-benchmark-") as temp:
        files = []
        for index in range(3):
            target = Path(temp) / f"fixture-{index + 1}.mp3"
            shutil.copyfile(fixture, target)
            files.append(target)

        # Warm OS caches and native initialization outside both measurements.
        run([str(binary), "transcribe", str(files[0]), *common])

        individual_seconds = 0.0
        individual_texts = []
        for source in files:
            elapsed, payload, _ = run(
                [str(binary), "transcribe", str(source), *common]
            )
            individual_seconds += elapsed
            individual_texts.append(payload["text"])

        batch_seconds, batch_payload, batch_stderr = run(
            [str(binary), "transcribe", *(str(path) for path in files), *common]
        )
        batch_texts = [item["result"]["text"] for item in batch_payload]

    if individual_texts != batch_texts:
        raise SystemExit(
            f"transcript mismatch:\nindividual={individual_texts!r}\nbatch={batch_texts!r}"
        )
    load_count = batch_stderr.count("Loading model once")
    if load_count != 1:
        raise SystemExit(f"expected one batch model load diagnostic, observed {load_count}")

    speedup = individual_seconds / batch_seconds
    print(f"individual: {individual_seconds:.3f}s")
    print(f"batch:      {batch_seconds:.3f}s")
    print(f"speedup:    {speedup:.2f}x")
    print("text:       equivalent")
    print("model load: once")
    if speedup < args.minimum_speedup:
        raise SystemExit(
            f"batch speedup {speedup:.2f}x is below required "
            f"{args.minimum_speedup:.2f}x"
        )


if __name__ == "__main__":
    main()
