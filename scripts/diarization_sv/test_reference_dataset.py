#!/usr/bin/env python3

import json
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


class ReferenceDatasetTests(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
