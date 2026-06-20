# Diarization Benchmark

Ground-truth clips for measuring Sagascript's diarization pipeline:
pyannote segmentation-3.0 (ONNX) -> WeSpeaker ResNet34-LM embeddings (ONNX) ->
agglomerative clustering -> speaker-attributed Whisper transcript.

Two metrics:
- **DER** (Diarization Error Rate): measures speaker turn accuracy against reference RTTM.
- **cpWER** (concatenated minimum-permutation WER): measures speaker-attributed transcription accuracy.

---

## Fetched clips (committed: ground truth only, audio gitignored)

### dj_2022_feu — PRIMARY BENCHMARK CLIP

| Property | Value |
|---|---|
| Source | [medkit/simsamu](https://huggingface.co/datasets/medkit/simsamu) on HF |
| License | MIT |
| Language | French |
| Duration | 58.6 seconds |
| Speakers | 2 (`medecin` / `patient`, French emergency dispatch simulation) |
| Audio format | WAV, 16 kHz, mono, pcm_s16le (converted from 8 kHz M4A) |
| Ground truth | RTTM (23 speaker segments) + per-word JSON (speaker + timing + text) |
| Audio file | `dj_2022_feu.wav` — gitignored, fetch with `fetch.sh` |

**Ground truth files committed:**
- `dj_2022_feu.rttm` — raw RTTM from dataset (file_id is `<NA>`, non-standard)
- `dj_2022_feu_ref.rttm` — fixed RTTM with file_id = `dj_2022_feu` (use this for scoring)
- `dj_2022_feu_ref_transcript.txt` — speaker-attributed transcript (`[medecin] text`, `[patient] text`)
- `dj_2022_feu_ref.stm` — STM format reference for meeteval cpWER

**Caveats:**
1. Speaker labels are `medecin` / `patient` (not `SPEAKER_0` / `SPEAKER_1`). pyannote.metrics handles the label permutation automatically via Hungarian assignment — no manual mapping needed.
2. Audio was originally 8 kHz telephone M4A; resampled to 16 kHz mono by fetch.sh. Whisper handles it fine; WER will be French-specific but DER results are language-agnostic.
3. Language is French, not Swedish. The pyannote/WeSpeaker embedding models are language-agnostic, so DER results transfer to Swedish usage. WER is French-specific.

**Fetch audio:**
```bash
./test-audio/diarization/fetch.sh
```

---

## Dataset shortlist

### 1. medkit/simsamu (RECOMMENDED — fetched above)
- **What it is:** 61 French medical dispatch simulations, 2 speakers each (`medecin` + `patient`), 58s–3min per clip.
- **Good for:** DER + cpWER. Has RTTM + per-word JSON with speaker IDs. Smallest clip is 289 KB / 58.6s. Total dataset 53.3 MB audio.
- **License:** MIT (fully free, no restrictions).
- **Gated:** No.
- **Fetch:** `curl -L https://huggingface.co/datasets/medkit/simsamu/resolve/main/dj_2022_feu/dj_2022_feu.m4a`
- **Caveat:** RTTM has non-standard `<NA>` file_id — fix with `sed 's/^SPEAKER <NA> <NA>/SPEAKER dj_2022_feu 1/'`.

### 2. AMI Meeting Corpus (diarizers-community/ami + edinburghcstr/ami + pyannote/AMI-diarization-setup)
- **What it is:** 16 test meetings, 4 speakers each, 17–29 min per meeting. IHM (close-talk) microphones.
- **Good for:** DER + cpWER. RTTMs from `github.com/pyannote/AMI-diarization-setup`; word-level transcripts from `edinburghcstr/ami`.
- **License:** CC-BY-4.0.
- **Gated:** No.
- **Fetch RTTM (one meeting):**
  ```bash
  curl -o EN2002a.rttm https://raw.githubusercontent.com/pyannote/AMI-diarization-setup/main/only_words/rttms/test/EN2002a.rttm
  ```
- **Fetch audio (requires datasets library or 912 MB parquet download):**
  ```python
  from datasets import load_dataset
  import soundfile as sf
  ds = load_dataset("diarizers-community/ami", "ihm", split="test")
  row = ds[0]  # EN2002a, ~29 min
  sf.write("EN2002a_ihm.wav", row["audio"]["array"], row["audio"]["sampling_rate"])
  ```
- **Caveat:** 4 speakers per meeting (not 2). Meeting register, not conversational. Full meeting audio is large (~57 MB each embedded in 912 MB parquet). Use ffmpeg to clip to 5 min.

### 3. Sprakbanken/nb_samtale (Norwegian conversational — Bokmal/Nynorsk)
- **What it is:** 1195 pre-segmented speaker-turn WAV clips from Norwegian conversations. Test set has 35 source recordings, 11 of which are 2-speaker.
- **Good for:** cpWER (has transcripts). DER requires reconstruction — CRITICAL CAVEAT: audio files are pre-segmented per-speaker turn (not continuous multi-speaker recordings). Diarization benchmark requires reassembling turns in order, but original gap timing is lost.
- **License:** CC0-1.0 (public domain).
- **Gated:** No.
- **Fetch metadata:** `curl -sL https://huggingface.co/datasets/Sprakbanken/nb_samtale/resolve/main/data/test_metadata.jsonl -o test_metadata.jsonl`
- **Fetch audio tar (174 MB):** `curl -L https://huggingface.co/datasets/Sprakbanken/nb_samtale/resolve/main/data/test_bm_1.tar.gz | tar -xzf - --wildcards "data/test/bm/nb-2_*.wav"`
- **Original full recordings** (needed for proper DER): `https://www.nb.no/sbfil/taledata/nb_samtale.zip` (3.77 GB, no auth, HTTP 200). Required to run diarization on a continuous audio stream.
- **Caveat:** NOT a drop-in benchmark without the 3.77 GB ZIP. The HF dataset Viewer also has 500 errors on all API calls.

### 4. diarizers-community/voxconverse
- **What it is:** 216 clips (YouTube debates, panels), variable speaker count (1–21). Dev set RTTMs at `github.com/joonson/voxconverse`.
- **Good for:** DER only — no transcripts in the HF dataset.
- **License:** CC-BY-4.0.
- **Gated:** No.
- **Fetch one RTTM:**
  ```bash
  curl -o oenox.rttm https://raw.githubusercontent.com/joonson/voxconverse/master/dev/oenox.rttm
  ```
- **Fetch audio:** Must download full dev WAV zip (1.85 GB from Oxford) or a ~485 MB parquet shard from HF. No per-clip direct URL.
- **Caveat:** DER only. Many clips have 5–10 speakers. Audio download is all-or-nothing (no per-clip URLs).

---

## Running the benchmark

### Step 1: Install scoring tools (one-time)

```bash
pip install pyannote-audio meeteval
```

`pyannote-audio` includes `pyannote.core` and `pyannote.metrics`.

### Step 2: Fetch audio (if not already present)

```bash
./test-audio/diarization/fetch.sh
```

### Step 3: Run Sagascript diarization

```bash
sagascript transcribe test-audio/diarization/dj_2022_feu.wav --diarize --json \
  > test-audio/diarization/dj_2022_feu_hyp.json
```

### Step 4: Convert hypothesis JSON to RTTM

Sagascript `--json` output is a top-level **object** `{"segments": [...], "speakers": [...], "language", "model", "file", "duration_seconds"}`. Each entry in `segments` has `speaker`, `start` (s), `end` (s), `text`.

```python
import json

data = json.load(open("test-audio/diarization/dj_2022_feu_hyp.json"))
with open("test-audio/diarization/dj_2022_feu_hyp.rttm", "w") as f:
    for seg in data["segments"]:
        spk = seg["speaker"]
        start = seg["start"]
        dur = seg["end"] - seg["start"]
        f.write(f"SPEAKER dj_2022_feu 1 {start:.3f} {dur:.3f} <NA> <NA> {spk} <NA> <NA>\n")
```

Also build hypothesis STM for cpWER:

```python
import json

data = json.load(open("test-audio/diarization/dj_2022_feu_hyp.json"))
with open("test-audio/diarization/dj_2022_feu_hyp.stm", "w") as f:
    for seg in data["segments"]:
        spk = seg["speaker"]
        text = seg.get("text", "").strip()
        f.write(f"dj_2022_feu 1 {spk} {seg['start']:.3f} {seg['end']:.3f} {text}\n")
```

### Step 5: Score DER

```python
from pyannote.core import Annotation, Segment
from pyannote.metrics.diarization import DiarizationErrorRate

def load_rttm(path):
    ann = Annotation()
    for line in open(path):
        parts = line.strip().split()
        if parts[0] != "SPEAKER":
            continue
        start, dur, spk = float(parts[3]), float(parts[4]), parts[7]
        ann[Segment(start, start + dur)] = spk
    return ann

ref = load_rttm("test-audio/diarization/dj_2022_feu_ref.rttm")
hyp = load_rttm("test-audio/diarization/dj_2022_feu_hyp.rttm")

metric = DiarizationErrorRate(collar=0.25)   # collar=0.25s is standard
der = metric(ref, hyp)
print(f"DER: {der:.1%}")
print(metric)  # breakdown: missed speech, false alarm, confusion
```

DER = (missed speech + false alarm + speaker confusion) / total reference speech.
Lower is better. A naive single-speaker baseline on a 2-speaker clip typically gives 40–50% DER.

### Step 6: Score cpWER

```bash
python3 -m meeteval.wer cpwer \
  -r test-audio/diarization/dj_2022_feu_ref.stm \
  -h test-audio/diarization/dj_2022_feu_hyp.stm
```

cpWER = concatenated minimum-permutation WER. It finds the speaker assignment (permutation)
that minimises WER, then reports per-speaker WER and overall. Unlike plain WER it correctly
handles speaker label mismatches between reference (`medecin`/`patient`) and hypothesis
(`SPEAKER_0`/`SPEAKER_1`).

---

## Output format notes

Sagascript `--diarize --json` emits a top-level object whose `segments` array holds the
diarized turns. Field names come straight from `DiarizedSegment` (`src-tauri/src/diarization/mod.rs`):
`start` (f64 seconds), `end` (f64 seconds), `speaker` (`"SPEAKER_0"`, …), `text`. Sanity-check with:

```bash
sagascript transcribe test-audio/diarization/dj_2022_feu.wav --diarize --json 2>/dev/null | python3 -c "
import json, sys
data = json.load(sys.stdin)
print('Top-level keys:', list(data.keys()))
print('Speakers:', data['speakers'])
print('First segment:', data['segments'][0] if data['segments'] else 'empty')
"
```

The plain (non-`--json`) output is one `[SPEAKER_x] text` line per consolidated segment.
