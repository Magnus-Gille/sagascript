# System-audio capture (issue #289, epic #299)

Goal: record what the computer plays (the remote side of Teams, Zoom, Meet,
browser calls), optionally together with the microphone, fully locally, and feed
the existing meeting pipeline. This document records the phase-1 design and the
limits that were decided up front. Phase 1 is CLI and core only; the app UI
(source picker, indicator, permission flow) is phase 2.

Status of verification: API names and signatures below were checked against the
macOS SDK headers (`CoreAudio/AudioHardware.h`, `AudioHardwareTapping.h`,
`CATapDescription.h`) and the `windows` 0.61 crate sources. Nothing has been run
live; see "Live testing".

## CLI surface

```
sagascript record --source mic|system|both [--app <bundle-id|pid|exe>] [--output <file>]
sagascript doctor [--json]      # read-only: support + permission state
```

`--source mic` (default) is the unchanged existing behaviour. `--app` is only
valid with `system` or `both`. Without `--output`, `record` transcribes (for
`both`: a mono mix of the aligned tracks). With `--output`, `mic` and `system`
write a mono 16 kHz WAV and `both` writes the two-track file below.
Plain `sagascript transcribe` accepts the two-track file unchanged (decoders
downmix to mono). With `--diarize` / `--meeting-json` the channels are split into
"Me" and the other participants, see "Me versus the others".

## macOS

### Preferred path: Core Audio process taps (macOS 14.2+, supported from 14.4)

- `CATapDescription` (Objective-C, `initStereoGlobalTapButExcludeProcesses:` for
  everything, `initStereoMixdownOfProcesses:` for chosen apps) and
  `AudioHardwareCreateProcessTap(CATapDescription*, AudioObjectID*)` /
  `AudioHardwareDestroyProcessTap`, both `API_AVAILABLE(macos(14.2))`.
- The tap is read through a **private aggregate device**
  (`AudioHardwareCreateAggregateDevice`) whose dictionary lists the tap
  (`taps`, with `drift` compensation) and the default output device as clock
  source (`master` / `subdevices`), with `private`. `tapautostart` is deliberately
  not set: it makes `AudioDeviceStart` block until a tapped process first plays
  audio (AudioHardware.h), which would hang startup, `--duration` and Ctrl+C
  while nothing plays; idle time is silence-filled instead. An
  `AudioDeviceCreateIOProcID` callback receives float32 buffers; the format comes
  from `kAudioTapPropertyFormat` ('tfmt'). Non-interleaved buffer lists are
  interleaved in the callback. The aggregate also contains the physical output
  device, so its input streams (if any) would appear next to the tap's. Capture
  therefore **refuses to start when the default output device
  (`kAudioHardwarePropertyDefaultOutputDevice`, 'dOut', where media plays; not the
  alert-only 'sOut' device) has input channels**, and the callback forwards a
  buffer list only if it has exactly the tap's shape. Refused setups: USB headsets
  and audio interfaces, and Bluetooth headsets that expose a microphone. Working
  setups: built-in speakers and wired headphones on the headphone jack. The device
  is resolved once, so the UID, the clock and the duplex check refer to the same
  device. The refusal is broader than the risk; a documented alternative is
  per-IOProc stream usage (AudioHardware.h, `AudioHardwareIOProcStreamUsage`),
  disabling the physical input streams so only the tap's buffers are delivered. It
  needs hardware validation and is the first item of #303.
- The 14.2+ symbols are resolved with `dlsym`, so the binary still launches on
  older systems; the capability probe checks the OS version, the
  `CATapDescription` class and the symbol.
- **Per-app capture:** `--app` is resolved against
  `kAudioHardwarePropertyProcessObjectList` ('prs#') using
  `kAudioProcessPropertyPID` ('ppid') and `kAudioProcessPropertyBundleID` ('pbid'),
  plus `proc_pidpath` for executable names. A bundle id also matches helper
  processes whose id extends it (`com.google.Chrome` matches
  `com.google.Chrome.helper`), because browsers and Electron apps render audio
  in helpers. The app must be running and have opened an audio device, so join
  the call before starting.
