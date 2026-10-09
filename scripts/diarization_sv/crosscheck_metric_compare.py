"""Pure-stdlib comparison helpers for offline metric cross-check receipts."""

import math


MISSING = object()


def compare_metric(actual, expected, tolerance=1e-8):
    """Return ``(difference, passed)`` while preserving undefined metrics.

    ``None`` is the serialized representation of an undefined normalized
    metric, such as DER when reference speaker-time is zero.  A matching
    ``None`` passes; a null/non-null mismatch is represented by a JSON-safe
    detail object so the receipt explains the failure instead of raising from
    arithmetic on ``None``.
    """
    if expected is None:
        if actual is None:
            return None, True
        return {
            "expected": None,
            "actual": "missing" if actual is MISSING else actual,
            "reason": "expected undefined metric (null)",
        }, False

    if actual is None or actual is MISSING:
        return {
            "expected": expected,
            "actual": "missing" if actual is MISSING else None,
            "reason": "expected numeric metric, received null",
        }, False

    if (
        isinstance(expected, bool)
        or not isinstance(expected, (int, float))
        or not math.isfinite(expected)
        or isinstance(actual, bool)
        or not isinstance(actual, (int, float))
        or not math.isfinite(actual)
    ):
        return {
            "expected": expected,
            "actual": actual,
            "reason": "metric values must be finite numbers or null",
        }, False

    difference = float(abs(actual - expected))
    return difference, bool(difference < tolerance)
