"""Reproduce QNN GatherElements lowering and verify a one-dimensional Gather."""
import argparse
import json
from pathlib import Path
import numpy as np
import onnx
from onnx import helper as h,numpy_helper as nh,TensorProto as T
import onnxruntime as ort
import onnxruntime_qnn as qnn
import sys
sys.path.insert(0,str(Path(__file__).resolve().parent))
from relative_shift import shift_nodes


def gather_model(flat=False,indices32=False,shift=False):
    frames,heads,context=256,8,256
    relative=np.broadcast_to(np.arange(frames)[None,:]-np.arange(frames)[:,None]+context,(1,heads,frames,frames)).copy()
    initializers=[]
    if shift:
        nodes,initializers=shift_nodes('data','output','shift',frames,heads,context)
    elif flat:
        offsets=np.arange(heads*frames).reshape(1,heads,frames,1)*(2*context+1)
        indices=(offsets+relative).astype('int32')
        initializers=[nh.from_array(np.array([-1],dtype='int64'),'flatshape'),nh.from_array(indices,'indices')]
        nodes=[h.make_node('Reshape',['data','flatshape'],['flat']),h.make_node('Gather',['flat','indices'],['output'],axis=0)]
    else:
        initializers=[nh.from_array(relative.astype('int32' if indices32 else 'int64'),'indices')]
        nodes=[h.make_node('GatherElements',['data','indices'],['output'],axis=3)]
    graph=h.make_graph(nodes,'gather',[h.make_tensor_value_info('data',T.FLOAT,[1,heads,frames,2*context+1])],[h.make_tensor_value_info('output',T.FLOAT,[1,heads,frames,frames])],initializers)
    return h.make_model(graph,opset_imports=[h.make_opsetid('',17)],ir_version=10)


def run(folder):
    ort.register_execution_provider_library(qnn.get_ep_name(),qnn.get_library_path())
    device=next(d for d in ort.get_ep_devices() if d.ep_name==qnn.get_ep_name() and d.device.type==ort.OrtHardwareDeviceType.NPU)
    # Exact FP16-representable integers, at the real attention tensor dimensions.
    data=(np.arange(8*256*513,dtype='float32')%1024).reshape(1,8,256,513)
    results=[];reference=None
    for label,flat,i32,shift in [('elements64',False,False,False),('flat',True,True,False),('shift',False,False,True)]:
        path=folder/f'gather-probe-{label}.onnx';onnx.save(gather_model(flat,i32,shift),path)
        expected=ort.InferenceSession(str(path),providers=['CPUExecutionProvider']).run(None,{'data':data})[0]
        if reference is None: reference=expected
        else: np.testing.assert_array_equal(expected,reference)
        so=ort.SessionOptions();so.enable_profiling=True;so.profile_file_prefix=str(folder/f'gather-probe-{label}')
        so.add_provider_for_devices([device],{'backend_path':qnn.get_qnn_htp_path()})
        session=ort.InferenceSession(str(path),so);actual=session.run(None,{'data':data})[0]
        events=json.loads(Path(session.end_profiling()).read_text())
        providers=sorted({e['args']['provider'] for e in events if e.get('args',{}).get('provider')})
        results.append({'variant':label,'mismatches':int(np.count_nonzero(np.abs(actual-expected)>0.01)),'max_abs_error':float(np.max(np.abs(actual-expected))),'providers':providers})
    (folder/'gather-probe.json').write_text(json.dumps(results,indent=2),encoding='utf-8');print(json.dumps(results),flush=True)
    if results[-1]['mismatches'] or 'QNNExecutionProvider' not in results[-1]['providers']:
        raise RuntimeError('Reshape/slice relative shift not verified on NPU')


if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--lab',required=True,type=Path);run(p.parse_args().lab)
