"""Explicit local paired execution. Reports and diagnostics stay in a new private directory.

The operator must supply a trusted Sagascript executable and locally installed
models. A digest binds that executable; this is not an OS/network sandbox or a
proof that the declared source revision produced the binary.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess

from clip_score import score_clip
from corpus_manifest import validate_manifest
from normalization import normalize_text
from paired_plan import build_plan
from quality_report import read_quality_report
from runner_plan import build_command, validate_plan
from text_metrics import score_text


MAX_AUDIO_BYTES = 128 * 1024 * 1024
MAX_BINARY_BYTES = 256 * 1024 * 1024
MAX_REFERENCE_BYTES = 400_000
MAX_JSON_BYTES = 16 * 1024 * 1024


def _invalid():
    return ValueError("invalid local paired execution input")


def _path(value):
    if not isinstance(value, (str, Path)) or "\0" in str(value):
        raise _invalid()
    result = Path(value)
    if not result.is_absolute():
        raise _invalid()
    return result


def _file_digest(path, limit):
    """Bound reads and reject special files; no adversarial filesystem attestation."""
    try:
        if not stat.S_ISREG(path.lstat().st_mode):
            raise _invalid()
        digest = hashlib.sha256()
        count = 0
        with path.open("rb") as handle:
            if not stat.S_ISREG(os.fstat(handle.fileno()).st_mode):
                raise _invalid()
            while chunk := handle.read(32 * 1024):
                count += len(chunk)
                if count > limit:
                    raise _invalid()
                digest.update(chunk)
        return digest.hexdigest()
    except (OSError, ValueError):
        raise _invalid() from None


def _write_new(path, value):
    encoded = (json.dumps(value, ensure_ascii=False, allow_nan=False,
                          sort_keys=True, indent=2) + "\n").encode("utf-8")
    if len(encoded) > MAX_JSON_BYTES:
        raise _invalid()
    # Serialize before exclusive creation. Keep a partial file on write failure;
    # never unlink a name that another process could have replaced.
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as handle:
        handle.write(encoded)
        handle.flush()
        os.fsync(handle.fileno())


def freeze_plan(manifest, configurations, *, split, seed, iterations,
                source_revision, binary_path, output_path):
    binary = _path(binary_path)
    output = _path(output_path)
    plan = build_plan(manifest, configurations, split=split, seed=seed,
                      iterations=iterations, source_revision=source_revision,
                      binary_sha256=_file_digest(binary, MAX_BINARY_BYTES))
    try:
        _write_new(output, plan)
    except (OSError, ValueError, TypeError, UnicodeError):
        raise _invalid() from None
    return {"schema_version": 1, "planned": len(plan["order"]),
            "decision": "inconclusive", "plan_written": True}


def _inputs(manifest, plan, audio_map, reference_map, terms_map):
    rows = {row["id"]: row for row in manifest["utterances"]
            if row["split"] == plan["split"]}
    for mapping in (audio_map, reference_map, terms_map):
        if type(mapping) is not dict or set(mapping) != set(rows):
            raise _invalid()
    prepared = {}
    for identifier, row in rows.items():
        audio, reference = _path(audio_map[identifier]), _path(reference_map[identifier])
        if _file_digest(audio, MAX_AUDIO_BYTES) != row["audio_sha256"]:
            raise _invalid()
        # Freeze exact reference bytes in memory; later path edits cannot change
        # the selected ground truth during a run.
        if not stat.S_ISREG(reference.lstat().st_mode):
            raise _invalid()
        with reference.open("rb") as handle:
            raw = handle.read(MAX_REFERENCE_BYTES + 1)
        if len(raw) > MAX_REFERENCE_BYTES or hashlib.sha256(raw).hexdigest() != row["reference_sha256"]:
            raise _invalid()
        text = raw.decode("utf-8")
        if row["origin"] != "silence" and not normalize_text(text):
            raise _invalid()
        terms = terms_map[identifier]
        if type(terms) is not dict or set(terms) != {"specialist_terms", "control_terms"}:
            raise _invalid()
        specialist, controls = terms["specialist_terms"], terms["control_terms"]
        # Reuse bounded term/reference validation before launching any child.
        score_text(text, "", specialist, controls, is_silence="silence" in row["tags"],
                   is_ordinary_control="ordinary" in row["tags"], false_glossary_replacements=None)
        if "specialist" in row["tags"] and not specialist:
            raise _invalid()
        if ({"numbers", "negation"} & set(row["tags"])) and not controls:
            raise _invalid()
        prepared[identifier] = (row, audio, text, list(specialist), list(controls))
    return prepared


def run_evaluation(manifest, plan, audio_map, reference_map, terms_map,
                   binary_path, output_dir, timeout_seconds=900):
    """Execute all frozen pairs, preserving per-pair failure and scoring evidence.

    No existing output is reused. Preflight failures create nothing. Interruption
    or disk errors may leave a clearly partial directory; there is no implicit
    resume or cleanup. Stderr is retained privately, never echoed publicly.
    """
    try:
        if type(timeout_seconds) is not int or not 1 <= timeout_seconds <= 3600:
            raise _invalid()
        manifest = validate_manifest(manifest)
        plan = validate_plan(manifest, plan)
        binary, output = _path(binary_path), _path(output_dir)
        if _file_digest(binary, MAX_BINARY_BYTES) != plan["binary_sha256"]:
            raise _invalid()
        prepared = _inputs(manifest, plan, audio_map, reference_map, terms_map)
        # mkdir is exclusive and does not create missing parents. Unix0700;
        # Windows relies on the operator-selected parent's ACL.
        output.mkdir(mode=0o700)
    except (OSError, ValueError, TypeError, UnicodeError, OverflowError, RecursionError):
        raise _invalid() from None

    _write_new(output / "plan.json", plan)
    _write_new(output / "manifest.json", manifest)
    configurations = {value["id"]: value for value in plan["configurations"]}
    completed = 0
    statuses = {}
    identity_failed = False
    for index, pair in enumerate(plan["order"]):
        row, audio, reference, specialist, controls = prepared[pair["utterance_id"]]
        config = configurations[pair["configuration_id"]]
        report_path = output / f"{index:05d}-report.json"
        result = {"schema_version": 1, "index": index, **pair,
                  "status": "not_attempted", "exit_code": None, "score": None,
                  "source_audio_sha256_before": None, "source_audio_sha256_after": None}
        try:
            if identity_failed:
                _write_new(output / f"{index:05d}-result.json", result)
                statuses["not_attempted"] = statuses.get("not_attempted", 0) + 1
                continue
            before = _file_digest(audio, MAX_AUDIO_BYTES)
            result["source_audio_sha256_before"] = before
            if (before != row["audio_sha256"]
                    or _file_digest(binary, MAX_BINARY_BYTES) != plan["binary_sha256"]):
                identity_failed = True
                result["status"] = "identity_changed"
            else:
                command = build_command(str(binary), str(audio), str(report_path), config,
                                        plan["iterations"], "silence" in row["tags"])
                descriptor = os.open(output / f"{index:05d}-stderr.log",
                                     os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                with os.fdopen(descriptor, "wb") as diagnostic:
                    try:
                        process = subprocess.run(command, stdin=subprocess.DEVNULL,
                                                 stdout=subprocess.DEVNULL, stderr=diagnostic,
                                                 timeout=timeout_seconds, check=False)
                        result["exit_code"] = process.returncode
                        result["status"] = "completed" if process.returncode == 0 else "cli_failed"
                    except subprocess.TimeoutExpired:
                        result["status"] = "timeout"
                    except OSError:
                        result["status"] = "launch_failed"
                after = _file_digest(audio, MAX_AUDIO_BYTES)
                result["source_audio_sha256_after"] = after
                if (after != row["audio_sha256"]
                        or _file_digest(binary, MAX_BINARY_BYTES) != plan["binary_sha256"]):
                    identity_failed = True
                    result["status"] = "identity_changed"
                if result["status"] in {"completed", "cli_failed"}:
                    try:
                        report = read_quality_report(report_path)
                        if (any(report[key] != config[key] for key in
                                ("model", "language", "beam_size", "temperature_fallback"))
                                or report["allow_empty"] != ("silence" in row["tags"])
                                or len(report["runs"]) != plan["iterations"] + 1):
                            raise _invalid()
                        result["score"] = score_clip(manifest, row["id"], report,
                                                     reference, specialist, controls)
                        if not report["cli_checks_passed"]:
                            result["status"] = "cli_failed"
                    except (ValueError, TypeError, UnicodeError, OverflowError, RecursionError):
                        result["status"] = "invalid_report"
        except (OSError, ValueError):
            # Preserve a fixed failure category and the private child diagnostic,
            # never interpolate paths or transcript-bearing exception messages.
            result["status"] = "runner_failed"
            identity_failed = True
        _write_new(output / f"{index:05d}-result.json", result)
        completed += result["status"] == "completed"
        statuses[result["status"]] = statuses.get(result["status"], 0) + 1

    summary = {"schema_version": 1, "decision": "inconclusive",
               "measurement_endpoint": "live_inference_call_not_visible_text",
               "planned": len(plan["order"]), "completed": completed,
               "failed": len(plan["order"]) - completed, "statuses": statuses}
    _write_new(output / "summary.json", summary)
    return summary


__all__ = ["freeze_plan", "run_evaluation"]
