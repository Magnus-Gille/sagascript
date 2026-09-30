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

if [[ $# -ne 4 ]]; then
  echo "Usage: $0 /path/to/Sagascript.app /path/to/Sagascript.dmg VERSION FULL_GIT_SHA" >&2
  exit 2
fi

app=$1
dmg=$2
version=$3
expected_sha=$4
expected_identifier=ai.gille.sagascript
expected_team_id=7C6WF6GFZ4

[[ "$expected_sha" =~ ^[0-9a-f]{40}$ ]] || { echo "Expected a full 40-character release SHA, got: $expected_sha" >&2; exit 2; }
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

# The Pianissimo engine host lives inside the app, so the app's notarization
# ticket covers it; it must still be individually signed with the hardened
# runtime, the production team, and carry the exact release revision.
host="$app/Contents/Resources/EngineHost/sagascript-engine-host"
[[ -x "$host" ]] || {
  echo "Bundled engine host is missing or not executable: $host" >&2
  exit 1
}
host_architectures=$(lipo -archs "$host")
[[ "$host_architectures" == "arm64" ]] || {
  echo "Unexpected engine host architectures: $host_architectures (wanted arm64)" >&2
  exit 1
}
codesign --verify --strict --verbose=2 "$host"
host_signature=$(codesign -d --verbose=4 "$host" 2>&1)
grep -Eq '^CodeDirectory .*flags=0x10000\(runtime\)' <<<"$host_signature" || {
  echo "Engine host is not signed with the hardened runtime (flags=0x10000(runtime))" >&2
  exit 1
}
grep -q "^TeamIdentifier=${expected_team_id}$" <<<"$host_signature" || {
  echo "Engine host is not signed by team ${expected_team_id}" >&2
  exit 1
}
grep -q '^Authority=Developer ID Application:' <<<"$host_signature" || {
  echo "Engine host is not signed with Developer ID Application" >&2
  exit 1
}
"$(dirname "$0")/check-engine-host-identity.sh" "$host" "$expected_sha"
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
