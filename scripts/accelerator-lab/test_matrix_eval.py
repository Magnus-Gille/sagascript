"""Focused local tests for scripts/accelerator-lab/matrix_eval.py."""
import io
import json
import struct
import sys
import tarfile
import tempfile
import unittest
import wave
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from matrix_eval import (
    prepare_fixtures,
    run_matrix,
    sha256_bytes,
    wav_to_pcm16,
    wer,
)


def pcm16_fixture(frames: int) -> bytes:
    raw = b"\x01\x00" * frames
    output = io.BytesIO()
    with wave.open(output, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(16_000)
        wav.writeframes(raw)
    return output.getvalue()


class MatrixEvalTests(unittest.TestCase):
    def test_wer_uses_casefolded_word_tokens_and_edit_distance(self):
        self.assertEqual(wer("A, b!", "a c") , {"errors": 1, "words": 2, "wer": 0.5})

    def test_float32_wav_is_accepted_and_converted_to_pcm16(self):
        samples = struct.pack("<4f", -1.0, -0.5, 0.5, 1.0)
        fmt_chunk = struct.pack("<HHIIHH", 3, 1, 16_000, 64_000, 4, 32)
        data = b"RIFF" + struct.pack("<I", 36 + len(samples)) + b"WAVE"
        data += b"fmt " + struct.pack("<I", len(fmt_chunk)) + fmt_chunk
        data += b"data" + struct.pack("<I", len(samples)) + samples
        frames, pcm, fmt = wav_to_pcm16(data)
        self.assertEqual(frames, 4)
        self.assertEqual(fmt["format"], 3)
        self.assertEqual(struct.unpack("<4h", pcm), (-32768, -16384, 16384, 32767))

    def test_prepare_writes_distinct_identical_inputs_and_exact_long_wave(self):
        with tempfile.TemporaryDirectory() as temp:
            lab = Path(temp) / "lab"
            audio_dir = lab / "audio"
            audio_dir.mkdir(parents=True)
            short_name = "short-public.wav"
            short_bytes = pcm16_fixture(3 * 16_000)
            long_a = pcm16_fixture(2 * 16_000)
            long_b = pcm16_fixture(3 * 16_000)
            (audio_dir / "long-a.wav").write_bytes(long_a)
            (audio_dir / "long-b.wav").write_bytes(long_b)
            (lab / "corpus.json").write_text(json.dumps([
                {"id": "a", "file": str(audio_dir / "long-a.wav"), "reference": "one", "gender": "MALE"},
                {"id": "b", "file": str(audio_dir / "long-b.wav"), "reference": "two", "gender": "FEMALE"},
                {"id": "fixture", "file": str(audio_dir / "fixture.wav"), "reference": "skip", "id2": "x"},
            ]), encoding="utf-8")
            (lab / "fleurs-sv-dev.tsv").write_text(
                "speaker\tfilename\treference\tx\ty\tsamples\tgender\n"
                f"s1\t{short_name}\tshort words\t\t\t{3 * 16000}\tFEMALE\n",
                encoding="utf-8",
            )
            archive = lab / "fleurs-sv-dev.tar.gz"
            with tarfile.open(archive, "w:gz") as tar:
                info = tarfile.TarInfo(f"audio/{short_name}")
                info.size = len(short_bytes)
                tar.addfile(info, io.BytesIO(short_bytes))
            manifest_path = prepare_fixtures(lab, lab / "fixtures")
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            short = manifest["fixtures"]["short"]
            long = manifest["fixtures"]["long"]
            self.assertEqual(short["duration_seconds"], 3)
            self.assertEqual(short["sha256"], sha256_bytes(short_bytes))
            self.assertNotEqual(short["cold_path"], short["warm_path"])
            self.assertEqual(short["cold_sha256"], short["warm_sha256"])
            with wave.open(long["cold_path"], "rb") as wav:
                self.assertEqual(wav.getnframes(), 180 * 16_000)
                self.assertEqual(wav.getframerate(), 16_000)
            self.assertTrue(long["synthetic"])
            references = [part["reference"] for part in long["composition"]]
            self.assertEqual(references[:2], ["one", "two"])
            self.assertGreaterEqual(manifest["long_public_speech_seconds"], 175)
            self.assertLessEqual(manifest["long_trailing_silence_seconds"], 5)
            self.assertTrue(any("repeat_index" in part for part in long["composition"]))
            self.assertEqual(long["cold_sha256"], long["warm_sha256"])

    def test_runner_captures_jsonl_progress_and_performance_without_inference(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            inputs = {}
            for case in ("short", "long"):
                payload = pcm16_fixture(48_000 if case == "short" else 180 * 16_000)
                cold = root / f"{case}-cold.wav"
                warm = root / f"{case}-warm.wav"
                cold.write_bytes(payload)
                warm.write_bytes(payload)
                inputs[case] = {
                    "cold_path": str(cold),
                    "warm_path": str(warm),
                    "sha256": sha256_bytes(payload),
                    "reference": "hello world",
                    "duration_seconds": 3.0 if case == "short" else 180.0,
                }
            manifest = root / "fixtures.json"
            manifest.write_text(json.dumps({"fixtures": inputs}), encoding="utf-8")
            fake_cli = root / "fake_cli.py"
            fake_cli.write_text(
                """import json, pathlib, sys, time
paths = [p for p in sys.argv[1:] if p.endswith('.wav')]
for i, path in enumerate(paths):
 print(json.dumps({'event':'transcription_progress','phase':'decoding','percent':0,'elapsed_ms':0}), file=sys.stderr, flush=True)
 time.sleep(0.02)
 item = {'status':'ok','source':str(pathlib.Path(path).resolve()),'result':{'text':'hello world','duration_seconds':180 if 'long' in path else 3,'performance':{'total_seconds':0.02,'model_load_seconds':0.1 if i == 0 else 0,'model_verification_seconds':0.01 if i == 0 else 0,'engine_warm':i > 0}}}
 print(json.dumps(item), flush=True)
""",
                encoding="utf-8",
            )
            config = root / "backends.json"
            config.write_text(json.dumps({
                "timeout_seconds": 10,
                "backends": {name: {"cli": [sys.executable, str(fake_cli)], "env": {}}
                             for name in ("cpu", "gpu", "npu")},
            }), encoding="utf-8")
            out = root / "results"
            summary_path = run_matrix(config, manifest, out)
            summary = json.loads(summary_path.read_text(encoding="utf-8"))
            self.assertTrue(summary["success"])
            self.assertEqual(len(summary["runs"]), 6)
            for cell in summary["runs"]:
                self.assertEqual(cell["exit_code"], 0)
                self.assertEqual(cell["result_item_count"], 2)
                self.assertEqual(cell["progress_event_count"], 2)
                self.assertNotIn("--beam", cell["argv"])
                self.assertEqual([item["slot"] for item in cell["items"]], ["cold", "warm"])
                self.assertEqual(cell["items"][0]["quality"]["wer"], 0)
                self.assertEqual(cell["items"][0]["model_load_seconds"], 0.1)
                self.assertEqual(cell["items"][1]["model_load_seconds"], 0)
                self.assertGreaterEqual(cell["items"][1]["previous_result_to_result_ms"], 0)
                self.assertTrue(Path(cell["stdout_jsonl"]).is_file())
                self.assertTrue(Path(cell["stderr_log"]).is_file())


if __name__ == "__main__":
    unittest.main()
