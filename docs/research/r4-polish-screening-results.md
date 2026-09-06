# R4 Polish specialist screening — final no-go

## Decision

Do not add the Polish specialist or explicit Polish language support in R4.
Keep existing language defaults unchanged; do not promote generic **Small** to
a new Polish default merely because it is the size-matched comparator. Polish is a conditional extra
language, not a release quota. The specialist did not meet the fixed screening
rule: its WER was **16.35% worse than generic Small**, and its warm total-time
p95 was **2,645.95 ms**, above the **1,500 ms** absolute budget.

This is a screening no-go, not a claim that the model is unusable. It does not
establish native-speaker quality, visible-text application latency, or a reason
to change any existing language default. The Polish implementation checkpoint
`f9170a4` was retained for review and then reversibly reverted; it is **not** a
shipping runtime change. The follow-up is documentation-only in the same PR.

## Fixed method and identity

The [fixed plan](r4-polish-screening-plan.md) and its thresholds, corpus,
decoding settings, pair ordering, and outputs were not changed after inference.
The content-free summary reports are retained outside the repository:

- `sagascript-r4-pl-score-v2.json`
- `sagascript-r4-pl-regression.json`
- `sagascript-r4-pl-duration-audit.json`
- `sagascript-r4-pl-screen.B9ZC6G/memory-check/summary.json`

The fixed corpus contained **48 clips, 37 speaker hashes, and 1,040 normalized
reference words**, with exact decoded audio duration **516.312125 s**. The
materialized manifest reports the rounded total **516.314 s**. There were 144
raw model pairs and **864 transcriptions (one cold plus five warm per pair),
zero execution failures**, no inference reruns, and no cherrypicking. Raw audio,
references, and transcript text remain outside the repository.

The measured run used source revision
`68178b45ad067c6401d43a30413022c36d47be5f`; the lean CLI binary SHA-256 was
`78daf997597ab230a8af9d56d8d85e742691cf94b11ba8c30ae80d0c37afbdb1`. The
artifacts and publisher revisions are pinned in the [fixed plan](r4-polish-screening-plan.md):
OpenAI/whisper.cpp at `5359861c739e955e79d9a303bcbc70fb988958b1` for the generic
models, and BardsAI `whisper-small-pl` at
`baca145d78e8dbf3f2cc9c7ccf372f650ee1209c` for the specialist. The plan records
the corresponding model hashes, sizes, licenses, VoxPopuli snapshot, and
dataset/legal-notice links.

An initial duration-precision check was inconclusive: 66 reports were rejected
when the initial scorer treated millisecond-rounded metadata as exact. An independent
ffprobe audit covered all 48 source files and all 144 reports; the maximum
actual-versus-rounded duration difference was **0.4375 ms**. The scorer was then
corrected with seven passing tests, including a RED/GREEN regression for the
strict half-millisecond metadata boundary. The audit also verified each report
duration against its source WAV's exact 16 kHz sample count. Its SHA-256 is
`2446ef013748a063ed30e82ac6ae6f6c65a711a21402efc4090e8a5c51bcc2a3`.
The
same corpus, thresholds, and model outputs were retained; v2 validated **all
144 pairs**.

## Quality and timing

WER is total word edits divided by the 1,040 reference words. CER uses the
fixed normalized-token character definition from the plan. Timing is the
time through local inference completion, not key-up-to-visible-text latency.
Audio-file decoding is outside these per-call timings; cold includes model
initialization, but does not flush the system disk cache.

| Model | Artifact bytes | WER | CER | Cold total median / p95 | Warm total median / p95 |
|---|---:|---:|---:|---:|---:|
| Generic Base | 147,951,465 | 22.1154% (230 edits) | 7.5671% | 1,322.37 / 2,414.38 ms | 449.98 / 1,077.36 ms |
| Generic Small | 487,601,967 | 10.0000% (104 edits) | 4.2039% | 4,228.87 / 6,396.86 ms | 1,414.68 / 3,032.55 ms |
| Polish specialist | 487,601,967 | 11.6346% (121 edits) | 4.7111% | 4,201.23 / 6,512.26 ms | 1,407.18 / 2,645.95 ms |

Relative to Base, the specialist improved WER by **47.39%**. Relative to the
size-matched Small baseline, it was **16.35% worse**. Its warm p95 was 0.873×
Small (12.75% lower), so it passed the relative 1.25× timing gate but failed the
separate absolute 1,500 ms gate.

Per-clip comparisons were mixed: Polish versus Base was **36 better, 6 equal,
6 worse**; versus Small it was **11 better, 18 equal, 19 worse**. Generic Base
had one normalized cold hypothesis over twice the reference length. Polish had
no empty, over-2× runaway, or exact whole-phrase-repeat result; no model had an
empty or whole-phrase-repeat result. These mechanical checks do not establish
linguistic or native-speaker acceptance. Base differed from its cold transcript
in 8 of 240 warm calls across two clips; both Small models matched their cold
transcripts in every warm call.

## Memory and backend caveats

The memory report is supplemental: one benchmark-dictation process per model,
one cold plus five warm calls, using the longest selected clip (25.96 s). It is
not app-wide memory, GPU memory, or a release acceptance measurement.

| Model | Peak RSS bytes | Peak RSS MiB |
|---|---:|---:|
| Generic Base | 253,181,952 | 241.45 |
| Generic Small | 642,613,248 | 612.84 |
| Polish specialist | 645,103,616 | 615.22 |

No matching CoreML encoder artifact was present for any of the three models.
The binary was compiled with Metal/CoreML support, but that fact does not prove
actual GPU or ANE utilization. The report therefore does not claim a measured
backend-device identity.

## Dataset and acceptance limits

The fixed source was the pinned Polish VoxPopuli test selection. VoxPopuli
declares CC0 and points to the [European Parliament legal
notice](https://www.europarl.europa.eu/legal-notice/en/) for underlying
recordings; that metadata is not blanket redistribution clearance for every
item. The references are not marked gold, and the specialist card already
reports VoxPopuli results. Its declared CV11/FLEURS training provenance makes
this a plausible training-distinct screen, not proof of zero overlap or an
untouched acceptance set.

Still unmeasured are spontaneous native Polish, short words, real microphone
silence/headset noise, signed Mac and native Windows visible-text timing,
application-level memory pressure, and timestamp/alignment quality. These must
remain pending in any test build; this read-speech screen must not be described
as native acceptance.

The optional-language cap is not a quota. Do not add Polish or an
accuracy-driven default change solely from this screening. Existing generic
Small availability for other supported selections is unchanged. Finnish remains
the R4 language decision already selected by the owner; this Polish candidate
is rejected for the current R4 build while the evidence and pinned artifacts
remain available for a separately planned future evaluation.
