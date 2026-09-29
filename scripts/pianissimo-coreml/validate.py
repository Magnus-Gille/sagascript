#!/usr/bin/env python3
"""Numerical, greedy-decode, long-form, and latency validation for a model set."""

from __future__ import annotations

import argparse
import importlib.util
import json
import sys
import time
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
from compat import install_lzma_backport  # noqa: E402

install_lzma_backport()

import coremltools as ct  # noqa: E402
import jiwer  # noqa: E402
import numpy as np  # noqa: E402
import soundfile as sf  # noqa: E402
import torch  # noqa: E402
import nemo.collections.asr as nemo_asr  # noqa: E402

from components import patch_local_attention, trace_inputs  # noqa: E402


BLANK_ID = 8192
FRAME_S = 0.08
BENCH_ROOT = Path("/Users/magnus/.cache/sagascript-bench/bench")
ALIGNMENT_PATH = BENCH_ROOT / ".venv/lib/python3.12/site-packages/parakeet_mlx/alignment.py"
NORM_PATH = BENCH_ROOT / "norm.py"


def load_module(path: Path, name: str):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"Cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def rel_l2(actual: np.ndarray, reference: np.ndarray) -> float:
    delta = np.asarray(actual, dtype=np.float32) - np.asarray(reference, dtype=np.float32)
    denom = max(float(np.linalg.norm(np.asarray(reference, dtype=np.float32))), 1e-12)
    return float(np.linalg.norm(delta) / denom)


def max_abs(actual: np.ndarray, reference: np.ndarray) -> float:
    return float(np.max(np.abs(np.asarray(actual, dtype=np.float32) - np.asarray(reference, dtype=np.float32))))


def compute_units(name: str) -> ct.ComputeUnit:
    normalized = name.strip().upper()
    values = {
        "CPU_AND_NE": ct.ComputeUnit.CPU_AND_NE,
        "CPU_AND_NEURAL_ENGINE": ct.ComputeUnit.CPU_AND_NE,
        "ALL": ct.ComputeUnit.ALL,
        "CPU_ONLY": ct.ComputeUnit.CPU_ONLY,
    }
    if normalized not in values:
        raise ValueError(f"Unknown compute units {name}")
    return values[normalized]


def load_bundle(
    directory: Path,
    units: ct.ComputeUnit,
    encoder_path: Path | None = None,
) -> dict[str, ct.models.MLModel]:
    paths = {
        name: directory / f"{name}.mlpackage"
        for name in ("Preprocessor", "Encoder", "Decoder", "JointDecisionv3")
    }
    if encoder_path is not None:
        paths["Encoder"] = encoder_path
    return {
        name: ct.models.MLModel(str(path), compute_units=units)
        for name, path in paths.items()
    }


def restore(nemo_path: Path):
    model = nemo_asr.models.EncDecRNNTBPEModel.restore_from(str(nemo_path), map_location="cpu")
    model.eval()
    return model


def read_window(audio_path: Path, start: int, samples: int) -> tuple[np.ndarray, int]:
    data, rate = sf.read(str(audio_path), start=start, stop=start + samples, dtype="float32")
    if rate != 16_000:
        raise ValueError(f"Expected 16 kHz audio, got {rate}")
    if data.ndim > 1:
        data = data[:, 0]
    valid = int(data.size)
    if valid < samples:
        data = np.pad(data, (0, samples - valid))
    return data.astype(np.float16, copy=False).reshape(1, samples), valid


def reference_features(model, audio: np.ndarray, valid_samples: int):
    waveform = torch.from_numpy(audio.astype(np.float32, copy=False))
    length = torch.tensor([valid_samples], dtype=torch.long)
    with torch.inference_mode():
        mel, mel_length = model.preprocessor(input_signal=waveform, length=length)
        encoded, encoded_length = model.encoder(audio_signal=mel, length=mel_length)
    return mel.numpy(), int(mel_length[0]), encoded.numpy(), int(encoded_length[0])


def nemo_greedy_tokens(model, encoded: np.ndarray, encoded_length: int) -> list[int]:
    with torch.inference_mode():
        output = model.decoding.rnnt_decoder_predictions_tensor(
            torch.from_numpy(encoded),
            torch.tensor([encoded_length], dtype=torch.long),
            return_hypotheses=True,
        )
    hypothesis = output[0] if isinstance(output, list) else output
    if isinstance(hypothesis, list):
        hypothesis = hypothesis[0]
    sequence = getattr(hypothesis, "y_sequence", hypothesis)
    if isinstance(sequence, torch.Tensor):
        return [int(token) for token in sequence.detach().cpu().tolist()]
    return [int(token) for token in sequence]


