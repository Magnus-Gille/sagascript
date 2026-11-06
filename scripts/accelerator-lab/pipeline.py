"""Complete local Pianissimo inference with measured encoder placement.

Matches the shipped greedy TDT loop; compare CPU token IDs against bench.py
before interpreting accelerator results. This is an isolated research runner.
"""
import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import sys
import time

import numpy as np
import onnx
import onnxruntime as ort
import psutil

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bench import pcm, wer


def options(threads=4):
    so = ort.SessionOptions()
    so.intra_op_num_threads = threads
    so.inter_op_num_threads = 1
    so.enable_cpu_mem_arena = False
    so.enable_mem_pattern = False
    so.log_severity_level = 3
    return so


def decode(decoder, encoded, lengths, vocab, maximum=10):
    blank = max(vocab)
    vocab_size = blank + 1
    state1 = np.zeros((2, 1, 640), np.float32)
    state2 = np.zeros_like(state1)
    frames = encoded[0].T
    valid = min(int(np.asarray(lengths).flat[0]), len(frames))
    t, emitted, last = 0, 0, blank
    tokens = []
    while t < valid:
        output = decoder.run(None, {
            'encoder_outputs': frames[t][None, :, None].copy(),
            'targets': np.array([[last]], np.int32),
            'target_length': np.array([1], np.int32),
            'input_states_1': state1, 'input_states_2': state2,
        })
        values = dict(zip([x.name for x in decoder.get_outputs()], output))
        logits = values['outputs'].ravel()
        if not np.isfinite(logits).all():
            raise ValueError('Non-finite decoder logits')
        token = int(logits[:vocab_size].argmax())
        step = int(logits[vocab_size:].argmax())
        if token != blank:
            state1, state2 = values['output_states_1'], values['output_states_2']
            tokens.append({'id': token, 'frame': t, 'duration_frames': step})
            last = token
            emitted += 1
        if step > 0:
            t += step
            emitted = 0
        elif token == blank or emitted == maximum:
            t += 1
            emitted = 0
    text = ''.join(vocab[t['id']] for t in tokens).replace('▁', ' ').strip()
    return text, tokens


