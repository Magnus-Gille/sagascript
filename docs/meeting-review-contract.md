# Meeting review contract (R6 / #193)

Implementation contract; not a claim of released capability.

## Immutable source and explicit persistence

Keep the existing schema-1 `MeetingTranscript` unchanged. A separate schema-1
`MeetingReview` envelope stores the immutable original transcript, its SHA-256
revision, a monotonically increasing generation, ordered correction batches,
and a SHA-256 review revision. Hash deterministic serialized validated values,
not display text. Segment IDs identify segments only within that original
revision; ordinal IDs must never be used alone to match reprocessed transcripts.

All core operations are pure: validate the entire request, create a new value,
and return it only on success. No core operation writes files, accesses models,
updates a dictionary, or makes network requests. Existing schema-1 transcript
imports and exports remain compatible.

## Correction file version 1

A correction request includes `schema_version: 1`, `source_sha256`,
`original_revision`, `expected_revision`, and a nonempty ordered `operations`
array. Supported tagged operations are `edit_segment` (segment ID and optional
text and/or speaker ID, at least one required), `rename_speaker`, and
`merge_speakers`. Reject unknown fields, unsupported versions, invalid IDs,
wrong source/original/current revisions and excessive counts/bytes. Empty text
is a deliberate correction, not an absent field. Validate the resulting
transcript using its existing rules. One request is one atomic undo batch.

Undo removes the latest correction batch. Reset removes all correction batches
and restores machine output, including original speaker labels/assignments.
Both require the expected current revision and advance generation; never reuse
an earlier revision hash (ABA protection). Reject undo with no prior batch.
Retained batches provide operation provenance; undo intentionally removes the
undone batch, not the immutable original. No wall-clock timestamp is required.

The review revision binds schema, immutable original revision, generation and
ordered batches. Deserialization must validate hashes, limits and replayed
results, not trust supplied derived values. Enforce a bounded operation count
and serialized size to keep replay and document loading bounded.
Generation must not exceed JavaScript's exact integer range
(`9_007_199_254_740_991`) because the same envelope crosses the desktop IPC boundary.

## Presentation and export

Materialize reviewed text/speakers by replaying valid batches over the original.
Plain text, Markdown, SRT and VTT use that materialized transcript. Review JSON
preserves original, revisions and correction provenance. UI edits use explicit
Apply/Discard, undo/reset are explicit, and file persistence requires a separate
Save review/export action. A cancelled or failed save leaves the working review
unchanged. Save uses a new filename and the existing no-overwrite publication
policy. Opening a saved review does not automatically open or copy audio.

Audio is explicitly selected and verified against the original source hash.
Playback never starts from ordinary text selection/editing. Follow playback is
user-controlled; manual scrolling disables following. Missing/moved audio must
leave transcript correction and export available. No new transcript library,
hidden audio copies, or automatic glossary writes are introduced.

## Reprocessing boundary (#194)

Never match a correction onto a new machine revision by ordinal segment ID or
approximate timestamp alone. Preserve the last successful review until a whole
replacement revision succeeds. A prior correction must be applied exactly once
to a validated target or retained as an explicit unresolved conflict. Detailed
selective-reprocessing mapping/dependency contract follows after #193 is stable.
