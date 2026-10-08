#!/usr/bin/env python3
"""Produce anonymous, numeric diarization evidence from an explicit local WAV.

This runner intentionally does not read Sagascript outputs or reference labels.  It
supports the reviewed public LS-EEND runtime and a standalone Silero ONNX VAD.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import importlib.util
import json
import contextlib
import math
import os
import platform
import shutil
import sys
import time
import traceback
from collections import defaultdict
from pathlib import Path
from typing import Any, Iterable

import numpy as np
import soundfile as sf

SAMPLE_RATE = 16_000
SILERO_HOP = 512
SILERO_CONTEXT = 64
RUNNER_VERSION = "1"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _regular_file(path: Path, label: str) -> Path:
    if not path.is_file():
        raise ValueError(f"{label} must be an existing regular file")
    return path


def _source_tree_sha256(root: Path) -> str:
    """Hash reviewed Python runtime sources, excluding weights and generated files."""
    digest = hashlib.sha256()
    excluded = {".git", "__pycache__", "model_checkpoints", "logs"}
    files = [
        path
        for path in root.rglob("*.py")
        if path.is_file() and not any(part in excluded for part in path.relative_to(root).parts)
    ]
    for path in sorted(files, key=lambda item: item.relative_to(root).as_posix()):
        relative = path.relative_to(root).as_posix().encode("utf-8")
        digest.update(len(relative).to_bytes(8, "big"))
        digest.update(relative)
        digest.update(bytes.fromhex(sha256_file(path)))
    return digest.hexdigest()


def _aligned_window_samples(window_seconds: float) -> int:
    if not math.isfinite(window_seconds) or window_seconds <= 0:
        raise ValueError("window seconds must be positive")
    requested = max(1, int(round(window_seconds * SAMPLE_RATE)))
    return max(SILERO_HOP, (requested // SILERO_HOP) * SILERO_HOP)


def _silero_frame_count(num_samples: int, window_samples: int) -> int:
    if num_samples < 0 or window_samples < SILERO_HOP or window_samples % SILERO_HOP:
        raise ValueError("window samples must be a positive multiple of the Silero hop")
    return sum((size + SILERO_HOP - 1) // SILERO_HOP for size in (
        min(window_samples, num_samples - start)
        for start in range(0, num_samples, window_samples)
    ))


def _frame_timestamps(frame_count: int, hop_seconds: float, duration: float) -> list[float]:
    return [min(float(index * hop_seconds), duration) for index in range(frame_count)]


def validate_wav(path: Path) -> tuple[np.ndarray, float]:
    path = _regular_file(path, "input WAV")
    try:
        info = sf.info(path)
    except Exception as exc:  # pragma: no cover - backend-specific error text
        raise ValueError("input is not a readable WAV") from exc
    if info.format != "WAV" or info.samplerate != SAMPLE_RATE or info.channels != 1:
        raise ValueError("input WAV must be mono 16 kHz")
    audio, rate = sf.read(path, dtype="float32", always_2d=False)
    audio = np.asarray(audio, dtype=np.float32)
    if rate != SAMPLE_RATE or audio.ndim != 1 or audio.size == 0:
        raise ValueError("input WAV must contain non-empty mono 16 kHz audio")
    if not np.isfinite(audio).all():
        raise ValueError("input WAV contains non-finite samples")
    return audio, float(audio.size / SAMPLE_RATE)


def merge_intervals(
    intervals: Iterable[tuple[float, float]], duration_seconds: float
) -> list[tuple[float, float]]:
    """Clip, sort, and merge overlapping or touching numeric intervals."""
    if not math.isfinite(duration_seconds) or duration_seconds < 0:
        raise ValueError("duration must be finite and non-negative")
    clipped: list[tuple[float, float]] = []
    for start, end in intervals:
        start = min(max(float(start), 0.0), duration_seconds)
        end = min(max(float(end), 0.0), duration_seconds)
        if end > start:
            clipped.append((start, end))
    clipped.sort()
    merged: list[tuple[float, float]] = []
    for start, end in clipped:
        if merged and start <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(merged[-1][1], end))
        else:
            merged.append((start, end))
    return merged


def probabilities_to_intervals(
    probabilities: Iterable[float],
    hop_seconds: float,
    duration_seconds: float,
    threshold: float,
    neg_threshold: float | None = None,
) -> list[tuple[float, float]]:
    """Convert frame probabilities to hysteretic, duration-clipped intervals."""
    if not math.isfinite(hop_seconds) or hop_seconds <= 0:
        raise ValueError("hop must be finite and positive")
    if not 0 < threshold <= 1:
        raise ValueError("threshold must be in (0, 1]")
    if neg_threshold is None:
        neg_threshold = threshold - 0.15
    if not 0 <= neg_threshold <= 1:
        raise ValueError("negative threshold must be in [0, 1]")
    active = False
    start = 0.0
    intervals: list[tuple[float, float]] = []
    for index, probability in enumerate(probabilities):
        probability = float(probability)
        if not math.isfinite(probability):
            raise ValueError("probabilities must be finite")
        frame_start = index * hop_seconds
        if not active and probability >= threshold:
            active = True
            start = frame_start
        elif active and probability < neg_threshold:
            active = False
            intervals.append((start, frame_start))
    if active:
        intervals.append((start, min(duration_seconds, (index + 1) * hop_seconds)))
    return merge_intervals(intervals, duration_seconds)


def _create_output_dir(path: Path) -> None:
    if path.exists():
        raise FileExistsError("output directory already exists")
    old_umask = os.umask(0o077)
    try:
        path.mkdir(parents=True, mode=0o700)
    finally:
        os.umask(old_umask)
    path.chmod(0o700)


def _package_versions(names: Iterable[str]) -> dict[str, str | None]:
    versions: dict[str, str | None] = {}
    for name in names:
        try:
            versions[name] = importlib.metadata.version(name)
        except importlib.metadata.PackageNotFoundError:
            versions[name] = None
    return versions


def _public_config(path: Path) -> dict[str, Any]:
    """Parse the reviewed public YAML using SafeLoader and scalar !ref only."""
    if importlib.util.find_spec("hyperpyyaml") is not None:
        raise RuntimeError("hyperpyyaml is unsupported; use the reviewed safe YAML runtime")
    import yaml

    class PublicLoader(yaml.SafeLoader):
        pass

    PublicLoader.add_constructor("!ref", lambda loader, node: loader.construct_scalar(node))
    with path.open("r", encoding="utf-8") as handle:
        config = yaml.load(handle, Loader=PublicLoader)
    if not isinstance(config, dict) or not isinstance(config.get("data"), dict):
        raise ValueError("LS-EEND config has an invalid public schema")
    data = config["data"]
    if not isinstance(data.get("feat"), dict) or not isinstance(data["feat"].get("sample_rate"), int):
        raise ValueError("LS-EEND config lacks a sample-rate declaration")
    if int(data["feat"]["sample_rate"]) != 8000:
        raise ValueError("LS-EEND config must use the reviewed 8 kHz feature rate")
    if not isinstance(data.get("max_speakers"), int) or data["max_speakers"] < 1:
        raise ValueError("LS-EEND config has an invalid max speaker count")
    return config


class SafeLSEENDInferenceEngine:  # constructed lazily to keep Silero usable without torch
    def __new__(cls, checkpoint: Path, config: Path):
        root = checkpoint.parent.parent
        if str(root) not in sys.path:
            sys.path.insert(0, str(root))
        runtime = __import__("torch_inference.ls_eend_runtime", fromlist=["LSEENDInferenceEngine"])
        model_module = __import__(
            "nnet.model.onl_conformer_retention_enc_1dcnn_tfm_retention_enc_linear_non_autoreg_pos_enc_l2norm_emb_loss_mask",
            fromlist=["OnlineConformerRetentionDADiarization"],
        )
        torch = __import__("torch")
        base = runtime.LSEENDInferenceEngine
        model_class = model_module.OnlineConformerRetentionDADiarization
        safe_checkpoint = checkpoint

        class _Safe(base):
            def _load_model(self):  # type: ignore[no-untyped-def]
                model = model_class(
                    n_speakers=self.config["data"]["num_speakers"],
                    in_size=(2 * self.config["data"]["context_recp"] + 1) * self.config["data"]["feat"]["n_mels"],
                    **self.config["model"]["params"],
                )
                with torch.serialization.safe_globals([defaultdict, float]):
                    state_dict = torch.load(safe_checkpoint, map_location="cpu", weights_only=True)
                if isinstance(state_dict, dict) and "state_dict" in state_dict:
                    state_dict = state_dict["state_dict"]
                cleaned = {
                    key[len("model.") :] if key.startswith("model.") else key: value
                    for key, value in state_dict.items()
                }
                model.load_state_dict(cleaned)
                return model

        # The base runtime has been reviewed: with hyperpyyaml unavailable its only
        # loader is its SafeLoader with scalar !ref.  We pre-parse the same file above.
        return _Safe(checkpoint_path=checkpoint, config_path=config, device="cpu", actual_num_speakers=None)


def _silero_probabilities(audio: np.ndarray, model_path: Path, window_samples: int) -> tuple[np.ndarray, float]:
    if window_samples < SILERO_HOP or window_samples % SILERO_HOP:
        raise ValueError("window samples must be a positive multiple of the Silero hop")
    os.environ.setdefault("ORT_DISABLE_TELEMETRY", "1")
    import onnxruntime as ort

    session = ort.InferenceSession(str(model_path), providers=["CPUExecutionProvider"])
    probabilities: list[float] = []
    for window_start in range(0, len(audio), window_samples):
        window = audio[window_start : window_start + window_samples]
        state = np.zeros((2, 1, 128), dtype=np.float32)
        for frame_start in range(0, len(window), SILERO_HOP):
            left = max(0, frame_start - SILERO_CONTEXT)
            frame = window[left : min(len(window), frame_start + SILERO_HOP)]
            left_pad = SILERO_CONTEXT - (frame_start - left)
            right_pad = SILERO_CONTEXT + SILERO_HOP - left_pad - len(frame)
            if left_pad or right_pad:
                frame = np.pad(frame, (left_pad, max(0, right_pad)))
            frame = np.asarray(frame[: SILERO_CONTEXT + SILERO_HOP], dtype=np.float32)[None, :]
            output, state = session.run(["output", "stateN"], {"input": frame, "state": state, "sr": np.array([SAMPLE_RATE], dtype=np.int64)})
            probabilities.append(float(np.asarray(output).reshape(-1)[0]))
    return np.asarray(probabilities, dtype=np.float32), SILERO_HOP / SAMPLE_RATE


def _write_scores(path: Path, probabilities: np.ndarray, frame_hz: float, duration: float) -> str:
    scores = np.asarray(probabilities, dtype=np.float32)
    if scores.ndim != 2 or not np.isfinite(scores).all():
        raise ValueError("LS-EEND scores must be a finite 2-D numeric array")
    if not math.isfinite(frame_hz) or frame_hz <= 0 or not math.isfinite(duration) or duration < 0:
        raise ValueError("LS-EEND score metadata is invalid")
    np.savez(
        path,
        probabilities=scores,
        frame_hz=np.array(frame_hz, dtype=np.float64),
        duration_seconds=np.array(duration, dtype=np.float64),
    )
    return sha256_file(path)


def _track_payload(probabilities: np.ndarray, frame_hz: float, duration: float, threshold: float) -> list[dict[str, Any]]:
    tracks: list[dict[str, Any]] = []
    for track_index in range(probabilities.shape[1]):
        values = probabilities[:, track_index]
        intervals = probabilities_to_intervals(values, 1.0 / frame_hz, duration, threshold)
        segments = []
        for start, end in intervals:
            first = max(0, int(math.floor(start * frame_hz)))
            last = min(len(values), max(first + 1, int(math.ceil(end * frame_hz))))
            segments.append({"start_seconds": start, "end_seconds": end, "mean_probability": float(np.mean(values[first:last]))})
        tracks.append({"speaker_track": track_index, "segments": segments})
    return tracks


@contextlib.contextmanager
def _isolated_matplotlib_cache(output_dir: Path):
    cache_dir = output_dir / ".mplconfig"
    cache_dir.mkdir(mode=0o700)
    previous = os.environ.get("MPLCONFIGDIR")
    os.environ["MPLCONFIGDIR"] = str(cache_dir)
    try:
        yield
    finally:
        if previous is None:
            os.environ.pop("MPLCONFIGDIR", None)
        else:
            os.environ["MPLCONFIGDIR"] = previous
        shutil.rmtree(cache_dir, ignore_errors=True)


def run(args: argparse.Namespace, output_dir: Path) -> dict[str, Any]:
    started = time.monotonic()
    input_path = _regular_file(Path(args.input_wav), "input WAV")
    model_path = _regular_file(Path(args.model), "model")
    config_path = Path(args.config) if args.config else None
    if args.backend == "lseend":
        if config_path is None:
            raise ValueError("--config is required for lseend")
        _regular_file(config_path, "config")
        _public_config(config_path)
    elif config_path is not None:
        raise ValueError("--config is only valid for lseend")
    if not args.offline:
        raise ValueError("--offline is required")
    if args.device != "cpu":
        raise ValueError("only CPU execution is supported")
    if args.window_samples is not None:
        if args.window_samples < SILERO_HOP or args.window_samples % SILERO_HOP:
            raise ValueError("window samples must be a positive multiple of the Silero hop")
        window_samples = args.window_samples
    else:
        window_samples = _aligned_window_samples(args.window_seconds)
    audio, duration = validate_wav(input_path)
    before_hashes = {"input": sha256_file(input_path), "model": sha256_file(model_path)}
    if config_path is not None:
        before_hashes["config"] = sha256_file(config_path)
    runtime_source_before = None
    if args.backend == "lseend":
        runtime_source_before = _source_tree_sha256(model_path.parent.parent)
    if args.backend == "silero":
        probabilities, hop_seconds = _silero_probabilities(audio, model_path, window_samples)
        speech = probabilities_to_intervals(probabilities, hop_seconds, duration, args.threshold)
        result: dict[str, Any] = {
            "backend": "silero",
            "threshold": args.threshold,
            "sample_rate": SAMPLE_RATE,
            "duration_seconds": duration,
            "frame_hop_seconds": hop_seconds,
            "window_samples": window_samples,
            "frame_timestamps_seconds": _frame_timestamps(len(probabilities), hop_seconds, duration),
            "probabilities": [float(value) for value in probabilities],
            "speech_intervals": [{"start_seconds": s, "end_seconds": e} for s, e in speech],
            "tracks": [],
        }
        packages = ["numpy", "soundfile", "onnxruntime"]
    else:
        with _isolated_matplotlib_cache(output_dir):
            engine = SafeLSEENDInferenceEngine(model_path, config_path)  # type: ignore[arg-type]
            inferred = engine.infer_audio(audio, SAMPLE_RATE)
        track_probs = np.asarray(inferred.probabilities, dtype=np.float32)
        result = {
            "backend": "lseend",
            "threshold": args.threshold,
            "sample_rate": SAMPLE_RATE,
            "duration_seconds": duration,
            "frame_hop_seconds": 1.0 / float(inferred.frame_hz),
            "frame_hz": float(inferred.frame_hz),
            "frame_count": int(track_probs.shape[0]),
            "frame_timestamps_seconds": _frame_timestamps(int(track_probs.shape[0]), 1.0 / float(inferred.frame_hz), duration),
            "tracks": _track_payload(track_probs, float(inferred.frame_hz), duration, args.threshold),
        }
        scores_path = output_dir / "scores.npz"
        result["scores_file"] = "scores.npz"
        result["scores_sha256"] = _write_scores(scores_path, track_probs, float(inferred.frame_hz), duration)
        packages = ["numpy", "soundfile", "torch", "librosa", "pyyaml"]
    after_hashes = {"input": sha256_file(input_path), "model": sha256_file(model_path)}
    if config_path is not None:
        after_hashes["config"] = sha256_file(config_path)
    runtime_source_after = None
    if args.backend == "lseend":
        runtime_source_after = _source_tree_sha256(model_path.parent.parent)
    if before_hashes != after_hashes or runtime_source_before != runtime_source_after:
        raise RuntimeError("input, model, config, or runtime source changed during inference")
    result["hashes_before"] = before_hashes
    result["hashes_after"] = after_hashes
    result["runtime_source_sha256_before"] = runtime_source_before
    result["runtime_source_sha256_after"] = runtime_source_after
    result["runner_version"] = RUNNER_VERSION
    result["elapsed_seconds"] = time.monotonic() - started
    result["package_versions"] = _package_versions(packages)
    result["python_version"] = platform.python_version()
    return result


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Generate anonymous numeric local diarization evidence")
    parser.add_argument("--input-wav", required=True)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--config")
    parser.add_argument("--backend", choices=("silero", "lseend"), required=True)
    parser.add_argument("--threshold", type=float, default=0.5)
    window_group = parser.add_mutually_exclusive_group()
    window_group.add_argument("--window-seconds", type=float, default=60.0)
    window_group.add_argument("--window-samples", type=int)
    parser.add_argument("--device", choices=("cpu",), default="cpu")
    parser.add_argument("--offline", action="store_true", help="required; forbids network-backed execution")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    output_dir = Path(args.output_dir)
    try:
        _create_output_dir(output_dir)
        result = run(args, output_dir)
        metadata = {key: result[key] for key in ("backend", "runner_version", "hashes_before", "hashes_after", "runtime_source_sha256_before", "runtime_source_sha256_after", "python_version", "package_versions", "elapsed_seconds")}
        for key in ("scores_file", "scores_sha256"):
            if key in result:
                metadata[key] = result[key]
        (output_dir / "metadata.json").write_text(json.dumps(metadata, sort_keys=True, indent=2) + "\n", encoding="utf-8")
        (output_dir / "result.json").write_text(json.dumps(result, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    except Exception as exc:
        # Explicit task paths and public config diagnostics are useful; unexpected failures get a traceback.
        print(f"error: {type(exc).__name__}: {exc}", file=sys.stderr)
        if not isinstance(exc, (ValueError, FileExistsError)):
            traceback.print_exc()
        return 2
    print(json.dumps({"backend": result["backend"], "duration_seconds": result["duration_seconds"], "track_count": len(result["tracks"]), "elapsed_seconds": result["elapsed_seconds"]}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
