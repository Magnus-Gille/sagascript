# R4 Finnish compact-model screening — 2026-09-06

## Decision

Keep multilingual **Base** as the Finnish default selected by the owner. Retain
**Finnish-NLP Tiny** as an explicitly optional evaluation model, not an automatic
migration or a production-quality recommendation.

The specialist is promising: lower corpus word error than both generic baselines
and lower warm latency than Base in this sample. However, **51.19% WER is still
high**. Previously used FLEURS read speech cannot establish acceptable spontaneous
dictation, native-speaker quality, alignment quality or signed application latency.

## Method and identity

The [fixed plan](r4-finnish-screening-plan.md) was committed before inference.
All 144 clip/model pairs completed with one cold and five warm transcriptions:
**864 transcriptions, zero execution failures**, no substituted retries. No builds
or other inference ran concurrently with the timed matrix.

- Source: `aa5eb24adedfd86d7a91203339ef1967fa611de4`.
- CLI SHA-256: `74070fde648b3ae657cea42884661251e54d3e4516d0a2ae4606ae83e0d0e19c`.
- CLI identity: `1.1.3 (git aa5eb24adedfd86d7a91203339ef1967fa611de4, built 2026-09-06)`.
- Build: `cargo build -p sagascript-cli --no-default-features --release --locked`,
  explicit source/date metadata, separate fresh release target.
- Host: Apple M4, 32 GiB RAM, macOS 26.6.2 (25G83).
- Fixture manifest SHA-256: `7edad8de9da2a60f0aeffc5c0548a25ccd75eebb4fcb2966d6ea097d6e3b6403`.
- Data: 48 pinned Finnish FLEURS test clips, 552.06 seconds, 758 normalized words.
  Gender-balanced selection is not speaker balance.
- Decoder: explicit `fi`, greedy, temperature fallback enabled, no prompt or VAD.
  Pair order shuffled with seed `20260906`.
- macOS build enables Metal/CoreML. No matching CoreML encoders were installed
  for these three artifacts; retained logs show CoreML-load fallback. Reports do
  not expose a measured GPU-device/backend identity. This is not an ANE benchmark.

Reports were validated against the frozen plan: exact build string, pinned model
bytes/hash, language, source/decoded audio hashes, duration, decoder settings,
one cold/five warm ordering, cache status, and CLI validation. Per-pair results
and final index were checked for missing/mismatched entries. The driver binds
the binary hash, which reports do not expose. The full source revision is
embedded in and validated through the build string. Raw audio, references and
transcripts remain outside the repository.

## Results

WER is total word edits divided by total reference words, not mean utterance WER.
The existing evaluator uses Unicode NFC/casefold normalization, retaining Finnish
letters. CER uses normalized tokens joined with single ASCII spaces, including
those spaces in the reference denominator. Quality uses the first cold transcript.

| Model | Download | Word edits / 758 | WER | CER | Cold total median / p95 | Warm total median / p95 |
|---|---:|---:|---:|---:|---:|---:|
| Generic Tiny | 77.69 MB | 553 | 72.96% | 21.91% | 678 / 911 ms | 255 / 392 ms |
| Generic Base | 147.95 MB | 417 | 55.01% | 14.72% | 1,169 / 1,544 ms | 405 / 604 ms |
| Finnish-NLP Tiny | 77.69 MB | 388 | 51.19% | 13.72% | 638 / 1,197 ms | 263 / 417 ms |

Each model has 48 cold and 240 warm samples. Medians average the middle pair for
even counts; p95 is nearest-rank. Total time ends at the live inference call,
**not visible pasted text**. Cold includes preparation/loading in a new process,
not a flushed OS disk cache.

The specialist has 29.84% lower relative WER than Tiny and 6.95% lower than Base.
Its warm median is 35.00% lower than Base, with p95 also lower. These satisfy the
plan's three aggregate accuracy/latency screens, not statistical significance or
user acceptance. Per-clip versus Base: 26 improved, 5 equal, 17 worse; versus
Tiny: 41 improved, 2 equal, 5 worse. No model produced an empty cold hypothesis,
over twice the reference token count, or exact whole-phrase repetition. These
mechanical checks cannot establish the absence of severe linguistic errors.

All Base and specialist warm outputs matched their normalized cold outputs.
Generic Tiny differed in 10/240 warm samples, across two fixtures. Fallback
remained enabled, rather than disabled to force deterministic output.

## Separate checks and pending acceptance

Synthetic zero-PCM live-path tests used 0.1, 0.5 and 2.0 seconds with all three
models: nine cases, **54 empty transcripts**, zero inference time because the
near-silence guard skipped model access. This does not reproduce Bluetooth noise,
speech cutoffs or short Finnish words.

Still pending: spontaneous native Finnish, real microphone/headset silence and
speech, signed Mac key-release-to-visible-text and memory-pressure measurements,
native Windows behavior, and fine-tuned timestamp/alignment quality. FLEURS was
previously used in model selection; training overlap is not fully established.
A test build must describe these as pending, not call Finnish production-ready.