def main(a):
    result = {'label': a.label, 'ort': ort.__version__, 'provider': a.provider,
              'encoder': str(a.encoder), 'fixed_frames': a.frames,
              'threads': a.threads, 'runs': [], 'errors': []}
    target = a.lab / (a.label + '.json')
    def save():
        target.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
    result['status'] = 'starting'
    save()
    profile = None
    try:
        if a.qos:
            from windows_qos import process_qos
            result['process_qos'] = process_qos(os.getpid(), a.qos)
        corpus = json.loads((a.lab / 'corpus.json').read_text(encoding='utf-8'))[:a.limit]
        vocab = dict((int(i), p) for p, i in (line.rsplit(' ', 1) for line in
                     (a.model / 'vocab.txt').read_text(encoding='utf-8').splitlines()))
        pre = ort.InferenceSession(str(a.model / 'nemo128.onnx'), options(a.threads),
                                   providers=['CPUExecutionProvider'])
        dec = ort.InferenceSession(str(a.model / 'decoder_joint-model.int8.onnx'), options(2),
                                   providers=['CPUExecutionProvider'])
        encoder_path = a.encoder
        if a.frames:
            encoder_path = a.lab / (a.encoder.stem + f'-frames{a.frames}.onnx')
            if not encoder_path.exists():
                model = onnx.load(str(a.encoder))
                for inp in model.graph.input:
                    shape = (1, 128, a.frames) if inp.name == 'audio_signal' else (1,)
                    for dim, size in zip(inp.type.tensor_type.shape.dim, shape):
                        dim.ClearField('dim_param')
                        dim.dim_value = size
                onnx.save(model, str(encoder_path))
                del model
        so = options(a.threads)
        so.enable_profiling = True
        so.profile_file_prefix = str(a.lab / (a.label + '-profile'))
        if a.basic:
            so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_BASIC
        if a.context_cache:
            so.add_session_config_entry('ep.context_enable', '1')
            so.add_session_config_entry('ep.context_embed_mode', '0')
            so.add_session_config_entry('ep.context_file_path', str(a.context_cache))
        if a.provider != 'cpu':
            import onnxruntime_qnn as qnn
            result['qnn_version'] = qnn.__version__
            ort.register_execution_provider_library(qnn.get_ep_name(), qnn.get_library_path())
            kind = ort.OrtHardwareDeviceType.NPU if a.provider == 'qnn-npu' else ort.OrtHardwareDeviceType.GPU
            devices = [d for d in ort.get_ep_devices() if d.ep_name == qnn.get_ep_name() and d.device.type == kind]
            result['matching_devices'] = len(devices)
            if not devices:
                raise RuntimeError('Requested accelerator not found')
            backend = qnn.get_qnn_htp_path() if a.provider == 'qnn-npu' else qnn.get_qnn_gpu_path()
            config = {'backend_path': backend}
            if a.provider == 'qnn-npu':
                config.update(htp_performance_mode='burst')
            if a.strict:
                so.add_session_config_entry('session.disable_cpu_ep_fallback', '1')
            so.add_provider_for_devices(devices[:1], config)
        started = time.perf_counter()
        result['status'] = 'loading_encoder'
        save()
        enc = ort.InferenceSession(str(encoder_path), so, providers=['CPUExecutionProvider'] if a.provider == 'cpu' else None)
        result['encoder_load_ms'] = (time.perf_counter() - started) * 1000
        result['providers'] = enc.get_providers()
        result['status'] = 'running'
        print(json.dumps({'label': a.label, 'loaded_ms': result['encoder_load_ms']}), flush=True)
        save()
        for clip in corpus:
            samples = pcm(clip['file'])
            for iteration in range(a.repeats):
                start = time.perf_counter()
                features, lens = pre.run(None, {'waveforms': samples[None], 'waveforms_lens': np.array([len(samples)], np.int64)})
                if a.frames:
                    if features.shape[2] > a.frames:
                        raise ValueError(f"Audio exceeds fixed encoder width: {features.shape}")
                    features = np.pad(features, ((0, 0), (0, 0), (0, a.frames - features.shape[2])))
                prepared = time.perf_counter()
                encoded, lengths = enc.run(None, {'audio_signal': features, 'length': lens})
                encoded_at = time.perf_counter()
                if not np.isfinite(encoded).all():
                    raise ValueError('Non-finite encoder output')
                text, tokens = decode(dec, encoded, lengths, vocab)
                done = time.perf_counter()
                row = {'id': clip['id'], 'iteration': iteration, 'audio_s': len(samples)/16000,
                       'wall_ms': (done-start)*1000, 'timings': {'preprocess_ms': (prepared-start)*1000,
                       'encode_ms': (encoded_at-prepared)*1000, 'decode_ms': (done-encoded_at)*1000},
                       'text': text, 'reference': clip['reference'], 'quality': wer(clip['reference'], text),
                       'tokens': tokens, 'rss_mb': psutil.Process().memory_info().rss/1e6,
                       'encoded_lengths': lengths.tolist(), 'features_shape': list(features.shape)}
                result['runs'].append(row)
                save()
                print(json.dumps({k: row[k] for k in ['id', 'iteration', 'wall_ms', 'quality']}, ensure_ascii=True), flush=True)
        profile = enc.end_profiling()
        result['profile'] = profile
        events = json.loads(Path(profile).read_text())
        nodes = [e for e in events if e.get('args', {}).get('provider')]
        result['placement_events'] = dict(Counter(e['args']['provider'] for e in nodes))
        result['provider_node_duration_us'] = {p: sum(e.get('dur',0) for e in nodes if e['args']['provider']==p)
                                             for p in result['placement_events']}
        result['unique_provider_nodes'] = {p: sorted({e['name'] for e in nodes if e['args']['provider']==p})
                                           for p in result['placement_events']}
        result['status'] = 'complete'
    except Exception as e:
        result['errors'].append(repr(e))
        result['status'] = 'failed'
        print(json.dumps({'label': a.label, 'error': repr(e)}), flush=True)
    finally:
        save()
    return 1 if result['errors'] else 0


if __name__ == '__main__':
    p = argparse.ArgumentParser()
    p.add_argument('--lab', required=True, type=Path)
    p.add_argument('--model', required=True, type=Path)
    p.add_argument('--encoder', required=True, type=Path)
    p.add_argument('--provider', choices=['cpu','qnn-npu','qnn-gpu'], default='cpu')
    p.add_argument('--label', required=True)
    p.add_argument('--frames', type=int, default=0)
    p.add_argument('--threads', type=int, default=4)
    p.add_argument('--limit', type=int, default=11)
    p.add_argument('--repeats', type=int, default=2)
    p.add_argument('--strict', action='store_true')
    p.add_argument('--basic', action='store_true')
    p.add_argument('--context-cache', type=Path)
    p.add_argument('--qos', choices=['system', 'high', 'eco'])
    sys.exit(main(p.parse_args()))
