#!/usr/bin/env python3
"""Compare two local NeMo-Speech.cpp runtimes without exposing speech text.

Audio, model and binaries are supplied explicitly. No transcript is written to
disk or included in diagnostics; this is a paired implementation regression
check, not WER against an independent reference corpus.
"""

import argparse
import json
import math
import random
import statistics
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "dictation_eval"))
from edit_counts import edit_counts
from normalization import normalize_text


def positive_float(value: str) -> float:
    number = float(value)
    if not 0 <= number < float("inf"):
        raise argparse.ArgumentTypeError("expected a finite nonnegative number")
    return number


def run(executable: Path, audio: Path, model: Path, device: str) -> tuple[dict, float]:
    command = [
        str(executable), "transcribe", str(audio), "--model", str(model),
        "--language", "sv", "--device", device, "--format", "json", "--word-times",
    ]
    start = time.perf_counter()
    completed = subprocess.run(command, capture_output=True, text=True, timeout=900, check=False)
    elapsed = time.perf_counter() - start
    if completed.returncode:
        raise RuntimeError(f"native runtime exited with status {completed.returncode}")
    result = json.loads(completed.stdout)
    if not isinstance(result.get("text"), str) or not isinstance(result.get("words"), list):
        raise RuntimeError("native runtime returned an invalid transcript shape")
    return result, elapsed


def word_timings(result: dict) -> list[tuple[str, float, float]]:
    return [(word["word"], word["start"], word["end"]) for word in result["words"]]


def timing_summary(times: list[float]) -> dict:
    """Keep the first use distinct from subsequent-process warm timings."""
    warm = times[1:]
    return {
        "first_use_s": round(times[0], 3),
        "warm_repeats": len(warm),
        "warm_median_s": round(statistics.median(warm), 3) if warm else None,
        "warm_p95_s": round(sorted(warm)[math.ceil(0.95 * len(warm)) - 1], 3) if warm else None,
    }


def compare(baseline: dict, candidate: dict, baseline_seconds: float, candidate_seconds: float) -> dict:
    reference = normalize_text(baseline["text"])
    hypothesis = normalize_text(candidate["text"])
    # The shared scorer deliberately caps its edit-distance matrix. An exact
    # match needs no matrix, so long recordings stay checkable without lifting
    # that guard or leaking their contents to another service.
    if baseline["text"] == candidate["text"]:
        substitutions = deletions = insertions = 0
    else:
        try:
            substitutions, deletions, insertions = edit_counts(reference, hypothesis)
        except ValueError as error:
            raise RuntimeError("differing long transcripts exceed the edit-distance limit") from error
    first = word_timings(baseline)
    second = word_timings(candidate)
    max_timestamp_delta_ms = None
    timestamp_changed_words = None
    if len(first) == len(second) and all(a[0] == b[0] for a, b in zip(first, second)):
        deltas = [max(abs(a[1] - b[1]), abs(a[2] - b[2])) * 1000
                  for a, b in zip(first, second)]
        max_timestamp_delta_ms = max(deltas, default=0)
        timestamp_changed_words = sum(delta > 0.001 for delta in deltas)
    return {
        "audio_seconds": baseline.get("duration"),
        "baseline_seconds": round(baseline_seconds, 3),
        "candidate_seconds": round(candidate_seconds, 3),
        "speedup": round(baseline_seconds / candidate_seconds, 3),
        "baseline_words": len(reference),
        "candidate_words": len(hypothesis),
        "normalized_word_edits": substitutions + deletions + insertions,
        "text_exact": baseline["text"] == candidate["text"],
        "word_timings_exact": first == second,
        "timestamp_changed_words": timestamp_changed_words,
        "max_timestamp_delta_ms": (
            round(max_timestamp_delta_ms, 3) if max_timestamp_delta_ms is not None else None
        ),
    }


