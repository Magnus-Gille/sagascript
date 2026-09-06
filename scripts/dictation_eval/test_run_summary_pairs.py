"""Regression coverage for paired, content-free run summaries."""

from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

import test_runner as fixtures
from paired_plan import build_plan
from runner import run_evaluation
from run_summary import summarize_run


SOURCE_REVISION = "a" * 40


def _sha256(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def _run(kind: str, iteration: int, text: str, total_ms: float) -> dict[str, object]:
    return {
        "kind": kind,
        "iteration": iteration,
        "text": text,
        "model_ms": 10.0 if kind == "cold" else 0.0,
        "inference_ms": total_ms,
        "total_ms": total_ms,
        "model_cached": kind == "warm",
    }


def _config(config_id: str, model: str, role: str) -> dict[str, object]:
    return {
        "id": config_id,
        "language": "en",
        "model": model,
        "beam_size": 0,
        "temperature_fallback": True,
        "role": role,
    }


class PairedSummaryFixture:
    """Build a real runner ledger with human, synthetic, and silence rows."""

    def __init__(self, root: Path, *, zero_baseline_warm: bool = False):
        self.root = root
        self.audio: dict[str, Path] = {}
        self.references: dict[str, Path] = {}
        self.rows: dict[str, dict[str, object]] = {}
        self.reports: dict[str, dict[str, object]] = {}
        self.terms_map: dict[str, dict[str, list[str]]] = {
            "human_short": {"specialist_terms": ["alpha"], "control_terms": []},
            "human_long": {
                "specialist_terms": ["alpha"],
                "control_terms": ["not", "42"],
            },
            "synthetic": {"specialist_terms": [], "control_terms": []},
            "silence": {"specialist_terms": [], "control_terms": []},
        }
        self._add_row(
            "human_short",
            "alpha beta",
            origin="human",
            tags=["specialist"],
            duration_bucket="short",
            environment="quiet",
        )
        self._add_row(
            "human_long",
            "not 42 alpha beta",
            origin="human",
            tags=["specialist", "numbers", "negation", "ordinary"],
            duration_bucket="long",
            environment="unknown",
        )
        self._add_row(
            "synthetic",
            "synthetic words",
            origin="synthetic",
            tags=[],
            duration_bucket="medium",
            environment="noisy",
        )
        self._add_row(
            "silence",
            "",
            origin="silence",
            tags=["silence"],
            duration_bucket="short",
            environment="quiet",
        )

        self.manifest = {"schema_version": 1, "utterances": list(self.rows.values())}
        self.configurations = [
            _config("en_baseline", "base.en", "baseline"),
            _config("en_smaller", "tiny.en", "smaller"),
        ]
        self.binary = Path(sys.executable).resolve()
        self.binary_sha256 = _sha256(self.binary.read_bytes())
        self.plan = build_plan(
            self.manifest,
            self.configurations,
            split="heldout",
            seed=187,
            iterations=5,
            source_revision=SOURCE_REVISION,
            binary_sha256=self.binary_sha256,
        )
        self.audio_map = {key: str(path.resolve()) for key, path in self.audio.items()}
        self.reference_map = {
            key: str(path.resolve()) for key, path in self.references.items()
        }
        self.zero_baseline_warm = zero_baseline_warm

    def _add_row(
        self,
        identifier: str,
        reference: str,
        *,
        origin: str,
        tags: list[str],
        duration_bucket: str,
        environment: str,
    ) -> None:
        audio = self.root / f"{identifier}.wav"
        reference_path = self.root / f"{identifier}.txt"
        audio.write_bytes(f"audio bytes for {identifier}".encode())
        reference_path.write_text(reference, encoding="utf-8")
        audio_sha256 = _sha256(audio.read_bytes())
        row = {
            "id": identifier,
            "language": "en",
            "split": "heldout",
            "speaker_id": "speaker_1" if identifier != "human_long" else "speaker_2",
            "audio_sha256": audio_sha256,
            "reference_sha256": _sha256(reference.encode()),
            "origin": origin,
            "duration_bucket": duration_bucket,
            "environment": environment,
            "tags": tags,
        }
        _, report = fixtures.case(reference=reference, tags=tags, origin=origin, warm_texts=[reference] * 5)
        report["source_audio_sha256"] = audio_sha256
        report["allow_empty"] = origin == "silence"
        self.audio[identifier] = audio
        self.references[identifier] = reference_path
        self.rows[identifier] = row
        self.reports[identifier] = report

    def _warm_texts(self, identifier: str, model: str) -> list[str]:
        if identifier == "human_short":
            if model == "base.en":
                return ["wrong", "alpha beta", "alpha beta", "alpha beta", "alpha beta"]
            return ["alpha beta"] * 5
        if identifier == "human_long":
            return ["not 42 alpha beta"] * 5
        if identifier == "synthetic":
            return ["synthetic words"] * 5
        return [""] * 5

    def _report_for(self, identifier: str, model: str) -> dict[str, object]:
        report = copy.deepcopy(self.reports[identifier])
        report["model"] = model
        warm_texts = self._warm_texts(identifier, model)
        if model == "base.en" and identifier == "human_short" and self.zero_baseline_warm:
            warm_totals = [0.0] * 5
        elif model == "base.en":
            warm_totals = [100.0, 101.0, 102.0, 103.0, 104.0]
        else:
            warm_totals = [80.0, 81.0, 82.0, 83.0, 84.0]
        report["runs"] = [_run("cold", 0, str(self._reference(identifier)), 120.0)] + [
            _run("warm", index, text, total)
            for index, (text, total) in enumerate(zip(warm_texts, warm_totals), 1)
        ]
        return report

    def _reference(self, identifier: str) -> str:
        return self.references[identifier].read_text(encoding="utf-8")

    def run_kwargs(self) -> dict[str, object]:
        return {
            "manifest": self.manifest,
            "plan": self.plan,
            "audio_map": self.audio_map,
            "reference_map": self.reference_map,
            "terms_map": self.terms_map,
            "binary_path": str(self.binary),
        }

    def run(self, output: Path, *, fail_pair: tuple[str, str] | None = None) -> dict[str, object]:
        def callback(argv: list[str], **_: object) -> subprocess.CompletedProcess[str]:
            audio_path = Path(argv[2])
            identifier = audio_path.stem
            model = argv[argv.index("--model") + 1]
            report_path = Path(argv[argv.index("--quality-output") + 1])
            report_path.write_text(
                json.dumps(self._report_for(identifier, model)), encoding="utf-8"
            )
            failed = fail_pair == (identifier, model)
            return subprocess.CompletedProcess(argv, 17 if failed else 0)

        with mock.patch("runner.subprocess.run", side_effect=callback):
            run_evaluation(**self.run_kwargs(), output_dir=str(output))
        return summarize_run(
            self.manifest,
            self.plan,
            self.reference_map,
            self.terms_map,
            str(output),
        )


class RunSummaryPairTests(unittest.TestCase):
    def _fixture(self, *, zero_baseline_warm: bool = False):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        fixture = PairedSummaryFixture(
            Path(temporary.name), zero_baseline_warm=zero_baseline_warm
        )
        return fixture, Path(temporary.name) / "output"

    def test_paired_configs_use_first_warm_and_join_unequal_human_rows(self):
        fixture, output = self._fixture()
        summary = fixture.run(output)

        self.assertEqual(summary["planned"], 8)
        self.assertEqual(summary["completed"], 8)
        self.assertEqual(summary["failed"], 0)
        baseline = next(
            row for row in summary["configurations"] if row["model"] == "base.en"
        )
        self.assertEqual(baseline["metrics"]["utterances"], 2)
        self.assertEqual(baseline["metrics"]["reference_words"], 6)
        self.assertEqual(baseline["metrics"]["errors"], 2)
        self.assertEqual(baseline["metrics"]["wer"], 2 / 6)
        self.assertEqual(baseline["metrics"]["warm"]["count"], 10)
        self.assertEqual(baseline["synthetic_utterances"], 1)
        self.assertEqual(baseline["controls"]["silence_utterances"], 1)
        self.assertEqual(baseline["controls"]["specialist_expected"], 2)
        self.assertEqual(baseline["controls"]["specialist_recalled"], 1)
        self.assertEqual(baseline["controls"]["first_warm_control_errors"], 0)

        candidate = next(
            row for row in summary["configurations"] if row["model"] == "tiny.en"
        )
        self.assertEqual(candidate["metrics"]["wer"], 0)
        self.assertEqual(len(summary["comparisons"]), 1)
        comparison = summary["comparisons"][0]
        self.assertEqual(comparison["accuracy"]["utterances"], 2)
        self.assertEqual(comparison["accuracy"]["baseline_wer"], 2 / 6)
        self.assertEqual(comparison["accuracy"]["candidate_wer"], 0)
        self.assertIsNotNone(comparison["warm_total_ms"])
        self.assertEqual(comparison["warm_total_ms"]["utterances"], 2)

        coverage = summary["coverage"]["languages"]["en"]
        self.assertEqual(coverage["unknown_environment_human"], 1)
        self.assertFalse(coverage["eligible"])

    def test_failed_pair_removes_whole_config_metrics_and_comparison(self):
        fixture, output = self._fixture()
        summary = fixture.run(output, fail_pair=("human_long", "tiny.en"))

        self.assertEqual(summary["planned"], 8)
        self.assertEqual(summary["completed"], 7)
        self.assertEqual(summary["failed"], 1)
        self.assertEqual(summary["statuses"], {"completed": 7, "cli_failed": 1})
        baseline = next(
            row for row in summary["configurations"] if row["model"] == "base.en"
        )
        candidate = next(
            row for row in summary["configurations"] if row["model"] == "tiny.en"
        )
        self.assertEqual(baseline["failed"], 0)
        self.assertIsNotNone(baseline["metrics"])
        self.assertEqual(candidate["completed"], 3)
        self.assertEqual(candidate["failed"], 1)
        self.assertIsNone(candidate["metrics"])
        self.assertIsNone(candidate["controls"])
        self.assertEqual(summary["comparisons"], [])

    def test_zero_baseline_warm_latency_is_inconclusive_not_an_error(self):
        fixture, output = self._fixture(zero_baseline_warm=True)
        summary = fixture.run(output)

        self.assertEqual(len(summary["comparisons"]), 1)
        self.assertIsNone(summary["comparisons"][0]["warm_total_ms"])
        self.assertEqual(summary["configurations"][0]["metrics"]["warm"]["count"], 10)


if __name__ == "__main__":
    unittest.main()
