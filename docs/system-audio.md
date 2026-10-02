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
`sagascript transcribe` and the meeting workflow accept the two-track file
unchanged (decoders downmix to mono); splitting the channels for "me vs. others"
diarization is phase 2 (`read_two_track_wav` already exists for it).

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
channel convention is documented and identified by the user choosing `both`. WAV
is capped at 4 GiB (about 34 hours at this rate), which the writer enforces.

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
