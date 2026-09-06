import copy
import unittest

from corpus_manifest import coverage_report, validate_manifest


def row(
    number: int,
    *,
    language: str = "en",
    split: str = "heldout",
    speaker_id: str = "speaker_a",
    origin: str = "human",
    tags: list[str] | None = None,
    duration_bucket: str = "short",
    environment: str = "quiet",
) -> dict[str, object]:
    return {
        "id": f"utterance_{number}",
        "language": language,
        "split": split,
        "speaker_id": speaker_id,
        "audio_sha256": f"{number:064x}",
        "reference_sha256": f"{number + 1000:064x}",
        "origin": origin,
        "duration_bucket": duration_bucket,
        "environment": environment,
        "tags": list(tags or []),
    }


def manifest(rows: list[dict[str, object]]) -> dict[str, object]:
    return {"schema_version": 1, "utterances": rows}


def passing_rows(*, include_finnish: bool = False) -> list[dict[str, object]]:
    rows: list[dict[str, object]] = []
    number = 1
    languages = ("en", "sv", "no") + (("fi",) if include_finnish else ())
    for language in languages:
        for split in ("dev", "heldout"):
            count = 10 if split == "dev" else 40
            for index in range(count):
                rows.append(
                    row(
                        number,
                        language=language,
                        split=split,
                        speaker_id=f"speaker_{index % 2}",
                        tags=["specialist", "numbers", "negation", "ordinary"],
                        duration_bucket=("short", "medium", "long")[index % 3],
                        environment=("quiet", "noisy")[index % 2],
                    )
                )
                number += 1
        rows.append(
            row(
                number,
                language=language,
                split="heldout",
                origin="silence",
                speaker_id=f"silence_{language}",
                tags=["silence"],
            )
        )
        number += 1
    return rows