- **Permission:** capturing needs the audio-only "System Audio Recording"
  permission (System Settings > Privacy & Security > Screen & System Audio
  Recording > System Audio Recording Only), introduced in macOS 14.4. The
  Info.plist must carry `NSAudioCaptureUsageDescription` (added to
  `src-tauri/Info.plist`; it is the text in the prompt). No new entitlement is
  needed for a non-sandboxed Developer ID app; the existing
  `com.apple.security.device.audio-input` stays for the microphone.
  Unverified until live testing: whether a denied permission fails
  `AudioHardwareCreateProcessTap` or yields silent buffers; the recorder
  handles both (error, or a "track is silent" warning).
- **Querying without prompting:** there is no public API. `doctor` uses the
  private `TCCAccessPreflight("kTCCServiceAudioCapture")` through `dlopen`/`dlsym`
  (0 granted, 1 denied, 2 not determined) and reports `unknown` if it is missing
  or answers anything else. It never prompts. When run from a terminal the
  answer concerns the responsible process (the terminal app), not Sagascript.app.
  This is a private, best-effort probe, acceptable for a directly distributed
  app; it must not be relied on for flow control.

### macOS 13: explicitly unsupported for system audio

ScreenCaptureKit audio (`SCStreamConfiguration.capturesAudio`, macOS 13) would be
the fallback, but it needs the broad Screen Recording permission, a delegate-based
Objective-C callback stack (a second backend with its own lifecycle), and no
per-app audio without window selection. macOS 13 is two major versions old, the
acceptance criteria target 14.4+, and asking for screen recording to hear a call
is a poor privacy story. Decision: macOS < 14.4 reports `supported: false` with
a message; microphone recording keeps working. Revisit only if users on 13 ask.
Documented limitations: macOS 14.2/14.3 are also reported unsupported because the
permission model before 14.4 was not verified.

## Windows (Windows 10/11, x64 and ARM64)

- **All system audio:** WASAPI loopback. `IMMDeviceEnumerator::GetDefaultAudioEndpoint(eRender, eConsole)`,
  `IAudioClient::Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, ...)`
  with the mix format, read with `IAudioCaptureClient` (polled every 10 ms).
  No permission and no OS prompt.
- **Per-process:** `ActivateAudioInterfaceAsync` on
  `VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK` with
  `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK` and
  `PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE` (the target and its child
  processes). Windows 10 version 2004 (build 19041) or later. This mode has no
  mix format, so a fixed 16-bit PCM stereo 44.1 kHz format is requested with
  `AUDCLNT_STREAMFLAGS_LOOPBACK | EVENTCALLBACK | AUTOCONVERTPCM`, zero buffer
  duration and an event handle, following Microsoft's ApplicationLoopback sample.
  `--app` accepts a pid or an executable name (`Teams.exe`);
  bundle ids are rejected with an explanatory error.
- **Multi-process apps (Teams, Chrome, Edge):** an executable name matches many
  processes. Sagascript picks the process-tree roots (matches whose parent is not
  itself a match) and tries them in ascending pid order until one opens;
  `INCLUDE_TARGET_PROCESS_TREE` then covers the helper processes. If an app runs
  several independent root instances, only the first root that opens is captured;
  pass `--app <pid>` to choose a specific one. This path has not been run on real
  Windows hardware yet.
- Loopback delivers **no packets while nothing is playing**, which would shorten
  the track against the microphone. The recorder fills such gaps with silence
  from the wall clock (see "Timeline").
- Only default render endpoint is captured: if a call is routed to a non-default
  device, switch the default or use per-process capture.

## Linux

`record --source system|both` returns a clear "unsupported" error; `doctor` reports
`supported: false`. (PulseAudio/PipeWire monitor sources are a possible later
backend, not planned.)

## Audio pipeline

1. Backend callbacks hand native-rate interleaved f32 chunks to a channel (the
   callback only copies and sends).
2. A worker thread downmixes to mono and resamples to 16 kHz with a streaming
   sinc resampler (`rubato`, same parameters as the file path), per track.
