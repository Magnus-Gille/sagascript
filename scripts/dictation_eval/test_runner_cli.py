import contextlib
import io
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import types
import unittest
from unittest import mock

from evaluate import main


SCRIPT = Path(__file__).with_name("evaluate.py")


def manifest():
    return {
        "schema_version": 1,
        "utterances": [
            {
                "id": "clip_1",
                "language": "en",
                "split": "dev",
                "speaker_id": "speaker_1",
                "audio_sha256": "a" * 64,
                "reference_sha256": "b" * 64,
                "origin": "human",
                "duration_bucket": "short",
                "environment": "quiet",
                "tags": [],
            }
        ],
    }


def configurations():
    return [
        {
            "id": "en_baseline",
            "language": "en",
            "model": "base.en",
            "beam_size": 0,
            "temperature_fallback": True,
            "role": "baseline",
        }
    ]


class RunnerCliTests(unittest.TestCase):
    def run_cli(self, *args):
        return subprocess.run(
            [sys.executable, str(SCRIPT), *map(str, args)],
            capture_output=True,
            text=True,
            check=False,
        )

    def write_json(self, directory, name, value):
        path = Path(directory) / name
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def test_version_and_help_expose_runner_commands(self):
        version = self.run_cli("--version")
        self.assertEqual(version.returncode, 0)
        self.assertIn("0.2.0", version.stdout)
        help_result = self.run_cli("--help")
        self.assertEqual(help_result.returncode, 0)
        self.assertIn("freeze-plan", help_result.stdout)
        self.assertIn("run-plan", help_result.stdout)

    def test_freeze_plan_subprocess_writes_private_new_file(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest_path = self.write_json(root, "manifest.json", manifest())
            configs_path = self.write_json(root, "configs.json", configurations())
            output = root / "frozen plan.json"
            binary = str(Path(sys.executable).resolve())
            result = self.run_cli(
                "freeze-plan",
                "--manifest",
                manifest_path,
                "--configurations",
                configs_path,
                "--split",
                "dev",
                "--seed",
                "187",
                "--iterations",
                "5",
                "--source-revision",
                "a" * 40,
                "--binary",
                binary,
                "--output",
                output,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            summary = json.loads(result.stdout)
            self.assertEqual(summary, {
                "schema_version": 1,
                "planned": 1,
                "decision": "inconclusive",
                "plan_written": True,
            })
            self.assertFalse("clip_1" in result.stdout or "a" * 64 in result.stdout)
            if os.name == "posix":
                self.assertEqual(stat.S_IMODE(output.stat().st_mode), 0o600)
            frozen = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(frozen["order"], [{"utterance_id": "clip_1", "configuration_id": "en_baseline"}])
            original = output.read_bytes()

            second = self.run_cli(
                "freeze-plan",
                "--manifest",
                manifest_path,
                "--configurations",
                configs_path,
                "--split",
                "dev",
                "--seed",
                "187",
                "--source-revision",
                "a" * 40,
                "--binary",
                binary,
                "--output",
                output,
            )
            self.assertEqual(second.returncode, 2)
            self.assertEqual(second.stdout, "")
            self.assertEqual(second.stderr, "Invalid local evaluation input; no result produced.\n")
            self.assertEqual(output.read_bytes(), original)

    def test_run_plan_input_failure_is_content_free(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest_path = self.write_json(root, "manifest.json", manifest())
            plan_path = self.write_json(root, "plan.json", {})
            missing = root / "PRIVATE_SENTINEL_missing.json"
            binary = (root / "binary").resolve()
            result = self.run_cli(
                "run-plan",
                "--manifest",
                manifest_path,
                "--plan",
                plan_path,
                "--audio-map",
                missing,
                "--reference-map",
                missing,
                "--terms",
                missing,
                "--binary",
                binary,
                "--output-dir",
                root / "output",
            )
            self.assertEqual(result.returncode, 2)
            self.assertEqual(result.stdout, "")
            self.assertEqual(result.stderr, "Invalid local evaluation input; no result produced.\n")
            self.assertNotIn("PRIVATE_SENTINEL", result.stderr)

    def test_run_plan_returns_one_for_retained_failed_summary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            paths = {
                name: self.write_json(root, f"{name}.json", {})
                for name in ("manifest", "plan", "audio", "reference", "terms")
            }
            fake_runner = types.ModuleType("runner")
            fake_runner.run_evaluation = lambda *args: {
                "schema_version": 1,
                "decision": "inconclusive",
                "planned": 2,
                "completed": 1,
                "failed": 1,
                "statuses": {"cli_failed": 1, "completed": 1},
            }
            output = io.StringIO()
            binary = str((root / "binary").resolve())
            with mock.patch.dict(sys.modules, {"runner": fake_runner}):
                with contextlib.redirect_stdout(output):
                    code = main(
                        [
                            "run-plan",
                            "--manifest",
                            str(paths["manifest"]),
                            "--plan",
                            str(paths["plan"]),
                            "--audio-map",
                            str(paths["audio"]),
                            "--reference-map",
                            str(paths["reference"]),
                            "--terms",
                            str(paths["terms"]),
                            "--binary",
                            binary,
                            "--output-dir",
                            str(root / "output"),
                        ]
                    )
            self.assertEqual(code, 1)
            self.assertEqual(json.loads(output.getvalue())["failed"], 1)


if __name__ == "__main__":
    unittest.main()
