# Reproducible Klang Pianissimo CoreML conversion

This directory converts the pinned `KlangAI/pianissimo-sv` checkpoint into the
four-model layout loaded by FluidAudio Parakeet TDT v3:

| File | Contract |
| --- | --- |
| `Preprocessor.mlpackage` | `audio_signal`, `audio_length` → `mel`, `mel_length` |
| `Encoder.mlpackage` | `mel`, `mel_length` → `encoder`, `encoder_length` |
| `Decoder.mlpackage` | `targets`, `target_length`, `h_in`, `c_in` → `decoder`, `h_out`, `c_out` |
| `JointDecisionv3.mlpackage` | `encoder_step`, `decoder_step` → TDT decision outputs |

The packages use fixed 16 kHz windows: 240,000 samples / 1,501 mel frames /
188 encoder frames for 15 s, 480,000 / 3,001 / 376 for 30 s, and 640,000 / 4,001 / 501
for 40 s (`--window-s 15|30|40`). For 30/40 s windows `--rel-shift auto` (default) aligns
the relative-position scores with an exact reshape/slice skew instead of a gather: the gather
runs on the CPU in Core ML and makes the 30 s encoder about 4x slower end to end
(`docs/benchmarks/pianissimo-coreml-2026-09.md`, section 7). The encoder
weights use CoreML linear int8 per-channel quantization. The encoder computes
in fp16 by default (`--encoder-precision fp16`) so it runs on the Apple Neural
Engine, which executes fp16 only; `--encoder-precision fp32` keeps the older
CPU/GPU-only variant (about 7x slower on `CPU_AND_NE`). The preprocessor
computes in fp32 (its fp16 form yields NaN mel on the ANE path for 30 s windows
and 2-4 % mel error for 15 s); decoder and joint compute in fp16. Every model's
*interface* is float32 (int32 for lengths and token ids), identical to the
community conversion and FluidAudio: fp16 compute happens inside the program,
so hosts never see fp16 tensors from the packages. (A float16 interface made
the host reject the encoder output; the host now accepts either.) Every output
directory contains a manifest with per-file sizes, SHA-256 checksums and the
encoder precision.

## Exact reproduction

The commands below use only PyPI, the public Hugging Face model, and the pinned
Mobius source clone. All caches and temporary artifacts belong under the scratch
directory. The Python 3.10.13 interpreter on the reference host was built
without `_lzma`; `backports-lzma` is therefore installed from PyPI with the
already-installed Homebrew xz headers, and `compat.py` activates it before NeMo
imports.

```bash
cd "$(git rev-parse --show-toplevel)"
export SCRATCH=$HOME/.cache/sagascript-bench/convert
export UV_CACHE_DIR=$SCRATCH/.uv-cache
export PIP_CACHE_DIR=$SCRATCH/.pip-cache
export HF_HOME=$SCRATCH/hf
export TORCH_HOME=$SCRATCH/torch
export XDG_CACHE_HOME=$SCRATCH/.cache
export PIANISSIMO_NEMO=$SCRATCH/model/pianissimo-sv.nemo
export PY=$SCRATCH/.venv/bin/python

# The converter was adapted from this exact Mobius revision.
git clone https://github.com/FluidInference/mobius $SCRATCH/mobius
git -C $SCRATCH/mobius checkout 864ef8050f2f281d0761de26e3a03108f9f1ce73

# Python 3.10 or 3.11; the lock was captured from Python 3.10.13.
$SCRATCH/uv-tools/bin/uv venv --python 3.10 $SCRATCH/.venv
$SCRATCH/uv-tools/bin/uv pip install --python $PY --index-url https://pypi.org/simple \
  'torch==2.7.0' 'nemo_toolkit[asr]==2.3.1' 'coremltools==9.0b1' \
  'soundfile==0.13.1' 'jiwer==4.0.0' 'huggingface_hub==0.33.1' \
  'typer==0.16.0'
export CFLAGS='-I/opt/homebrew/opt/xz/include'
export LDFLAGS='-L/opt/homebrew/opt/xz/lib'
$SCRATCH/uv-tools/bin/uv pip install --python $PY --index-url https://pypi.org/simple \
  'scipy==1.14.1' 'backports.lzma==0.0.14'

$PY - <<'PY'
import os
from huggingface_hub import hf_hub_download
hf_hub_download(
    repo_id='KlangAI/pianissimo-sv',
    filename='pianissimo-sv.nemo',
    revision='8f1f6d8f8bd7482a5ea1d2bfaf6ef5be61597138',
    local_dir=os.path.expanduser('~/.cache/sagascript-bench/convert/model'),
)
PY
shasum -a 256 $PIANISSIMO_NEMO

# Shipping artifact: 15 s window, Neural-Engine encoder (default --encoder-precision fp16)
$PY scripts/pianissimo-coreml/convert.py --window-s 15 \
  --nemo $PIANISSIMO_NEMO --out $SCRATCH/out/pianissimo-sv-coreml-own-15s
# Optional: --window-s 30 (see the 30 s findings below), or --encoder-precision fp32
# (CPU/GPU-only encoder) for comparison.
```

