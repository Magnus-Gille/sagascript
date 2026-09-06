"""Read-only, content-free aggregation of complete private runner ledgers.

Re-score hash-bound references rather than trusting stored metric values. This
checks internal consistency, not authenticity of operator-owned evidence files.
No binary/model/audio is executed or re-attested here; no adoption is inferred.
"""

import json
import os
from pathlib import Path
import platform
import stat
import unicodedata

from aggregate_counts import aggregate_counts
from clip_score import score_clip
from corpus_manifest import coverage_report, validate_manifest
from paired_accuracy import paired_wer_interval
from paired_stats import paired_cluster_interval
from quality_report import validate_quality_report
from runner_plan import _strict_equal, validate_plan

_RESULT_KEYS = {"schema_version", "index", "utterance_id", "configuration_id", "status",
                "exit_code", "score", "source_audio_sha256_before", "source_audio_sha256_after"}
_STATUSES = {"completed", "cli_failed", "timeout", "launch_failed", "identity_changed",
             "invalid_report", "runner_failed", "not_attempted"}


def _invalid():
    return ValueError("invalid private run summary input")


def _pairs(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise _invalid()
        result[key] = value
    return result


def _constant(_value):
    raise _invalid()


def _read(path, limit=16 * 1024 * 1024):
    path = Path(path)
    if not path.is_absolute() or not stat.S_ISREG(path.lstat().st_mode):
        raise _invalid()
    with path.open("rb") as handle:
        if not stat.S_ISREG(os.fstat(handle.fileno()).st_mode):
            raise _invalid()
        data = handle.read(limit + 1)
    if len(data) > limit:
        raise _invalid()
    return data.decode("utf-8")


def _json(path):
    return json.loads(_read(path), object_pairs_hook=_pairs, parse_constant=_constant)


def _count_row(score):
    accuracy = score["first_warm_accuracy"]["text_metrics"]
    return {**{key: accuracy[key] for key in
               ("reference_words", "substitutions", "deletions", "insertions")},
            "cold_total_ms": score["cold"]["total_ms"],
            "warm_total_ms": [row["total_ms"] for row in score["warm_metrics"]]}


def _controls(scores, rows):
    metrics = [score["first_warm_accuracy"]["text_metrics"] for score in scores.values()]
    expected = sum(row["specialist_expected"] for row in metrics)
    recalled = sum(row["specialist_recalled"] for row in metrics)
    return {"silence_utterances": sum(rows[key]["origin"] == "silence" for key in scores),
            "first_warm_silence_hallucinations": sum(row["silence_hallucination"] for row in metrics),
            "first_warm_control_errors": sum(row["control_errors"] for row in metrics),
            "specialist_expected": expected, "specialist_recalled": recalled,
            "specialist_recall": recalled / expected if expected else None,
            "false_glossary_replacements": None,
            "false_replacements_per_1000_control_words": None}


def summarize_run(manifest, plan, reference_map, terms_map, output_dir):
    """Require every planned ledger row; never silently drop failed pairs.

    A config with any failed pair has no quality/latency metrics. Silence and
    synthetic speech never enter the human-speech WER or latency comparison.
    The public output deliberately excludes all caller-chosen opaque IDs.
    """
    try:
        return _summarize(manifest, plan, reference_map, terms_map, output_dir)
    except (OSError, ValueError, TypeError, KeyError, UnicodeError, OverflowError, RecursionError):
        raise _invalid() from None


def _summarize(manifest, plan, reference_map, terms_map, output_dir):
    manifest = validate_manifest(manifest)
    plan = validate_plan(manifest, plan)
    root = Path(output_dir)
    if not root.is_absolute() or not stat.S_ISDIR(root.lstat().st_mode):
        raise _invalid()
    if not _strict_equal(_json(root / "manifest.json"), manifest) or not _strict_equal(_json(root / "plan.json"), plan):
        raise _invalid()
    rows = {row["id"]: row for row in manifest["utterances"] if row["split"] == plan["split"]}
    for mapping in (reference_map, terms_map):
        if type(mapping) is not dict or set(mapping) != set(rows):
            raise _invalid()
    references = {key: _read(Path(reference_map[key]), 400_000) for key in rows}
    expected_names = {f"{index:05d}-result.json" for index in range(len(plan["order"]))}
    if {p.name for p in root.iterdir() if p.name.endswith("-result.json")} != expected_names:
        raise _invalid()
    configs = {config["id"]: config for config in plan["configurations"]}
    scores = {key: {} for key in configs}
    failed = {key: 0 for key in configs}
    statuses = {}
    for index, pair in enumerate(plan["order"]):
        record = _json(root / f"{index:05d}-result.json")
        if type(record) is not dict or set(record) != _RESULT_KEYS:
            raise _invalid()
        if (type(record["schema_version"]) is not int or record["schema_version"] != 1
                or type(record["index"]) is not int or record["index"] != index
                or any(not _strict_equal(record[key], value) for key, value in pair.items())
                or type(record["status"]) is not str or record["status"] not in _STATUSES
                or (record["exit_code"] is not None and type(record["exit_code"]) is not int)):
            raise _invalid()
        status = record["status"]
        statuses[status] = statuses.get(status, 0) + 1
        config_id, utterance_id = pair["configuration_id"], pair["utterance_id"]
        if status != "completed":
            failed[config_id] += 1
            continue
        row, config = rows[utterance_id], configs[config_id]
        if record["exit_code"] != 0 or any(record[key] != row["audio_sha256"] for key in
                ("source_audio_sha256_before", "source_audio_sha256_after")):
            raise _invalid()
        report = validate_quality_report(_json(root / f"{index:05d}-report.json"))
        if (any(not _strict_equal(report[key], config[key]) for key in
                ("language", "model", "beam_size", "temperature_fallback"))
                or not report["cli_checks_passed"]
                or report["allow_empty"] != (row["origin"] == "silence")
                or len(report["runs"]) != plan["iterations"] + 1):
            raise _invalid()
        terms = terms_map[utterance_id]
        if type(terms) is not dict or set(terms) != {"specialist_terms", "control_terms"}:
            raise _invalid()
        scored = score_clip(manifest, utterance_id, report, references[utterance_id],
                            terms["specialist_terms"], terms["control_terms"])
        if not _strict_equal(scored, record["score"]):
            raise _invalid()
        scores[config_id][utterance_id] = scored
    expected_summary = {"schema_version": 1, "decision": "inconclusive",
                        "measurement_endpoint": "live_inference_call_not_visible_text",
                        "planned": len(plan["order"]), "completed": statuses.get("completed", 0),
                        "failed": sum(failed.values()), "statuses": statuses}
    if not _strict_equal(_json(root / "summary.json"), expected_summary):
        raise _invalid()
    public_configs, human_groups = [], {}
    for key, config in configs.items():
        complete = not failed[key]
        human = {uid: _count_row(score) for uid, score in scores[key].items()
                 if rows[uid]["origin"] == "human"} if complete else {}
        human_groups[key] = human
        strata = {}
        for bucket in ("short", "medium", "long"):
            values = [value for uid, value in human.items() if rows[uid]["duration_bucket"] == bucket]
            strata[bucket] = {"metrics": aggregate_counts(values) if values else None,
                              "p95_exploratory": len(values) < 40}
        public_configs.append({**{k: v for k, v in config.items() if k != "id"},
                               "completed": len(scores[key]), "failed": failed[key],
                               "metrics": aggregate_counts(list(human.values())) if human else None,
                               "p95_exploratory": len(human) < 40,
                               "duration_strata": strata,
                               "synthetic_utterances": sum(rows[uid]["origin"] == "synthetic" for uid in scores[key]),
                               "controls": _controls(scores[key], rows) if complete else None})
    comparisons = []
    for key, config in configs.items():
        baseline_id = next((bid for bid, base in configs.items()
                            if base["language"] == config["language"] and base["role"] == "baseline"), None)
        if config["role"] == "baseline" or baseline_id is None:
            continue
        baseline, candidate = human_groups[baseline_id], human_groups[key]
        if not baseline or set(baseline) != set(candidate):
            continue
        ordered = sorted(baseline)
        b, c = [baseline[uid] for uid in ordered], [candidate[uid] for uid in ordered]
        if any(x["reference_words"] != y["reference_words"] for x, y in zip(b, c)):
            raise _invalid()
        errors = lambda values: [sum(row[k] for k in ("substitutions", "deletions", "insertions")) for row in values]
        timing = None
        if all(value > 0 for row in b for value in row["warm_total_ms"]):
            timing = paired_cluster_interval([row["warm_total_ms"] for row in b],
                                             [row["warm_total_ms"] for row in c], seed=plan["seed"])
        comparisons.append({"language": config["language"], "candidate_role": config["role"],
                            "accuracy": paired_wer_interval([row["reference_words"] for row in b], errors(b), errors(c), seed=plan["seed"]),
                            "warm_total_ms": timing, "p95_exploratory": len(b) < 40})
    return {**expected_summary, "split": plan["split"], "python_version": platform.python_version(),
            "unicode_version": unicodedata.unidata_version,
            "normalization_version": "nfc-casefold-nfc-words-v1",
            "coverage": coverage_report(manifest), "configurations": public_configs,
            "comparisons": comparisons}
