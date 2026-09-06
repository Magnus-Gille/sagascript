# Cross-product dictation evaluation protocol

Status: draft, freeze-ready protocol for issue #196.  This document records the
comparison design and the public capability evidence available on 2026-09-06;
it contains no measurements, benchmark results, or product recommendation.
The source, model, machine, order, and evaluator must be frozen before any
data capture.  Unicode casefolding can change the post-NFC token stream, so the
normalization/runtime version is part of that freeze.  Until the required runs
and evidence ledger are complete, every comparison is `UNMEASURED` or
`INCONCLUSIVE` and issue #196 remains open.

## Scope and non-goals

The comparison is Sagascript versus Handy, FluidVoice core, and Vibe on the
same named Apple Silicon machine.  OpenWhispr and VoiceInk are optional only if
their exact build, license, local-processing mode, and repeatable measurement
surface can be verified before the freeze.  No product is judged from a
marketing claim, an undocumented flag, or a run using a different audio file.

The evaluation is local-only.  It does not purchase software, upload private
audio, enable a cloud service, add a model backend, or change a product's
defaults.  A UI-only product is not forced into a batch interface: it gets a
desktop result or an explicit `N/A (no supported automation surface)` entry.

Two lanes are mandatory and must never be merged:

1. **Same-model/settings lane.**  Run only the intersection of a product's
   documented local backend, language, model, precision/quantization, VAD,
   diarization, and decoding settings.  An unsupported intersection is `N/A`,
   not a recognition failure.  This lane compares the inference configuration.
2. **Product-default lane.**  Use each product's documented, practical local
   defaults, recording the exact selected model and settings.  “Practical
   default” means a shipped fresh profile before tuning, for every product;
   never silently reuse a previously configured or hand-tuned profile.  This lane
   describes the user experience, not an engine-only comparison.  A product
   default that cannot be inspected or reproduced is `INCONCLUSIVE`.

The same weights used by two different runtimes are a separately labelled
`same_weights_different_runtime` subcase, not evidence of same-backend parity.
Keep that subcase out of the same-model/settings aggregate.

The same source utterance bytes, language, split, order plan, timing
definitions, and acceptance rules apply to both lanes in the file/CLI path.
The desktop path uses the fixed acoustic replay procedure below and is not
byte-paired with the file/CLI path.  Do not pool file and desktop WER or report
a combined score.

## Frozen identity and evidence record

Before audio is captured, freeze a private machine-readable identity record
containing:

- product and lane IDs;
- repository URL, exact source revision, build command, binary/app SHA-256,
  and build date;
- machine model, Apple Silicon generation, RAM, macOS version, AC/low-power
  state, fixed cooldown interval, input/output devices, sample rate, and
  permission state;
- model identifier, exact model revision and weight SHA-256, model size,
  precision/quantization, VAD, diarization, language, beam/temperature and
  other decoder settings;
- corpus manifest revision and every input audio SHA-256;
- runner/protocol revision, timestamp convention, and the frozen interleaved
  product-order seed.

The binary and model checksums are identity fields, not evidence that a model
was available for every language.  A missing/unsupported model is recorded in
the availability ledger with its primary-source reason.  Raw audio hashes,
utterance IDs, private paths, raw reports, and raw diagnostics stay in this
private artifact; they are never copied into the public aggregate.

### Verified public product matrix

The following is a capability matrix, not a benchmark result.  README and
license links are pinned to the exact revisions checked on 2026-09-06.  No
release-version claim is made where a release was not independently verified.

