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
188 encoder frames for 15 s, and 480,000 / 3,001 / 376 for 30 s. The encoder
weights alone use CoreML linear int8 per-channel quantization; its Core ML
compute precision is float32 to preserve the 30 s local-attention boundary
numerics. The other three components are fp16. Every output directory contains
a manifest with per-file sizes and SHA-256 checksums.

## Exact reproduction

The commands below use only PyPI, the public Hugging Face model, and the pinned
Mobius source clone. All caches and temporary artifacts belong under the scratch
directory. The Python 3.10.13 interpreter on the reference host was built
without `_lzma`; `backports-lzma` is therefore installed from PyPI with the
already-installed Homebrew xz headers, and `compat.py` activates it before NeMo
imports.

```bash
cd /Users/magnus/repos/sagascript-rn-coreml-convert
export SCRATCH=/Users/magnus/.cache/sagascript-bench/convert
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
from huggingface_hub import hf_hub_download
hf_hub_download(
    repo_id='KlangAI/pianissimo-sv',
    filename='pianissimo-sv.nemo',
    revision='8f1f6d8f8bd7482a5ea1d2bfaf6ef5be61597138',
    local_dir='/Users/magnus/.cache/sagascript-bench/convert/model',
)
PY
shasum -a 256 $PIANISSIMO_NEMO

$PY scripts/pianissimo-coreml/convert.py --window-s 15 \
  --nemo $PIANISSIMO_NEMO \
  --out $SCRATCH/out/pianissimo-sv-coreml-own-15s
$PY scripts/pianissimo-coreml/convert.py --window-s 30 \
  --nemo $PIANISSIMO_NEMO \
  --out $SCRATCH/out/pianissimo-sv-coreml-own-30s
```

The conversion basis is Fluid Inference Mobius commit
`864ef8050f2f281d0761de26e3a03108f9f1ce73`. The exporter changes its names and
I/O to match FluidAudio, pins both fixed windows, keeps the decoder/joint fp16,
and quantizes only the encoder.

## Validation

Run numerical parity on three real windows from the supplied FLEURS Swedish
audio. `validate.py` runs both `CPU_AND_NE` and `ALL`, compares mel and encoder
relative L2 to NeMo, and compares TDT greedy token IDs. It loads the supplied
`parakeet_mlx` alignment module directly from the benchmark venv so importing
the package does not require a Metal device in a headless shell.

```bash
$PY scripts/pianissimo-coreml/validate.py \
  --model-dir $SCRATCH/out/pianissimo-sv-coreml-own-15s \
  --fp32-encoder $SCRATCH/out/.pianissimo-sv-coreml-own-15s.work/Encoder.fp32.mlpackage \
  --nemo $PIANISSIMO_NEMO \
  --audio /Users/magnus/.cache/sagascript-bench/longform/fleurs-sv-distinct-5min.wav \
  --window-s 15 --compute-units CPU_AND_NE ALL --speed --report $SCRATCH/validate-15.json

$PY scripts/pianissimo-coreml/validate.py \
  --model-dir $SCRATCH/out/pianissimo-sv-coreml-own-30s \
  --fp32-encoder $SCRATCH/out/.pianissimo-sv-coreml-own-30s.work/Encoder.fp32.mlpackage \
  --nemo $PIANISSIMO_NEMO \
  --audio /Users/magnus/.cache/sagascript-bench/longform/fleurs-sv-distinct-5min.wav \
  --window-s 30 --compute-units CPU_AND_NE ALL --speed --report $SCRATCH/validate-30.json
```

For the long-form measurements used in this task, run the matching fixed-window
variant separately:

```bash
$PY scripts/pianissimo-coreml/validate.py \
  --model-dir $SCRATCH/out/pianissimo-sv-coreml-own-15s \
  --nemo $PIANISSIMO_NEMO \
  --audio /Users/magnus/.cache/sagascript-bench/longform/fleurs-sv-distinct-5min.wav \
  --window-s 15 --compute-units CPU_AND_NE --longform \
  --report $SCRATCH/longform-own-15.json
$PY scripts/pianissimo-coreml/validate.py \
  --model-dir $SCRATCH/out/pianissimo-sv-coreml-own-30s \
  --nemo $PIANISSIMO_NEMO \
  --audio /Users/magnus/.cache/sagascript-bench/longform/fleurs-sv-distinct-5min.wav \
  --window-s 30 --compute-units CPU_AND_NE --longform \
  --report $SCRATCH/longform-own-30.json
$PY scripts/pianissimo-coreml/validate.py \
  --model-dir /Users/magnus/.cache/sagascript-bench/models/pianissimo-sv-coreml-markstrom \
  --nemo $PIANISSIMO_NEMO \
  --audio /Users/magnus/.cache/sagascript-bench/longform/fleurs-sv-distinct-5min.wav \
  --window-s 15 --compute-units CPU_AND_NE --longform \
  --report $SCRATCH/longform-markstrom-15.json
```

For long-form scoring, add `--longform`. It uses the same two overlap settings
as the benchmark runner (15 s / 2 s and 30 s / 6 s),
`merge_longest_contiguous` with the benchmark fallback to
`merge_longest_common_subsequence`, `norm.py`, and `jiwer.process_words`.
The supplied `fleurs-sv-distinct-5min` and `-15min` audio/reference paths are
defaults. To compare the community conversion, pass its model directory to a
separate invocation; its package names and contracts are identical.

The 30 s attention proof is also available independently in the conversion
implementation: it uses NeMo's same relative-position table and dense scores,
with a constant additive mask for `abs(key-query) > 256`. The conversion check
compares this patched PyTorch forward to NeMo's original local-attention forward
before export; `validate.py` then checks the resulting CoreML encoder. The
optional fp32 pre-quantization packages are retained only in the scratch work
directory for validation. `--speed` reports first model load, first prediction,
warmed encoder, and warmed full-pipeline latency for the requested compute units.
On the reference headless host, `CPU_AND_NE` executes successfully; Core ML
`ALL` model-plan creation returns error code `-6` before prediction.

Do not commit `.nemo` files, `.mlpackage` outputs, `.venv`, caches, or the
conversion work directories.
