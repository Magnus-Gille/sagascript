#!/usr/bin/env bash
set -euo pipefail

verify_audio_input_entitlement() {
  local value
  value=$(plutil -extract 'com\.apple\.security\.device\.audio-input' raw "$1" 2>/dev/null) || return 1
  [[ "$value" == "true" ]]
}

if [[ ${1:-} == "--check-entitlements-plist" ]]; then
  [[ $# -eq 2 ]] || { echo "Usage: $0 --check-entitlements-plist /path/to/entitlements.plist" >&2; exit 2; }
  verify_audio_input_entitlement "$2"
  exit
fi

if [[ $# -ne 3 ]]; then
  echo "Usage: $0 /path/to/Sagascript.app /path/to/Sagascript.dmg VERSION" >&2
  exit 2
fi

app=$1
dmg=$2
version=$3
expected_identifier=ai.gille.sagascript
expected_team_id=7C6WF6GFZ4

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

binary="$app/Contents/MacOS/sagascript"
actual_architectures=$(lipo -archs "$binary")
[[ "$actual_architectures" == "arm64" ]] || {
  echo "Unexpected release architectures: $actual_architectures (wanted arm64)" >&2
  exit 1
}

codesign --verify --deep --strict --verbose=2 "$app"
runtime="$app/Contents/Resources/PianissimoRuntime"
[[ -x "$runtime/bin/nemo-speech" ]] || {
  echo "Bundled Pianissimo native executable is missing" >&2
  exit 1
}
[[ ! -e "$runtime/python/bin/python3.12" ]] || {
  echo "Unexpected Python runtime remains in app bundle" >&2
  exit 1
}
for native in "$runtime/bin/nemo-speech" "$runtime"/lib/*.dylib; do
  [[ -e "$native" ]] || { echo "Missing Pianissimo native component: $native" >&2; exit 1; }
  codesign --verify --strict "$native"
done
signature=$(codesign -dvvv "$app" 2>&1)
grep -q '^Authority=Developer ID Application:' <<<"$signature" || {
  echo "App is not signed with Developer ID Application" >&2
  exit 1
}
grep -q "^TeamIdentifier=${expected_team_id}$" <<<"$signature" || {
  actual_team_id=$(sed -n 's/^TeamIdentifier=//p' <<<"$signature")
  echo "Unexpected signing Team ID: ${actual_team_id:-missing} (wanted ${expected_team_id})" >&2
  exit 1
}
grep -Eq "^Authority=Developer ID Application:.*\\(${expected_team_id}\\)$" <<<"$signature" || {
  echo "Developer ID authority does not belong to team ${expected_team_id}" >&2
  exit 1
}
grep -Eq '^CodeDirectory .*flags=.*\(runtime\)' <<<"$signature" || {
  echo "Hardened runtime is not enabled" >&2
  exit 1
}

entitlements=$(mktemp)
trap 'rm -f "$entitlements"' EXIT
codesign -d --entitlements :- "$app" >"$entitlements" 2>/dev/null
verify_audio_input_entitlement "$entitlements" || {
  echo "Signed app is missing the audio-input entitlement" >&2
  exit 1
}
if [[ $(plutil -extract 'com\.apple\.security\.cs\.allow-unsigned-executable-memory' raw "$entitlements" 2>/dev/null || true) == "true" ]]; then
  echo "Signed app unexpectedly allows unsigned executable memory" >&2
  exit 1
fi

xcrun stapler validate "$app"
xcrun stapler validate "$dmg"
spctl --assess --type execute --verbose=2 "$app"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"

echo "Verified signed, hardened, notarized Sagascript ${version} (${expected_identifier}, Team ID ${expected_team_id})"
