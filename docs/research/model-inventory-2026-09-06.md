# Sagascript model inventory — 2026-09-06

Status: factual inventory and measurement plan for [issue #199](https://github.com/Magnus-Gille/sagascript/issues/199). This document does not change a model, runtime, default, download URL, or cache. Retain the shipped defaults; candidate recommendations remain provisional until measured evidence exists.

Evidence was cross-checked on 2026-09-06 against the corrected current-evidence handoff, the local registry at [`manager.rs`](https://github.com/Magnus-Gille/sagascript/blob/7c6b237a727a1300a986109805325906e875ff21/src-tauri/crates/sagascript-core/src/settings/manager.rs), the diarization registry at [`diarization/model.rs`](https://github.com/Magnus-Gille/sagascript/blob/7c6b237a727a1300a986109805325906e875ff21/src-tauri/crates/sagascript-core/src/diarization/model.rs), [`docs/model-sources.md`](https://github.com/Magnus-Gille/sagascript/blob/7c6b237a727a1300a986109805325906e875ff21/docs/model-sources.md), and the local `whisper-rs-sys 0.14.1` source. This local tuple verification is current-day evidence; `docs/model-sources.md` retains its historical publisher-metadata review date of 2026-07-10. No weights or private audio were downloaded.

## Current shipped ASR models

All ASR artifacts are GGML `.bin` files loaded through the local `whisper-rs` CPU/Metal/CoreML backend. The OpenAI family is MIT and pinned to `ggerganov/whisper.cpp` revision `5359861c739e955e79d9a303bcbc70fb988958b1`. The language-specific families are Apache-2.0 and use their own immutable model-card revisions.

| Family and IDs exposed by the registry | Pinned revision | Artifact size / format | Language and current default |
|---|---|---|---|
| OpenAI: `tiny.en`, `tiny`, `base.en`, `base`, `small.en`, `small`, `medium.en`, `medium`, `large-v3-turbo`, `large-v3-turbo-q8_0` | `5359861c739e955e79d9a303bcbc70fb988958b1` | GGML `.bin`; exact registry sizes are 77,691,713–1,624,555,275 bytes; `large-v3-turbo-q8_0` is 874,188,075 bytes | MIT; English default `base.en`; Auto default `base` |
| KBLab Swedish: `kb-whisper-tiny`, `kb-whisper-base`, `kb-whisper-small`, `kb-whisper-medium`, `kb-whisper-large` | `76d796af43a50fa34321efa562c9b9887a187463`, `1499d2d2f0c7ed545bd6f2eec85287cf8d8c8b38`, `3564d61a42fc210ceaa55a22a96dd64478959c78`, `0abe10b9d7f75d0902656e5c06c5c4d549604dc5`, `d5d5984b4d8f7c4847a8ea203f1976285fb28300` respectively | `ggml-model-q5_0.bin` renamed to Sagascript IDs; 29,875,738 / 55,295,450 / 175,209,680 / 539,212,484 / 1,081,140,203 bytes | Apache-2.0, Swedish RixVox v2 fine-tunes; Swedish default `kb-whisper-base` |
| NbAiLab Norwegian: `nb-whisper-tiny`, `nb-whisper-base`, `nb-whisper-small`, `nb-whisper-medium`, `nb-whisper-large` | `8b38492d0e4111d5d6ad825e979cb082a2da013a`, `2ab372b6baa181a22f54f18030cae3703402c59e`, `e9bb5cb83cb74c96239fd506163aa97cff2fce4c`, `0ed074d5985bd56ca4140159a9dbffbc3fb5117e`, `8c6249fdeeb4dcd05e5735a4c39640607eb6e4ac` respectively | `ggml-model-q5_0.bin` renamed to Sagascript IDs; 29,875,738 / 55,295,450 / 175,209,680 / 539,212,484 / 1,081,140,203 bytes | Apache-2.0, NCC/NST/NPSC fine-tunes; Norwegian default `nb-whisper-base` |

Primary publisher references for these rows are the [OpenAI/whisper.cpp card](https://huggingface.co/ggerganov/whisper.cpp), and these pinned model cards: KBLab [`tiny`](https://huggingface.co/KBLab/kb-whisper-tiny/tree/76d796af43a50fa34321efa562c9b9887a187463), [`base`](https://huggingface.co/KBLab/kb-whisper-base/tree/1499d2d2f0c7ed545bd6f2eec85287cf8d8c8b38), [`small`](https://huggingface.co/KBLab/kb-whisper-small/tree/3564d61a42fc210ceaa55a22a96dd64478959c78), [`medium`](https://huggingface.co/KBLab/kb-whisper-medium/tree/0abe10b9d7f75d0902656e5c06c5c4d549604dc5), [`large`](https://huggingface.co/KBLab/kb-whisper-large/tree/d5d5984b4d8f7c4847a8ea203f1976285fb28300); and NbAiLab [`tiny`](https://huggingface.co/NbAiLab/nb-whisper-tiny/tree/8b38492d0e4111d5d6ad825e979cb082a2da013a), [`base`](https://huggingface.co/NbAiLab/nb-whisper-base/tree/2ab372b6baa181a22f54f18030cae3703402c59e), [`small`](https://huggingface.co/NbAiLab/nb-whisper-small/tree/e9bb5cb83cb74c96239fd506163aa97cff2fce4c), [`medium`](https://huggingface.co/NbAiLab/nb-whisper-medium/tree/0ed074d5985bd56ca4140159a9dbffbc3fb5117e), [`large`](https://huggingface.co/NbAiLab/nb-whisper-large/tree/8c6249fdeeb4dcd05e5735a4c39640607eb6e4ac). The local registry and model-sources document remain authoritative for the exact downloaded bytes.

The exact downloaded-file SHA-256 and byte-size pairs remain authoritative in
[`manager.rs`](https://github.com/Magnus-Gille/sagascript/blob/7c6b237a727a1300a986109805325906e875ff21/src-tauri/crates/sagascript-core/src/settings/manager.rs)
and [`docs/model-sources.md`](https://github.com/Magnus-Gille/sagascript/blob/7c6b237a727a1300a986109805325906e875ff21/docs/model-sources.md).
Publisher commit IDs and Git-LFS IDs must not be substituted for those local
artifact hashes.

### Language mapping, runtime, and acceleration

The shared core registry is used by the CLI model commands and the app model
commands. Its current selectable sets are:

| Language | Selectable IDs | `recommended()` default |
|---|---|---|
| English | `tiny.en`, `base.en`, `small.en`, `medium.en` | `base.en` |
| Swedish | all five `kb-whisper-*` models | `kb-whisper-base` |
| Norwegian | all five `nb-whisper-*` models | `nb-whisper-base` |
| Auto | multilingual OpenAI `tiny`, `base`, `small`, `medium`, `large-v3-turbo`, `large-v3-turbo-q8_0` | `base` |

English-only models are not valid for Swedish, Norwegian, or Auto. The
language-specific fine-tunes are intentionally scoped to their language and
are not used as independent language detectors.

The manifest declares `whisper-rs = "0.15"`; the current local
`whisper-rs-sys 0.14.1` source bundles whisper.cpp **1.7.6**. On macOS, only
the OpenAI family has optional CoreML encoder archives; those models use
CoreML+Metal when the verified archive is present and Metal otherwise. KBLab
and NbAiLab fine-tunes use Metal without an upstream CoreML encoder. Windows
and Linux use the CPU backend. These are runtime/backend facts, not quality
measurements.

## Other shipped models

| Capability / CLI ID | Exact revision and artifact | Runtime, license, and size |
|---|---|---|
| Optional built-in VAD, not a `WhisperModel`: `ggml-silero-v5.1.2.bin` | [`ggml-org/whisper-vad@9ffd54a`](https://huggingface.co/ggml-org/whisper-vad/tree/9ffd54a1e1ee413ddf265af9913beaf518d1639b); local SHA-256 `29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf`; 885,098 bytes | GGML model consumed by whisper.cpp VAD when VAD is enabled; MIT; approximately 0.9 MB |
| Feature-gated diarization segmentation: `pyannote-segmentation` | [`csukuangfj/sherpa-onnx-pyannote-segmentation-3-0@9403a690`](https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/tree/9403a6902bb58e3d5ae8c7e77c3422de279db2e0); `model.onnx`; local SHA-256 `220ad67ca923bef2fa91f2390c786097bf305bceb5e261d4af67b38e938e1079`; 5,992,913 bytes | ONNX Runtime; mirror card has no license field, while its LICENSE is MIT and identifies CNRS/pyannote provenance; approximately 6 MB |
| Feature-gated diarization speaker embedding: `wespeaker-embedding` | [`Wespeaker/wespeaker-voxceleb-resnet34-LM@f0c48c29`](https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34-LM/tree/f0c48c298fd835726c27956a5d617bad7115627e); `voxceleb_resnet34_LM.onnx`; local SHA-256 `7bb2f06e9df17cdf1ef14ee8a15ab08ed28e8d0ef5054ee135741560df2ec068`; 26,530,309 bytes | ONNX Runtime, 256-dimensional ResNet34-LM; CC-BY-4.0; approximately 27 MB |

`download-model diarization` is a feature-gated meta-ID that obtains both ONNX artifacts. The app and default CLI enable the diarization feature; the lean CLI built with `--no-default-features` does not expose it.
The two files are not interchangeable with the GGML ASR models.

## Experiments and bounded shortlist

These candidates are directly compatible in format or runtime family, but none
has Sagascript quality, latency, memory, or physical visible-text evidence.
They are experiments only; no automatic integration or default change is
recommended.

| Candidate | Verified metadata | Known caveats and required work |
|---|---|---|
| whisper.cpp **v1.9.3** runtime | Upstream release [v1.9.3](https://github.com/ggml-org/whisper.cpp/releases/tag/v1.9.3), target commit `371b5a7561823ab2bb32142d2751e35e7534727b`, published 2026-08-20; MIT upstream | This is a newer runtime, not newer weights. The release notes mention very-short-audio OOB-read and malformed-header fixes, but there is no local API, Metal/CoreML, Windows, memory, speed, or transcription-quality result. The `whisper-rs` binding compatibility must be proven before adoption. |
| Silero VAD **v6.2.0** | [Publisher metadata](https://huggingface.co/api/models/ggml-org/whisper-vad?blobs=true) verified 2026-09-06: revision `9ffd54a1e1ee413ddf265af9913beaf518d1639b`, `ggml-silero-v6.2.0.bin`, 885,098 bytes, LFS SHA-256 `2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987`, MIT. Repository last modified 2025-11-17 (not a separately verified model-release date). | Present alongside v5.1.2 in the same pinned repository, not a new repository revision. No bytes downloaded or local model integrity verification, runtime compatibility, or measured silence/short-word behavior yet. Keep v5.1.2 until those gates pass. |
| OpenAI `ggml-base-q5_1.bin` | `ggerganov/whisper.cpp` revision `5359861c739e955e79d9a303bcbc70fb988958b1`; 59,707,625 bytes; LFS SHA-256 `422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898`; MIT | Publication date is unknown from current evidence. Same GGML runtime family; requires a new stable ID, registry integrity entry, CLI/GUI mapping, download/cache namespace, and measurements. Peak memory and quality are unknown. |
| OpenAI `ggml-base-q8_0.bin` | Same revision; 81,768,585 bytes; LFS SHA-256 `c577b9a86e7e048a0b7eada054f4dd79a56bbfa911fbdacf900ac5b567cbb7d9`; MIT | Publication date is unknown from current evidence. Same requirements; quality, cold/warm speed, and peak memory are unknown. |
| OpenAI `ggml-large-v3-turbo-q5_0.bin` | Same revision; 574,041,195 bytes; LFS SHA-256 `394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2`; MIT | Publication date is unknown from current evidence. Same requirements; quantization compatibility, quality, latency, and peak memory are unknown. It would need a distinct model ID and must not silently replace either shipped turbo artifact. |

No newer shipped KBLab, NbAiLab, Pyannote, WeSpeaker, or OpenAI revision was
found in the supplied current evidence: their public API heads match the
immutable revisions already pinned locally. A repository head match is not a
quality or compatibility result.

## Measurement blockers and acceptance gates

The bounded decision is **retain**, with these separate experiments only after
their prerequisite data is available:

| Candidate | Relevant shipped comparison | Expected benefit to test, not a measured result | Decision now |
|---|---|---|---|
| Runtime v1.9.3 | Bundled v1.7.6, same verified model weights | Robustness fixes and possible backend improvements on Mac/Windows; verify binding and build compatibility before speech measurements | Defer to a separate runtime experiment; retain v1.7.6 here |
| VAD v6.2.0 | Optional v5.1.2, both 885,098-byte artifacts | Better silence/short-word boundary decisions; equal download size does not prove equal memory or behavior | Retain v5.1.2 pending silence and clipping evidence |
| Base Q5/Q8 | Generic `base`, 147,951,465 bytes; not the English `base.en` default | Smaller published payloads (59,707,625 / 81,768,585 bytes); quality and actual memory/latency unknown | Defer; resolve explicit language/Auto and CLI/GUI mapping in a separate candidate |
| Turbo Q5 | Existing turbo and turbo-Q8, 1,624,555,275 / 874,188,075 bytes | Smaller published 574,041,195-byte payload; speed and quality can regress despite smaller size | Defer; never replace either existing artifact under its old ID |

The shortlist favors the current local runtime family to keep experiments
bounded. It does not establish that a new backend or every language candidate
in roadmap #198 has been evaluated. Finnish #168 and the Danish prototype #169
retain their separate integration and native-language acceptance requirements.

The following blockers prevent a defensible adopt/adapt/reject decision:

* **#187:** real Swedish, Norwegian, and English development/held-out data
  with speaker/session coverage and transcripts is not yet available as a
  completed scorer input. Do not fabricate multilingual WER/CER or speaker
  metrics from model cards.
* **#163:** physical key-up-to-visible-text measurement is not complete. Offline
  timings cannot claim the endpoint users experience in the app.
* **#157:** long-file behavior and memory/quality boundaries remain open.

For every candidate, record a separate immutable evidence row before exposing
it to users:

1. **Provenance:** publisher URL, exact 40-character revision, artifact file,
   LFS/download SHA-256, byte size, license, runtime revision, and build ID.
2. **Quality:** #187 dev and held-out scores for Swedish/Norwegian/English,
   including speaker/session splits; short words, silence, code-switching, and
   long-file behavior where applicable. Keep the shipped default as the control.
3. **Speed:** cold and warm model load, inference, and #163 phase timings;
   separately capture physical stop/key-up to visible text when claiming app
   latency. Report p50/p95 and the machine/OS/backend.
4. **Memory:** peak resident memory during cold load and representative short
   and long utterances, plus download size. Model-card size alone is not peak
   runtime memory.
5. **Cache and rollback:** cache keys must include model ID, exact artifact
   digest, runtime/backend/build identity, and language policy. A failed or
   rejected candidate must be removable without touching the shipped artifact;
   rollback must restore the prior pinned tuple and defaults.
6. **CLI/GUI parity:** the same candidate must appear, download, verify, select,
   and transcribe through `list-models`/`download-model`/`transcribe` and the
   GUI model picker/settings path. No candidate should be CLI-only or GUI-only.

Until those gates pass, retain the current local-first defaults and model
registry. Newer runtime code and newer quantized weights must be evaluated as
separate changes; neither is an automatic upgrade of the other.

## Explicit unknowns

The following are intentionally unresolved rather than inferred: all candidate WER/CER, diarization,
short-word/silence, cold/warm latency, visible-text latency, and peak-memory
results; cross-platform binding/build compatibility for whisper.cpp v1.9.3;
and whether any candidate improves the shipped default enough to justify a
migration. No claim in this inventory closes #187, #163, or #157.