def coreml_greedy_emissions(
    bundle: dict[str, ct.models.MLModel], encoded: np.ndarray, encoded_length: int
) -> list[tuple[int, int, int]]:
    decoder = bundle["Decoder"]
    joint = bundle["JointDecisionv3"]
    h = np.zeros((2, 1, 640), dtype=np.float16)
    c = np.zeros((2, 1, 640), dtype=np.float16)
    last_token = BLANK_ID
    time_index = 0
    tokens: list[tuple[int, int, int]] = []
    symbols_at_time = 0
    while time_index < encoded_length:
        decoder_output = decoder.predict(
            {
                "targets": np.array([[last_token]], dtype=np.int32),
                "target_length": np.array([1], dtype=np.int32),
                "h_in": h,
                "c_in": c,
            }
        )
        decision = joint.predict(
            {
                "encoder_step": encoded[:, :, time_index : time_index + 1].astype(np.float16),
                "decoder_step": np.asarray(decoder_output["decoder"], dtype=np.float16),
            }
        )
        token = int(np.asarray(decision["token_id"]).reshape(-1)[0])
        duration = int(np.asarray(decision["duration"]).reshape(-1)[0])
        if token == BLANK_ID:
            time_index += max(duration, 1)
            symbols_at_time = 0
            continue

        tokens.append((token, time_index, duration))
        h = np.asarray(decoder_output["h_out"], dtype=np.float16)
        c = np.asarray(decoder_output["c_out"], dtype=np.float16)
        last_token = token
        symbols_at_time += 1
        time_index += duration
        if symbols_at_time >= 10:
            time_index += 1
            symbols_at_time = 0
    return tokens


def coreml_greedy_tokens(
    bundle: dict[str, ct.models.MLModel], encoded: np.ndarray, encoded_length: int
) -> list[int]:
    return [token for token, _, _ in coreml_greedy_emissions(bundle, encoded, encoded_length)]


def numerical(args: argparse.Namespace) -> dict[str, Any]:
    samples, mel_frames, encoder_frames = trace_inputs(args.window_s)
    audio_path = args.audio.resolve()
    model = restore(args.nemo.resolve())
    starts = [0, samples, 2 * samples]
    results: list[dict[str, Any]] = []
    for start in starts:
        audio, valid = read_window(audio_path, start, samples)
        mel_ref, mel_length, enc_ref, enc_length = reference_features(model, audio, valid)
        row: dict[str, Any] = {"start_s": start / 16_000, "valid_s": valid / 16_000}
        for units_name in args.compute_units:
            try:
                bundle = load_bundle(args.model_dir.resolve(), compute_units(units_name))
                pre = bundle["Preprocessor"].predict(
                    {"audio_signal": audio, "audio_length": np.array([valid], dtype=np.int32)}
                )
                core_mel = np.asarray(pre["mel"], dtype=np.float32)
                row[f"{units_name}.mel_rel_l2"] = rel_l2(core_mel, mel_ref)
                row[f"{units_name}.mel_max_abs"] = max_abs(core_mel, mel_ref)
                enc = bundle["Encoder"].predict(
                    {
                        "mel": mel_ref.astype(np.float16),
                        "mel_length": np.array([mel_length], dtype=np.int32),
                    }
                )
                core_enc = np.asarray(enc["encoder"], dtype=np.float32)
                row[f"{units_name}.encoder_rel_l2"] = rel_l2(core_enc, enc_ref)
                row[f"{units_name}.encoder_max_abs"] = max_abs(core_enc, enc_ref)
                row[f"{units_name}.tokens"] = coreml_greedy_tokens(bundle, core_enc.astype(np.float16), enc_length)
                if units_name == args.compute_units[0]:
                    row["nemo.tokens"] = nemo_greedy_tokens(model, enc_ref, enc_length)
                    row[f"{units_name}.tokens_identical"] = row[f"{units_name}.tokens"] == row["nemo.tokens"]
                if args.fp32_encoder is not None:
                    fp32 = ct.models.MLModel(
                        str(args.fp32_encoder.resolve()), compute_units=compute_units(units_name)
                    )
                    fp32_out = fp32.predict(
                        {
                            "mel": mel_ref.astype(np.float16),
                            "mel_length": np.array([mel_length], dtype=np.int32),
                        }
                    )
                    fp32_enc = np.asarray(fp32_out["encoder"], dtype=np.float32)
                    row[f"{units_name}.fp32_encoder_rel_l2"] = rel_l2(fp32_enc, enc_ref)
                    row[f"{units_name}.fp32_vs_int8_rel_l2"] = rel_l2(fp32_enc, core_enc)
            except Exception as exc:
                row[f"{units_name}.error"] = f"{type(exc).__name__}: {exc}"
        results.append(row)

    summary: dict[str, Any] = {"windows": results, "window_s": args.window_s}
    for units_name in args.compute_units:
        available = [row for row in results if f"{units_name}.mel_rel_l2" in row]
        if not available:
            summary[f"{units_name}.error"] = next(
                row[f"{units_name}.error"] for row in results if f"{units_name}.error" in row
            )
            continue
        for key in ("mel_rel_l2", "mel_max_abs", "encoder_rel_l2", "encoder_max_abs"):
            values = [float(row[f"{units_name}.{key}"]) for row in available]
            summary[f"{units_name}.{key}.mean"] = float(np.mean(values))
            summary[f"{units_name}.{key}.max"] = float(np.max(values))
        if args.fp32_encoder is not None:
            for key in ("fp32_encoder_rel_l2", "fp32_vs_int8_rel_l2"):
                values = [float(row[f"{units_name}.{key}"]) for row in available]
                summary[f"{units_name}.{key}.mean"] = float(np.mean(values))
                summary[f"{units_name}.{key}.max"] = float(np.max(values))
        identities = [bool(row.get(f"{units_name}.tokens_identical", False)) for row in available]
        summary[f"{units_name}.token_identity"] = f"{sum(identities)}/{len(identities)}"
    return summary


