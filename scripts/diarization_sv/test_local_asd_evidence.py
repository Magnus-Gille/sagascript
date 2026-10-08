#!/usr/bin/env python3

import argparse
import tempfile
import unittest
import wave
from pathlib import Path

import numpy as np

import sys

sys.path.insert(0, str(Path(__file__).parent))
import local_asd_evidence as asd


class LocalASDEvidenceTests(unittest.TestCase):
    def test_alignment_uses_25_hz_video_and_100_hz_mfcc_contract(self):
        self.assertEqual(asd.aligned_frame_count(101, 100 * 640), 100)
        self.assertEqual(asd.expected_mfcc_frames(1), 3)
        self.assertEqual(asd.expected_mfcc_frames(100), 399)

    def test_audio_shift_is_one_second_noncyclic_zero_padded(self):
        audio = np.arange(32_000, dtype=np.float32)
        shifted = asd.shift_audio_plus_one_second(audio)
        self.assertTrue(np.array_equal(shifted[:16_000], np.zeros(16_000, dtype=np.float32)))
        self.assertTrue(np.array_equal(shifted[16_000:], audio[:16_000]))

    def test_freeze_uses_first_valid_face_and_zeroes_actor_without_one(self):
        video = np.zeros((2, 4, 112, 112), dtype=np.uint8)
        video[0, 1].fill(17)
        video[0, 3].fill(29)
        valid = np.asarray([[False, True, False, True], [False, False, False, False]], dtype=bool)
        original = video.copy()
        frozen = asd.freeze_first_valid_face(video, valid)
        self.assertTrue(np.array_equal(frozen[0, 0], video[0, 1]))
        self.assertTrue(np.array_equal(frozen[0, 2], video[0, 1]))
        self.assertTrue(np.array_equal(frozen[0, 1], video[0, 1]))
        self.assertTrue(np.array_equal(frozen[0, 3], video[0, 1]))
        self.assertEqual(int(frozen[1].sum()), 0)
        self.assertTrue(np.array_equal(video, original), "freezing must not mutate caller input")

    def test_controls_apply_freeze_shift_and_silence(self):
        video = np.ones((1, 2, 112, 112), dtype=np.uint8)
        valid = np.asarray([[True, False]], dtype=bool)
        audio = np.ones(32_000, dtype=np.float32)
        controlled_video, controlled_audio = asd.apply_controls(
            video, valid, audio, ("audio_shift_1s", "freeze_first_valid_face", "silence_audio")
        )
        self.assertTrue(np.array_equal(controlled_video[:, 0], controlled_video[:, 1]))
        self.assertTrue(np.array_equal(controlled_audio, np.zeros_like(audio)))

    def test_faces_npz_shape_and_dtype_are_checked(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "faces.npz"
            video = np.zeros((2, 3, 112, 112), dtype=np.uint8)
            valid = np.asarray([[True, False, True], [False, False, True]], dtype=bool)
            np.savez(path, video=video, valid=valid)
            loaded_video, loaded_valid = asd.load_faces(path)
            self.assertEqual(loaded_video.shape, (2, 3, 112, 112))
            self.assertEqual(loaded_valid.dtype, np.bool_)

    def test_wav_reader_requires_16k_mono_and_returns_float(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "audio.wav"
            payload = (np.arange(160, dtype=np.int16) - 80).tobytes()
            with wave.open(str(path), "wb") as writer:
                writer.setnchannels(1)
                writer.setsampwidth(2)
                writer.setframerate(16_000)
                writer.writeframes(payload)
            audio = asd.load_audio(path)
            self.assertEqual(audio.shape, (160,))
            self.assertTrue(np.isfinite(audio).all())

    def test_mfcc_restores_pcm16_scale_used_by_official_talknet_demo(self):
        from python_speech_features import mfcc

        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "audio.wav"
            samples = (np.arange(32_000, dtype=np.int32) % 20_001 - 10_000).astype(np.int16)
            with wave.open(str(path), "wb") as writer:
                writer.setnchannels(1)
                writer.setsampwidth(2)
                writer.setframerate(16_000)
                writer.writeframes(samples.tobytes())
            normalized = asd.load_audio(path)
            expected = mfcc(
                samples,
                samplerate=asd.AUDIO_RATE_HZ,
                winlen=0.025,
                winstep=0.01,
                numcep=13,
            )
            actual = asd._mfcc(normalized)
            self.assertEqual(actual.shape, expected.shape)
            np.testing.assert_allclose(actual, expected, rtol=0.0, atol=1e-5)

    def test_controls_and_output_dir_guards(self):
        with self.assertRaises(asd.InputError):
            asd.parse_controls("silence_audio,not-a-control")
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "existing"
            output.mkdir()
            args = argparse.Namespace(
                faces=Path(temporary) / "missing.npz",
                audio16kmono=Path(temporary) / "missing.wav",
                code_dir=Path(temporary) / "missing-code",
                weights=Path(temporary) / "missing.model",
                output_dir=output,
                controls="",
                chunk_seconds=4.0,
                threads=2,
            )
            with self.assertRaisesRegex(asd.InputError, "refusing to overwrite"):
                asd.run(args)


if __name__ == "__main__":
    unittest.main()
