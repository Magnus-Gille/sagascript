"""Independent scalar reference for the Mac-derived ONNX attention adapter."""
import sys
from pathlib import Path
import unittest
import numpy as np
import onnx
import onnxruntime as ort
sys.path.insert(0, str(Path(__file__).resolve().parent))
from dense_attention import attention_nodes


class DenseAttentionTest(unittest.TestCase):
    def test_relative_band_and_padding(self):
        # More frames than the band width; asymmetric positions catch a reversed
        # relative index, and padded queries/keys must not leak into valid output.
        frames, heads, width, context = 7, 2, 4, 2
        rng = np.random.default_rng(37)
        q, k, v = [rng.normal(size=(1, heads, frames, width)).astype('float32') for _ in range(3)]
        pos = rng.normal(size=(1, heads, width, 2*context+1)).astype('float32')
        u, w = [rng.normal(size=(heads, 1, width)).astype('float32') for _ in range(2)]
        for valid in (1, 5, frames):
            pad = np.arange(frames)[None, :] >= valid
            inputs = {'q': q, 'k': k, 'v': v, 'pos': pos, 'u': u, 'w': w, 'pad': pad}
            nodes, initializers, output = attention_nodes('test', frames, heads, width, context)
            infos = [onnx.helper.make_tensor_value_info(n, onnx.TensorProto.BOOL if x.dtype == bool else onnx.TensorProto.FLOAT, x.shape) for n,x in inputs.items()]
            graph = onnx.helper.make_graph(nodes, 'test', infos, [onnx.helper.make_tensor_value_info(output, onnx.TensorProto.FLOAT, [1,heads,frames,width])], initializers)
            model = onnx.helper.make_model(graph, opset_imports=[onnx.helper.make_opsetid('',17)], ir_version=10)
            session = ort.InferenceSession(model.SerializeToString(), providers=['CPUExecutionProvider'])
            actual = session.run(None, inputs)[0]
            expected = np.zeros_like(q)
            for h in range(heads):
                for i in range(valid):
                    keys = [j for j in range(valid) if abs(j-i) <= context]
                    scores = np.array([(np.dot(q[0,h,i]+u[h,0],k[0,h,j]) + np.dot(q[0,h,i]+w[h,0],pos[0,h,:,j-i+context])) / np.sqrt(width) for j in keys])
                    probs = np.exp(scores-scores.max()); probs /= probs.sum()
                    expected[0,h,i] = sum(probs[n]*v[0,h,j] for n,j in enumerate(keys))
            np.testing.assert_allclose(actual, expected, atol=2e-6, rtol=2e-6)


if __name__ == '__main__':
    unittest.main()
