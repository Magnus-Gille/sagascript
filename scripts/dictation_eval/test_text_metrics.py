import unittest

from text_metrics import score_text


class TextMetricsTests(unittest.TestCase):
    def test_corpus_suitable_counts_are_not_mean_wer(self):
        result = score_text("hello world", "hello there", [], [])

        self.assertEqual(
            {key: result[key] for key in ("reference_words", "hypothesis_words", "substitutions", "deletions", "insertions")},
            {
                "reference_words": 2,
                "hypothesis_words": 2,
                "substitutions": 1,
                "deletions": 0,
                "insertions": 0,
            },
        )
        self.assertEqual(result["wer"], 0.5)

    def test_blank_audio_is_not_a_silence_hallucination(self):
        result = score_text("", "", [], [], is_silence=True)

        self.assertIsNone(result["wer"])
        self.assertFalse(result["silence_hallucination"])
        self.assertEqual(result["reference_words"], 0)
        self.assertEqual(result["hypothesis_words"], 0)

    def test_silence_hallucination_is_true_only_for_nonempty_output(self):
        result = score_text("", "[BLANK_AUDIO] words", [], [], is_silence=True)

        self.assertTrue(result["silence_hallucination"])
        self.assertEqual(result["hypothesis_words"], 3)
        self.assertFalse(score_text("spoken", "spoken", [], [])['silence_hallucination'])

    def test_repeated_specialist_phrase_recall_is_capped_by_reference(self):
        result = score_text(
            "alpha alpha beta",
            "alpha beta",
            ["alpha"],
            [],
        )

        self.assertEqual(result["specialist_expected"], 2)
        self.assertEqual(result["specialist_recalled"], 1)
        self.assertEqual(result["specialist_recall"], 0.5)

    def test_number_and_negation_insertions_and_deletions_count(self):
        deletion = score_text(
            "not 42",
            "42",
            [],
            ["not", "42", "7"],
        )
        insertion = score_text(
            "42",
            "42 7",
            [],
            ["not", "42", "7"],
        )

        self.assertEqual(deletion["control_errors"], 1)
        self.assertEqual(deletion["deletions"], 1)
        self.assertEqual(insertion["control_errors"], 1)
        self.assertEqual(insertion["insertions"], 1)

    def test_ordinary_control_annotation_and_rate_are_aggregate_only(self):
        result = score_text(
            "no 42 words",
            "no 43 words",
            [],
            ["no", "42", "43"],
            is_ordinary_control=True,
            false_glossary_replacements=2,
        )

        self.assertEqual(result["ordinary_control_words"], 3)
        self.assertEqual(result["false_glossary_replacements"], 2)
        self.assertEqual(result["false_replacements_per_1000_control_words"], 2000 / 3)

    def test_annotation_without_control_words_has_no_rate(self):
        result = score_text(
            "",
            "",
            [],
            [],
            is_ordinary_control=True,
            false_glossary_replacements=0,
        )
        self.assertEqual(result["ordinary_control_words"], 0)
        self.assertIsNone(result["false_replacements_per_1000_control_words"])

    def test_invalid_flags_annotations_and_term_lists_fail_closed(self):
        invalid_calls = [
            lambda: score_text("", "", [], [], is_silence=1),
            lambda: score_text("", "", [], [], is_ordinary_control="yes"),
            lambda: score_text("", "", [], [], false_glossary_replacements=True),
            lambda: score_text("", "", [], [], false_glossary_replacements=-1),
            lambda: score_text("", "", [], [], false_glossary_replacements=100_001),
            lambda: score_text("", "", [], [], false_glossary_replacements=1),
            lambda: score_text("", "", [""], []),
            lambda: score_text("", "", ["a", "A"], []),
            lambda: score_text("", "", ["a"] * 257, []),
            lambda: score_text("", "", [None], []),
            lambda: score_text("spoken", "spoken", ["missing"], []),
            lambda: score_text("spoken", "", [], [], is_silence=True),
        ]
        for call in invalid_calls:
            with self.assertRaises(ValueError):
                call()

    def test_results_and_errors_do_not_echo_private_content(self):
        sentinel = "PRIVATE_SENTINEL_7f4e"
        result = score_text(sentinel, sentinel, [], [])
        self.assertNotIn(sentinel, repr(result))
        with self.assertRaises(ValueError) as raised:
            score_text("spoken", "spoken", [sentinel], [])
        self.assertNotIn(sentinel, str(raised.exception))


if __name__ == "__main__":
    unittest.main()
