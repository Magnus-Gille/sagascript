#!/usr/bin/env python3
"""Reproducibly export Klang Pianissimo to FluidAudio's four-model v3 layout."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
from compat import install_lzma_backport  # noqa: E402

install_lzma_backport()

import coremltools as ct  # noqa: E402
import numpy as np  # noqa: E402
import torch  # noqa: E402
import nemo.collections.asr as nemo_asr  # noqa: E402
from coremltools.optimize.coreml import (  # noqa: E402
    OpLinearQuantizerConfig,
    OptimizationConfig,
    linear_quantize_weights,
)

from components import (  # noqa: E402
    DecoderWrapper,
    EncoderWrapper,
    JointDecisionV3,
    PreprocessorWrapper,
    patch_local_attention,
    trace_inputs,
)


SOURCE_REPO = "KlangAI/pianissimo-sv"
SOURCE_REVISION = "8f1f6d8f8bd7482a5ea1d2bfaf6ef5be61597138"
EXPECTED_NEMO_SHA256 = "ca340b827dc9e18d2019341fa7b6dc163f00284d84066ce2cbfffcaa129920cd"
MOBIUS_REPO = "https://github.com/FluidInference/mobius"
MOBIUS_COMMIT = "864ef8050f2f281d0761de26e3a03108f9f1ce73"
FRAME_S = 0.08


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def package_size(path: Path) -> int:
    return sum(item.stat().st_size for item in path.rglob("*") if item.is_file())


def package_files(root: Path) -> list[dict[str, Any]]:
    files: list[dict[str, Any]] = []
    for path in sorted(root.rglob("*")):
        if not path.is_file() or path.name == "manifest.json":
            continue
        files.append(
            {
                "path": path.relative_to(root).as_posix(),
                "size": path.stat().st_size,
                "sha256": sha256(path),
            }
        )
    return files


def restore(nemo_path: Path):
    actual = sha256(nemo_path)
    if actual != EXPECTED_NEMO_SHA256:
        raise ValueError(f"Unexpected .nemo SHA-256: {actual} != {EXPECTED_NEMO_SHA256}")
    model = nemo_asr.models.EncDecRNNTBPEModel.restore_from(
        str(nemo_path), map_location="cpu"
    )
    model.eval()
    if list(model.encoder.att_context_size) != [256, 256]:
        raise ValueError(f"Unexpected encoder context: {model.encoder.att_context_size}")
    if model.encoder.n_layers != 24 or model.encoder.d_model != 1024:
        raise ValueError("Unexpected Pianissimo encoder dimensions")
    return model, actual


def convert_model(
    module: torch.jit.ScriptModule,
    inputs,
    outputs,
    description: str,
    precision=ct.precision.FLOAT16,
):
    model = ct.convert(
        module,
        convert_to="mlprogram",
        inputs=inputs,
        outputs=outputs,
        compute_units=ct.ComputeUnit.CPU_ONLY,
        minimum_deployment_target=ct.target.iOS17,
        compute_precision=precision,
    )
    model.short_description = description
    model.author = "Klang AI AB (Klang Pianissimo, CC BY 4.0); Sagascript CoreML conversion"
    model.version = f"{SOURCE_REPO}@{SOURCE_REVISION[:8]}"
    model.license = "CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/)"
    model.user_defined_metadata["source_repo"] = SOURCE_REPO
    model.user_defined_metadata["source_revision"] = SOURCE_REVISION
    model.user_defined_metadata["mobius_commit"] = MOBIUS_COMMIT
    model.user_defined_metadata["conversion_layout"] = "FluidAudio Parakeet TDT v3"
    return model


def save(model: ct.models.MLModel, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    model.save(str(path))


def vocabulary(model, path: Path) -> None:
    ids = list(range(int(model.tokenizer.vocab_size)))
    pieces = model.tokenizer.ids_to_tokens(ids)
    path.write_text(
        json.dumps({str(index): piece for index, piece in enumerate(pieces)}, indent=2, ensure_ascii=False)
        + "\n"
    )


def copy_attribution(path: Path) -> None:
    path.write_text(
        f"""Klang Pianissimo (Sagascript CoreML conversion)

Model: Klang Pianissimo by Klang AI AB
Source: https://huggingface.co/{SOURCE_REPO}
Revision: {SOURCE_REVISION}
pianissimo-sv.nemo SHA-256: {EXPECTED_NEMO_SHA256}
License: Creative Commons Attribution 4.0 International (CC BY 4.0)
https://creativecommons.org/licenses/by/4.0/

The model weights are converted, not retrained. Please preserve Klang AI AB's
attribution and the CC BY 4.0 license when redistributing the model.

