#!/usr/bin/env bash
set -euo pipefail

# Stage NVIDIA NeMo-Speech.cpp's pinned Apple Silicon Metal runtime.
# The archive is verified before it is unpacked.  Set
# PIANISSIMO_NATIVE_ARCHIVE to use a local archive in offline builds/tests.

repo_root=$(cd "$(dirname "$0")/.." && pwd)
output=${1:-"${PIANISSIMO_NATIVE_OUTPUT:-$repo_root/build/pianissimo-native-runtime}"}
archive_override=${PIANISSIMO_NATIVE_ARCHIVE:-}

[[ $(uname -s) == Darwin && $(uname -m) == arm64 ]] || {
  echo "Pianissimo native runtime packaging requires macOS arm64" >&2
  exit 1
}

archive_name="nemo-speech-0.1.0-macos-aarch64-metal.tar.gz"
archive_url="https://github.com/NVIDIA/NeMo-Speech.cpp/releases/download/v0.1.0/$archive_name"
archive_sha256="f1dff4f9dd9c96214f8cb78b982812459132df8a4ad1a42409fd94de4a366244"

[[ ! -e "$output" ]] || {
  echo "Native runtime output already exists: $output" >&2
  exit 1
}

scratch=$(mktemp -d "${TMPDIR:-/tmp}/sagascript-pianissimo-native.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
archive="$scratch/$archive_name"

if [[ -n "$archive_override" ]]; then
  [[ -f "$archive_override" ]] || {
    echo "Native runtime archive does not exist: $archive_override" >&2
    exit 1
  }
  cp -p "$archive_override" "$archive"
else
  command -v curl >/dev/null || {
    echo "curl is required to download the native runtime" >&2
    exit 1
  }
  curl --fail --location --retry 3 --connect-timeout 20 --output "$archive" "$archive_url"
fi

actual_sha256=$(shasum -a 256 "$archive" | awk '{print $1}')
[[ "$actual_sha256" == "$archive_sha256" ]] || {
  echo "Native runtime archive checksum mismatch: expected $archive_sha256, found $actual_sha256" >&2
  exit 1
}

# Validate archive member names and links before extraction.  In particular,
# tar must not be allowed to write through an archive-provided symlink.
python3 - "$archive" <<'PY'
import posixpath
import sys
import tarfile

archive = sys.argv[1]
root = "nemo-speech"
members = {}
symlinks = {}

with tarfile.open(archive, "r:gz") as tar:
    for member in tar.getmembers():
        name = member.name
        if name in members:
            raise SystemExit(f"duplicate archive member: {name}")
        if name != root and not name.startswith(root + "/"):
            raise SystemExit(f"archive member outside {root}/: {name}")
        if name.startswith("/") or any(part in ("", ".", "..") for part in name.split("/")):
            raise SystemExit(f"unsafe archive member path: {name}")
        if not (member.isdir() or member.isfile() or member.issym() or member.islnk()):
            raise SystemExit(f"unsupported archive member type: {name}")
        members[name] = member
        if member.issym() or member.islnk():
            link = member.linkname
            if link.startswith("/"):
                raise SystemExit(f"absolute archive link: {name} -> {link}")
            target = posixpath.normpath(posixpath.join(posixpath.dirname(name), link))
            if target != root and not target.startswith(root + "/"):
                raise SystemExit(f"archive link escapes {root}/: {name} -> {link}")
            symlinks[name] = target

    # No member may be placed below an archive symlink.  That would make a
    # normal-looking path resolve outside the extraction directory.
    for name in members:
        parts = name.split("/")
        prefix = []
        for part in parts[:-1]:
            prefix.append(part)
            if "/".join(prefix) in symlinks:
                raise SystemExit(f"archive member traverses symlink: {name}")

    # Resolve symlink chains lexically and reject a link that eventually
    # leaves the archive root.  The member-prefix check above handles links
    # used as directories.
    for name, target in symlinks.items():
        seen = set()
        while target in symlinks:
            if target in seen:
                raise SystemExit(f"symlink cycle in archive at: {name}")
            seen.add(target)
            target = symlinks[target]
            if target != root and not target.startswith(root + "/"):
                raise SystemExit(f"archive link escapes {root}/: {name}")
PY

mkdir -p "$scratch/extracted"
tar -xzf "$archive" -C "$scratch/extracted"
stage_source="$scratch/extracted/nemo-speech"
[[ -d "$stage_source" ]] || {
  echo "Native runtime archive has no nemo-speech directory" >&2
  exit 1
}

mkdir -p "$output"
cp -a "$stage_source/." "$output/"

[[ -x "$output/bin/nemo-speech" ]] || {
  echo "Native runtime executable missing or not executable: $output/bin/nemo-speech" >&2
  exit 1
}
for library in \
  lib/libnemo_speech_asr.dylib \
  lib/libnemo_speech_asr_c.dylib \
  lib/libggml.dylib \
  lib/libggml-base.dylib \
  lib/libggml-cpu.dylib \
  lib/libggml-metal.dylib \
  lib/libllama.dylib; do
  [[ -e "$output/$library" ]] || {
    echo "Native runtime library missing: $output/$library" >&2
    exit 1
  }
done
for notice in \
  share/licenses/nemo-speech/LICENSE \
  share/licenses/nemo-speech/NOTICE \
  share/licenses/nemo-speech/THIRD_PARTY_NOTICES.md; do
  [[ -f "$output/$notice" ]] || {
    echo "Native runtime notice missing: $output/$notice" >&2
    exit 1
  }
done

# Check the copied tree too: links must resolve inside the staged runtime and
# every link target must exist before the directory is handed to the bundler.
python3 - "$output" <<'PY'
import os
import sys
from pathlib import Path

root = Path(sys.argv[1]).resolve(strict=True)
for path in root.rglob("*"):
    if not path.is_symlink():
        continue
    resolved = Path(os.path.realpath(path))
    try:
        resolved.relative_to(root)
    except ValueError as exc:
        raise SystemExit(f"staged symlink escapes runtime: {path} -> {os.readlink(path)}") from exc
    if not resolved.exists():
        raise SystemExit(f"staged symlink target is missing: {path} -> {os.readlink(path)}")
PY

DYLD_LIBRARY_PATH="$output/lib${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}" \
  "$output/bin/nemo-speech" --version
echo "Pianissimo native runtime ready: $output"
