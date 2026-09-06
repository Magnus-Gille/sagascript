# Bounded public-speech evaluation — 2026-09-06

Status: **inconclusive; retain the shipped baselines**. This is partial evidence
for #187, not completed release acceptance or a recommendation to change a model.
Norwegian, English and Swedish bounded runs are complete. No personal settings
were changed.

The smaller models improved warm inference-call timing in all three languages,
but all increased heldout word error rate (WER). Disabling temperature fallback
did not demonstrate a reliable latency improvement. Neither this file-input
experiment nor its timing endpoint measures Bluetooth capture, hotkey release
to visible text, paste reliability, or complete desktop performance.

## Heldout results

Each language has 40 distinct human-speech utterances, evaluated in all three
configurations: 120 planned pairs, 120 completed, zero failures or exclusions.
Each pair has one cold call and five warm calls. Accuracy uses the **first warm
transcript**, not the best repetition; pooled WER is (S+D+I)/reference words.
Warm percentiles use all 200 calls per configuration.

| Language | Role / model | S / D / I | Reference words | WER | Warm p50 / p95 (ms) |
|---|---|---:|---:|---:|---:|
| Norwegian | Baseline / `nb-whisper-base` | 66 / 61 / 4 | 907 | 14.443% | 131.087 / 256.374 |
| Norwegian | Smaller / `nb-whisper-tiny` | 84 / 72 / 7 | 907 | 17.971% | 81.706 / 178.667 |
| Norwegian | Decoder / `nb-whisper-base` | 66 / 61 / 4 | 907 | 14.443% | 124.117 / 266.321 |
| English | Baseline / `base.en` | 47 / 10 / 2 | 1164 | 5.069% | 128.851 / 285.308 |
| English | Smaller / `tiny.en` | 52 / 12 / 8 | 1164 | 6.186% | 85.183 / 189.335 |
| English | Decoder / `base.en` | 47 / 10 / 2 | 1164 | 5.069% | 129.643 / 272.378 |
| Swedish | Baseline / `kb-whisper-base` | 110 / 50 / 67 | 1014 | 22.387% | 150.352 / 292.307 |
| Swedish | Smaller / `kb-whisper-tiny` | 119 / 48 / 75 | 1014 | 23.866% | 90.499 / 204.607 |
| Swedish | Decoder / `kb-whisper-base` | 110 / 50 / 67 | 1014 | 22.387% | 142.266 / 304.612 |

All use explicit language and beam size 0. Baseline and smaller use temperature
fallback; decoder uses the baseline model with fallback disabled. No glossary,
VAD model, or remote transcription was used.

| Language / candidate | WER change (percentage points; 95% interval) | Warm p95 gain (95% interval) |
|---|---:|---:|
| Norwegian / smaller | +3.528 (+1.386 to +5.806) | +30.310% (+23.927% to +39.450%) |
| Norwegian / decoder | 0.000 (0.000 to 0.000) | −3.880% (−19.871% to +13.801%) |
| English / smaller | +1.117 (+0.093 to +2.088) | +33.638% (+20.813% to +37.361%) |
| English / decoder | 0.000 (0.000 to 0.000) | +4.532% (−7.590% to +9.282%) |
| Swedish / smaller | +1.479 (−0.683 to +3.663) | +30.003% (+21.875% to +37.374%) |
| Swedish / decoder | 0.000 (0.000 to 0.000) | −4.210% (−16.164% to +10.779%) |

Paired bootstrap: seed 187, 2,000 resamples; utterances for accuracy and paired
utterance clusters containing all repetitions for timing. These intervals are
conditional on these selected clips, not speaker-population confidence claims.
Positive timing gain means faster. Cold metrics and all short/medium/long
stratum metrics are in [Norwegian JSON](no-heldout.json) and
[English JSON](en-heldout.json) and [Swedish JSON](sv-heldout.json).
Stratum p95 is exploratory: each contains only
13 or 14 utterances. The JSON includes missing-coverage counts; zero observed
control errors with zero control fixtures must **not** be read as a passing test.

## Corpus provenance and selection

