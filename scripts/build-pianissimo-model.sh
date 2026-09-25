#!/usr/bin/env bash
set -euo pipefail

# Build the corrected Pianissimo Q8 model used by the native runtime.
#
# This is deliberately a local-input build.  The distributed app contains no
# Python and users never run this converter.  Keeping the source checkpoint,
# converter checkout, and conversion environment explicit makes release model
# generation reproducible and prevents an unpinned Hugging Face download from
# silently changing the artifact.

readonly SOURCE_SHA256="ca340b827dc9e18d2019341fa7b6dc163f00284d84066ce2cbfffcaa129920cd"
readonly CONVERTER_COMMIT="4f9676226f667d14608487df744f375db87127f8"
readonly OUTPUT_SHA256="56ed7a0199c2c6116b3254505296e36b8126275f5c2bce826613aeb1fca7b1b5"

usage() {
  cat >&2 <<'EOF'
Usage: build-pianissimo-model.sh SOURCE_NEMO OUTPUT_GGUF CONVERTER_ROOT PYTHON

SOURCE_NEMO must be the pinned KlangAI Pianissimo .nemo checkpoint.
CONVERTER_ROOT must be a clean NeMo-Speech.cpp checkout at the pinned commit.
PYTHON must be the conversion environment's interpreter.  Set PYTHONPATH when
the environment keeps gguf outside site-packages.
EOF
  exit 2
}

[[ $# -eq 4 ]] || usage

source_model=$1
output=$2
converter_root=$3
python_bin=$4

[[ -f "$source_model" ]] || {
  echo "Pianissimo source checkpoint does not exist: $source_model" >&2
  exit 1
}
[[ -f "$converter_root/convert_model.py" ]] || {
  echo "NeMo-Speech.cpp converter not found under: $converter_root" >&2
  exit 1
}
[[ -x "$python_bin" ]] || {
  echo "Python interpreter is not executable: $python_bin" >&2
  exit 1
}
[[ ! -e "$output" ]] || {
  echo "Refusing to overwrite existing model: $output" >&2
  exit 1
}

sha256_file() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    sha256sum "$1" | awk '{print $1}'
  fi
}

actual_source_sha256=$(sha256_file "$source_model")
[[ "$actual_source_sha256" == "$SOURCE_SHA256" ]] || {
  echo "Unexpected Pianissimo source SHA-256: $actual_source_sha256" >&2
  echo "Expected: $SOURCE_SHA256" >&2
  exit 1
}

actual_converter_commit=$(git -C "$converter_root" rev-parse HEAD 2>/dev/null || true)
[[ "$actual_converter_commit" == "$CONVERTER_COMMIT" ]] || {
  echo "Unexpected NeMo-Speech.cpp converter commit: $actual_converter_commit" >&2
  echo "Expected: $CONVERTER_COMMIT" >&2
  exit 1
}

# librosa imports some modules lazily.  This exact probe catches the old
# Python environment that reported a misleading "librosa not installed"
# warning and caused the runtime to use unit-peak mel triangles.
PYTHONNOUSERSITE=1 "$python_bin" - <<'PY'
import importlib.util

if importlib.util.find_spec("_lzma") is None:
    raise SystemExit("Python conversion environment has no _lzma module")

import librosa

filterbank = librosa.filters.mel(
    sr=16000,
    n_fft=512,
    n_mels=128,
    fmin=0.0,
    fmax=8000.0,
    norm="slaney",
    htk=False,
)
if filterbank.shape != (128, 257):
    raise SystemExit(f"unexpected mel filterbank shape: {filterbank.shape}")
print("conversion prerequisites: _lzma and librosa mel filterbank OK")
PY

output_dir=$(dirname "$output")
mkdir -p "$output_dir"
scratch=$(mktemp -d "${TMPDIR:-/tmp}/sagascript-pianissimo-model.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
# The converter stores the output stem in GGUF's general.name metadata.  Keep
# the canonical stem here so the checked SHA-256 remains stable even when a
# caller places the resulting artifact in a different directory or gives it
# a release-specific filename.
temporary_output="$scratch/pianissimo-sv-q8-melfix.gguf"

echo "Converting pinned Pianissimo checkpoint to corrected Q8 GGUF..."
# Preserve a caller-provided PYTHONPATH (for example a pinned gguf wheel in an
# offline build cache) while putting the pinned converter first.
if [[ -n "${PYTHONPATH:-}" ]]; then
  converter_pythonpath="$converter_root:$PYTHONPATH"
else
  converter_pythonpath="$converter_root"
fi
PYTHONNOUSERSITE=1 PYTHONPATH="$converter_pythonpath" \
  "$python_bin" "$converter_root/convert_model.py" "$source_model" \
  --outfile "$temporary_output" \
  --architecture asr \
  --head-type tdt \
  --outtype q8_0 \
  --q8-layout block

[[ -s "$temporary_output" ]] || {
  echo "Converter did not produce a non-empty GGUF" >&2
  exit 1
}

# Validate the artifact before publishing it to the requested path.  The
# tensor count and filterbank shape are the guard against silently recreating
# the lossy preprocessor fallback.
PYTHONNOUSERSITE=1 PYTHONPATH="$converter_pythonpath" GGUF_PATH="$temporary_output" \
  "$python_bin" - <<'PY'
import os
from gguf import GGMLQuantizationType, GGUFReader

reader = GGUFReader(os.environ["GGUF_PATH"])
if len(reader.tensors) != 701:
    raise SystemExit(f"unexpected tensor count: {len(reader.tensors)} (expected 701)")

filterbanks = [tensor for tensor in reader.tensors if tensor.name == "preprocessor.fb"]
if len(filterbanks) != 1:
    raise SystemExit("GGUF must contain exactly one preprocessor.fb tensor")

filterbank = filterbanks[0]
if tuple(int(value) for value in filterbank.data.shape) != (128, 257):
    raise SystemExit(f"unexpected preprocessor.fb shape: {filterbank.data.shape}")
if filterbank.tensor_type != GGMLQuantizationType.F32:
    raise SystemExit(f"preprocessor.fb must be F32, got {filterbank.tensor_type}")
print("GGUF validation: 701 tensors and F32 preprocessor.fb (128, 257)")
PY

actual_output_sha256=$(sha256_file "$temporary_output")
[[ "$actual_output_sha256" == "$OUTPUT_SHA256" ]] || {
  echo "Unexpected corrected Pianissimo output SHA-256: $actual_output_sha256" >&2
  echo "Expected: $OUTPUT_SHA256" >&2
  exit 1
}

mv "$temporary_output" "$output"
echo "Pianissimo model ready: $output"
echo "SHA-256: $actual_output_sha256"
