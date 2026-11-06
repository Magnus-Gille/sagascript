"""Hardware regression probe: QNN must preserve ONNX truncation of positive lengths."""
import argparse
import json
from pathlib import Path
import numpy as np
import onnx
from onnx import helper as h, numpy_helper as nh, TensorProto as T
import onnxruntime as ort
import onnxruntime_qnn as qnn


def length_model(floor):
    nodes=[h.make_node('Cast',['length'],['f0'],to=T.FLOAT)]
    for i in range(3):
        nodes.extend([h.make_node('Sub',[f'f{i}','one'],[f's{i}']),h.make_node('Div',[f's{i}','two'],[f'd{i}'])])
        name=f'd{i}'
        if floor:
            nodes.append(h.make_node('Floor',[name],[f'floor{i}']));name=f'floor{i}'
        nodes.extend([h.make_node('Cast',[name],[f'long{i}'],to=T.INT64),h.make_node('Cast',[f'long{i}'],[f'back{i}'],to=T.FLOAT),h.make_node('Add',[f'back{i}','one'],[f'f{i+1}'])])
    nodes.append(h.make_node('Cast',['f3'],['result'],to=T.INT64))
    graph=h.make_graph(nodes,'length',[h.make_tensor_value_info('length',T.INT64,[2048])],[h.make_tensor_value_info('result',T.INT64,[2048])],[nh.from_array(np.array(1,dtype='float32'),'one'),nh.from_array(np.array(2,dtype='float32'),'two')])
    return h.make_model(graph,opset_imports=[h.make_opsetid('',17)],ir_version=10)


def run(folder):
    ort.register_execution_provider_library(qnn.get_ep_name(),qnn.get_library_path())
    device=next(d for d in ort.get_ep_devices() if d.ep_name==qnn.get_ep_name() and d.device.type==ort.OrtHardwareDeviceType.NPU)
    lengths=np.arange(1,2049,dtype='int64'); expected=(lengths+7)//8
    results=[]
    for floor in [False,True]:
        source=folder/f'length-floor-{floor}.onnx';onnx.save(length_model(floor),source)
        cpu=ort.InferenceSession(str(source),providers=['CPUExecutionProvider']).run(None,{'length':lengths})[0]
        np.testing.assert_array_equal(cpu,expected)
        so=ort.SessionOptions();so.enable_profiling=True;so.profile_file_prefix=str(folder/f'length-floor-{floor}')
        so.add_provider_for_devices([device],{'backend_path':qnn.get_qnn_htp_path(),'htp_performance_mode':'burst'})
        session=ort.InferenceSession(str(source),so)
        actual=session.run(None,{'length':lengths})[0]
        profile=json.loads(Path(session.end_profiling()).read_text())
        providers=sorted({e['args']['provider'] for e in profile if e.get('args',{}).get('provider')})
        bad=np.flatnonzero(actual!=expected)
        results.append({'explicit_floor':floor,'mismatches':len(bad),'first_mismatches':[(int(lengths[i]),int(expected[i]),int(actual[i])) for i in bad[:12]],'providers':providers})
    (folder/'length-probe.json').write_text(json.dumps(results,indent=2),encoding='utf-8')
    print(json.dumps(results),flush=True)
    if results[1]['mismatches'] or 'QNNExecutionProvider' not in results[1]['providers']:
        raise RuntimeError('Explicit Floor did not establish correct NPU lengths')


if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--lab',required=True,type=Path)
    run(p.parse_args().lab)
