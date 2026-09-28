"""The TEST-only public audio fixture must exercise, never overwrite, the tiled path."""

import importlib.util
import sys
import tempfile
import unittest
import wave
from pathlib import Path

script = Path(__file__).with_name("make-pianissimo-long-smoke.py")
spec = importlib.util.spec_from_file_location("pianissimo_long_smoke", script)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)


class PublicLongSmokeTests(unittest.TestCase):
    def test_public_fixture_crosses_tiling_gate_and_never_overwrites(self):
        source = Path(__file__).resolve().parent.parent / "test-audio/english-jfk.wav"
        with tempfile.TemporaryDirectory() as root:
            destination = Path(root) / "long.wav"
            self.assertEqual(module.make_smoke_audio(source, destination), 231)
            with wave.open(str(destination), "rb") as audio:
                self.assertEqual(audio.getnframes(), 231 * 16_000)
                self.assertEqual(audio.getframerate(), 16_000)
            with self.assertRaises(FileExistsError):
                module.make_smoke_audio(source, destination)


if __name__ == "__main__":
    unittest.main()
