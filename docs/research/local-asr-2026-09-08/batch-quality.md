# Quality-first local batch assessment

Added following Magnus's clarification on 2026-09-08: larger models remain
interesting for batch jobs even when unsuitable for interactive dictation. Weight
size is a scheduling constraint, not a quality verdict. **Research Gemma 4 E4B/12B
alongside specialist ASR; do not rule them out on key-up latency.** No new engine
was run, so this is an evidence-backed experiment recommendation, not an adoption.

## What published quality actually tells us

All numbers below are publisher results, not a shared run. WER percentages are
lower-is-better. Different prompts, splits, normalization and decoding prevent a
cross-publisher winner claim. [quality-evidence.json](quality-evidence.json)
preserves metric identity, language, source and missing comparability information.

| Model / configuration | EN FLEURS WER | SV FLEURS WER | FI FLEURS WER | Reading for batch research |
|---|---:|---:|---:|---|
| Gemma 4 E2B | 8.0% | not reported | not reported | Audio baseline within Gemma family |
| Gemma 4 E4B | 6.5% | not reported | not reported | Material EN improvement over E2B; first Gemma batch candidate |
| Gemma 4 12B Unified | 6.3% | not reported | not reported | Only 0.2 pp better than E4B in this EN table; quality ceiling experiment |
| Qwen3-ASR 0.6B / 1.7B | 4.39% / 3.35% | not individually reported in inspected table | not individually reported | 1.7B gains quality over smaller Qwen; inspect Nordic strata before preference |
| Parakeet TDT v3 | 4.85% | 15.08% | 13.21% | Useful throughput candidate, but multilingual coverage does not guarantee Nordic quality |
| Canary 1B v2 | 4.50% | 9.57% | 8.59% | Batch candidate with explicit Nordic quality evidence and long-form results |

