"""Compare CPU/QNN intermediate tensors on the same public audio and graph."""
import argparse
import gc
import json
from pathlib import Path
import sys
import time
import numpy as np
import onnx
import onnxruntime as ort
import onnxruntime_qnn as qnn
sys.path.insert(0,str(Path(__file__).resolve().parent))
from bench import pcm
from pipeline import options


def cut(source,target,names,frames):
    model=onnx.load(source,load_external_data=False)
    if target.exists(): raise FileExistsError(target)
    needed=set(names);nodes=[]
    for n in reversed(model.graph.node):
        if any(o in needed for o in n.output):
            nodes.append(n);needed.update(n.input)
    del model.graph.node[:];model.graph.node.extend(reversed(nodes))
    initializers=[x for x in model.graph.initializer if x.name in needed]
    del model.graph.initializer[:];model.graph.initializer.extend(initializers)
    del model.graph.output[:]
    for name in names:
        count=(frames+7)//8
        suffix=name.rsplit('/',1)[-1]
        if '/dense/' in name:
            shape=([1,8,128,513] if suffix=='pos' else
                   [1,8,count,513] if suffix=='bd' else
                   [1,8,count,128] if suffix in ('q','k','v','qu','qw','weighted_values') else
                   [1,8,count,count])
        else:
            shape=[1,count,1024]
        model.graph.output.append(onnx.helper.make_tensor_value_info(name,onnx.TensorProto.FLOAT,shape))
    onnx.save(model,target);onnx.checker.check_model(str(target))


def main(a):
    names=a.names.split(','); target=a.lab/(a.label+'.onnx')
    cut(a.encoder,target,names,a.frames)
    corpus=json.loads((a.lab/'corpus.json').read_text(encoding='utf-8'))
    samples=pcm(corpus[a.clip]['file'])
    pre=ort.InferenceSession(str(a.model/'nemo128.onnx'),options(4),providers=['CPUExecutionProvider'])
    features,length=pre.run(None,{'waveforms':samples[None],'waveforms_lens':np.array([len(samples)],dtype='int64')})
    actual_frames=features.shape[2];valid=(int(length[0])+7)//8
    features=np.pad(features,((0,0),(0,0),(0,a.frames-actual_frames)))
    del pre;gc.collect()
    feed={'audio_signal':features,'length':length}
    cpu=ort.InferenceSession(str(target),options(4),providers=['CPUExecutionProvider'])
    expected=cpu.run(None,feed);del cpu;gc.collect()
    ort.register_execution_provider_library(qnn.get_ep_name(),qnn.get_library_path())
    device=next(d for d in ort.get_ep_devices() if d.ep_name==qnn.get_ep_name() and d.device.type==ort.OrtHardwareDeviceType.NPU)
    so=options(4);so.enable_profiling=True;so.profile_file_prefix=str(a.lab/(a.label+'-profile'))
    so.add_provider_for_devices([device],{'backend_path':qnn.get_qnn_htp_path(),'htp_performance_mode':'burst'})
    npu=ort.InferenceSession(str(target),so);actual=npu.run(None,feed)
    result={'label':a.label,'clip_id':corpus[a.clip]['id'],'valid_frames':valid,'tensors':[]}
    for name,x,y in zip(names,expected,actual):
        if x.ndim==3:
            x=x[:,:valid];y=y[:,:valid]
        elif x.ndim==4 and not name.endswith('/pos'):
            x=x[:,:,:valid];y=y[:,:,:valid]
        x=x.astype('float64');y=y.astype('float64')
        result['tensors'].append({'name':name,'finite':bool(np.isfinite(y).all()),'cpu_absmax':float(np.max(np.abs(x))),
            'npu_absmax':float(np.max(np.abs(y))),'max_absolute_error':float(np.max(np.abs(x-y))),
            'mean_absolute_error':float(np.mean(np.abs(x-y))),'relative_l2':float(np.linalg.norm(x-y)/max(np.linalg.norm(x),1e-12))})
    events=json.loads(Path(npu.end_profiling()).read_text())
    result['placement']={p:len({e['name'] for e in events if e.get('args',{}).get('provider')==p}) for p in ['QNNExecutionProvider','CPUExecutionProvider']}
    (a.lab/(a.label+'.json')).write_text(json.dumps(result,indent=2),encoding='utf-8');print(json.dumps(result),flush=True)


if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--lab',type=Path,required=True);p.add_argument('--model',type=Path,required=True);p.add_argument('--encoder',type=Path,required=True)
    p.add_argument('--frames',type=int,default=2048);p.add_argument('--clip',type=int,default=0);p.add_argument('--label',required=True)
    p.add_argument('--names',default='/pre_encode/out/Add_output_0,/layers.0/Add_output_0,/layers.0/Add_1_output_0,/layers.0/Add_2_output_0,/layers.0/norm_out/LayerNormalization_output_0')
    main(p.parse_args())
