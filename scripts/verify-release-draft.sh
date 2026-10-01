#!/usr/bin/env bash
# Automated release verification (#266). Runs the checks that used to be done by
# hand against a draft (or published) Sagascript release and prints a checklist.
#
#   verify-release-draft.sh TAG [--repo OWNER/REPO] [--skip-smoke]
#       Downloads TAG (and windows-beta-VERSION, when present) with `gh`.
#   verify-release-draft.sh --assets-dir DIR --version V --sha FULL_SHA [--repo OWNER/REPO] [--skip-smoke]
#       Verifies already-downloaded assets: DIR/release (macOS + Windows arm64),
#       DIR/windows-beta (Windows x64), optional DIR/evidence/windows-ARCH/
#       windows-acceptance-ps51.json.
#
# Needs macOS (codesign, spctl, stapler, hdiutil) and python3. Writes a Markdown
# summary to $GITHUB_STEP_SUMMARY when set. Exit status is non-zero if any check fails.
set -uo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
repo=Magnus-Gille/sagascript
skip_smoke=0
assets_dir=
version=
sha=
tag=

while [[ $# -gt 0 ]]; do
  case $1 in
    --repo) repo=${2:?}; shift 2 ;;
    --assets-dir) assets_dir=${2:?}; shift 2 ;;
    --version) version=${2:?}; shift 2 ;;
    --sha) sha=${2:?}; shift 2 ;;
    --skip-smoke) skip_smoke=1; shift ;;
    -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
    -*) echo "Unknown option: $1" >&2; exit 2 ;;
    *) [[ -z $tag ]] || { echo "Unexpected argument: $1" >&2; exit 2; }; tag=$1; shift ;;
  esac
done

[[ "$(uname -s)" == Darwin ]] || { echo "This script needs macOS" >&2; exit 2; }
work=$(mktemp -d)
mounted=
cleanup() {
  [[ -n $mounted ]] && hdiutil detach -quiet "$mounted" >/dev/null 2>&1
  rm -rf "$work"
}
trap cleanup EXIT

if [[ -n $tag ]]; then
  [[ -z $assets_dir ]] || { echo "Give either TAG or --assets-dir" >&2; exit 2; }
  [[ $tag =~ ^v([0-9]+\.[0-9]+\.[0-9]+)$ ]] || { echo "Tag must look like vX.Y.Z: $tag" >&2; exit 2; }
  version=${BASH_REMATCH[1]}
  sha=$(gh api "repos/$repo/commits/$tag" --jq .sha) || { echo "Cannot resolve $tag" >&2; exit 1; }
  assets_dir=$work/assets
  mkdir -p "$assets_dir"
  gh release download "$tag" -R "$repo" -D "$assets_dir/release" || exit 1
  if gh release view "windows-beta-$version" -R "$repo" >/dev/null 2>&1; then
    gh release download "windows-beta-$version" -R "$repo" -D "$assets_dir/windows-beta" || exit 1
  fi
else
  [[ -n $assets_dir && -n $version && -n $sha ]] || { echo "Need TAG, or --assets-dir with --version and --sha" >&2; exit 2; }
fi
[[ $sha =~ ^[0-9a-f]{40}$ ]] || { echo "Expected a full 40-character SHA, got: $sha" >&2; exit 2; }
short=${sha:0:7}
rel=$assets_dir/release
beta=$assets_dir/windows-beta

results=()
failed=0
run_check() {
  local name=$1 log
  shift
  log=$(mktemp "$work/check.XXXXXX")
  if ( set -euo pipefail; "$@" ) >"$log" 2>&1; then
    results+=("PASS|$name")
    echo "PASS  $name"
  else
    results+=("FAIL|$name")
    failed=1
    echo "FAIL  $name"
    sed 's/^/      /' "$log" | tail -25
  fi
}
skip_check() { results+=("SKIP|$1"); echo "SKIP  $1"; }

# --- checks -----------------------------------------------------------------

check_mac_assets_present() {
  for f in Sagascript.dmg Sagascript.app.tar.gz Sagascript.app.tar.gz.sig latest.json SHA256SUMS; do
    [[ -s "$rel/$f" ]] || { echo "missing or empty: $f"; return 1; }
  done
}

