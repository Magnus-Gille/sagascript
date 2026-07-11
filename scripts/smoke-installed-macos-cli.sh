#!/usr/bin/env bash
set -euo pipefail

expected_version=${1:?usage: smoke-installed-macos-cli.sh VERSION GIT_HASH AUDIO_FILE}
expected_git_hash=${2:?usage: smoke-installed-macos-cli.sh VERSION GIT_HASH AUDIO_FILE}
audio_file=${3:?usage: smoke-installed-macos-cli.sh VERSION GIT_HASH AUDIO_FILE}

installed_cli=/usr/local/bin/sagascript
expected_executable=/Applications/Sagascript.app/Contents/MacOS/sagascript

[[ -L "$installed_cli" ]] || {
  echo "$installed_cli must be a symlink into the installed app bundle" >&2
  exit 1
}

actual_target=$(readlink "$installed_cli")
[[ "$actual_target" == "$expected_executable" ]] || {
  echo "$installed_cli points to $actual_target, expected $expected_executable" >&2
  exit 1
}

version_output=$("$installed_cli" --version)
[[ "$version_output" == *"$expected_version"* ]] || {
  echo "Installed CLI reports the wrong version: $version_output" >&2
  exit 1
}
[[ "$version_output" == *"git $expected_git_hash"* ]] || {
  echo "Installed CLI reports the wrong source revision: $version_output" >&2
  exit 1
}

"$installed_cli" download-model nb-whisper-tiny
result=${RUNNER_TEMP:-/tmp}/sagascript-installed-cli-smoke.json
"$installed_cli" transcribe \
  --language no \
  --model nb-whisper-tiny \
  --beam 0 \
  --json \
  "$audio_file" > "$result"

python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); assert "storting" in d["text"].lower(); assert d["language"] == "no"; assert d["segments"]' "$result"

echo "Verified installed CLI: $version_output"
