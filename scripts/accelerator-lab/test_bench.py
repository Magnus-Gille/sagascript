"""Regression checks for the two WAV encodings present in the public corpus."""
from pathlib import Path
import struct
import sys
import tempfile
import unittest

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bench import pcm, wer


class AudioAndScoringTests(unittest.TestCase):
    def read_fixture(self, kind, bits, data):
        fmt = struct.pack('<HHIIHH', kind, 1, 16000, 16000 * bits // 8, bits // 8, bits)
        # Include an odd-length unknown chunk to exercise RIFF padding.
        body = b'WAVEJUNK' + struct.pack('<I', 1) + b'x\0'
        body += b'fmt ' + struct.pack('<I', len(fmt)) + fmt
        body += b'data' + struct.pack('<I', len(data)) + data
        with tempfile.TemporaryDirectory(prefix='sagascript-audio-test-') as folder:
            path = Path(folder) / 'fixture.wav'
            path.write_bytes(b'RIFF' + struct.pack('<I', len(body)) + body)
            return pcm(path)

    def test_pcm16_extremes_and_zero(self):
        actual = self.read_fixture(1, 16, struct.pack('<hhh', -32768, 0, 32767))
        np.testing.assert_array_equal(actual, [-1.0, 0.0, 32767 / 32768])

    def test_fleurs_float32_preserves_samples(self):
        actual = self.read_fixture(3, 32, struct.pack('<fff', -0.25, 0.0, 0.5))
        np.testing.assert_array_equal(actual, [-0.25, 0.0, 0.5])

    def test_wer_detects_lost_negation(self):
        self.assertEqual(wer('Vi kan inte göra det.', 'Vi kan göra det'),
                         {'errors': 1, 'words': 5, 'wer': 0.2})

    def test_wer_normalization_is_explicit(self):
        self.assertEqual(wer('Hej, VÄRLDEN!', 'hej världen')['errors'], 0)
        self.assertEqual(wer('allt mer', 'alltmer')['errors'], 2)


if __name__ == '__main__':
    unittest.main()
