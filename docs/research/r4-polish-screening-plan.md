# R4 Polish specialist screening — fixed before inference

Polish is a conditional additional R4 language (#211), not a release quota.
This plan tests whether a compact existing publisher artifact merits an optional
evaluation slot. Generic multilingual Base remains the initial recommendation;
any later default change requires a separately recorded evidence-backed decision.

## Model artifacts

| Model | Role | Bytes | SHA-256 |
|---|---|---:|---|
| Generic Base | Current small/default baseline | 147,951,465 | `60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe` |
| Generic Small | Size-matched baseline | 487,601,967 | `1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b` |
| BardsAI Polish Small | Optional specialist candidate | 487,601,967 | `e4c77eb6a61c7dbbfa72cf810ee472c546f8af2394a26e109e5ac358f7b16112` |

Generic models use the existing `ggerganov/whisper.cpp` revision
`5359861c739e955e79d9a303bcbc70fb988958b1` (MIT). Polish uses the unchanged existing
`bardsai/whisper-small-pl` GGML at `baca145d78e8dbf3f2cc9c7ccf372f650ee1209c`
(declared Apache-2.0). No custom hosting, quantization, or mismatched generic
CoreML encoder is part of this comparison. Download size is not a latency claim.

## Fixed corpus and provenance

- [VoxPopuli](https://huggingface.co/datasets/facebook/voxpopuli), Polish test,
  HF snapshot `5ab868a7cddb87d84224ebfc8112ebd0905fc5bd`.
- Test archive: 600,845,710 bytes, SHA-256
  `f835667aaebf8297bf00ae32bfb976a9c1fc6b1f99df264bad3d78d465e64adf`.
- Test TSV SHA-256: `42146f45fd17972ed156b58f9b7db370c627b1ab3acbd7e7c02c1e890d991621`.
- Official compressed Polish annotation SHA-256:
  `044eb3d3ee77c60e5a480a0af045576b624b38af2e249352df8ec68df66c9ce0`.
- Selection seed: `sagascript-r4-polish-fixed48-v1`; hash-ranked selection with
  speaker diversity first, 16 clips each under 5 s, 5–15 s inclusive, and over
  15 s. All clips are at most 30 s. Exactly 48 distinct clips and 37 speaker hashes.
- Frozen selection-plan SHA-256:
  `ccbe6eda7857ce3dee2c368b8843e9856f1472915f6d8282436dfc52d329d066`.
- Verified materialized-manifest SHA-256:
  `92c31d54a33de5b250cfe3c968e80c7b14de04af64d270e107366ad0e972de2b`.
- Actual float-PCM WAV durations were checked with ffprobe: 1.000–25.960 s,
  516.314 s total. Preserve every source audio/reference hash in the run plan.
- The scratch selection plan contained an unverified 32-character segmenter
  reference. It is explicitly marked unverified in materialized provenance; it
  is not cited as an immutable source commit or used to generate the downloaded
  audio. The archive and reference files are independently pinned by full hashes.

The dataset declares CC0 and refers to the
[European Parliament legal notice](https://www.europarl.europa.eu/legal-notice/en/)
for underlying recordings. Acknowledge VoxPopuli and the European Parliament,
retain source links and keep raw audio/text outside the repository and release.
Do not treat dataset metadata as blanket redistribution clearance for every item.

The fine-tune declares CV11 and FLEURS training, not VoxPopuli; this is a plausible
training-distinct screen, not proof of zero overlap. Its card already reports
VoxPopuli results, so this is not an untouched acceptance set. Selected references
are not marked gold. Parliamentary speech and reference/alignment errors limit
what WER can establish about everyday spontaneous dictation.

## Execution and scoring

Use this session's Apple M4 host, 32 GiB RAM, macOS 26.6.2 (25G83).
Before inference, freeze one clean committed source revision and SHA-bound lean
release CLI, preserving existing benchmark binaries. Use explicit `pl`, greedy
decoding, temperature fallback enabled, no prompt or VAD. Run 48 clips against
three models, deterministic shuffled pair order with seed `20260907`, sequentially
without concurrent compilation or inference. Each pair gets one fresh-process
cold run and five warm in-process runs. Retain every failure; no silent retries.

Use the existing Unicode NFC/casefold token normalizer on the original reference
text and the first cold hypothesis. Corpus WER is total edits / total reference
words, never best-of-six or mean clip WER. CER uses normalized tokens joined by
single ASCII spaces. Report cold and warm separately: total/inference median
(average middle pair for even counts) and nearest-rank p95; report warm transcript
variation and per-clip regressions separately. Endpoint is inference completion,
not visible pasted text; cold does not flush the OS disk cache. Record actual
backend evidence and explicitly name fields that the report does not expose.

## Prospective decision rule

Admit the specialist as an optional R4 test choice only if all aggregate screens
pass, with no unresolved execution/integrity failures or severe regression:

1. At least 10% relative corpus WER improvement over generic Small.
2. At least 20% relative corpus WER improvement over Base, justifying the larger file.
3. Warm p95 total time no worse than 1.25 times generic Small, and at most 1,500 ms
   on this fixed host/corpus. This is a screening budget, not a GUI latency promise.

A boundary result is inconclusive; do not move thresholds after seeing outputs.
Absolute quality, per-clip failures and footprint still require judgment. Passing
does not mandate a default change or establish native-speaker acceptance. A no-go
keeps this conditional extra language out of the R4 build and records the evidence.

Separate acceptance: spontaneous native Polish, short words, real silence/headset
noise, signed Mac/native Windows visible-text timing and memory pressure, and
timestamp/alignment quality. Test builds may explicitly mark human acceptance
pending; they must not call these checks passed from this read-speech screen.