def quality_passes(result: dict, max_word_edits: int, max_timestamp_delta_ms: float,
                   max_timestamp_changed_words: int, *, require_exact_text: bool) -> bool:
    return (
        (not require_exact_text or result["text_exact"])
        and result["normalized_word_edits"] <= max_word_edits
        and result["max_timestamp_delta_ms"] is not None
        and result["max_timestamp_delta_ms"] <= max_timestamp_delta_ms
        and result["timestamp_changed_words"] <= max_timestamp_changed_words
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--audio", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--device", choices=("cpu", "metal"), default="metal")
    parser.add_argument("--min-speedup", type=positive_float, default=1.0)
    parser.add_argument("--quality-only", action="store_true",
                        help="Enforce text/timestamp parity without a noisy CI speed gate")
    parser.add_argument("--require-exact-text", action="store_true",
                        help="Reject punctuation/formatting differences hidden by normalization")
    parser.add_argument("--max-word-edits", type=int, default=0)
    parser.add_argument("--max-timestamp-delta-ms", type=positive_float, default=0.0)
    parser.add_argument("--max-timestamp-changed-words", type=int, default=0)
    parser.add_argument("--repeats", type=int, default=1,
                        help="paired runs in alternating randomized order; use >=5 for a decision")
    parser.add_argument("--seed", type=int, default=187)
    args = parser.parse_args()
    if (args.max_word_edits < 0 or args.max_timestamp_changed_words < 0
            or args.min_speedup < 1 or not 1 <= args.repeats <= 20):
        parser.error("word/timing changes must be nonnegative, speedup >=1, and repeats 1–20")
    for name in ("baseline", "candidate", "audio", "model"):
        path = getattr(args, name)
        if not path.is_file() or path.is_symlink():
            parser.error(f"{name} must be an existing regular file, not a symlink")

    try:
        reports: dict[str, list[dict]] = {"baseline": [], "candidate": []}
        times: dict[str, list[float]] = {"baseline": [], "candidate": []}
        start_with_candidate = bool(random.Random(args.seed).getrandbits(1))
        for index in range(args.repeats):
            order = ("candidate", "baseline") if (index % 2 == 0) == start_with_candidate else (
                "baseline", "candidate")
            for name in order:
                report, elapsed = run(getattr(args, name), args.audio, args.model, args.device)
                reports[name].append(report)
                times[name].append(elapsed)
        baseline_seconds = statistics.median(times["baseline"][1:] or times["baseline"])
        candidate_seconds = statistics.median(times["candidate"][1:] or times["candidate"])
        result = compare(reports["baseline"][0], reports["candidate"][0],
                         baseline_seconds, candidate_seconds)
    except (RuntimeError, ValueError, KeyError, subprocess.TimeoutExpired) as error:
        # Only our fixed RuntimeError strings are safe to print; library
        # exceptions could contain excerpts of private audio-derived text.
        diagnostic = str(error) if isinstance(error, RuntimeError) else type(error).__name__
        print(f"evaluation failed: {diagnostic}", file=sys.stderr)
        return 2
    result["baseline_runs_s"] = [round(elapsed, 3) for elapsed in times["baseline"]]
    result["candidate_runs_s"] = [round(elapsed, 3) for elapsed in times["candidate"]]
    result["repeats"] = args.repeats
    result["speedup_basis"] = "warm_runs" if args.repeats > 1 else "single_run"
    result["quality_only"] = args.quality_only
    result["timing"] = {name: timing_summary(measurements) for name, measurements in times.items()}
    result["stable_outputs"] = all(
        report["text"] == reports[name][0]["text"]
        and word_timings(report) == word_timings(reports[name][0])
        for name in reports for report in reports[name]
    )
    print(json.dumps(result, sort_keys=True))
    if reports["baseline"][0].get("duration") != reports["candidate"][0].get("duration"):
        return 1
    if not result["stable_outputs"]:
        return 1
    if not quality_passes(result, args.max_word_edits, args.max_timestamp_delta_ms,
                          args.max_timestamp_changed_words, require_exact_text=args.require_exact_text):
        return 1
    if not args.quality_only and baseline_seconds / candidate_seconds < args.min_speedup:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
