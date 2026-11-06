#!/usr/bin/env bash
# Build whisper.cpp for the apple-m1 baseline instead of the runner's CPU (see
# scripts/cmake/macos-arm64-portable.cmake). Run it before the Rust cache step:
# rust-cache keys on CMAKE_* variables, so a changed policy never restores
# whisper.cpp objects built under the old one.
set -euo pipefail

: "${GITHUB_ENV:?GITHUB_ENV must be set (GitHub Actions only)}"
: "${GITHUB_WORKSPACE:?GITHUB_WORKSPACE must be set (GitHub Actions only)}"

policy="$GITHUB_WORKSPACE/scripts/cmake/macos-arm64-portable.cmake"
test -f "$policy"
echo "CMAKE_PROJECT_INCLUDE=$policy" >> "$GITHUB_ENV"
echo "whisper.cpp CPU policy: $policy"
