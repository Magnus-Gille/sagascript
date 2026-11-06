import sys
from pathlib import Path
import unittest
import tempfile
import numpy as np
import onnx
from onnx import helper as h, TensorProto as T
import onnxruntime as ort
sys.path.insert(0,str(Path(__file__).resolve().parent))
from relative_shift import shift_nodes, patch


class RelativeShiftTest(unittest.TestCase):
    def test_rejects_wrong_frame_count_before_writing_model(self):
        with tempfile.TemporaryDirectory() as directory:
            source, target = Path(directory)/'source.onnx', Path(directory)/'target.onnx'
            graph = h.make_graph([], 'long', [h.make_tensor_value_info('audio_signal', T.FLOAT, [1,128,3001])], [])
            onnx.save(h.make_model(graph), source)
            with self.assertRaisesRegex(ValueError, 'encoded frames'):
                patch(source, target, 256)
            self.assertFalse(target.exists())

    def test_matches_scalar_relative_gather_inside_band(self):
        rng=np.random.default_rng(123)
        for context,frames in [(4,1),(4,3),(4,7),(4,8),(256,96),(256,256),(256,376)]:
            data=rng.normal(size=(1,2,frames,2*context+1)).astype('float32')
            nodes,init=shift_nodes('data','out','test',frames,2,context)
            graph=h.make_graph(nodes,'shift',[h.make_tensor_value_info('data',T.FLOAT,list(data.shape))],[h.make_tensor_value_info('out',T.FLOAT,[1,2,frames,frames])],init)
            model=h.make_model(graph,opset_imports=[h.make_opsetid('',17)],ir_version=10)
            actual=ort.InferenceSession(model.SerializeToString(),providers=['CPUExecutionProvider']).run(None,{'data':data})[0]
            for i in range(frames):
                lo,hi=max(0,i-context),min(frames,i+context+1)
                np.testing.assert_array_equal(actual[:,:,i,lo:hi],data[:,:,i,lo-i+context:hi-i+context])
        with self.assertRaises(ValueError): shift_nodes('data','out','test',513)


if __name__=='__main__': unittest.main()