class CorpusManifestTests(unittest.TestCase):
    def test_validate_copies_nested_data_and_accepts_valid_row(self):
        source = manifest([row(1, tags=["specialist"])])
        validated = validate_manifest(source)
        source["utterances"][0]["id"] = "changed"
        source["utterances"][0]["tags"].append("ordinary")
        self.assertEqual(validated["utterances"][0]["id"], "utterance_1")
        self.assertEqual(validated["utterances"][0]["tags"], ["specialist"])

    def test_rejects_top_level_shape_and_scalar_types(self):
        valid = manifest([row(1)])
        for value in (
            None,
            [],
            {"schema_version": 1, "utterances": [], "extra": 1},
            {"schema_version": True, "utterances": [row(1)]},
            {"schema_version": 2, "utterances": [row(1)]},
            {"schema_version": 1, "utterances": "rows"},
        ):
            with self.assertRaises(ValueError):
                validate_manifest(value)
        too_many = [row(index) for index in range(1, 502)]
        with self.assertRaises(ValueError):
            validate_manifest(manifest(too_many))
        bad_row = copy.deepcopy(valid)
        bad_row["utterances"][0]["extra"] = "nope"
        with self.assertRaises(ValueError):
            validate_manifest(bad_row)

    def test_rejects_ids_hashes_duplicates_and_noncanonical_values(self):
        invalid_rows = [
            [row(1), {**row(2), "id": "contains space"}],
            [row(1), {**row(2), "id": "utterance_1"}],
            [{**row(1), "speaker_id": "speaker/name"}],
            [{**row(1), "audio_sha256": "A" * 64}],
            [{**row(1), "reference_sha256": "g" * 64}],
            [row(1), {**row(2), "audio_sha256": row(1)["audio_sha256"]}],
            [{**row(1), "language": True}],
            [{**row(1), "tags": "specialist"}],
            [{**row(1), "tags": ["specialist", "specialist"]}],
            [{**row(1), "tags": ["unknown"]}],
        ]
        for rows in invalid_rows:
            with self.assertRaises(ValueError):
                validate_manifest(manifest(rows))

    def test_rejects_invalid_enums_and_silence_mismatch(self):
        fields = (
            ("language", "da"),
            ("language", "auto"),
            ("split", "test"),
            ("origin", "generated"),
            ("duration_bucket", "tiny"),
            ("environment", "studio"),
        )
        for field, value in fields:
            candidate = row(1)
            candidate[field] = value
            with self.assertRaises(ValueError):
                validate_manifest(manifest([candidate]))
        for candidate in (
            row(1, origin="silence"),
            row(1, tags=["silence"]),
            row(1, origin="human", tags=["silence"]),
        ):
            with self.assertRaises(ValueError):
                validate_manifest(manifest([candidate]))
        valid_silence = row(1, origin="silence", tags=["silence"])
        self.assertEqual(validate_manifest(manifest([valid_silence]))["schema_version"], 1)

    def test_unknown_environment_is_valid_but_not_a_required_coverage_bucket(self):
        validated = validate_manifest(manifest([row(1, environment="unknown")]))
        self.assertEqual(validated["utterances"][0]["environment"], "unknown")

        rows = passing_rows()
        for candidate in rows:
            if candidate["split"] == "heldout" and candidate["origin"] == "human":
                candidate["environment"] = "unknown"
        report = coverage_report(validate_manifest(manifest(rows)))
        self.assertFalse(report["eligible"])
        for language in ("en", "sv", "no"):
            language_report = report["languages"][language]
            self.assertEqual(language_report["missing_environments"], ["quiet", "noisy"])
            self.assertEqual(language_report["unknown_environment_human"], 40)
            self.assertFalse(language_report["eligible"])

    def test_coverage_requires_human_heldout_evidence_and_ignores_synthetic(self):
        rows = [
            row(index, split="dev", origin="synthetic") for index in range(1, 11)
        ] + [
            row(index, origin="synthetic") for index in range(11, 51)
        ]
        report = coverage_report(validate_manifest(manifest(rows)))
        self.assertFalse(report["eligible"])
        for language in ("en", "sv", "no"):
            language_report = report["languages"][language]
            self.assertEqual(language_report["dev_human"], 0)
            self.assertEqual(language_report["heldout_human"], 0)
            self.assertEqual(language_report["human_speakers"], 0)
            self.assertEqual(language_report["heldout_human_speakers"], 0)
            self.assertFalse(language_report["eligible"])

    def test_eligible_report_is_aggregate_and_requires_all_languages(self):
        rows = passing_rows()
        validated = validate_manifest(manifest(rows))
        report = coverage_report(validated)
        self.assertTrue(report["eligible"])
        self.assertNotIn("audio_sha256", repr(report))
        self.assertNotIn("utterance_", repr(report))
        self.assertEqual(report["languages"]["en"]["dev_human"], 10)
        self.assertEqual(report["languages"]["sv"]["heldout_human"], 40)
        self.assertEqual(report["languages"]["no"]["heldout_human_speakers"], 2)
        self.assertEqual(report["languages"]["no"]["heldout_silence"], 1)

    def test_each_missing_heldout_requirement_is_ineligible(self):
        base = passing_rows()
        cases: list[tuple[str, list[dict[str, object]]]] = []

        cases.append(("below dev count", [row for row in base if not (row["language"] == "en" and row["split"] == "dev" and row["id"] == "utterance_1")]))
        cases.append(("below heldout count", [row for row in base if not (row["language"] == "en" and row["split"] == "heldout" and row["origin"] == "human" and row["id"] == "utterance_11")]))

        one_heldout_speaker = copy.deepcopy(base)
        for candidate in one_heldout_speaker:
            if candidate["language"] == "en" and candidate["split"] == "heldout" and candidate["origin"] == "human":
                candidate["speaker_id"] = "speaker_only"
        cases.append(("one heldout speaker", one_heldout_speaker))

        for missing_tag in ("specialist", "numbers", "negation", "ordinary"):
            missing = copy.deepcopy(base)
            for candidate in missing:
                if candidate["language"] == "en" and candidate["split"] == "heldout" and candidate["origin"] == "human":
                    candidate["tags"] = [tag for tag in candidate["tags"] if tag != missing_tag]
            cases.append((f"missing {missing_tag}", missing))

        for missing_duration in ("short", "medium", "long"):
            missing = copy.deepcopy(base)
            for candidate in missing:
                if candidate["language"] == "en" and candidate["split"] == "heldout" and candidate["origin"] == "human":
                    candidate["duration_bucket"] = "short" if missing_duration != "short" else "medium"
            cases.append((f"missing {missing_duration}", missing))

        for missing_environment in ("quiet", "noisy"):
            missing = copy.deepcopy(base)
            for candidate in missing:
                if candidate["language"] == "en" and candidate["split"] == "heldout" and candidate["origin"] == "human":
                    candidate["environment"] = "quiet" if missing_environment != "quiet" else "noisy"
            cases.append((f"missing {missing_environment}", missing))

        cases.append(("missing silence", [row for row in base if not (row["language"] == "en" and row["origin"] == "silence")]))
        for name, rows in cases:
            with self.subTest(name=name):
                report = coverage_report(validate_manifest(manifest(rows)))
                self.assertFalse(report["eligible"])
                self.assertFalse(report["languages"]["en"]["eligible"])

    def test_historical_languages_remain_the_default_coverage_gate(self):
        report = coverage_report(validate_manifest(manifest(passing_rows())))
        self.assertTrue(report["eligible"])
        self.assertEqual(set(report["languages"]), {"en", "sv", "no"})

    def test_finnish_rows_explicitly_opt_into_finnish_coverage(self):
        report = coverage_report(
            validate_manifest(manifest(passing_rows(include_finnish=True)))
        )
        self.assertTrue(report["eligible"])
        self.assertEqual(set(report["languages"]), {"en", "sv", "no", "fi"})
        self.assertEqual(report["languages"]["fi"]["heldout_human"], 40)

    def test_dev_or_synthetic_coverage_cannot_satisfy_heldout_requirements(self):
        rows = passing_rows()
        for candidate in rows:
            if candidate["language"] == "en" and candidate["split"] == "heldout" and candidate["origin"] == "human":
                candidate["tags"] = []
        report = coverage_report(validate_manifest(manifest(rows)))
        self.assertFalse(report["eligible"])
        self.assertEqual(
            report["languages"]["en"]["missing_coverage_tags"],
            ["specialist", "numbers", "negation", "ordinary"],
        )

    def test_missing_language_is_ineligible(self):
        rows = [row for row in passing_rows() if row["language"] != "sv"]
        report = coverage_report(validate_manifest(manifest(rows)))
        self.assertFalse(report["eligible"])
        self.assertFalse(report["languages"]["sv"]["eligible"])

    def test_duplicate_audio_hash_across_dev_and_heldout_is_rejected(self):
        dev = row(1, split="dev")
        heldout = row(2, split="heldout")
        heldout["audio_sha256"] = dev["audio_sha256"]
        with self.assertRaises(ValueError):
            validate_manifest(manifest([dev, heldout]))


if __name__ == "__main__":
    unittest.main()
