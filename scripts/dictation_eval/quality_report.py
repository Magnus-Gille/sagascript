"""Strict reader for explicit Sagascript benchmark quality reports.

The report is an evidence container, not a quality or adoption decision.  In
particular, ``cli_checks_passed`` records only the CLI's input gates and the
model digest fields identify the selected expected model artifact; this module
does not verify a model download or infer transcript quality.
"""

from __future__ import annotations

import json
import math
from pathlib import Path
from typing import Any

MAX_REPORT_BYTES = 16 * 1024 * 1024
MAX_TEXT_CHARS = 100_000

_LANGUAGES = {"en", "sv", "no", "fi"}
_MODELS = {
    "tiny.en",
    "tiny",
    "base.en",
    "base",
    "kb-whisper-tiny",
    "kb-whisper-base",
    "kb-whisper-small",
    "kb-whisper-medium",
    "kb-whisper-large",
    "nb-whisper-tiny",
    "nb-whisper-base",
    "nb-whisper-small",
    "nb-whisper-medium",
    "nb-whisper-large",
    "small.en",
    "small",
    "medium.en",
    "medium",
    "large-v3-turbo",
    "large-v3-turbo-q8_0",
}

_TOP_LEVEL_KEYS = {
    "schema_version",
    "build_version",
    "language",
    "model",
    "model_expected_sha256",
    "model_expected_bytes",
    "source_audio_sha256",
    "decoded_audio_sha256",
    "duration_seconds",
    "decode_duration_ms",
    "beam_size",
    "temperature_fallback",
    "allow_empty",
    "measurement_endpoint",
    "cold_definition",
    "cli_checks_passed",
    "runs",
}
_RUN_KEYS = {
    "kind",
    "iteration",
    "text",
    "model_ms",
    "inference_ms",
    "total_ms",
    "model_cached",
}


def _invalid() -> ValueError:
    # Keep all validation errors content-free: reports can contain private
    # transcripts and fixture identifiers.
    return ValueError("invalid quality report")


def _reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise _invalid()
        result[key] = value
    return result


def _reject_nonfinite_constant(_value: str) -> Any:
    raise _invalid()


def _object(value: Any, keys: set[str]) -> dict[str, Any]:
    if type(value) is not dict or set(value) != keys:
        raise _invalid()
    return value


def _string(value: Any, *, nonempty: bool = False) -> str:
    if type(value) is not str or len(value) > MAX_TEXT_CHARS:
        raise _invalid()
    if any(0xD800 <= ord(character) <= 0xDFFF for character in value):
        raise _invalid()
    if nonempty and not value:
        raise _invalid()
    return value


def _boolean(value: Any) -> bool:
    if type(value) is not bool:
        raise _invalid()
    return value


def _integer(value: Any) -> int:
    if type(value) is not int:
        raise _invalid()
    return value


def _finite_nonnegative(value: Any) -> int | float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise _invalid()
    try:
        finite_value = float(value)
    except (OverflowError, ValueError):
        raise _invalid() from None
    if not math.isfinite(finite_value):
        raise _invalid()
    if value < 0:
        raise _invalid()
    return value


def _sha256(value: Any) -> str:
    if (
        type(value) is not str
        or len(value) != 64
        or any(character not in "0123456789abcdef" for character in value)
    ):
        raise _invalid()
    return value


def validate_quality_report(value: Any) -> dict[str, Any]:
    """Validate and shallow-copy a decoded quality report.

    The returned dictionary and run dictionaries/lists are copies of the
    decoded input.  Transcript text is retained only in this returned value;
    validation failures never include report content in their messages.
    """

    report = _object(value, _TOP_LEVEL_KEYS)
    if _integer(report["schema_version"]) != 1:
        raise _invalid()
    _string(report["build_version"], nonempty=True)
    if _string(report["language"]) not in _LANGUAGES:
        raise _invalid()
    if _string(report["model"], nonempty=True) not in _MODELS:
        raise _invalid()
    _sha256(report["model_expected_sha256"])
    model_expected_bytes = _integer(report["model_expected_bytes"])
    if not 1 <= model_expected_bytes <= 2**64 - 1:
        raise _invalid()
    _sha256(report["source_audio_sha256"])
    _sha256(report["decoded_audio_sha256"])
    duration_seconds = _finite_nonnegative(report["duration_seconds"])
    if not 0 < duration_seconds <= 120:
        raise _invalid()
    _finite_nonnegative(report["decode_duration_ms"])
    beam_size = _integer(report["beam_size"])
    if beam_size != 0 and not 2 <= beam_size <= 16:
        raise _invalid()
    _boolean(report["temperature_fallback"])
    _boolean(report["allow_empty"])
    if _string(report["measurement_endpoint"]) != "live_inference_call_not_visible_text":
        raise _invalid()
    if _string(report["cold_definition"]) != "first_call_in_new_backend_not_system_cold":
        raise _invalid()
    _boolean(report["cli_checks_passed"])

    runs = report["runs"]
    if type(runs) is not list or not 3 <= len(runs) <= 31:
        raise _invalid()
    copied_runs: list[dict[str, Any]] = []
    for position, value in enumerate(runs):
        run = _object(value, _RUN_KEYS)
        kind = _string(run["kind"])
        iteration = _integer(run["iteration"])
        _string(run["text"])
        _finite_nonnegative(run["model_ms"])
        _finite_nonnegative(run["inference_ms"])
        _finite_nonnegative(run["total_ms"])
        _boolean(run["model_cached"])
        if position == 0:
            if kind != "cold" or iteration != 0:
                raise _invalid()
        elif kind != "warm" or iteration != position:
            raise _invalid()
        copied_runs.append(dict(run))

    copied = dict(report)
    copied["runs"] = copied_runs
    return copied


def parse_quality_report(payload: bytes | bytearray | str) -> dict[str, Any]:
    """Parse bounded UTF-8 JSON and validate its quality-report schema."""

    if isinstance(payload, str):
        try:
            raw = payload.encode("utf-8")
        except UnicodeError:
            raise _invalid() from None
    elif isinstance(payload, (bytes, bytearray)):
        raw = bytes(payload)
    else:
        raise _invalid()
    if len(raw) > MAX_REPORT_BYTES:
        raise _invalid()
    try:
        decoded = raw.decode("utf-8")
        value = json.loads(
            decoded,
            object_pairs_hook=_reject_duplicate_keys,
            parse_constant=_reject_nonfinite_constant,
        )
    except (UnicodeError, json.JSONDecodeError, RecursionError, ValueError):
        raise _invalid() from None
    return validate_quality_report(value)


def read_quality_report(path: str | Path) -> dict[str, Any]:
    """Read at most 16 MiB from a local report and validate it."""

    try:
        with Path(path).open("rb") as handle:
            raw = handle.read(MAX_REPORT_BYTES + 1)
    except (OSError, TypeError, ValueError):
        raise _invalid() from None
    return parse_quality_report(raw)


__all__ = [
    "MAX_REPORT_BYTES",
    "MAX_TEXT_CHARS",
    "parse_quality_report",
    "read_quality_report",
    "validate_quality_report",
]
