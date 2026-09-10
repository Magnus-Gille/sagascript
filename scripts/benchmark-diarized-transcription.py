#!/usr/bin/env python3
"""Benchmark one long diarized Sagascript transcription."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


MIN_FIXTURE_SECONDS = 15 * 60
MAX_FIXTURE_SECONDS = 30 * 60


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary",
        type=Path,
        default=Path("src-tauri/target/release/sagascript"),
    )
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--language", required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--threshold", type=float, default=0.75)
    parser.add_argument("--cache", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument(
        "--allow-short",
        action="store_true",
        help="Allow a fixture outside the release-benchmark 15-30 minute range",
    )
    return parser.parse_args()


def peak_rss_bytes(stderr: str) -> int | None:
    """Parse macOS `/usr/bin/time -l` maximum resident set size (bytes)."""
    match = re.search(r"^\s*(\d+)\s+maximum resident set size\s*$", stderr, re.MULTILINE)
    return int(match.group(1)) if match else None


def command_for(args: argparse.Namespace) -> list[str]:
    command = [
        str(args.binary.resolve()),
        "transcribe",
        str(args.input.resolve()),
        "--language",
        args.language,
        "--model",
        args.model,
        "--json",
        "--diarize",
        "--diarize-threshold",
        str(args.threshold),
    ]
    if args.cache:
        command.extend(["--diarize-cache", str(args.cache.resolve())])
    return command


def run(command: list[str]) -> tuple[float, int | None, dict[str, Any]]:
    measured = ["/usr/bin/time", "-l", *command] if sys.platform == "darwin" else command
    started = time.perf_counter()
    completed = subprocess.run(measured, text=True, capture_output=True, check=False)
    wall_seconds = time.perf_counter() - started

    # Preserve native diagnostics and Sagascript's phase summary for the log.
    sys.stderr.write(completed.stderr)
    if completed.returncode != 0:
        raise SystemExit(f"benchmark command failed with exit code {completed.returncode}")
    try:
        result = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise SystemExit(f"benchmark command emitted invalid JSON: {error}") from error
    if not isinstance(result, dict):
        raise SystemExit("benchmark command JSON must be a top-level object")
    if not isinstance(result.get("performance"), dict):
        raise SystemExit("benchmark command JSON is missing structured performance timings")
    if not isinstance(result.get("duration_seconds"), (int, float)):
        raise SystemExit("benchmark command JSON is missing duration_seconds")
    rss = peak_rss_bytes(completed.stderr) if sys.platform == "darwin" else None
    return wall_seconds, rss, result


def main() -> None:
    args = parse_args()
    if not args.binary.is_file():
        raise SystemExit(f"binary not found: {args.binary}")
    if not args.input.is_file():
        raise SystemExit(f"input not found: {args.input}")

    command = command_for(args)
    wall_seconds, rss, result = run(command)
    duration_seconds = float(result["duration_seconds"])
    if not args.allow_short and not (
        MIN_FIXTURE_SECONDS <= duration_seconds <= MAX_FIXTURE_SECONDS
    ):
        raise SystemExit(
            f"fixture duration {duration_seconds:.2f}s is outside 15-30 minutes; "
            "use --allow-short only for smoke tests"
        )

    report = {
        "command": {
            "binary": str(args.binary.resolve()),
            "input": str(args.input.resolve()),
            "language": args.language,
            "model": args.model,
            "threshold": args.threshold,
            "cache": str(args.cache.resolve()) if args.cache else None,
        },
        "wall_seconds": round(wall_seconds, 3),
        "audio_duration_seconds": round(duration_seconds, 3),
        "realtime_factor": round(duration_seconds / wall_seconds, 3),
        "peak_rss_bytes": rss,
        "performance": result["performance"],
        "quality": {
            "coverage_ratio": result.get("coverage_ratio"),
            "speaker_count": len(result.get("speakers", [])),
            "segment_count": len(result.get("segments", [])),
            "uncovered_span_count": len(result.get("uncovered_spans", [])),
            "warning_codes": sorted(
                {
                    warning.get("code")
                    for warning in result.get("warnings", [])
                    if isinstance(warning, dict) and isinstance(warning.get("code"), str)
                }
            ),
        },
    }
    encoded = json.dumps(report, separators=(",", ":"), sort_keys=True)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded + "\n", encoding="utf-8")
    print(encoded)


if __name__ == "__main__":
    main()
