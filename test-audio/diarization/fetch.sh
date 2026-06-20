#!/usr/bin/env bash
# Fetch diarization benchmark audio and ground truth from medkit/simsamu (MIT, ungated)
# Run from the repo root or from this directory.
# Requires: curl, ffmpeg, and python3 with numpy + soundfile (pip install numpy soundfile)
# for the nb_samtale clip reconstruction.

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
python3 - "$DEST" <<'PYEOF'
import json, os, sys
dest = sys.argv[1]
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
python3 - "$DEST" <<'PYEOF'
import json, os, sys
dest = sys.argv[1]
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
echo "=========================================================================="
echo "Fetching nb_samtale_nb12 (41.8s, 2 speakers, Norwegian Bokmål, CC0)..."
echo "=========================================================================="
# Native-Scandinavian benchmark clip for Sagascript's Swedish/Norwegian path (issue #67).
# Source: Sprakbanken/nb_samtale (HF, CC0). The dataset ships pre-segmented per-turn WAVs
# (no continuous recording without the 3.77 GB source ZIP), BUT each segment's file name
# encodes its ABSOLUTE start_ms-end_ms offset within the source recording. We reconstruct a
# faithful continuous clip by placing each turn at its true offset (summing overlaps), so the
# inter-turn timing — and one genuine speaker overlap — is preserved. The source recording
# "nb-12" is a 2-speaker Bokmål interview (P9 = guest, P63 = host) about Nordic history.
# We take the dense, contiguous 41.8s window [952.841s .. 994.610s] of that recording.
#
# The 9 per-turn WAVs live inside two HF tarballs (train_bm 1.41 GB, test_bm 174 MB). We
# stream-extract ONLY the 9 needed members (≈1.3 MB total) without saving the full tarballs.

NBSRC="$DEST/nb_samtale_src"
mkdir -p "$NBSRC"
NB_BASE="https://huggingface.co/datasets/Sprakbanken/nb_samtale/resolve/main/data"

# 8 segments live in train_bm, 1 (the overlapping "ja" backchannel) in test_bm.
cat > "$NBSRC/.train_targets" <<'EOF'
data/train/bm/nb-12_0952841-0963097.wav
data/train/bm/nb-12_0963097-0965240.wav
data/train/bm/nb-12_0965270-0975841.wav
data/train/bm/nb-12_0975903-0977658.wav
data/train/bm/nb-12_0977671-0978083.wav
data/train/bm/nb-12_0978060-0990170.wav
data/train/bm/nb-12_0990190-0993219.wav
data/train/bm/nb-12_0993219-0994610.wav
EOF
echo 'data/test/bm/nb-12_0973878-0974390.wav' > "$NBSRC/.test_targets"

if [ "$(ls "$NBSRC"/nb-12_*.wav 2>/dev/null | wc -l | tr -d ' ')" -lt 9 ]; then
  echo "Stream-extracting 9 per-turn WAVs from HF tarballs (only the needed members)..."
  # Stream the tarball through tar; tar extracts the named members as they pass by.
  # tar can't know to stop early, so we run it in the background and kill the stream
  # once all targets are present. Files are clustered early in the archive (~tens of MB read).
  ( curl -sL "$NB_BASE/train_bm_1.tar.gz" | tar -xzv -C "$NBSRC" --strip-components=3 \
      -T "$NBSRC/.train_targets" >/dev/null 2>&1 ) &
  TRAIN_PID=$!
  ( curl -sL "$NB_BASE/test_bm_1.tar.gz"  | tar -xzv -C "$NBSRC" --strip-components=3 \
      -T "$NBSRC/.test_targets"  >/dev/null 2>&1 ) &
  TEST_PID=$!
  # Wait (max 180s) for all 9 files, then terminate any lingering stream.
  for _ in $(seq 1 90); do
    [ "$(ls "$NBSRC"/nb-12_*.wav 2>/dev/null | wc -l | tr -d ' ')" -ge 9 ] && break
    sleep 2
  done
  # Kill only the curl|tar pipelines we started, by PID + their children — NOT a broad
  # `pkill -f`, which could match unrelated processes on a shared machine.
  pkill -P $TRAIN_PID 2>/dev/null || true; kill $TRAIN_PID 2>/dev/null || true
  pkill -P $TEST_PID  2>/dev/null || true; kill $TEST_PID  2>/dev/null || true
  wait $TRAIN_PID $TEST_PID 2>/dev/null || true
fi
NBCNT="$(ls "$NBSRC"/nb-12_*.wav 2>/dev/null | wc -l | tr -d ' ')"
if [ "$NBCNT" -lt 9 ]; then
  echo "ERROR: expected 9 nb-12 source WAVs, got $NBCNT. Aborting nb_samtale build." >&2
  exit 1
