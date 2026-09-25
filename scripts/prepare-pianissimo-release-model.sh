#!/usr/bin/env bash
set -euo pipefail

# Prepare the verified optional GGUF asset for a signed test build or release.
# Python is used only on the build runner; it is never copied into the app.
repo_root=$(cd "$(dirname "$0")/.." && pwd)
output=${1:-"$repo_root/build/pianissimo-sv-q8-melfix.gguf"}
[[ ! -e "$output" ]] || { echo "Model output already exists: $output" >&2; exit 1; }

scratch=$(mktemp -d "${TMPDIR:-/tmp}/sagascript-pianissimo-release.XXXXXX")
trap 'rm -rf "$scratch"' EXIT

source_model=${PIANISSIMO_SOURCE_NEMO:-"$scratch/pianissimo-sv.nemo"}
if [[ -z "${PIANISSIMO_SOURCE_NEMO:-}" ]]; then
  curl --fail --location --retry 3 --connect-timeout 30 \
    --output "$source_model" \
    'https://huggingface.co/KlangAI/pianissimo-sv/resolve/8f1f6d8f8bd7482a5ea1d2bfaf6ef5be61597138/pianissimo-sv.nemo'
fi

converter_root=${PIANISSIMO_CONVERTER_ROOT:-"$scratch/NeMo-Speech.cpp"}
if [[ -z "${PIANISSIMO_CONVERTER_ROOT:-}" ]]; then
  git clone --depth 1 --branch v0.1.0 https://github.com/NVIDIA/NeMo-Speech.cpp.git "$converter_root"
fi

python_bin=${PIANISSIMO_CONVERTER_PYTHON:-"$scratch/venv/bin/python"}
if [[ -z "${PIANISSIMO_CONVERTER_PYTHON:-}" ]]; then
  python3 -m venv "$scratch/venv"
  "$python_bin" -m pip install --disable-pip-version-check --no-cache-dir \
    torch==2.14.0 numpy==2.5.3 librosa==1.0.0 gguf==0.19.0 pyyaml==6.0.3 sentencepiece==0.2.2
fi

"$repo_root/scripts/build-pianissimo-model.sh" \
  "$source_model" "$output" "$converter_root" "$python_bin"
