import json
from pathlib import Path
import tempfile
import subprocess
import sys
import unittest
from unittest import mock

from runner import run_evaluation
import test_runner as fixtures


class RunSummaryTests(unittest.TestCase):
    def fixture(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        fixture = fixtures.RunnerFixture(root)
        output = root / "output"
        with mock.patch("runner.subprocess.run", side_effect=fixtures.RunnerTests._child_callback(fixture, [])):
            run_evaluation(**fixture.run_kwargs(), output_dir=str(output))
        return fixture, output

    def summarize(self, fixture, output):
        from run_summary import summarize_run
        return summarize_run(fixture.manifest, fixture.plan, fixture.reference_map,
                             fixture.terms_map, str(output))

    def test_recomputes_complete_aggregate_without_private_content_or_writes(self):
        fixture, output = self.fixture()
        before = {p.name: p.read_bytes() for p in output.iterdir()}
        result = self.summarize(fixture, output)
        self.assertEqual(result["completed"], 1)
        self.assertEqual(result["decision"], "inconclusive")
        row = result["configurations"][0]
        self.assertEqual(row["metrics"]["reference_words"], 1)
        self.assertEqual(row["metrics"]["wer"], 0)
        self.assertEqual(row["metrics"]["warm"]["count"], 5)
        for private in ("alpha", "clip_1", "en_baseline", str(output), fixture.binary_sha256):
            self.assertNotIn(private, json.dumps(result))
        self.assertEqual(before, {p.name: p.read_bytes() for p in output.iterdir()})

    def test_cli_summary_is_content_free_and_invalid_input_is_exit_two(self):
        fixture, output = self.fixture()
        root = output.parent
        for name, value in (("input-manifest.json", fixture.manifest), ("input-plan.json", fixture.plan),
                            ("reference-map.json", fixture.reference_map), ("terms.json", fixture.terms_map)):
            (root / name).write_text(json.dumps(value))
        argv = [sys.executable, str(Path(__file__).with_name("evaluate.py")), "summarize-run",
                "--manifest", str(root / "input-manifest.json"), "--plan", str(root / "input-plan.json"),
                "--reference-map", str(root / "reference-map.json"), "--terms", str(root / "terms.json"),
                "--output-dir", str(output)]
        result = subprocess.run(argv, capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("alpha", result.stdout)
        self.assertEqual(json.loads(result.stdout)["completed"], 1)
        (output / "00000-result.json").unlink()
        invalid = subprocess.run(argv, capture_output=True, text=True, check=False)
        self.assertEqual(invalid.returncode, 2)
        self.assertEqual(invalid.stdout, "")
        self.assertNotIn(str(root), invalid.stderr)

    def test_missing_extra_or_wrong_identity_result_rejected(self):
        for mutation in ("missing", "extra", "index", "pair", "hash", "score", "version"):
            with self.subTest(mutation=mutation):
                fixture, output = self.fixture()
                path = output / "00000-result.json"
                row = json.loads(path.read_text())
                if mutation == "missing":
                    path.unlink()
                elif mutation == "extra":
                    (output / "00001-result.json").write_text(json.dumps(row))
                else:
                    if mutation == "index": row["index"] = False
                    if mutation == "pair": row["utterance_id"] = "different"
                    if mutation == "hash": row["source_audio_sha256_after"] = "b" * 64
                    if mutation == "score": row["score"]["first_warm_accuracy"]["text_metrics"]["wer"] = 1
                    if mutation == "version": row["score"]["python_version"] = "different"
                    path.write_text(json.dumps(row))
                with self.assertRaises(ValueError): self.summarize(fixture, output)

    def test_report_reference_plan_and_summary_mutations_rejected(self):
        for mutation in ("report", "reference", "plan", "summary", "duplicate", "symlink"):
            with self.subTest(mutation=mutation):
                fixture, output = self.fixture()
                if mutation == "reference": fixture.reference.write_text("changed")
                elif mutation == "duplicate":
                    (output / "summary.json").write_text('{"schema_version":1,"schema_version":1}')
                elif mutation == "symlink":
                    path = output / "00000-result.json"
                    other = output / "private-result-copy"
                    path.rename(other)
                    try: path.symlink_to(other)
                    except OSError: continue  # Host may lack symlink privilege (Windows).
                else:
                    path = output / {"report":"00000-report.json", "plan":"plan.json", "summary":"summary.json"}[mutation]
                    row = json.loads(path.read_text())
                    if mutation == "report": row["cli_checks_passed"] = False
                    if mutation == "plan": row["seed"] = 188
                    if mutation == "summary": row["completed"] = 0
                    path.write_text(json.dumps(row))
                with self.assertRaises(ValueError): self.summarize(fixture, output)

    def test_failed_pair_count_retained_and_no_success_subset_metrics(self):
        fixture, output = self.fixture()
        path = output / "00000-result.json"
        row = json.loads(path.read_text())
        row.update(status="timeout", exit_code=None, score=None)
        path.write_text(json.dumps(row))
        summary = json.loads((output / "summary.json").read_text())
        summary.update(completed=0, failed=1, statuses={"timeout": 1})
        (output / "summary.json").write_text(json.dumps(summary))
        result = self.summarize(fixture, output)
        self.assertEqual(result["failed"], 1)
        self.assertEqual(result["statuses"], {"timeout": 1})
        self.assertIsNone(result["configurations"][0]["metrics"])
        self.assertEqual(result["comparisons"], [])

    def test_silence_not_in_speech_wer_but_retained_as_control(self):
        from clip_score import score_clip
        from paired_plan import build_plan
        fixture, output = self.fixture()
        fixture.reference.write_text("")
        import hashlib
        row = fixture.manifest["utterances"][0]
        row.update(origin="silence", tags=["silence"], reference_sha256=hashlib.sha256(b"").hexdigest())
        fixture.plan = build_plan(fixture.manifest, fixture.plan["configurations"], split="heldout",
                                 seed=187, iterations=5, source_revision="a" * 40,
                                 binary_sha256=fixture.binary_sha256)
        report = fixture.report
        report["allow_empty"] = True
        score = score_clip(fixture.manifest, "clip_1", report, "", [], [])
        result = json.loads((output / "00000-result.json").read_text())
        result["score"] = score
        for name, value in (("manifest.json", fixture.manifest), ("plan.json", fixture.plan),
                            ("00000-report.json", report), ("00000-result.json", result)):
            (output / name).write_text(json.dumps(value))
        actual = self.summarize(fixture, output)["configurations"][0]
        self.assertIsNone(actual["metrics"])
        self.assertEqual(actual["controls"]["silence_utterances"], 1)
        self.assertEqual(actual["controls"]["first_warm_silence_hallucinations"], 1)


if __name__ == "__main__":
    unittest.main()
