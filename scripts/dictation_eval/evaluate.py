#!/usr/bin/env python3
"""Local evaluation CLI with explicit private paired-runner execution."""

import argparse
import json
from pathlib import Path
import sys

from corpus_manifest import coverage_report
from runner import ExecutionOutputError


VERSION = "Sagascript dictation evaluator 0.3.0 (schema 1)"
MAX_JSON_BYTES = 16 * 1024 * 1024
MAX_REFERENCE_BYTES = 400_000


def _invalid():
    return ValueError("invalid local evaluation input")


def _pairs(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise _invalid()
        result[key] = value
    return result


def _constant(_value):
    raise _invalid()


def _read(path, limit):
    try:
        with Path(path).open("rb") as handle:
            raw = handle.read(limit + 1)
        if len(raw) > limit:
            raise _invalid()
        return raw.decode("utf-8")
    except (OSError, UnicodeError, TypeError, ValueError):
        raise _invalid() from None


def _read_json(path):
    try:
        return json.loads(
            _read(path, MAX_JSON_BYTES), object_pairs_hook=_pairs,
            parse_constant=_constant,
        )
    except (ValueError, RecursionError):
        raise _invalid() from None


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Local-only, content-free evaluation summaries; never an adoption gate.")
    parser.add_argument("--version", action="version", version=VERSION)
    commands = parser.add_subparsers(dest="command", required=True)
    manifest = commands.add_parser("validate-manifest", help="Check declared corpus prerequisites")
    manifest.add_argument("manifest")
    clip = commands.add_parser("score-clip", help="Score all runs against one hash-bound reference")
    clip.add_argument("--manifest", required=True)
    clip.add_argument("--utterance-id", required=True)
    clip.add_argument("--report", required=True)
    clip.add_argument("--reference", required=True, help="Exact UTF-8 reference bytes; no trimming")
    clip.add_argument("--specialist-terms", help="Local JSON array of expected specialist phrases")
    clip.add_argument("--control-terms", help="Local JSON array of number/negation control phrases")
    freeze = commands.add_parser("freeze-plan", help="Freeze a private paired evaluation plan")
    freeze.add_argument("--manifest", required=True)
    freeze.add_argument("--configurations", required=True)
    freeze.add_argument("--split", required=True, choices=("dev", "heldout"))
    freeze.add_argument("--seed", required=True, type=int)
    freeze.add_argument("--iterations", type=int, default=5)
    freeze.add_argument("--source-revision", required=True)
    freeze.add_argument("--binary", required=True)
    freeze.add_argument("--output", required=True)
    run = commands.add_parser("run-plan", help="Execute a frozen plan with a trusted local binary")
    run.add_argument("--manifest", required=True)
    run.add_argument("--plan", required=True)
    run.add_argument("--audio-map", required=True)
    run.add_argument("--reference-map", required=True)
    run.add_argument("--terms", required=True)
    run.add_argument("--binary", required=True)
    run.add_argument("--output-dir", required=True)
    run.add_argument("--timeout-seconds", type=int, default=900)
    summary = commands.add_parser("summarize-run", help="Re-score and aggregate a complete private run ledger")
    summary.add_argument("--manifest", required=True)
    summary.add_argument("--plan", required=True)
    summary.add_argument("--reference-map", required=True)
    summary.add_argument("--terms", required=True)
    summary.add_argument("--output-dir", required=True, help="Existing private run directory; read-only")
    args = parser.parse_args(argv)
    try:
        if args.command == "validate-manifest":
            manifest_value = _read_json(args.manifest)
            result = coverage_report(manifest_value)
        elif args.command == "score-clip":
            manifest_value = _read_json(args.manifest)
            from clip_score import score_clip
            from quality_report import read_quality_report

            result = score_clip(
                manifest_value, args.utterance_id, read_quality_report(args.report),
                _read(args.reference, MAX_REFERENCE_BYTES),
                _read_json(args.specialist_terms) if args.specialist_terms else [],
                _read_json(args.control_terms) if args.control_terms else [],
            )
        elif args.command == "freeze-plan":
            from runner import freeze_plan

            result = freeze_plan(
                _read_json(args.manifest),
                _read_json(args.configurations),
                split=args.split,
                seed=args.seed,
                iterations=args.iterations,
                source_revision=args.source_revision,
                binary_path=args.binary,
                output_path=args.output,
            )
        elif args.command == "summarize-run":
            from run_summary import summarize_run

            result = summarize_run(
                _read_json(args.manifest), _read_json(args.plan),
                _read_json(args.reference_map), _read_json(args.terms), args.output_dir,
            )
        else:
            from runner import run_evaluation

            result = run_evaluation(
                _read_json(args.manifest),
                _read_json(args.plan),
                _read_json(args.audio_map),
                _read_json(args.reference_map),
                _read_json(args.terms),
                args.binary,
                args.output_dir,
                args.timeout_seconds,
            )
        encoded = json.dumps(result, allow_nan=False, sort_keys=True, indent=2)
    except ExecutionOutputError:
        print("Evaluation output failed; retain the private partial output for inspection.",
              file=sys.stderr)
        return 3
    except (OSError, ValueError, TypeError, UnicodeError, OverflowError, RecursionError):
        # Never print source paths, opaque IDs, transcript text, or an exception
        # representation. Fine-grained diagnostics remain in pure helper tests.
        print("Invalid local evaluation input; no result produced.", file=sys.stderr)
        return 2
    print(encoded)
    return 1 if args.command in {"run-plan", "summarize-run"} and result.get("failed", 0) > 0 else 0


if __name__ == "__main__":
    raise SystemExit(main())
