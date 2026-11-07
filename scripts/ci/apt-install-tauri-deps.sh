#!/usr/bin/env bash
# Install the Linux system packages the app crate links against (SAGA-546).
#
# The Azure Ubuntu mirror of the hosted runners sometimes stalls: apt fetched 60 MB in 15 minutes in
# one run, against about a minute normally. apt's own timeouts only catch a connection that goes
# silent, not a crawl, so each attempt also runs under a wall-clock `timeout`. A stuck attempt is
# killed (apt resumes partial downloads) and the retry moves off the Azure mirror. The package list
# is unchanged; only the time spent on a bad mirror is bounded.
#
# Only the network part is bounded: `update` and `install --download-only` run under `timeout`. The
# real install then runs once with `--no-download` and no short limit, so dpkg is never killed
# mid-unpack (that leaves "dpkg was interrupted" or a held lock that breaks the retry).
#
# Mirror fallback: on the hosted ubuntu images the apt sources point at `mirror+file:` with the list
# in apt-mirrors.txt (azure priority 1, then archive.ubuntu.com, then security.ubuntu.com), see
# actions/runner-images images/ubuntu/scripts/build/configure-apt-sources.sh. apt's mirror method
# fails over only on errors, not on a slow crawl, so the retry deletes the azure line from that list.
# Images that name azure.archive.ubuntu.com directly in their sources are rewritten instead.
#
# Deb cache (SAGA-546): when APT_DEB_CACHE_DIR is set, the .deb files in it (restored by the setup action
# from actions/cache) may pre-seed apt's archive directory. apt can reuse a file already in its archive
# by name and size without rehashing it, so a cached file is only seeded after a successful `update`
# and only if its SHA-256 is the one the signed package index lists for that exact package and
# version; any other file (stale, corrupt, tampered, not a regular file) is discarded and apt
# downloads as before. After the install the archive's .deb files are copied back (owned by the
# runner user) for the action to save. Seeding and export never fail the step: a missing cache, a
# failed copy or a full disk is only a warning, and the install behaves as without the cache.
set -euo pipefail

PACKAGES=(libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libasound2-dev build-essential file xdotool)
ATTEMPTS="${APT_ATTEMPTS:-3}"
ATTEMPT_SECONDS="${APT_ATTEMPT_SECONDS:-240}"
APT_OPTS=(-o Acquire::Retries=3 -o Acquire::http::Timeout=20 -o Acquire::https::Timeout=20)
SOURCES_DIR="${APT_SOURCES_DIR:-/etc/apt}"
ARCHIVE_DIR="${APT_ARCHIVE_DIR:-/var/cache/apt/archives}"
DEB_CACHE_DIR="${APT_DEB_CACHE_DIR:-}"

SEEDED=0

# Copies each cached .deb whose SHA-256 the signed index lists for its package and version into apt's
# archive. Runs once, after the first successful `update`. Never fails the script.
seed_verified_cache() {
  local f meta pkg ver sum kept=0 dropped=0
  [ -n "$DEB_CACHE_DIR" ] && [ "$SEEDED" -eq 0 ] || return 0
  SEEDED=1
  compgen -G "$DEB_CACHE_DIR/*.deb" >/dev/null || return 0
  for f in "$DEB_CACHE_DIR"/*.deb; do
    if [ ! -f "$f" ] || [ -L "$f" ]; then
      dropped=$((dropped + 1))
      continue
    fi
    if ! meta="$(dpkg-deb -f "$f" Package Version 2>/dev/null)"; then
      dropped=$((dropped + 1))
      continue
    fi
    pkg="$(printf '%s\n' "$meta" | sed -n 's/^Package: //p')"
    ver="$(printf '%s\n' "$meta" | sed -n 's/^Version: //p')"
    sum="$(sha256sum "$f" | cut -d' ' -f1)"
    if [ -n "$pkg" ] && [ -n "$ver" ] &&
      apt-cache show "${pkg}=${ver}" 2>/dev/null | grep -qx "SHA256: ${sum}" &&
      sudo cp -n "$f" "$ARCHIVE_DIR"/; then
      kept=$((kept + 1))
    else
      dropped=$((dropped + 1))
    fi
  done
  echo "Seeded apt with ${kept} verified cached packages (${dropped} discarded)"
}

# Copies the archive's .deb files to the cache directory for the action to save. Never fails the script.
export_to_cache() {
  [ -n "$DEB_CACHE_DIR" ] || return 0
  if ! mkdir -p "$DEB_CACHE_DIR" 2>/dev/null; then
    echo "could not create the apt cache directory; not caching" >&2
    return 0
  fi
  compgen -G "$ARCHIVE_DIR/*.deb" >/dev/null || return 0
  if ! { sudo cp -u "$ARCHIVE_DIR"/*.deb "$DEB_CACHE_DIR"/ && sudo chown -R "$(id -u):$(id -g)" "$DEB_CACHE_DIR"; }; then
    echo "could not export the apt archive for caching" >&2
  fi
  return 0
}

use_main_archive() {
  local file
  if [ -f "$SOURCES_DIR/apt-mirrors.txt" ]; then
    sudo sed -i '/azure\.archive\.ubuntu\.com/d' "$SOURCES_DIR/apt-mirrors.txt"
  fi
  for file in "$SOURCES_DIR/sources.list" "$SOURCES_DIR"/sources.list.d/*.sources "$SOURCES_DIR"/sources.list.d/*.list; do
    [ -f "$file" ] || continue
    sudo sed -i 's#//azure\.archive\.ubuntu\.com/#//archive.ubuntu.com/#g' "$file"
  done
}

attempt=1
while true; do
  if timeout "$ATTEMPT_SECONDS" sudo apt-get "${APT_OPTS[@]}" update; then
    seed_verified_cache || true
    if timeout "$ATTEMPT_SECONDS" sudo apt-get "${APT_OPTS[@]}" install -y --download-only "${PACKAGES[@]}"; then
      break
    fi
  fi
  if [ "$attempt" -ge "$ATTEMPTS" ]; then
    echo "apt failed after $ATTEMPTS attempts" >&2
    exit 1
  fi
  echo "apt attempt $attempt failed or exceeded ${ATTEMPT_SECONDS}s; retrying on archive.ubuntu.com" >&2
  use_main_archive
  attempt=$((attempt + 1))
done

# Everything is downloaded; unpack and configure without a time limit.
sudo apt-get "${APT_OPTS[@]}" install -y --no-download "${PACKAGES[@]}"

export_to_cache || true
