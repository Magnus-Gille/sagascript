"""Bounded QNN static-quantization experiment using separate public calibration audio.

Never modifies publisher models. Run through supervise.py with memory/time limits.
"""
import argparse
import csv
import functools
import gc
import hashlib
import json
from pathlib import Path
import sys
import tarfile
import time

import numpy as np
import onnxruntime as ort
from onnxruntime.quantization import CalibrationDataReader, QuantType, quantize
from onnxruntime.quantization.execution_providers.qnn import get_qnn_qdq_config, qnn_preprocess_model
import onnxruntime.quantization.execution_providers.qnn.preprocess as qnn_preprocess_module

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bench import pcm
from windows_qos import process_qos
import os


class Reader(CalibrationDataReader):
    def __init__(self, samples):
        self.samples = samples
        self.rewind()

    def get_next(self):
        return next(self.iterator, None)

    def rewind(self):
        self.iterator = iter(self.samples)


def main(a):
    status = {'status': 'preparing', 'frames': a.frames, 'calibration_count': a.count,
              'activation': 'uint16', 'weights': 'uint8', 'source': str(a.encoder)}
    target = a.lab / (a.label + '.json')
    def save():
        target.write_text(json.dumps(status, indent=2), encoding='utf-8')
    save()
    process_qos(os.getpid(), 'high')
    existing = json.loads((a.lab/'corpus.json').read_text(encoding='utf-8'))
    excluded_files = {Path(c['file']).name for c in existing}
    excluded_refs = {c['reference'].strip().casefold() for c in existing}
    chosen, seen = [], set()
    with (a.lab/'fleurs-sv-dev.tsv').open(encoding='utf-8') as stream:
        for row in csv.reader(stream, delimiter='\t'):
            if row[1] in excluded_files or row[2].strip().casefold() in excluded_refs or row[0] in seen:
                continue
            if not 6 <= int(row[5])/16000 <= 19:
                continue
            chosen.append(row)
            seen.add(row[0])
            if len(chosen) == a.count:
                break
    if len(chosen) != a.count:
        raise ValueError('Not enough disjoint public calibration utterances')
    folder = a.lab/'calibration-audio'
    folder.mkdir(exist_ok=True)
    wanted = {row[1] for row in chosen}
    with tarfile.open(a.lab/'fleurs-sv-dev.tar.gz') as archive:
        for member in archive:
            name = Path(member.name).name
            if name in wanted and member.isfile():
                (folder/name).write_bytes(archive.extractfile(member).read())
    so = ort.SessionOptions()
    so.intra_op_num_threads = 4
    so.inter_op_num_threads = 1
    pre = ort.InferenceSession(str(a.model/'nemo128.onnx'), so, providers=['CPUExecutionProvider'])
    samples, manifest = [], []
    for row in chosen:
        path = folder/row[1]
        waveform = pcm(path)[None, :]
        features, length = pre.run(None, {'waveforms': waveform, 'waveforms_lens': np.array([waveform.shape[1]], np.int64)})
        if features.shape[-1] > a.frames:
            raise ValueError('Calibration audio does not fit fixed model')
        padded = np.zeros((1,128,a.frames), np.float32)
        padded[:,:,:features.shape[-1]] = features
        samples.append({'audio_signal': padded, 'length': length})
        manifest.append({'file':str(path), 'sentence_id':row[0], 'audio_s':waveform.shape[1]/16000,
                         'sha256':hashlib.sha256(path.read_bytes()).hexdigest()})
    del pre
    gc.collect()
    (a.lab/'calibration-manifest.json').write_text(json.dumps(manifest, indent=2), encoding='utf-8')
    processed = a.lab/(a.label + '-preprocessed.onnx')
    output = a.lab/(a.label + '-u16-u8.onnx')
    if processed.exists() or output.exists():
        raise FileExistsError('Keep prior quantization artifacts; choose a fresh lab directory')
    status['status'] = 'preprocessing'
    save()
    if a.external_optimizer:
        # ORT's QNN preprocessor does not forward its external-data flag to this
        # internal optimization step. CPU promotion of FP16 weights exceeds the
        # 2 GB protobuf limit. Change only this research process's call default.
        qnn_preprocess_module.optimize_model = functools.partial(
            qnn_preprocess_module.optimize_model, use_external_initializers=True)
    changed = qnn_preprocess_model(a.encoder, processed, fuse_layernorm=True,
                                   save_as_external_data=True, all_tensors_to_one_file=True,
                                   external_data_location=a.label + '-preprocessed.weights')
    source = processed if changed else a.encoder
    status['preprocessing_changed'] = changed
    status['status'] = 'calibrating_and_quantizing'
    save()
    reader = Reader(samples)
    config = get_qnn_qdq_config(source, reader, activation_type=QuantType.QUInt16,
                                weight_type=QuantType.QUInt8, per_channel=True,
                                calibration_providers=['CPUExecutionProvider'])
    config.use_external_data_format = True
    config.extra_options['CalibMaxIntermediateOutputs'] = 1
    started = time.perf_counter()
    quantize(source, output, config)
    status.update(status='complete', quantization_s=time.perf_counter()-started, output=str(output))
    save()
    print(json.dumps(status), flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--lab', required=True, type=Path)
    parser.add_argument('--model', required=True, type=Path)
    parser.add_argument('--encoder', required=True, type=Path)
    parser.add_argument('--frames', type=int, default=2048)
    parser.add_argument('--count', type=int, default=5)
    parser.add_argument('--label', default='qdq-preparation')
    parser.add_argument('--external-optimizer', action='store_true')
    args = parser.parse_args()
    main(args)