fi
echo "Have $NBCNT/9 source WAVs. Building continuous clip + ground truth..."

# Build the continuous clip (placing turns at true offsets, summing the one overlap) and
# all ground-truth files (RTTM, transcript, STM) from the segment metadata embedded below.
python3 - "$DEST" "$NBSRC" <<'PYEOF'
import os, sys, numpy as np, soundfile as sf
dest, nbsrc = sys.argv[1], sys.argv[2]
SR = 16000
NAME = "nb_samtale_nb12"
spk_map = {"P9": "SPEAKER_A", "P63": "SPEAKER_B"}
# (abs_start_ms, abs_end_ms, speaker, verbatim_text). Order = turn order; one overlap (the
# P9 "ja" at 973878 sits inside P63's 965270-975841 turn). Text = `verbatim` field from
# Sprakbanken/nb_samtale metadata (lowercased, disfluencies as %eee, no punctuation).
SEGS = [
 (952841, 963097, "P9",  "man vet en del man tror man vet en del man antar det for å være sannsynlig at %eee og så i tillegg så spiller all politikken inn"),
 (963097, 965240, "P9",  "hvordan skal man gjøre dette her"),
 (965270, 975841, "P63", "helt til slutt når begynner Norden egentlig å ligne på %eee det vi tenker oss som Norden når når begynner det å ligne på moderne kart"),
 (973878, 974390, "P9",  "ja"),
 (975903, 977658, "P9",  "slutten av femtenhundretallet vil jeg si"),
 (977671, 978083, "P63", "ok"),
 (978060, 990170, "P9",  "%eee vipper vi ut på sekstenhundretallet så er det %eee vi kjenner oss veldig godt igjen på %emm Nordenkart fra slutten av femtenhundretallet og ut på sekstenhundretallet da er det ingen tvil"),
 (990190, 993219, "P63", "da takker jeg for en veldig morsom reise"),
 (993219, 994610, "P63", "tusen takk"),
]
# map abs_start_ms -> source wav filename (named nb-12_<start>-<end>.wav, zero-padded to 7)
def fname(a, b): return f"nb-12_{a:07d}-{b:07d}.wav"

t0 = min(s[0] for s in SEGS); t1 = max(s[1] for s in SEGS)
total = int(round((t1 - t0) / 1000.0 * SR))
mix = np.zeros(total, dtype=np.float32)
for a, b, spk, _ in SEGS:
    path = os.path.join(nbsrc, fname(a, b))
    audio, sr = sf.read(path, dtype="float32")
    assert sr == SR, f"{path}: sr={sr}"
    if audio.ndim > 1: audio = audio.mean(axis=1)
    # Guard against a truncated stream-extract: the per-turn WAV must be ~(end-start) ms long.
    expected = int(round((b - a) / 1000.0 * SR))
    if len(audio) < 0.9 * expected:
        raise SystemExit(f"{path}: {len(audio)} samples (<90% of expected {expected}) — "
                         "truncated fetch; delete nb_samtale_src/ and re-run fetch.sh")
    off = int(round((a - t0) / 1000.0 * SR)); end = off + len(audio)
    if end > total: audio = audio[:total - off]; end = total
    mix[off:end] += audio            # SUM preserves the overlap region
peak = float(np.max(np.abs(mix)))
if peak > 1.0: mix = mix / peak * 0.98   # avoid clipping from summed overlap
sf.write(os.path.join(dest, f"{NAME}.wav"), mix, SR, subtype="PCM_16")

# Reference RTTM (rebased to window start = 0)
with open(os.path.join(dest, f"{NAME}_ref.rttm"), "w") as f:
    for a, b, spk, _ in SEGS:
        s = (a - t0) / 1000.0; d = (b - a) / 1000.0
        f.write(f"SPEAKER {NAME} 1 {s:.3f} {d:.3f} <NA> <NA> {spk_map[spk]} <NA> <NA>\n")
# Speaker-attributed transcript (visual inspection)
with open(os.path.join(dest, f"{NAME}_ref_transcript.txt"), "w") as f:
    for a, b, spk, txt in SEGS:
        f.write(f"[{spk_map[spk]}] {txt}\n")
# STM reference for meeteval cpWER
with open(os.path.join(dest, f"{NAME}_ref.stm"), "w") as f:
    for a, b, spk, txt in SEGS:
        s = (a - t0) / 1000.0; e = (b - a) / 1000.0 + s
        f.write(f"{NAME} 1 {spk_map[spk]} {s:.3f} {e:.3f} {txt}\n")
print(f"Built {NAME}.wav ({total/SR:.2f}s, peak={peak:.2f}) + ref.rttm/_ref.stm/_ref_transcript.txt")
PYEOF

echo ""
echo "Done. Files in $DEST:"
ls -lh "$DEST"
echo ""
echo "Next: see README.md for scoring commands."
