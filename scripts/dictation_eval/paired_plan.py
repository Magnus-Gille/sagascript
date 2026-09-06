"""Deterministic local utterance/configuration plans; keep opaque IDs private."""

from __future__ import annotations

import hashlib
import json
import random
import re
from collections.abc import Mapping
from typing import Any

from corpus_manifest import validate_manifest
from quality_report import _MODELS


_CONFIG_KEYS = {
    "id",
    "language",
    "model",
    "beam_size",
    "temperature_fallback",
    "role",
}
_LANGUAGES = {"en", "sv", "no"}
_ROLES = {"baseline", "smaller", "decoder"}
_ID_PATTERN = re.compile(r"[A-Za-z0-9_-]{1,80}")
_SHA40_PATTERN = re.compile(r"[0-9a-f]{40}")
_SHA64_PATTERN = re.compile(r"[0-9a-f]{64}")


def _invalid() -> ValueError:
    # Keep IDs and hashes out of errors; plans can contain private corpus
    # references even though they never contain audio or transcript text.
    return ValueError("invalid paired evaluation plan input")


def _canonical_sha256(value: Any) -> str:
    try:
        encoded = json.dumps(
            value,
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
            allow_nan=False,
        ).encode("utf-8")
    except (TypeError, UnicodeError, ValueError):
        raise _invalid() from None
    return hashlib.sha256(encoded).hexdigest()


def _validate_configurations(value: Any) -> list[dict[str, Any]]:
    if type(value) is not list or not 1 <= len(value) <= 9:
        raise _invalid()
    copied: list[dict[str, Any]] = []
    seen_ids: set[str] = set()
    seen_tuples: set[tuple[Any, ...]] = set()
    role_counts: dict[str, dict[str, int]] = {}
    for config in value:
        if not isinstance(config, Mapping) or set(config) != _CONFIG_KEYS:
            raise _invalid()
        config_id = config["id"]
        if (
            type(config_id) is not str
            or _ID_PATTERN.fullmatch(config_id) is None
            or not config_id.isascii()
            or config_id in seen_ids
        ):
            raise _invalid()
        seen_ids.add(config_id)
        language = config["language"]
        model = config["model"]
        if type(language) is not str or language not in _LANGUAGES:
            raise _invalid()
        if type(model) is not str or model not in _MODELS:
            raise _invalid()
        beam_size = config["beam_size"]
        if type(beam_size) is not int or (beam_size != 0 and not 2 <= beam_size <= 16):
            raise _invalid()
        temperature_fallback = config["temperature_fallback"]
        if type(temperature_fallback) is not bool:
            raise _invalid()
        role = config["role"]
        if type(role) is not str or role not in _ROLES:
            raise _invalid()
        config_tuple = (language, model, beam_size, temperature_fallback)
        if config_tuple in seen_tuples:
            raise _invalid()
        seen_tuples.add(config_tuple)
        language_roles = role_counts.setdefault(language, {})
        language_roles[role] = language_roles.get(role, 0) + 1
        if language_roles[role] > 1:
            raise _invalid()
        copied.append(
            {
                "id": config_id,
                "language": language,
                "model": model,
                "beam_size": beam_size,
                "temperature_fallback": temperature_fallback,
                "role": role,
            }
        )
    return copied


def build_plan(
    manifest: Mapping[str, Any],
    configurations: list[Mapping[str, Any]],
    *,
    split: str,
    seed: int,
    iterations: int,
    source_revision: str,
    binary_sha256: str,
) -> dict[str, Any]:
    """Build a reproducible paired evaluation order without running it."""

    try:
        validated_manifest = validate_manifest(manifest)
    except (TypeError, ValueError):
        raise _invalid() from None
    if type(split) is not str or split not in {"dev", "heldout"}:
        raise _invalid()
    if type(seed) is not int or not 0 <= seed <= 2**32 - 1:
        raise _invalid()
    if type(iterations) is not int or not 5 <= iterations <= 20:
        raise _invalid()
    if type(source_revision) is not str or _SHA40_PATTERN.fullmatch(source_revision) is None:
        raise _invalid()
    if type(binary_sha256) is not str or _SHA64_PATTERN.fullmatch(binary_sha256) is None:
        raise _invalid()
    try:
        validated_configurations = _validate_configurations(configurations)
    except (TypeError, ValueError):
        raise _invalid() from None

    rows = validated_manifest["utterances"]
    assert isinstance(rows, list)
    selected_rows = [row for row in rows if row["split"] == split]
    selected_languages = {row["language"] for row in selected_rows}
    config_languages = {config["language"] for config in validated_configurations}
    if config_languages != selected_languages:
        raise _invalid()
    for language in selected_languages:
        language_configs = [
            config for config in validated_configurations if config["language"] == language
        ]
        if sum(config["role"] == "baseline" for config in language_configs) != 1:
            raise _invalid()

    pairs = [
        {"utterance_id": row["id"], "configuration_id": config["id"]}
        for row in selected_rows
        for config in validated_configurations
        if config["language"] == row["language"]
    ]
    if not pairs or len(pairs) > 1500:
        raise _invalid()
    random.Random(seed).shuffle(pairs)

    return {
        "schema_version": 1,
        "source_revision": source_revision,
        "binary_sha256": binary_sha256,
        "split": split,
        "seed": seed,
        "iterations": iterations,
        "manifest_sha256": _canonical_sha256(validated_manifest),
        "configurations_sha256": _canonical_sha256(validated_configurations),
        "configurations": validated_configurations,
        "order": pairs,
    }


__all__ = ["build_plan"]
