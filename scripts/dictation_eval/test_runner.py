"""Contract tests for the content-free local paired-evaluation runner."""

from __future__ import annotations

import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from paired_plan import build_plan
from runner import run_evaluation
from test_clip_score import case


SOURCE_REVISION = "a" * 40


def _file_sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _config() -> dict[str, object]:
    return {
        "id": "en_baseline",
        "language": "en",
        "model": "base.en",
        "beam_size": 0,
        "temperature_fallback": True,
        "role": "baseline",
    }


def _small_config() -> dict[str, object]:
    return {
        "id": "en_smaller",
        "language": "en",
        "model": "tiny.en",
        "beam_size": 0,
        "temperature_fallback": True,
        "role": "smaller",
    }


def _decoder_config() -> dict[str, object]:
    return {
        "id": "en_decoder",
        "language": "en",
        "model": "base.en",
        "beam_size": 2,
        "temperature_fallback": False,
        "role": "decoder",
    }


def _write_report(path: Path, report: dict[str, object]) -> None:
    path.write_text(json.dumps(report), encoding="utf-8")


class RunnerFixture:
    def __init__(self, root: Path, *, binary: Path | None = None):
        self.root = root
        self.audio = root / "audio fixture.wav"
        self.reference = root / "reference fixture.txt"
        self.audio.write_bytes(b"synthetic audio bytes for runner tests")
        self.reference.write_text("alpha", encoding="utf-8")

        manifest, report = case(reference="alpha", warm_texts=["alpha"] * 5)
        row = manifest["utterances"][0]
        assert isinstance(row, dict)
        row["audio_sha256"] = _file_sha256(self.audio)
        report["source_audio_sha256"] = row["audio_sha256"]
        self.manifest = manifest
        self.report = report
        self.binary = (binary or Path(sys.executable)).resolve()
        self.binary_sha256 = _file_sha256(self.binary)
        self.plan = build_plan(
            self.manifest,
            [_config()],
            split="heldout",
            seed=187,
            iterations=5,
            source_revision=SOURCE_REVISION,
            binary_sha256=self.binary_sha256,
        )
        self.audio_map = {"clip_1": str(self.audio.resolve())}
        self.reference_map = {"clip_1": str(self.reference.resolve())}
        self.terms_map = {
            "clip_1": {"specialist_terms": [], "control_terms": []}
        }

    def run_kwargs(self) -> dict[str, object]:
        return {
            "manifest": self.manifest,
            "plan": self.plan,
            "audio_map": self.audio_map,
            "reference_map": self.reference_map,
            "terms_map": self.terms_map,
            "binary_path": str(self.binary),
        }


