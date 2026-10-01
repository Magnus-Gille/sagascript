#!/usr/bin/env python3
"""Numerical, greedy-decode, and latency validation of a Pianissimo Core ML model set.

Compares mel and encoder outputs with NeMo PyTorch fp32, and compares greedy TDT
token sequences. The Core ML decode loop is a port of FluidAudio's ``TdtDecoderV3``
main loop (blank handling, duration bins, force-advance after ``max_symbols_per_step``
symbols at one frame, decoder state updated only on non-blank emissions, a token is
emitted only when the frame index is still inside the window after its duration).
The last-chunk finalization pass of FluidAudio is not ported.

Long-form WER is intentionally not computed here: a standalone Python pipeline gave
misleading numbers (about 10 % for the community model versus 5.9 % through
FluidAudio). Measure long-form WER through the engine host instead.
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import sys
import time
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
from compat import install_lzma_backport  # noqa: E402

install_lzma_backport()

import coremltools as ct  # noqa: E402
import numpy as np  # noqa: E402
import soundfile as sf  # noqa: E402
import torch  # noqa: E402
import nemo.collections.asr as nemo_asr  # noqa: E402

from components import SUPPORTED_WINDOWS, trace_inputs  # noqa: E402


BLANK_ID = 8192
DURATION_BINS = (0, 1, 2, 3, 4)
MAX_SYMBOLS_PER_STEP = 10
MAX_TOKENS_PER_CHUNK = 150


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
    """Port of FluidAudio TdtDecoderV3.decodeWithTimings for one window.

    Returns (token, frame, duration) per emitted token.
    """
    decoder = bundle["Decoder"]
    joint = bundle["JointDecisionv3"]
    if encoded_length <= 1:
        return []

    def run_decoder(token: int, h: np.ndarray, c: np.ndarray):
        out = decoder.predict(
            {
                "targets": np.array([[token]], dtype=np.int32),
                "target_length": np.array([1], dtype=np.int32),
                "h_in": h,
                "c_in": c,
            }
        )
        return (
            np.asarray(out["decoder"], dtype=np.float32),
            np.asarray(out["h_out"], dtype=np.float16),
            np.asarray(out["c_out"], dtype=np.float16),
        )

    def run_joint(frame: int, decoder_step: np.ndarray) -> tuple[int, int]:
        decision = joint.predict(
            {
                "encoder_step": np.ascontiguousarray(encoded[:, :, frame : frame + 1], dtype=np.float16),
                "decoder_step": decoder_step.astype(np.float16),
            }
        )
        return (
            int(np.asarray(decision["token_id"]).reshape(-1)[0]),
            int(np.asarray(decision["duration"]).reshape(-1)[0]),
        )

    effective = encoded_length
    last_timestep = effective - 1
    time_index = 0
    safe_time = 0
    active = time_index < effective
    # Prime with blank as start-of-sequence (zero state).
    zeros = np.zeros((2, 1, 640), dtype=np.float16)
    predictor_out, h, c = run_decoder(BLANK_ID, zeros, zeros.copy())
    last_token: int | None = None
    last_emission_ts = -1
    emissions_at_ts = 0
    processed = 0
    tokens: list[tuple[int, int, int]] = []
    current_label_time = time_index

    while active:
        decision_token, duration_bin = run_joint(safe_time, predictor_out)
        label = decision_token
        duration = DURATION_BINS[duration_bin]
        blank = label == BLANK_ID
        current_time = time_index
        if not blank and duration == 0 and current_time == last_emission_ts and emissions_at_ts >= 1:
            duration = 1
        if blank and duration == 0:
            duration = 1
        current_label_time = time_index
        time_index += duration
        safe_time = min(time_index, last_timestep)
        active = time_index < effective
        advance = active and blank
        # Inner loop: skip blanks reusing the same decoder projection.
        while advance:
            current_label_time = time_index
            label, duration_bin = run_joint(safe_time, predictor_out)
            duration = DURATION_BINS[duration_bin]
            blank = label == BLANK_ID
            if blank and duration == 0:
                duration = 1
            time_index += duration
            safe_time = min(time_index, last_timestep)
            active = time_index < effective
            advance = active and blank
        if active and label != BLANK_ID:
            processed += 1
            if processed > MAX_TOKENS_PER_CHUNK:
                break
            tokens.append((label, current_label_time, duration))
            last_token = label
            # Decoder state advances only on non-blank emissions.
            predictor_out, h, c = run_decoder(label, h, c)
            if current_label_time == last_emission_ts:
                emissions_at_ts += 1
            else:
                last_emission_ts = current_label_time
                emissions_at_ts = 1
            if emissions_at_ts >= MAX_SYMBOLS_PER_STEP:
                time_index = min(time_index + 1, last_timestep)
                safe_time = min(time_index, last_timestep)
                emissions_at_ts = 0
                last_emission_ts = -1
        active = time_index < effective
    del last_token
    return tokens


def coreml_greedy_tokens(
    bundle: dict[str, ct.models.MLModel], encoded: np.ndarray, encoded_length: int
) -> list[int]:
    return [token for token, _, _ in coreml_greedy_emissions(bundle, encoded, encoded_length)]


def edit_distance(a: list[int], b: list[int]) -> int:
    previous = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        current = [i]
        for j, y in enumerate(b, 1):
            current.append(min(previous[j] + 1, current[j - 1] + 1, previous[j - 1] + (x != y)))
        previous = current
    return previous[-1]


def token_agreement(hyp: list[int], ref: list[int]) -> float:
    """1 - token edit distance / reference length (1.0 = identical)."""
    return 1.0 - edit_distance(hyp, ref) / max(len(ref), 1)


def bench_dir() -> Path:
    value = os.environ.get("SAGASCRIPT_BENCH_DIR")
    return Path(value) if value else Path.home() / ".cache" / "sagascript-bench"


def numerical(args: argparse.Namespace) -> dict[str, Any]:
    samples, _, _ = trace_inputs(args.window_s)
    audio_path = args.audio.resolve()
    model = restore(args.nemo.resolve())
    results: list[dict[str, Any]] = []
    bundles = {
        name: load_bundle(args.model_dir.resolve(), compute_units(name)) for name in args.compute_units
    }
    fp32 = {
        name: ct.models.MLModel(str(args.fp32_encoder.resolve()), compute_units=compute_units(name))
        for name in args.compute_units
    } if args.fp32_encoder is not None else {}
    for index in range(args.num_windows):
        start = index * samples
        audio, valid = read_window(audio_path, start, samples)
        if valid < samples:
            break
        mel_ref, mel_length, enc_ref, enc_length = reference_features(model, audio, valid)
        ref_tokens = nemo_greedy_tokens(model, enc_ref, enc_length)
        row: dict[str, Any] = {"start_s": start / 16_000, "nemo_tokens": len(ref_tokens)}
        for units_name, bundle in bundles.items():
            try:
                pre = bundle["Preprocessor"].predict(
                    {"audio_signal": audio, "audio_length": np.array([valid], dtype=np.int32)}
                )
                core_mel = np.asarray(pre["mel"], dtype=np.float32)
                row[f"{units_name}.mel_rel_l2"] = rel_l2(core_mel, mel_ref)
                # Encoder alone, fed the NeMo mel to isolate encoder error.
                enc = bundle["Encoder"].predict(
                    {"mel": mel_ref.astype(np.float16), "mel_length": np.array([mel_length], dtype=np.int32)}
                )
                core_enc = np.asarray(enc["encoder"], dtype=np.float32)
                row[f"{units_name}.encoder_rel_l2"] = rel_l2(core_enc, enc_ref)
                # Loop parity: Core ML decoder/joint on the NeMo fp32 encoder output.
                loop_tokens = coreml_greedy_tokens(bundle, enc_ref, enc_length)
                row[f"{units_name}.loop_on_nemo_encoder.identical"] = loop_tokens == ref_tokens
                row[f"{units_name}.loop_on_nemo_encoder.agreement"] = token_agreement(loop_tokens, ref_tokens)
                # Deployment pipeline: Core ML preprocessor -> encoder -> decode loop.
                enc_full = bundle["Encoder"].predict(
                    {
                        "mel": np.asarray(pre["mel"], dtype=np.float16),
                        "mel_length": np.asarray(pre["mel_length"], dtype=np.int32),
                    }
                )
                pipe_tokens = coreml_greedy_tokens(
                    bundle, np.asarray(enc_full["encoder"], dtype=np.float32), enc_length
                )
                row[f"{units_name}.pipeline.identical"] = pipe_tokens == ref_tokens
                row[f"{units_name}.pipeline.agreement"] = token_agreement(pipe_tokens, ref_tokens)
                if units_name in fp32:
                    fp32_out = fp32[units_name].predict(
                        {"mel": mel_ref.astype(np.float16), "mel_length": np.array([mel_length], dtype=np.int32)}
                    )
                    row[f"{units_name}.pre_quantization_encoder_rel_l2"] = rel_l2(
                        np.asarray(fp32_out["encoder"], dtype=np.float32), enc_ref
                    )
            except Exception as exc:
                row[f"{units_name}.error"] = f"{type(exc).__name__}: {exc}"
        results.append(row)

    summary: dict[str, Any] = {"windows": results, "window_s": args.window_s}
    for units_name in args.compute_units:
        ok = [row for row in results if f"{units_name}.mel_rel_l2" in row]
        if not ok:
            summary[f"{units_name}.error"] = next(
                (row[f"{units_name}.error"] for row in results if f"{units_name}.error" in row), "no windows"
            )
            continue
        for key in ("mel_rel_l2", "encoder_rel_l2"):
            values = [float(row[f"{units_name}.{key}"]) for row in ok]
            summary[f"{units_name}.{key}.mean"] = float(np.mean(values))
            summary[f"{units_name}.{key}.max"] = float(np.max(values))
        for stage in ("loop_on_nemo_encoder", "pipeline"):
            summary[f"{units_name}.{stage}.identical"] = f"{sum(bool(r[f'{units_name}.{stage}.identical']) for r in ok)}/{len(ok)}"
            summary[f"{units_name}.{stage}.agreement.mean"] = float(
                np.mean([r[f"{units_name}.{stage}.agreement"] for r in ok])
            )
    return summary


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
            enc_warm = bundle["Encoder"].predict({"mel": mel, "mel_length": ml})
            first_predict = time.perf_counter() - first
            for _ in range(3):
                bundle["Encoder"].predict({"mel": mel, "mel_length": ml})
                coreml_greedy_tokens(bundle, np.asarray(enc_warm["encoder"], dtype=np.float16), trace_inputs(args.window_s)[2])
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
                "runs": args.runs,
                "encoder_ms_median": statistics.median(enc_times),
                "encoder_ms_p95": float(np.percentile(enc_times, 95)),
                "full_pipeline_ms_median": statistics.median(full_times),
                "full_pipeline_ms_p95": float(np.percentile(full_times, 95)),
            }
        except Exception as exc:
            output[units_name] = {"error": f"{type(exc).__name__}: {exc}"}
    return output


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--model-dir", type=Path, required=True)
    parser.add_argument("--nemo", type=Path, default=Path(os.environ.get("PIANISSIMO_NEMO", "")))
    parser.add_argument(
        "--audio",
        type=Path,
        default=bench_dir() / "longform" / "fleurs-sv-distinct-5min.wav",
        help="16 kHz mono wav (default: $SAGASCRIPT_BENCH_DIR/longform/fleurs-sv-distinct-5min.wav)",
    )
    parser.add_argument("--window-s", type=int, choices=SUPPORTED_WINDOWS, required=True)
    parser.add_argument("--num-windows", type=int, default=8, help="consecutive full windows from t=0")
    parser.add_argument("--compute-units", nargs="+", default=["CPU_AND_NE"])
    parser.add_argument("--report", type=Path)
    parser.add_argument("--fp32-encoder", type=Path, help="optional pre-quantization encoder package")
    parser.add_argument("--speed", action="store_true")
    parser.add_argument("--runs", type=int, default=20)
    args = parser.parse_args()
    if not str(args.nemo) or str(args.nemo) == ".":
        parser.error("--nemo is required (or set PIANISSIMO_NEMO)")
    report: dict[str, Any] = {"model_dir": str(args.model_dir), "numerical": numerical(args)}
    if args.speed:
        report["speed"] = speed(args)
    text = json.dumps(report, indent=2, ensure_ascii=False)
    print(text)
    if args.report:
        args.report.write_text(text + "\n")


if __name__ == "__main__":
    main()
