# Meeting review CLI usage

Status: unreleased implementation; manual acceptance is pending.

This guide describes the current CLI/core surface for meeting review. It is a
read-only transformation pipeline: inputs are validated, a new value is
emitted on standard output, and the CLI does not write the input files, open a
model, make a network request, or save a review implicitly.

## Commands

The implemented command group is:

```text
sagascript meeting review new INPUT
sagascript meeting review inspect INPUT
sagascript meeting review apply --corrections CORRECTIONS INPUT
sagascript meeting review undo --expected-revision REVISION INPUT
sagascript meeting review reset --expected-revision REVISION INPUT
sagascript meeting review export --format FORMAT INPUT
sagascript meeting review audio-info --audio AUDIO INPUT
sagascript meeting review audio-range [--start START] --audio AUDIO --length LENGTH INPUT
```

Run `sagascript meeting review --help` or add `--help` to a subcommand for
the current descriptions. `FORMAT` is one of `plain`, `markdown`, `json`,
`srt`, or `vtt`.

`new` creates a review envelope from a validated machine transcript.
`inspect` validates and re-emits a saved review envelope. `apply` validates
one correction file and emits a new review with one correction batch.
`undo` removes the latest batch, while `reset` removes all batches and
restores the immutable machine transcript. Both require the review's current
revision. `export` materializes the review in the selected presentation
format; JSON export preserves the complete review envelope and provenance.

Every text/JSON command emits one result to stdout. The CLI has no
`--output` option. The caller chooses whether and where to persist it, and
should redirect to a new destination:

```sh
sagascript meeting review new transcript.json > review.json
sagascript meeting review apply review.json \
  --corrections corrections.json > review-next.json
sagascript meeting review export review-next.json \
  --format markdown > reviewed.md
```

Shell redirection is the caller's responsibility; choose a new filename and
handle any overwrite policy in the calling workflow. GUI saves use the
no-overwrite publication policy and an explicit new-file save. A cancelled or
failed save leaves the working review unchanged.

## Correction file

`apply` accepts a version-1 JSON correction file. The three revision/source
bindings must be copied from the review and its immutable original; each is a
64-character lowercase SHA-256 string. `expected_revision` must equal the
review's current `revision` at apply time.

```json
{
  "schema_version": 1,
  "source_sha256": "<64 lowercase hex from review.original.source_sha256>",
  "original_revision": "<64 lowercase hex from review.original_revision>",
  "expected_revision": "<64 lowercase hex from review.revision>",
  "operations": [
    {
      "kind": "edit_segment",
      "segment_id": "seg-000001",
      "text": "Edited text",
      "speaker_id": "speaker-1"
    },
    {
      "kind": "rename_speaker",
      "speaker_id": "speaker-1",
      "label": "Chair"
    },
    {
      "kind": "merge_speakers",
      "from_id": "speaker-2",
      "into_id": "speaker-1"
    }
  ]
}
```

`operations` is nonempty and ordered. The exact tagged operation fields are:

- `edit_segment`: `segment_id`, plus at least one of `text` or `speaker_id`.
  An empty `text` is an intentional edit.
- `rename_speaker`: `speaker_id` and `label`.
- `merge_speakers`: `from_id` and `into_id`; the IDs must differ.

IDs must exist in the immutable original/replayed transcript, and unknown
fields, invalid IDs, mismatched source/original/current revisions, excessive
operation counts, and oversized input are rejected. The review has a
cumulative ceiling of 1024 correction operations across all retained batches;
each `apply` adds to that total, while `undo` removes the latest batch and
frees its operations. One correction file is one atomic undo batch. Failed
validation emits an error and does not write a replacement file.

## Review envelope and limitations

Review JSON contains the immutable `original` transcript,
`original_revision`, `generation`, ordered `batches`, and current `revision`.
The original and every derived revision are validated and hash-bound; the
review does not store mutable text without its correction provenance.

`undo` removes only the latest correction batch. `reset` clears all batches
and restores the immutable original machine transcript, including its speaker
labels and assignments; the original remains retained in the review and is
not lost. Both advance `generation` and therefore produce a new revision;
undo cannot be used when there is no batch. The current schema has no
wall-clock timestamp or author field, and undo does not preserve the removed
batch in the resulting review.

Segment IDs are scoped to the immutable original revision. This CLI does not
automatically map corrections onto a reprocessed transcript by ordinal ID or
approximate timestamp. Reprocessing/migration requires a separate validated
workflow. Opening a saved review does not automatically open or copy its
audio.

## Explicit audio reads

Audio is selected separately with `--audio` and must hash to the review's
`original.source_sha256`. The commands never auto-play, decode for playback,
create a hidden copy, or start from ordinary transcript text selection.

```sh
sagascript meeting review audio-info review-next.json \
  --audio recording.wav > audio-info.json
sagascript meeting review audio-range review-next.json \
  --audio recording.wav --start 0 --length 65536 > audio-range.bin
```

`audio-info` emits JSON with `length` (file length in bytes) and `mime`.
`audio-range` emits raw container bytes to stdout, with `--start` defaulting
to byte offset `0`; `--length` is required and is bounded to 8 MiB. These
coordinates are byte offsets, not seconds, samples, or decoded audio. The
caller chooses how to persist or hand the bytes to a player/decoder.

The current source-bound reader recognizes common WAV, FLAC, Ogg, MP4, and
MPEG container signatures. Native codec/decoder support varies by the
consumer; this CLI does not promise playback support merely because a
container signature is accepted. Missing, moved, changed, mismatched, empty,
oversized, or unsupported audio is rejected while transcript review/export
remains a separate operation.

### GUI audio acceptance

The GUI attachment opens an explicit file picker offering WAV, FLAC, Ogg, Opus,
M4A, MP4, and MP3; MOV/WebM are not offered by this picker. The selected file
must also match the reviewed transcript's original source SHA-256 before a
playback token is issued. A recognized common container is not a codec or
playback guarantee. The media protocol deliberately rejects an oversized
no-Range `GET` (over 8 MiB); bounded range requests are required for such
files. Chromium HTTP and native macOS WK synthetic probes passed, but actual
Windows Tauri/WebView2 playback and seeking remain a mandatory manual
acceptance gate before release.
