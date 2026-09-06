"""Subprocess checks for the local-only evaluation command."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from test_clip_score import case


SCRIPT = Path(__file__).with_name("evaluate.py")


class EvaluateCliTests(unittest.TestCase):
    def test_ci_selects_supported_python_before_all_evaluator_suites(self):
        workflow = SCRIPT.parents[2] / ".github" / "workflows" / "ci.yml"
        source = workflow.read_text(encoding="utf-8")
        for name in ("check-macos", "check-linux", "check-windows"):
            job = source.split(f"  {name}:\n", 1)[1].split("\n  check-", 1)[0]
            setup = job.index("actions/setup-python@ece7cb06caefa5fff74198d8649806c4678c61a1")
            suite = job.index("Test offline dictation evaluation tooling")
            self.assertLess(setup, suite)
            self.assertIn('python-version: "3.12"', job[setup:suite])

    def run_cli(self, *args):
        return subprocess.run(
            [sys.executable, str(SCRIPT), *map(str, args)],
            capture_output=True, text=True, check=False,
        )

    def test_help_and_version(self):
        result = self.run_cli("--help")
        self.assertEqual(result.returncode, 0)
        self.assertIn("validate-manifest", result.stdout)
        self.assertIn("score-clip", result.stdout)
        self.assertIn("0.3.0", self.run_cli("--version").stdout)

    def test_manifest_command_emits_coverage_without_identifiers(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "manifest.json"
            path.write_text(json.dumps({"schema_version": 1, "utterances": [{
                "id": "private_utterance", "speaker_id": "private_speaker",
                "language": "en", "split": "dev", "origin": "human",
                "audio_sha256": "a" * 64, "reference_sha256": "b" * 64,
                "duration_bucket": "short", "environment": "quiet",
                "tags": ["ordinary"],
            }]}), encoding="utf-8")
            result = self.run_cli("validate-manifest", path)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse(json.loads(result.stdout)["eligible"])
            for value in ("private_utterance", "private_speaker", "a" * 64, directory):
                self.assertNotIn(value, result.stdout + result.stderr)

    def test_errors_are_content_free_and_duplicate_keys_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "PRIVATE_INPUT.json"
            for data in (
                '{"schema_version":1,"schema_version":1,"utterances":[]}',
                '{"private_content": "SECRET_PHRASE"}',
                '{"schema_version":NaN}',
                'not JSON SECRET_PHRASE',
            ):
                path.write_text(data, encoding="utf-8")
                result = self.run_cli("validate-manifest", path)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.stdout, "")
                self.assertEqual(result.stderr, "Invalid local evaluation input; no result produced.\n")
                self.assertNotIn("SECRET_PHRASE", result.stderr)
                self.assertNotIn(directory, result.stderr)
                self.assertNotIn("Traceback", result.stderr)
            result = self.run_cli("validate-manifest", Path(directory) / "absent")
            self.assertEqual(result.returncode, 2)
            self.assertNotIn(directory, result.stderr)

    def test_score_clip_binds_reference_and_preserves_first_warm(self):
        reference = "PRIVATE_REVIEW_SENTINEL_187 alpha"
        manifest, report = case(reference=reference, warm_texts=["wrong", reference])
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
            (root / "report.json").write_text(json.dumps(report), encoding="utf-8")
            (root / "reference.txt").write_text(reference, encoding="utf-8")
            args = ("score-clip", "--manifest", root / "manifest.json",
                    "--utterance-id", "clip_1", "--report", root / "report.json",
                    "--reference", root / "reference.txt")
            result = self.run_cli(*args)
            self.assertEqual(result.returncode, 0, result.stderr)
            scored = json.loads(result.stdout)
            self.assertEqual(scored["decision"], "inconclusive")
            self.assertEqual(scored["first_warm_accuracy_iteration"], 1)
            self.assertGreater(scored["first_warm_accuracy"]["text_metrics"]["wer"], 0)
            self.assertEqual(scored["warm_metrics"][1]["text_metrics"]["wer"], 0)
            for value in (reference, "PRIVATE_REVIEW_SENTINEL_187", directory, "clip_1"):
                self.assertNotIn(value, result.stdout + result.stderr)
            (root / "reference.txt").write_text(reference + "\n", encoding="utf-8")
            invalid = self.run_cli(*args)
            self.assertEqual(invalid.returncode, 2)
            self.assertEqual(invalid.stdout, "")
            self.assertEqual(invalid.stderr, "Invalid local evaluation input; no result produced.\n")

    def test_oversized_manifest_is_rejected_before_json_parsing(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "oversized.json"
            path.write_bytes(b" " * (16 * 1024 * 1024 + 1))
            result = self.run_cli("validate-manifest", path)
            self.assertEqual(result.returncode, 2)
            self.assertEqual(result.stdout, "")
            self.assertEqual(result.stderr, "Invalid local evaluation input; no result produced.\n")


if __name__ == "__main__":
    unittest.main()