3. The 16 kHz mono result is appended to a temporary **spool file** (16-bit PCM)
   so long meetings do not grow in RAM while recording; at stop it is read back,
   aligned, and written to the final file.

### Timeline and drift

- Each track starts when its first sample is produced: the mic at its first
  callback, the system track at the moment its capture was requested (idle
  silence is filled). `track_start_padding` prepends silence to the later one.
- Mic and system run on independent clocks. Phase 1 does not resample for drift;
  on the tap path `drift` compensation is enabled in the aggregate device. The
  worst expected skew over 30 minutes (50 ppm) is about 90 ms, below what
  speaker separation needs, and the length difference is printed as `drift`
  on the `Track lengths` line for the owner test. If measured drift is larger, phase 2 adds periodic
  re-timing against the system clock.

### Echo and duplication

With speakers (no headphones) the microphone also hears the remote voices, so the
mic track contains them, delayed. Decisions: keep tracks separate (no echo
cancellation in phase 1), mix only for plain transcription (`both` without
`--output`), and tell users to use headphones for best results. Phase 2 can use
the system track as an echo-cancellation reference or suppress mic segments that
correlate with the system track before diarization.

## Two-track file format

Single stereo WAV, 16 kHz, 16-bit PCM: **left = microphone ("me"), right = system
audio ("the others")**. Rationale over two files plus a manifest: one artefact to
move, play, and pass to `transcribe`/`meeting`; any decoder downmixes it; the
channel convention is documented and marked in the file (below). WAV
is capped at 4 GiB (about 34 hours at this rate), which the writer enforces.

**Marker.** The recorder writes a standard RIFF `LIST`/`INFO` chunk between `fmt `
and `data`, carrying `ISFT` (`Sagascript <version>`) and an `ICMT` comment
`sagascript-two-track/1 left=microphone right=system recorder=<version>`.
`LIST/INFO` is the chunk ffmpeg and most taggers write; players and decoders skip
it, and the existing reader walks chunks, so the file stays playable and readable.
The marker must precede `data` so it is found by reading the first KiB only. Only
two-channel files written by `record --source both` carry it; mono outputs do not.
Files written before this change have no marker: pass `--two-track` once.

**Detection.** `transcribe --diarize` / `--meeting-json` treat a file as two-track
when it carries a valid marker (layout version 1, left = microphone, right =
system). Override: `--two-track` forces the split for a stereo WAV without a marker
(16 kHz 16-bit PCM; anything else is an error), `--no-two-track` forces the old
downmix even for a marked file. Both require `--diarize`, and are mutually
exclusive. A paired on/off flag follows the existing `--vad` / `--no-vad`
convention, keeps the common case (a file from `record`) flag-free, and is clearer
than a `--channels mic,system` mini-language that has only one valid value.
An ordinary stereo file without marker behaves exactly as before.

## Files, cleanup, privacy

