#!/usr/bin/env bash
# Usage: check-engine-host-identity.sh [--release] HOST_BINARY EXPECTED_40_CHAR_SHA
# Fails unless `sagascript-engine-host --version` reports exactly that SHA. With --release (signed
# and released builds) it also fails unless the host reports dirty=false, i.e. it was built from
# a clean tree (scripts/engine-host-identity.sh).
set -euo pipefail

release=false
if [[ "${1:-}" == "--release" ]]; then
  release=true
  shift
fi
[[ $# -eq 2 ]] || { echo "Usage: $0 [--release] HOST_BINARY EXPECTED_SHA" >&2; exit 2; }
host=$1
expected=$2
[[ "$expected" =~ ^[0-9a-f]{40}$ ]] || { echo "Expected a full 40-character SHA, got: $expected" >&2; exit 2; }

output=$("$host" --version)
if [[ "$output" != *"($expected,"* ]]; then
  echo "Engine host revision mismatch: wanted $expected, got: $output" >&2
  exit 1
fi
if [[ "$release" == true && "$output" != *"dirty=false"* ]]; then
  echo "Engine host was not built from a clean tree (release builds need dirty=false): $output" >&2
  exit 1
fi
echo "Engine host identity OK: $output"
