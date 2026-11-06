import sys
from pathlib import Path
import tempfile
import unittest
import numpy as np
import onnx
from onnx import helper as h, numpy_helper as nh, TensorProto as T
import onnxruntime as ort
sys.path.insert(0,str(Path(__file__).resolve().parent))
from externalize_fp32 import convert


class ExternalWeightsTest(unittest.TestCase):
    def test_promotes_weight_cast_without_changing_values(self):
        with tempfile.TemporaryDirectory() as d:
            p=Path(d); source=p/'source.onnx'; target=p/'target.onnx'
            weight=np.array([[0.1,2.5],[-3,1.125]],dtype='float16')
            nodes=[h.make_node('Cast',['half'],['full'],name='weight_cast',to=T.FLOAT),h.make_node('MatMul',['x','full'],['y'],name='mm')]
            graph=h.make_graph(nodes,'tiny',[h.make_tensor_value_info('x',T.FLOAT,[1,2])],[h.make_tensor_value_info('y',T.FLOAT,[1,2])],[nh.from_array(weight,'half')])
            model=h.make_model(graph,opset_imports=[h.make_opsetid('',17)],ir_version=10)
            onnx.save_model(model,source,save_as_external_data=True,all_tensors_to_one_file=True,location='source.weights',size_threshold=0)
            before=source.read_bytes(); original_weights=(p/'source.weights').read_bytes()
            report=convert(source,target)
            x=np.array([[2,-1]],dtype='float32')
            actual=ort.InferenceSession(str(target),providers=['CPUExecutionProvider']).run(None,{'x':x})[0]
            np.testing.assert_array_equal(actual,x@weight.astype('float32'))
            self.assertEqual(report['promoted_initializers'],1)
            out=onnx.load(target,load_external_data=False)
            self.assertEqual(out.opset_import[0].version,21)
            self.assertFalse(any(n.op_type=='Cast' for n in out.graph.node))
            self.assertFalse(out.graph.initializer[0].raw_data)
            self.assertEqual(source.read_bytes(),before)
            self.assertEqual((p/'source.weights').read_bytes(),original_weights)
            with self.assertRaises(FileExistsError): convert(source,target)


if __name__=='__main__': unittest.main()
