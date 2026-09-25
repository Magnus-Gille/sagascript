#!/usr/bin/env bash
set -euo pipefail

# Build the arm64, relocatable Python environment used only by the macOS app.
# The checkpoint stays in the normal model download cache, outside the app.
repo_root=$(cd "$(dirname "$0")/.." && pwd)
output=${1:-"$repo_root/build/pianissimo-runtime"}
[[ $(uname -s) == Darwin && $(uname -m) == arm64 ]] || {
  echo "Pianissimo runtime packaging requires macOS arm64" >&2
  exit 1
}
command -v uv >/dev/null || { echo "Install uv before building the Pianissimo runtime" >&2; exit 1; }
[[ ! -e "$output" ]] || { echo "Runtime output already exists: $output" >&2; exit 1; }

scratch=$(mktemp -d "${TMPDIR:-/tmp}/sagascript-pianissimo-runtime.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
UV_NO_CACHE=1 uv python install 3.12.9 --install-dir "$scratch/managed" --no-bin
python_home="$scratch/managed/cpython-3.12.9-macos-aarch64-none"
[[ -x "$python_home/bin/python3.12" ]] || { echo "Managed Python was not installed" >&2; exit 1; }
uv venv --python "$python_home/bin/python3.12" "$scratch/venv"
UV_NO_CACHE=1 uv pip sync "$repo_root/scripts/pianissimo-runtime-hashed.txt" \
  --python "$scratch/venv/bin/python" --link-mode copy --strict --require-hashes

mkdir -p "$output"
mv "$python_home" "$output/python"
mv "$scratch/venv/lib/python3.12/site-packages" "$output/site-packages"
cp "$repo_root/scripts/pianissimo-cpython-LICENSE" "$output/CPYTHON_LICENSE"

python3 "$repo_root/scripts/verify-pianissimo-runtime.py" "$output"

PYTHONHOME="$output/python" PYTHONPATH="$output/site-packages" \
  "$output/python/bin/python3.12" -c \
  'import nemo, torch; assert nemo.__version__ == "2.7.3"; assert torch.__version__ == "2.14.0"'
echo "Pianissimo runtime ready: $output"
