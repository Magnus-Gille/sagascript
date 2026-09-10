#!/usr/bin/env python3
"""Verify the default machine-readable CLI stream contract."""

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


def verify(stdout_path: Path, stderr_path: Path) -> None:
    _, stdout = decode_utf8(stdout_path)
    stderr_bytes, stderr = decode_utf8(stderr_path)

    result = json.loads(stdout)
    if not isinstance(result, dict):
        raise AssertionError("stdout must contain exactly one JSON object")
    if not result.get("text"):
        raise AssertionError("JSON result has no transcript text")

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


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit("usage: verify-json-cli-streams.py STDOUT STDERR")
    verify(Path(sys.argv[1]), Path(sys.argv[2]))


if __name__ == "__main__":
    main()
