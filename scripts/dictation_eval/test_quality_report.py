import copy
import json
import re
import tempfile
import unittest
from pathlib import Path

from quality_report import (
    MAX_REPORT_BYTES,
    MAX_TEXT_CHARS,
    parse_quality_report,
    read_quality_report,
    validate_quality_report,
)


def valid_report():
    return {
        "schema_version": 1,
        "build_version": "1.2.3",
        "language": "sv",
        "model": "kb-whisper-base",
        "model_expected_sha256": "a" * 64,
        "model_expected_bytes": 123,
        "source_audio_sha256": "b" * 64,
        "decoded_audio_sha256": "c" * 64,
        "duration_seconds": 2.5,
        "decode_duration_ms": 4.0,
        "beam_size": 2,
        "temperature_fallback": False,
        "allow_empty": True,
        "measurement_endpoint": "live_inference_call_not_visible_text",
        "cold_definition": "first_call_in_new_backend_not_system_cold",
        "cli_checks_passed": False,
        "runs": [
            {
                "kind": "cold",
                "iteration": 0,
                "text": "kall text",
                "model_ms": 10.0,
                "inference_ms": 20.0,
                "total_ms": 30.0,
                "model_cached": False,
            },
            {
                "kind": "warm",
                "iteration": 1,
                "text": "varm ett",
                "model_ms": 0.0,
                "inference_ms": 15.0,
                "total_ms": 15.0,
                "model_cached": True,
            },
            {
                "kind": "warm",
                "iteration": 2,
                "text": "varm två",
                "model_ms": 0.0,
                "inference_ms": 16.0,
                "total_ms": 16.0,
                "model_cached": True,
            },
        ],
    }


