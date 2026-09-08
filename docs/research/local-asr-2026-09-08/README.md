# Local ASR inventory and decision record — 2026-09-08

**Checkpoint draft — independent review pending; session paused for cmux.**

Research spike for [#222](https://github.com/Magnus-Gille/sagascript/issues/222).
**Decision: retain the current language defaults.** Prioritize five new families for
controlled local evaluation: Qwen3-ASR, Parakeet v3, Nemotron 3.5, Meta omnilingual
CTC and Gemma 4. The [batch-quality assessment](batch-quality.md) follows Magnus’s
clarification: larger models remain eligible for quality-first batch jobs. None is an adoption winner. This spike uses the
issue's permitted **per-candidate blocker** outcome: no new inference benchmark was
run, and there is no measured cross-engine Pareto frontier.

Only fully local inference is eligible, following the owner's issue clarification.
Cloud/API offerings, including hosted Whisper, are outside the candidate set. A
local model downloader is not permission to send audio to its publisher. No engine,
download registry, application default, cloud option or automation changes here.

## Evidence and reproducibility

| File | Purpose |
|---|---|
| [current-baselines.json](current-baselines.json) | All 21 source-registered models, pinned URLs, expected hashes/bytes, optional CoreML archives and current language defaults |
| [publisher-metadata.json](publisher-metadata.json) | Dated selected inference-file metadata from 37 publisher/community repositories, with repository SHAs and LFS hashes/bytes |
| [runtime-assets.json](runtime-assets.json) | NVIDIA C++ runtime's six macOS/Windows release archives, sizes and publisher hashes |
| [inventory.json](inventory.json) | Families, variants, language qualifications, architecture, runtime paths, shortlist decisions and missing measurements |
| [quality-evidence.json](quality-evidence.json) | Separate publisher quality evidence for batch research, including Gemma language-specific scores |
| [comparisons.json](comparisons.json) | Published and prior local accuracy comparisons, absolute/relative effects, comparability flags and explicit unknowns |
| [verify.py](verify.py) | Offline consistency checks; does not download or run models |

All new source observations are dated 2026-09-08. Publisher file metadata is not a
local hash verification. Files in one repository can be alternative formats; do
not sum every listed file. Configs, some tokenizers, wheels, drivers, caches and
unpacked runtime sizes are not completely inventoried. Known file subtotals are
**lower bounds on download cost**, not installed footprint or RAM estimates.
`null` means unmeasured/unknown, never zero. No absent benchmark is a quality loss.

The source baseline is Sagascript commit
`9e76c364de3ac17b2c3f56478644458779a39eec`. The separately inspected installed CLI
reported `1.1.3`, git `e4db9c6`, built `2026-09-07`; it is not that source revision.
No installed-model digest or inference result was inferred from its version.
The research host was Apple M4, 32 GiB, macOS 26.6.2 (25G83), arm64. There was no
Windows test host. The inspected Python environment lacked MLX Audio, NeMo,
omnilingual-asr and qwen-asr. No candidate runtime, weights or corpus were installed
by this spike. Findings below distinguish source capability, publisher claims and
previously measured behavior.

## Two baselines, not one

The current product defaults are English `BaseEn`, Swedish `KbWhisperBase`,
Norwegian `NbWhisperBase`, Finnish `Base`, and automatic language `Base`. The app's
`no` language does not establish separate Bokmål/Nynorsk accuracy. Finnish Tiny is
already an opt-in model; the earlier inventory predating that addition is stale.
The canonical artifacts/configuration come from
[settings/manager.rs](../../../src-tauri/crates/sagascript-core/src/settings/manager.rs).

For matched architecture comparisons, retain generic OpenAI Whisper tiny/base/
small/medium and large-v2/large-v3 as relevant references; for English also retain
`.en` variants. For a practical budget comparison include large-v3-turbo and its
Q8 artifact. The dated `ggerganov/whisper.cpp` snapshot includes these experiments;
large-v3 is a reference candidate, not a claim that it is registered in the app.
“Latest” means the relevant published Whisper generation at this observation date,
not the largest available file or a floating `main` download during a benchmark.

| Current/reference artifact | Main model MB (decimal) | Optional CoreML archive MB |
|---|---:|---:|
| Generic tiny / base | 77.69 / 147.95 | 15.04 / 37.92 |
| English base.en | 147.96 | 37.95 |
| KB or NB tiny / base / small Q5_0 | 29.88 / 55.30 / 175.21 | none in registry |
| KB or NB medium / large Q5_0 | 539.21 / 1,081.14 | none in registry |
| Generic small / medium | 487.60 / 1,533.76 | 163.08 / 567.83 |
| Generic large-v3-turbo / Q8_0 | 1,624.56 / 874.19 | 1,173.39 for either |
| Finnish Whisper tiny | 77.69 | none in registry |

CoreML is a separate, optional download. Its ZIP bytes do not equal its unpacked
size; a compatible encoder must actually load before claiming ANE acceleration.
KB/NB Q5_0 versus unquantized generic Whisper is **not** precision matched, even
when architecture names and parameter counts match. Future runs must compare
like quantization or explicitly measure the quantization effect.

## Matrix A: language-specialized versus generic Whisper at matched architecture size

These are publisher benchmarks except the last row. WER is percent; Δ is candidate
minus generic in percentage points. Relative improvement is `(generic-candidate)/generic`.
They support candidate selection, not current Q5 artifact performance or adoption.
Each source's corpus/normalizer/decoder remains distinct in the JSON.

| Language / corpus | Size | Generic WER → specialized WER | Δ pp | Relative reduction |
|---|---|---:|---:|---:|
| SV / FLEURS | tiny | 59.2 → 13.2 | -46.0 | 77.70% |
| SV / FLEURS | base | 39.6 → 9.1 | -30.5 | 77.02% |
| SV / FLEURS | small | 20.6 → 7.3 | -13.3 | 64.56% |
| SV / FLEURS | medium | 12.1 → 6.6 | -5.5 | 45.45% |
| SV / FLEURS | large | 7.8 → 5.4 | -2.4 | 30.77% |
| NB / FLEURS | tiny | 76.4 → 15.2 | -61.2 | 80.10% |
| NB / FLEURS | base | 56.8 → 11.5 | -45.3 | 79.75% |
| NB / FLEURS | small | 29.6 → 8.3 | -21.3 | 71.96% |
| NB / FLEURS | medium | 15.5 → 7.2 | -8.3 | 53.55% |
| NB / FLEURS | large | 10.4 → 6.6 | -3.8 | 36.54% |
| NN / Common Voice | tiny / base / small | >100 → 28.0 / 23.2 / 19.9 | censored | not computable exactly |
| NN / Common Voice | medium | 60.2 → 17.0 | -43.2 | 71.76% |
| NN / Common Voice | large | 30.0 → 12.6 | -17.4 | 58.00% |
| FI / previous local FLEURS DEV | tiny | 72.96 → 51.19 | -21.77 | 29.84% |

Sources: [KB model card](https://huggingface.co/KBLab/kb-whisper-small),
[NB Whisper paper, tables 1–3](https://www.isca-archive.org/interspeech_2024/kummervold24_interspeech.pdf),
and [R4 Finnish screening](../r4-finnish-screening-results.md).
The JSON additionally preserves the NB NST comparisons. Publisher benchmark
artifact revisions, exact tensor precision, hardware and paired confidence
intervals are not established here. Do not attach current download SHAs to those
historical scores. There is no newly harmonized English `.en` versus multilingual
Whisper measurement; both remain references rather than an invented matrix row.

The prior Finnish run used 48 clips, 552.06 seconds and 758 normalized words on
Apple M4/32 GiB. It reported Tiny/Base/Finnish Tiny WER 72.96/55.01/51.19 and CER
21.91/14.72/13.72. Cold median/p95 inference times were 678/911, 1169/1544,
638/1197 ms; warm median/p95 were 255/392, 405/604, 263/417 ms. It used greedy `fi`,
temperature fallback, no prompt/VAD, one cold plus five warm calls per clip/model,
seed 20260906, 864 successful calls. Accuracy was taken from the **first cold** run,
whereas #187 specifies first warm. Cold meant new process, not flushed OS cache.
The original report pins source, binary, models and manifest. This was development
data used for selection, not new held-out evidence. No accelerator identity,
peak RAM or key-release-to-visible-text latency was measured. Finnish Tiny versus
Base is cross-size (6.95% relative WER reduction); it does not justify a default
change or a same-budget claim.

## Matrix B: practical download, installed disk, RAM and latency budgets

These rows identify *experiments*, not equivalent sizes. The parameter labels in
model names are insufficient: serialized full-chain Qwen “0.6B” has 938,008,576 BF16
tensor elements; Gemma “E2B” has 5,123,178,051 in its BF16 checkpoint. Packed integer
counts in quantized files and tied duplicates must not be interpreted as independent
trainable parameters. Architecture totals, tensor metadata and actual file bytes
are separately recorded.

| Exact artifact choice | Known model/auxiliary bytes | Known macOS / Windows runtime archive | Installed disk / peak RAM / cold + warm p50,p95 / visible-text latency |
|---|---:|---:|---|
| Qwen3-ASR-0.6B MLX 4-bit | 708,236,945 | unmeasured / MLX not a Windows path | all unmeasured |
| Qwen3-ASR-0.6B source BF16 | 1,876,091,704 | unmeasured / unmeasured | all unmeasured |
| Parakeet v3 official Q8_0 GGUF | 713,975,456 | 3,465,028 Metal / 4,730,421 CPU | all unmeasured |
| Nemotron 3.5 official Q8_0 GGUF | 741,548,352 | 3,465,028 Metal / 4,730,421 CPU | all unmeasured |
| Meta CTC 300M v2 | unknown exact bytes/hash | unverified / unverified | all unmeasured |
| Meta CTC 300M v1 reference + tokenizer | 1,302,201,454 | unmeasured / unverified | all unmeasured |
| Moonshine Streaming Tiny HF F32 | 176,237,308 | unmeasured / native export not pinned | all unmeasured |
| Gemma 4 E2B QAT Q4_0 + mmproj | 4,336,349,920 | unmeasured / unmeasured | all unmeasured |
| Canary v2 F32 safetensors | 3,916,173,816 | unmeasured / unverified | all unmeasured |
| RASMUS Finnish Canary R2 | checkpoint absent from inspected file list | unverified / unverified | all unmeasured |
| Finnish XLS-R 300M + KenLM + unigrams | 2,298,379,008 | unmeasured / unverified | all unmeasured |
| Granite Speech 5.0 470M TurboCTC | 946,180,704 | unmeasured / unverified | all unmeasured |

File-level identity and provenance: [publisher metadata](publisher-metadata.json).
For NVIDIA the known model + Metal archive subtotals are 717,440,484 and
745,013,380 bytes; with Windows CPU archives 718,705,877 and 746,278,773 bytes.
Dependencies and expanded installation remain additional. This prevents claiming
“714 MB installed” or inferring RAM from Q8 weight size.

[NeMo-Speech.cpp v0.1.0](https://github.com/NVIDIA/NeMo-Speech.cpp/releases/tag/v0.1.0)
provides macOS arm64 Metal/CPU, macOS x64 CPU and Windows x64 CPU/Vulkan/CUDA
archives. No Windows ARM64 archive was present. Publisher documentation supports
both selected GGUF families, but neither binary compatibility nor performance was
executed here. Windows lacks the same text-normalization path; use raw decoder
output under one scorer. MLX alternatives exist for Qwen, Parakeet and Nemotron in
[MLX Audio](https://github.com/Blaizzy/mlx-audio); they cannot supply Windows evidence.

Gemma's two files are the actual
[QAT GGUF pair](https://huggingface.co/google/gemma-4-E2B-it-qat-q4_0-gguf).
[llama.cpp audio support](https://github.com/ggml-org/llama.cpp/pull/21421) merged at
`547765a93e5ad7b4e8ca84d78f6d83f36ad8ee25`; that is evidence of a local C++ route,
not a tested macOS/Windows build for this pair. The
[Gemma 4 card](https://ai.google.dev/gemma/docs/core/model_card_4) distinguishes
E2B/E4B/12B audio models from non-audio variants; audio segments are limited to
30 seconds. Aggregate FLEURS results do not establish SV/FI/NB/NN dictation quality.

## Broad inventory, shortlist and blockers

The machine-readable inventory covers Whisper regional fine-tunes and the major
new families plus secondary discovery paths. Target-language membership is not
proof of usable quality. English-only candidates can complement Nordic engines;
they do not replace them merely because they rank well in an English leaderboard.

| Priority (five families) | Target contribution | Concrete blocker before comparable results |
|---|---|---|
| Qwen3-ASR 0.6B, first MLX 4-bit | EN/SV/FI; no Norwegian in listed language set | MLX runtime absent in inspected environment; scorer adapter missing; Windows needs its own frozen runtime/conversion; fresh held-out corpus absent |
| Parakeet TDT 0.6B v3, official Q8 | EN/SV/FI | Runtime/weights not installed or executed for this spike; scorer adapter and fresh corpus missing; Windows host absent |
| Nemotron 3.5 ASR 0.6B, official Q8 | Streaming; EN ready, SV/FI/NB broader coverage | Same execution/corpus blockers; incremental output stability/finalization adapter missing; NN is adaptation-ready rather than ready-to-use |
| Meta omniASR CTC 300M v2 | Broad coverage; EN/SV/FI/NB language identifiers found | Exact v2 byte size/hash and product-platform runtime not frozen; scorer/corpus absent; NN eligibility unverified |
| Gemma 4 E4B / 12B (E2B reference) | Quality-first batch; language-specific Nordic quality unknown | Separate exact runtime/conversion pins for each architecture, fresh corpus and batch adapter missing; E2B C++ support does not establish 12B compatibility |

Language/streaming sources: [Qwen](https://huggingface.co/Qwen/Qwen3-ASR-1.7B),
[Parakeet](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3),
[Nemotron](https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b),
[Meta](https://github.com/facebookresearch/omnilingual-asr),
[Meta language inventory](https://github.com/facebookresearch/omnilingual-asr/blob/main/src/omnilingual_asr/models/wav2vec2_llama/lang_ids.py),
[Moonshine Tiny](https://huggingface.co/moonshine-ai/moonshine-streaming-tiny).
Meta's inspected language list contained `eng_Latn`, `swe_Latn`, `fin_Latn`,
`nob_Latn`, but not `nno_Latn`; this is a coverage lead, not CTC per-language accuracy.
The official v2 checkpoint is linked in the Meta README, not the guessed HF v2 ID.
Moonshine's Transformers path is offline; it does not by itself validate native
streaming. Nemotron chunk/lookahead settings (80–1120 ms) do not measure actual
partial arrival, stability or finalization.

Reserves and exclusions:

- **Canary/RASMUS:** local Finnish specialization remains interesting, but the
  [RASMUS card](https://huggingface.co/RASMUS/Finnish-ASR-Canary-v2) reports greedy
  generic→R2 WER 17.95→5.41 on Common Voice, 7.79→8.39 on FLEURS and 7.96→13.91 on
  VoxPopuli. Normalization and number representation differ. Its referenced
  `models/canary-finnish-v2.nemo` is absent from the inspected pinned repository.
  Do not silently replace it with generic Canary or choose LM settings after seeing
  the test set. Generic Canary, RASMUS and LM-assisted runs are separate configs.
- **Finnish Whisper:** actual Tiny/Medium/Large-v2/Large-v3 and GGML variants were
  inspected; no Finnish Base checkpoint was identified. Larger variants remain
  references, not automatic upgrades. **Finnish XLS-R+LM** is a distinct decoder
  chain whose LM adds over a gigabyte. See
  [Finnish-NLP models](https://huggingface.co/Finnish-NLP/models).
- **Granite Speech 5.0 TurboCTC:** current English 470M alternative, with 946 MB
  weight file and documented Transformers/MLX routes. It remains an English
  reserve while the five-family first pass prioritizes multilingual coverage and
  the requested Gemma batch investigation.
  [Publisher card](https://huggingface.co/ibm-granite/granite-speech-5.0-470m-turboctc).
- **Gemma 4:** shortlisted for batch quality following the owner clarification;
  exact platform builds and Nordic quality remain unverified. Moonshine becomes
  the compact English reserve. See [batch-quality.md](batch-quality.md).
- **Distil-Whisper, Kyutai STT, SenseVoice, GLM-ASR, Cohere Transcribe and local
  Voxtral Realtime:** inspected secondary English paths; exact promotion would
  require full artifact/dependency pins and platform tests. **MMS** has a
  noncommercial license; **Seamless and VibeVoice ASR** remain discovery backlog
  with variant/coverage/license/chain gaps. Their individual primary links and
  exclusion reasons are in [inventory.json](inventory.json). These are explicit
  evidence gaps, not an assertion that no usable models exist.
- Meta's SSL encoder alone is not ASR. Its LLM “300M” variant has a substantially
  larger complete chain than the 325,494,996-parameter CTC model. Larger LLM variants
  are outside the five-family first pass. Do not confuse VibeVoice TTS with ASR.

Publisher license metadata is preserved per artifact: notably Nemotron's
OpenMDW-1.1, Gemma/Qwen/Meta Apache-2.0 and Moonshine MIT. Community conversions
need provenance and redistribution checks before bundling. Access-gated terms
were not accepted. No cloud/API candidate appears in the ranking.

## Language and resource recommendations

These recommendations concern interactive dictation. [Batch jobs](batch-quality.md)
use a separate quality/throughput decision and retain larger candidates.

The following **download-only triage bands** help schedule research. They are not
product RAM limits, matched budgets, or a performance ranking. An adoption proposal
must also fix an installed-disk cap, peak-RAM cap and end-to-end latency target on
each named hardware class before running held-out evaluation. Those caps remain an
explicit follow-up input; no engine has been proven to fit them here.

| Language | ≤250 MB model tier | 250 MB–1 GB tier | >1 GB tier |
|---|---|---|---|
| EN | Retain BaseEn; research Moonshine Tiny | Research Parakeet/Nemotron/Qwen 4-bit; Granite reserve | Generic Whisper and Qwen 1.7B references; Gemma batch research |
| SV | Retain KB Base; KB Small already optional | Research Parakeet/Nemotron/Qwen; KB Medium reference | KB Large/generic Whisper references; Meta CTC research |
| NB | Retain NB Base; NB Small optional | Research Nemotron, with explicit Bokmål scoring | NB Large reference; Meta CTC only after runtime eligibility |
| NN | Retain current NB Base pending NN held-out results | No demonstrated replacement; do not label Nemotron adaptation-ready as supported | NB Medium/Large references; resolve Meta NN eligibility first |
| FI | Retain generic Base; Finnish Tiny stays opt-in | Research Qwen/Parakeet/Nemotron | Finnish Whisper variants and Meta CTC research; RASMUS blocked; Gemma batch research |

A converted Qwen artifact fitting the download tier on Apple Silicon does not prove
that the Windows chain fits it. No recommendation to **change** a default passes
the evidence gate. “Retain” describes product continuity; it is not proof that the
current defaults are globally optimal.

## Follow-up experiment using #187, not a second evaluator

Reuse [the dictation protocol](../../dictation-evaluation-protocol.md) and
[scripts/dictation_eval](../../../scripts/dictation_eval). The existing manifest
supports `en`, `sv`, `no`, `fi`; add separate NB/NN strata/language metadata through
[#187](https://github.com/Magnus-Gille/sagascript/issues/187) before Norwegian
comparisons. Do not pass unsupported `nb`/`nn` IDs to today's CLI or relabel the
old `no` corpus without checking its transcripts.

1. Freeze candidate artifacts, full runtime/dependency versions and hashes,
   hardware/OS/backend, decoder, quantization, language hints, prompts, VAD,
   resampling, chunk/lookahead and normalization. Hash any conversion input/output.
   Lock practical download/disk/RAM/latency budgets before held-out execution.
2. Prepare licensed local audio: at least 10 development and 40 fresh held-out
   human clips per language, at least two speakers, dialect/noise variation,
   names, numbers, negation, code-switching and silence. Treat NB and NN separately.
   Review training-set overlap; public FLEURS/Common Voice scores can overlap
   training and do not substitute for fresh holdout. The previous Finnish set is
   DEV. Obtain human transcript review for Finnish; Magnus cannot be its sole judge.
3. Extend #187 with a trusted local engine adapter/import contract preserving clip
   IDs, raw text, configuration hashes, failures and event timing. Current
   `run-plan` expects Sagascript; there is no existing arbitrary-engine flag to use.
   Freeze shortlist/configuration decisions on DEV. The protocol's ≤3 configurations
   per language still applies; five families is a discovery shortlist, not approval
   to run all five on every held-out language and pick the best afterward.
4. Reuse the scorer: NFC/casefold/NFC, preserve letters/numbers/marks and internal
   apostrophes, punctuation separators, no accent stripping or number expansion.
   Report corpus WER from total edits/reference words, CER separately, critical
   number/negation errors, name recall and silence hallucinations. Accuracy comes
   from first warm run; preserve all repeats and failures.
5. Run at least five warm repetitions in randomized fixed-seed order, plus cold
   process runs. Record cold and warm p50/p95 (nearest rank), peak process-tree RAM,
   RTF, total download/installed bytes and key-release-to-visible-text latency.
   Measure streaming lookahead, first partial, revisions/stability and finalization
   separately. Separate model loading from decoder time and the actual app path.
6. Pair on identical utterances; report Δ WER percentage points, relative WER
   reduction and paired utterance-bootstrap confidence intervals. Never bootstrap
   words as independent samples. Stratify results by language/platform and show
   quality/latency/disk/RAM tradeoffs; report Pareto dominance only when all relevant
   measured dimensions support it. Repeat separately on Apple Silicon, Windows x64
   and Windows ARM64 when supported; an unavailable platform stays blocked.
7. Apply #187's adoption gate: ≥20% lower warm p95 **end-to-end** latency OR ≥15%
   relative WER reduction, with no >1 percentage-point WER regression, no >2-point
   name-recall regression, no new number/negation/silence failure, representative
   data and uncertainty reviewed. Inference-only timing cannot satisfy the latency
   gate. A model/default change needs its own reviewed proposal.

Current evaluator entry points can be inspected without downloading models:

```sh
python3 scripts/dictation_eval/evaluate.py --help
python3 -m unittest discover -s scripts/dictation_eval -p 'test_*.py'
python3 docs/research/local-asr-2026-09-08/verify.py
```

The existing subcommands include `validate-manifest`, `freeze-plan`, `run-plan`,
`score-clip` and `summarize-run`. Use the script entry point; module invocation is
not supported by its current sibling imports. Once the runtime is installed and
pinned, NVIDIA's documented local shape is
`nemo-speech transcribe /path/audio.wav --model /path/exact.gguf`; this is not an
executed experiment or the missing #187 adapter.

## Integration and regression map

This spike introduces no integration. A subsequent engine proposal must cover:

| Area | Minimum work and regression evidence |
|---|---|
| CLI capability | Local model identity/config, transcription, diagnostics and machine-readable timing available through canonical CLI before UI exposure |
| Core engine | Adapter for load/transcribe/cancel, typed failures, no silent fallback, bounded caches, chunking, timestamp semantics, partial/final text and main-thread constraints |
| Registry/downloads | Pinned checksums for every model/encoder/tokenizer/LM, full-chain disk accounting, interrupted/corrupt download tests, no automatic network inference |
| Dictation path | Key-up through actual text insertion, no duplicate/stale partials, cancellation/restart, silence, numbers/negation, Unicode and language changes; connect latency work to #163 |
| Evaluation | Extend #187 schema/adapter once; old scorer fixtures remain unchanged; fresh NB/NN and Finnish transcript review; paired source/quantized accuracy checks |
| Platform packaging | Clean offline macOS signed-app run; Windows x64 and ARM64 dependency/backend paths, capture platform blockers in #184; no assumed MLX portability |
| Privacy | After explicit model provisioning, transcription succeeds with outbound networking denied; missing files fail locally, audio/text are not uploaded |

These are proposed follow-ups to existing issues, not newly posted tickets or
implicit implementation scope. A future implementation runs the repository Rust
workspace checks and, for frontend changes, Svelte checks; documentation/data-only
work here does not require rebuilding or altering the installed app.

## Lightweight recurring review proposal

Manually revisit quarterly, or when a credible local model adds a target language,
materially changes total artifact cost, or gains a product-platform runtime. Refresh
publisher metadata, license and runtime availability first. Promote at most five
families to DEV screening, then freeze a smaller held-out plan. Keep a stable
regression corpus and add a fresh held-out slice to reduce repeated-selection bias.
Record a dated decision and exact baseline revision. Schedule no job and switch no
model automatically. Track missing artifacts/runtime/language evidence explicitly
so the next review starts from unresolved questions rather than repeating discovery.

## Completion and verification

The inventory, both comparison matrices, five-family shortlist with individual
blockers, per-language/resource recommendations, evaluator reuse and integration/
regression plan cover #222's research deliverables through its blocker alternative.
Independent review and final acceptance reconciliation remain pending.
Actual candidate measurements and adoption are deliberately unresolved follow-ups.
No universal local winner, Windows performance result or new default is claimed.

Validation commands and independent review outcome are recorded in
[verification.md](verification.md). M5 was attempted for a bounded arithmetic leaf
using `qwen3-coder-next-80b`, but timed out without output. Doctor reported reachable
public/private transport and did not validate inference; conductor calculations
and deterministic checks replaced that leaf. Adoption outcome: `redo`, not useful
M5 work.
