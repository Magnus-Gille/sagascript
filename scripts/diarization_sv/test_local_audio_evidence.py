from __future__ import annotations

import json
import os
import stat
import tempfile
import unittest
from pathlib import Path

import numpy as np
import soundfile as sf

import local_audio_evidence as evidence


class LocalAudioEvidenceTests(unittest.TestCase):
    def test_window_rounding_and_timestamps_do_not_drift(self) -> None:
        window = evidence._aligned_window_samples(1.03)
        self.assertEqual(window, 32 * evidence.SILERO_HOP)
        frame_count = evidence._silero_frame_count(32_000, window)
        timestamps = evidence._frame_timestamps(frame_count, evidence.SILERO_HOP / 16_000, 2.0)
        self.assertEqual(frame_count, 63)
        self.assertAlmostEqual(timestamps[-1], 1.984)
        with self.assertRaises(ValueError):
            evidence._silero_frame_count(32_000, window + 1)

    def test_probability_runs_hysteresis_and_clips(self) -> None:
        values = [0.1, 0.8, 0.7, 0.3, 0.1, 0.9]
        intervals = evidence.probabilities_to_intervals(values, 1.0, 5.5, 0.5)
        self.assertEqual(intervals, [(1.0, 3.0), (5.0, 5.5)])

    def test_probability_run_does_not_emit_inverted_tail(self) -> None:
        values = [0.8, 0.8, 0.1]
        self.assertEqual(evidence.probabilities_to_intervals(values, 1.0, 2.5, 0.5), [(0.0, 2.0)])

    def test_overlap_merge_and_duration_bounds(self) -> None:
        self.assertEqual(
            evidence.merge_intervals([(-1, 1), (0.5, 2), (2, 3), (4, 9)], 5),
            [(0.0, 3.0), (4.0, 5.0)],
        )

    def test_wav_bounds(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            good = root / "good.wav"
            sf.write(good, np.zeros(1600, dtype=np.float32), 16_000, subtype="PCM_16")
            samples, duration = evidence.validate_wav(good)
            self.assertEqual(samples.shape, (1600,))
            self.assertAlmostEqual(duration, 0.1)
            bad = root / "bad.wav"
            sf.write(bad, np.zeros((1600, 2), dtype=np.float32), 8_000, subtype="PCM_16")
            with self.assertRaises(ValueError):
                evidence.validate_wav(bad)

    def test_output_refuses_overwrite_and_uses_private_mode(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / "out"
            evidence._create_output_dir(output)
            self.assertEqual(stat.S_IMODE(output.stat().st_mode), 0o700)
            with self.assertRaises(FileExistsError):
                evidence._create_output_dir(output)

    def test_scores_are_numeric_npz_without_pickle(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "scores.npz"
            digest = evidence._write_scores(path, np.array([[0.1, 0.9], [0.2, 0.8]], dtype=np.float32), 10.0, 0.2)
            with np.load(path, allow_pickle=False) as scores:
                np.testing.assert_allclose(scores["probabilities"], [[0.1, 0.9], [0.2, 0.8]])
                self.assertEqual(float(scores["frame_hz"]), 10.0)
            self.assertEqual(len(digest), 64)

    def test_hash_is_stable_and_metadata_is_numeric(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "model.bin"
            path.write_bytes(b"synthetic")
            first = evidence.sha256_file(path)
            second = evidence.sha256_file(path)
            self.assertEqual(first, second)
            metadata = {"hashes": {"model": first}, "elapsed_seconds": 0.1}
            parsed = json.loads(json.dumps(metadata))
            self.assertEqual(parsed["hashes"]["model"], first)
            self.assertTrue(isinstance(parsed["elapsed_seconds"], float))


if __name__ == "__main__":
    unittest.main()
