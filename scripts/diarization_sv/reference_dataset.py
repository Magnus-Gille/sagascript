#!/usr/bin/env python3
"""Conservative, reviewable reference intervals for diarization evaluation.

The input is deliberately small: a source hash, duration, known speaker IDs,
optional whole recording windows, and interval proposals.  Only explicitly
human-reviewed ``verified`` intervals are exported for scoring.  Candidate and
unknown intervals remain useful annotation context but can never become scored
speech implicitly.
"""

from __future__ import annotations

import argparse
import datetime as _datetime
import hashlib
import json
import math
import os
import re
import sys
from pathlib import Path
from typing import Any, Iterable


SOURCE_HASH_RE = re.compile(r"[0-9a-f]{64}\Z")
SPLITS = ("train", "dev", "eval")
TOP_LEVEL_KEYS = {"source_sha256", "duration_seconds", "speakers", "windows", "intervals"}
INTERVAL_KEYS = {
    "start",
    "end",
    "speakers",
    "activity",
    "status",
    "evidence",
    "reviewer",
    "reviewed_at",
    "window_id",
}
WINDOW_KEYS = {"id", "start", "end"}
EVIDENCE_KEYS = {"kind", "artifact"}


class ReferenceError(ValueError):
    """A user-facing schema or consistency error."""


