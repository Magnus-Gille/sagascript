# 0008. Publishing new files without replacing, and related outputs

- Status: Accepted (2026-10-14)
- Related: [#342](https://github.com/Magnus-Gille/sagascript/issues/342),
  [#373](https://github.com/Magnus-Gille/sagascript/issues/373),
  [#374](https://github.com/Magnus-Gille/sagascript/issues/374),
  [#368](https://github.com/Magnus-Gille/sagascript/issues/368) (model downloads),
  [ADR 0002](0002-privacy-first.md)

## Context

Exports, reprocessing proposals and reusable analysis caches are user-named new files that must
never replace an existing one. Three copies of "temp file, fsync, hard link, remove temp" existed,
each with its own answer to what is on disk when a step fails: `write_atomic_new`
(`meeting_proposal.rs`), `save_new` (`diarization_cache.rs`) and `write_new_export`
(`meeting_jobs.rs`). They differed in what a failed temp cleanup after a successful publication
means, and none said what happens between two related publications.

`meeting reprocess execute` publishes two files, the new cache and the proposal. The cache was
written at its final name in the middle of inference, and the proposal was written last, after
the review revision was re-read. A review edited during inference, a full disk or a proposal path
that appeared meanwhile left a finished cache without a proposal, and the retry with the same
output names was refused by the existence check (#342).

## Decision

1. **One primitive.** `sagascript_core::fsutil::publish_new(path, bytes)` writes the bytes to a
   private staging file beside the destination (`.<name>.<uuid>.tmp`, `0600` on Unix), fsyncs it,
   hard-links it to the destination and removes the staging name. A hard link is the no-replace
   commit point because it fails when the destination exists, which `rename` does not. Of several
   publishers exactly one wins; a reader sees no file or the complete file. Where hard links are
   unavailable (FAT, exFAT, most network shares) the publication fails cleanly. Like
   `download.rs` (#368) the data is fsynced before it gets its final name; the directory is
   fsynced afterwards on Unix, best effort.
2. **Defined outcomes.** `Ok`: the destination holds exactly the bytes. `AlreadyExists`: it was
   not touched. Any other error: nothing was created, so a retry is safe. A staging file that
   cannot be removed is reported (`leftover`) and never silently ignored; after a success it is a
   warning, because the artifact is complete. A process killed mid-publication leaves at most
   a staging file, found by `fsutil::staging_leftovers`.
3. **Related outputs: stage all, validate, then publish in a documented order.**
   `StagedFile` stages an artifact (or reserves a staging path for a pipeline that writes a large
   one) and removes it when dropped, so every early return publishes nothing. A caller serializes
   and stages every output, runs its last checks, and only then publishes, one output after the
   other. The last output is the completion marker. No cross-file atomicity is promised.
4. **Reprocessing.** `execute` stages the cache privately during inference and the proposal after
   it, re-reads the review, then publishes the cache and then the proposal
   (`meeting_reprocessing.rs`). Every failure before the first publication leaves no output and
   the same command can run again. Only the failure of the proposal publication leaves a complete
   cache without a proposal, never the reverse; a `recluster` plan from that cache recovers the
   proposal without inference. A later `execute` with the same paths detects that state and says
   so instead of reporting a plain "exists". `docs/meeting-reprocessing.md` ("Outputs, failure and
   retry") is the user-facing contract.
5. **No deletion of user files.** Only staging names this code created (a UUID in the name) are
   ever removed.

## Consequences

- One place, `fsutil.rs`, answers "what is on disk if this fails"; its tests inject failures into
  the link and the unlink (`PublishFs`) and race publishers.
- The cache is no longer visible at its destination during the long inference; the stderr line
  `Saved reusable diarization cache: PATH` now appears when it is published, after the checks.
- `fsutil::publish_new` is the call for any new "never replace" output. `download.rs`, the
  settings store and `diarization_cache::save` replace by design and keep their own paths
  (#368 owns the downloads).
- Known gap: `benchmark_quality::write_private_new` creates its report at the final name with
  `create_new` and can leave a partial file after a failed write; it is outside #342.
- A killed run can leave `.<name>.<uuid>.tmp` files. They hold private content, are never read,
  and the CLI warns about them on the next `execute` with the same output path.
