#!/usr/bin/env python3

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import sys

sys.path.insert(0, str(Path(__file__).parent))
import reference_dataset as dataset


SOURCE = "a" * 64
REVIEWED_AT = "2026-01-01T00:00:00Z"


def verified(start, end, speakers=("A",), **extra):
    item = {
        "start": start,
        "end": end,
        "speakers": list(speakers),
        "status": "verified",
        "evidence": [{"kind": "human_review", "artifact": "synthetic-note"}],
        "reviewer": "reviewer-one",
        "reviewed_at": REVIEWED_AT,
    }
    item.update(extra)
    return item


def reference(intervals, *, duration=10, speakers=("A", "B"), windows=None):
    value = {
        "source_sha256": SOURCE,
        "duration_seconds": duration,
        "speakers": list(speakers),
        "intervals": intervals,
    }
    if windows is not None:
        value["windows"] = windows
    return value


def qualification_fixture():
    windows = [
        {"id": "train-1", "start": 0, "end": 10},
        {"id": "dev-1", "start": 10, "end": 20},
        {"id": "eval-1", "start": 20, "end": 30},
    ]
    intervals = [
        verified(0, 4, ("A",), window_id="train-1"),
        verified(10, 14, ("B",), window_id="dev-1"),
        verified(20, 24, ("A",), window_id="eval-1"),
        verified(24, 29.5, ("B",), window_id="eval-1"),
        {"start": 29.5, "end": 30, "speakers": [], "status": "unknown", "window_id": "eval-1"},
    ]
    value = reference(intervals, duration=30, windows=windows)
    split = {
        "reference_id": "synthetic-reference-v1",
        "source_sha256": SOURCE,
        "seed": "synthetic",
        "split_id": "synthetic-split-v1",
        "frozen": True,
        "policy": {"id": "human-review-v1", "version": "1", "frozen": True},
        "windows": [
            {"id": "train-1", "start": 0.0, "end": 10.0, "split": "train", "stratum": "ordinary"},
            {"id": "dev-1", "start": 10.0, "end": 20.0, "split": "dev", "stratum": "ordinary"},
            {"id": "eval-1", "start": 20.0, "end": 30.0, "split": "eval", "stratum": "difficult"},
        ],
    }
    return dataset.validate_reference(value), split


