import copy
import hashlib
import platform
import unicodedata
import unittest

from clip_score import score_clip


def run(kind, iteration, text):
    return {
        "kind": kind,
        "iteration": iteration,
        "text": text,
        "model_ms": 10.0 if kind == "cold" else 0.0,
        "inference_ms": 20.0 + iteration,
        "total_ms": 30.0 + iteration,
        "model_cached": kind == "warm",
    }


def case(reference="alpha", *, tags=None, origin="human", warm_texts=None):
    audio_sha256 = "a" * 64
    row = {
        "id": "clip_1",
        "language": "en",
        "split": "heldout",
        "speaker_id": "speaker_1",
        "audio_sha256": audio_sha256,
        "reference_sha256": hashlib.sha256(reference.encode("utf-8")).hexdigest(),
        "origin": origin,
        "duration_bucket": "short",
        "environment": "quiet",
        "tags": list(tags or []),
    }
    warm_texts = list(warm_texts or [reference, reference])
    report = {
        "schema_version": 1,
        "build_version": "1.2.3",
        "language": "en",
        "model": "base.en",
        "model_expected_sha256": "b" * 64,
        "model_expected_bytes": 123,
        "source_audio_sha256": audio_sha256,
        "decoded_audio_sha256": "c" * 64,
        "duration_seconds": 1.0,
        "decode_duration_ms": 1.0,
        "beam_size": 0,
        "temperature_fallback": True,
        "allow_empty": origin == "silence",
        "measurement_endpoint": "live_inference_call_not_visible_text",
        "cold_definition": "first_call_in_new_backend_not_system_cold",
        "cli_checks_passed": True,
        "runs": [run("cold", 0, reference)]
        + [run("warm", index, text) for index, text in enumerate(warm_texts, 1)],
    }
    return {"schema_version": 1, "utterances": [row]}, report


class ClipScoreTests(unittest.TestCase):
    def test_first_warm_is_fixed_accuracy_selection_and_all_warms_are_scored(self):
        manifest, report = case(
            tags=["specialist"],
            warm_texts=["wrong", "alpha", "alpha alpha"],
        )
        result = score_clip(manifest, "clip_1", report, "alpha", ["alpha"], [])

        self.assertEqual(result["decision"], "inconclusive")
        self.assertEqual(result["first_warm_accuracy_iteration"], 1)
        self.assertEqual(result["first_warm_accuracy"]["iteration"], 1)
        self.assertEqual(result["first_warm_accuracy"], result["warm_metrics"][0])
        self.assertEqual(len(result["warm_metrics"]), 3)
        self.assertEqual(result["warm_text_variants"], 3)
        self.assertGreater(
            result["warm_metrics"][0]["text_metrics"]["wer"],
            result["warm_metrics"][1]["text_metrics"]["wer"],
        )

    def test_source_language_reference_and_id_must_match_without_echoing_values(self):
        manifest, report = case(reference="Hello world")
        sentinel = "PRIVATE_CLIP_SENTINEL_187"
        invalid_cases = []

        wrong_source = copy.deepcopy(report)
        wrong_source["source_audio_sha256"] = "d" * 64
        invalid_cases.append(("clip_1", wrong_source, "Hello world"))

        wrong_language = copy.deepcopy(report)
        wrong_language["language"] = "sv"
        invalid_cases.append(("clip_1", wrong_language, "Hello world"))

        invalid_cases.append(("missing", report, "Hello world"))
        invalid_cases.append(("clip_1", report, " Hello world"))
        invalid_cases.append((sentinel, report, "Hello world"))

        for utterance_id, candidate_report, reference in invalid_cases:
            with self.assertRaises(ValueError) as raised:
                score_clip(manifest, utterance_id, candidate_report, reference, [], [])
            self.assertNotIn(sentinel, str(raised.exception))
        result_repr = repr(score_clip(manifest, "clip_1", report, "Hello world", [], []))
        self.assertNotIn("clip_1", result_repr)
        self.assertNotIn("a" * 64, result_repr)
        self.assertNotIn("Hello world", result_repr)

    def test_silence_reports_hallucination_and_ordinary_missing_annotation_stays_none(self):
        silence_manifest, silence_report = case(
            reference="",
            origin="silence",
            tags=["silence"],
            warm_texts=["[BLANK_AUDIO]", "spoken"],
        )
        silence = score_clip(silence_manifest, "clip_1", silence_report, "", [], [])
        self.assertTrue(silence["cold"]["text_metrics"]["silence_hallucination"] is False)
        self.assertTrue(
            silence["warm_metrics"][0]["text_metrics"]["silence_hallucination"]
        )
        self.assertTrue(
            silence["warm_metrics"][1]["text_metrics"]["silence_hallucination"]
        )

        ordinary_manifest, ordinary_report = case(
            reference="ordinary words",
            tags=["ordinary"],
            warm_texts=["ordinary words", "ordinary words"],
        )
        ordinary = score_clip(
            ordinary_manifest,
            "clip_1",
            ordinary_report,
            "ordinary words",
            [],
            [],
        )
        self.assertIsNone(
            ordinary["warm_metrics"][0]["text_metrics"]["false_glossary_replacements"]
        )

    def test_manifest_tags_require_their_metric_terms(self):
        manifest, report = case(tags=["specialist"])
        with self.assertRaises(ValueError):
            score_clip(manifest, "clip_1", report, "alpha", [], [])

        manifest, report = case(reference="not 42", tags=["numbers", "negation"])
        with self.assertRaises(ValueError):
            score_clip(manifest, "clip_1", report, "not 42", [], [])

        result = score_clip(manifest, "clip_1", report, "not 42", [], ["not", "42"])
        self.assertEqual(result["warm_metrics"][0]["text_metrics"]["control_errors"], 0)

    def test_synthetic_clip_is_scored_but_never_adoptable(self):
        manifest, report = case(origin="synthetic", warm_texts=["alpha", "alpha"])
        result = score_clip(manifest, "clip_1", report, "alpha", [], [])
        self.assertEqual(result["decision"], "inconclusive")
        self.assertEqual(result["schema_version"], 1)
        self.assertEqual(result["measurement_endpoint"], "live_inference_call_not_visible_text")
        self.assertEqual(result["normalization_version"], "nfc-casefold-nfc-words-v1")
        self.assertEqual(result["python_version"], platform.python_version())
        self.assertEqual(result["unicode_version"], unicodedata.unidata_version)

    def test_non_silence_human_and_synthetic_references_need_normalized_words(self):
        for origin in ("human", "synthetic"):
            for reference in ("", "   ", "?!"):
                manifest, report = case(reference=reference, origin=origin)
                with self.subTest(origin=origin, reference=reference):
                    with self.assertRaises(ValueError):
                        score_clip(manifest, "clip_1", report, reference, [], [])

    def test_quality_report_warm_count_bounds_are_preserved(self):
        manifest, report = case()
        self.assertEqual(len(score_clip(manifest, "clip_1", report, "alpha", [], [])["warm_metrics"]), 2)

        too_few = copy.deepcopy(report)
        too_few["runs"] = too_few["runs"][:2]
        with self.assertRaises(ValueError):
            score_clip(manifest, "clip_1", too_few, "alpha", [], [])

        too_many = copy.deepcopy(report)
        for iteration in range(3, 32):
            too_many["runs"].append(run("warm", iteration, "alpha"))
        with self.assertRaises(ValueError):
            score_clip(manifest, "clip_1", too_many, "alpha", [], [])


if __name__ == "__main__":
    unittest.main()
