#!/usr/bin/env bash
# Build identity of an engine host (#367), printed as three lines:
#
#   version=<the app version in src-tauri/tauri.conf.json>
#   git_sha=<the full 40-character HEAD revision>
#   dirty=<true|false>
#
# dirty is true when `git status` lists anything besides the generated files in
# scripts/build-identity-generated-paths.txt (gitignored files never count). That list is the
# one definition of a dirty tree: the ONNX host's build script
# (src-tauri/engine-host/ort/build_identity.rs) and the Windows packaging gate
# (scripts/ci-build-identity.mjs) read the same file instead of invoking this script, because
# both run on Windows, where `bash` is not reliably Git's. scripts/build-engine-host.sh bakes the
# output into BuildInfo.swift; scripts/check-engine-host-identity.sh --release rejects a dirty host.
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/.." && pwd)

version=$(sed -n 's/^[[:space:]]*"version":[[:space:]]*"\([^"]*\)".*/\1/p' "$REPO_ROOT/src-tauri/tauri.conf.json" | head -n 1)
if [[ -z "$version" ]]; then
  echo "Could not read version from src-tauri/tauri.conf.json" >&2
  exit 1
fi

git_sha=$(git -C "$REPO_ROOT" rev-parse HEAD)
if [[ ! "$git_sha" =~ ^[0-9a-f]{40}$ ]]; then
  echo "git rev-parse HEAD did not return a full SHA" >&2
  exit 1
fi

allowed=$(tr -d '\r' < "$SCRIPT_DIR/build-identity-generated-paths.txt" | grep -v -e '^[[:space:]]*#' -e '^[[:space:]]*$' || true)
status=$(git -C "$REPO_ROOT" status --porcelain=v1 --untracked-files=all)
dirty=false
while IFS= read -r line; do
  [[ -z "$line" ]] && continue
  # `XY path`; a quoted or renamed path is never one of the generated files.
  if ! grep -qxF -e "${line:3}" <<<"$allowed"; then
    dirty=true
    break
  fi
done <<<"$status"

printf 'version=%s\ngit_sha=%s\ndirty=%s\n' "$version" "$git_sha" "$dirty"