class ReferenceDatasetTests(unittest.TestCase):
    def test_schema_rejects_unknown_top_level_fields(self):
        with self.assertRaisesRegex(dataset.ReferenceError, "unsupported top-level"):
            dataset.validate_reference({**reference([]), "top_level_extra": True})

    def test_candidate_is_never_exported_or_promoted(self):
        candidate = {
            "start": 1,
            "end": 3,
            "speakers": ["A"],
            "status": "candidate",
            "evidence": [{"kind": "model_consensus", "artifact": "synthetic-proposal"}],
        }
        normalized = dataset.validate_reference(reference([candidate]))
        rttm, uem, summary = dataset.export_intervals(normalized)
        self.assertEqual(rttm, [])
        self.assertEqual(uem, [])
        self.assertEqual(summary["interval_counts"], {"verified": 0, "candidate": 1, "unknown": 0})

    def test_unknown_hole_is_removed_from_scored_uem(self):
        unknown = {"start": 4, "end": 6, "speakers": [], "status": "unknown"}
        normalized = dataset.validate_reference(reference([verified(0, 10), unknown]))
        rttm, uem, summary = dataset.export_intervals(normalized)
        self.assertEqual(len(rttm), 1)
        self.assertEqual(len(uem), 2)
        self.assertIn(" 0.000000000 4.000000000", uem[0])
        self.assertIn(" 6.000000000 10.000000000", uem[1])
        self.assertEqual(summary["scored_seconds"], 8.0)

    def test_explicit_simultaneous_speech_exports_two_rttm_entries(self):
        normalized = dataset.validate_reference(reference([verified(1, 3, ("A", "B"))]))
        rttm, _, _ = dataset.export_intervals(normalized)
        self.assertEqual(len(rttm), 2)
        self.assertTrue(any(" A " in line for line in rttm))
        self.assertTrue(any(" B " in line for line in rttm))

    def test_adjacent_identical_verified_intervals_are_coalesced(self):
        normalized = dataset.validate_reference(reference([verified(0, 2), verified(2, 4)]))
        rttm, _, summary = dataset.export_intervals(normalized)
        self.assertEqual(len(rttm), 1)
        self.assertIn(" 0.000000000 4.000000000", rttm[0])
        self.assertEqual(summary["verified_interval_count_after_coalesce"], 1)

    def test_invalid_finite_bounds_reviewer_and_unknown_id_are_rejected(self):
        bad_documents = [
            reference([verified(float("nan"), 2)]),
            reference([verified(1, 11)]),
            reference([{**verified(1, 2), "reviewer": None}]),
            reference([{**verified(1, 2), "evidence": []}]),
            reference([{**verified(1, 2), "reviewer": "model"}]),
            reference([{**verified(1, 2), "speakers": ["C"]}]),
            reference([{"start": 1, "end": 2, "speakers": ["A"], "status": "unknown"}]),
        ]
        for document in bad_documents:
            with self.subTest(document=document):
                with self.assertRaises(dataset.ReferenceError):
                    dataset.validate_reference(document)

    def test_incompatible_verified_singleton_overlap_is_rejected(self):
        with self.assertRaisesRegex(dataset.ReferenceError, "incompatible"):
            dataset.validate_reference(reference([verified(0, 4, ("A",)), verified(2, 5, ("B",))]))

    def test_overlapping_supplied_windows_are_rejected(self):
        windows = [
            {"id": "w1", "start": 0, "end": 5},
            {"id": "w2", "start": 4, "end": 8},
        ]
        with self.assertRaisesRegex(dataset.ReferenceError, "overlapping windows"):
            dataset.validate_reference(reference([], duration=10, windows=windows))

    def test_any_different_verified_sets_overlap_is_rejected(self):
        with self.assertRaisesRegex(dataset.ReferenceError, "incompatible"):
            dataset.validate_reference(
                reference([verified(0, 4, ("A", "B")), verified(2, 5, ("A",))])
            )

    def test_verified_silence_scores_false_alarms_and_unknown_remains_a_hole(self):
        silence = verified(3, 4, (), activity="silence")
        unknown = {"start": 4, "end": 5, "speakers": [], "status": "unknown"}
        normalized = dataset.validate_reference(
            reference([verified(0, 3), silence, unknown, verified(5, 8)], duration=10)
        )
        rttm, uem, summary = dataset.export_intervals(normalized)
        self.assertEqual(len(rttm), 2, "verified silence must never create an RTTM speaker turn")
        self.assertEqual(len(uem), 2)
        self.assertIn(" 0.000000000 4.000000000", uem[0])
        self.assertIn(" 5.000000000 8.000000000", uem[1])
        self.assertEqual(summary["scored_seconds"], 7.0)
        self.assertEqual(summary["interval_counts"]["verified"], 3)
        self.assertEqual(summary["activity_counts"]["silence"], 1)

    def test_split_is_deterministic_and_assigns_each_whole_window_once(self):
        windows = [
            {"id": "w1", "start": 0, "end": 10},
            {"id": "w2", "start": 10, "end": 20},
            {"id": "w3", "start": 20, "end": 30},
            {"id": "w4", "start": 30, "end": 40},
        ]
        normalized = dataset.validate_reference(reference([], duration=40, windows=windows))
        first = dataset.split_windows(normalized, 0.5, 0.25, 0.25, "synthetic-seed")
        second = dataset.split_windows(normalized, 0.5, 0.25, 0.25, "synthetic-seed")
        self.assertEqual(first, second)
        self.assertEqual(len(first["windows"]), 4)
        self.assertEqual({item["id"] for item in first["windows"]}, {"w1", "w2", "w3", "w4"})
        self.assertEqual(first["counts"], {"train": 2, "dev": 1, "eval": 1})

    def test_export_is_deterministic_and_refuses_overwrite(self):
        normalized = dataset.validate_reference(reference([verified(0, 2)]))
        rttm, uem, _ = dataset.export_intervals(normalized)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            rttm_path = root / "ref.rttm"
            uem_path = root / "ref.uem"
            dataset.write_new(rttm_path, "".join(rttm))
            dataset.write_new(uem_path, "".join(uem))
            self.assertEqual(rttm_path.read_text(), "".join(rttm))
            self.assertEqual(uem_path.read_text(), "".join(uem))
            with self.assertRaises(dataset.ReferenceError):
                dataset.write_new(rttm_path, "different\n")

    def test_qualification_report_passes_only_a_frozen_balanced_fixture(self):
        normalized, split = qualification_fixture()
        report = dataset.qualification_report(normalized, split, expected_policy_id="human-review-v1")
        self.assertTrue(report["ready_for_quality_adoption"])
        self.assertEqual(report["status"], "gold")
        self.assertEqual(report["coverage"]["by_split"]["eval"]["human_identified_speech_seconds"], 9.5)
        self.assertEqual(report["coverage"]["by_split"]["eval"]["verified_speaker_time_seconds"], 9.5)
        self.assertEqual(report["coverage"]["by_speaker"]["A"]["eval_represented"], True)
        self.assertEqual(report["frozen_split_identity"]["id"], "synthetic-split-v1")

    def test_qualification_rejects_empty_gold_low_coverage_bias_and_identity_leaks(self):
        normalized, split = qualification_fixture()
        empty = dict(normalized, intervals=[])
        empty_report = dataset.qualification_report(empty, split, expected_policy_id="human-review-v1")
        self.assertFalse(empty_report["ready_for_quality_adoption"])
        self.assertIn("zero-verified", {failure["code"] for failure in empty_report["failures"]})

        low_intervals = [item for item in normalized["intervals"] if item["start"] < 24]
        low_intervals.append({"start": 24, "end": 30, "speakers": ["B"], "status": "candidate", "window_id": "eval-1"})
        low = dict(normalized, intervals=low_intervals)
        low_report = dataset.qualification_report(low, split, expected_policy_id="human-review-v1")
        self.assertFalse(low_report["ready_for_quality_adoption"])
        self.assertIn("low-eval-coverage", {failure["code"] for failure in low_report["failures"]})
        self.assertIn("missing-eval-speaker", {failure["code"] for failure in low_report["failures"]})
        no_difficult_eval = dict(normalized, intervals=[item for item in normalized["intervals"] if item["start"] < 20])
        no_stratum_report = dataset.qualification_report(no_difficult_eval, split, expected_policy_id="human-review-v1")
        self.assertIn("missing-eval-stratum", {failure["code"] for failure in no_stratum_report["failures"]})
        self.assertEqual(no_stratum_report["coverage"]["by_stratum"]["difficult"]["human_identified_speech_seconds"], 0.0)

        stale = dataset.qualification_report(normalized, split, expected_source_sha256="b" * 64, expected_policy_id="old-policy")
        codes = {failure["code"] for failure in stale["failures"]}
        self.assertIn("source-hash-mismatch", codes)
        self.assertIn("stale-policy", codes)

        leaked_split = dict(split, windows=[*split["windows"], dict(split["windows"][0], id="eval-1")])
        leaked = dataset.qualification_report(normalized, leaked_split, expected_policy_id="human-review-v1")
        self.assertIn("split-leak", {failure["code"] for failure in leaked["failures"]})

    def test_qualification_cli_writes_machine_readable_report_without_overwrite(self):
        normalized, split = qualification_fixture()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            reference_path = root / "reference.json"
            split_path = root / "qualification.json"
            report_path = root / "report.json"
            reference_path.write_text(json.dumps({key: value for key, value in normalized.items() if key != "summary"}))
            split_path.write_text(json.dumps(split))
            command = [
                sys.executable,
                str(Path(__file__).with_name("reference_dataset.py")),
                "qualify",
                str(reference_path),
                "--split",
                str(split_path),
                "--output",
                str(report_path),
                "--policy-id",
                "human-review-v1",
            ]
            result = subprocess.run(command, check=False, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            report = json.loads(report_path.read_text())
            self.assertTrue(report["ready_for_quality_adoption"])
            second = subprocess.run(command, check=False, capture_output=True, text=True)
            self.assertNotEqual(second.returncode, 0)


if __name__ == "__main__":
    unittest.main()
