#!/usr/bin/env bash
# Flake detector (SAGA-381): runs the engine_host integration tests repeatedly while every CPU is
# kept busy, WITHOUT retries, and reports how many runs failed. A retry would hide the race this
# job exists to find, so a single red run is a finding. The build happens once, before the stress.
#
# Usage: scripts/ci/flake-detector.sh [runs]   (default 20)
# Env:   FLAKE_RESULT_FILE  where to write the JSON summary (default: flake-result.json)
#        FLAKE_LOG_DIR      where the per-run logs go (default: flake-logs)
set -uo pipefail

runs="${1:-20}"
if ! [[ $runs =~ ^[1-9][0-9]*$ ]]; then
  echo "runs must be a positive integer, got '$runs'" >&2
  exit 64
fi
result_file="${FLAKE_RESULT_FILE:-flake-result.json}"
log_dir="${FLAKE_LOG_DIR:-flake-logs}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
case "$log_dir" in /*) ;; *) log_dir="$root/$log_dir" ;; esac
case "$result_file" in /*) ;; *) result_file="$root/$result_file" ;; esac
rm -rf "$log_dir" "$result_file"
mkdir -p "$log_dir"

cd "$root/src-tauri"
cargo test -p sagascript-core --test engine_host --no-run || exit 2

# One busy loop per CPU: a 3-vCPU runner is the slowest machine the suite has to survive.
workers=$(nproc 2>/dev/null || echo 2)
pids=()
trap 'kill "${pids[@]}" 2>/dev/null || true' EXIT
for _ in $(seq "$workers"); do
  (while :; do :; done) &
  pids+=("$!")
done

failed=0
failed_runs=()
for run in $(seq "$runs"); do
  log="$log_dir/run-$run.log"
  if cargo test -p sagascript-core --test engine_host >"$log" 2>&1; then
    echo "run $run: ok"
  else
    echo "run $run: FAILED"
    failed=$((failed + 1))
    failed_runs+=("$run")
  fi
done

kill "${pids[@]}" 2>/dev/null || true
printf '{"runs":%s,"failed":%s,"failed_runs":[%s],"stress_workers":%s}\n' \
  "$runs" "$failed" "$(IFS=,; echo "${failed_runs[*]:-}")" "$workers" >"$result_file"
cat "$result_file"
[ "$failed" -eq 0 ]
