import math
import random
import unittest

from paired_accuracy import paired_wer_interval


class PairedAccuracyTests(unittest.TestCase):
    def test_exact_zero_wer_has_zero_change(self):
        references = [10, 20, 30, 40, 50]
        baseline = [0, 0, 0, 0, 0]
        candidate = [0, 0, 0, 0, 0]
        result = paired_wer_interval(references, baseline, candidate, resamples=100)

        self.assertEqual(result["baseline_wer"], 0.0)
        self.assertEqual(result["candidate_wer"], 0.0)
        self.assertEqual(result["absolute_wer_change"], 0.0)
        self.assertIsNone(result["relative_wer_reduction"])
        self.assertEqual(result["relative_wer_reduction_interval"], None)
        self.assertEqual(result["absolute_wer_change_interval"], [0.0, 0.0])

    def test_candidate_worse_has_negative_reduction(self):
        references = [10, 20, 30, 40, 50]
        baseline = [1, 2, 3, 4, 5]
        candidate = [2, 4, 6, 8, 10]
        result = paired_wer_interval(references, baseline, candidate, resamples=100)

        self.assertAlmostEqual(result["baseline_wer"], 0.1)
        self.assertAlmostEqual(result["candidate_wer"], 0.2)
        self.assertAlmostEqual(result["absolute_wer_change"], 0.1)
        self.assertAlmostEqual(result["relative_wer_reduction"], -1.0)
        self.assertGreater(result["absolute_wer_change_interval"][0], 0.0)
        self.assertGreater(result["absolute_wer_change_interval"][1], 0.0)
        self.assertLess(result["relative_wer_reduction_interval"][1], 0.0)

    def test_pools_unequal_utterances_instead_of_averaging_clip_wer(self):
        references = [1, 100]
        baseline = [1, 0]
        candidate = [0, 0]
        result = paired_wer_interval(references, baseline, candidate, resamples=100)
        self.assertAlmostEqual(result["baseline_wer"], 1 / 101)
        self.assertAlmostEqual(result["candidate_wer"], 0.0)
        self.assertAlmostEqual(result["relative_wer_reduction"], 1.0)

    def test_seeded_bootstrap_matches_independent_utterance_resampler(self):
        references = [10, 20, 30, 40, 50]
        baseline = [1, 4, 9, 16, 25]
        candidate = [0, 2, 12, 8, 30]
        result = paired_wer_interval(
            references, baseline, candidate, seed=42, resamples=100
        )
        rng = random.Random(42)
        changes = []
        relative = []
        for _ in range(100):
            selected = [rng.randrange(5) for _ in range(5)]
            references_sum = sum(references[index] for index in selected)
            baseline_sum = sum(baseline[index] for index in selected)
            candidate_sum = sum(candidate[index] for index in selected)
            baseline_wer = baseline_sum / references_sum
            candidate_wer = candidate_sum / references_sum
            changes.append(candidate_wer - baseline_wer)
            if baseline_sum:
                relative.append(1 - candidate_wer / baseline_wer)

        def rank(values, percentile):
            return sorted(values)[math.ceil(percentile * len(values)) - 1]

        self.assertEqual(
            result["absolute_wer_change_interval"],
            [rank(changes, 0.025), rank(changes, 0.975)],
        )
        self.assertEqual(
            result["relative_wer_reduction_interval"],
            [rank(relative, 0.025), rank(relative, 0.975)],
        )

    def test_bootstrap_zero_baseline_draw_is_not_dropped(self):
        references = [10, 20, 30, 40, 50]
        baseline = [1, 0, 0, 0, 0]
        candidate = [0, 0, 0, 0, 0]
        result = paired_wer_interval(
            references, baseline, candidate, seed=187, resamples=100
        )
        self.assertIsNone(result["relative_wer_reduction_interval"])

    def test_validation_rejects_mismatches_bool_values_and_mutation(self):
        references = [10, 20, 30, 40, 50]
        baseline = [1, 2, 3, 4, 5]
        candidate = [0, 1, 2, 3, 4]
        original = (references.copy(), baseline.copy(), candidate.copy())
        for args in (
            ([], [], []),
            (references[:-1], baseline, candidate),
            ([True] + references[1:], baseline, candidate),
            (references, [False] + baseline[1:], candidate),
            (references, baseline, [10_001] + candidate[1:]),
        ):
            with self.assertRaises(ValueError):
                paired_wer_interval(*args, resamples=100)
        for seed in (True, -1, 2**32):
            with self.assertRaises(ValueError):
                paired_wer_interval(references, baseline, candidate, seed=seed, resamples=100)
        for resamples in (True, 99, 10_001):
            with self.assertRaises(ValueError):
                paired_wer_interval(references, baseline, candidate, resamples=resamples)
        self.assertEqual((references, baseline, candidate), original)


if __name__ == "__main__":
    unittest.main()
