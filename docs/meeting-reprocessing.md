# Meeting reprocessing

This is the current #194 workflow for producing a new machine transcript while
keeping the active review unchanged until a user accepts the result. It covers
the GUI and the CLI surfaces implemented in
[`MeetingReprocessing.svelte`](../src/lib/MeetingReprocessing.svelte), the
Tauri commands in [`meeting_reprocessing_commands.rs`](../src-tauri/src/meeting_reprocessing_commands.rs),
and the CLI adapters in
[`meeting_reprocessing_cli.rs`](../src-tauri/crates/sagascript-cli/src/meeting_reprocessing_cli.rs)
and [`meeting_proposal.rs`](../src-tauri/crates/sagascript-cli/src/meeting_proposal.rs).
The lower-level invariants are in the
[implementation contract](meeting-reprocessing-contract.md).

## GUI: plan, execute, review, accept

1. Start a meeting import or open a saved meeting review from the Transcribe
   view. Once a meeting review and transcript are present, the **Meeting
   reprocessing** panel appears.
2. Choose a mode and a speaker threshold in the inclusive range `0..2`. The
   next plan will use these controls. If a new cache is wanted, only **Full recomputation**
   offers the opt-in **Save a new analysis cache** option. The save dialog
   chooses the exact new local path. Click **Plan reprocessing** and choose the
   original local recording in the picker. For selective modes, choose the
   reusable meeting cache as well. Planning validates the source, previous-review
   revision, cache identity, and threshold without decoding audio or loading a
   model. The selected plan then displays its immutable revision and the exact
   work marked **Run** or **Skip**.
3. Click **Execute selected … plan**. Execution is tied to the displayed plan
   revision and publishes a separate migration proposal. The current review is
   not replaced. The result shows the required work and measured phase timings.
4. Review every correction migration. Automatic mappings are read-only. The
   first conflict shows the old operation, original segment text/times, and
   candidate targets. Later conflicts remain visibly blocked until the earlier
   one is resolved. Find candidate segments by text, ID, or timestamp; filtering
   keeps already selected targets visible. Segment edits can explicitly select more than one target;
   speaker rename and merge conflicts require selected candidate speakers.
   Empty or invalid resolutions cannot be applied.
5. **Accept proposal** is enabled when every correction has an applied
   disposition, automatically or explicitly (including when there are no corrections).
   Acceptance checks the active review revision again, so
   a concurrent edit leaves the active review untouched. **Save proposal** can
   preserve an unresolved proposal separately; cancelling its save picker is
   not reported as a successful save. Existing unsaved review edits must be
   applied or discarded before planning, execution, conflict resolution, or
   acceptance.

Editing, undoing, or resetting the active review makes an older plan or proposal
stale. Its execution/acceptance controls are disabled; saved provenance remains
exportable. Save a proposal before acceptance if its original history is needed:
acceptance replaces the active review, while discarding a proposal only removes
that separate draft. Both actions require confirmation when appropriate.
If previewing a completed job fails, its proposal is retained for retry or export.

## Work selected by each mode

The plan's `required_work` is authoritative; selective cache misses are errors,
not permission to silently run a full job.

| Mode | Reuses/skips | Runs |
| --- | --- | --- |
| `recluster` | Reuses validated timed transcription and analysis; skips audio decoding, Whisper transcription, language detection, segmentation, and embeddings. | Speaker clustering. |
| `rediarize` | Reuses validated timed transcription; skips Whisper transcription and language detection. | Audio decoding, segmentation, speaker embeddings, and clustering. |
| `full` | Does not read an input cache. | Audio decoding, transcription, language detection, segmentation, speaker embeddings, and clustering. |

These flags are derived in
[`meeting_reprocess_plan.rs`](../src-tauri/crates/sagascript-core/src/meeting_reprocess_plan.rs)
and enforced by
[`meeting_reprocessing.rs`](../src-tauri/crates/sagascript-cli/src/meeting_reprocessing.rs).

## CLI: explicit files and revisions

The top-level command is `sagascript meeting reprocess`. Replace the uppercase
placeholders below with existing local input paths and new output paths. An
existing output path is rejected; commands do not overwrite the selected
review, plan, proposal, or cache.

Reprocessing requires the `diarization` feature, enabled in default CLI builds.
The lean `--no-default-features` CLI omits `meeting reprocess`; document-only
`meeting proposal` commands remain available without model inference.

### Plan

Selective plan:

```text
sagascript meeting reprocess plan AUDIO \
  --previous-review REVIEW \
  --mode recluster \
  --threshold 0.75 \
  --cache CACHE \
  --language en \
  --output PLAN
```

Use `--mode rediarize` for the other selective mode. For full recomputation,
omit `--cache` and use `--mode full`. `--language` can be replaced by the
existing `--profile`; `--model`, `--prompt`, and `--prompt-file` are also
available as declared by the CLI. The plan JSON contains the revision required
by the execute step.

