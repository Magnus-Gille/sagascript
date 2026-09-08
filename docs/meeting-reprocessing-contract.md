# Selective meeting reprocessing — implementation contract

Status: in progress for #194; not a delivered capability.

## Publication and preservation

The active version-1 `MeetingReview` remains unchanged while inference and
correction migration run. A separate, saveable reprocessing proposal contains
the entire previous review and the new immutable machine transcript. It is not
an active review and must never be exported as one while conflicts remain.

Each previous correction has exactly one ordered disposition. Automatic
translation uses exact, unique original segment signatures (numeric start/end
and text), not ordinal IDs or fuzzy timestamps. Automatic speaker translation
requires identical complete segment membership under that mapping. Changed
source identity disables all automatic correction translation.

Translated operations replay in original order. Stop replay at the first
unresolved or invalid operation; retain it and every following operation in
the proposal. A later correction must not silently run without its predecessor.
The UI shows the original operation/text and proposed transcript when requesting
explicit targets. No operation is dropped to manufacture a successful result.

An explicit resolution maps one old correction to one or more validated new
operations. Empty resolutions are forbidden. This supports user-directed
one-to-many text corrections without guessing. The old operation remains in
the proposal for comparison. Replacing an earlier resolution recomputes the
entire candidate, never appends the correction a second time.

Proposal revision checks protect resolution updates; the previous review's
revision is checked again before final acceptance. Concurrent editing wins over
stale reprocessing: report a conflict and leave the active review untouched.
Only a fully resolved, validated proposal can yield a new version-1 review.
Its correction batches retain the resulting operations and ordinary review
undo/reset semantics. The proposal can be retained separately as migration
provenance; it does not introduce recursively nested review ancestry.

## Work selection

- Recluster: reuse validated analysis and timed transcription. No Whisper,
  embedding inference, or audio decoding.
- Re-diarize: reuse validated timed transcription; decode audio and rerun
  segmentation/embedding analysis. No Whisper or language-detection inference.
- Full: explicitly selected transcription and diarization. Show the required
  work before execution; never silently fall back from a selective cache miss.

Existing #157 source/model/language/prompt/schema validation is a floor, not
permission to weaken validation. A plan must bind its input, cache and review
revisions and revalidate them before execution/publication. Analysis-specific
dependencies must be tracked separately from timed transcription dependencies.

The existing cache advances to schema 4, recording the pinned segmentation and
embedding SHA-256 identities plus `min_segment` and `min_gap` separately from
the existing source/model/language/prompt identity. Threshold is deliberately
excluded. Legacy schema-3 files are explicit misses, not assumed to have known
analysis provenance. Re-diarization may reuse compatible timed transcription
when well-formed analysis metadata differs, but must still validate the entire
cached payload. Reclustering additionally requires current analysis identity.

## Storage and ownership

Reusable artifacts are opt-in at an explicit path. They contain transcript and
voice-derived data and require privacy/deletion documentation. Failed saves
must not replace the old reviewed file. Proposal persistence and final review
publication must use private, atomic create-new output, not remove-then-rename.

This implementation session uses synthetic fixtures and isolated UI checks.
The separate release-testing agent owns installed apps, real recordings,
model execution and representative 15–30 minute timing/acceptance measurements.
