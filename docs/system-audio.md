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
`"crosstalk_guard": false|true`; the legacy `--diarize --json` output gets both too. Plain/markdown/
SRT/VTT exports and `transcribe` text show the label `Me`; renaming the speaker
keeps `local_speaker` (it is an id), merging the local speaker into another keeps
the field on the merge target. Old readers that reject unknown fields reject only
files that have the field. Reprocessing: full recomputation works; selective
(`recluster`/`rediarize`) is refused for marked files because it works from a
downmix cache. `--diarize-cache` is neither read nor written for two-track input.

### Crosstalk (no headphones)

**Record with headphones.** Without them the microphone also hears the remote
participants, and **other participants' speech may be attributed to "Me"**. That
is visible and correctable in review, so it is the default. When Sagascript sees a
substantial stretch of the microphone that mirrors the system audio it prints one
stderr hint recommending headphones or `--crosstalk-guard`. Real echo cancellation
(using the system channel as a waveform reference) is follow-up work.

`--crosstalk-guard` (or `SAGASCRIPT_TWO_TRACK_CROSSTALK_GUARD=on`) switches on an
**opt-in** guard; the meeting JSON records it as `"crosstalk_guard": true` and
reprocessing repeats it. Every microphone speech interval is cut into windows of
about a second. In each window the loudness envelopes (20 ms RMS) of the
microphone (fixed window) and the system channel (reference shifted by the
acoustic delay, +/-300 ms) are compared. A window is dropped when the envelope
correlation is at least 0.8 (best of 31 delays inflates chance correlation between
unrelated speech, so the bar is high; the table below was measured at 0.6) and the fitted system envelope leaves at most half of
the microphone energy unexplained. Windows shorter than 0.5 s, windows without a
valid aligned system reference (channel edges), and windows where the system is
silent are never judged and are kept; only the judged window is ever removed.

Why it is not the default: the test works on envelopes, not waveforms. When
independent local speech carries about as much energy as the echo (a worked
counterexample is a regression test, `equal_energy_double_talk_is_a_known_limitation`),
the window still looks like pure echo and **the user's own words are deleted**.
Deleting the user's speech silently is worse than a mislabelled echo.
Measured on the synthetic set below (leak and double-talk at -15 dB, guard at this
head): `--crosstalk-guard` takes false "Me" from 37-43 % to 0-2 % and DER from
35-74 % to 6-23 % in the leak and double-talk variants. In double-talk it keeps
all (hc10606, 92 -> 92 %) or 94 % (hd10115, 93 -> 87 %) of the voice-activity-
detected local speech, but those channels are simulated and not equal-energy.
Real rooms (reverberation, far quieter loudspeaker, non-linear processing)
correlate less; only an owner-run recording can say how the thresholds behave.

Two guard unit tests were loosened in round three, when the delay search was
changed to keep the microphone window fixed (and the threshold raised from 0.6 to
0.8): the simulated user's voice gain went from 0.1 to 0.4 (about 20 dB above the
echo, a user close to the microphone), because at 0.1 two unrelated synthetic
envelopes happened to correlate above the bar over a 1 s window; and the
"pure echo is removed" bound went from under 0.5 s kept to at most 1.0 s kept of
7 s, because the first window of the file has no negative-delay reference and sits
on the burst onset. Both are limits of the simulation and of envelope matching,
not new capability.

### Offline evaluation

Nothing was recorded. `scripts/two_track_eval/build_synthetic.py` builds a two-track
WAV from a mono Riksdag interpellation (sv2 set, RTTM references): one reference
speaker's turns go to the left channel (silence elsewhere), everything else to the
right. Variants: `clean`; `leak15` (the right channel is added to the left at -15 dB,
delayed 40 ms, low-passed: no headphones); `dt15` (leak plus unreferenced remote
speech playing while the user talks: double-talk). `eval_two_track.py` scores
`transcribe --diarize --meeting-json` on the two-track path against the downmix
baseline (`--no-two-track`): frame-based DER (10 ms, 0.25 s collar, optimal speaker
mapping, scored inside reference speech), and for "Me" the share of the user's
reference speech labelled Me, the share of the others' speech wrongly labelled Me,
and **words**: agreement of the Me text with a proxy transcript produced by the same
model on a mono file of the user's turns only (word recall by LCS, and WER against
the proxy). This measures consistency with what that model hears in clean speech,
**not absolute word accuracy**: there is no human reference, and errors the model
makes in both runs do not show. It also reports where speech is lost (voice-activity coverage, after
the guard, raw Whisper words vs kept words). `run_eval.sh` drives it, `summarize.py`
prints the table; raw results are in `docs/benchmarks/data/two-track/results.json`.
First 420 s of IP_hc10606 (2 speakers) and IP_hd10115 (3), "me" = Jessica Roden (S),
default diarization threshold, models `kb-whisper-tiny` and the larger installed
`kb-whisper-small`:

