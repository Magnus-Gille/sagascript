import itertools
import unittest

from edit_counts import edit_counts


class CountsTests(unittest.TestCase):
    def test_examples(self):
        examples = [
            ([], [], (0, 0, 0)),
            (["a"], [], (0, 1, 0)),
            ([], ["a", "b"], (0, 0, 2)),
            (["a"], ["b"], (1, 0, 0)),
            (["a", "b"], ["b", "a"], (2, 0, 0)),
            (["a", "b", "c"], ["a", "d", "c", "e"], (1, 0, 1)),
            (["a", "a"], ["a"], (0, 1, 0)),
        ]
        for reference, hypothesis, expected in examples:
            self.assertEqual(edit_counts(reference, hypothesis), expected)

    def test_bad_inputs(self):
        for reference, hypothesis in [
            ("abc", []), ([], ["a", None]), (None, []),
            (["a"] * 2049, []), (["a"] * 1025, ["a"] * 1025),
        ]:
            with self.assertRaises(ValueError):
                edit_counts(reference, hypothesis)

    def test_exhaustive_small_minimum_cost(self):
        strings = [list(p) for n in range(5) for p in itertools.product(["a", "b"], repeat=n)]
        for reference in strings:
            for hypothesis in strings:
                matrix = [[0] * (len(hypothesis) + 1) for _ in range(len(reference) + 1)]
                for i in range(len(reference) + 1):
                    matrix[i][0] = i
                for j in range(len(hypothesis) + 1):
                    matrix[0][j] = j
                for i in range(1, len(reference) + 1):
                    for j in range(1, len(hypothesis) + 1):
                        matrix[i][j] = min(
                            matrix[i - 1][j] + 1,
                            matrix[i][j - 1] + 1,
                            matrix[i - 1][j - 1] + (reference[i - 1] != hypothesis[j - 1]),
                        )
                substitutions, deletions, insertions = edit_counts(reference, hypothesis)
                self.assertEqual(substitutions + deletions + insertions, matrix[-1][-1])
                self.assertEqual(len(reference) - deletions + insertions, len(hypothesis))
                self.assertTrue(all(isinstance(x, int) and x >= 0 for x in (substitutions, deletions, insertions)))


if __name__ == "__main__":
    unittest.main()
