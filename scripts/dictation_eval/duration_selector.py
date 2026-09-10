"""Deterministic, metadata-only duration-bucket selection for evaluation."""

from __future__ import annotations

import hashlib
import re
from typing import Any


_ID_PATTERN = re.compile(r"[A-Za-z0-9_-]{1,80}")
_BUCKETS = ("short", "medium", "long")
_ROW_KEYS = {"id", "duration_ms"}
_QUOTA_KEYS = set(_BUCKETS)


def _invalid() -> ValueError:
    return ValueError("invalid duration selection input")


def _opaque_ascii(value: Any) -> bool:
    return type(value) is str and _ID_PATTERN.fullmatch(value) is not None


def _quota(value: Any) -> bool:
    return type(value) is int and 0 <= value <= 500


def _bucket(duration_ms: int) -> str:
    if duration_ms < 5_000:
        return "short"
    if duration_ms <= 15_000:
        return "medium"
    return "long"


def select_by_duration(
    rows: list[dict[str, Any]],
    quotas: dict[str, int],
    *,
    seed: str,
    split: str,
) -> list[dict[str, Any]]:
    """Select deterministic, independently ranked rows from duration buckets."""

    try:
        if type(rows) is not list or not 1 <= len(rows) <= 5_000:
            raise _invalid()
        if type(quotas) is not dict or set(quotas) != _QUOTA_KEYS:
            raise _invalid()
        if not all(_quota(quotas[bucket]) for bucket in _BUCKETS):
            raise _invalid()
        total_quota = sum(quotas.values())
        if not 1 <= total_quota <= 500:
            raise _invalid()
        if (
            not _opaque_ascii(seed)
            or type(split) is not str
            or split not in {"dev", "heldout"}
        ):
            raise _invalid()

        seen_ids: set[str] = set()
        buckets: dict[str, list[dict[str, Any]]] = {bucket: [] for bucket in _BUCKETS}
        for row in rows:
            if type(row) is not dict or set(row) != _ROW_KEYS:
                raise _invalid()
            row_id = row["id"]
            duration_ms = row["duration_ms"]
            if not _opaque_ascii(row_id) or row_id in seen_ids:
                raise _invalid()
            if type(duration_ms) is not int or not 1 <= duration_ms <= 120_000:
                raise _invalid()
            seen_ids.add(row_id)
            buckets[_bucket(duration_ms)].append(dict(row))

        selected: list[dict[str, Any]] = []
        for bucket in _BUCKETS:
            candidates = buckets[bucket]
            quota = quotas[bucket]
            if len(candidates) < quota:
                raise _invalid()
            ranked = sorted(
                candidates,
                key=lambda row: (
                    hashlib.sha256(
                        f"{seed}:{split}:{row['id']}".encode("utf-8")
                    ).hexdigest(),
                    row["id"],
                ),
            )
            selected.extend(dict(row) for row in ranked[:quota])
        return selected
    except (TypeError, ValueError, UnicodeError, OverflowError):
        raise _invalid() from None


__all__ = ["select_by_duration"]
