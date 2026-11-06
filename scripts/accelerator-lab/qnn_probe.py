"""Probe actual NPU placement, rejecting silent whole-model CPU fallback."""
import argparse
from collections import Counter
import json
from pathlib import Path
import time
import wave
import numpy as np
import onnx
import onnxruntime as ort
import onnxruntime_qnn as qnn

p = argparse.ArgumentParser()
p.add_argument('--lab', type=Path, required=True)
p.add_argument('--model', type=Path, required=True)
p.add_argument('--audio', type=Path, required=True)
p.add_argument('--encoder', type=Path)
p.add_argument('--fixed', action='store_true')
p.add_argument('--allow-fallback', action='store_true')
p.add_argument('--label', default='qnn-int8')
a = p.parse_args()
out = {'ort':ort.__version__, 'qnn':qnn.__version__, 'fixed':a.fixed, 'allow_fallback':a.allow_fallback}
try:
    ort.register_execution_provider_library(qnn.get_ep_name(), qnn.get_library_path())
    devices = [d for d in ort.get_ep_devices() if d.ep_name == 'QNNExecutionProvider' and d.device.type == ort.OrtHardwareDeviceType.NPU]
    out['npu_devices'] = len(devices)
    if not devices:
        raise RuntimeError('No NPU found')
    so = ort.SessionOptions()
    so.intra_op_num_threads = 4
    so.inter_op_num_threads = 1
    pre = ort.InferenceSession(str(a.model/'nemo128.onnx'), so, providers=['CPUExecutionProvider'])
    with wave.open(str(a.audio)) as w:
        assert (w.getnchannels(),w.getframerate(),w.getsampwidth()) == (1,16000,2)
        waveforms = np.frombuffer(w.readframes(w.getnframes()), dtype='<i2').astype(np.float32)[None,:]/32768
    features, lens = pre.run(None, {'waveforms':waveforms,'waveforms_lens':np.array([waveforms.shape[1]],dtype=np.int64)})
    np.savez(a.lab/'features.npz',audio_signal=features,length=lens)
    enc_path = a.encoder or a.model/'encoder-model.int8.onnx'
    m = onnx.load(str(enc_path))
    out['ops'] = dict(Counter((n.domain+':'+n.op_type) for n in m.graph.node))
    out['inputs'] = {i.name:str(i.type.tensor_type.shape) for i in m.graph.input}
    if a.fixed:
        for inp in m.graph.input:
            shape = features.shape if inp.name=='audio_signal' else lens.shape
            for dim,size in zip(inp.type.tensor_type.shape.dim,shape):
                dim.ClearField('dim_param')
                dim.dim_value=size
        enc_path = a.lab/(a.label+'-fixed.onnx')
        onnx.save(m,str(enc_path))
    del m
    so = ort.SessionOptions()
    so.intra_op_num_threads=4
    so.inter_op_num_threads=1
    so.enable_profiling=True
    so.log_severity_level=3
    so.profile_file_prefix=str(a.lab/(a.label+'-profile'))
    if not a.allow_fallback:
        so.add_session_config_entry('session.disable_cpu_ep_fallback','1')
    so.add_provider_for_devices(devices[:1], {'backend_path':qnn.get_qnn_htp_path(),'htp_performance_mode':'burst'})
    started=time.perf_counter()
    enc=ort.InferenceSession(str(enc_path),so)
    out['load_ms']=(time.perf_counter()-started)*1000
    out['providers']=enc.get_providers()
    out['runs']=[]
    for _ in range(3):
        started=time.perf_counter()
        values=enc.run(None,{'audio_signal':features,'length':lens})
        out['runs'].append((time.perf_counter()-started)*1000)
        np.savez(a.lab/(a.label+'-output.npz'),outputs=values[0],encoded_lengths=values[1])
    out['profile']=enc.end_profiling()
    events=json.loads(Path(out['profile']).read_text())
    out['placement']=dict(Counter(e.get('args',{}).get('provider') for e in events if e.get('args',{}).get('provider')))
except Exception as e:
    out['error']=repr(e)
finally:
    (a.lab/(a.label+'.json')).write_text(json.dumps(out,ensure_ascii=False,indent=2),encoding='utf-8')
    print(json.dumps({k:v for k,v in out.items() if k not in ['ops','inputs']},ensure_ascii=False),flush=True)
