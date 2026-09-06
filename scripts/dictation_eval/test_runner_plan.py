import copy
from pathlib import Path, PureWindowsPath
import tempfile
import unittest
from unittest import mock

from paired_plan import build_plan
from runner_plan import build_command, validate_plan


def row(number, *, language="en"):
    return {
        "id": f"utterance_{number}",
        "language": language,
        "split": "dev",
        "speaker_id": "speaker_1",
        "audio_sha256": f"{number:064x}",
        "reference_sha256": f"{number + 1000:064x}",
        "origin": "human",
        "duration_bucket": "short",
        "environment": "quiet",
        "tags": [],
    }


def manifest():
    return {"schema_version": 1, "utterances": [row(1), row(2)]}


def config(*, temperature_fallback=True):
    return {
        "id": "en_baseline",
        "language": "en",
        "model": "base.en",
        "beam_size": 0,
        "temperature_fallback": temperature_fallback,
        "role": "baseline",
    }


def plan(candidate_manifest=None, candidate_config=None):
    candidate_manifest = candidate_manifest or manifest()
    candidate_config = candidate_config or [config()]
    return build_plan(
        candidate_manifest,
        candidate_config,
        split="dev",
        seed=187,
        iterations=5,
        source_revision="a" * 40,
        binary_sha256="b" * 64,
    )


class RunnerPlanTests(unittest.TestCase):
    def test_validate_plan_requires_exact_frozen_order_and_returns_copy(self):
        source_manifest = manifest()
        source_plan = plan(source_manifest)
        result = validate_plan(source_manifest, source_plan)
        self.assertEqual(result, source_plan)
        self.assertIsNot(result, source_plan)
        result["order"][0]["utterance_id"] = "changed"
        self.assertNotEqual(result["order"], source_plan["order"])

        for mutation in (
            lambda candidate: candidate["order"].reverse(),
            lambda candidate: candidate["order"].append(copy.deepcopy(candidate["order"][0])),
            lambda candidate: candidate["order"].pop(),
            lambda candidate: candidate.__setitem__("manifest_sha256", "c" * 64),
            lambda candidate: candidate["configurations"].__setitem__(0, config(temperature_fallback=False)),
        ):
            candidate = copy.deepcopy(source_plan)
            mutation(candidate)
            with self.assertRaises(ValueError):
                validate_plan(source_manifest, candidate)

    def test_validate_plan_rejects_shape_and_bool_scalar_confusion(self):
        source_manifest = manifest()
        source_plan = plan(source_manifest)
        candidates = []
        extra = copy.deepcopy(source_plan)
        extra["extra"] = 1
        candidates.append(extra)
        missing = copy.deepcopy(source_plan)
        del missing["order"]
        candidates.append(missing)
        for key in ("schema_version", "seed", "iterations"):
            candidate = copy.deepcopy(source_plan)
            candidate[key] = True
            candidates.append(candidate)
        for candidate in candidates:
            with self.assertRaises(ValueError):
                validate_plan(source_manifest, candidate)

        changed_manifest = manifest()
        changed_manifest["utterances"][0]["audio_sha256"] = "c" * 64
        with self.assertRaises(ValueError):
            validate_plan(changed_manifest, source_plan)

    def test_build_command_keeps_paths_literal_and_emits_exact_flags(self):
        with tempfile.TemporaryDirectory(prefix="sagascript path ") as directory:
            root = Path(directory).resolve()
            binary = str(root / "tool dir" / "saga;echo")
            audio = str(root / "audio [fixture];.wav")
            report = str(root / "reports" / "run one.json")
            command = build_command(binary, audio, report, config(), 5, False)
            self.assertEqual(
                command,
                [
                    binary,
                    "benchmark-dictation",
                    audio,
                    "--language",
                    "en",
                    "--model",
                    "base.en",
                    "--beam-size",
                    "0",
                    "--iterations",
                    "5",
                    "--quality-output",
                    report,
                ],
            )
            disabled = build_command(binary, audio, report, config(temperature_fallback=False), 20, True)
            self.assertEqual(disabled[-2:], ["--disable-temperature-fallback", "--allow-empty"])
            self.assertNotIn(" ".join(disabled), disabled)

    def test_build_command_accepts_windows_paths_literally(self):
        binary = r"C:\private tool dir\saga;echo.exe"
        audio = r"C:\audio [fixture];.wav"
        report = r"C:\reports\run one.json"
        with mock.patch("runner_plan.Path", PureWindowsPath):
            command = build_command(binary, audio, report, config(), 5, False)
        self.assertEqual(command[0], binary)
        self.assertEqual(command[2], audio)
        self.assertEqual(command[-1], report)
        self.assertIn(";", command[0])
        self.assertIn("[", command[2])

    def test_build_command_rejects_bad_paths_types_and_config(self):
        with tempfile.TemporaryDirectory(prefix="sagascript validation ") as directory:
            root = Path(directory).resolve()
            valid_paths = tuple(str(root / name) for name in ("bin", "audio", "report"))
            for invalid in ("", "relative/path", "/private/with\x00nul"):
                with self.assertRaises(ValueError):
                    build_command(invalid, valid_paths[1], valid_paths[2], config(), 5, False)
                with self.assertRaises(ValueError):
                    build_command(valid_paths[0], invalid, valid_paths[2], config(), 5, False)
                with self.assertRaises(ValueError):
                    build_command(valid_paths[0], valid_paths[1], invalid, config(), 5, False)
            for iterations in (True, 4, 21):
                with self.assertRaises(ValueError):
                    build_command(*valid_paths, config(), iterations, False)
            with self.assertRaises(ValueError):
                build_command(*valid_paths, config(), 5, 1)
            bad_config = config()
            bad_config["model"] = "unknown"
            with self.assertRaises(ValueError):
                build_command(*valid_paths, bad_config, 5, False)
            bad_config = config()
            bad_config["beam_size"] = True
            with self.assertRaises(ValueError):
                build_command(*valid_paths, bad_config, 5, False)


if __name__ == "__main__":
    unittest.main()
