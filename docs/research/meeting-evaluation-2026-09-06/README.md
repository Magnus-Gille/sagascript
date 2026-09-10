# Sagascript AMI long-file baseline — 2026-09-06

This directory is a content-free, pinned baseline report for one
19-minute English AMI meeting-roleplay fixture (`ES2003a`). It is not an issue
closure, a release-quality claim, a Swedish acceptance gate, or a product
adoption decision.

The published bundle is sufficient to identify the inputs, pins, formulas, and
content-free results, but it is not a complete third-party reproduction of the
exact scorer execution while the reviewed scorer remains private scratch. A
fully independent rerun of the exact scoring implementation requires that
scorer to be published separately.

The fixture is the public AMI Mix-Headset signal, losslessly converted from
WAV to an ALAC M4A input (`channels=1`, `sample_rate=16000 Hz`), with decoded
PCM format `s16le`. Earlier frozen provenance verification found that the
source WAV and ALAC input decode to the same PCM SHA-256
`8a6853f922b0849b6d45075746cb6e9591141b7b671e5d4cb1c558d4a40d2f31`; this
identity is recorded in the provenance file, rather than re-proving the source
file's PCM payload from this public bundle. It contains four reference
participants and has measured duration `1139.765375` seconds, versus the AMI
table's official `1124.0` seconds. The scorer used an explicit UEM of
`[0,1139.765375]` with the annotations unchanged; hypothesis speech where no
reference speech exists is scored as false alarm.
Public provenance and license
links are in [`provenance-v1.json`](./provenance-v1.json). The public source
metadata records CC BY 4.0; no audio playback or private recording was used.

## Controlled result

The frozen CLI is Sagascript `1.1.3`, source revision
`491f400dc08cbcd35c59c2418b84080ecc0c1239`, with binary SHA-256
`06e2ecc0a1fce738cfc1e56fc2e70f1d1b6307de28e1d9f7708828ab1232cfbb`.
It was an optimized ARM64 `cargo build -p sagascript-cli --release
--no-default-features --features diarization`, not a signed desktop build.
Every phase used `language=en`, `model=base.en`, the explicit `meeting` prompt,
diarization debug output, a fresh process, and the same input/model identities.
The exact source-binary, three-model, and six-file Core ML companion hashes are
in [`performance-v1.json`](./performance-v1.json).

| phase | cache result | diarization threshold | WER | DER |
| --- | --- | ---: | ---: | ---: |
| no-cache | miss | 0.75 | 34.4644% | 57.1583% |
| populate | miss, cache written | 0.75 | 34.4644% | 57.1583% |
| reuse-055 | hit | 0.55 | 34.4644% | 57.1584% |
| reuse-090 | hit | 0.90 | 34.4644% | 59.9254% |

The four content-free scorer results are available as
[`quality-score-no-cache.json`](./quality-score-no-cache.json),
[`quality-score-populate.json`](./quality-score-populate.json),
[`quality-score-reuse-055.json`](./quality-score-reuse-055.json), and
[`quality-score-reuse-090.json`](./quality-score-reuse-090.json).

WER is chronological raw-word WER after the pinned public
[`normalization.py`](https://github.com/Magnus-Gille/sagascript/blob/69b0118399bf73c74ff812dbf608a8a57e171ea5/scripts/dictation_eval/normalization.py)
rules: NFC, casefold, NFC again; Unicode letters, numbers, and combining marks
form tokens; an ASCII apostrophe or U+2019 is retained only internally between
word characters, with U+2019 canonicalized to ASCII; every other character,
including hyphen, separates tokens. No filler-removal step is applied. WER is chronological
raw-word WER: 2,063 normalized reference tokens,
substitutions `185`, deletions `509`, insertions `17`, and total errors `711`.
The native logs contained 1,569 raw WORD records and 1,571 normalized
hypothesis tokens. Consolidated stdout independently normalized to 1,571
tokens in all four runs; that crosscheck is informational only and is not a
gate because consolidation boundaries can differ.

DER uses 134 reference speaker intervals over the full `1139.765375`-second
UEM, collar `0`, overlap included, and optimal Hungarian mapping. Its
`751.5549999999996`-second denominator is the sum of per-speaker reference
unions over the full UEM, not one global union. The raw DIARSEG parser found 87
segments and unique hypothesis-speaker counts of 2, 2, 2, and 1 for
no-cache, populate, reuse-055, and reuse-090 respectively; only these counts,
not speaker labels, are published in the performance JSON's per-phase derived
metadata. It is based on raw
debug diarization intervals, not consolidated transcript spans. The
chronological overlap ordering caveat means this is not cpWER and does not
claim speaker-permutation-insensitive word accuracy.

## Performance context

The two cache-hit phases report approximately `0.04 s` wall time and zero
model-load, Whisper-inference, diarization-segmentation, and
diarization-embedding timing in the native phase fields (small cache lookup,
clustering, and diagnostic assembly timings remain). The two
non-hit phases report `23.600 s` and `24.102 s` wall time, with native phase
totals `23.512 s` and `24.010 s`. These are cache-matrix observations, not a
general speed ranking: each run is a fresh process, but the phases do not claim
an OS-file-cache-cold condition. The approximately `0.04 s` values are observed
wall values with a `20 ms` polling-resolution bound, not timer precision.
Peak RSS and all native phase fields are in
[`performance-v1.json`](./performance-v1.json).

The reuse-055 missed-detection value is `94.932` seconds versus `94.931`
seconds at 0.75, a 1 ms difference. Its cause is not established here; no
rounding or merge explanation is inferred.

An earlier exploratory `24.12201175 s` run is excluded because the installed
Core ML companion bundle had not been prehashed before that run. It is retained
only as an excluded provenance note in the performance JSON.

The backend reported `coreml+metal` / `active`; these are capability/status
labels from the runtime, not encoder-execution attestations. The measured host
was a Mac16,12 Apple M4 (10 cores: 4 performance, 6 efficiency), 32 GB RAM,
macOS `26.6.2`, on AC power at 100%. Thermal state was not independently
attested. No release comparison, hardware ranking, or quality adoption
threshold is inferred.

## Reproduction and privacy boundary

The reviewed scorer is private scratch rather than repository-published; its
SHA-256 is recorded in [`provenance-v1.json`](./provenance-v1.json). The frozen
protocol is the four-phase matrix: no-cache, populate, reuse at threshold
`0.55`, and reuse at threshold `0.90`, with receipt-bound result/stdout/stderr
artifacts and pre/post model, binary, input, and cache identity checks.

Only aggregate metrics, hashes, counts, configuration, and public provenance
are published here. Raw audio, transcripts, speaker labels, stdout, native
logs, cache files, and private filesystem paths remain outside this report.
