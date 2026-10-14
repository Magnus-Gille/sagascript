#!/usr/bin/env python3
"""Verify the machine-readable CLI stream contract of `sagascript transcribe`.

Modes: `--json` (the default) for one result object, `--jsonl` for one item per
line, `--error` for the structured error object of a failed `--json` run. In
every mode stderr must stay bounded and free of native trace output.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

MAX_STDERR_BYTES = 64 * 1024
NATIVE_TRACE_PATTERNS = (
    re.compile(r"\b0x[0-9a-fA-F]{6,}\b"),
    re.compile(r"\bggml_metal_(?:init|free)\b"),
    re.compile(r"\bwhisper_model_load\b"),
    re.compile(r"\btoken\s*=\s*\d+\b", re.IGNORECASE),
)


def decode_utf8(path: Path) -> tuple[bytes, str]:
    raw = path.read_bytes()
    try:
        return raw, raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise AssertionError(f"{path.name} is not valid UTF-8: {error}") from error


def verify_stderr(stderr_path: Path) -> None:
    stderr_bytes, stderr = decode_utf8(stderr_path)
    if len(stderr_bytes) >= MAX_STDERR_BYTES:
        raise AssertionError(
            f"default stderr is unbounded: {len(stderr_bytes)} bytes "
            f"(limit {MAX_STDERR_BYTES})"
        )
    for pattern in NATIVE_TRACE_PATTERNS:
        if pattern.search(stderr):
            raise AssertionError(
                f"default stderr contains native trace output matching {pattern.pattern!r}"
            )


def verify(stdout_path: Path, stderr_path: Path) -> None:
    """`transcribe --json` on one file: one result object with text."""
    _, stdout = decode_utf8(stdout_path)

    result = json.loads(stdout)
    if not isinstance(result, dict):
        raise AssertionError("stdout must contain exactly one JSON object")
    if not result.get("text"):
        raise AssertionError("JSON result has no transcript text")

    verify_stderr(stderr_path)


def verify_jsonl(stdout_path: Path, stderr_path: Path) -> None:
    """`transcribe --jsonl`: one compact `{status,source,result}` item per line."""
    _, stdout = decode_utf8(stdout_path)
    if not stdout.endswith("\n"):
        raise AssertionError("JSONL stdout must end with a newline")
    lines = stdout.splitlines()
    if not lines:
        raise AssertionError("JSONL stdout has no items")
    for number, line in enumerate(lines, start=1):
        item = json.loads(line)
        if not isinstance(item, dict):
            raise AssertionError(f"JSONL line {number} is not an object")
        if item.get("status") != "ok":
            raise AssertionError(f"JSONL line {number} failed: {item.get('error')!r}")
        if not isinstance(item.get("source"), str) or not item["source"]:
            raise AssertionError(f"JSONL line {number} has no source")
        result = item.get("result")
        if not isinstance(result, dict) or not result.get("text"):
            raise AssertionError(f"JSONL line {number} has no transcript text")

    verify_stderr(stderr_path)


def verify_error(stdout_path: Path, stderr_path: Path) -> None:
    """A failed `--json` run: one `{status: "error", code, error}` object (#364)."""
    _, stdout = decode_utf8(stdout_path)
    _, stderr = decode_utf8(stderr_path)

    result = json.loads(stdout)
    if not isinstance(result, dict):
        raise AssertionError("stdout must contain exactly one JSON error object")
    if result.get("status") != "error":
        raise AssertionError(f"expected status 'error', got {result.get('status')!r}")
    for key in ("code", "error"):
        if not isinstance(result.get(key), str) or not result[key]:
            raise AssertionError(f"error object has no {key}")
    if "Error: " not in stderr:
        raise AssertionError("stderr must still carry the human-readable error")

    verify_stderr(stderr_path)


MODES = {"--json": verify, "--jsonl": verify_jsonl, "--error": verify_error}


def main() -> None:
    args = sys.argv[1:]
    mode = args.pop(0) if args and args[0] in MODES else "--json"
    if len(args) != 2:
        raise SystemExit("usage: verify-json-cli-streams.py [--json|--jsonl|--error] STDOUT STDERR")
    MODES[mode](Path(args[0]), Path(args[1]))


if __name__ == "__main__":
    main()
