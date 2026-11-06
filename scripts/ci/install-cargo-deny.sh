#!/usr/bin/env bash
# Install the pinned cargo-deny release as the official prebuilt binary instead of compiling it
# with `cargo install` (SAGA-546: over a minute of download plus several of compile on a cache
# miss). The archive is checked against a pinned SHA-256 before it is unpacked. Linux x86_64 only
# (the static musl build). Keep VERSION in step with CARGO_DENY_VERSION in scripts/verify.mjs; a
# test holds them together. To bump: download the new asset from the release page and pin the
# digest of the bytes you downloaded (`shasum -a 256`), not one copied from another source.
set -euo pipefail

VERSION=0.20.2
SHA256=9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f
NAME="cargo-deny-${VERSION}-x86_64-unknown-linux-musl"
URL="https://github.com/EmbarkStudios/cargo-deny/releases/download/${VERSION}/${NAME}.tar.gz"
DEST="${CARGO_DENY_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/cargo-deny-bin}"
# Test hook: a local copy of the archive replaces the download (the checksum is still enforced).
ARCHIVE_OVERRIDE="${CARGO_DENY_ARCHIVE:-}"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
if [ -n "$ARCHIVE_OVERRIDE" ]; then
  cp "$ARCHIVE_OVERRIDE" "$work/cargo-deny.tar.gz"
else
  curl --fail --silent --show-error --location --retry 3 --connect-timeout 20 --max-time 120 \
    --output "$work/cargo-deny.tar.gz" "$URL"
fi
echo "${SHA256}  ${work}/cargo-deny.tar.gz" | sha256sum --check --strict -
tar -xzf "$work/cargo-deny.tar.gz" -C "$work" "${NAME}/cargo-deny"
mkdir -p "$DEST"
install -m 0755 "$work/${NAME}/cargo-deny" "$DEST/cargo-deny"
if [ -n "${GITHUB_PATH:-}" ]; then
  echo "$DEST" >> "$GITHUB_PATH"
fi
"$DEST/cargo-deny" --version