| Corpus | Immutable Hub revision | Development / heldout speakers | Reference and license |
|---|---|---:|---|
| [NbAiLab/NPSC](https://huggingface.co/datasets/NbAiLab/NPSC/tree/e906435cc445e38961d86f3ebf53a7b38603b903) | `e906435cc445e38961d86f3ebf53a7b38603b903` | 5 / 8 | Exact publisher `sentence_text`; sound/transcriptions CC0, HF curation CC-BY-SA 3.0 |
| [OpenSLR LibriSpeech](https://huggingface.co/datasets/openslr/librispeech_asr/tree/71cacbfb7e2354c4226d01e70d77d5fca3d04ba1) | `71cacbfb7e2354c4226d01e70d77d5fca3d04ba1` | 2 / 5 | Exact publisher `text`; CC-BY 4.0 |
| [KBLab RixVox v2](https://huggingface.co/datasets/KBLab/rixvox-v2/tree/1f5f37f5ec8740eae318eeae7bf190074454d0d1) | `1f5f37f5ec8740eae318eeae7bf190074454d0d1` | 7 / 23 | Publisher protocol `text`, force-aligned, **not verbatim audio transcription**; publisher card lists ODC-By |

All three have ten development and forty heldout utterances, speaker-disjoint across
the two splits. Duration buckets are short <5 seconds, medium 5–15 seconds,
and long >15–30 seconds. Quotas are 3/4/3 for development and 13/14/13 for heldout.
All audio hashes are distinct within each corpus. Exact publisher references
were encoded as UTF-8 plus one LF, without normalization or machine translation.
Scoring subsequently uses `nfc-casefold-nfc-words-v1`, Python 3.10.13 / Unicode
13.0.0. Per-utterance identities, hashes, private paths, transcripts and audio
are intentionally not published in these aggregate artifacts.

NPSC uses source dates 20170209 (`eval`) and 20170207 (`test`), Norwegian Bokmål,
nonempty text without angle-bracket annotations, and seed `sagascript187-npsc-v1`.
This source is **not verified to be NPSC 2.0**. Its parliamentary speech is not
a representative sample of spontaneous computer dictation.

LibriSpeech uses the `all` configuration, `validation.clean` offset 100 and
`test.clean` offset 0, initially 100 candidates each. The first heldout pool had
only four long clips, so two additional 100-row heldout pages (offsets 100/200)
were retrieved before selection or inference. The original cache and deficit
were preserved; quotas were not relaxed. Selection seed is
`sagascript187-librispeech-v1`. Dataset Viewer URLs are not caller-pinnable;
the observed `x-revision` was required to match the exact Hub revision above.
The selected audio bytes were hashed locally. Read speech from audiobooks is
not spontaneous dictation, and no training-unseen claim is made.

RixVox has only a `train` split. Before inference, speakers were partitioned by
SHA-256 of `sagascript187-rixvox-speaker-v1:` plus publisher speaker ID: the first
eight hex digits modulo five equal to zero select development, otherwise heldout.
Eligible rows have known speakers, nonempty protocol text, `is_silence=false`,
and integer duration 1–30,000 **milliseconds**. A decoded WAV verified the unit;
the card's seconds label did not match the actual fields. Segment identity is
`speech_id` plus `chunk_id`, not the nonunique speech ID alone.

The first 500 metadata rows lacked enough short clips. Additional 100-row pages
were retrieved up to row 3,999; the earliest quota-feasible prefix, rows 0–2,699,
was selected with seed `sagascript187-rixvox-v1`. Later metadata did not enter
selection. Viewer revision was checked as for LibriSpeech. Fifty audio files
totaling 21,050,710 bytes were materialized and duration/hash-verified. Two
pre-inference partial materializations were retained: one failed before downloads
because of a tuple-unpacking bug; one stopped after three audio files (333,913
bytes) because an overly strict assertion rejected repeated reference text.
The corrected corpus preserves distinct audio while permitting repeated text.

RixVox references are parliamentary protocol text aligned to human speech, not
manual word-for-word transcription of the audio. Machine ASR fields were not
used. Known KB-Whisper training overlap is not ruled out. Consequently its WER
is **disagreement with this protocol reference**, not a clean estimate of
dictation accuracy; do not compare absolute WER across these corpora as model
rankings.

Native development runs completed 30/30 pairs per language. A separate Swedish
sandbox attempt produced 30 invalid reports because Metal was unavailable;
that entire failed ledger remains preserved. Its native retry used a new
directory, unchanged plan/configuration, and zero failed pairs. No failed
attempt was silently overwritten or mixed into the successful timing sample.
Baseline / smaller / decoder development WER was 9.565% / 13.913% / 9.565%
for Norwegian, 3.203% / 4.982% / 3.203% for English and
30.909% / 29.545% / 30.909% against Swedish protocol text.
The three configurations and original
thresholds were frozen unchanged before each heldout run; no clips were
dropped or configurations tuned after seeing heldout results.

## Reproducibility and timing boundary

- Source: `5e1e7f77f9f5c0f9aac5b9ee4a03ec5ecfe7b441`.
- Actual CLI: `sagascript 1.1.3 (git 5e1e7f7-dirty, built 2026-09-06)`.
- Binary SHA-256: `a13adcf4ca47e43abdf38fbb10305ef52fb473f63e12c1c2da33f18bf8f038cb`.
- Build: optimized ARM64, `cargo build --offline --release -p sagascript-cli --no-default-features`.
- The dirty marker reflects a borrowed untracked `node_modules` link; this is
  not a clean-source or signed-desktop attestation.
- Host: MacBook Air Mac16,12; Apple M4, 4 performance + 6 efficiency cores;
  32 GB RAM; macOS 26.6.2; AC power. No thermal-state attestation was collected.
- Plan seed 187; randomized utterance/configuration order; five warm repeats;
  no deliberate cooldown between pairs, no competing local inference during runs.
- Endpoint: `live_inference_call_not_visible_text`. Cold is the first call in
  a newly created backend, **not system-cold**, and can include native first-call
  overhead beyond weight loading. Warm calls execute native Whisper inference,
  not cached transcript retrieval. Backend capability alone does not attest
  which accelerator executed each operation.
- Aggregation: evaluator 0.3.0 at `427802529b2221e4a8c52a9d490271459b279e57`.
  `summarize-run` checked internal ledger/plan/reference consistency and
  recomputed scores; a separate calculation cross-checked pooled totals,
  percentiles and pair/speaker counts. This is not cryptographic execution
  attestation. Evaluator revision differs from the frozen benchmark binary.

Model files were independently hashed locally; registry expected hashes in a
report alone do not prove the bytes loaded by the runtime.

| Model | Bytes | SHA-256 |
|---|---:|---|
| `nb-whisper-base` | 55,295,450 | `dcb9f3ab963cd288974c826c1519ff73b78b2372e80d388a6ce94f29c6a5b40f` |
| `nb-whisper-tiny` | 29,875,738 | `e5fb42192cdf31bea624a524d035e8895030b2bb4b31d4ea2a1ebf0ea8f57237` |
| `base.en` | 147,964,211 | `a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002` |
| `tiny.en` | 77,704,715 | `921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f` |
| `kb-whisper-base` | 55,295,450 | `aead29b356bca8840e72a8dc2286e2d69e6702639751a1e60cb3c8eacefec546` |
| `kb-whisper-tiny` | 29,875,738 | `98d46b7d23e5528d006e8a42e29eb0cb39b44bed94e1329f10f57d1fd15c658b` |

## Why the decision remains inconclusive

The predeclared adoption gate requires ≥20% lower warm p95 end-to-end latency
or ≥15% relative WER reduction, at most +1 percentage point WER and −2 percentage
points specialist recall, and no new number/negation or silence failures.
The smaller models exceed the WER-loss point threshold here, while decoder
timing intervals cross zero. More importantly, the actual end-to-end endpoint
and required control evidence are absent, so none is eligible for adoption.

All environments are `unknown`. There are no specialist, number, negation,
ordinary, quiet/noisy or silence control annotations in these subsets, no
human semantic adjudication, no memory measurements, no physical Bluetooth or
visible-text measurements, and no representative spontaneous-dictation claim.
These missing measurements are not zeros and cannot be supplied by more warm
repetitions. The remaining work follows the [evaluation protocol](../../dictation-evaluation-protocol.md).
