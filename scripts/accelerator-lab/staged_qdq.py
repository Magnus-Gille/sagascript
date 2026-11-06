"""Run QNN calibration and QDQ export as separate, provenance-bound stages.

The calibration stage uses one ORT calibrator and submits one clip per call so
MinMaxCalibrater can merge ranges without retaining every clip's intermediates.
The quantize stage consumes that cache after the calibration process has exited,
avoiding a second model/session coexisting with the quantizer's model.
"""

from __future__ import annotations

import argparse
import gc
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
from typing import Iterable, Sequence

import numpy as np
import onnx
from onnx import AttributeProto, TensorProto
import onnxruntime as ort
from onnxruntime.quantization import CalibrationDataReader, QuantType
from onnxruntime.quantization.calibrate import (
    TensorsData,
    create_calibrator,
    save_tensors_data,
)
from onnxruntime.quantization.execution_providers.qnn import get_qnn_qdq_config
from onnxruntime.quantization.quantize import quantize_static


CALIBRATION_PIPELINE = "nemo128-pcm16k-fp32-zero-pad-v1"


class Reader(CalibrationDataReader):
    """A rewindable reader with ORT's optional strided-range interface."""

    def __init__(self, samples: Sequence[dict[str, np.ndarray]]):
        self.samples = samples
        self.start = 0
        self.end = len(samples)
        self.rewind()

    def __len__(self) -> int:
        return len(self.samples)

    def set_range(self, start_index: int, end_index: int) -> None:
        if not 0 <= start_index <= end_index <= len(self.samples):
            raise ValueError("Calibration reader range is out of bounds")
        self.start = start_index
        self.end = end_index
        self.rewind()

    def get_next(self):
        if self.index >= self.end:
            return None
        sample = self.samples[self.index]
        self.index += 1
        return sample

    def rewind(self) -> None:
        self.index = self.start


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(4 * 1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _iter_graph_tensors(graph: onnx.GraphProto) -> Iterable[onnx.TensorProto]:
    yield from graph.initializer
    for sparse in graph.sparse_initializer:
        yield sparse.values
        yield sparse.indices
    for node in graph.node:
        for attribute in node.attribute:
            if attribute.type == AttributeProto.TENSOR:
                yield attribute.t
            elif attribute.type == AttributeProto.TENSORS:
                yield from attribute.tensors
            elif attribute.type == AttributeProto.GRAPH:
                yield from _iter_graph_tensors(attribute.g)
            elif attribute.type == AttributeProto.GRAPHS:
                for nested in attribute.graphs:
                    yield from _iter_graph_tensors(nested)


def _external_locations(model: onnx.ModelProto) -> list[str]:
    locations = set()
    for tensor in _iter_graph_tensors(model.graph):
        if onnx.external_data_helper.uses_external_data(tensor):
            location = onnx.external_data_helper.ExternalDataInfo(tensor).location
            if not location:
                raise ValueError(f"External tensor {tensor.name!r} has no location")
            locations.add(location)
    return sorted(locations)


def _model_fingerprint(path: Path) -> dict[str, object]:
    """Hash a model and every external-data file without loading weight bytes into ONNX."""
    path = path.resolve(strict=True)
    metadata_model = onnx.load_model(path, load_external_data=False)
    base = path.parent.resolve(strict=True)
    external = []
    for location in _external_locations(metadata_model):
        relative = Path(location)
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError(f"External data location must stay beside its model: {location!r}")
        data_path = (base / relative).resolve(strict=True)
        if not data_path.is_relative_to(base) or not data_path.is_file():
            raise ValueError(f"External data file is missing or outside the model directory: {location!r}")
        external.append({"location": relative.as_posix(), "sha256": _sha256_file(data_path)})

    result: dict[str, object] = {
        "model_sha256": _sha256_file(path),
        "external_data": external,
    }
    encoded = json.dumps(result, sort_keys=True, separators=(",", ":")).encode("utf-8")
    result["fingerprint"] = hashlib.sha256(encoded).hexdigest()
    return result


def _input_shape(value_info: onnx.ValueInfoProto) -> list[int | None]:
    tensor_type = value_info.type.tensor_type
    shape = []
    for dim in tensor_type.shape.dim:
        shape.append(dim.dim_value if dim.HasField("dim_value") else None)
    return shape


def _source_frames(path: Path) -> int:
    model = onnx.load_model(path, load_external_data=False)
    opset = next((item.version for item in model.opset_import if item.domain in ("", "ai.onnx")), None)
    if opset != 21:
        raise ValueError(f"Expected an ONNX opset 21 encoder, found {opset}")

    inputs = {item.name: item for item in model.graph.input}
    if "audio_signal" not in inputs:
        raise ValueError("Encoder is missing the audio_signal input")
    audio = inputs["audio_signal"]
    if audio.type.tensor_type.elem_type != TensorProto.FLOAT:
        raise ValueError("Encoder audio_signal input must be FP32")
    shape = _input_shape(audio)
    if len(shape) != 3 or shape[0] != 1 or shape[1] != 128 or shape[2] is None or shape[2] < 1:
        raise ValueError(f"Expected fixed audio_signal shape [1, 128, frames], found {shape}")
    if shape[2] != 2048:
        raise ValueError(f"Expected a fixed 2048-frame encoder, found {shape[2]}")
    length = inputs.get("length")
    if length is None or length.type.tensor_type.elem_type != TensorProto.INT64 or _input_shape(length) != [1]:
        raise ValueError("Expected an int64 length input with fixed shape [1]")
    return shape[2]


def _read_manifest_audio(lab: Path) -> list[tuple[dict[str, object], Path, str]]:
    manifest_path = lab / "calibration-manifest.json"
    audio_dir = (lab / "calibration-audio").resolve(strict=True)
    entries = json.loads(manifest_path.read_text(encoding="utf-8"))
    if not isinstance(entries, list) or not entries:
        raise ValueError("Calibration manifest must contain at least one sample")

    result = []
    seen_names = set()
    for entry in entries:
        if not isinstance(entry, dict) or not isinstance(entry.get("file"), str):
            raise ValueError("Calibration manifest entry is missing its audio file")
        name = Path(entry["file"]).name
        if not name or name in seen_names:
            raise ValueError(f"Calibration manifest has an empty or duplicate audio name: {name!r}")
        seen_names.add(name)
        audio_path = (audio_dir / name).resolve(strict=True)
        if not audio_path.is_relative_to(audio_dir) or not audio_path.is_file():
            raise ValueError(f"Calibration audio is missing or outside calibration-audio: {name!r}")
        actual_hash = _sha256_file(audio_path)
        expected_hash = entry.get("sha256")
        if expected_hash is not None and expected_hash != actual_hash:
            raise ValueError(f"Calibration audio hash does not match its manifest: {name!r}")
        result.append((entry, audio_path, actual_hash))
    return result


def _provenance(source: Path, model_dir: Path, lab: Path, frames: int) -> dict[str, object]:
    source_info = _model_fingerprint(source)
    preprocessor_path = (model_dir / "nemo128.onnx").resolve(strict=True)
    preprocessor_info = _model_fingerprint(preprocessor_path)
    audio = _read_manifest_audio(lab)
    quantization_spec = {
        "ort_version": ort.__version__,
        "onnx_version": onnx.__version__,
        "numpy_version": np.__version__,
        "calibration_method": "MinMax",
        "activation_type": "QUInt16",
        "weight_type": "QUInt8",
        "per_channel": True,
        "calibration_provider": "CPUExecutionProvider",
    }
    calibration_code = {
        "staged_qdq_sha256": _sha256_file(Path(__file__).resolve()),
        "audio_reader_sha256": _sha256_file(Path(__file__).resolve().parent / "bench.py"),
    }
    data_payload = {
        "pipeline": CALIBRATION_PIPELINE,
        "frames": frames,
        "preprocessor": preprocessor_info["fingerprint"],
        "quantization": quantization_spec,
        "code": calibration_code,
        "audio": [
            {
                "name": audio_path.name,
                "sha256": audio_hash,
                "sentence_id": entry.get("sentence_id"),
            }
            for entry, audio_path, audio_hash in audio
        ],
    }
    data_hash = hashlib.sha256(
        json.dumps(data_payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    ).hexdigest()
    return {
        "schema_version": 1,
        "source": source_info,
        "preprocessor": preprocessor_info,
        "quantization": quantization_spec,
        "calibration_code": calibration_code,
        "frames": frames,
        "calibration_count": len(audio),
        "data_hash": data_hash,
        "audio": data_payload["audio"],
    }


def _provenance_path(cache: Path) -> Path:
    return Path(str(cache) + ".provenance.json")


def verify_provenance(cache: Path, expected: dict[str, object]) -> None:
    sidecar = _provenance_path(cache)
    if not cache.is_file():
        raise ValueError(f"Calibration cache is missing: {cache}")
    if not sidecar.is_file():
        raise ValueError(f"Calibration provenance is missing: {sidecar}")
    actual = json.loads(sidecar.read_text(encoding="utf-8"))
    if actual != expected:
        raise ValueError("Calibration cache provenance mismatch; rerun the calibrate stage")


def _samples_from_audio(lab: Path, model_dir: Path, frames: int) -> list[dict[str, np.ndarray]]:
    # bench.pcm is intentionally imported lazily so provenance-only validation
    # in the quantize stage does not initialize an audio preprocessing session.
    if str(Path(__file__).resolve().parent) not in sys.path:
        sys.path.insert(0, str(Path(__file__).resolve().parent))
    from bench import pcm

    audio = _read_manifest_audio(lab)
    options = ort.SessionOptions()
    options.intra_op_num_threads = 4
    options.inter_op_num_threads = 1
    preprocessor = ort.InferenceSession(
        str((model_dir / "nemo128.onnx").resolve(strict=True)),
        options,
        providers=["CPUExecutionProvider"],
    )
    samples = []
    try:
        for _, path, _ in audio:
            waveform = pcm(path)[None, :]
            features, length = preprocessor.run(
                None,
                {
                    "waveforms": waveform,
                    "waveforms_lens": np.asarray([waveform.shape[1]], dtype=np.int64),
                },
            )
            if features.ndim != 3 or features.shape[:2] != (1, 128):
                raise ValueError(f"Preprocessor returned unexpected feature shape {features.shape}")
            if features.shape[-1] > frames:
                raise ValueError(f"Calibration audio {path.name!r} exceeds the fixed encoder frame count")
            padded = np.zeros((1, 128, frames), dtype=np.float32)
            padded[:, :, : features.shape[-1]] = features
            samples.append(
                {
                    "audio_signal": padded,
                    "length": np.asarray(length, dtype=np.int64),
                }
            )
    finally:
        del preprocessor
        gc.collect()
    return samples


def _qnn_config(source: Path) -> object:
    # get_qnn_qdq_config reads graph/initializer metadata with
    # load_external_data=False; the empty reader is only a config placeholder.
    config = get_qnn_qdq_config(
        source,
        Reader(()),
        activation_type=QuantType.QUInt16,
        weight_type=QuantType.QUInt8,
        per_channel=True,
        calibration_providers=["CPUExecutionProvider"],
    )
    config.use_external_data_format = True
    return config


def _save_cache(cache: Path, tensors: TensorsData, provenance: dict[str, object]) -> None:
    sidecar = _provenance_path(cache)
    cache.parent.mkdir(parents=True, exist_ok=True)
    if cache.exists() or sidecar.exists():
        raise FileExistsError("Calibration cache or provenance already exists; choose a fresh cache path")
    with tempfile.TemporaryDirectory(prefix=".staged-qdq-", dir=cache.parent) as temp_dir:
        temp = Path(temp_dir)
        temp_cache = temp / cache.name
        temp_provenance = temp / sidecar.name
        save_tensors_data(tensors, temp_cache)
        temp_provenance.write_text(json.dumps(provenance, indent=2), encoding="utf-8")
        os.replace(temp_cache, cache)
        os.replace(temp_provenance, sidecar)


def collect_qnn_minmax_ranges(
    source: Path,
    config: object,
    samples: Sequence[dict[str, np.ndarray]],
    scratch_dir: Path,
) -> TensorsData:
    """Collect ranges with one ORT model/session and one fixture batch at a time."""
    with tempfile.TemporaryDirectory(prefix="staged-qdq-calibrator-", dir=scratch_dir) as temp_dir:
        calibrator = create_calibrator(
            source,
            config.op_types_to_quantize,
            augmented_model_path=Path(temp_dir) / "augmented.onnx",
            calibrate_method=config.calibrate_method,
            use_external_data_format=True,
            providers=config.calibration_providers,
        )
        # collect_data computes and merges one range batch per call; do not set
        # CalibMaxIntermediateOutputs, which clears before MinMax reduction in
        # the installed ORT 1.29 implementation.
        for sample in samples:
            calibrator.collect_data(Reader([sample]))
        ranges = calibrator.calibrate_tensors_range
        if ranges is None or not ranges.data:
            raise RuntimeError("Calibration produced no tensor ranges")
        del calibrator
        gc.collect()
        return ranges


def calibrate_stage(source: Path, lab: Path, model_dir: Path, cache: Path) -> dict[str, object]:
    frames = _source_frames(source)
    provenance = _provenance(source, model_dir, lab, frames)
    samples = _samples_from_audio(lab, model_dir, frames)
    config = _qnn_config(source)
    ranges = collect_qnn_minmax_ranges(source, config, samples, lab)
    del samples
    gc.collect()
    _save_cache(cache, ranges, provenance)
    return provenance


def quantize_stage(source: Path, lab: Path, model_dir: Path, cache: Path, output: Path) -> dict[str, object]:
    frames = _source_frames(source)
    expected = _provenance(source, model_dir, lab, frames)
    verify_provenance(cache, expected)
    output.parent.mkdir(parents=True, exist_ok=True)
    output_data = Path(str(output) + ".data")
    if output.exists() or output_data.exists():
        raise FileExistsError(f"Refusing to overwrite QDQ output or its external data: {output}")

    config = _qnn_config(source)
    quantize_static(
        source,
        output,
        calibration_data_reader=None,
        calibrate_method=config.calibrate_method,
        quant_format=config.quant_format,
        activation_type=config.activation_type,
        weight_type=config.weight_type,
        op_types_to_quantize=config.op_types_to_quantize,
        nodes_to_quantize=config.nodes_to_quantize,
        nodes_to_exclude=config.nodes_to_exclude,
        per_channel=config.per_channel,
        reduce_range=config.reduce_range,
        use_external_data_format=True,
        calibration_providers=config.calibration_providers,
        extra_options=config.extra_options,
        calibration_cache_path=cache,
    )
    return expected


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stage", required=True, choices=("calibrate", "quantize"))
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--lab", required=True, type=Path)
    parser.add_argument("--model", required=True, type=Path, help="Installed preprocessing/vocabulary model directory")
    parser.add_argument("--cache", required=True, type=Path)
    parser.add_argument("--output", type=Path, help="Required for --stage quantize")
    parser.add_argument("--qos", choices=("system", "high", "eco"), help="Power QoS of this isolated converter process only")
    args = parser.parse_args()
    if args.qos:
        sys.path.insert(0, str(Path(__file__).resolve().parent))
        from windows_qos import process_qos
        process_qos(os.getpid(), args.qos)

    source = args.source.resolve(strict=True)
    lab = args.lab.resolve(strict=True)
    model_dir = args.model.resolve(strict=True)
    cache = args.cache.resolve()
    if not cache.is_relative_to(lab):
        parser.error("--cache must be inside --lab")
    if args.stage == "calibrate":
        provenance = calibrate_stage(source, lab, model_dir, cache)
    else:
        if args.output is None:
            parser.error("--output is required for --stage quantize")
        output = args.output.resolve()
        if not output.is_relative_to(lab):
            parser.error("--output must be inside --lab")
        provenance = quantize_stage(source, lab, model_dir, cache, output)
    print(json.dumps({"stage": args.stage, "status": "complete", "provenance": provenance}), flush=True)


if __name__ == "__main__":
    main()
