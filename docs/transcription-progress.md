# Testing file-transcription progress

`sagascript transcribe FILE --progress-json --json` emits progress JSON lines
on stderr even when redirected, alongside existing human-readable diagnostics.
Stdout remains the transcript JSON. Select stderr objects whose `event` is
`transcription_progress`:

```json
{"event":"transcription_progress","phase":"resampling","percent":25,"elapsed_ms":15000}
```

`elapsed_ms` is elapsed time for the current input file. `percent` is local to
the named stage, **not overall completion or a time estimate**. It is null for
stages without measured progress. Each input in a batch starts its own timeline.

`step` (1–3) and `steps_total` (3) expose the GUI's three main steps:
1 = Read audio, 2 = Prepare audio & model, 3 = Transcribe. Preparation includes
resampling through encoding; resampling reaching 100% does not finish step 2.
The `completed` event marks successful completion, not merely native progress
reaching 100%. The GUI leaves failed/cancelled steps unchecked and preserves
green checks only for steps already completed. This grouping is for plain
transcription, not the meeting workflow.

Meeting transcription uses the same `transcription_progress` event name with
phase boundaries and elapsed time. `sagascript transcribe FILE --diarize
--progress-json` emits `analyzing`, `clustering`, `finalizing`, and a
successful `completed` event after the existing decode/model events. The
`--meeting-json` output path emits the same terminal events before writing its
shared transcript JSON. These four boundary events have `percent: null`,
`step: null`, and `steps_total: null`; they do not borrow the plain three-step grouping or invent
an overall percentage. Their phase order is:

1. `analyzing`: concurrent Whisper timestamping and speaker segmentation/
   embedding analysis. This is a boundary, not a heartbeat.
2. `clustering`: speaker clustering and label assignment.
3. `finalizing`: merge, diagnostics, source verification, and output assembly.
4. `completed`: emitted only after the requested output has been assembled
   successfully; failures do not emit it.

`elapsed_ms` is wall-clock time for the current input file. It is diagnostic
feedback and is not a completion estimate. Earlier decode/model events retain
their existing stage-local percentages and plain stage fields; those fields
do not describe overall meeting completion.

The desktop meeting view shows the current phase, total elapsed time, time in
that phase, and an indeterminate status bar. Native monotonic job durations
anchor the display, which ticks between status polls and freezes when the
worker exits. Repeated checkpoints in the same phase do not reset its timer.
The last successful status check is shown separately: it confirms the app
answered, not that native analysis made progress. Failed or delayed responses
pause the bar and explain that processing may still be running. Cancellation
keeps timing until the worker actually stops. Elapsed time never determines
a completion percentage or ETA.

- `decoding`: packet bytes / source bytes; container overhead can leave a
  remainder until packet decoding ends. This is an approximate byte fraction,
  especially for video containers with other tracks.
- `resampling`: separate input-chunk progress for conversion to 16 kHz mono.
  Starts at zero before mono mixing, ends at 100 after conversion.
- `loading`, `language_detection`, `preparing`, `encoding`: named boundaries,
  without invented percentages. A boundary is not a heartbeat from native code.
- `transcribing`: native Whisper percentages.
- `finalizing`, `completed`: postprocessing and successful completion.

The detailed terminal-independent timeline covers both plain transcription and
the meeting phase boundaries described above. GUI plain imports use the same
core decode/conversion callbacks; they do not perform the CLI's separate
language diagnostics.

GUI plain imports have run-ID-scoped `plain-transcription-progress` events.
They lease a warm backend separate from live dictation/training, so Stop cannot
abort those jobs. One plain import runs at a time; the slot remains reserved
until the actual worker exits, including after cancellation or inference timeout.
This retains an additional warm model runtime for file imports. Decode has no
compressed-file-size timeout. Cancellation is checked between decoder packets
and resampler chunks and after model loading; native model loading itself is
not interruptible. The UI remains busy while it stops rather than claiming the
worker has exited early.

Re-run retains the file's profile/context/diarization choice; language/model/
decoder settings are read from current Settings. It is not an exact replay of
all historical engine parameters. No audio or transcript result is kept in the
five-entry in-memory re-run history.

## Regression and acceptance

Run the regression test that forces packet decoding to reach 99% and then
requires many independent conversion updates between 20% and 80%:

```sh
cargo test -p sagascript-core --lib conversion_progress_is_not_suppressed
```

Run terminal-independent acceptance on an explicitly selected >16 kHz fixture:

```sh
python3 scripts/check-transcribe-progress.py PATH_TO_CLI PATH_TO_FIXTURE
```

It captures both streams without a PTY, validates stdout JSON, asserts phase
ordering and independently increasing decode/conversion percentages, and prints
timings without transcript content. Requires an already-provisioned Base EN
model. It does not download models.

2026-09-13 local acceptance on the existing synthetic 21-minute M4A fixture:

| Stage | First event (ms) | Last event (ms) | Events |
|---|---:|---:|---:|
| Decoding | 0 | 12406 | 100 |
| Resampling | 12406 | 95403 | 101 |
| Loading | 95403 | 95403 | 1 |
| Encoding | 101274 | 101274 | 1 |
| Transcribing | 102427 | 117761 | 44 |
| Completed | 117972 | 117972 | 1 |

Browser component verification used the actual Settings component with mocked
Tauri transport: decode 99% followed by conversion 0%, 25%, 50%, 99% changed
both visible text and bar width. This tests rendering and listener wiring,
not native WebKit IPC delivery. No native app replacement was part of that test.

## Correction to earlier verification claims

Previously the packet decoder reached 99%, and conversion progress was mapped
to 90–100 using the same last-reported value. This suppressed virtually all
conversion updates. A sorted list of percentages ending at 100 did not prove
continuous progress. Earlier Ctrl-C experiments also did not establish clean
cancellation: the PTY child remained alive. These are not acceptance evidence.
