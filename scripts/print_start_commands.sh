#!/usr/bin/env bash
set -euo pipefail

echo "=== FlowDictate Autopilot Start Commands ==="
echo
echo "1) Install Ralph loop plugin (once):"
echo "/plugin install ralph-loop@claude-plugins-official"
echo
echo "2) Start the Ralph loop:"
echo "/ralph-loop:ralph-loop "Read PROMPT.md and execute it exactly. Re-read PROMPT.md at the start of every iteration. Do not ask the user questions. Make reasonable assumptions and document them." --max-iterations 80 --completion-promise "FLOWDICTATE_COMPLETE""
echo
echo "3) Cancel (if needed):"
echo "/ralph-loop:cancel-ralph"
