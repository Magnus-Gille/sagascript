#!/usr/bin/env bash
# Install the Linux system packages the app crate links against (SAGA-546).
#
# The Azure Ubuntu mirror of the hosted runners sometimes stalls: apt fetched 60 MB in 15 minutes in
# one run, against about a minute normally. apt's own timeouts only catch a connection that goes
# silent, not a crawl, so each attempt also runs under a wall-clock `timeout`. A stuck attempt is
# killed (apt resumes partial downloads) and the retry switches to the main Ubuntu archive. The
# package list is unchanged; only the time spent on a bad mirror is bounded.
set -euo pipefail

PACKAGES=(libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libasound2-dev build-essential file xdotool)
ATTEMPTS="${APT_ATTEMPTS:-3}"
ATTEMPT_SECONDS="${APT_ATTEMPT_SECONDS:-240}"
APT_OPTS=(-o Acquire::Retries=3 -o Acquire::http::Timeout=20 -o Acquire::https::Timeout=20)
SOURCES_DIR="${APT_SOURCES_DIR:-/etc/apt}"

use_main_archive() {
  local file
  for file in "$SOURCES_DIR/sources.list" "$SOURCES_DIR"/sources.list.d/*.sources "$SOURCES_DIR"/sources.list.d/*.list; do
    [ -f "$file" ] || continue
    sudo sed -i.bak 's#//azure\.archive\.ubuntu\.com/#//archive.ubuntu.com/#g' "$file"
  done
}

attempt=1
while true; do
  if timeout "$ATTEMPT_SECONDS" sudo apt-get "${APT_OPTS[@]}" update &&
    timeout "$ATTEMPT_SECONDS" sudo apt-get "${APT_OPTS[@]}" install -y "${PACKAGES[@]}"; then
    exit 0
  fi
  if [ "$attempt" -ge "$ATTEMPTS" ]; then
    echo "apt failed after $ATTEMPTS attempts" >&2
    exit 1
  fi
  echo "apt attempt $attempt failed or exceeded ${ATTEMPT_SECONDS}s; retrying on archive.ubuntu.com" >&2
  use_main_archive
  attempt=$((attempt + 1))
done