# Every listed file must exist and match; `--check` alone would pass an empty
# list, so also require the expected names to be covered.
check_sums() { # DIR SUMS_FILE NAMES...
  local dir=$1 sums=$2
  shift 2
  (cd "$dir" && shasum -a 256 -c "$sums")
  local name
  for name in "$@"; do
    grep -Eqi "^[0-9a-f]{64} [ *]?${name//./\\.}\$" "$dir/$sums" || { echo "$sums does not cover $name"; return 1; }
  done
}

check_mac_sums() {
  check_sums "$rel" SHA256SUMS Sagascript.dmg Sagascript.app.tar.gz Sagascript.app.tar.gz.sig latest.json
}

check_latest_json() {
  python3 - "$rel" "$version" "$repo" <<'PY'
import json, sys
rel, version, repo = sys.argv[1:]
d = json.load(open(f"{rel}/latest.json"))
assert d["version"] == version, f"latest.json version {d['version']} != {version}"
p = d["platforms"]["darwin-aarch64"]
want = f"https://github.com/{repo}/releases/download/v{version}/Sagascript.app.tar.gz"
assert p["url"] == want, f"latest.json url {p['url']} != {want}"
sig = open(f"{rel}/Sagascript.app.tar.gz.sig").read().strip()
assert p["signature"] == sig, "latest.json signature differs from Sagascript.app.tar.gz.sig"
print("latest.json OK:", d["version"], p["url"])
PY
}

# Runs in the parent shell (not via run_check) so `cleanup` can detach the image.
mount_and_copy_app() {
  mkdir -p "$work/dmg"
  hdiutil attach -quiet -nobrowse -readonly -mountpoint "$work/dmg" "$rel/Sagascript.dmg" || return 1
  mounted=$work/dmg
  ditto "$mounted/Sagascript.app" "$work/Sagascript.app"
}
app=$work/Sagascript.app
dmg=$rel/Sagascript.dmg
binary=$app/Contents/MacOS/sagascript
host=$app/Contents/Resources/EngineHost/sagascript-engine-host

