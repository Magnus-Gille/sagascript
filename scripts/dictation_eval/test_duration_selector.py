import copy
import hashlib
import unittest

from duration_selector import select_by_duration


def row(identifier, duration_ms):
    return {"id": identifier, "duration_ms": duration_ms}


def quotas(short=1, medium=1, long=1):
    return {"short": short, "medium": medium, "long": long}


class DurationSelectorTests(unittest.TestCase):
    def test_boundaries_are_short_medium_inclusive_and_long(self):
        rows = [
            row("short_last", 4_999),
            row("medium_first", 5_000),
            row("medium_last", 15_000),
            row("long_first", 15_001),
        ]
        result = select_by_duration(rows, quotas(1, 2, 1), seed="seed", split="dev")
        self.assertEqual(
            {item["id"] for item in result},
            {"short_last", "medium_first", "medium_last", "long_first"},
        )
        self.assertEqual(
            [item["id"] for item in result[:1]],
            ["short_last"],
        )
        self.assertEqual(len(result), 4)

    def test_hash_ranking_is_independent_and_tie_breaks_by_id(self):
        rows = [row("short_b", 100), row("short_a", 200), row("short_c", 300)]
        expected = sorted(
            rows,
            key=lambda item: (
                hashlib.sha256(b"seed:heldout:" + item["id"].encode()).hexdigest(),
                item["id"],
            ),
        )[:2]
        result = select_by_duration(rows, quotas(2, 0, 0), seed="seed", split="heldout")
        self.assertEqual(result, expected)
        self.assertEqual(
            result,
            select_by_duration(
                list(reversed(rows)), quotas(2, 0, 0), seed="seed", split="heldout"
            ),
        )

    def test_result_is_bucket_ordered_copied_and_inputs_are_not_mutated(self):
        rows = [row("long", 20_000), row("short", 100), row("medium", 5_000)]
        original_rows = copy.deepcopy(rows)
        original_quotas = quotas(1, 1, 1)
        result = select_by_duration(rows, original_quotas, seed="seed", split="dev")
        self.assertEqual([item["id"] for item in result], ["short", "medium", "long"])
        self.assertEqual(rows, original_rows)
        self.assertEqual(original_quotas, quotas(1, 1, 1))
        self.assertIsNot(result[0], rows[1])
        result[0]["id"] = "changed"
        self.assertEqual(rows, original_rows)

    def test_quota_deficits_fail_closed(self):
        rows = [row("short", 100), row("medium", 5_000), row("long", 20_000)]
        for candidate in (quotas(2, 0, 0), quotas(0, 2, 0), quotas(0, 0, 2)):
            with self.assertRaisesRegex(ValueError, "^invalid duration selection input$"):
                select_by_duration(rows, candidate, seed="seed", split="dev")

    def test_malformed_rows_quotas_seed_and_split_are_rejected(self):
        valid_rows = [row("short", 100), row("medium", 5_000), row("long", 20_000)]
        invalid_calls = [
            lambda: select_by_duration([], quotas(), seed="seed", split="dev"),
            lambda: select_by_duration(
                [row(f"row_{index}", 100) for index in range(5_001)],
                quotas(1, 0, 0),
                seed="seed",
                split="dev",
            ),
            lambda: select_by_duration((item for item in valid_rows), quotas(), seed="seed", split="dev"),
            lambda: select_by_duration([{"id": "bad", "duration_ms": 1, "extra": 2}, *valid_rows[1:]], quotas(), seed="seed", split="dev"),
            lambda: select_by_duration([row("has space", 100), *valid_rows[1:]], quotas(), seed="seed", split="dev"),
            lambda: select_by_duration([row("duplicate", 100), row("duplicate", 200), valid_rows[2]], quotas(), seed="seed", split="dev"),
            lambda: select_by_duration([row("bool", True), *valid_rows[1:]], quotas(), seed="seed", split="dev"),
            lambda: select_by_duration([row("zero", 0), *valid_rows[1:]], quotas(), seed="seed", split="dev"),
            lambda: select_by_duration([row("too_long", 120_001), *valid_rows[1:]], quotas(), seed="seed", split="dev"),
            lambda: select_by_duration([row("x" * 81, 100), *valid_rows[1:]], quotas(), seed="seed", split="dev"),
            lambda: select_by_duration(valid_rows, {"short": 1, "medium": 1}, seed="seed", split="dev"),
            lambda: select_by_duration(valid_rows, {**quotas(), "short": True}, seed="seed", split="dev"),
            lambda: select_by_duration(valid_rows, quotas(0, 0, 0), seed="seed", split="dev"),
            lambda: select_by_duration(valid_rows, quotas(500, 1, 0), seed="seed", split="dev"),
            lambda: select_by_duration(valid_rows, quotas(), seed=True, split="dev"),
            lambda: select_by_duration(valid_rows, quotas(), seed="a" * 81, split="dev"),
            lambda: select_by_duration(valid_rows, quotas(), seed="å", split="dev"),
            lambda: select_by_duration(valid_rows, quotas(), seed="not valid", split="dev"),
            lambda: select_by_duration(valid_rows, quotas(), seed="seed", split="test"),
            lambda: select_by_duration(valid_rows, quotas(), seed="seed", split=[]),
            lambda: select_by_duration(
                valid_rows,
                quotas(),
                seed="seed",
                split=type("Text", (str,), {})("dev"),
            ),
        ]
        for call in invalid_calls:
            with self.assertRaisesRegex(ValueError, "^invalid duration selection input$"):
                call()


if __name__ == "__main__":
    unittest.main()