def finite_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def nonempty_identifier(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value or any(char.isspace() for char in value):
        raise ReferenceError(f"{label} must be a non-empty identifier without whitespace")
    return value


def nonempty_string(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ReferenceError(f"{label} must be a non-empty string")
    return value


def parse_review_timestamp(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise ReferenceError(f"{label} is required for verified intervals")
    candidate = value[:-1] + "+00:00" if value.endswith("Z") else value
    try:
        parsed = _datetime.datetime.fromisoformat(candidate)
    except ValueError as exc:
        raise ReferenceError(f"{label} must be an ISO-8601 timestamp") from exc
    if parsed.tzinfo is None:
        raise ReferenceError(f"{label} must include a timezone")
    return value


def validate_evidence(value: Any, *, required: bool) -> list[dict[str, Any]]:
    if value is None:
        if required:
            raise ReferenceError("verified intervals require non-empty evidence")
        return []
    if not isinstance(value, list) or (required and not value):
        raise ReferenceError("evidence must be a non-empty list for verified intervals")
    validated: list[dict[str, Any]] = []
    for index, item in enumerate(value, start=1):
        if not isinstance(item, dict):
            raise ReferenceError(f"evidence item {index} must be an object")
        missing = EVIDENCE_KEYS - item.keys()
        if missing:
            raise ReferenceError(f"evidence item {index} is missing: {', '.join(sorted(missing))}")
        nonempty_identifier(item["kind"], f"evidence item {index} kind")
        if not isinstance(item["artifact"], str) or not item["artifact"]:
            raise ReferenceError(f"evidence item {index} artifact must be a non-empty string")
        validated.append(item)
    return validated


def _interval_window(interval: dict[str, Any], windows: list[dict[str, Any]]) -> str | None:
    explicit = interval.get("window_id")
    if explicit is not None:
        return explicit
    containing = [window["id"] for window in windows if interval["start"] >= window["start"] and interval["end"] <= window["end"]]
    if len(containing) == 1:
        return containing[0]
    return None


def validate_reference(document: Any) -> dict[str, Any]:
    if not isinstance(document, dict):
        raise ReferenceError("reference must be a JSON object")
    unknown = set(document) - TOP_LEVEL_KEYS
    if unknown:
        raise ReferenceError(f"unsupported top-level keys: {', '.join(sorted(str(k) for k in unknown))}")

    source_sha256 = document.get("source_sha256")
    if not isinstance(source_sha256, str) or not SOURCE_HASH_RE.fullmatch(source_sha256):
        raise ReferenceError("source_sha256 must be 64 lowercase hexadecimal characters")
    duration = document.get("duration_seconds")
    if not finite_number(duration) or duration <= 0:
        raise ReferenceError("duration_seconds must be a finite number greater than zero")
    duration = float(duration)

    speakers = document.get("speakers")
    if not isinstance(speakers, list):
        raise ReferenceError("speakers must be a list of known IDs")
    known_speakers: list[str] = []
    for value in speakers:
        speaker = nonempty_identifier(value, "speaker ID")
        if speaker in known_speakers:
            raise ReferenceError(f"duplicate speaker ID: {speaker}")
        known_speakers.append(speaker)
    known_set = set(known_speakers)

    raw_windows = document.get("windows", [])
    if not isinstance(raw_windows, list):
        raise ReferenceError("windows must be a list when present")
    windows: list[dict[str, Any]] = []
    window_ids: set[str] = set()
    for index, raw in enumerate(raw_windows, start=1):
        if not isinstance(raw, dict):
            raise ReferenceError(f"window {index} must be an object")
        if set(raw) - WINDOW_KEYS:
            raise ReferenceError(f"window {index} has unsupported keys")
        window_id = nonempty_identifier(raw.get("id"), f"window {index} id")
        if window_id in window_ids:
            raise ReferenceError(f"duplicate window ID: {window_id}")
        start = raw.get("start")
        end = raw.get("end")
        if not finite_number(start) or not finite_number(end) or not 0 <= start < end <= duration:
            raise ReferenceError(f"window {index} must satisfy 0 <= start < end <= duration_seconds")
        window_ids.add(window_id)
        windows.append({"id": window_id, "start": float(start), "end": float(end)})
    for left_index, left in enumerate(windows):
        for right in windows[left_index + 1 :]:
            if max(left["start"], right["start"]) < min(left["end"], right["end"]):
                raise ReferenceError("overlapping windows are not allowed")

    intervals = document.get("intervals")
    if not isinstance(intervals, list):
        raise ReferenceError("intervals must be a list")
    validated: list[dict[str, Any]] = []
    for index, raw in enumerate(intervals, start=1):
        if not isinstance(raw, dict):
            raise ReferenceError(f"interval {index} must be an object")
        if set(raw) - INTERVAL_KEYS:
            raise ReferenceError(f"interval {index} has unsupported keys")
        start = raw.get("start")
        end = raw.get("end")
        if not finite_number(start) or not finite_number(end) or not 0 <= start < end <= duration:
            raise ReferenceError(f"interval {index} must satisfy 0 <= start < end <= duration_seconds")
        status = raw.get("status")
        if status not in {"candidate", "verified", "unknown"}:
            raise ReferenceError(f"interval {index} status must be candidate, verified, or unknown")
        activity = raw.get("activity", "speech") if status != "unknown" else raw.get("activity")
        if activity is not None and activity not in {"speech", "silence"}:
            raise ReferenceError(f"interval {index} activity must be speech or silence")
        interval_speakers = raw.get("speakers")
        if not isinstance(interval_speakers, list):
            raise ReferenceError(f"interval {index} speakers must be a list")
        normalized_speakers: list[str] = []
        for speaker in interval_speakers:
            speaker_id = nonempty_identifier(speaker, f"interval {index} speaker ID")
            if speaker_id not in known_set:
                raise ReferenceError(f"interval {index} names unknown speaker: {speaker_id}")
            if speaker_id in normalized_speakers:
                raise ReferenceError(f"interval {index} repeats speaker: {speaker_id}")
            normalized_speakers.append(speaker_id)
        if status == "unknown" and normalized_speakers:
            raise ReferenceError(f"unknown interval {index} must have no speakers")
        if status in {"candidate", "verified"}:
            if activity == "speech" and not normalized_speakers:
                raise ReferenceError(f"{status} speech interval {index} must name at least one speaker")
            if activity == "silence" and normalized_speakers:
                raise ReferenceError(f"{status} silence interval {index} must have no speakers")

        evidence = validate_evidence(raw.get("evidence"), required=status == "verified")
        reviewer = raw.get("reviewer")
        reviewed_at = raw.get("reviewed_at")
        if status == "verified":
            reviewer = nonempty_string(reviewer, f"verified interval {index} reviewer")
            if reviewer.strip().lower() in {"model", "system", "automatic", "auto", "consensus", "assistant", "unknown"}:
                raise ReferenceError(f"verified interval {index} reviewer must identify a human")
            reviewed_at = parse_review_timestamp(reviewed_at, f"verified interval {index} reviewed_at")
        elif reviewer is not None:
            reviewer = nonempty_string(reviewer, f"interval {index} reviewer")
        if reviewed_at is not None and status != "verified":
            parse_review_timestamp(reviewed_at, f"interval {index} reviewed_at")

        window_id = raw.get("window_id")
        if window_id is not None:
            window_id = nonempty_identifier(window_id, f"interval {index} window_id")
            if window_id not in window_ids:
                raise ReferenceError(f"interval {index} names unknown window: {window_id}")
            window = next(window for window in windows if window["id"] == window_id)
            if not window["start"] <= start or not end <= window["end"]:
                raise ReferenceError(f"interval {index} is outside its window")
        elif windows:
            containing = _interval_window({"start": float(start), "end": float(end)}, windows)
            if containing is None:
                raise ReferenceError(f"interval {index} must fit exactly one supplied window")
            window_id = containing

        normalized = dict(raw)
        normalized.update(
            start=float(start),
            end=float(end),
            speakers=normalized_speakers,
            evidence=evidence,
        )
        if activity is not None:
            normalized["activity"] = activity
        if window_id is not None:
            normalized["window_id"] = window_id
        validated.append(normalized)

    verified = [item for item in validated if item["status"] == "verified"]
    for left_index, left in enumerate(verified):
        left_set = set(left["speakers"])
        for right in verified[left_index + 1 :]:
            overlap_start = max(left["start"], right["start"])
            overlap_end = min(left["end"], right["end"])
            if overlap_start < overlap_end and left_set != set(right["speakers"]):
                # Simultaneous speech is represented once by a multi-speaker set;
                # a second, different verified set is an incompatible claim.
                raise ReferenceError("incompatible verified speaker overlap")

    summary = summarize(validated, source_sha256, duration, known_speakers, windows)
    return {
        "source_sha256": source_sha256,
        "duration_seconds": duration,
        "speakers": known_speakers,
        "windows": windows,
        "intervals": validated,
        "summary": summary,
    }


def merge_regions(regions: Iterable[tuple[float, float]]) -> list[tuple[float, float]]:
    merged: list[tuple[float, float]] = []
    for start, end in sorted(regions):
        if merged and start <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(merged[-1][1], end))
        else:
            merged.append((start, end))
    return merged


def subtract_regions(base: Iterable[tuple[float, float]], holes: Iterable[tuple[float, float]]) -> list[tuple[float, float]]:
    hole_regions = merge_regions(holes)
    output: list[tuple[float, float]] = []
    for start, end in merge_regions(base):
        cursor = start
        for hole_start, hole_end in hole_regions:
            if hole_end <= cursor:
                continue
            if hole_start >= end:
                break
            if hole_start > cursor:
                output.append((cursor, min(hole_start, end)))
            cursor = max(cursor, hole_end)
            if cursor >= end:
                break
        if cursor < end:
            output.append((cursor, end))
    return [(start, end) for start, end in output if start < end]


def export_intervals(reference: dict[str, Any]) -> tuple[list[str], list[str], dict[str, Any]]:
    intervals = reference["intervals"]
    verified = [item for item in intervals if item["status"] == "verified"]
    coalesced: list[dict[str, Any]] = []
    for item in sorted(verified, key=lambda value: (value.get("window_id", "__whole__"), value["start"], value["end"], tuple(value["speakers"]))):
        if (
            coalesced
            and coalesced[-1].get("window_id", "__whole__") == item.get("window_id", "__whole__")
            and coalesced[-1]["end"] == item["start"]
            and coalesced[-1]["speakers"] == item["speakers"]
            and coalesced[-1].get("activity", "speech") == item.get("activity", "speech")
        ):
            coalesced[-1]["end"] = item["end"]
        else:
            coalesced.append(dict(item))

    rttm_lines: list[str] = []
    for item in sorted(coalesced, key=lambda value: (value["start"], value["end"], tuple(value["speakers"]))):
        duration = item["end"] - item["start"]
        for speaker in item["speakers"]:
            rttm_lines.append(
                f"SPEAKER {reference['source_sha256']} 1 {item['start']:.9f} {duration:.9f} <NA> <NA> {speaker} <NA> <NA>"
            )

    scored = subtract_regions(
        ((item["start"], item["end"]) for item in verified),
        ((item["start"], item["end"]) for item in intervals if item["status"] == "unknown"),
    )
    uem_lines = [
        f"{reference['source_sha256']} 1 {start:.9f} {end:.9f}" for start, end in scored
    ]
    export_summary = dict(reference["summary"])
    export_summary.update(
        {
            "verified_rttm_turn_count": len(rttm_lines),
            "verified_interval_count_after_coalesce": len(coalesced),
            "scored_uem_region_count": len(uem_lines),
            "scored_seconds": round(sum(end - start for start, end in scored), 9),
            "scored_coverage_ratio": round(sum(end - start for start, end in scored) / reference["duration_seconds"], 9),
        }
    )
    return [line + "\n" for line in rttm_lines], [line + "\n" for line in uem_lines], export_summary


def summarize(intervals: list[dict[str, Any]], source_sha256: str, duration: float, speakers: list[str], windows: list[dict[str, Any]]) -> dict[str, Any]:
    counts = {status: 0 for status in ("verified", "candidate", "unknown")}
    seconds = {status: 0.0 for status in counts}
    activity_counts = {activity: 0 for activity in ("speech", "silence")}
    activity_seconds = {activity: 0.0 for activity in activity_counts}
    for item in intervals:
        counts[item["status"]] += 1
        seconds[item["status"]] += item["end"] - item["start"]
        activity = item.get("activity")
        if activity is None and item["status"] != "unknown":
            activity = "speech"
        if activity in activity_counts:
            activity_counts[activity] += 1
            activity_seconds[activity] += item["end"] - item["start"]
    verified_union_seconds = sum(
        end - start
        for start, end in merge_regions(
            (item["start"], item["end"]) for item in intervals if item["status"] == "verified"
        )
    )
    return {
        "valid": True,
        "source_sha256": source_sha256,
        "duration_seconds": duration,
        "known_speaker_count": len(speakers),
        "window_count": len(windows),
        "interval_count": len(intervals),
        "interval_counts": counts,
        "interval_seconds": {key: round(value, 9) for key, value in seconds.items()},
        "activity_counts": activity_counts,
        "activity_seconds": {key: round(value, 9) for key, value in activity_seconds.items()},
        "verified_coverage_ratio": round(verified_union_seconds / duration, 9),
    }


def read_reference(path: Path) -> dict[str, Any]:
    try:
        return validate_reference(json.loads(path.read_text(encoding="utf-8")))
    except (OSError, json.JSONDecodeError) as exc:
        raise ReferenceError(f"cannot read reference JSON: {path}") from exc


def write_new(path: Path, content: str) -> None:
    if path.exists():
        raise ReferenceError(f"refusing to overwrite existing export: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8") as output:
        output.write(content)


def ratio(value: str) -> float:
    try:
        parsed = float(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("split ratios must be finite numbers") from exc
    if not math.isfinite(parsed) or parsed < 0:
        raise argparse.ArgumentTypeError("split ratios must be finite and non-negative")
    return parsed


def split_windows(reference: dict[str, Any], train: float, dev: float, evaluation: float, seed: str) -> dict[str, Any]:
    if not reference["windows"]:
        raise ReferenceError("split requires supplied windows")
    if not math.isclose(train + dev + evaluation, 1.0, rel_tol=0.0, abs_tol=1e-9):
        raise ReferenceError("train, dev, and eval ratios must sum to 1")
    ranked = sorted(
        reference["windows"],
        key=lambda window: hashlib.sha256(f"{reference['source_sha256']}\0{seed}\0{window['id']}".encode()).hexdigest(),
    )
    requested = [len(ranked) * train, len(ranked) * dev, len(ranked) * evaluation]
    counts = [int(value) for value in requested]
    for index in sorted(range(3), key=lambda item: (requested[item] - counts[item], -item), reverse=True)[: len(ranked) - sum(counts)]:
        counts[index] += 1
    assignments: list[dict[str, Any]] = []
    cursor = 0
    for split, count in zip(SPLITS, counts):
        for window in ranked[cursor : cursor + count]:
            assignments.append({"id": window["id"], "start": window["start"], "end": window["end"], "split": split})
        cursor += count
    assignments.sort(key=lambda window: window["id"])
    return {
        "source_sha256": reference["source_sha256"],
        "seed": seed,
        "ratios": {"train": train, "dev": dev, "eval": evaluation},
        "windows": assignments,
        "counts": {split: sum(item["split"] == split for item in assignments) for split in SPLITS},
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Validate and export conservative diarization references")
    sub = parser.add_subparsers(dest="command", required=True)

    validate = sub.add_parser("validate", help="validate a reference and print numeric summary")
    validate.add_argument("input", type=Path)

    export = sub.add_parser("export", help="export verified intervals as RTTM and UEM")
    export.add_argument("input", type=Path)
    export.add_argument("--rttm", required=True, type=Path)
    export.add_argument("--uem", required=True, type=Path)
    export.add_argument("--summary", type=Path)

    split = sub.add_parser("split", help="assign whole supplied windows deterministically")
    split.add_argument("input", type=Path)
    split.add_argument("--output", required=True, type=Path)
    split.add_argument("--seed", required=True)
    split.add_argument("--train", "--train-ratio", required=True, dest="train", type=ratio)
    split.add_argument("--dev", "--dev-ratio", required=True, dest="dev", type=ratio)
    split.add_argument("--eval", "--eval-ratio", required=True, dest="evaluation", type=ratio)
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        reference = read_reference(args.input)
        if args.command == "validate":
            print(json.dumps(reference["summary"], sort_keys=True, indent=2))
            return 0
        if args.command == "export":
            rttm, uem, summary = export_intervals(reference)
            paths = [args.rttm, args.uem] + ([args.summary] if args.summary else [])
            if len({path.resolve() for path in paths}) != len(paths):
                raise ReferenceError("export paths must be distinct")
            if any(path.exists() for path in paths):
                raise ReferenceError("refusing to overwrite existing export")
            write_new(args.rttm, "".join(rttm))
            write_new(args.uem, "".join(uem))
            if args.summary:
                write_new(args.summary, json.dumps(summary, sort_keys=True, indent=2) + "\n")
            print(json.dumps(summary, sort_keys=True, indent=2))
            return 0
        split = split_windows(reference, args.train, args.dev, args.evaluation, args.seed)
        write_new(args.output, json.dumps(split, sort_keys=True, indent=2) + "\n")
        print(json.dumps({"valid": True, "window_count": len(split["windows"]), "counts": split["counts"]}, sort_keys=True))
        return 0
    except (OSError, ReferenceError) as exc:
        parser.error(str(exc))
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
