#!/usr/bin/env bash
set -euo pipefail

app_bundle=${1:?usage: smoke-single-instance-macos.sh /path/to/Sagascript.app}
executable="$app_bundle/Contents/MacOS/sagascript"

[[ $(uname -s) == "Darwin" ]] || {
  echo "single-instance smoke test requires macOS" >&2
  exit 2
}
[[ -x "$executable" ]] || {
  echo "app executable is missing or not executable: $executable" >&2
  exit 2
}

# Refuse to disturb or accidentally test against an already-running installed
# instance (or a concurrent CLI process with the same executable name).
if pgrep -x sagascript >/dev/null 2>&1; then
  echo "refusing to run while another sagascript process is active" >&2
  exit 2
fi

runtime_root=$(mktemp -d "${TMPDIR:-/tmp}/sagascript-single-instance.XXXXXX")
primary_pid=""
secondary_pid=""
replacement_pid=""

cleanup() {
  for pid in "$secondary_pid" "$replacement_pid" "$primary_pid"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill -TERM "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  rm -rf "$runtime_root"
}
trap cleanup EXIT

candidate_pids() {
  ps -axo pid=,command= | awk -v target="$executable" '
    {
      pid = $1
      sub(/^[[:space:]]*[0-9]+[[:space:]]+/, "", $0)
      if ($0 == target) print pid
    }
  '
}

wait_for_alive() {
  local pid=$1
  for _ in {1..100}; do
    if kill -0 "$pid" 2>/dev/null; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

wait_for_exit() {
  local pid=$1
  for _ in {1..100}; do
    if ! kill -0 "$pid" 2>/dev/null; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

wait_for_log() {
  local path=$1
  local pattern=$2
  for _ in {1..100}; do
    if [[ -f "$path" ]] && grep -q "$pattern" "$path"; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

settings_path=$(HOME="$runtime_root/home" "$executable" config path)
case "$settings_path" in
  "$runtime_root/home/"*) ;;
  *)
    echo "candidate settings escaped isolated HOME: $settings_path" >&2
    exit 1
    ;;
esac

HOME="$runtime_root/home" RUST_LOG=info "$executable" >"$runtime_root/primary.log" 2>&1 &
primary_pid=$!
wait_for_alive "$primary_pid" || {
  echo "primary GUI process did not stay alive" >&2
  exit 1
}
wait_for_log "$runtime_root/primary.log" "Loaded settings" || {
  echo "primary GUI process did not finish single-instance startup" >&2
  exit 1
}

HOME="$runtime_root/home" RUST_LOG=info "$executable" >"$runtime_root/secondary.log" 2>&1 &
secondary_pid=$!
wait_for_exit "$secondary_pid" || {
  echo "secondary GUI process did not exit" >&2
  exit 1
}
wait "$secondary_pid"
secondary_pid=""

mapfile_output=$(candidate_pids)
[[ "$mapfile_output" == "$primary_pid" ]] || {
  echo "expected exactly primary PID $primary_pid after second launch; found: ${mapfile_output:-none}" >&2
  exit 1
}

# CLI dispatch happens before Tauri/plugin initialization and must remain usable
# while the GUI owns the single-instance key.
cli_language=$(HOME="$runtime_root/home" "$executable" config get language)
grep -Eq '^(auto|en|sv|no|fi)$' <<<"$cli_language"

# Simulate an unclean termination. OS-backed ownership must disappear with the
# process so a replacement launch cannot be stranded by stale state.
kill -KILL "$primary_pid"
wait "$primary_pid" 2>/dev/null || true
primary_pid=""

HOME="$runtime_root/home" RUST_LOG=info "$executable" >"$runtime_root/replacement.log" 2>&1 &
replacement_pid=$!
wait_for_alive "$replacement_pid" || {
  echo "replacement GUI process did not recover after an unclean exit" >&2
  exit 1
}
wait_for_log "$runtime_root/replacement.log" "Loaded settings" || {
  echo "replacement GUI process did not finish startup after an unclean exit" >&2
  exit 1
}

mapfile_output=$(candidate_pids)
[[ "$mapfile_output" == "$replacement_pid" ]] || {
  echo "expected exactly replacement PID $replacement_pid; found: ${mapfile_output:-none}" >&2
  exit 1
}

echo "Verified one GUI instance, concurrent CLI dispatch, and crash recovery."
