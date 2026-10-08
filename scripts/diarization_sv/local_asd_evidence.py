#!/usr/bin/env python3
"""CPU-only TalkNet active-speaker evidence from caller-supplied arrays.

This wrapper intentionally does not import the official CUDA/training wrapper
(``talkNet``).  It constructs only the public inference modules, loads a
weights-only state dict strictly, and records raw class-1 logits and softmax
probabilities with the caller's per-frame validity mask.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import math
import os
import sys
import time
import wave
from pathlib import Path
from typing import Any

import numpy as np


os.umask(0o077)
sys.dont_write_bytecode = True
FACE_RATE_HZ = 25
AUDIO_RATE_HZ = 16_000
SAMPLES_PER_VIDEO_FRAME = AUDIO_RATE_HZ // FACE_RATE_HZ
ALLOWED_CONTROLS = frozenset({"audio_shift_1s", "freeze_first_valid_face", "silence_audio"})


class InputError(ValueError):
    pass


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def sha256_tree(path: Path) -> str:
    digest = hashlib.sha256()
    files = sorted(child for child in path.rglob("*") if child.is_file() and not child.is_symlink())
    for child in files:
        digest.update(child.relative_to(path).as_posix().encode("utf-8"))
        digest.update(b"\0")
        with child.open("rb") as source:
            while chunk := source.read(1024 * 1024):
                digest.update(chunk)
    return digest.hexdigest()


def parse_positive_float(value: str) -> float:
    try:
        parsed = float(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("must be a finite positive number") from exc
    if not math.isfinite(parsed) or parsed <= 0:
        raise argparse.ArgumentTypeError("must be a finite positive number")
    return parsed


def parse_positive_int(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("must be a positive integer") from exc
    if parsed < 1:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return parsed


def parse_controls(value: str) -> tuple[str, ...]:
    controls = tuple(sorted({part.strip() for part in value.split(",") if part.strip()}))
    unknown = set(controls) - ALLOWED_CONTROLS
    if unknown:
        raise InputError(f"unsupported controls: {', '.join(sorted(unknown))}")
    return controls


def load_faces(path: Path) -> tuple[np.ndarray, np.ndarray]:
    try:
        with np.load(path, allow_pickle=False) as archive:
            if "video" not in archive or "valid" not in archive:
                raise InputError("faces NPZ must contain video and valid arrays")
            video = np.asarray(archive["video"])
            valid = np.asarray(archive["valid"])
    except (OSError, ValueError) as exc:
        if isinstance(exc, InputError):
            raise
        raise InputError(f"cannot read faces NPZ: {path}") from exc
    if video.dtype != np.uint8 or video.ndim != 4 or video.shape[2:] != (112, 112):
        raise InputError("video must be uint8 with shape [actors, frames, 112, 112]")
    if valid.dtype != np.bool_ or valid.shape != video.shape[:2]:
        raise InputError("valid must be bool with shape [actors, frames]")
    if video.shape[0] < 1 or video.shape[1] < 1:
        raise InputError("video must contain at least one actor and frame")
    return video, valid


def load_audio(path: Path) -> np.ndarray:
    try:
        with wave.open(str(path), "rb") as reader:
            if reader.getframerate() != AUDIO_RATE_HZ or reader.getnchannels() != 1:
                raise InputError("audio WAV must be 16 kHz mono")
            if reader.getcomptype() != "NONE":
                raise InputError("audio WAV must be uncompressed PCM")
            width = reader.getsampwidth()
            payload = reader.readframes(reader.getnframes())
    except (OSError, wave.Error) as exc:
        raise InputError(f"cannot read audio WAV: {path}") from exc
    if width == 1:
        samples = (np.frombuffer(payload, dtype=np.uint8).astype(np.float32) - 128.0) / 128.0
    elif width == 2:
        samples = np.frombuffer(payload, dtype="<i2").astype(np.float32) / 32768.0
    elif width == 4:
        samples = np.frombuffer(payload, dtype="<i4").astype(np.float32) / 2147483648.0
    else:
        raise InputError("audio WAV must use 8-, 16-, or 32-bit PCM")
    if not np.isfinite(samples).all():
        raise InputError("audio samples must be finite")
    return samples


def aligned_frame_count(video_frames: int, audio_samples: int) -> int:
    return min(video_frames, audio_samples // SAMPLES_PER_VIDEO_FRAME)


def expected_mfcc_frames(video_frames: int) -> int:
    if video_frames < 1:
        raise InputError("video frame count must be positive")
    # For exactly 640 samples per 25 Hz frame, the official framing produces
    # 4*N-1 10 ms MFCC rows; the TalkNet audio frontend reduces this to N.
    return 4 * video_frames - 1


def freeze_first_valid_face(video: np.ndarray, valid: np.ndarray) -> np.ndarray:
    frozen = video.copy()
    for actor in range(video.shape[0]):
        valid_indices = np.flatnonzero(valid[actor])
        if len(valid_indices) == 0:
            frozen[actor] = 0
        else:
            frozen[actor] = video[actor, valid_indices[0]]
    return frozen


def shift_audio_plus_one_second(audio: np.ndarray) -> np.ndarray:
    shifted = np.zeros_like(audio)
    offset = AUDIO_RATE_HZ
    if len(audio) > offset:
        shifted[offset:] = audio[:-offset]
    return shifted


def apply_controls(video: np.ndarray, valid: np.ndarray, audio: np.ndarray, controls: tuple[str, ...]) -> tuple[np.ndarray, np.ndarray]:
    controlled_video = freeze_first_valid_face(video, valid) if "freeze_first_valid_face" in controls else video.copy()
    controlled_audio = shift_audio_plus_one_second(audio) if "audio_shift_1s" in controls else audio.copy()
    if "silence_audio" in controls:
        controlled_audio.fill(0)
    return controlled_video, controlled_audio


def _add_import_paths(code_dir: Path) -> None:
    code_text = str(code_dir)
    if code_text not in sys.path:
        sys.path.insert(0, code_text)
    dependency_dir = code_dir.parent / "deps"
    if dependency_dir.is_dir() and str(dependency_dir) not in sys.path:
        sys.path.insert(0, str(dependency_dir))


def _load_network(code_dir: Path, weights: Path, threads: int):
    import torch
    import torch.nn as nn

    _add_import_paths(code_dir)
    from loss import lossA, lossAV, lossV
    from model.talkNetModel import talkNetModel

    class CPUInferenceNetwork(nn.Module):
        def __init__(self):
            super().__init__()
            self.model = talkNetModel()
            self.lossAV = lossAV()
            self.lossA = lossA()
            self.lossV = lossV()

    torch.set_num_threads(threads)
    network = CPUInferenceNetwork()
    state = torch.load(weights, map_location=torch.device("cpu"), weights_only=True)
    if not isinstance(state, dict):
        raise InputError("weights must be a state dict")
    try:
        network.load_state_dict(state, strict=True)
    except RuntimeError as exc:
        raise InputError("TalkNet weights do not match the official inference modules") from exc
    network.eval()
    return network, torch


def _mfcc(audio: np.ndarray) -> np.ndarray:
    try:
        from python_speech_features import mfcc
    except ImportError as exc:
        raise InputError("python_speech_features is required for TalkNet MFCC extraction") from exc
    # load_audio keeps samples normalized for safe controls and hashing, while
    # the official TalkNet demo feeds wavfile.read's signed PCM16 values to
    # python_speech_features. Restore that scale before matching its MFCCs.
    pcm16_audio = np.asarray(audio, dtype=np.float64) * 32768.0
    features = np.asarray(
        mfcc(pcm16_audio, samplerate=AUDIO_RATE_HZ, winlen=0.025, winstep=0.01, numcep=13),
        dtype=np.float32,
    )
    if features.ndim != 2 or features.shape[1] != 13 or not np.isfinite(features).all():
        raise InputError("MFCC extraction produced invalid features")
    return features


def infer_scores(
    video: np.ndarray,
    valid: np.ndarray,
    audio: np.ndarray,
    network: Any,
    torch: Any,
    chunk_seconds: float,
) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    frames = aligned_frame_count(video.shape[1], len(audio))
    if frames < 1:
        raise InputError("faces and audio have no aligned 25 Hz frame")
    video = video[:, :frames]
    valid = valid[:, :frames]
    audio = audio[: frames * SAMPLES_PER_VIDEO_FRAME]
    chunk_frames = max(1, int(round(chunk_seconds * FACE_RATE_HZ)))
    logits = np.zeros((video.shape[0], frames), dtype=np.float32)
    probabilities = np.zeros_like(logits)
    with torch.inference_mode():
        for start in range(0, frames, chunk_frames):
            end = min(frames, start + chunk_frames)
            count = end - start
            audio_features = _mfcc(audio[start * SAMPLES_PER_VIDEO_FRAME : end * SAMPLES_PER_VIDEO_FRAME])
            expected = expected_mfcc_frames(count)
            if audio_features.shape[0] < expected:
                raise InputError("MFCC framing produced fewer rows than the aligned video chunk")
            audio_features = audio_features[:expected]
            audio_tensor = torch.from_numpy(audio_features).unsqueeze(0)
            video_tensor = torch.from_numpy(video[:, start:end].astype(np.float32, copy=False))
            audio_embed = network.model.forward_audio_frontend(audio_tensor)
            visual_embed = network.model.forward_visual_frontend(video_tensor)
            if audio_embed.shape[1] != count or visual_embed.shape[1] != count:
                raise InputError("TalkNet frontend frame counts do not match the 25 Hz video chunk")
            audio_embed = audio_embed[:, :count].expand(video.shape[0], -1, -1)
            visual_embed = visual_embed[:, :count]
            audio_embed, visual_embed = network.model.forward_cross_attention(audio_embed, visual_embed)
            fused = network.model.forward_audio_visual_backend(audio_embed, visual_embed)
            class_logits = network.lossAV.FC(fused)
            chunk_logits = class_logits[:, 1].reshape(video.shape[0], count)
            chunk_probabilities = torch.softmax(class_logits, dim=-1)[:, 1].reshape(video.shape[0], count)
            logits[:, start : start + count] = chunk_logits.cpu().numpy()
            probabilities[:, start : start + count] = chunk_probabilities.cpu().numpy()
    if not np.isfinite(logits).all() or not np.isfinite(probabilities).all():
        raise InputError("TalkNet produced non-finite scores")
    return logits, probabilities, valid


def package_versions() -> dict[str, str | None]:
    versions: dict[str, str | None] = {}
    for package in ("numpy", "torch", "python_speech_features"):
        try:
            versions[package] = importlib.metadata.version(package)
        except importlib.metadata.PackageNotFoundError:
            versions[package] = None
    return versions


def write_private(path: Path, content: str) -> None:
    with path.open("x", encoding="utf-8") as output:
        output.write(content)
    path.chmod(0o600)


def run(args: argparse.Namespace) -> dict[str, Any]:
    faces_path = args.faces.expanduser().resolve()
    audio_path = args.audio16kmono.expanduser().resolve()
    code_dir = args.code_dir.expanduser().resolve()
    weights = args.weights.expanduser().resolve()
    output_dir = args.output_dir.expanduser().resolve()
    if output_dir.exists():
        raise InputError(f"refusing to overwrite existing output directory: {output_dir}")
    for path, label in ((faces_path, "faces"), (audio_path, "audio"), (weights, "weights")):
        if not path.is_file():
            raise InputError(f"{label} must be an existing file: {path}")
    if not code_dir.is_dir():
        raise InputError(f"code directory must exist: {code_dir}")
    controls = parse_controls(args.controls)
    output_dir.parent.mkdir(parents=True, exist_ok=True)
    output_dir.mkdir(mode=0o700)
    output_dir.chmod(0o700)
    started = time.monotonic()
    face_hash_before = sha256_file(faces_path)
    audio_hash_before = sha256_file(audio_path)
    weights_hash_before = sha256_file(weights)
    code_hash_before = sha256_tree(code_dir)
    video, valid = load_faces(faces_path)
    audio = load_audio(audio_path)
    controlled_video, controlled_audio = apply_controls(video, valid, audio, controls)
    network, torch = _load_network(code_dir, weights, args.threads)
    logits, probabilities, aligned_valid = infer_scores(
        controlled_video, valid, controlled_audio, network, torch, args.chunk_seconds
    )
    face_hash_after = sha256_file(faces_path)
    audio_hash_after = sha256_file(audio_path)
    weights_hash_after = sha256_file(weights)
    code_hash_after = sha256_tree(code_dir)
    if (face_hash_before, audio_hash_before, weights_hash_before, code_hash_before) != (
        face_hash_after,
        audio_hash_after,
        weights_hash_after,
        code_hash_after,
    ):
        raise InputError("faces, audio, weights, or code changed during inference")
    if not np.isfinite(logits).all() or not np.isfinite(probabilities).all():
        raise InputError("scores must be finite")
    np.savez_compressed(
        output_dir / "scores.npz",
        speaking_logit=logits,
        speaking_probability=probabilities,
        valid=aligned_valid,
        frame_rate_hz=np.asarray(FACE_RATE_HZ, dtype=np.int32),
        duration_seconds=np.asarray(logits.shape[1] / FACE_RATE_HZ, dtype=np.float64),
    )
    (output_dir / "scores.npz").chmod(0o600)
    metadata = {
        "schema_version": 1,
        "code_dir": str(code_dir),
        "code_sha256_before": code_hash_before,
        "code_sha256_after": code_hash_after,
        "weights": str(weights),
        "weights_sha256_before": weights_hash_before,
        "weights_sha256_after": weights_hash_after,
        "faces_npz": str(faces_path),
        "faces_sha256_before": face_hash_before,
        "faces_sha256_after": face_hash_after,
        "audio16kmono_wav": str(audio_path),
        "audio_sha256_before": audio_hash_before,
        "audio_sha256_after": audio_hash_after,
        "controls": list(controls),
        "chunk_seconds": args.chunk_seconds,
        "threads": args.threads,
        "face_rate_hz": FACE_RATE_HZ,
        "face_temporal_provenance": "caller-supplied 25 Hz frames duplicated from original 16 Hz tracking",
        "audio_rate_hz": AUDIO_RATE_HZ,
        "mfcc": {
            "numcep": 13,
            "winlen_seconds": 0.025,
            "winstep_seconds": 0.01,
            "input_scale": "normalized_float_wav_to_pcm16",
        },
        "video_frames_input": int(video.shape[1]),
        "actors": int(video.shape[0]),
        "aligned_frames": int(logits.shape[1]),
        "valid_frames": int(aligned_valid.sum()),
        "scores_shape": list(logits.shape),
        "package_versions": package_versions(),
        "wall_seconds": round(time.monotonic() - started, 3),
        "score_semantics": "class-1 raw logit and softmax probability; evidence only, no truth claim",
    }
    write_private(output_dir / "metadata.json", json.dumps(metadata, sort_keys=True, indent=2) + "\n")
    summary = {
        "actors": int(logits.shape[0]),
        "frames": int(logits.shape[1]),
        "valid_frames": int(aligned_valid.sum()),
        "frame_rate_hz": FACE_RATE_HZ,
        "duration_seconds": round(logits.shape[1] / FACE_RATE_HZ, 6),
        "controls": list(controls),
        "output": "scores.npz",
        "provenance": "metadata.json",
    }
    write_private(output_dir / "summary.json", json.dumps(summary, sort_keys=True, indent=2) + "\n")
    return summary


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Produce local CPU TalkNet active-speaker evidence")
    parser.add_argument("--faces", required=True, type=Path, help="NPZ with uint8 video and bool valid arrays")
    parser.add_argument("--audio16kmono", required=True, type=Path, help="16 kHz mono PCM WAV")
    parser.add_argument("--code-dir", required=True, type=Path, help="immutable TalkNet source directory")
    parser.add_argument("--weights", required=True, type=Path, help="strict weights-only TalkNet state dict")
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--chunk-seconds", type=parse_positive_float, default=4.0)
    parser.add_argument("--threads", type=parse_positive_int, default=2)
    parser.add_argument(
        "--controls",
        default="",
        help="comma-separated controls: audio_shift_1s, freeze_first_valid_face, silence_audio",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        summary = run(args)
    except (InputError, OSError, RuntimeError) as exc:
        parser.error(str(exc))
    print(json.dumps(summary, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
