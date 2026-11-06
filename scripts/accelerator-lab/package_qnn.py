"""Normalize a qualified fixed QNN encoder into a reproducible local model package.

This is build-time tooling; it does not install anything or publish model files.
The generated manifest must be reviewed/pinned with the host before distribution.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import onnx


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def package(source, output):
    source, output = source.resolve(), output.resolve()
    if output.exists():
        raise FileExistsError(output)
    model = onnx.load(source, load_external_data=False)
    inputs = {i.name: i for i in model.graph.input}
    shape = [d.dim_value for d in inputs['audio_signal'].type.tensor_type.shape.dim]
    if shape != [1, 128, 2048]:
        raise ValueError('Only the qualified 2048-frame graph is supported')
    external = set()
    for tensor in model.graph.initializer:
        for entry in tensor.external_data:
            if entry.key == 'location':
                path = (source.parent / entry.value).resolve()
                if path.parent != source.parent or not path.is_file():
                    raise ValueError('External weights must be a sibling file')
                external.add(path)
                entry.value = 'encoder.weights'
    if len(external) != 1:
        raise ValueError('Expected one external weight file')
    output.mkdir(parents=True)
    shutil.copyfile(external.pop(), output / 'encoder.weights')
    onnx.save(model, output / 'encoder.onnx')
    onnx.checker.check_model(str(output / 'encoder.onnx'))
    manifest = {'schema': 1, 'revision': 'dense-exact-shift-v1', 'mel_frames': 2048,
                'source_model': 'pianissimo-sv-onnx-32118e6',
                'source_graph_sha256': digest(source),
                'files': [{'name': name, 'size': (output/name).stat().st_size,
                           'sha256': digest(output/name)}
                          for name in ('encoder.onnx', 'encoder.weights')]}
    (output/'manifest.json').write_text(json.dumps(manifest, indent=2)+'\n', encoding='utf-8')
    print(json.dumps(manifest))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    package(args.source, args.out)
