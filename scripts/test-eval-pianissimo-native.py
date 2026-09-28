"""Behavioral checks for separating first-use cost from warm latency."""

import importlib.util
import sys
import unittest
from pathlib import Path

script = Path(__file__).with_name("eval-pianissimo-native.py")
spec = importlib.util.spec_from_file_location("pianissimo_evaluation", script)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)


class TimingSummaryTests(unittest.TestCase):
    def test_first_launch_does_not_pollute_warm_median_or_p95(self):
        result = module.timing_summary([9.0, 0.5, 0.6, 0.7, 0.8, 0.9])
        self.assertEqual(result, {
            "first_use_s": 9.0,
            "warm_repeats": 5,
            "warm_median_s": 0.7,
            "warm_p95_s": 0.9,
        })

    def test_single_run_has_no_claimed_warm_measurement(self):
        result = module.timing_summary([1.4])
        self.assertEqual(result["first_use_s"], 1.4)
        self.assertEqual(result["warm_repeats"], 0)
        self.assertIsNone(result["warm_median_s"])
        self.assertIsNone(result["warm_p95_s"])


class QualityGateTests(unittest.TestCase):
    def test_strict_text_gate_detects_punctuation_loss_hidden_by_normalization(self):
        word = {"word": "Hej", "start": 0.0, "end": 0.1}
        baseline = {"text": "Hej.", "words": [word]}
        candidate = {"text": "Hej", "words": [word]}
        report = module.compare(baseline, candidate, 1.0, 1.0)
        self.assertEqual(report["normalized_word_edits"], 0)
        self.assertFalse(report["text_exact"])
        self.assertTrue(module.quality_passes(report, 0, 0, 0, require_exact_text=False))
        self.assertFalse(module.quality_passes(report, 0, 0, 0, require_exact_text=True))


if __name__ == "__main__":
    unittest.main()
