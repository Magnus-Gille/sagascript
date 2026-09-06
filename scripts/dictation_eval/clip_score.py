"""Pure scoring of one manifest utterance against a captured quality report."""

from __future__ import annotations

import hashlib
from collections.abc import Mapping
from typing import Any

from corpus_manifest import validate_manifest
from quality_report import validate_quality_report
from text_metrics import score_text


_MEASUREMENT_ENDPOINT = "live_inference_call_not_visible_text"


def _invalid() -> ValueError:
    # Manifest IDs, hashes, paths, and transcript text are all potentially
    # private.  Do not interpolate any caller-controlled value in failures.
    return ValueError("invalid clip scoring input")


def _run_metrics(
    run: Mapping[str, Any],
    reference_text: str,
    specialist_terms: list[str],
    control_terms: list[str],
    *,
    is_silence: bool,
    is_ordinary_control: bool,
) -> dict[str, Any]:
    try:
        text_metrics = score_text(
            reference_text,
            run["text"],
            specialist_terms,
            control_terms,
            is_silence=is_silence,
            is_ordinary_control=is_ordinary_control,
            false_glossary_replacements=None,
        )
    except (TypeError, ValueError):
        raise _invalid() from None
    return {
        "iteration": run["iteration"],
        "text_metrics": text_metrics,
        "total_ms": run["total_ms"],
        "model_ms": run["model_ms"],
        "inference_ms": run["inference_ms"],
        "model_cached": run["model_cached"],
    }


def score_clip(
    manifest: Mapping[str, Any],
    utterance_id: str,
    quality_report: Mapping[str, Any],
    reference_text: str,
    specialist_terms: list[str],
    control_terms: list[str],
) -> dict[str, Any]:
    """Score every captured run for one manifest utterance.

    The first warm run is explicitly exposed as the fixed accuracy-selection
    row.  Later warm runs remain available for stability and control checks,
    but this function never chooses a best transcript and never makes an
    adoption recommendation.
    """

    try:
        validated_manifest = validate_manifest(manifest)
        validated_report = validate_quality_report(quality_report)
    except (TypeError, ValueError):
        raise _invalid() from None

    if type(utterance_id) is not str or type(reference_text) is not str:
        raise _invalid()
    rows = validated_manifest["utterances"]
    assert isinstance(rows, list)
    row = next((candidate for candidate in rows if candidate["id"] == utterance_id), None)
    if row is None:
        raise _invalid()
    if (
        validated_report["language"] != row["language"]
        or validated_report["source_audio_sha256"] != row["audio_sha256"]
    ):
        raise _invalid()
    try:
        reference_digest = hashlib.sha256(reference_text.encode("utf-8")).hexdigest()
    except UnicodeError:
        raise _invalid() from None
    if reference_digest != row["reference_sha256"]:
        raise _invalid()

    tags = row["tags"]
    assert isinstance(tags, list)
    if "specialist" in tags and (type(specialist_terms) is not list or not specialist_terms):
        raise _invalid()
    if ("numbers" in tags or "negation" in tags) and (
        type(control_terms) is not list or not control_terms
    ):
        raise _invalid()

    is_silence = "silence" in tags
    is_ordinary_control = "ordinary" in tags
    runs = validated_report["runs"]
    assert isinstance(runs, list)
    scored_runs = [
        _run_metrics(
            run,
            reference_text,
            specialist_terms,
            control_terms,
            is_silence=is_silence,
            is_ordinary_control=is_ordinary_control,
        )
        for run in runs
    ]
    cold_metrics = scored_runs[0]
    warm_metrics = scored_runs[1:]
    first_warm_accuracy = dict(warm_metrics[0])

    return {
        "schema_version": 1,
        "language": validated_report["language"],
        "model": validated_report["model"],
        "decision": "inconclusive",
        "measurement_endpoint": _MEASUREMENT_ENDPOINT,
        "first_warm_accuracy_iteration": 1,
        "cold": cold_metrics,
        "first_warm_accuracy": first_warm_accuracy,
        "warm_metrics": warm_metrics,
        "warm_text_variants": len({run["text"] for run in runs[1:]}),
    }


__all__ = ["score_clip"]
