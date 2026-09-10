import unittest

from normalization import count_phrase_occurrences, normalize_text


class NormalizationTests(unittest.TestCase):
    def test_english_swedish_and_norwegian_are_casefolded(self):
        self.assertEqual(
            normalize_text("Hello, WORLD!"),
            ["hello", "world"],
        )
        self.assertEqual(normalize_text("Sjöfart ÄR svensk"), ["sjöfart", "är", "svensk"])
        self.assertEqual(normalize_text("NÅR DET ER riktig"), ["når", "det", "er", "riktig"])

    def test_nfc_before_and_after_casefold(self):
        self.assertEqual(normalize_text("Cafe\u0301 CAFÉ"), ["café", "café"])
        self.assertEqual(normalize_text("A\u030Angström"), ["ångström"])

    def test_internal_apostrophes_are_canonical_and_boundary_checked(self):
        self.assertEqual(
            normalize_text("Don't don’t rock'n'roll 'leading' trailing'"),
            ["don't", "don't", "rock'n'roll", "leading", "trailing"],
        )

    def test_numbers_negation_and_compounds_are_tokens(self):
        self.assertEqual(
            normalize_text("No 42, not 3.14; state-of-the-art under_score"),
            ["no", "42", "not", "3", "14", "state", "of", "the", "art", "under", "score"],
        )

    def test_punctuation_and_emoji_are_separators(self):
        self.assertEqual(
            normalize_text("Hej—världen! 🙂 foo/bar #tag"),
            ["hej", "världen", "foo", "bar", "tag"],
        )

    def test_phrase_count_is_whole_token_and_non_overlapping(self):
        self.assertEqual(
            count_phrase_occurrences(
                "Hello world hello WORLD helloworld hello-world",
                "HELLO world",
            ),
            3,
        )
        self.assertEqual(count_phrase_occurrences("a a a", "a a"), 1)

    def test_phrase_count_does_not_match_substrings(self):
        self.assertEqual(count_phrase_occurrences("countryside country", "country"), 1)
        self.assertEqual(count_phrase_occurrences("notebook book", "book"), 1)

    def test_empty_phrase_is_rejected(self):
        for phrase in ["", "---", "🙂"]:
            with self.assertRaises(ValueError):
                count_phrase_occurrences("speech", phrase)

    def test_non_string_and_oversized_inputs_are_rejected(self):
        for value in [None, 42, ["speech"]]:
            with self.assertRaises(ValueError):
                normalize_text(value)
        with self.assertRaises(ValueError):
            normalize_text("a" * 100_001)
        with self.assertRaises(ValueError):
            count_phrase_occurrences("speech", "a" * 100_001)


if __name__ == "__main__":
    unittest.main()