Conversion basis
----------------
This exporter adapts Fluid Inference mobius ({MOBIUS_REPO}, commit {MOBIUS_COMMIT}),
licensed under the Apache License, Version 2.0.
https://www.apache.org/licenses/LICENSE-2.0

The Mobius-derived changes are: FluidAudio v3 component names and I/O, fixed 15/30 s
contracts, the exact dense fixed-shape +/-256-frame local-attention adapter,
encoder-only int8 per-channel linear quantization, manifest generation, and
validation tooling. Decoder and JointDecisionv3 remain fp16.

Required third-party notices
----------------------------
Fluid Inference mobius: Apache License 2.0, copyright and notices retained at
{MOBIUS_REPO}@{MOBIUS_COMMIT}.
Klang Pianissimo: CC BY 4.0, attribution above.
NVIDIA Parakeet TDT v3 architecture/base: CC BY 4.0 as stated by the upstream
model card; the present checkpoint is Klang AI AB's fine-tune.
"""
    )


def build_manifest(out: Path, nemo_path: Path, nemo_sha: str, window_s: int, samples: int, mel_frames: int, enc_frames: int) -> None:
    components = {}
    for name in ("Preprocessor", "Encoder", "Decoder", "JointDecisionv3"):
        package = out / f"{name}.mlpackage"
        components[name] = {
            "path": package.name,
            "size": package_size(package),
            "files": package_files(package),
        }
    manifest = {
        "schema": 1,
        "model": {
            "repo": SOURCE_REPO,
            "revision": SOURCE_REVISION,
            "file": nemo_path.name,
            "sha256": nemo_sha,
            "license": "CC-BY-4.0",
            "credit": "Klang Pianissimo by Klang AI AB",
        },
        "mobius": {"repo": MOBIUS_REPO, "commit": MOBIUS_COMMIT, "license": "Apache-2.0"},
        "window_s": window_s,
        "frame_s": FRAME_S,
        "audio_samples": samples,
        "mel_frames": mel_frames,
        "encoder_frames": enc_frames,
        "attention": {"kind": "rel_pos_local_attn", "left_frames": 256, "right_frames": 256, "export": "dense constant additive band mask"},
        "quantization": {
            "encoder": "int8 linear per-channel weights",
            "encoder_compute_precision": "float32",
            "decoder": "fp16",
            "joint": "fp16",
            "preprocessor": "fp16",
        },
        "converter_versions": {
            "python": platform.python_version(),
            "torch": torch.__version__,
            "nemo_toolkit": nemo_asr.__version__ if hasattr(nemo_asr, "__version__") else "2.3.1",
            "coremltools": ct.__version__,
            "numpy": np.__version__,
        },
        "components": components,
        "files": package_files(out),
    }
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--window-s", type=int, choices=(15, 30), required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--nemo", type=Path, default=Path(os.environ.get("PIANISSIMO_NEMO", "")))
    args = parser.parse_args()
    if not args.nemo:
        parser.error("--nemo is required (or set PIANISSIMO_NEMO)")
    args.nemo = args.nemo.resolve()
    out = args.out.resolve()
    if out.exists() and any(out.iterdir()):
        raise SystemExit(f"Refusing to overwrite non-empty output: {out}")
    out.mkdir(parents=True, exist_ok=True)

    torch.set_num_threads(int(os.environ.get("SAGASCRIPT_TORCH_THREADS", "8")))
    samples, mel_frames, enc_frames = trace_inputs(args.window_s)
    model, nemo_sha = restore(args.nemo)
    print(f"source={SOURCE_REPO}@{SOURCE_REVISION}")
    print(f"nemo={args.nemo} sha256={nemo_sha}")
    print(f"window={args.window_s}s samples={samples} mel_frames={mel_frames} encoder_frames={enc_frames}")

    work = out.parent / f".{out.name}.work"
    work.mkdir(parents=True, exist_ok=True)
    audio = torch.zeros((1, samples), dtype=torch.float32)
    audio_length = torch.tensor([samples], dtype=torch.int32)
    pre = PreprocessorWrapper(model.preprocessor.eval())
    with torch.inference_mode():
        mel, mel_length = pre(audio, audio_length)
    mel = mel.clone()
    mel_length = mel_length.to(dtype=torch.int32).clone()
    print(f"trace mel={tuple(mel.shape)} mel_length={mel_length.tolist()}")

    pre_trace = torch.jit.trace(pre, (audio, audio_length), strict=False).eval()
    pre_ml = convert_model(
        pre_trace,
        [ct.TensorType(name="audio_signal", shape=(1, samples), dtype=np.float16), ct.TensorType(name="audio_length", shape=(1,), dtype=np.int32)],
        [ct.TensorType(name="mel", dtype=np.float16), ct.TensorType(name="mel_length", dtype=np.int32)],
        f"Pianissimo preprocessor ({args.window_s} s window)",
    )
    save(pre_ml, out / "Preprocessor.mlpackage")

    patch_local_attention(model.encoder, enc_frames)
    encoder = EncoderWrapper(model.encoder.eval())
    with torch.inference_mode():
        encoded, encoded_length = encoder(mel, mel_length)
    encoded = encoded.clone()
    encoded_length = encoded_length.to(dtype=torch.int32).clone()
    print(f"trace encoder={tuple(encoded.shape)} encoder_length={encoded_length.tolist()}")
    enc_trace = torch.jit.trace(encoder, (mel, mel_length), strict=False).eval()
    enc_fp32 = convert_model(
        enc_trace,
        [ct.TensorType(name="mel", shape=(1, 128, mel_frames), dtype=np.float16), ct.TensorType(name="mel_length", shape=(1,), dtype=np.int32)],
        [ct.TensorType(name="encoder", dtype=np.float16), ct.TensorType(name="encoder_length", dtype=np.int32)],
        f"Pianissimo encoder ({args.window_s} s window, exact +/-256 local attention, fp32 pre-quantization)",
        precision=ct.precision.FLOAT32,
    )
    fp32_path = work / "Encoder.fp32.mlpackage"
    save(enc_fp32, fp32_path)
    quant_cfg = OptimizationConfig(
        global_config=OpLinearQuantizerConfig(mode="linear", granularity="per_channel")
    )
    quantized = linear_quantize_weights(
        ct.models.MLModel(str(fp32_path), compute_units=ct.ComputeUnit.CPU_ONLY), quant_cfg
    )
    quantized.short_description = f"Pianissimo encoder ({args.window_s} s window, int8 per-channel local attention)"
    quantized.author = pre_ml.author
    quantized.version = pre_ml.version
    quantized.license = pre_ml.license
    quantized.user_defined_metadata.update(enc_fp32.user_defined_metadata)
    save(quantized, out / "Encoder.mlpackage")

    model.decoder._rnnt_export = True
    decoder = DecoderWrapper(model.decoder.eval())
    targets = torch.full((1, 1), int(model.decoder.blank_idx), dtype=torch.int32)
    target_length = torch.ones((1,), dtype=torch.int32)
    state = torch.zeros((int(model.decoder.pred_rnn_layers), 1, int(model.decoder.pred_hidden)), dtype=torch.float32)
    dec_trace = torch.jit.trace(decoder, (targets, target_length, state, state), strict=False).eval()
    dec_ml = convert_model(
        dec_trace,
        [ct.TensorType(name="targets", shape=(1, 1), dtype=np.int32), ct.TensorType(name="target_length", shape=(1,), dtype=np.int32), ct.TensorType(name="h_in", shape=tuple(state.shape), dtype=np.float16), ct.TensorType(name="c_in", shape=tuple(state.shape), dtype=np.float16)],
        [ct.TensorType(name="decoder", dtype=np.float16), ct.TensorType(name="h_out", dtype=np.float16), ct.TensorType(name="c_out", dtype=np.float16)],
        "Pianissimo decoder (TDT prediction network)",
    )
    save(dec_ml, out / "Decoder.mlpackage")

    jd = JointDecisionV3(model.joint.eval(), int(model.tokenizer.vocab_size), int(model.joint.num_extra_outputs))
    encoder_step = encoded[:, :, :1]
    decoder_step = torch.zeros((1, 640, 1), dtype=torch.float32)
    jd_trace = torch.jit.trace(jd, (encoder_step, decoder_step), strict=False).eval()
    jd_ml = convert_model(
        jd_trace,
        [ct.TensorType(name="encoder_step", shape=(1, 1024, 1), dtype=np.float16), ct.TensorType(name="decoder_step", shape=(1, 640, 1), dtype=np.float16)],
        [ct.TensorType(name="token_id", dtype=np.int32), ct.TensorType(name="token_prob", dtype=np.float16), ct.TensorType(name="duration", dtype=np.int32), ct.TensorType(name="top_k_ids", dtype=np.int32), ct.TensorType(name="top_k_logits", dtype=np.float16)],
        "Pianissimo JointDecisionv3 (single-step TDT decision head)",
    )
    save(jd_ml, out / "JointDecisionv3.mlpackage")

    vocabulary(model, out / "parakeet_vocab.json")
    copy_attribution(out / "LICENSE-and-attribution.txt")
    build_manifest(out, args.nemo, nemo_sha, args.window_s, samples, mel_frames, enc_frames)
    print(f"wrote={out}")


if __name__ == "__main__":
    main()
