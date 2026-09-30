#!/usr/bin/env bash
# Usage: smoke-pianissimo-installed.sh CLI_PATH_OR_SYMLINK AUDIO_FILE [EXPECT_WORD]
# Exercises Pianissimo end to end through an installed CLI path (for example a
# symlink to Sagascript.app/Contents/MacOS/sagascript): engine status, model
# download, engine doctor, and a Swedish-mode transcription. Every step fails
# loudly; nothing is skipped when the model download is unavailable.
set -euo pipefail

[[ $# -eq 2 || $# -eq 3 ]] || { echo "Usage: $0 CLI AUDIO_FILE [EXPECT_WORD]" >&2; exit 2; }
cli=$1
audio=$2
expect=${3:-}
[[ -x "$cli" ]] || { echo "CLI is not executable: $cli" >&2; exit 1; }
[[ -f "$audio" ]] || { echo "Audio sample is missing: $audio" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

echo "== engine status"
"$cli" engine status --json | tee "$work/status.json"
python3 - "$work/status.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
text = json.dumps(d)
assert "Contents/Resources/EngineHost/sagascript-engine-host" in text, \
    "engine status does not resolve the host inside the app bundle"
PY

echo "== download pianissimo-sv"
"$cli" download-model pianissimo-sv

echo "== engine doctor"
"$cli" engine doctor --json | tee "$work/doctor.json"

echo "== transcribe"
"$cli" transcribe --language sv --model pianissimo-sv --json "$audio" > "$work/transcript.json"
python3 - "$work/transcript.json" "$expect" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
assert isinstance(d.get("text"), str) and d["text"].strip(), "empty transcript text"
print("transcript:", d["text"][:200])
expect = sys.argv[2].lower()
if expect:
    assert expect in d["text"].lower(), f"expected word {expect!r} not in transcript"
PY
echo "Pianissimo installed-CLI smoke passed"
