#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
verifier="$root/scripts/verify-macos-release.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

make_plist() {
  plutil -create xml1 "$1"
}

make_plist "$tmp/true.plist"
/usr/libexec/PlistBuddy -c 'Add :com.apple.security.device.audio-input bool true' "$tmp/true.plist"
"$verifier" --check-entitlements-plist "$tmp/true.plist"

make_plist "$tmp/false.plist"
/usr/libexec/PlistBuddy -c 'Add :com.apple.security.device.audio-input bool false' "$tmp/false.plist"
if "$verifier" --check-entitlements-plist "$tmp/false.plist"; then
  echo "False audio-input entitlement unexpectedly passed" >&2
  exit 1
fi

make_plist "$tmp/missing.plist"
if "$verifier" --check-entitlements-plist "$tmp/missing.plist"; then
  echo "Missing audio-input entitlement unexpectedly passed" >&2
  exit 1
fi

echo "Audio-input entitlement verifier tests passed"