| Case | Pipeline | DER % | Conf % | Miss % | FA % | Me speech recall % | False Me % | Me word recall / WER % | VAD / after-guard coverage % | words ref / raw / kept |
|---|---|---|---|---|---|---|---|---|---|---|
| kb-whisper-small.IP_hc10606.clean guard=off | two-track | 5.2 | 0.6 | 3.3 | 1.3 | 97 | 0 | 90 / 11 | 91 / 91 | 424 / 419 / 408 |
| kb-whisper-small.IP_hc10606.clean guard=off | downmix | 34.5 | 34.5 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-small.IP_hc10606.dt15 guard=off | two-track | 51.6 | 10.1 | 18.8 | 22.6 | 86 | 43 | 88 / 85 | 92 / 92 | 424 / 729 / 710 |
| kb-whisper-small.IP_hc10606.dt15 guard=off | downmix | 34.6 | 34.5 | 0.1 | 0.0 | 100 | 100 |  | | |
| kb-whisper-small.IP_hc10606.dt15 guard=on | two-track | 22.8 | 6.4 | 4.7 | 11.7 | 86 | 0 | 88 / 21 | 92 / 92 | 424 / 449 / 433 |
| kb-whisper-small.IP_hc10606.leak15 guard=off | two-track | 35.0 | 4.3 | 18.5 | 12.2 | 96 | 42 | 89 / 83 | 91 / 91 | 424 / 720 / 703 |
| kb-whisper-small.IP_hc10606.leak15 guard=off | downmix | 34.5 | 34.5 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-small.IP_hc10606.leak15 guard=on | two-track | 6.3 | 0.6 | 4.3 | 1.3 | 96 | 0 | 89 / 18 | 91 / 91 | 424 / 440 / 427 |
| kb-whisper-small.IP_hd10115.clean guard=off | two-track | 2.9 | 0.0 | 2.9 | 0.0 | 90 | 0 | 89 / 14 | 81 / 81 | 181 / 181 / 170 |
| kb-whisper-small.IP_hd10115.clean guard=off | downmix | 24.1 | 24.1 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-small.IP_hd10115.dt15 guard=off | two-track | 74.1 | 11.2 | 37.6 | 25.3 | 71 | 37 | 92 / 317 | 93 / 93 | 181 / 757 / 733 |
| kb-whisper-small.IP_hd10115.dt15 guard=off | downmix | 24.1 | 24.1 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-small.IP_hd10115.dt15 guard=on | two-track | 14.6 | 4.5 | 4.3 | 5.8 | 73 | 2 | 90 / 29 | 93 / 87 | 181 / 232 / 199 |
| kb-whisper-small.IP_hd10115.leak15 guard=off | two-track | 63.4 | 6.6 | 35.4 | 21.4 | 99 | 37 | 89 / 324 | 81 / 81 | 181 / 762 / 732 |
| kb-whisper-small.IP_hd10115.leak15 guard=off | downmix | 24.1 | 24.1 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-small.IP_hd10115.leak15 guard=on | two-track | 6.4 | 0.4 | 4.8 | 1.2 | 90 | 2 | 89 / 28 | 81 / 81 | 181 / 225 / 194 |
| kb-whisper-tiny.IP_hc10606.clean guard=off | two-track | 9.2 | 0.0 | 8.0 | 1.2 | 92 | 0 | 70 / 34 | 91 / 91 | 454 / 368 / 361 |
| kb-whisper-tiny.IP_hc10606.clean guard=off | downmix | 34.9 | 34.5 | 0.4 | 0.0 | 99 | 100 |  | | |
| kb-whisper-tiny.IP_hc10606.dt15 guard=off | two-track | 40.8 | 12.5 | 15.2 | 13.1 | 86 | 43 | 76 / 99 | 92 / 92 | 454 / 755 / 734 |
| kb-whisper-tiny.IP_hc10606.dt15 guard=off | downmix | 34.5 | 34.5 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-tiny.IP_hc10606.dt15 guard=on | two-track | 19.7 | 4.8 | 8.8 | 6.2 | 85 | 1 | 76 / 32 | 92 / 92 | 454 / 447 / 427 |
| kb-whisper-tiny.IP_hc10606.leak15 guard=off | two-track | 37.7 | 14.8 | 15.7 | 7.1 | 83 | 42 | 64 / 107 | 91 / 91 | 454 / 693 / 674 |
| kb-whisper-tiny.IP_hc10606.leak15 guard=off | downmix | 34.5 | 34.5 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-tiny.IP_hc10606.leak15 guard=on | two-track | 15.4 | 6.8 | 7.7 | 0.9 | 83 | 1 | 64 / 40 | 91 / 91 | 454 / 385 / 368 |
| kb-whisper-tiny.IP_hd10115.clean guard=off | two-track | 8.4 | 2.6 | 5.8 | 0.0 | 65 | 0 | 73 / 43 | 81 / 81 | 174 / 196 / 186 |
| kb-whisper-tiny.IP_hd10115.clean guard=off | downmix | 24.1 | 24.1 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-tiny.IP_hd10115.dt15 guard=off | two-track | 64.7 | 19.5 | 26.0 | 19.2 | 67 | 38 | 92 / 347 | 93 / 93 | 174 / 777 / 755 |
| kb-whisper-tiny.IP_hd10115.dt15 guard=off | downmix | 24.1 | 24.1 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-tiny.IP_hd10115.dt15 guard=on | two-track | 15.8 | 5.3 | 4.7 | 5.8 | 69 | 1 | 91 / 30 | 93 / 87 | 174 / 224 / 198 |
| kb-whisper-tiny.IP_hd10115.leak15 guard=off | two-track | 60.8 | 15.1 | 30.4 | 15.3 | 67 | 38 | 72 / 406 | 81 / 81 | 174 / 845 / 818 |
| kb-whisper-tiny.IP_hd10115.leak15 guard=off | downmix | 24.1 | 24.1 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-tiny.IP_hd10115.leak15 guard=on | two-track | 11.7 | 2.9 | 8.0 | 0.8 | 65 | 1 | 73 / 59 | 81 / 81 | 174 / 242 / 213 |

