#!/usr/bin/env bash
# Build the Swift Core ML engine host and stage it at the stable path that
# scripts/tauri-engine-host-bundle.json bundles into
# Sagascript.app/Contents/Resources/EngineHost/. Apple Silicon only.
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
REPO_ROOT=$(cd "$SCRIPT_DIR/.." && pwd)
STAGE_DIR="$REPO_ROOT/build/engine-host"

built=$("$SCRIPT_DIR/build-engine-host.sh" | tail -n 1)
[[ -x "$built" ]] || { echo "Engine host binary not found: $built" >&2; exit 1; }

archs=$(lipo -archs "$built")
[[ "$archs" == "arm64" ]] || {
  echo "Engine host must be arm64 only, got: $archs" >&2
  exit 1
}

mkdir -p "$STAGE_DIR"
rm -f "$STAGE_DIR/sagascript-engine-host"
cp "$built" "$STAGE_DIR/sagascript-engine-host"
chmod 755 "$STAGE_DIR/sagascript-engine-host"

# Fail early when the staged binary does not carry this checkout's revision.
expected=$(git -C "$REPO_ROOT" rev-parse HEAD)
"$SCRIPT_DIR/check-engine-host-identity.sh" "$STAGE_DIR/sagascript-engine-host" "$expected"
echo "$STAGE_DIR/sagascript-engine-host"