check_codesign() { codesign --verify --deep --strict --verbose=2 "$app"; }
check_identity() {
  local sig
  sig=$(codesign -dvvv "$app" 2>&1)
  grep -q '^TeamIdentifier=7C6WF6GFZ4$' <<<"$sig" || { echo "Team ID is not 7C6WF6GFZ4"; return 1; }
  grep -Eq '^Authority=Developer ID Application:.*\(7C6WF6GFZ4\)$' <<<"$sig" || { echo "not Developer ID Application for 7C6WF6GFZ4"; return 1; }
  grep -Eq '^CodeDirectory .*flags=.*\(runtime\)' <<<"$sig" || { echo "hardened runtime missing"; return 1; }
  echo "Team ID 7C6WF6GFZ4, Developer ID Application, hardened runtime"
}
check_spctl() {
  local out
  out=$(spctl --assess --type execute --verbose=4 "$app" 2>&1)
  echo "$out"
  grep -q 'source=Notarized Developer ID' <<<"$out"
}
check_stapler_app() { xcrun stapler validate "$app"; }
check_stapler_dmg() { xcrun stapler validate "$dmg"; }
check_release_verifier() { "$repo_root/scripts/verify-macos-release.sh" "$app" "$dmg" "$version" "$sha"; }
check_versions() {
  local out host_out
  out=$("$binary" --version)
  echo "app/CLI: $out"
  [[ "$out" == "sagascript $version (git $short, "* ]] || { echo "expected 'sagascript $version (git $short, ...'"; return 1; }
  [[ "$out" != *dirty* ]] || { echo "dirty=true build"; return 1; }
  host_out=$("$host" --version)
  echo "host: $host_out"
  [[ "$host_out" == *"($sha,"* && "$host_out" == *"dirty=false"* ]] || { echo "engine host is not the exact clean release SHA"; return 1; }
}
check_updater_archive() {
  local dest=$work/updater
  mkdir -p "$dest"
  tar -xzf "$rel/Sagascript.app.tar.gz" -C "$dest"
  xcrun stapler validate "$dest/Sagascript.app"
  spctl --assess --type execute --verbose=2 "$dest/Sagascript.app"
  [[ $(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$dest/Sagascript.app/Contents/Info.plist") == "$version" ]]
}
check_pianissimo_smoke() {
  "$repo_root/scripts/smoke-pianissimo-installed.sh" "$binary" "$repo_root/test-audio/swedish-fleurs-hongkong.wav" hongkong
}

check_windows() { # ARCH DIR SUMS_FILE_DIR
  local arch=$1 dir=$2
  local names=("Sagascript-Windows-$arch-CLI.exe" "Sagascript-Windows-$arch-Portable.exe"
    "Sagascript-Windows-$arch-Setup.exe" "Sagascript-Windows-$arch.msi")
  # The ARM64 set also carries the portable zip (#269): 5 files; x64 has 4.
  [[ $arch == arm64 ]] && names+=("Sagascript-Windows-$arch-Portable.zip")
  check_sums "$dir" "SHA256SUMS-Windows-$arch" "${names[@]}"
  local count
  count=$(grep -c . "$dir/SHA256SUMS-Windows-$arch")
  [[ $count == ${#names[@]} ]] || { echo "SHA256SUMS-Windows-$arch lists $count files, expected ${#names[@]}"; return 1; }
}
check_windows_evidence() { # ARCH JSON
  python3 - "$2" "$version" "$short" <<'PY'
import json, sys
path, version, short = sys.argv[1:]
d = json.load(open(path))
assert d["expected_version"] == version, d["expected_version"]
rv = d["reported_version"]
assert rv.startswith(f"sagascript {version} (git {short}, "), rv
assert "dirty" not in rv, rv
assert d["automated_cli_acceptance"] == "pass"
print("acceptance OK:", rv)
PY
}

# --- run ----------------------------------------------------------------------

echo "Verifying Sagascript $version at $sha ($assets_dir)"
run_check "macOS assets present (dmg, tar.gz, sig, latest.json, SHA256SUMS)" check_mac_assets_present
run_check "SHA256SUMS matches macOS assets" check_mac_sums
run_check "latest.json version, URL and signature" check_latest_json
if mount_and_copy_app >"$work/mount.log" 2>&1 && [[ -d $app ]]; then
  results+=("PASS|Mount DMG and copy app"); echo "PASS  Mount DMG and copy app"
  run_check "codesign --verify --deep --strict" check_codesign
  run_check "Team ID 7C6WF6GFZ4, Developer ID, hardened runtime" check_identity
  run_check "spctl: Notarized Developer ID" check_spctl
  run_check "stapler validate (app)" check_stapler_app
  run_check "stapler validate (dmg)" check_stapler_dmg
  run_check "Release verifier (arch, engine host, entitlements, Gatekeeper)" check_release_verifier
  run_check "app/CLI/host --version = $version + ${short} (not dirty)" check_versions
  run_check "Updater archive: stapled, Gatekeeper, version" check_updater_archive
  if [[ $skip_smoke == 1 ]]; then
    skip_check "Pianissimo smoke (engine doctor + exact-word transcription)"
  else
    run_check "Pianissimo smoke (engine doctor + transcription contains 'hongkong')" check_pianissimo_smoke
  fi
else
  results+=("FAIL|Mount DMG and copy app"); failed=1
  echo "FAIL  Mount DMG and copy app"; sed 's/^/      /' "$work/mount.log"
fi

for arch in arm64 x64; do
  dir=$rel
  [[ $arch == x64 ]] && dir=$beta
  if [[ -f "$dir/SHA256SUMS-Windows-$arch" ]]; then
    run_check "Windows $arch: SHA256 file matches artifacts" check_windows "$arch" "$dir"
  else
    skip_check "Windows $arch: no SHA256SUMS-Windows-$arch in the draft"
  fi
  evidence=$assets_dir/evidence/windows-$arch/windows-acceptance-ps51.json
  if [[ -f $evidence ]]; then
    run_check "Windows $arch: acceptance JSON reports $version @ ${short}" check_windows_evidence "$arch" "$evidence"
  else
    skip_check "Windows $arch: acceptance JSON (only available from the prebuild artifacts)"
  fi
done

summary() {
  echo "### Release draft verification: Sagascript $version (\`$short\`)"
  echo
  echo "| Result | Check |"
  echo "|---|---|"
  local r
  for r in "${results[@]}"; do echo "| ${r%%|*} | ${r#*|} |"; done
}
summary
[[ -z ${GITHUB_STEP_SUMMARY:-} ]] || summary >> "$GITHUB_STEP_SUMMARY"
if [[ $failed == 0 ]]; then echo "All checks passed"; else echo "VERIFICATION FAILED" >&2; fi
exit "$failed"
