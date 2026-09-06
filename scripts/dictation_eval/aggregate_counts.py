"""Bounded, content-free aggregation of one utterance's evaluation counts."""

from __future__ import annotations

import math
from typing import Any

from paired_stats import nearest_rank


_ERROR = "invalid aggregate counts input"
_ROW_KEYS = {
    "reference_words",
    "substitutions",
    "deletions",
    "insertions",
    "cold_total_ms",
    "warm_total_ms",
}
_COUNT_FIELDS = ("reference_words", "substitutions", "deletions", "insertions")


def _invalid() -> ValueError:
    return ValueError(_ERROR)


def _count(value: Any, *, minimum: int, maximum: int) -> int:
    if type(value) is not int or not minimum <= value <= maximum:
        raise _invalid()
    return value


def _milliseconds(value: Any) -> float:
    if type(value) is not int and type(value) is not float:
        raise _invalid()
    try:
        converted = float(value)
    except (OverflowError, ValueError):
        raise _invalid() from None
    if not math.isfinite(converted) or converted < 0.0:
        raise _invalid()
    return converted


def aggregate_counts(rows: list[dict[str, Any]]) -> dict[str, Any]:
    """Aggregate validated per-utterance counts without mutating *rows*.

    The function intentionally accepts only the bounded, content-free fields
    needed by a public summary. Every malformed input fails with the same
    sanitized error so callers cannot accidentally publish input values.
    """

    if type(rows) is not list or not 1 <= len(rows) <= 500:
        raise _invalid()

    checked_rows: list[tuple[dict[str, int], float, list[float]]] = []
    repetitions: int | None = None
    for row in rows:
        if type(row) is not dict or set(row) != _ROW_KEYS:
            raise _invalid()
        counts = {
            "reference_words": _count(row["reference_words"], minimum=1, maximum=2048),
            "substitutions": _count(row["substitutions"], minimum=0, maximum=10_000),
            "deletions": _count(row["deletions"], minimum=0, maximum=10_000),
            "insertions": _count(row["insertions"], minimum=0, maximum=10_000),
        }
        cold = _milliseconds(row["cold_total_ms"])
        warm_value = row["warm_total_ms"]
        if type(warm_value) is not list or not 5 <= len(warm_value) <= 20:
            raise _invalid()
        if repetitions is None:
            repetitions = len(warm_value)
        elif len(warm_value) != repetitions:
            raise _invalid()
        warm = [_milliseconds(value) for value in warm_value]
        checked_rows.append((counts, cold, warm))

    assert repetitions is not None
    reference_words = sum(row[0]["reference_words"] for row in checked_rows)
    substitutions = sum(row[0]["substitutions"] for row in checked_rows)
    deletions = sum(row[0]["deletions"] for row in checked_rows)
    insertions = sum(row[0]["insertions"] for row in checked_rows)
    errors = substitutions + deletions + insertions
    cold_values = [row[1] for row in checked_rows]
    warm_values = [value for row in checked_rows for value in row[2]]

    return {
        "utterances": len(checked_rows),
        "reference_words": reference_words,
        "substitutions": substitutions,
        "deletions": deletions,
        "insertions": insertions,
        "errors": errors,
        "wer": errors / reference_words,
        "cold": {
            "count": len(cold_values),
            "p50_ms": nearest_rank(cold_values, 0.5),
            "p95_ms": nearest_rank(cold_values, 0.95),
        },
        "warm": {
            "count": len(warm_values),
            "repetitions_per_utterance": repetitions,
            "p50_ms": nearest_rank(warm_values, 0.5),
            "p95_ms": nearest_rank(warm_values, 0.95),
        },
    }
