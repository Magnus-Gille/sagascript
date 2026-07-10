#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "Usage: $0 /path/to/Sagascript.app /path/to/Sagascript.dmg VERSION" >&2
  exit 2
fi

app=$1
dmg=$2
version=$3
expected_identifier=ai.gille.sagascript

[[ -d "$app" ]] || { echo "Missing app bundle: $app" >&2; exit 1; }
[[ -f "$dmg" ]] || { echo "Missing disk image: $dmg" >&2; exit 1; }

info="$app/Contents/Info.plist"
actual_identifier=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$info")
actual_version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$info")
[[ "$actual_identifier" == "$expected_identifier" ]] || {
  echo "Unexpected bundle identifier: $actual_identifier" >&2
  exit 1
}
[[ "$actual_version" == "$version" ]] || {
  echo "Unexpected bundle version: $actual_version (wanted $version)" >&2
  exit 1
}

codesign --verify --deep --strict --verbose=2 "$app"
signature=$(codesign -dvvv "$app" 2>&1)
grep -q '^Authority=Developer ID Application:' <<<"$signature" || {
  echo "App is not signed with Developer ID Application" >&2
  exit 1
}
grep -Eq '^TeamIdentifier=.+$' <<<"$signature" || {
  echo "Signed app has no TeamIdentifier" >&2
  exit 1
}
grep -Eq '^CodeDirectory .*flags=.*\(runtime\)' <<<"$signature" || {
  echo "Hardened runtime is not enabled" >&2
  exit 1
}

entitlements=$(mktemp)
trap 'rm -f "$entitlements"' EXIT
codesign -d --entitlements :- "$app" >"$entitlements" 2>/dev/null
[[ "$(plutil -extract com.apple.security.device.audio-input raw "$entitlements")" == "true" ]] || {
  echo "Signed app is missing the audio-input entitlement" >&2
  exit 1
}

xcrun stapler validate "$app"
xcrun stapler validate "$dmg"
spctl --assess --type execute --verbose=2 "$app"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"

echo "Verified signed, hardened, notarized Sagascript ${version} (${expected_identifier})"