| Product | Primary revision and license | Documented local capability | Measurement surface at this freeze |
| --- | --- | --- | --- |
| Sagascript | [source revision `4ea7f9168bb996b634e2e70780a9fb831f92700f`](https://github.com/Magnus-Gille/sagascript/tree/4ea7f9168bb996b634e2e70780a9fb831f92700f) on `main`, commit date 2026-09-06; source/build commands and first-run defaults are verified from the pinned Cargo/settings sources | Local Whisper model registry and CLI file transcription are documented in the repository; feature-gated diarization and its cache are part of the CLI surface. The default CLI build is `cd src-tauri && cargo build --release -p sagascript-cli` (default `record` + `diarization` features); the no-capture diarization variant is `cargo build --release -p sagascript-cli --no-default-features --features diarization`. At this revision, `Settings::default` uses English, automatic model selection, greedy beam `0`, temperature fallback on, and VAD off; language-specific recommendations come from the registry. | File/CLI and desktop paths are eligible. Freeze the exact build command, registry model tuple, and fresh-profile settings from source before running; do not infer a default from a previously warmed checkout. |
| Handy | README `c62a5fcdef4196e0ab36ea56cd3863f8f17fd9c5`; [README](https://github.com/cjpais/Handy/blob/c62a5fcdef4196e0ab36ea56cd3863f8f17fd9c5/README.md); [MIT license](https://github.com/cjpais/Handy/blob/c62a5fcdef4196e0ab36ea56cd3863f8f17fd9c5/LICENSE) | The README documents offline local Whisper and Parakeet V3 options, VAD, and macOS/Windows/Linux desktop use. Its CLI flags control a running instance/startup (for example, toggling transcription); they are not a documented batch-ASR interface. | Desktop hotkey path is eligible after owner-permission setup. Batch/CLI quality and latency are `N/A` unless a supported surface is verified at the frozen revision. |
| FluidVoice core | README `49f13beb7c0d96b2c7be93376fb3a7513cfe5530`; [README](https://github.com/altic-dev/FluidVoice/blob/49f13beb7c0d96b2c7be93376fb3a7513cfe5530/README.md); [GPL-3.0 license](https://github.com/altic-dev/FluidVoice/blob/49f13beb7c0d96b2c7be93376fb3a7513cfe5530/LICENSE) | The README documents a local-first macOS app, live preview, global hotkey, microphone/accessibility permissions, and model choices including Parakeet, Whisper, Apple Speech, and others. It distinguishes the GPLv3 core from the separately maintained Fluid Intelligence runtime. | Desktop hotkey path is eligible after owner-permission setup. No general batch CLI was verified from the pinned README; programmatic quality/latency is `N/A` unless a supported surface is verified without relying on private runtime code. |
| Vibe | README `e1eba11ad37dec2ef8bb5ba181394567020302c8`; [README](https://github.com/thewh1teagle/vibe/blob/e1eba11ad37dec2ef8bb5ba181394567020302c8/README.md); [MIT license](https://github.com/thewh1teagle/vibe/blob/e1eba11ad37dec2ef8bb5ba181394567020302c8/LICENSE) | The README documents local/offline file and microphone work, batch export, Whisper/Nemotron/Parakeet choices, speaker diarization, CLI support, and an HTTP API with Swagger. | File/CLI lane is a candidate. Exact command-line arguments and API request schema must be captured from this pinned revision's help/source before use; do not invent a command. Desktop lane is separate. |

The matrix's Vibe README pin above is the historical capability pin for this
protocol. The later bounded NPSC execution used Vibe revision
[`57c93c3c5a862d630459341cd71b0326a46a8a19`](https://github.com/thewh1teagle/vibe/commit/57c93c3c5a862d630459341cd71b0326a46a8a19),
which is recorded in the [dated aggregate report](./cross-product-evaluation-2026-09-06/README.md); this clarification does not
rewrite the frozen capability matrix.

The repository metadata checked the same day identifies the default branches as
`main` and the licenses as Handy MIT, FluidVoice GPL-3.0, and Vibe MIT.  The
table intentionally uses README commit revisions rather than floating `main`
links.  If a checkout differs from these revisions, invalidate the row and
freeze a replacement identity before measuring.

Public model cards and backend documentation are evidence for availability,
not evidence of accuracy.  For example, a language/model pair must be checked
against the exact model card (such as [Parakeet TDT 0.6B v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3))
before it is placed in the same-model lane.  If Norwegian, Swedish, or another
requested language is not supported by the selected model/backend, record
`N/A (unsupported language/model pair)` and retain the source URL.

## Corpus and pairing

Use the validated #187 manifest and its opaque utterance IDs.  The target is at
least 10 human development and 40 human held-out utterances per language
(English, Swedish, Norwegian), at least two held-out human speakers per
language, every required specialist/numbers/negation/ordinary tag, every
duration bucket and environment, and at least one held-out silence per
language.  Synthetic clips may exercise plumbing but never count toward human
coverage.  A manifest that does not meet this gate remains a corpus gap, not a
partial product win.

For every file/CLI run, the audio map must resolve the exact manifest SHA-256
and the byte-identical input must be used for every product.  Keep reference
text and recordings outside the repository.  Raw hashes and opaque utterance
IDs are private pairing keys; the public report contains only aggregate counts
and metrics plus public product, build, and model IDs.  Existing checked-in
test audio and diarization fixtures are smoke fixtures, not a substitute for
the #187 held-out corpus or consent/provenance evidence.

For the file/CLI lane, all products receive the same clip bytes and language
label.  The held-out assignment, utterance/configuration pairing, and
randomized order are frozen once by the #187 paired-plan helper.  Never select
a best utterance, best warm iteration, or successful subset after observing
results.  Desktop runs use the acoustic-replay procedure below instead of
claiming byte identity with file inputs.

## Execution protocol

### File and CLI path

Use the supported product CLI/API only.  Capture `--help` or the pinned API
schema as part of the identity record; preserve literal argument boundaries and
do not use shell interpolation for paths.  Sagascript's future #187 runner
integration consumes a frozen plan and carries the manifest, binary, model,
and configuration hashes through every result.  It must retain failed rows and
return a content-free summary; this comparison document does not create a
second runner.

For each configuration, collect one explicitly labelled cold call followed by
5--20 warm calls (the exact count is frozen before the run).  “Cold” means the
first call in a new backend process, not a claim about a system-cold machine.
The first warm hypothesis is the only predeclared accuracy input.  Its timing
is also retained because all 5--20 warm calls are utterance clusters for
latency p50/p95; no warm call may be removed because its timing is unfavorable.
Keep model acquisition, decode, and end-to-end phases distinct where a product
exposes them.  If it exposes only end-to-end timing, leave component fields
null rather than estimating them.

Freeze the AC/thermal state, low-power state, cooldown interval, and
interleaved product order before the first run.  Use a recorded seed and a
blocked/interleaved order to avoid giving one product all first or coolest
runs.  Do not invent or depend on an undocumented temperature API; record the
declared power/thermal controls and their observed state instead.

Run short, medium, and long duration buckets separately.  For each bucket,
report p50 and p95 over the frozen paired population and preserve the utterance
level rows needed for cluster bootstrap.  File throughput/RSS and diarization
(DER/cpWER) are separate endpoints; do not mix them into plain transcription
latency or WER.

### Desktop/live path

Desktop measurement is an owner-operated procedure requiring the owner's
microphone and Accessibility permissions.  Record the physical microphone or
headset, input level, room/noise condition, app permission state, and exact
shortcut.  Replay each private source utterance through the same calibrated
speaker at fixed distance, level, room, microphone/headset, and acoustic
condition.  Interleave products with the frozen seed and record acoustic
variation privately; do not claim byte pairing with the file/CLI lane.  Do not
install a new audio driver or virtual device merely to make a desktop product
automatable.  Use a known editor and a visible key-down/key-up marker.  Measure
key-release to preview-visible and key-release to final/committed text as two
separate endpoints; record the observer/instrumentation and resolution.  A
paste completion callback is not itself visible text and must not be reported
as such.

Do not call, automate, or claim completion of a physical desktop action from a
headless protocol run.  If permissions, hardware, editor instrumentation, or
the product's live path are unavailable, record `N/A` with the exact blocker.
Desktop results never silently stand in for a file/CLI result, and file and
desktop WER are never pooled.

### Failure ledger and coverage denominators

Every planned pair gets one canonical terminal status: `completed`,
`unsupported`, `failed`, `blocked`, or `inconclusive`.  Preserve a bounded
`reason_code` for detail: `permission_blocked` maps to `blocked`;
`build_failed`, `model_unavailable` after a supported run was started,
`cli_failed`, `timeout`, `invalid_output`, and `input_changed` map to
`failed`; and `measurement_inconclusive` maps to `inconclusive`.  A verified
unsupported language/model or absent automation surface maps to `unsupported`
(`N/A`) before execution.  The mapping is exhaustive and appears in the
coverage summary; no terminal state is silently dropped.  The uppercase
document-level states `UNMEASURED` and `INCONCLUSIVE` describe overall
protocol/report readiness; the lowercase values above are per-row ledger
statuses used in machine-readable coverage.

The ledger stores a bounded content-free error code, stage, and exit status.
Raw audio hashes, utterance IDs, private paths, raw logs, diagnostics, and
exception payloads remain private.  Sanitize any help or subprocess log before
it is copied into a public release artifact.  Keep failed, blocked, unsupported,
and timed-out rows in the machine-readable private report; do not remove them
before computing coverage denominators.

Complete-case paired intersections may be used for descriptive effects only.
All planned rows, including failures and unsupported cells, remain in the
coverage denominators and public status counts.  Predeclare
`MAX_EXCLUSION=0` for any adoption or superiority claim on required supported
rows: one failed/blocked/inconclusive required row makes that acceptance item
inconclusive.  Unsupported cells are predeclared `N/A`, not silently removed.

A timeout or identity change invalidates that row and the affected comparison
cell; it is not a zero-latency or zero-error observation.

## Scoring and uncertainty

Use the #187 normalization contract: Unicode NFC, casefold, NFC, then Unicode
word tokenization with the specified apostrophe rule.  Use the fixed reference
and first warm hypothesis for paired WER; report corpus WER as pooled errors /
pooled reference words, never the mean of clip WERs.  Report CER alongside WER
when the frozen scorer provides it, and keep specialist recall,
number/negation counts, and glossary/manual-correction burden as separate
diagnostics rather than a semantic quality claim.  Silence hallucination and
ordinary-control metrics remain separate.

Latency uses utterance-cluster paired resampling.  Accuracy uses paired
utterance resampling of the pooled WER ratio.  Fix and report the seed,
replicate count, sampling unit, and percentile method.  Keep baseline,
candidate, and every interval input in the same lane and split.  If a baseline
denominator is zero or an expected field is missing, return an inconclusive
interval rather than dropping the problematic resample.

Any p95 stratum with fewer than 40 utterance clusters is explicitly low
precision and exploratory.  It cannot support a threshold or superiority claim,
even if its point estimate looks favorable.

The 40 held-out utterances per language are split across three duration buckets,
so a per-bucket p95 may have fewer than 40 clusters and remains exploratory
under this rule.

The result may state a difference and its uncertainty; it must not turn that
difference into a recommendation until the acceptance thresholds, coverage
gate, failure ledger, and reproducibility evidence are all satisfied.  A
missing required endpoint makes the affected acceptance item inconclusive.

## Machine-readable public aggregate template (documentation only)

This is a schema sketch for the future #187 runner/report integration, not a
file to populate in this change.  It is deliberately aggregate-only: raw audio
hashes, per-utterance IDs, private paths, raw reports, and diagnostics belong
to the private run artifact and are not public fields.  Public fields are
limited to counts/metrics and public product, build, and model IDs.

```json
{
  "schema_version": 1,
  "status": "UNMEASURED",
  "product": "vibe",
  "lane": "same_model",
  "build_id": "<public-build-id>",
  "model_id": "<public-model-id>",
  "machine": {"class": "<public-hardware-class>", "os": "<frozen>"},
  "configurations": [{"language": "en", "duration_bucket": "short"}],
  "availability": [{"language": "en", "duration_bucket": "short", "status": "inconclusive", "reason_code": "measurement_inconclusive"}],
  "timing": {"cold": null, "warm": null, "endpoint": "not_run"},
  "quality": {"first_warm": null, "paired_interval": null},
  "failure_reason_counts": {},
  "coverage": {"planned": 0, "completed": 0, "unsupported": 0, "failed": 0, "blocked": 0, "inconclusive": 0}
}
```

The public aggregate must not contain transcript text, private audio paths,
raw logs, arbitrary subprocess errors, per-utterance IDs, or per-utterance
hashes.  A private local artifact may retain the identity hashes, source
utterance IDs, and bounded diagnostics needed for audit under the owner's
existing controls, but it is not committed or uploaded by this protocol.

## Acceptance checklist

Before any recommendation is drafted, the report must show:

- a validated #187 manifest and held-out human coverage for all three
  languages, or an explicit corpus blocker;
- frozen source, binary, model, machine, and configuration identity for every
  completed row;
- separate same-model and product-default lanes, with unsupported cells marked
  `N/A` and reasons retained;
- cold versus first-warm versus subsequent-warm labels, paired uncertainty,
  failure ledger, and no best-of selection;
- coverage counts for every planned row in the canonical
  `completed`/`unsupported`/`failed`/`blocked`/`inconclusive` mapping, with
  `MAX_EXCLUSION=0` enforced for any required-row superiority claim;
- file/CLI and live/desktop endpoints separated, including preview versus final
  visibility and any physical-permission blocker;
- reproducible commands or UI procedure for every completed surface, plus the
  captured help/API evidence for third-party products;
- machine-readable aggregate output with no private transcript/path leakage;
- threshold decisions only after the protocol, coverage, reproducibility, and
  failure gates pass.  Otherwise the conclusion is `INCONCLUSIVE`.

This protocol does not close issue #196.  It freezes what must be measured and
what evidence is required before a later, separately reviewed report can make
comparative claims.

## Primary evidence checked

- [Handy repository metadata](https://api.github.com/repos/cjpais/Handy), [FluidVoice repository metadata](https://api.github.com/repos/altic-dev/FluidVoice), and [Vibe repository metadata](https://api.github.com/repos/thewh1teagle/vibe) were read on 2026-09-06 to confirm repository identity/default branch/license metadata.  Floating metadata is not used as a benchmark identity.
- The pinned product README and LICENSE links in the matrix are the source of the capability and license statements.  The README commit dates were 2026-08-30 (Handy), 2026-08-26 (FluidVoice), and 2026-09-05 (Vibe); the date is recorded to make later drift visible.
- The Sagascript matrix row is pinned to main merge `4ea7f9168bb996b634e2e70780a9fb831f92700f` (2026-09-06); its Cargo feature defaults, CLI build commands, `Settings::default`, and language-specific model recommendations were read from the source at that revision.
- [Issue #196](https://github.com/Magnus-Gille/sagascript/issues/196) is the acceptance source for the requested products, languages, corpus scale, hardware/desktop endpoints, and reuse of the #187/#163/#157 measurement surfaces.
- [Sagascript latency measurement contract](../latency-measurement.md) is the local source document for timing terminology and report privacy.  The #187 normalization/runner contract is carried by the future shared evaluator integration rather than duplicated here.  Local source documents are referenced, not modified, by this leaf.