class RunnerTests(unittest.TestCase):
    def _new_fixture(self) -> tuple[tempfile.TemporaryDirectory[str], RunnerFixture, Path]:
        temporary = tempfile.TemporaryDirectory()
        root = Path(temporary.name)
        return temporary, RunnerFixture(root), root / "new output"

    @staticmethod
    def _child_callback(fixture: RunnerFixture, calls: list[tuple[list[str], dict[str, object]]], *, returncode=0, mutate=None, invalid=False):
        def callback(argv, **kwargs):
            calls.append((list(argv), dict(kwargs)))
            report_path = Path(argv[argv.index("--quality-output") + 1])
            if invalid:
                report_path.write_text(
                    json.dumps({"PRIVATE_INVALID_REPORT_SENTINEL": True}),
                    encoding="utf-8",
                )
            else:
                _write_report(report_path, fixture.report)
            stderr = kwargs["stderr"]
            try:
                stderr.write("PRIVATE_CHILD_STDERR_SENTINEL\n")
            except TypeError:
                stderr.write(b"PRIVATE_CHILD_STDERR_SENTINEL\n")
            stderr.flush()
            if mutate is not None:
                mutate()
            return subprocess.CompletedProcess(argv, returncode)

        return callback

    def test_success_creates_private_ledger_and_content_free_summary(self):
        temporary, fixture, output = self._new_fixture()
        self.addCleanup(temporary.cleanup)
        calls: list[tuple[list[str], dict[str, object]]] = []
        callback = self._child_callback(fixture, calls)

        with mock.patch("runner.subprocess.run", side_effect=callback):
            summary = run_evaluation(**fixture.run_kwargs(), output_dir=str(output))

        self.assertEqual(summary["schema_version"], 1)
        self.assertEqual(summary["decision"], "inconclusive")
        self.assertEqual(summary["measurement_endpoint"], "live_inference_call_not_visible_text")
        self.assertEqual(summary["planned"], 1)
        self.assertEqual(summary["completed"], 1)
        self.assertEqual(summary["failed"], 0)
        self.assertEqual(len(calls), 1)
        self.assertTrue(output.is_dir())
        result_path = output / "00000-result.json"
        self.assertTrue(result_path.is_file())
        record = json.loads(result_path.read_text(encoding="utf-8"))
        self.assertEqual(record["status"], "completed")
        self.assertIn("score", record)
        stderr_paths = [path for path in output.iterdir() if "stderr" in path.name]
        self.assertEqual(len(stderr_paths), 1)
        self.assertIn("PRIVATE_CHILD_STDERR_SENTINEL", stderr_paths[0].read_text())

        if os.name == "posix":
            self.assertEqual(output.stat().st_mode & 0o777, 0o700)
            self.assertEqual(result_path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(stderr_paths[0].stat().st_mode & 0o777, 0o600)

        public = json.dumps(summary, sort_keys=True)
        for private_value in (
            "clip_1",
            "alpha",
            str(fixture.audio),
            str(fixture.reference),
            fixture.binary_sha256,
            SOURCE_REVISION,
            "PRIVATE_CHILD_STDERR_SENTINEL",
        ):
            self.assertNotIn(private_value, public)

    def test_frozen_argv_and_subprocess_privacy_contract(self):
        temporary, fixture, output = self._new_fixture()
        self.addCleanup(temporary.cleanup)
        calls: list[tuple[list[str], dict[str, object]]] = []
        with mock.patch(
            "runner.subprocess.run",
            side_effect=self._child_callback(fixture, calls),
        ):
            run_evaluation(**fixture.run_kwargs(), output_dir=str(output), timeout_seconds=901)

        argv, kwargs = calls[0]
        self.assertEqual(
            argv,
            [
                str(fixture.binary),
                "benchmark-dictation",
                str(fixture.audio.resolve()),
                "--language",
                "en",
                "--model",
                "base.en",
                "--beam-size",
                "0",
                "--iterations",
                "5",
                "--quality-output",
                str(output / "00000-report.json"),
            ],
        )
        self.assertIs(kwargs["stdin"], subprocess.DEVNULL)
        self.assertIs(kwargs["stdout"], subprocess.DEVNULL)
        self.assertIs(kwargs["check"], False)
        self.assertEqual(kwargs["timeout"], 901)
        stderr = kwargs["stderr"]
        self.assertTrue(hasattr(stderr, "fileno"))
        if os.name == "posix":
            stderr_paths = [path for path in output.iterdir() if "stderr" in path.name]
            self.assertEqual(len(stderr_paths), 1)
            self.assertEqual(stderr_paths[0].stat().st_mode & 0o777, 0o600)

    def test_multiconfig_argv_flags_and_all_planned_results(self):
        temporary, fixture, output = self._new_fixture()
        self.addCleanup(temporary.cleanup)
        configurations = [_config(), _small_config(), _decoder_config()]
        plan = build_plan(
            fixture.manifest,
            configurations,
            split="heldout",
            seed=187,
            iterations=5,
            source_revision=SOURCE_REVISION,
            binary_sha256=fixture.binary_sha256,
        )
        kwargs = {**fixture.run_kwargs(), "plan": plan}
        calls: list[tuple[list[str], dict[str, object]]] = []
        by_id = {config["id"]: config for config in configurations}

        def config_for_argv(argv):
            model = argv[argv.index("--model") + 1]
            beam_size = int(argv[argv.index("--beam-size") + 1])
            temperature_fallback = "--disable-temperature-fallback" not in argv
            return next(
                config
                for config in configurations
                if config["model"] == model
                and config["beam_size"] == beam_size
                and config["temperature_fallback"] == temperature_fallback
            )

        def callback(argv, **call_kwargs):
            calls.append((list(argv), dict(call_kwargs)))
            config = config_for_argv(argv)
            report = copy.deepcopy(fixture.report)
            report["model"] = config["model"]
            report["beam_size"] = config["beam_size"]
            report["temperature_fallback"] = config["temperature_fallback"]
            _write_report(Path(argv[argv.index("--quality-output") + 1]), report)
            return subprocess.CompletedProcess(argv, 0)

        with mock.patch("runner.subprocess.run", side_effect=callback):
            summary = run_evaluation(**kwargs, output_dir=str(output))

        self.assertEqual(len(calls), len(plan["order"]))
        for index, (argv, call_kwargs) in enumerate(calls):
            config = by_id[plan["order"][index]["configuration_id"]]
            expected = [
                str(fixture.binary),
                "benchmark-dictation",
                str(fixture.audio.resolve()),
                "--language",
                config["language"],
                "--model",
                config["model"],
                "--beam-size",
                str(config["beam_size"]),
                "--iterations",
                "5",
                "--quality-output",
                str(output / f"{index:05d}-report.json"),
            ]
            if not config["temperature_fallback"]:
                expected.append("--disable-temperature-fallback")
            self.assertEqual(argv, expected)
            self.assertIs(call_kwargs["stdin"], subprocess.DEVNULL)
            self.assertIs(call_kwargs["stdout"], subprocess.DEVNULL)

        self.assertEqual(summary["planned"], 3)
        self.assertEqual(summary["completed"], 3)
        self.assertEqual(summary["failed"], 0)
        for index in range(3):
            result = output / f"{index:05d}-result.json"
            self.assertEqual(json.loads(result.read_text())["status"], "completed")

    def test_every_planned_entry_is_retained_after_identity_failure(self):
        temporary, fixture, output = self._new_fixture()
        self.addCleanup(temporary.cleanup)
        configurations = [_config(), _small_config()]
        plan = build_plan(
            fixture.manifest,
            configurations,
            split="heldout",
            seed=187,
            iterations=5,
            source_revision=SOURCE_REVISION,
            binary_sha256=fixture.binary_sha256,
        )
        kwargs = {**fixture.run_kwargs(), "plan": plan}
        calls: list[tuple[list[str], dict[str, object]]] = []

        def callback(argv, **call_kwargs):
            calls.append((list(argv), dict(call_kwargs)))
            config = next(
                config
                for config in configurations
                if config["model"] == argv[argv.index("--model") + 1]
            )
            report = copy.deepcopy(fixture.report)
            report["model"] = config["model"]
            _write_report(Path(argv[argv.index("--quality-output") + 1]), report)
            fixture.audio.write_bytes(fixture.audio.read_bytes() + b" mutated")
            return subprocess.CompletedProcess(argv, 0)

        with mock.patch("runner.subprocess.run", side_effect=callback):
            summary = run_evaluation(**kwargs, output_dir=str(output))

        self.assertEqual(len(calls), 1)
        self.assertEqual(summary["planned"], 2)
        self.assertEqual(summary["completed"], 0)
        self.assertEqual(summary["failed"], 2)
        self.assertEqual(
            json.loads((output / "00000-result.json").read_text())["status"],
            "identity_changed",
        )
        self.assertEqual(
            json.loads((output / "00001-result.json").read_text())["status"],
            "not_attempted",
        )

    def test_existing_output_is_rejected_before_child_and_not_overwritten(self):
        temporary, fixture, output = self._new_fixture()
        self.addCleanup(temporary.cleanup)
        output.mkdir(mode=0o700)
        sentinel = output / "existing.txt"
        sentinel.write_text("DO NOT OVERWRITE", encoding="utf-8")
        with mock.patch("runner.subprocess.run") as child:
            with self.assertRaises(ValueError):
                run_evaluation(**fixture.run_kwargs(), output_dir=str(output))
        child.assert_not_called()
        self.assertEqual(sentinel.read_text(encoding="utf-8"), "DO NOT OVERWRITE")

    def test_missing_output_parent_is_rejected_before_child(self):
        temporary, fixture, output = self._new_fixture()
        self.addCleanup(temporary.cleanup)
        missing_parent = output / "missing parent"
        candidate = missing_parent / "output"
        with mock.patch("runner.subprocess.run") as child:
            with self.assertRaises(ValueError):
                run_evaluation(**fixture.run_kwargs(), output_dir=str(candidate))
        child.assert_not_called()
        self.assertFalse(missing_parent.exists())

    def test_dangling_or_existing_output_symlink_is_not_followed(self):
        if os.name != "posix":
            self.skipTest("POSIX symlink mode is required for this safety check")
        temporary, fixture, output = self._new_fixture()
        self.addCleanup(temporary.cleanup)
        target = output.parent / "target"
        target.mkdir(mode=0o700)
        sentinel = target / "existing.txt"
        sentinel.write_text("DO NOT OVERWRITE", encoding="utf-8")
        os.symlink(target, output)
        with mock.patch("runner.subprocess.run") as child:
            with self.assertRaises(ValueError):
                run_evaluation(**fixture.run_kwargs(), output_dir=str(output))
        child.assert_not_called()
        self.assertTrue(output.is_symlink())
        self.assertEqual(sentinel.read_text(encoding="utf-8"), "DO NOT OVERWRITE")

    def test_binary_hash_and_all_maps_are_preflight_bound(self):
        def wrong_plan(fixture):
            candidate = copy.deepcopy(fixture.plan)
            candidate["binary_sha256"] = "0" * 64
            return {**fixture.run_kwargs(), "plan": candidate}

        def missing_audio(fixture):
            candidate = dict(fixture.audio_map)
            del candidate["clip_1"]
            return {**fixture.run_kwargs(), "audio_map": candidate}

        def extra_audio(fixture):
            candidate = {**fixture.audio_map, "extra": fixture.audio_map["clip_1"]}
            return {**fixture.run_kwargs(), "audio_map": candidate}

        def missing_reference(fixture):
            candidate = dict(fixture.reference_map)
            del candidate["clip_1"]
            return {**fixture.run_kwargs(), "reference_map": candidate}

        def extra_terms(fixture):
            candidate = {**fixture.terms_map, "extra": fixture.terms_map["clip_1"]}
            return {**fixture.run_kwargs(), "terms_map": candidate}

        def bad_reference_hash(fixture):
            fixture.reference.write_text("changed reference", encoding="utf-8")
            return fixture.run_kwargs()

        cases = [
            wrong_plan,
            missing_audio,
            extra_audio,
            missing_reference,
            extra_terms,
            bad_reference_hash,
        ]
        for index, make_kwargs in enumerate(cases):
            with tempfile.TemporaryDirectory() as directory:
                fixture = RunnerFixture(Path(directory))
                kwargs = make_kwargs(fixture)
                candidate_output = Path(directory) / "rejected"
                with self.subTest(index=index), mock.patch("runner.subprocess.run") as child:
                    with self.assertRaises(ValueError):
                        run_evaluation(**kwargs, output_dir=str(candidate_output))
                    child.assert_not_called()
                self.assertFalse(candidate_output.exists())

    def test_nonzero_cli_keeps_valid_score_with_cli_failed_status(self):
        temporary, fixture, output = self._new_fixture()
        self.addCleanup(temporary.cleanup)
        calls: list[tuple[list[str], dict[str, object]]] = []
        with mock.patch(
            "runner.subprocess.run",
            side_effect=self._child_callback(fixture, calls, returncode=17),
        ):
            summary = run_evaluation(**fixture.run_kwargs(), output_dir=str(output))
        record = json.loads((output / "00000-result.json").read_text())
        self.assertEqual(record["status"], "cli_failed")
        self.assertIn("score", record)
        self.assertEqual(record["score"]["decision"], "inconclusive")
        self.assertEqual(summary["failed"], 1)

    def test_mismatched_report_metadata_is_invalid_report(self):
        mutations = {
            "model": lambda report: report.__setitem__("model", "tiny.en"),
            "beam_size": lambda report: report.__setitem__("beam_size", 2),
            "iterations": lambda report: report.__setitem__("runs", report["runs"][:-1]),
            "allow_empty": lambda report: report.__setitem__("allow_empty", True),
        }
        for field, mutate in mutations.items():
            with self.subTest(field=field), tempfile.TemporaryDirectory() as directory:
                fixture = RunnerFixture(Path(directory))
                output = Path(directory) / "output"
                calls: list[tuple[list[str], dict[str, object]]] = []

                def callback(argv, **kwargs):
                    calls.append((list(argv), dict(kwargs)))
                    report = copy.deepcopy(fixture.report)
                    mutate(report)
                    _write_report(Path(argv[argv.index("--quality-output") + 1]), report)
                    return subprocess.CompletedProcess(argv, 0)

                with mock.patch("runner.subprocess.run", side_effect=callback):
                    summary = run_evaluation(
                        **fixture.run_kwargs(), output_dir=str(output)
                    )
                record = json.loads((output / "00000-result.json").read_text())
                self.assertEqual(record["status"], "invalid_report")
                self.assertIsNone(record["score"])
                self.assertEqual(summary["failed"], 1)

    def test_valid_report_with_failed_cli_gate_keeps_score_as_cli_failed(self):
        temporary, fixture, output = self._new_fixture()
        self.addCleanup(temporary.cleanup)
        calls: list[tuple[list[str], dict[str, object]]] = []

        def callback(argv, **kwargs):
            calls.append((list(argv), dict(kwargs)))
            report = copy.deepcopy(fixture.report)
            report["cli_checks_passed"] = False
            _write_report(Path(argv[argv.index("--quality-output") + 1]), report)
            return subprocess.CompletedProcess(argv, 0)

        with mock.patch("runner.subprocess.run", side_effect=callback):
            summary = run_evaluation(**fixture.run_kwargs(), output_dir=str(output))
        record = json.loads((output / "00000-result.json").read_text())
        self.assertEqual(record["status"], "cli_failed")
        self.assertIsNotNone(record["score"])
        self.assertEqual(summary["failed"], 1)

    def test_invalid_report_is_retained_as_failed_ledger_entry(self):
        temporary, fixture, output = self._new_fixture()
        self.addCleanup(temporary.cleanup)
        calls: list[tuple[list[str], dict[str, object]]] = []
        with mock.patch(
            "runner.subprocess.run",
            side_effect=self._child_callback(fixture, calls, invalid=True),
        ):
            summary = run_evaluation(**fixture.run_kwargs(), output_dir=str(output))
        record = json.loads((output / "00000-result.json").read_text())
        self.assertEqual(record["status"], "invalid_report")
        self.assertNotIn("PRIVATE_INVALID_REPORT_SENTINEL", json.dumps(summary))
        self.assertEqual(summary["failed"], 1)

    def test_timeout_is_retained_as_failed_ledger_entry(self):
        temporary, fixture, output = self._new_fixture()
        self.addCleanup(temporary.cleanup)

        def timeout(argv, **kwargs):
            raise subprocess.TimeoutExpired(argv, kwargs["timeout"])

        with mock.patch("runner.subprocess.run", side_effect=timeout):
            summary = run_evaluation(**fixture.run_kwargs(), output_dir=str(output))
        record = json.loads((output / "00000-result.json").read_text())
        self.assertEqual(record["status"], "timeout")
        self.assertEqual(summary["failed"], 1)

    def test_audio_mutation_after_child_is_identity_changed(self):
        temporary, fixture, output = self._new_fixture()
        self.addCleanup(temporary.cleanup)
        calls: list[tuple[list[str], dict[str, object]]] = []

        def mutate_audio():
            fixture.audio.write_bytes(fixture.audio.read_bytes() + b" changed")

        with mock.patch(
            "runner.subprocess.run",
            side_effect=self._child_callback(fixture, calls, mutate=mutate_audio),
        ):
            summary = run_evaluation(**fixture.run_kwargs(), output_dir=str(output))
        record = json.loads((output / "00000-result.json").read_text())
        self.assertEqual(record["status"], "identity_changed")
        self.assertEqual(summary["failed"], 1)

    def test_binary_mutation_after_child_is_identity_changed(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        binary = root / "sagascript-test-binary"
        shutil.copyfile(sys.executable, binary)
        binary.chmod(0o700)
        fixture = RunnerFixture(root, binary=binary)
        output = root / "output"
        calls: list[tuple[list[str], dict[str, object]]] = []

        def mutate_binary():
            binary.write_bytes(binary.read_bytes() + b" changed")

        with mock.patch(
            "runner.subprocess.run",
            side_effect=self._child_callback(fixture, calls, mutate=mutate_binary),
        ):
            summary = run_evaluation(**fixture.run_kwargs(), output_dir=str(output))
        record = json.loads((output / "00000-result.json").read_text())
        self.assertEqual(record["status"], "identity_changed")
        self.assertEqual(summary["failed"], 1)


if __name__ == "__main__":
    unittest.main()
