# Local meeting import and review

In **Transcribe**, select an explicit language profile when its personal
dictionary should apply, enable **Speaker diarization**, and open or drop one
recording. A file-only prompt adds temporary decoder context. Language, model,
and dictionary are frozen together when the import starts; editing settings
afterward does not reinterpret that job.

The review shows speaker groups and timestamps. Rename a speaker or merge one
speaker into another, then explicitly export plain text, Markdown, JSON, SRT,
or WebVTT. Speaker operations use the same validated core implementation as the
CLI. Export opens a native Save dialog and requires a **new filename**: existing
recordings and documents are never overwritten. A cancelled Save dialog writes
nothing. Export errors leave the review available for retry.

## Local data and cancellation

- Opening a recording does not capture the microphone, paste text, submit a
  message, or upload audio. Required models must already be available; missing
  models produce an actionable download instruction rather than a silent
  download during import.
- This MVP retains one current job and the current review in app memory. It
  does not create a recording library or automatically persist transcripts,
  speaker names, dictionaries, or a diarization cache. Restarting the app loses
  an unexported review. Preserve wanted results with an explicit export.
- Failed or cancelled imports leave the previous valid review visible. The
  original recording is read-only. A source SHA-256 binds the document to its
  bytes; the file is checked again before a result is accepted.
- Each import owns a separate backend and a generated job ID. Only the matching
  ID can cancel it; the live-capture lease remains held until its native worker
  and scoped children have actually stopped. **Cancelling** is not reported as
  completed cancellation while an ONNX/codec/resampler call is still running.
- Decode and processing boundaries check cancellation. Native Whisper receives
  abort requests on the job's own backend. Native operations without an abort
  interface must finish their current call. After 30 minutes the app requests
  cancellation but still waits for actual worker termination; it never detaches
  a busy worker and claims success.
- Progress is stage-based, not a fabricated percentage. If status retrieval
  fails, use **Retry status check**; this resumes observation of the same job
  and does not start another transcription.

## CLI equivalent

Discover current arguments with `sagascript --help` and
`sagascript transcribe --help`. The meeting producer requires the `diarization`
build feature and exactly one regular input file; directories, recursive mode,
and multiple inputs are rejected before inference. Hashing is streaming and
limited to 2 GiB; decoded audio and transcript duration retain the four-hour
ceiling. Legacy `--json` and `--jsonl` contracts are unchanged.

```sh
sagascript transcribe meeting.m4a --diarize --meeting-json --language en --model base.en
sagascript meeting inspect meeting.json
sagascript meeting rename meeting.json --speaker SPEAKER_0 --label Chair
sagascript meeting merge meeting.json --from SPEAKER_1 --into SPEAKER_0
sagascript meeting export meeting.json --format vtt
```

The CLI commands read their input and write the new document/export to stdout;
they never modify the input file. If redirecting stdout, use a different,
previously nonexistent destination. The GUI exporter enforces this itself;
shell redirection is controlled by the caller. Existing CLI `--diarize-cache`
remains the explicit cache-reuse path; this GUI MVP does not silently enable it.

## Document contract

Schema version `1` carries source SHA-256, language/model identifiers, duration,
stable segment IDs, text/time bounds, speaker IDs, and display labels. Validation
rejects unsupported versions, duplicate IDs, unknown speakers, non-finite or
out-of-range times, invalid labels, and excessive counts/text. New segments are
ordered by start/end and receive stable ordinal IDs. Rename/merge returns a new
validated document without changing segment IDs, text, timing, or source hash.

JSON exports can be reopened using `meeting inspect` and the other CLI document
operations. The GUI opens audio/video recordings, not saved JSON documents;
reimporting audio is a new analysis. Audio playback, text corrections, undo,
revision-aware reprocessing, and automatic speaker naming are later roadmap
work, not capabilities claimed by this MVP.

## Acceptance checklist

Automated contract, cancellation, export/no-overwrite, CLI and frontend tests
are necessary but do not prove installed-app behavior. Test a signed candidate
on each target platform with a real 15–30-minute meeting: import with the chosen
profile, inspect timestamps/speaker boundaries, rename, merge, export all five
formats, cancel during different stages, and retry a failed import. Verify that
the previous review and the recording survive failure/cancellation, then check
normal dictation still works. A public English AMI fixture is supplementary
functional/performance evidence, not spontaneous Swedish or native Voice Memo
quality acceptance. Record the candidate's displayed version and build ID.
