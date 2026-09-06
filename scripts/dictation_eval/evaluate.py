#!/usr/bin/env python3
"""Local evaluation CLI; no network, inference, settings changes, or output writes."""

import argparse
import json
from pathlib import Path
import sys

from corpus_manifest import coverage_report


VERSION = "Sagascript dictation evaluator 0.1.0 (schema 1)"
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
    args = parser.parse_args(argv)
    try:
        manifest_value = _read_json(args.manifest)
        if args.command == "validate-manifest":
            result = coverage_report(manifest_value)
        else:
            from clip_score import score_clip
            from quality_report import read_quality_report

            result = score_clip(
                manifest_value, args.utterance_id, read_quality_report(args.report),
                _read(args.reference, MAX_REFERENCE_BYTES),
                _read_json(args.specialist_terms) if args.specialist_terms else [],
                _read_json(args.control_terms) if args.control_terms else [],
            )
        encoded = json.dumps(result, allow_nan=False, sort_keys=True, indent=2)
    except (ValueError, TypeError, UnicodeError, OverflowError, RecursionError):
        # Never print source paths, opaque IDs, transcript text, or an exception
        # representation. Fine-grained diagnostics remain in pure helper tests.
        print("Invalid local evaluation input; no result produced.", file=sys.stderr)
        return 2
    print(encoded)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
