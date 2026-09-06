import copy
import hashlib
import json
import unittest
from collections import Counter

from paired_plan import build_plan


def row(number, *, language="en", split="heldout"):
    return {
        "id": f"utterance_{number}",
        "language": language,
        "split": split,
        "speaker_id": "speaker_1",
        "audio_sha256": f"{number:064x}",
        "reference_sha256": f"{number + 1000:064x}",
        "origin": "human",
        "duration_bucket": "short",
        "environment": "quiet",
        "tags": [],
    }


def manifest(rows):
    return {"schema_version": 1, "utterances": rows}


def configs(*, languages=("en",), roles=("baseline",)):
    models = {
        "baseline": {
            "en": "base.en",
            "sv": "kb-whisper-base",
            "no": "nb-whisper-base",
            "fi": "base",
            "pl": "base",
        },
        "smaller": {
            "en": "tiny.en",
            "sv": "kb-whisper-tiny",
            "no": "nb-whisper-tiny",
            "fi": "tiny",
            "pl": "tiny",
        },
        "decoder": {
            "en": "base.en",
            "sv": "kb-whisper-base",
            "no": "nb-whisper-base",
            "fi": "base",
            "pl": "base",
        },
    }
    return [
        {
            "id": f"{language}_{role}",
            "language": language,
            "model": models[role][language],
            "beam_size": 2 if role == "decoder" else 0,
            "temperature_fallback": True,
            "role": role,
        }
        for language in languages
        for role in roles
    ]


def make_plan(candidate_manifest=None, candidate_configs=None, **kwargs):
    return build_plan(
        candidate_manifest
        or manifest(
            [
                row(1, language="en", split="dev"),
                row(2, language="en", split="dev"),
                row(3, language="sv", split="dev"),
            ]
        ),
        candidate_configs or configs(languages=("en", "sv")),
        split=kwargs.get("split", "dev"),
        seed=kwargs.get("seed", 187),
        iterations=kwargs.get("iterations", 5),
        source_revision=kwargs.get("source_revision", "a" * 40),
        binary_sha256=kwargs.get("binary_sha256", "b" * 64),
    )


