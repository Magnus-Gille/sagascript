# R4 Finnish screening plan — frozen before inference

This is model-selection screening, not native-speaker acceptance or a claim of
independent held-out quality. Finnish remains the R4 core language. Additional
languages are conditional on suitable models, licensing, datasets and manageable
local resource requirements; there is no obligation to fill all three extra slots.

## Candidates and attribution

| Model | Role | Pinned GGML bytes | SHA-256 |
|---|---|---:|---|
| Generic multilingual Base | Existing R4 default | 147,951,465 | `60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe` |
| Generic multilingual Tiny | Size-matched baseline | 77,691,713 | `be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21` |
| Finnish-NLP Tiny | Optional specialist candidate | 77,691,730 | `41cf309b7f50523cfca724ae90924fcd0e4794205de57a66abc3cce627103ce8` |

Generic artifacts come from `ggerganov/whisper.cpp` at
`5359861c739e955e79d9a303bcbc70fb988958b1` (MIT). The Finnish artifact comes from
`Finnish-NLP/Finnish-finetuned-whisper-models-ggml-format` at
`c58924b6deb4438756b3d38ecd67d65bdf20298d` (declared Apache-2.0).
See [the model manifest](../model-sources.md) for source relationships and limitations.

## Fixed data and execution

- Reuse the 48 Finnish FLEURS **test** fixtures selected in the earlier language
  spike: first 24 distinct female and 24 distinct male sentence IDs in source TSV
  order, interleaved. Total audio 552.06 seconds; individual clips 5.22–21.42 seconds.
- Dataset: [Google FLEURS](https://huggingface.co/datasets/google/fleurs), Finnish
  `fi_fi`, revision `70bb2e84b976b7e960aa89f1c648e09c59f894dd`, CC-BY-4.0. Preserve
  exact fixture audio/reference hashes with the run. Never infer speaker identity
  from sentence IDs or present gender-balanced selection as speaker-balanced.
- These fixtures were previously inspected in another model comparison. FLEURS
  train/test overlap with the underlying model lineage is not fully established.
  They are reusable screening data, not a new untouched acceptance set.
- Use one exact committed source revision and one separately built, SHA-bound
  Sagascript CLI binary for all three models. Preserve build identity and compiler
  configuration. Never overwrite frozen historical benchmark binaries.
- Use the production `benchmark-dictation` path with explicit Finnish, greedy
  decoding and **temperature fallback enabled**, no prompt or VAD. Collect one
  fresh-process cold sample and five warm in-process samples per clip/model.
- Shuffle the 144 clip/model pairs deterministically with seed `20260906`.
  Run sequentially without concurrent compilation or inference. Preserve all
  failures and timeouts; never substitute a successful retry silently.
- The measurement endpoint is inference completion, NOT visible pasted text.
  Cold here includes fresh-process model preparation/loading, not an OS disk-cache
  flush. Report actual acceleration configuration; no claim of signed GUI latency.

## Decision rule, fixed before results

Compare corpus WER/CER on identical references with the existing evaluator's
normalization, retaining Finnish letters. Report cold and warm timings separately,
and per-clip variation. Do not cherry-pick the best repetition as the quality score:
report the first cold transcript consistently and summarize warm variation separately.

To advance the specialist toward the default, require all of:

1. At least 10% relative corpus WER improvement over generic Tiny, demonstrating
   specialization rather than a larger parameter budget.
2. No more than 2 absolute percentage points worse corpus WER than generic Base.
3. No regression in warm median or p95 inference latency versus generic Base.
4. No unresolved execution/integrity failures or severe transcription regressions
   found in the retained outputs.

These are a screening go/no-go, not statistical or native-speaker acceptance.
Rejecting the candidate leaves the existing Base default unchanged; smaller file
size alone does not justify shipping inferior recognition. A result near a boundary
is inconclusive and requires additional independent evidence, not threshold changes.

## Separate release/acceptance checks

Silence and short-word regression checks, real microphone/headset behavior, signed
Mac and native Windows cold/warm behavior, memory pressure, and spontaneous Finnish
from a native speaker remain separate. Do not equate FLEURS read speech with those
checks. A clearly labelled test build may disclose pending human acceptance; it must
not claim those gates passed or install/change user profiles automatically.

Keep raw audio, reference text, model weights and detailed hypotheses outside the
repository. Publish only the content-free aggregate, artifact identities, method,
limitations and decision after review.