class QualityReportTests(unittest.TestCase):
    def test_valid_report_is_copied_and_preserves_text_and_cli_gate(self):
        source = valid_report()
        result = validate_quality_report(source)

        self.assertEqual(result["runs"][0]["text"], "kall text")
        self.assertFalse(result["cli_checks_passed"])
        self.assertEqual(result["model_expected_sha256"], "a" * 64)
        self.assertIsNot(result, source)
        self.assertIsNot(result["runs"], source["runs"])
        source["runs"].append(copy.deepcopy(source["runs"][-1]))
        self.assertEqual(len(result["runs"]), 3)

    def test_reader_accepts_utf8_file_and_rejects_oversized_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "quality.json"
            path.write_text(json.dumps(valid_report()), encoding="utf-8")
            self.assertEqual(read_quality_report(path)["schema_version"], 1)

            path.write_bytes(b"x" * (MAX_REPORT_BYTES + 1))
            with self.assertRaises(ValueError):
                read_quality_report(path)

    def test_duplicate_keys_and_nonfinite_numbers_fail_closed(self):
        duplicate = json.dumps(valid_report())[:-1] + ', "runs": []}'
        with self.assertRaises(ValueError):
            parse_quality_report(duplicate)
        with self.assertRaises(ValueError):
            parse_quality_report('{"schema_version": NaN}')
        with self.assertRaises(ValueError):
            parse_quality_report(json.dumps({**valid_report(), "duration_seconds": float("inf")}))

    def test_exact_top_level_schema_and_scalar_types_are_required(self):
        for key in ["model", "runs"]:
            missing = valid_report()
            del missing[key]
            with self.assertRaises(ValueError):
                validate_quality_report(missing)

        extra = valid_report()
        extra["unexpected"] = 1
        with self.assertRaises(ValueError):
            validate_quality_report(extra)

        for key in ["schema_version", "model_expected_bytes", "beam_size"]:
            wrong = valid_report()
            wrong[key] = True
            with self.assertRaises(ValueError):
                validate_quality_report(wrong)
        wrong = valid_report()
        wrong["allow_empty"] = 1
        with self.assertRaises(ValueError):
            validate_quality_report(wrong)

    def test_bounded_duration_timing_text_and_hash_fields(self):
        cases = []
        report = valid_report()
        report["duration_seconds"] = 0
        cases.append(report)
        report = valid_report()
        report["duration_seconds"] = 120.0001
        cases.append(report)
        report = valid_report()
        report["decode_duration_ms"] = -0.1
        cases.append(report)
        report = valid_report()
        report["runs"][1]["inference_ms"] = float("nan")
        cases.append(report)
        report = valid_report()
        report["runs"][1]["inference_ms"] = 10**400
        cases.append(report)
        report = valid_report()
        report["runs"][1]["text"] = "x" * (MAX_TEXT_CHARS + 1)
        cases.append(report)
        report = valid_report()
        report["runs"][1]["text"] = "lone surrogate: \ud800"
        cases.append(report)
        for field in ["source_audio_sha256", "decoded_audio_sha256", "model_expected_sha256"]:
            report = valid_report()
            report[field] = "A" * 64
            cases.append(report)
        for invalid in cases:
            with self.assertRaises(ValueError):
                validate_quality_report(invalid)

        report = valid_report()
        report["model_expected_bytes"] = 0
        with self.assertRaises(ValueError):
            validate_quality_report(report)

    def test_known_models_beam_bounds_and_fixed_constants_are_strict(self):
        for field, value in [
            ("model", "not-a-model"),
            ("language", "auto"),
            ("measurement_endpoint", "other"),
            ("cold_definition", "system-cold"),
        ]:
            report = valid_report()
            report[field] = value
            with self.assertRaises(ValueError):
                validate_quality_report(report)
        for beam_size in [1, 17, -1]:
            report = valid_report()
            report["beam_size"] = beam_size
            with self.assertRaises(ValueError):
                validate_quality_report(report)
        for beam_size in [0, 2, 16]:
            report = valid_report()
            report["beam_size"] = beam_size
            self.assertEqual(validate_quality_report(report)["beam_size"], beam_size)

    def test_model_inventory_matches_rust_model_id_string_source(self):
        source_path = (
            Path(__file__).resolve().parents[2]
            / "src-tauri"
            / "crates"
            / "sagascript-cli"
            / "src"
            / "transcribe.rs"
        )
        source = source_path.read_text(encoding="utf-8")
        start = source.index("pub fn model_id_string")
        body = source[start : source.index("\n}", start)]
        rust_models = set(
            re.findall(r"WhisperModel::[A-Za-z0-9_]+\s*=>\s*\"([^\"]+)\"", body)
        )
        self.assertTrue(rust_models)
        from quality_report import _MODELS

        self.assertEqual(_MODELS, rust_models)

    def test_run_order_count_and_shape_are_strict(self):
        for runs in [valid_report()["runs"][:2], valid_report()["runs"] + [copy.deepcopy(valid_report()["runs"][-1])] * 29]:
            report = valid_report()
            report["runs"] = runs
            with self.assertRaises(ValueError):
                validate_quality_report(report)

        for index, field, value in [
            (0, "kind", "warm"),
            (0, "iteration", 1),
            (1, "kind", "cold"),
            (1, "iteration", 2),
        ]:
            report = valid_report()
            report["runs"][index][field] = value
            with self.assertRaises(ValueError):
                validate_quality_report(report)

        report = valid_report()
        del report["runs"][1]["text"]
        with self.assertRaises(ValueError):
            validate_quality_report(report)

    def test_invalid_json_and_errors_do_not_echo_private_content(self):
        sentinel = "PRIVATE_TRANSCRIPT_SENTINEL_187"
        report = valid_report()
        report["model"] = sentinel
        with self.assertRaises(ValueError) as raised:
            validate_quality_report(report)
        self.assertNotIn(sentinel, str(raised.exception))

        with self.assertRaises(ValueError) as raised:
            parse_quality_report(b"not json")
        self.assertNotIn("not json", str(raised.exception))

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "private-sentinel-report.json"
            with self.assertRaises(ValueError) as raised:
                read_quality_report(path)
            self.assertNotIn(str(path), str(raised.exception))


if __name__ == "__main__":
    unittest.main()
