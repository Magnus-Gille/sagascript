"""Pure validation and argv construction for the private paired runner."""

from __future__ import annotations

import copy
from pathlib import Path
from collections.abc import Mapping
from typing import Any

from paired_plan import _validate_configurations, build_plan


_PLAN_KEYS = {
    "schema_version",
    "source_revision",
    "binary_sha256",
    "split",
    "seed",
    "iterations",
    "manifest_sha256",
    "configurations_sha256",
    "configurations",
    "order",
}


def _invalid() -> ValueError:
    return ValueError("invalid paired runner input")


def _strict_equal(left: Any, right: Any) -> bool:
    if type(left) is not type(right):
        return False
    if type(left) is dict:
        return set(left) == set(right) and all(
            _strict_equal(left[key], right[key]) for key in left
        )
    if type(left) is list:
        return len(left) == len(right) and all(
            _strict_equal(item_left, item_right)
            for item_left, item_right in zip(left, right)
        )
    return left == right


def validate_plan(manifest: Mapping[str, Any], plan: Mapping[str, Any]) -> dict[str, Any]:
    """Validate and copy a frozen plan without changing its authority."""

    if type(plan) is not dict or set(plan) != _PLAN_KEYS:
        raise _invalid()
    if type(plan["schema_version"]) is not int or plan["schema_version"] != 1:
        raise _invalid()
    if type(plan["configurations"]) is not list or type(plan["order"]) is not list:
        raise _invalid()
    try:
        expected = build_plan(
            manifest,
            plan["configurations"],
            split=plan["split"],
            seed=plan["seed"],
            iterations=plan["iterations"],
            source_revision=plan["source_revision"],
            binary_sha256=plan["binary_sha256"],
        )
    except (TypeError, ValueError):
        raise _invalid() from None
    if not _strict_equal(plan, expected):
        raise _invalid()
    return copy.deepcopy(plan)


def _absolute_path(value: Any) -> str:
    if type(value) is not str or not value or "\x00" in value:
        raise _invalid()
    try:
        if not Path(value).is_absolute():
            raise _invalid()
    except (TypeError, ValueError, OSError):
        raise _invalid() from None
    return value


def build_command(
    binary: str,
    audio: str,
    report: str,
    config: Mapping[str, Any],
    iterations: int,
    is_silence: bool,
) -> list[str]:
    """Build literal benchmark argv for one already-selected configuration."""

    binary = _absolute_path(binary)
    audio = _absolute_path(audio)
    report = _absolute_path(report)
    if type(iterations) is not int or not 5 <= iterations <= 20:
        raise _invalid()
    if type(is_silence) is not bool:
        raise _invalid()
    try:
        validated_config = _validate_configurations([config])[0]
    except (TypeError, ValueError, IndexError):
        raise _invalid() from None

    command = [
        binary,
        "benchmark-dictation",
        audio,
        "--language",
        validated_config["language"],
        "--model",
        validated_config["model"],
        "--beam-size",
        str(validated_config["beam_size"]),
        "--iterations",
        str(iterations),
        "--quality-output",
        report,
    ]
    if not validated_config["temperature_fallback"]:
        command.append("--disable-temperature-fallback")
    if is_silence:
        command.append("--allow-empty")
    return command


__all__ = ["build_command", "validate_plan"]
