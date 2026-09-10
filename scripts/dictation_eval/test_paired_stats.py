import copy
import math
import random
import unittest

from paired_stats import nearest_rank, paired_cluster_interval


def groups(values: list[float], utterances: int = 5, repetitions: int = 5) -> list[list[float]]:
    return [values[index * repetitions : (index + 1) * repetitions] for index in range(utterances)]


class PairedStatsTests(unittest.TestCase):
    def test_nearest_rank_validates_and_does_not_mutate(self):
        values = [9.0, 1.0, 7.0, 5.0]
        original = values.copy()
        self.assertEqual(nearest_rank(values, 0.5), 5.0)
        self.assertEqual(nearest_rank(values, 0.95), 9.0)
        self.assertEqual(values, original)
        for invalid in ([], [True], [float("nan")], [float("inf")], [-1.0]):
            with self.assertRaises(ValueError):
                nearest_rank(invalid, 0.5)
        for percentile in (0, -0.1, float("nan"), float("inf"), 1.1, True):
            with self.assertRaises(ValueError):
                nearest_rank([1.0], percentile)

    def test_identical_pairs_have_zero_gain_and_zero_interval(self):
        baseline = groups([float(value) for value in range(1, 26)])
        result = paired_cluster_interval(baseline, copy.deepcopy(baseline), resamples=100)
        self.assertEqual(result["relative_p95_gain"], 0.0)
        self.assertEqual(result["relative_p95_gain_interval"], [0.0, 0.0])
        self.assertEqual(result["sampling_unit"], "utterance_cluster")

    def test_fixed_twenty_percent_improvement(self):
        baseline = groups([float(value) for value in range(10, 35)])
        candidate = [[value * 0.8 for value in group] for group in baseline]
        result = paired_cluster_interval(baseline, candidate, resamples=100)
        self.assertAlmostEqual(result["relative_p95_gain"], 0.2)
        self.assertAlmostEqual(result["relative_p95_gain_interval"][0], 0.2)
        self.assertAlmostEqual(result["relative_p95_gain_interval"][1], 0.2)

    def test_seed_is_deterministic_and_candidate_can_be_slower(self):
        baseline = groups([float(value) for value in range(10, 35)])
        candidate = [[value * 1.2 for value in group] for group in baseline]
        first = paired_cluster_interval(baseline, candidate, seed=42, resamples=100)
        second = paired_cluster_interval(baseline, candidate, seed=42, resamples=100)
        self.assertEqual(first, second)
        self.assertAlmostEqual(first["relative_p95_gain"], -0.2)
        self.assertLess(first["relative_p95_gain_interval"][1], 0.0)

    def test_bootstrap_resamples_whole_clusters(self):
        baseline = [[1.0] * 5, [10.0] * 5, [100.0] * 5, [1000.0] * 5, [10000.0] * 5]
        candidate = [[1.0] * 5, [20.0] * 5, [50.0] * 5, [1500.0] * 5, [9000.0] * 5]
        result = paired_cluster_interval(baseline, candidate, seed=187, resamples=100)
        self.assertEqual(result["utterances"], 5)
        self.assertEqual(result["repetitions_per_utterance"], 5)

        def rank(values, percentile):
            return sorted(values)[math.ceil(percentile * len(values)) - 1]

        rng = random.Random(187)
        expected_gains = []
        for _ in range(100):
            selected = [rng.randrange(5) for _ in range(5)]
            baseline_sample = [value for index in selected for value in baseline[index]]
            candidate_sample = [value for index in selected for value in candidate[index]]
            expected_gains.append(
                1.0
                - rank(candidate_sample, 0.95) / rank(baseline_sample, 0.95)
            )
        expected_interval = [
            rank(expected_gains, 0.025),
            rank(expected_gains, 0.975),
        ]
        self.assertEqual(result["relative_p95_gain_interval"], expected_interval)

    def test_invalid_groups_seed_and_resamples_fail_closed(self):
        valid = groups([float(value) for value in range(25)])
        invalid_pairs = [
            ([], valid),
            (valid[:-1], valid),
            ([[1.0] * 4] * 5, valid),
            ([[1.0] * 5, [1.0] * 6, *valid[2:]], valid),
            ([[0.0] * 5] + valid[1:], valid),
            ([[float("nan")] * 5] + valid[1:], valid),
            (valid, [[1.0] * 5] * 4),
        ]
        for baseline, candidate in invalid_pairs:
            with self.assertRaises(ValueError):
                paired_cluster_interval(baseline, candidate, resamples=100)
        for seed in (True, -1, 2**32):
            with self.assertRaises(ValueError):
                paired_cluster_interval(valid, valid, seed=seed, resamples=100)
        for resamples in (True, 99, 10_001):
            with self.assertRaises(ValueError):
                paired_cluster_interval(valid, valid, resamples=resamples)


if __name__ == "__main__":
    unittest.main()