The conversion basis is Fluid Inference Mobius commit
`864ef8050f2f281d0761de26e3a03108f9f1ce73`. The exporter changes its names and
I/O to match FluidAudio, pins both fixed windows, keeps the decoder/joint fp16,
and quantizes only the encoder.

## Validation

`validate.py` compares, on consecutive full windows of the FLEURS Swedish
audio, the Core ML mel and encoder outputs with NeMo PyTorch fp32 (relative
L2) and the greedy TDT token sequences. Its decode loop is a port of
FluidAudio's `TdtDecoderV3` main loop (blank inner loop, duration bins
`[0..4]`, force-advance after 10 symbols at one frame, decoder state updated
only on non-blank emissions); the last-chunk finalization pass is not ported.
It reports two token checks against NeMo's own greedy decode: the loop on the
NeMo fp32 encoder output (isolates the loop and decoder/joint) and the full Core
ML pipeline (preprocessor, encoder, loop). Agreement is
`1 - token edit distance / reference length`. `--speed` reports first load and
first prediction, and the median/p95 of `--runs` (default 20) warm encoder and
full-pipeline runs. Set `SAGASCRIPT_BENCH_DIR` (default
`~/.cache/sagascript-bench`) or pass `--audio`.

```bash
export SAGASCRIPT_BENCH_DIR=$HOME/.cache/sagascript-bench
$PY scripts/pianissimo-coreml/validate.py \
  --model-dir $SCRATCH/out/pianissimo-sv-coreml-own-15s --nemo $PIANISSIMO_NEMO \
  --window-s 15 --num-windows 8 --compute-units CPU_AND_NE \
  --speed --report $SCRATCH/validate-15.json

# Baseline: community conversion (same package names and contracts)
$PY scripts/pianissimo-coreml/validate.py \
  --model-dir $SAGASCRIPT_BENCH_DIR/models/pianissimo-sv-coreml-markstrom \
  --nemo $PIANISSIMO_NEMO --window-s 15 --num-windows 8 --compute-units CPU_AND_NE
```

Op placement per compute unit (`MLComputePlan`, `const`/`constexpr` ops skipped):

```bash
$PY scripts/pianissimo-coreml/placement.py \
  $SCRATCH/out/pianissimo-sv-coreml-own-15s/Encoder.mlpackage --compute-units CPU_AND_NE
```

30 s windows (before the skew rel-shift, see above): the fp16 encoder is numerically sound (the earlier reported fp16
divergence came from the fp16 *preprocessor*, which produced non-finite mel on
`CPU_AND_NE`; the preprocessor now computes in fp32). Core ML's plan reports
about 96 % of the 30 s fp16 encoder ops on the ANE, but measured warm latency on
`CPU_AND_NE` is about 235 ms (roughly 7x the 15 s encoder for 2x the audio) and
the first load compiles for minutes, so treat 30 s fp16 as unproven on the ANE;
prefer the 15 s fp16 artifact. (The 30 s outputs were removed after these findings;
regenerate with `--window-s 30` if needed.)

Long-form WER is deliberately not computed here. A standalone Python pipeline
gave about 10 % WER even for the community model (5.9 % through FluidAudio), so
its long-form numbers were misleading; measure long-form WER through the engine
host.

The 30 s attention adapter uses NeMo's relative-position table and dense scores
with a constant additive mask for `abs(key-query) > 256`; the conversion checks
it against NeMo's original local-attention forward before export.

Do not commit `.nemo` files, `.mlpackage` outputs, `.venv`, caches, or the
conversion work directories. Core ML also keeps a compiled-model cache
(`~/Library/Caches/*/com.apple.e5rt.e5bundlecache`, about 1 GB per loaded
variant); first-load timings include that compilation.