def aligned_tokens(bundle, audio: np.ndarray, valid: int, offset_s: float, vocabulary: dict[str, str]):
    pre = bundle["Preprocessor"].predict(
        {"audio_signal": audio, "audio_length": np.array([valid], dtype=np.int32)}
    )
    mel = np.asarray(pre["mel"], dtype=np.float16)
    mel_length = int(np.asarray(pre["mel_length"]).reshape(-1)[0])
    enc = bundle["Encoder"].predict(
        {"mel": mel, "mel_length": np.array([mel_length], dtype=np.int32)}
    )
    encoded = np.asarray(enc["encoder"], dtype=np.float16)
    encoded_length = int(np.asarray(enc["encoder_length"]).reshape(-1)[0])
    emissions = coreml_greedy_emissions(bundle, encoded, encoded_length)
    alignment = load_module(ALIGNMENT_PATH, "pianissimo_alignment")
    result = []
    for token_id, frame, duration in emissions:
        piece = vocabulary[str(token_id)].replace("▁", " ")
        result.append(
            alignment.AlignedToken(
                id=token_id,
                text=piece,
                start=offset_s + frame * FRAME_S,
                duration=duration * FRAME_S,
            )
        )
    return result


def longform_one(model_dir: Path, audio_path: Path, reference_path: Path, window_s: int, overlap_s: float, units_name: str) -> dict[str, Any]:
    samples = window_s * 16_000
    audio, rate = sf.read(str(audio_path), dtype="float32")
    if rate != 16_000:
        raise ValueError(f"Expected 16 kHz audio, got {rate}")
    if audio.ndim > 1:
        audio = audio[:, 0]
    bundle = load_bundle(model_dir, compute_units(units_name))
    vocabulary = json.loads((model_dir / "parakeet_vocab.json").read_text())
    step = int(round((window_s - overlap_s) * 16_000))
    if len(audio) <= samples:
        starts = [0]
    else:
        starts = []
        for start in range(0, len(audio), step):
            starts.append(start)
            if start + samples >= len(audio):
                break
    merged = []
    for start in starts:
        chunk = audio[start : start + samples]
        valid = int(chunk.size)
        if valid < samples:
            chunk = np.pad(chunk, (0, samples - valid))
        tokens = aligned_tokens(
            bundle,
            chunk.astype(np.float16, copy=False).reshape(1, samples),
            valid,
            start / 16_000,
            vocabulary,
        )
        if not merged:
            merged = tokens
        else:
            alignment = load_module(ALIGNMENT_PATH, "pianissimo_alignment_merge")
            try:
                merged = alignment.merge_longest_contiguous(
                    merged, tokens, overlap_duration=overlap_s
                )
            except RuntimeError:
                merged = alignment.merge_longest_common_subsequence(
                    merged, tokens, overlap_duration=overlap_s
                )
    hypothesis = "".join(token.text for token in merged).strip()
    norm = load_module(NORM_PATH, "bench_norm")
    reference = reference_path.read_text()
    details = jiwer.process_words(norm.norm(reference), norm.norm(hypothesis))
    return {
        "model_dir": str(model_dir),
        "window_s": window_s,
        "overlap_s": overlap_s,
        "compute_units": units_name,
        "windows": len(starts),
        "reference_words": len(norm.norm(reference).split()),
        "hypothesis_words": len(norm.norm(hypothesis).split()),
        "wer": float(details.wer),
        "text": hypothesis,
    }


