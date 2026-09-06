import copy
import math
import unittest

from aggregate_counts import aggregate_counts


def row(reference_words=10, substitutions=1, deletions=2, insertions=3, cold=10.0, warm=None):
    return {
        "reference_words": reference_words,
        "substitutions": substitutions,
        "deletions": deletions,
        "insertions": insertions,
        "cold_total_ms": cold,
        "warm_total_ms": [1.0, 2.0, 3.0, 4.0, 5.0] if warm is None else warm,
    }


class AggregateCountsTests(unittest.TestCase):
    def test_weighted_wer_and_all_warm_percentiles(self):
        rows = [
            row(reference_words=10, substitutions=1, deletions=0, insertions=0, cold=10.0),
            row(reference_words=100, substitutions=0, deletions=2, insertions=1, cold=20.0),
        ]
        result = aggregate_counts(rows)
        self.assertEqual(result["utterances"], 2)
        self.assertEqual(result["reference_words"], 110)
        self.assertEqual(result["substitutions"], 1)
        self.assertEqual(result["deletions"], 2)
        self.assertEqual(result["insertions"], 1)
        self.assertEqual(result["errors"], 4)
        self.assertAlmostEqual(result["wer"], 4 / 110)
        self.assertEqual(result["cold"], {"count": 2, "p50_ms": 10.0, "p95_ms": 20.0})
        self.assertEqual(
            result["warm"],
            {
                "count": 10,
                "repetitions_per_utterance": 5,
                "p50_ms": 3.0,
                "p95_ms": 5.0,
            },
        )

    def test_output_has_exact_schema_and_does_not_mutate_input(self):
        rows = [row(warm=[9, 1, 7, 5, 3])]
        original = copy.deepcopy(rows)
        result = aggregate_counts(rows)
        self.assertEqual(
            set(result),
            {
                "utterances",
                "reference_words",
                "substitutions",
                "deletions",
                "insertions",
                "errors",
                "wer",
                "cold",
                "warm",
            },
        )
        self.assertEqual(set(result["cold"]), {"count", "p50_ms", "p95_ms"})
        self.assertEqual(
            set(result["warm"]),
            {"count", "repetitions_per_utterance", "p50_ms", "p95_ms"},
        )
        self.assertEqual(rows, original)

    def test_repetition_counts_must_match(self):
        with self.assertRaisesRegex(ValueError, "^invalid aggregate counts input$"):
            aggregate_counts([row(), row(warm=[1, 2, 3, 4, 5, 6])])

    def test_rejects_exact_key_and_type_violations(self):
        valid = row()
        invalid_rows = [
            {**valid, "unexpected": 1},
            {key: value for key, value in valid.items() if key != "insertions"},
            {**valid, "reference_words": True},
            {**valid, "substitutions": False},
            {**valid, "cold_total_ms": True},
            {**valid, "warm_total_ms": (1, 2, 3, 4, 5)},
            {**valid, "warm_total_ms": [1, 2, 3, 4]},
            {**valid, "warm_total_ms": [1, 2, 3, 4, math.nan]},
            {**valid, "warm_total_ms": [1, 2, 3, 4, math.inf]},
        ]
        for invalid in invalid_rows:
            with self.assertRaisesRegex(ValueError, "^invalid aggregate counts input$"):
                aggregate_counts([invalid])

    def test_rejects_bounds_and_overflow(self):
        valid = row()
        invalid_rows = [
            {**valid, "reference_words": 0},
            {**valid, "reference_words": 2049},
            {**valid, "substitutions": -1},
            {**valid, "deletions": 10_001},
            {**valid, "insertions": 10**400},
            {**valid, "cold_total_ms": 10**400},
            {**valid, "cold_total_ms": -0.1},
            {**valid, "cold_total_ms": math.nan},
            {**valid, "cold_total_ms": math.inf},
            {**valid, "warm_total_ms": [1, 2, 3, 4, 10**400]},
        ]
        for invalid in invalid_rows:
            with self.assertRaisesRegex(ValueError, "^invalid aggregate counts input$"):
                aggregate_counts([invalid])

    def test_row_count_bounds(self):
        valid = row()
        with self.assertRaisesRegex(ValueError, "^invalid aggregate counts input$"):
            aggregate_counts([])
        self.assertEqual(aggregate_counts([copy.deepcopy(valid) for _ in range(500)][-1:])["utterances"], 1)
        self.assertEqual(aggregate_counts([copy.deepcopy(valid) for _ in range(500)])["utterances"], 500)
        with self.assertRaisesRegex(ValueError, "^invalid aggregate counts input$"):
            aggregate_counts([copy.deepcopy(valid) for _ in range(501)])


if __name__ == "__main__":
    unittest.main()