- In-progress spool files live in a user-visible directory,
  `<local data dir>/Sagascript/recordings/` (macOS:
  `~/Library/Application Support/Sagascript/recordings/`, Windows:
  `%LOCALAPPDATA%\Sagascript\recordings\`), created mode 0700, files 0600 where
  supported. They are deleted when the recording finishes or fails; leftovers
  from a crash older than 24 hours are removed at the next start. The final
  `--output` file is the user's and never auto-deleted. A settings-driven
  retention policy is part of the phase-2 app work.
- Everything is local; nothing is uploaded.
- **Recording indicator:** the CLI prints a banner to stderr before capture
  starts and the process runs in the foreground until Ctrl+C. macOS also shows its
  own purple recording indicator for system-audio capture. The app UI indicator
  is phase 2; nothing records without it.
- **Consent reminder:** the banner and `record --help` state that recording
  other participants may require their consent (jurisdiction-dependent).

## Decisions

Decided for phase 1 of epic [#299](https://github.com/Magnus-Gille/sagascript/issues/299)
(issue #289):

- **macOS 14.4+ only for system audio.** Core Audio process taps are the only
  non-ScreenCaptureKit route with a narrow permission; macOS 13 and earlier are
  explicitly unsupported rather than approximated.
- **Two-track WAV for `--source both`:** left = microphone, right = system audio,
  16 kHz 16-bit, so later diarization can separate "me" from "others" without
  signal processing. Transcription uses a mono mix.
- **Best-effort private TCC preflight:** `TCCAccessPreflight` (dlopen/dlsym,
  fail-soft) is used by `sagascript doctor` only, because no public API reads
  the audio-capture permission without prompting. Only 0 (granted) and 1
  (denied) are interpreted; every other value is reported as unknown. Capture
  start never calls it and is never gated on it.

## Known limitations (tracked for phase 1.5)

macOS refuses output devices that have microphone inputs (USB headsets and
interfaces, Bluetooth headsets with a microphone); see the macOS section.

`--source both` is limited to 15 minutes: the microphone service buffers in
memory and stops appending at that length, so longer `both` recordings are
rejected up front (`--duration`) or stopped at 15 minutes (counted from when the
microphone was requested) with a message. Use
`--source system` for longer recordings. Spooling the microphone like the system
track lifts this and is tracked in #303. Timeline placement uses wall-clock
time, not per-chunk capture timestamps; only the startup gap is placed before
the first audio.

Realtime-safety of the Core Audio callback (it allocates and uses an unbounded
channel), clock drift between microphone and system tracks (gaps are filled with
silence only), default-output or device changes during a recording, spool cleanup
after SIGKILL (stale spools are removed after 24 h on the next start), and the
Windows start-timeout thread join are not handled in phase 1.

## Live testing (owner-run only)

Unit tests cover source selection, argument parsing, resampling, gap filling,
alignment, the two-track writer and reader, process matching, and permission
mapping. Tests that really capture are `#[ignore]` and must only be run with the
owner's explicit OK:

```
cd src-tauri
cargo test -p sagascript-core --features record macos_live -- --ignored --nocapture
cargo test -p sagascript-core --features record recorder_live -- --ignored --nocapture
```

The full owner test plan is in the phase-1 PR description.

## Me versus the others (two-track diarization, epic #299 phase 2)

For a two-track file `transcribe --diarize` (and `--meeting-json`, and meeting
reprocessing, which calls the same pipeline) does this instead of diarizing the
downmix:

1. Read both channels (they share one clock; padding at record time aligned them).
2. **System channel:** the unchanged diarization pipeline (default threshold, same
   options) plus Whisper word timestamps gives the other participants, labelled
   `SPEAKER_0`, `SPEAKER_1`, ...
3. **Microphone channel:** one fixed speaker, **`Me`**. Only the pyannote
   voice-activity part runs (all tracks collapsed, no embeddings, no clustering);
   Whisper transcribes the channel and keeps pieces that lie at least half inside
   microphone speech activity (drops hallucinations on quiet stretches).
4. Both timelines are merged ordered by start time (ties: `Me` first). Timestamps
   are the original file's, no offsets. The two channels are transcribed in
   separate passes, so it costs about two Whisper passes plus one segmentation.

Output (additive, absent for ordinary files): the meeting JSON gets a top-level
`"local_speaker": "Me"` (a speaker id; `Me` is always listed under `speakers`) and
the legacy `--diarize --json` output gets `"local_speaker": "Me"`. Plain/markdown/
SRT/VTT exports and `transcribe` text show the label `Me`; renaming the speaker
keeps `local_speaker` (it is an id), merging the local speaker into another keeps
the field on the merge target. Old readers that reject unknown fields reject only
files that have the field. Reprocessing: full recomputation works; selective
(`recluster`/`rediarize`) is refused for marked files because it works from a
downmix cache. `--diarize-cache` is neither read nor written for two-track input.

### Crosstalk (no headphones)

Without headphones the microphone also hears the remote participants. Assumption
for best results: **headphones**. A guard is nevertheless on by default because it
measurably helps on the simulation below: each microphone speech interval is
compared with the system channel over the same time. The loudness envelopes
(20 ms RMS frames) are correlated, maximised over a +/-300 ms acoustic delay; an
interval whose correlation is at least 0.6 is dropped as speaker echo (needs at
least 0.5 s and a non-silent system channel). Envelope correlation is gain
invariant, so it does not depend on how loud the microphone is relative to the
system capture. Limits: it judges whole intervals, so a local utterance that
merges with echo into one voice-activity interval (gap under 0.5 s) is lost
together with it; local speech talking over the remote side is kept only if it
decorrelates the envelopes. Set `SAGASCRIPT_TWO_TRACK_CROSSTALK_GUARD=off` to
disable it for diagnosis. Real rooms (reverberation, a speaker that is far
quieter than the system capture, non-linear processing) are weaker correlated
than the simulated leak: only an owner-run recording can confirm the threshold.
Echo cancellation against the system reference remains a follow-up.

### Offline evaluation

Nothing was recorded. `scripts/two_track_eval/build_synthetic.py` builds a two-track
WAV from a mono Riksdag interpellation (sv2 set, RTTM references): one reference
speaker's turns go to the left channel (silence elsewhere), everything else to the
right; `--leak-db -15` adds the right channel to the left at -15 dB, delayed 40 ms
and low-passed (a listener without headphones). `eval_two_track.py` scores
`transcribe --diarize --meeting-json` on the two-track path against the downmix
baseline (`--no-two-track`) with a frame-based DER (10 ms, 0.25 s collar, optimal
speaker mapping, scored inside reference speech); `run_eval.sh` drives it. Results
(`docs/benchmarks/data/two-track/results.json`), first 420 s of each file,
`kb-whisper-tiny`, default diarization threshold, "me" = Jessica Roden (S):

| Case | Pipeline | DER % | Confusion % | Miss % | FA % | Me recall % | False Me % | Speakers |
|---|---|---|---|---|---|---|---|---|
| IP_hc10606 (2 spk), clean | two-track | 13.1 | 1.0 | 11.5 | 0.6 | 85 | 0 | 3/2 |
| IP_hc10606, clean | downmix | 34.9 | 34.5 | 0.4 | 0.0 | (99) | (100) | 1/2 |
| IP_hc10606, leak -15 dB | two-track, guard on | 23.3 | 1.0 | 22.3 | 0.0 | 69 | 0 | 2/2 |
| IP_hc10606, leak -15 dB | two-track, guard off | 31.6 | 3.5 | 24.3 | 3.8 | 69 | 18 | 2/2 |
| IP_hc10606, leak -15 dB | downmix | 34.5 | 34.5 | 0.0 | 0.0 | (100) | (100) | 1/2 |
| IP_hd10115 (3 spk), clean | two-track | 14.7 | 3.7 | 10.6 | 0.4 | 41 | 0 | 2/2 |
| IP_hd10115, clean | downmix | 24.1 | 24.1 | 0.0 | 0.0 | (100) | (100) | 1/2 |
| IP_hd10115, leak -15 dB | two-track, guard on | 19.3 | 11.0 | 8.3 | 0.0 | 20 | 0 | 2/2 |
| IP_hd10115, leak -15 dB | two-track, guard off | 52.1 | 19.3 | 18.3 | 14.4 | 20 | 30 | 2/2 |
| IP_hd10115, leak -15 dB | downmix | 24.1 | 24.1 | 0.0 | 0.0 | (100) | (100) | 1/2 |

Reading: the downmix baseline collapses every speaker into one cluster (the known
#284 behaviour at this build's threshold), so its "me" columns are degenerate
(everything is "me") and its confusion is the whole minority share. Two-track
removes that confusion (1-4 % on clean, 1-11 % with leak) and, on the leak cases,
the guard cuts false "Me" from 18-30 % to 0 and DER by 8-33 points. The remaining
error is **miss**, not attribution: the microphone channel is mostly digital
silence with long turns, and Whisper tiny transcribes only part of it (Me recall
20-85 %), so the local user's words are under-reported. Follow-up: transcribe the
microphone channel only inside voice-activity intervals instead of the whole
channel, and re-measure with a larger model. Caveats: synthetic channels (perfect
separation, a clean linear leak), tiny model, 2 files, 7 minutes each, the
speaker count of the system channel comes from the build's diarization default.