### Execute

Selective execution consumes exactly one saved plan and its matching cache:

```text
sagascript meeting reprocess execute PLAN AUDIO \
  --previous-review REVIEW \
  --expected-revision PLAN_REVISION \
  --cache CACHE \
  --language en \
  --output PROPOSAL
```

Full execution omits `--cache`. It may opt in to a newly written reusable cache
at an explicitly chosen path:

```text
sagascript meeting reprocess execute PLAN AUDIO \
  --previous-review REVIEW \
  --expected-revision PLAN_REVISION \
  --language en \
  --cache-output CACHE_OUT \
  --output PROPOSAL
```

`--cache-output` is only valid for full recomputation. It is a new artifact,
must be different from `--output`, and is protected against replacing an
existing path. A full run never reads an input cache. Execution also checks
that the source, cache (when applicable), and previous-review revision remain
unchanged before publishing the proposal. Repeat the same runtime selectors
used for planning (`--language` or `--profile`, plus any `--model`,
`--prompt`, or `--prompt-file`) when executing; changing them makes the saved
plan's transcription context stale and execution fails closed.

### Inspect, resolve, and accept the proposal

Execution produces a proposal, not an active review. Inspect or preview it:

```text
sagascript meeting proposal inspect PROPOSAL
sagascript meeting proposal preview PROPOSAL
```

Resolve one correction with a JSON file containing the validated
`CorrectionOperation[]` shape used by
[`meeting_review.rs`](../src-tauri/crates/sagascript-core/src/meeting_review.rs):

```text
sagascript meeting proposal resolve PROPOSAL \
  --expected-revision PROPOSAL_REVISION \
  --index 0 \
  --operations OPERATIONS_JSON \
  --output RESOLVED_PROPOSAL
```

Repeat with the next proposal revision as needed. Acceptance explicitly names
the current active review and writes a new review file:

```text
sagascript meeting proposal accept RESOLVED_PROPOSAL \
  --current-review REVIEW \
  --output NEW_REVIEW
```

The lower-level `sagascript meeting proposal new PREVIOUS --proposed
TRANSCRIPT --output PROPOSAL` command is available when a new validated machine
transcript already exists. It follows the same preview, explicit resolution,
and revision-checked acceptance rules.

## Artifact privacy and cache compatibility

Saved proposals contain both the previous review and the new machine transcript,
including text, speaker labels, and correction history. Save them only to a
chosen private location and delete them there when no longer needed. They are
not uploaded or automatically deleted by this workflow.

Reusable caches contain transcript and voice-derived diarization data. They are
opt-in artifacts: the GUI's **Save a new analysis cache** dialog and the CLI's
`--cache-output CACHE_OUT` identify the exact local destination. The application
does not publish them remotely or delete them automatically. Deliberately
delete a cache file from its chosen location when it is no longer needed.

The cache format is schema **4** and records analysis provenance separately
from source/model/language/prompt identity. Schema-3 caches are incompatible:
they are explicit cache misses because their analysis provenance is unknown;
they are not migrated or guessed. Normal `recluster` reuse also requires the
current analysis identity. `rediarize` may reuse a schema-4 cache whose
analysis fingerprint or analysis parameters are stale, but only after the
complete cached payload and metadata pass validation. A miss must be handled by
choosing a compatible cache or explicitly planning a full recomputation.
Unknown analysis-provenance fields cause an explicit cache miss, not reuse: a
future analysis parameter must not accidentally permit reuse by an older binary.
An incompatible artifact therefore needs a compatible application or a regenerated
cache. Ordinary cache-enabled transcription can recompute on a miss; selective
execution never silently falls back. Malformed JSON or corrupt payloads remain
errors rather than exposing their contents in diagnostics.

## Cancellation and timing interpretation

Re-diarization checks model availability before decoding. Cancellation is
cooperative between analysis stages, segmentation windows, and speaker embeddings;
an individual running ONNX inference call cannot be interrupted. The job retains
its worker slot until the worker has actually stopped.

The total timer includes final source/cache integrity checks and orchestration
that are not all assigned to individual phase timers, so their sum can be smaller
than the total. Repeated integrity reads impose an I/O floor even in recluster
mode; realistic speed claims require the long-recording benchmark below.
Correction migration replays operations in order and may perform quadratic work
up to the 1,024-operation limit. After acceptance, undo steps follow the migrated
operations rather than preserving original multi-operation editing batches.

## Acceptance boundary

The workflow and its revision, conflict, cache, and output-path checks are
covered by source-level and synthetic tests. That does not establish model
quality or release acceptance for real recordings. Representative real-audio
processing, cancellation on long jobs, cache sizes, codec variations, and
15–30-minute timing measurements remain manual acceptance work; the separate
[contract](meeting-reprocessing-contract.md) records that boundary.
