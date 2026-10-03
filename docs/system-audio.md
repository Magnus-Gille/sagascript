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

Assumption for best results: **headphones**. Without them the microphone also
hears the remote participants. A guard is on by default because it measurably
helps on the simulations below. Every microphone speech interval is cut into
windows of about a second. For each window the loudness envelopes (20 ms RMS) of
microphone and system channel are compared at the best acoustic delay (+/-300 ms).
A window is dropped as speaker echo only when both hold: the envelope correlation
is at least 0.6, and the system channel explains at least half of the microphone
energy (the residual after the best scaled copy of the system envelope is at most
0.5). The second test keeps local speech at its own level on top of the echo
(double-talk). Windows under 0.5 s, and windows where the system channel is
silent, are never judged. Envelope correlation is gain invariant, so it does not
depend on how loud the microphone is relative to the system capture. A flat
(non-fluctuating) envelope has no correlation to measure, so such windows are
kept: the guard errs towards keeping local speech.

Double-talk measurement (`dt15`: remote speech keeps playing and leaking at -15 dB
while the user speaks): the share of the user's reference speech covered by
microphone activity before / after the guard is 92 -> 90 % (hc10606) and 93 -> 84 %
(hd10115), the same for both models, so 98 % and 90 % of the detected local speech
survives, while false "Me" falls from 38-43 % to 0. Word recall is unchanged
(tiny 76 -> 75 %, small 87 %). The guard therefore stays on by default.
`SAGASCRIPT_TWO_TRACK_CROSSTALK_GUARD=off` disables it for diagnosis. Limits: the
channels are synthetic (a clean delayed linear leak). Real rooms (reverberation, a
loudspeaker far quieter than the system capture, non-linear processing) correlate
less, so only an owner-run recording can confirm the thresholds. Echo cancellation
against the system reference remains a follow-up.

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
and **words**: the Me text against a reference transcript (the same model on a mono
file of the user's turns only, a Whisper-vs-Whisper proxy, no human transcript),
word recall (LCS) and WER, plus where speech is lost (voice-activity coverage, after
the guard, raw Whisper words vs kept words). `run_eval.sh` drives it, `summarize.py`
prints the table; raw results are in `docs/benchmarks/data/two-track/results.json`.
First 420 s of IP_hc10606 (2 speakers) and IP_hd10115 (3), "me" = Jessica Roden (S),
default diarization threshold, models `kb-whisper-tiny` and the larger installed
`kb-whisper-small`:

| Case | Pipeline | DER % | Conf % | Miss % | FA % | Me speech recall % | False Me % | Me word recall / WER % | VAD / after-guard coverage % | words ref / raw / kept |
|---|---|---|---|---|---|---|---|---|---|---|
| kb-whisper-small.IP_hc10606.clean guard=on | two-track | 5.2 | 0.6 | 3.3 | 1.3 | 97 | 0 | 90 / 11 | 91 / 91 | 424 / 419 / 408 |
| kb-whisper-small.IP_hc10606.clean guard=on | downmix | 34.5 | 34.5 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-small.IP_hc10606.dt15 guard=on | two-track | 22.6 | 7.1 | 4.6 | 10.9 | 85 | 0 | 87 / 21 | 92 / 90 | 424 / 443 / 425 |
| kb-whisper-small.IP_hc10606.dt15 guard=on | downmix | 34.6 | 34.5 | 0.1 | 0.0 | 100 | 100 |  | | |
| kb-whisper-small.IP_hc10606.leak15 guard=on | two-track | 5.8 | 0.6 | 3.9 | 1.2 | 96 | 0 | 89 / 17 | 91 / 91 | 424 / 434 / 424 |
| kb-whisper-small.IP_hc10606.leak15 guard=on | downmix | 34.5 | 34.5 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-small.IP_hd10115.clean guard=on | two-track | 2.9 | 0.0 | 2.9 | 0.0 | 90 | 0 | 89 / 14 | 81 / 81 | 181 / 181 / 170 |
| kb-whisper-small.IP_hd10115.clean guard=on | downmix | 24.1 | 24.1 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-small.IP_hd10115.dt15 guard=on | two-track | 11.3 | 4.6 | 2.3 | 4.3 | 72 | 0 | 87 / 18 | 93 / 84 | 181 / 191 / 169 |
| kb-whisper-small.IP_hd10115.dt15 guard=on | downmix | 24.1 | 24.1 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-small.IP_hd10115.leak15 guard=on | two-track | 3.0 | 0.0 | 2.9 | 0.1 | 90 | 0 | 89 / 15 | 81 / 81 | 181 / 184 / 171 |
| kb-whisper-small.IP_hd10115.leak15 guard=on | downmix | 24.1 | 24.1 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-tiny.IP_hc10606.clean guard=on | two-track | 9.2 | 0.0 | 8.0 | 1.2 | 92 | 0 | 70 / 34 | 91 / 91 | 454 / 368 / 361 |
| kb-whisper-tiny.IP_hc10606.clean guard=on | downmix | 34.9 | 34.5 | 0.4 | 0.0 | 99 | 100 |  | | |
| kb-whisper-tiny.IP_hc10606.dt15 guard=off | two-track | 40.8 | 12.5 | 15.2 | 13.1 | 86 | 43 | 76 / 99 | 92 / 92 | 454 / 755 / 734 |
| kb-whisper-tiny.IP_hc10606.dt15 guard=on | two-track | 19.2 | 5.4 | 8.2 | 5.6 | 84 | 0 | 75 / 32 | 92 / 90 | 454 / 441 / 419 |
| kb-whisper-tiny.IP_hc10606.dt15 guard=on | downmix | 34.5 | 34.5 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-tiny.IP_hc10606.leak15 guard=off | two-track | 37.7 | 14.8 | 15.7 | 7.1 | 83 | 42 | 64 / 107 | 91 / 91 | 454 / 693 / 674 |
| kb-whisper-tiny.IP_hc10606.leak15 guard=on | two-track | 14.8 | 6.6 | 7.3 | 0.9 | 83 | 0 | 64 / 40 | 91 / 91 | 454 / 379 / 365 |
| kb-whisper-tiny.IP_hc10606.leak15 guard=on | downmix | 34.5 | 34.5 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-tiny.IP_hd10115.clean guard=on | two-track | 8.4 | 2.6 | 5.8 | 0.0 | 65 | 0 | 73 / 43 | 81 / 81 | 174 / 196 / 186 |
| kb-whisper-tiny.IP_hd10115.clean guard=on | downmix | 24.1 | 24.1 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-tiny.IP_hd10115.dt15 guard=off | two-track | 64.7 | 19.5 | 26.0 | 19.2 | 67 | 38 | 92 / 347 | 93 / 93 | 174 / 777 / 755 |
| kb-whisper-tiny.IP_hd10115.dt15 guard=on | two-track | 12.9 | 5.4 | 2.5 | 5.0 | 68 | 0 | 88 / 19 | 93 / 84 | 174 / 180 / 166 |
| kb-whisper-tiny.IP_hd10115.dt15 guard=on | downmix | 24.1 | 24.1 | 0.0 | 0.0 | 100 | 100 |  | | |
| kb-whisper-tiny.IP_hd10115.leak15 guard=off | two-track | 60.8 | 15.1 | 30.4 | 15.3 | 67 | 38 | 72 / 406 | 81 / 81 | 174 / 845 / 818 |
| kb-whisper-tiny.IP_hd10115.leak15 guard=on | two-track | 8.6 | 2.6 | 5.9 | 0.0 | 65 | 0 | 73 / 44 | 81 / 81 | 174 / 198 / 187 |
| kb-whisper-tiny.IP_hd10115.leak15 guard=on | downmix | 24.1 | 24.1 | 0.0 | 0.0 | 100 | 100 |  | | |

Fix round (microphone words, Whisper over the whole channel -> Whisper only inside padded
voice-activity regions, zero-duration words kept): on the tiny model "Me" speech
recall on the clean files went from 85 / 41 % to 92 / 65 %, and DER from 13.1 / 14.7 %
to 9.2 / 8.4 %. Part of the earlier loss was a bug: single-token words have
`start == end` and were always dropped by the activity filter. (Old table: first
version of this section in the git history.)

Reading:
- The downmix baseline collapses every speaker into one cluster (the known #284 behaviour
  at this build's threshold): confusion 24-35 %, and its "me" columns are degenerate
  (everything is "me").
- Two-track removes that confusion. With the larger model the clean cases reach DER 2.9 and
  5.2 %, word recall about 90 %, WER 11-14 % against the proxy reference. With the tiny
  model word recall is 64-88 % and WER 18-44 % (hallucinated or dropped words), so word
  quality is mostly a model effect.
- Where "Me" speech is still missing: voice-activity coverage is 91 % (hc10606) and only
  81 % (hd10115) of the user's reference speech, so the segmenter, not Whisper, is the
  largest remaining loss on hd10115; the guard removes 0-9 points more in double-talk;
  Whisper then recovers 85-90 % of the words with the small model.
- Crosstalk: guard on vs off for `leak15`/`dt15` (tiny): false Me 38-43 % -> 0, DER
  38-65 % -> 9-19 %, raw Me words 1.5-5x the reference without the guard.
- The "remaining error is missed speech" conclusion of the first version no longer holds
  in general: with the small model confusion and false alarm matter as much as miss in
  double-talk (the unreferenced remote speech in `dt15` appears as extra other-speaker
  speech, counted as false alarm).
Caveats: synthetic channels (perfect separation, a linear leak), proxy word references,
two files of 7 minutes, the speaker count of the system channel comes from this build's
diarization default.
