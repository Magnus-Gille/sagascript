"""Small synthetic-model tests for staged QNN calibration and cache binding."""

from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import unittest

import numpy as np
import onnx
from onnx import TensorProto, helper

sys.path.insert(0, str(Path(__file__).resolve().parent))
import staged_qdq


class StagedQdqTests(unittest.TestCase):
    def make_tiny_encoder(self, folder: Path) -> Path:
        source = folder / "tiny.onnx"
        graph = helper.make_graph(
            [
                helper.make_node("MatMul", ["audio_signal", "weight"], ["hidden"], name="tiny_matmul"),
                helper.make_node("Identity", ["hidden"], ["encoded"], name="tiny_output"),
            ],
            "tiny_encoder",
            [helper.make_tensor_value_info("audio_signal", TensorProto.FLOAT, [1, 2])],
            [helper.make_tensor_value_info("encoded", TensorProto.FLOAT, [1, 2])],
            [onnx.numpy_helper.from_array(np.eye(2, dtype=np.float32), "weight")],
        )
        model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 21)])
        model.ir_version = 13
        onnx.checker.check_model(model)
        onnx.save_model(model, source)
        return source

    def test_single_fixture_batches_merge_minmax_ranges(self):
        with tempfile.TemporaryDirectory(prefix="staged-qdq-test-") as temp:
            folder = Path(temp)
            source = self.make_tiny_encoder(folder)
            config = staged_qdq._qnn_config(source)
            samples = [
                {"audio_signal": np.asarray([[1.0, 2.0]], dtype=np.float32)},
                {"audio_signal": np.asarray([[-3.0, 4.0]], dtype=np.float32)},
            ]

            ranges = staged_qdq.collect_qnn_minmax_ranges(source, config, samples, folder)

            self.assertIn("hidden", ranges.data)
            low, high = ranges.data["hidden"].range_value
            np.testing.assert_allclose(low, -3.0)
            np.testing.assert_allclose(high, 4.0)

    def test_cache_verification_rejects_stale_provenance(self):
        with tempfile.TemporaryDirectory(prefix="staged-qdq-test-") as temp:
            cache = Path(temp) / "ranges.json"
            cache.write_text("{}", encoding="utf-8")
            expected = {
                "schema_version": 1,
                "source": {"fingerprint": "source-a"},
                "frames": 2048,
                "data_hash": "data-a",
            }
            staged_qdq._provenance_path(cache).write_text(
                json.dumps(expected), encoding="utf-8"
            )

            staged_qdq.verify_provenance(cache, expected)
            stale = dict(expected, data_hash="data-b")
            with self.assertRaisesRegex(ValueError, "provenance mismatch"):
                staged_qdq.verify_provenance(cache, stale)

    def test_model_fingerprint_covers_external_weight_bytes(self):
        with tempfile.TemporaryDirectory(prefix="staged-qdq-test-") as temp:
            folder = Path(temp)
            source = self.make_tiny_encoder(folder)
            model = onnx.load_model(source)
            external_source = folder / "external.onnx"
            onnx.save_model(
                model,
                external_source,
                save_as_external_data=True,
                all_tensors_to_one_file=True,
                location="external.onnx.data",
                size_threshold=0,
            )
            before = staged_qdq._model_fingerprint(external_source)
            data_file = folder / "external.onnx.data"
            original = data_file.read_bytes()
            data_file.write_bytes(bytes([original[0] ^ 0x01]) + original[1:])

            after = staged_qdq._model_fingerprint(external_source)

            self.assertNotEqual(before["fingerprint"], after["fingerprint"])


if __name__ == "__main__":
    unittest.main()