Fix round (microphone words, Whisper over the whole channel -> Whisper only inside padded
voice-activity regions, zero-duration words kept): on the tiny model "Me" speech
recall on the clean files went from 85 / 41 % to 92 / 65 %, and DER from 13.1 / 14.7 %
to 9.2 / 8.4 %. Part of the earlier loss was a bug: single-token words have
`start == end` and were always dropped by the activity filter. (Old table: first
version of this section in the git history.)

Reading (every number above is from the code at this head; "guard=off" is the default, "guard=on" is `--crosstalk-guard`):
- The downmix baseline collapses every speaker into one cluster (the known #284 behaviour
  at this build's threshold): confusion 24-35 %, and its "me" columns are degenerate
  (everything is "me").
- Two-track removes that confusion; with the larger model the clean cases reach DER 2.9 and
  5.2 %. Word agreement with the proxy (same model, clean me-only audio) is about 90 %
  (WER 11-14 %) for the small model and 70-73 % (WER 34-43 %) for the tiny model on the
  clean files; this says how well the two-track path reproduces the model's own
  clean-speech transcript, not how correct the words are.
- Voice-activity coverage is 91 % (hc10606) and 81 % (hd10115) of the user's reference
  speech, so the segmenter, not Whisper, is the largest remaining loss on hd10115.
- Without headphones (leak15, dt15) and without the guard, leaked remote speech is labelled
  "Me": false Me 37-43 %, raw Me words 1.5-5x the proxy, DER 35-74 %. With `--crosstalk-guard`
  false Me is 0-2 % and DER 6-23 %. In double-talk the unreferenced remote speech counts as
  false alarm in both settings.
Caveats: synthetic channels (perfect separation, a linear leak), proxy word references (no
human transcript), two files of 7 minutes, the speaker count of the system channel comes
from this build's diarization default.
