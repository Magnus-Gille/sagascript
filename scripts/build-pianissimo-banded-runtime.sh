#!/usr/bin/env bash
# Build an experimental, source-pinned Metal runtime with bounded local attention.
# Not a release default: the published CPU runtime remains the signed baseline.
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
output=${1:?Usage: build-pianissimo-banded-runtime.sh OUTPUT_DIRECTORY}
patch="$repo_root/scripts/pianissimo-local-attention-v0.1.0.patch"
source_sha=4f9676226f667d14608487df744f375db87127f8
ggml_sha=c03b4e2bcece5134827881af90242086daf75be5
sentencepiece_sha=17d7580d6407802f85855d2cc9190634e2c95624
jobs=${PIANISSIMO_BUILD_JOBS:-4}

[[ $(uname -s) == Darwin && $(uname -m) == arm64 ]] || {
  echo 'The experimental Metal runtime requires Apple Silicon macOS.' >&2
  exit 1
}
[[ "$jobs" =~ ^[1-9][0-9]*$ && "$jobs" -le 8 ]] || {
  echo 'PIANISSIMO_BUILD_JOBS must be between 1 and 8.' >&2
  exit 1
}
[[ ! -e "$output" && ! -L "$output" ]] || {
  echo 'Output already exists; refusing to replace it.' >&2
  exit 1
}
[[ -d "$(dirname "$output")" && -f "$patch" ]] || {
  echo 'Output parent or pinned patch is missing.' >&2
  exit 1
}

scratch=$(mktemp -d "${PIANISSIMO_BUILD_TMPDIR:-${TMPDIR:-/tmp}}/sagascript-banded.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
deps="$scratch/deps"
sp_source="$scratch/sentencepiece-source"
sp_build="$scratch/sentencepiece-build"
sp_prefix="$deps/sentencepiece"
nemo_source="$scratch/nemo-source"
nemo_build="$scratch/nemo-build"
stage="$scratch/staged"

# SentencePiece is linked into ASR and stays in this private build prefix.
git clone --filter=blob:none --no-checkout 'https://github.com/google/sentencepiece.git' "$sp_source"
git -C "$sp_source" fetch --depth 1 origin "$sentencepiece_sha"
git -C "$sp_source" checkout --detach --quiet "$sentencepiece_sha"
[[ $(git -C "$sp_source" rev-parse HEAD) == "$sentencepiece_sha" ]]

sdk_root=${SDKROOT:-}
if [[ -z "$sdk_root" ]]; then
  sdk_root=$(xcrun --sdk macosx --show-sdk-path)
fi
[[ -d "$sdk_root" ]] || {
  echo 'A valid macOS SDKROOT is required to build the native runtime.' >&2
  exit 1
}
# Always nonempty: macOS's bundled Bash 3.2 treats an empty array expansion as
# an unbound variable under `set -u`.
sdk_args=("-DCMAKE_OSX_SYSROOT=$sdk_root")
cmake -S "$sp_source" -B "$sp_build" -G 'Unix Makefiles' \
  "${sdk_args[@]}" -DCMAKE_POLICY_VERSION_MINIMUM=3.5 \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_OSX_ARCHITECTURES=arm64 \
  -DCMAKE_OSX_DEPLOYMENT_TARGET=13.0 -DSPM_BUILD_TEST=OFF \
  -DSPM_ENABLE_SHARED=OFF -DSPM_ENABLE_TCMALLOC=OFF \
  "-DCMAKE_INSTALL_PREFIX=$sp_prefix"
cmake --build "$sp_build" --parallel "$jobs"
cmake --install "$sp_build"

license_dir="$sp_prefix/share/licenses/nemo-speech/third_party/sentencepiece"
mkdir -p "$license_dir"
install -m 0644 "$sp_source/LICENSE" "$license_dir/LICENSE"
install -m 0644 "$sp_source/third_party/absl/LICENSE" "$license_dir/absl-LICENSE"
install -m 0644 "$sp_source/third_party/darts_clone/LICENSE" "$license_dir/darts-clone-LICENSE"
install -m 0644 "$sp_source/third_party/protobuf-lite/LICENSE" "$license_dir/protobuf-lite-LICENSE"

git clone --depth 1 --branch v0.1.0 --single-branch \
  'https://github.com/NVIDIA/NeMo-Speech.cpp.git' "$nemo_source"
[[ $(git -C "$nemo_source" rev-parse HEAD) == "$source_sha" ]] || {
  echo 'NeMo-Speech.cpp did not resolve to the pinned source commit.' >&2
  exit 1
}
git -C "$nemo_source" submodule update --init --depth 1 ggml
[[ $(git -C "$nemo_source/ggml" rev-parse HEAD) == "$ggml_sha" ]] || {
  echo 'ggml did not resolve to the pinned submodule commit.' >&2
  exit 1
}
git -C "$nemo_source" apply --check "$patch"
git -C "$nemo_source" apply "$patch"

# Generic arm64 output is required; an M4-specific -mcpu=native build would
# fail on older Apple Silicon Macs even if it benchmarked well locally.
cmake -S "$nemo_source" -B "$nemo_build" -G 'Unix Makefiles' \
  "${sdk_args[@]}" -DCMAKE_POLICY_VERSION_MINIMUM=3.5 \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_OSX_ARCHITECTURES=arm64 \
  -DCMAKE_OSX_DEPLOYMENT_TARGET=13.0 -DGGML_NATIVE=OFF -DGGML_METAL=ON \
  -DNEMO_SPEECH_GGML_PATCHED=OFF -DNEMO_SPEECH_BUILD_ASR=ON \
  -DNEMO_SPEECH_BUILD_DIAR=OFF -DNEMO_SPEECH_BUILD_TTS=OFF \
  -DNEMO_SPEECH_BUILD_NMT=OFF -DNEMO_SPEECH_BUILD_CLI=ON \
  -DNEMO_SPEECH_BUILD_MIC_CAPTURE=OFF -DNEMO_SPEECH_BUILD_HTTP=OFF \
  -DNEMO_SPEECH_BUILD_GRPC=OFF -DNEMO_SPEECH_BUILD_TESTS=OFF \
  "-DNEMO_SPEECH_DEPENDENCY_PREFIX=$deps" "-DCMAKE_PREFIX_PATH=$sp_prefix"
cmake --build "$nemo_build" --parallel "$jobs"
cmake --install "$nemo_build" --prefix "$stage"

for required in bin/nemo-speech lib/libnemo_speech_asr.dylib lib/libggml-metal.dylib \
                share/licenses/nemo-speech/LICENSE \
                share/licenses/nemo-speech/third_party/sentencepiece/LICENSE; do
  [[ -f "$stage/$required" ]] || {
    echo "Experimental runtime is missing $required" >&2
    exit 1
  }
done
"$stage/bin/nemo-speech" --version
"$stage/bin/nemo-speech" doctor --json | python3 -c \
  'import json,sys; d=json.load(sys.stdin); assert d["features"]["backend_metal"] and d["accelerator_available"]'
mv "$stage" "$output"
echo "Experimental Pianissimo Metal runtime ready: $output"