Sources: [Gemma technical report, tables 7–8](https://arxiv.org/html/2607.02770v1),
[Qwen model card](https://huggingface.co/Qwen/Qwen3-ASR-1.7B),
[Parakeet metadata/card](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3),
[Canary metadata/card](https://huggingface.co/nvidia/canary-1b-v2).

Gemma's paper uses WER for English but CER for Korean/Japanese/Chinese. Its mixed
aggregate must not be called corpus WER. The paper's E4B aggregate is 0.075; the
model card rounds it to 0.08. The card's 12B 0.069 also excludes Chinese. These
aggregates do not provide Nordic accuracy or a clean cross-model comparison.
The selected GGUFs were not established as the artifacts behind the paper scores.

Qwen's extended ten-language FLEURS group, including SV/FI, reports Whisper
large-v3 **8.16%**, Qwen 0.6B **21.80%**, Qwen 1.7B **12.60%**. That is a meaningful
counterweight to its strong English results; it cannot isolate Swedish or Finnish.
Its 1.7B English FLEURS offline/streaming figures are **3.35% / 4.02%**, directly
illustrating why batch and streaming quality should be evaluated separately.

Canary reports long-form WER **13.78% on Earnings-22** and **9.87% on This American
Life**, using dynamic chunking with one-second overlaps. Its shorter-form
Earnings-22 leaderboard result is 11.79%; do not interchange them. Reported RTFx
749 comes from the publisher benchmark setup, not this Mac. Its metadata FI score
8.59 differs from RASMUS's generic baseline 7.79; these are separate evaluations.

NB/NN batch quality remains anchored in NB Whisper's language-specific evidence.
Nemotron's Bokmål coverage is promising but does not supply a measured local
quality advantage. Nynorsk requires its own evidence. Meta CTC and larger LLM
variants remain research paths; a larger or omnilingual model name cannot fill
missing target-language scores. Finnish Whisper larger variants and RASMUS remain
quality candidates, with the latter blocked by the missing exact checkpoint.

## Why Gemma is still worth investigating

The source checkpoint files for E2B/E4B/12B total **10.25/15.99/23.92 GB** respectively;
these are weight downloads, not RAM. E2B's known QAT GGUF + mmproj pair is 4.34 GB.
E4B and 12B quantized batch artifacts are not frozen in this spike; do not estimate
them by scaling the E2B filename. The 32 GiB host's RAM alone does not prove or
rule out a run: backend allocations, KV cache, activations and concurrency matter.

Gemma may be useful for a quality-oriented offline second pass, audio translation,
or structured extraction. Those are **hypotheses**, separate from faithful ASR.
First preserve an unmodified transcript. Test semantic enrichment separately;
summaries or polished prose must not silently replace what was said. Treat spoken
instructions as audio content, not permission for a batch worker to use tools.

The [Gemma audio documentation](https://ai.google.dev/gemma/docs/core/model_card_4)
limits audio input to 30 seconds despite its large text context. Long files need
segmentation and merge logic. Test split words, repeated context, omissions,
number formatting and hallucinations at segment boundaries. E2B/E4B's established
llama.cpp encoder route does not prove 12B Unified runtime compatibility; 12B uses
a different architecture and needs its own pinned local Transformers or C++ path.
Neither 31B nor 26B A4B is an audio-input Gemma candidate in this release.

Google's suggested ASR prompt converts spoken numbers to digits. Since #187's
scorer does not expand numbers, freeze the transcript convention and prompt before
testing. Report verbatim-ASR accuracy separately from a deliberately formatted
output task; a prompt mismatch must not masquerade as an acoustic error reduction.

## Batch experiment priorities and acceptance

The global first-pass shortlist remains capped at five families. With the clarified
batch interest, **Gemma replaces Moonshine in that shortlist**. Qwen, Parakeet,
Nemotron and Meta CTC remain; Moonshine is a compact English reserve. Canary is a
strong batch reserve, to promote in place of a blocked shortlisted family, not as
an unbounded sixth held-out contender. Screening order and held-out configurations
must be frozen before collecting comparative results.

Within those families, batch DEV screening should examine Qwen 1.7B as well as
0.6B and Gemma E4B before 12B. Larger Whisper language specialists are baseline
references. No size-based rejection applies to the quality assessment. If available
hardware cannot run a candidate, preserve its published quality evidence and name
the missing RAM/runtime/host requirement instead of calling its quality poor.
No remote host deployment, package installation or model execution is implied.

Reuse #187 normalization, clip identity and critical-error scoring; add a batch
run adapter and recording-level manifest through that evaluator. For long-form
holdout, include complete meetings/interviews with speaker changes, overlap,
silence, noise and technical names in each target language. Split train/DEV/holdout
by recording and speaker to prevent adjacent chunks leaking across splits. Report
both segment diagnostics and reconstructed full-recording WER/CER. Bootstrap
recordings (or speakers), not correlated adjacent chunks as independent utterances.

A proposed **batch-only gate**, to freeze before execution, is ≥15% relative corpus
WER reduction against the current batch baseline, no >1 pp regression in any
required language, no >2 pp name-recall loss, and no new number/negation/silence
failure, with paired uncertainty reviewed. This does not change #187's dictation
gate. Choose deadline, peak RAM and installed-disk ceilings for the actual batch
host separately; interactive key-up latency is irrelevant here. Until those
ceilings and the batch corpus are frozen, “batch-ready” remains unproven.

Record total job wall time, audio-hours processed per wall-hour, RTF, peak RAM,
batch size/concurrency, load time amortization, disk/temp usage, failed or truncated
segments, boundary errors, retry/resume behavior, and completed transcript
coverage. If timestamps or diarization are required, evaluate them separately and
include their extra models/runtime cost. A proposed two-pass run must include both
passes in cost and latency and prove that second-pass corrections improve accuracy.

This permits a useful outcome such as “higher quality, slower, appropriate for an
overnight job” without pretending it belongs in the dictation hot path. The current
evidence justifies testing that possibility; it does not yet identify the winner.
