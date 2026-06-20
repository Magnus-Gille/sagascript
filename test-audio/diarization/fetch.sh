#!/usr/bin/env bash
# Fetch diarization benchmark audio and ground truth from medkit/simsamu (MIT, ungated)
# Run from the repo root or from this directory.
# Requires: curl, ffmpeg

set -euo pipefail

DEST="$(cd "$(dirname "$0")" && pwd)"
BASE="https://huggingface.co/datasets/medkit/simsamu/resolve/main"

echo "Fetching dj_2022_feu (58.6s, 2 speakers: medecin / patient, French medical dispatch)..."

# Audio
curl -L "$BASE/dj_2022_feu/dj_2022_feu.m4a" -o "$DEST/dj_2022_feu.m4a"
# Reference diarization
curl -sL "$BASE/dj_2022_feu/dj_2022_feu.rttm" -o "$DEST/dj_2022_feu.rttm"
# Per-word JSON (used to build speaker-attributed transcript)
curl -sL "$BASE/dj_2022_feu/dj_2022_feu.json" -o "$DEST/dj_2022_feu.json"

# Convert to 16 kHz mono WAV (Whisper/diarization input format)
ffmpeg -y -i "$DEST/dj_2022_feu.m4a" -ar 16000 -ac 1 "$DEST/dj_2022_feu.wav"

# Fix non-standard <NA> file_id in RTTM (pyannote.metrics requires the filename stem)
sed 's/^SPEAKER <NA> <NA>/SPEAKER dj_2022_feu 1/' "$DEST/dj_2022_feu.rttm" > "$DEST/dj_2022_feu_ref.rttm"

# Build speaker-attributed reference transcript (plain text, for DER visual inspection)
python3 - <<'PYEOF'
import json, os, sys
dest = os.path.dirname(os.path.abspath(__file__))
data = json.load(open(os.path.join(dest, "dj_2022_feu.json")))
with open(os.path.join(dest, "dj_2022_feu_ref_transcript.txt"), "w") as f:
    for m in data["monologues"]:
        speaker = m["speaker"]["id"]
        text = " ".join(t["text"] for t in m["terms"] if t["type"] == "WORD")
        if text.strip():
            f.write(f"[{speaker}] {text}\n")
print("Wrote dj_2022_feu_ref_transcript.txt")
PYEOF

# Build STM reference for meeteval cpWER scoring
python3 - <<'PYEOF'
import json, os
dest = os.path.dirname(os.path.abspath(__file__))
data = json.load(open(os.path.join(dest, "dj_2022_feu.json")))
with open(os.path.join(dest, "dj_2022_feu_ref.stm"), "w") as f:
    for m in data["monologues"]:
        speaker = m["speaker"]["id"]
        words = [t for t in m["terms"] if t["type"] == "WORD"]
        if not words:
            continue
        text = " ".join(t["text"] for t in words)
        start = words[0]["start"]
        end = words[-1]["end"]
        f.write(f"dj_2022_feu 1 {speaker} {start:.3f} {end:.3f} {text}\n")
print("Wrote dj_2022_feu_ref.stm")
PYEOF

echo ""
echo "Done. Files in $DEST:"
ls -lh "$DEST"
echo ""
echo "Next: see README.md for scoring commands."
