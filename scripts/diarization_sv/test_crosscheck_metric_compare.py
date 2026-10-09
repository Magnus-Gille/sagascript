import unittest

from crosscheck_metric_compare import MISSING, compare_metric


class MetricComparisonTests(unittest.TestCase):
    def test_matching_undefined_metric_is_a_passing_null_difference(self):
        self.assertEqual(compare_metric(None, None), (None, True))

    def test_null_numeric_mismatch_is_a_receipt_failure(self):
        difference, passed = compare_metric(None, 0.25)
        self.assertFalse(passed)
        self.assertEqual(difference["expected"], 0.25)
        self.assertIsNone(difference["actual"])
        self.assertIn("null", difference["reason"])

    def test_undefined_expected_metric_rejects_numeric_actual(self):
        difference, passed = compare_metric(0.0, None)
        self.assertFalse(passed)
        self.assertIsNone(difference["expected"])
        self.assertEqual(difference["actual"], 0.0)

    def test_missing_metric_is_distinct_from_null(self):
        difference, passed = compare_metric(MISSING, 0.0)
        self.assertFalse(passed)
        self.assertEqual(difference["actual"], "missing")

    def test_numeric_controls_keep_tolerance_difference(self):
        difference, passed = compare_metric(1.0, 1.0 + 1e-10)
        self.assertTrue(passed)
        self.assertAlmostEqual(difference, 1e-10)

    def test_zero_reference_time_keeps_der_null_and_checks_raw_false_alarm(self):
        native_metrics = {
            "der": None,
            "reference_speaker_seconds": 0.0,
            "miss_seconds": 0.0,
            "false_alarm_seconds": 2.0,
            "confusion_seconds": 0.0,
            "jer": None,
        }
        expected = {
            "der": None,
            "reference_speaker_seconds": 0.0,
            "miss_seconds": 0.0,
            "false_alarm_seconds": 2.0,
            "confusion_seconds": 0.0,
            "jer": None,
        }
        comparisons = [
            compare_metric(native_metrics[key], expected[key])
            for key in expected
        ]
        self.assertTrue(all(passed for _, passed in comparisons))
        self.assertIsNone(comparisons[0][0])
        self.assertEqual(comparisons[3][0], 0.0)


if __name__ == "__main__":
    unittest.main()
