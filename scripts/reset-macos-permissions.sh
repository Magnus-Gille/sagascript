#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This helper is only for macOS." >&2
  exit 1
fi

echo "Quit every running Sagascript copy before continuing."
echo "Resetting only Sagascript TCC records (no other applications are affected)."

for identifier in ai.gille.sagascript com.sagascript.app; do
  for service in Microphone Accessibility; do
    if ! tccutil reset "$service" "$identifier" >/dev/null 2>&1; then
      echo "Could not reset $service for $identifier; remove the stale entry manually in System Settings."
    fi
  done
done

cat <<'EOF'

Reset complete. Now:
1. Remove every stale Sagascript entry from Privacy & Security > Microphone and
   Accessibility. Use the minus button when available.
2. Keep exactly one installed copy at /Applications/Sagascript.app. Do not run a
   target/debug bundle alongside it while testing permissions.
3. Launch /Applications/Sagascript.app and grant each permission once.
4. If Accessibility still shows the old copy, remove it, click +, and choose
   /Applications/Sagascript.app explicitly.
EOF
