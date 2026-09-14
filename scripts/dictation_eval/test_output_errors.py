import contextlib
import hashlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import evaluate
from paired_plan import build_plan
from runner import ExecutionOutputError, freeze_plan, run_evaluation
import test_runner
from test_runner import RunnerFixture, _config


class OutputErrorTests(unittest.TestCase):
    def test_silence_run_retains_hallucinations_and_literal_allow_empty(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = RunnerFixture(root)
            fixture.reference.write_bytes(b"\n")
            row = fixture.manifest["utterances"][0]
            row.update(origin="silence", tags=["silence"],
                       reference_sha256=hashlib.sha256(b"\n").hexdigest())
            fixture.report["allow_empty"] = True
            for run in fixture.report["runs"]:
                run["text"] = "tack tack [BLANK_AUDIO]"
            fixture.plan = build_plan(fixture.manifest, [_config()], split="heldout",
                                      seed=187, iterations=5, source_revision="a" * 40,
                                      binary_sha256=fixture.binary_sha256)
            calls = []
            with mock.patch("runner.subprocess.run",
                            side_effect=test_runner.RunnerTests._child_callback(fixture, calls)):
                summary = run_evaluation(**fixture.run_kwargs(), output_dir=root / "run")
            self.assertIn("--allow-empty", calls[0][0])
            self.assertEqual(summary["decision"], "inconclusive")
            record = json.loads((root / "run" / "00000-result.json").read_text())
            self.assertTrue(all(run["text_metrics"]["silence_hallucination"]
                                for run in record["score"]["warm_metrics"]))

    def test_partial_freeze_is_retained_with_distinct_error(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = RunnerFixture(root)
            output = root / "plan.json"
            with mock.patch("runner.os.fsync", side_effect=OSError("PRIVATE_PATH")):
                with self.assertRaises(ExecutionOutputError) as failure:
                    freeze_plan(fixture.manifest, [_config()], split="heldout", seed=187,
                                iterations=5, source_revision="a" * 40,
                                binary_path=fixture.binary, output_path=output)
            self.assertTrue(output.exists())
            self.assertNotIn("PRIVATE_PATH", str(failure.exception))

    def test_partial_execution_is_retained_without_launch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = RunnerFixture(root)
            output = root / "new-output"
            with mock.patch("runner.os.fsync", side_effect=OSError("PRIVATE_PATH")), \
                    mock.patch("runner.subprocess.run") as child:
                with self.assertRaises(ExecutionOutputError):
                    run_evaluation(**fixture.run_kwargs(), output_dir=output)
            child.assert_not_called()
            self.assertTrue((output / "plan.json").exists())

    def test_missing_output_parent_remains_preflight_error(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = RunnerFixture(root)
            with mock.patch("runner.subprocess.run") as child:
                with self.assertRaises(ValueError):
                    run_evaluation(**fixture.run_kwargs(), output_dir=root / "missing" / "run")
            child.assert_not_called()
            self.assertFalse((root / "missing").exists())

    def test_cli_distinguishes_partial_output_from_invalid_input(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = root / "manifest.json"
            config = root / "config.json"
            manifest.write_text(json.dumps({}), encoding="utf-8")
            config.write_text(json.dumps([]), encoding="utf-8")
            stdout, stderr = io.StringIO(), io.StringIO()
            with mock.patch("runner.freeze_plan", side_effect=ExecutionOutputError("PRIVATE_PATH")), \
                    contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                code = evaluate.main(["freeze-plan", "--manifest", str(manifest),
                                      "--configurations", str(config), "--split", "dev",
                                      "--seed", "187", "--source-revision", "a" * 40,
                                      "--binary", str(root / "binary"), "--output", str(root / "new")])
            self.assertEqual(code, 3)
            self.assertEqual(stdout.getvalue(), "")
            self.assertIn("retain the private partial output", stderr.getvalue())
            self.assertNotIn("PRIVATE_PATH", stderr.getvalue())
            self.assertNotIn(str(root), stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
