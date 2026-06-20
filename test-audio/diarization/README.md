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

### nb_samtale_nb12 — NATIVE-SCANDINAVIAN BENCHMARK CLIP (issue #67)

| Property | Value |
|---|---|
| Source | [Sprakbanken/nb_samtale](https://huggingface.co/datasets/Sprakbanken/nb_samtale) on HF, recording `nb-12` |
| License | CC0-1.0 (public domain) |
| Language | Norwegian Bokmål (native; matches Sagascript's `-l no` + Norwegian/Swedish Whisper path) |
| Duration | 41.77 seconds |
| Speakers | 2 (`SPEAKER_A` = guest P9, `SPEAKER_B` = host P63; Bokmål interview on Nordic history) |
| Audio format | WAV, 16 kHz, mono, pcm_s16le (native bandwidth, **not** upsampled telephone) |
| Ground truth | RTTM (9 turns) + speaker-attributed transcript + STM |
| Audio file | `nb_samtale_nb12.wav` — gitignored, built by `fetch.sh` |

**Why this clip exists:** the SimSAMU clip above is French 8 kHz telephone audio, which
mismatches Sagascript's Swedish/Norwegian-optimized, native-bandwidth path (issue #67). This
clip exercises the real path: native-language, 16 kHz, two clearly-distinct conversational
speakers, with one genuine speaker overlap (the guest's "ja" backchannel landing inside the
host's question at ~21 s).

**How it's built (no 3.77 GB download):** `nb_samtale` ships pre-segmented per-turn WAVs, but
each file name encodes its **absolute** `start_ms-end_ms` offset within the source recording.
`fetch.sh` stream-extracts only the 9 per-turn WAVs of recording `nb-12`'s densest contiguous
2-speaker window (`[952.841 s .. 994.610 s]`) from the HF tarballs (≈1.3 MB of the 1.41 GB +
174 MB archives — the streams are killed once the 9 members pass by), then places each turn at
its true offset (summing the one overlap) to reconstruct a faithful continuous clip. The
reference RTTM/STM timing is therefore the dataset's own segment metadata, rebased to 0.

**Ground truth files committed:**
- `nb_samtale_nb12_ref.rttm` — reference diarization (9 turns, `SPEAKER_A`/`SPEAKER_B`)
- `nb_samtale_nb12_ref_transcript.txt` — speaker-attributed transcript
- `nb_samtale_nb12_ref.stm` — STM for meeteval cpWER

**Measured results (collar=0.25). The headline for #67:** unlike the French telephone clip
(where the default `0.85` threshold collapsed 2 speakers → 1 cluster, 55 % DER), on this
**native-bandwidth, native-language** clip the diarizer cleanly resolves 2 speakers at *every*
threshold {0.5, 0.6, 0.7, 0.85}, and the sweep is **flat** — the two speakers' embeddings are
separated by a wide margin, so the threshold has no effect here. The default `0.85` behaves
well.

| model | `-l` | threshold(s) | clusters | DER | cpWER |
|---|---|---|---|---|---|
| `kb-whisper-small` (Swedish, the default) | `no` | 0.5–0.85 (flat) | 2 | **26.9 %** | 82.7 % |
| `nb-whisper-small` (Norwegian, language-matched) | `no` | 0.5–0.85 (flat) | 2 | **7.3 %** | 40.0 % |

DER is driven by the (language-agnostic) WeSpeaker embeddings, so even the Swedish model gets a
reasonable 26.9 % DER — but it transcribes the Norwegian audio *as Swedish* and merges the
guest's 37 s turn into one segment, wrecking cpWER (82.7 %) and the within-speaker boundaries.
The language-matched `nb-whisper-small` more than halves DER and roughly halves cpWER. Takeaway
for #67: keep the `0.85` default; it does not over-collapse native-bandwidth audio — the French
clip's collapse was a narrowband-embedding artifact, not a default-threshold problem.

**Fetch audio + build:**
```bash
./test-audio/diarization/fetch.sh   # streams nb-12's 9 turns, rebuilds nb_samtale_nb12.wav
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

### 3. Sprakbanken/nb_samtale (Norwegian conversational — Bokmål/Nynorsk) — FETCHED as `nb_samtale_nb12`
- **What it is:** ~10.7k pre-segmented speaker-turn WAVs (train+test+validation) from 35 Norwegian conversations. 8 source recordings are genuinely 2-speaker across the whole corpus.
- **Good for:** **DER + cpWER.** Originally flagged as "DER requires the 3.77 GB ZIP" because the audio is pre-segmented per turn — but the **segment file names encode absolute `start_ms-end_ms` offsets within the source recording** (e.g. `nb-12_0952841-0963097.wav` = 952.841–963.097 s of recording `nb-12`). So a faithful continuous clip can be reconstructed by placing each turn at its true offset, **without** the 3.77 GB ZIP. The one gotcha: a recording's turns are split across train/test/validation, so you must combine metadata from all splits to see a recording's full turn timeline and find a dense contiguous window.
- **License:** CC0-1.0 (public domain).
- **Gated:** No.
- **Fetch metadata (all splits):** `for s in train test validation; do curl -sL https://huggingface.co/datasets/Sprakbanken/nb_samtale/resolve/main/data/${s}_metadata.jsonl -o ${s}_metadata.jsonl; done` (note: the `validation`/`dev` URL may 404 in places; train+test is enough).
- **Fetch audio:** per-file `resolve/.../*.wav` URLs return **404** — the WAVs only exist inside the tarballs (`train_bm_1.tar.gz` 1.41 GB, `test_bm_1.tar.gz` 174 MB, etc.). Stream-extract only the members you need (see `fetch.sh`'s nb_samtale block) — you pull ~tens of MB, not the whole archive, since the per-recording files are clustered early. Kill the curl once your members have passed by.
- **Built clip:** `nb_samtale_nb12` (recording `nb-12`, 41.8 s, 2-speaker Bokmål, with one real overlap). See "Fetched clips" above.
- **3.77 GB ZIP** (`https://www.nb.no/sbfil/taledata/nb_samtale.zip`) is **not needed** for this approach; only required if you want the original raw recording with its real silence/non-target-speaker content between turns.

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

`pyannote-audio` includes `pyannote.core` and `pyannote.metrics` (and pulls in `numpy` + `soundfile`, which `fetch.sh` needs to rebuild the `nb_samtale` clip).

### Step 2: Fetch audio (if not already present)

```bash
./test-audio/diarization/fetch.sh
```

### Step 3: Run Sagascript diarization

> **Measurement note.** This clip is **French**, but Sagascript's default model is
> `kb-whisper-small` (Swedish, `language=sv`) — on French it mis-detects the language and
> hallucinates, which collapses ~11s of dialogue and contaminates the DER. Pin a
> **multilingual** model so you measure diarization, not ASR mismatch. **Pass `-l auto`
> explicitly** — the CLI's `-l` doesn't accept `fr`, and *omitting* it falls back to your
> configured language (e.g. `sv`), which reintroduces the exact mismatch. Also mind the
> clustering threshold: the default `0.85` collapses this 2-speaker telephone audio to a
> single cluster; `0.70–0.80` is the right operating point for this clip. Do **not**
> generalize that threshold to Swedish recordings without re-validating — `0.85` exists to
> curb over-clustering elsewhere (see #67).

```bash
sagascript transcribe test-audio/diarization/dj_2022_feu.wav \
  --diarize --model small -l auto --diarize-threshold 0.70 --json \
  > test-audio/diarization/dj_2022_feu_hyp.json
```

Reference operating points (multilingual `small`, DER vs `--diarize-threshold`):

| threshold | clusters | DER |
|---|---|---|
| 0.60 | 5 | 33.9% |
| **0.70–0.80** | 3 | **~21%** |
| 0.85 (default) | 2→1 | 55.4% |

~21% is roughly the floor for this 8 kHz upsampled telephone clip — WeSpeaker embeddings
degrade on narrowband audio. Tracked in #67.

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
sagascript transcribe test-audio/diarization/dj_2022_feu.wav --diarize --model small -l auto --json 2>/dev/null | python3 -c "
import json, sys
data = json.load(sys.stdin)
print('Top-level keys:', list(data.keys()))
print('Speakers:', data['speakers'])
print('First segment:', data['segments'][0] if data['segments'] else 'empty')
"
```

The plain (non-`--json`) output is one `[SPEAKER_x] text` line per consolidated segment.