class PairedPlanTests(unittest.TestCase):
    def test_plan_is_deterministic_seeded_and_does_not_mutate_inputs(self):
        source_manifest = manifest(
            [row(index, language="en", split="dev") for index in range(1, 9)]
        )
        source_configs = configs(languages=("en",), roles=("baseline", "smaller"))
        manifest_before = copy.deepcopy(source_manifest)
        configs_before = copy.deepcopy(source_configs)

        first = make_plan(source_manifest, source_configs, seed=187)
        second = make_plan(source_manifest, source_configs, seed=187)
        different = make_plan(source_manifest, source_configs, seed=188)
        self.assertEqual(first, second)
        self.assertNotEqual(first["order"], different["order"])
        self.assertEqual(source_manifest, manifest_before)
        self.assertEqual(source_configs, configs_before)

    def test_every_selected_utterance_pairs_once_with_each_same_language_config(self):
        source_manifest = manifest(
            [
                row(1, language="en", split="dev"),
                row(2, language="en", split="dev"),
                row(3, language="sv", split="dev"),
                row(4, language="en", split="heldout"),
            ]
        )
        source_configs = configs(languages=("en", "sv"), roles=("baseline", "smaller"))
        result = make_plan(source_manifest, source_configs)
        self.assertEqual(len(result["order"]), 6)
        pairs = Counter(
            (item["utterance_id"], item["configuration_id"]) for item in result["order"]
        )
        self.assertTrue(all(count == 1 for count in pairs.values()))
        self.assertEqual(
            {item["utterance_id"] for item in result["order"]},
            {"utterance_1", "utterance_2", "utterance_3"},
        )
        for item in result["order"]:
            expected_language = item["utterance_id"] == "utterance_3" and "sv" or "en"
            self.assertTrue(item["configuration_id"].startswith(expected_language + "_"))

    def test_split_isolation_and_canonical_input_hashes(self):
        source_manifest = manifest(
            [row(1, split="dev"), row(2, split="heldout")]
        )
        result = make_plan(source_manifest, configs(), split="dev")
        self.assertEqual([item["utterance_id"] for item in result["order"]], ["utterance_1"])
        canonical_manifest = json.dumps(
            {"schema_version": 1, "utterances": [row(1, split="dev"), row(2, split="heldout")]},
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
            allow_nan=False,
        ).encode("utf-8")
        self.assertEqual(result["manifest_sha256"], hashlib.sha256(canonical_manifest).hexdigest())
        self.assertEqual(result["source_revision"], "a" * 40)
        self.assertEqual(result["binary_sha256"], "b" * 64)

    def test_configuration_roles_and_languages_are_strict(self):
        cases = []
        cases.append(configs(languages=("en",), roles=("smaller",)))
        duplicate_role = configs(languages=("en",), roles=("baseline", "baseline"))
        duplicate_role[1]["id"] = "en_baseline_2"
        cases.append(duplicate_role)
        duplicate = configs(languages=("en",), roles=("baseline", "smaller"))
        duplicate[1]["model"] = duplicate[0]["model"]
        duplicate[1]["beam_size"] = duplicate[0]["beam_size"]
        duplicate[1]["temperature_fallback"] = duplicate[0]["temperature_fallback"]
        duplicate[1]["role"] = "decoder"
        cases.append(duplicate)
        unknown = configs()
        unknown[0]["model"] = "unknown"
        cases.append(unknown)
        bad_role = configs()
        bad_role[0]["role"] = "other"
        cases.append(bad_role)
        bad_id = configs()
        bad_id[0]["id"] = "id with spaces"
        cases.append(bad_id)
        for candidate in cases:
            with self.assertRaises(ValueError):
                make_plan(candidate_configs=candidate)

        no_rows = configs(languages=("sv",))
        with self.assertRaises(ValueError):
            make_plan(candidate_configs=no_rows)

    def test_finnish_language_and_model_are_accepted(self):
        candidate_manifest = manifest([row(1, language="fi", split="dev")])
        result = make_plan(
            candidate_manifest=candidate_manifest,
            candidate_configs=configs(languages=("fi",)),
        )
        self.assertEqual(result["configurations"][0]["language"], "fi")
        self.assertEqual(result["configurations"][0]["model"], "base")

    def test_finnish_specialist_tiny_model_is_accepted(self):
        candidate_manifest = manifest([row(1, language="fi", split="dev")])
        candidate_configs = configs(languages=("fi",))
        candidate_configs[0]["model"] = "fi-whisper-tiny"
        result = make_plan(
            candidate_manifest=candidate_manifest,
            candidate_configs=candidate_configs,
        )
        self.assertEqual(result["configurations"][0]["language"], "fi")
        self.assertEqual(result["configurations"][0]["model"], "fi-whisper-tiny")

    def test_polish_language_and_specialist_model_are_accepted(self):
        candidate_manifest = manifest([row(1, language="pl", split="dev")])
        candidate_configs = configs(languages=("pl",))
        candidate_configs[0]["model"] = "pl-whisper-small"
        result = make_plan(
            candidate_manifest=candidate_manifest,
            candidate_configs=candidate_configs,
        )
        self.assertEqual(result["configurations"][0]["language"], "pl")
        self.assertEqual(result["configurations"][0]["model"], "pl-whisper-small")

    def test_shape_and_numeric_bounds_are_rejected(self):
        valid_manifest = manifest([row(1, split="dev")])
        valid_configs = configs()
        invalid_calls = [
            lambda: make_plan(candidate_manifest=None, split="test"),
            lambda: make_plan(candidate_manifest=None, seed=True),
            lambda: make_plan(candidate_manifest=None, seed=-1),
            lambda: make_plan(candidate_manifest=None, seed=2**32),
            lambda: make_plan(candidate_manifest=None, iterations=4),
            lambda: make_plan(candidate_manifest=None, iterations=21),
            lambda: make_plan(candidate_manifest=None, source_revision="A" * 40),
            lambda: make_plan(candidate_manifest=None, source_revision="a" * 39),
            lambda: make_plan(candidate_manifest=None, binary_sha256="b" * 63),
            lambda: build_plan(
                valid_manifest,
                tuple(valid_configs),
                split="dev",
                seed=187,
                iterations=5,
                source_revision="a" * 40,
                binary_sha256="b" * 64,
            ),
        ]
        for call in invalid_calls:
            with self.assertRaises(ValueError):
                call()

        bad_beam = configs()
        bad_beam[0]["beam_size"] = 1
        with self.assertRaises(ValueError):
            make_plan(candidate_configs=bad_beam)

        bad_bool = configs()
        bad_bool[0]["temperature_fallback"] = 1
        with self.assertRaises(ValueError):
            make_plan(candidate_configs=bad_bool)


if __name__ == "__main__":
    unittest.main()
