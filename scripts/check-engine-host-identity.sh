#!/usr/bin/env bash
# Usage: check-engine-host-identity.sh HOST_BINARY EXPECTED_40_CHAR_SHA
# Fails unless `sagascript-engine-host --version` reports exactly that SHA.
set -euo pipefail

[[ $# -eq 2 ]] || { echo "Usage: $0 HOST_BINARY EXPECTED_SHA" >&2; exit 2; }
host=$1
expected=$2
[[ "$expected" =~ ^[0-9a-f]{40}$ ]] || { echo "Expected a full 40-character SHA, got: $expected" >&2; exit 2; }

output=$("$host" --version)
if [[ "$output" != *"($expected,"* ]]; then
  echo "Engine host revision mismatch: wanted $expected, got: $output" >&2
  exit 1
fi
echo "Engine host identity OK: $output"