def speed(args: argparse.Namespace) -> dict[str, Any]:
    samples, _, _ = trace_inputs(args.window_s)
    audio, valid = read_window(args.audio.resolve(), 0, samples)
    output: dict[str, Any] = {}
    for units_name in args.compute_units:
        try:
            units = compute_units(units_name)
            t0 = time.perf_counter()
            bundle = load_bundle(args.model_dir.resolve(), units)
            first_load = time.perf_counter() - t0
            # The first prediction includes CoreML's lazy device preparation on
            # macOS; report it separately from warmed predictions.
            first = time.perf_counter()
            pre = bundle["Preprocessor"].predict(
                {"audio_signal": audio, "audio_length": np.array([valid], dtype=np.int32)}
            )
            mel = np.asarray(pre["mel"], dtype=np.float16)
            ml = np.asarray(pre["mel_length"], dtype=np.int32)
            bundle["Encoder"].predict({"mel": mel, "mel_length": ml})
            first_predict = time.perf_counter() - first
            for _ in range(2):
                bundle["Encoder"].predict({"mel": mel, "mel_length": ml})
            enc_times = []
            full_times = []
            for _ in range(args.runs):
                t1 = time.perf_counter()
                enc = bundle["Encoder"].predict({"mel": mel, "mel_length": ml})
                enc_times.append((time.perf_counter() - t1) * 1000)
                t1 = time.perf_counter()
                pre = bundle["Preprocessor"].predict(
                    {"audio_signal": audio, "audio_length": np.array([valid], dtype=np.int32)}
                )
                enc = bundle["Encoder"].predict(
                    {"mel": np.asarray(pre["mel"], dtype=np.float16), "mel_length": np.asarray(pre["mel_length"], dtype=np.int32)}
                )
                coreml_greedy_tokens(bundle, np.asarray(enc["encoder"], dtype=np.float16), int(np.asarray(enc["encoder_length"]).reshape(-1)[0]))
                full_times.append((time.perf_counter() - t1) * 1000)
            output[units_name] = {
                "first_load_s": first_load,
                "first_window_predict_s": first_predict,
                "encoder_ms_mean": float(np.mean(enc_times)),
                "encoder_ms_p95": float(np.percentile(enc_times, 95)),
                "full_pipeline_ms_mean": float(np.mean(full_times)),
                "full_pipeline_ms_p95": float(np.percentile(full_times, 95)),
            }
        except Exception as exc:
            output[units_name] = {"error": f"{type(exc).__name__}: {exc}"}
    return output


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model-dir", type=Path, required=True)
    parser.add_argument("--nemo", type=Path, required=True)
    parser.add_argument("--audio", type=Path, required=True)
    parser.add_argument("--window-s", type=int, choices=(15, 30), required=True)
    parser.add_argument("--compute-units", nargs="+", default=["CPU_AND_NE", "ALL"])
    parser.add_argument("--report", type=Path)
    parser.add_argument("--fp32-encoder", type=Path)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--longform", action="store_true")
    parser.add_argument("--speed", action="store_true")
    parser.add_argument("--reference-5min", type=Path, default=Path("/Users/magnus/.cache/sagascript-bench/longform/fleurs-sv-distinct-5min.txt"))
    parser.add_argument("--audio-5min", type=Path, default=Path("/Users/magnus/.cache/sagascript-bench/longform/fleurs-sv-distinct-5min.wav"))
    parser.add_argument("--reference-15min", type=Path, default=Path("/Users/magnus/.cache/sagascript-bench/longform/fleurs-sv-distinct-15min.txt"))
    parser.add_argument("--audio-15min", type=Path, default=Path("/Users/magnus/.cache/sagascript-bench/longform/fleurs-sv-distinct-15min.wav"))
    args = parser.parse_args()
    report: dict[str, Any] = {"numerical": numerical(args)}
    if args.speed:
        report["speed"] = speed(args)
    if args.longform:
        if args.window_s == 15:
            overlap_s = 2
        else:
            overlap_s = 6
        report["longform"] = [
            longform_one(args.model_dir.resolve(), args.audio_5min.resolve(), args.reference_5min.resolve(), args.window_s, overlap_s, args.compute_units[0]),
            longform_one(args.model_dir.resolve(), args.audio_15min.resolve(), args.reference_15min.resolve(), args.window_s, overlap_s, args.compute_units[0]),
        ]
    text = json.dumps(report, indent=2, ensure_ascii=False)
    print(text)
    if args.report:
        args.report.write_text(text + "\n")


if __name__ == "__main__":
    main()
