# Recorder: architecture proposal

- Ticket: SAGA-547. Related: SAGA-289 (system audio, draft PR #300), SAGA-519 (live meetings, draft PR
  #521), SAGA-303, SAGA-309, and the iPhone recorder epic SAGA-531 (SAGA-533 recording domain, SAGA-534
  storage, SAGA-536 app).
- Status: **D1, D2 and D4-D6 decided by the owner on 2026-11-06; D3 is open and blocks phase 2.**
  Nothing here is implemented. Evidence:
  [recorder-formats-2026-11-05](research/recorder-formats-2026-11-05/README.md). Decision record:
  [ADR 0009](adr/0009-recording-storage.md) (Accepted).
- Date: 2026-11-05, base `dc6ef732`. Decisions recorded 2026-11-06.

## 1. Summary

Add a **recorder**: record the microphone, later also system audio, into files the user keeps, with a
lossless and a compact quality tier, from the app and the CLI.

1. A **recording is a folder**: one FLAC file per **track** (`mic`, later `system`) plus a small
   `recording.json` manifest. Tracks keep their native sample rate and are never resampled or mixed on
   disk. Alignment between tracks is metadata, and the 16 kHz audio the transcription pipeline wants
   is derived on demand.
2. Audio is copied off the realtime callback into a bounded queue; a **writer thread** encodes with a
   pure-Rust streaming FLAC encoder (`flacenc`), so memory stays flat for hours and a killed process
   leaves a playable file and a manifest marked `recording`, which recovery finalizes.
3. **Quality tiers** are both FLAC in v1: *Lossless* (native rate, 24-bit, about 295 MB/h per mono 48 kHz
   track) and *Compact* (16 kHz / 16-bit, 46 MB/h). A lossy tier (AAC) is a later, optional addition.
4. **Phase 1 is microphone only** and does not wait for the system-audio PRs. System audio and two
   tracks come in phase 2 behind a small `CaptureSource` trait.
5. The storage layer (manifest, FLAC writer, recovery) is portable core code with no audio-device
   dependency, so the future headless service and the iPhone recorder can share the same recording
   description (SAGA-533).

## 2. Requirements

From SAGA-547 and the owner's direction on 2026-11-05:

- Record from the app **and** the CLI (ADR 0001); source: microphone, or microphone + system audio.
- A lossless on/off setting.
- The file is saved in a place the user can see; nothing is stored unless the user records (ADR 0002).
  Recording is never silent: an indicator is visible while any source is active.
- Long recordings (hours) with bounded memory; a crash leaves something playable.
- A saved recording can go straight into file transcription and the meeting workflow.
- The same recording idea should later exist on the phone, so the description of a recording should
  not be desktop-specific.

## 3. What exists today

| Area | Today | Where |
|---|---|---|
| Mic capture | `cpal` default input only; device rate, mono average; samples in a `Vec<f32>` under a `Mutex` that can grow on the audio callback; silently stops storing after 15 min (`MAX_BUFFER_SECONDS`); resampled once to 16 kHz at stop | `sagascript-core/src/audio/capture.rs` (`record` feature) |
| Saved audio | `record -o` writes a 16 kHz mono 16-bit WAV with a plain `fs::write` | `audio/wav.rs`, `sagascript-cli/src/record.rs` |
| Files in | symphonia: WAV, FLAC, ALAC/M4A, AAC, MP3, Vorbis, MP4/MKV; **downmixes to mono 16 kHz**; no Opus | `audio/decoder.rs`, `Cargo.toml` |
| Playback | `<audio>` in the webview through the `meeting-audio:` protocol with range requests; the picker accepts wav, flac, ogg, opus, m4a, mp4, mp3 | `src-tauri/src/meeting_media.rs`, `MeetingReview.svelte` |
| Permissions | `NSMicrophoneUsageDescription` and the `audio-input` entitlement; no system-audio string | `src-tauri/Info.plist`, `Entitlements.plist` |
| System audio | not on `main`. Draft #300: process tap + aggregate device on macOS, WASAPI loopback on Windows, `record --source mic\|system\|both`, 16 kHz stereo WAV spool, mic held in memory (15 min) for `both`, conflicts with `main`. Draft #521: its own tap-only capture, host-clock timeline, AEC, live transcription, 16 kHz stereo WAV in an anonymous temp file; mergeable but behind `main`. The two share no code and touch the same four files | PRs #300, #521 |
| Durable writes | `fsutil::publish_new` (no-replace publish with fsync) is the ADR 0008 primitive; `record` does not use it | `sagascript-core/src/fsutil.rs` |

Consequences: capture needs a bounded, allocation-free callback path; both draft PRs save only the
transcription resolution; the decoder discards track separation.

## 4. Proposed architecture

```
 audio callbacks (realtime)            session / writer threads                 disk
 ┌────────────┐  Block   ┌────────────┐       ┌──────────────┐  frames   ┌───────────────────┐
 │ MicSource  │ ───────► │ bounded    │ ────► │ TrackWriter  │ ────────► │ rec-<id>.part/    │
 │ (cpal)     │  pooled  │ queue per  │       │ (FLAC enc.)  │           │   mic.flac        │
 ├────────────┤  blocks  │ track      │       │ + gap fill   │           │   recording.json  │
 │ SystemSrc  │ ───────► │            │       │ + meters     │           └─────────┬─────────┘
 │ (phase 2)  │          └────────────┘       └──────┬───────┘     stop/finalize:  │ rename (no replace)
 └────────────┘                                      │ levels, status               ▼
                                       RecordingSession (state machine) ──► events / CLI output
```

### 4.1 Principles

- **Capture the truth, derive the rest.** Store what the device delivered. Alignment, mixdown and the
  16 kHz transcription signal are computed from the files plus the manifest when needed.
- **The audio callback never allocates, locks or blocks.** It copies into a pre-allocated block and
  does a non-blocking push. This replaces the growing `Vec` behind a `Mutex` for the new path; the
  existing dictation path is left alone.
- **Lossless is a property of the pipeline, not a codec setting.** No resampling, gain, mixing or AEC
  on the stored track.
- **Crash tolerance by construction.** Every format and write order is chosen so that a kill at any
  point leaves a playable file and enough metadata to finalize.
- **Portable storage.** Manifest, FLAC writer and recovery live in `sagascript-core` without the
  `record` feature (no `cpal`), so a Linux or headless build and the iPhone work can use them.

### 4.2 Capture sources and the block contract

```rust
// sagascript-core, feature `record`
pub trait CaptureSource: Send {
    fn format(&self) -> NativeFormat;            // rate, channels, sample type as delivered
    fn start(&mut self, sink: BlockSink) -> Result<(), RecordingError>;
    fn stop(&mut self);
}
pub struct Block { /* pooled buffer: interleaved samples, host-clock stamp of first frame */ }
```

- `MicSource` wraps the current `cpal` setup (default input; a device choice can follow).
- `SystemSource` (phase 2) is the macOS process tap and the Windows loopback from the draft PRs, behind
  the same trait. A `FakeSource` that replays a fixture makes every session test run without hardware
  (ADR 0004).
- Level meters are computed in the callback as peak values stored in atomics; a poll thread reads them
  and emits events. No events are emitted from the callback.

### 4.3 Session, clock and gaps

`RecordingSession` is a small state machine: `Idle → Starting → Recording → Finalizing → Done | Failed`.
It owns the sources, one bounded queue and one writer per track, and a status snapshot.

- Each track records the host-clock time of its first frame (`start_host_ns`) and then counts samples.
  Two tracks with different rates and clocks stay alignable from metadata (offset plus measured rate
  ratio); the drift correction of SAGA-303 happens in the *derived* signal, not in the stored one.
  The existing `timeline.rs` in #521 already does host-clock alignment and a drift check and is the
  starting point.
- **Gaps:** if the queue overflows (`overrun`), the source stalls, or the device disappears, the writer
  inserts silence for the missing duration so sample index still maps to time, and records
  `{at_sample, length, reason}` in the manifest. Gaps are shown to the user, never hidden.
- **Device change** during a recording ends that track with `reason: device_changed` (SAGA-303 finding
  9); the other track continues. Auto-resume into a new segment is later work.
- **Disk space** is checked at start and polled; on a low-space threshold the session warns, at a
  critical threshold it stops and finalizes cleanly.

### 4.4 Writer and encoders

```rust
pub trait TrackEncoder: Send {
    fn push(&mut self, interleaved: &[f32]) -> Result<(), RecordingError>;
    fn finish(self: Box<Self>) -> Result<TrackSummary, RecordingError>; // samples, sha256, md5
}
```

- `FlacEncoder` (phase 1): `flacenc` frame API, block size 4096, written through a `BufWriter`; a
  placeholder STREAMINFO is written first and rewritten at finish (spike: bit-exact, 25 MB over an hour).
  Samples are converted to 24-bit integers (exact when the device delivers 16/24-bit data; the HAL float
  mantissa holds 24 bits). If the device reports 16-bit data the track is stored as 16-bit.
- Compact tier in phase 1 is the same encoder fed with a 16 kHz / 16-bit stream from the existing
  `rubato` resampler, so there is one codec, one crash model and no licence question.
- A lossy `AacEncoder` (AudioToolbox on macOS, Media Foundation on Windows) can implement the same trait
  later. It would need a crash-safe container plan (write ADTS as the journal and remux to M4A at stop,
  or fragmented MP4), because a plain M4A does not survive a kill (study §3), and a licence review.
- The writer thread drains the queue; at the measured 100-300x realtime it needs well under 1 % of a core
  per track. Disk stalls show up as overruns and gap markers, not as dropped audio without a trace.

### 4.5 On-disk layout, manifest and recovery

```
<recordings dir>/2026-11-05 14.02 Team meeting [a1b2c3]/
    mic.flac
    system.flac            # phase 2
    recording.json
```

```jsonc
{
  "schema_version": 1,
  "recording_id": "uuid-v4",
  "state": "complete",                      // "recording" while in progress
  "created_at": "2026-11-05T14:02:11Z",
  "client": { "kind": "desktop", "app_version": "1.x", "build": "<revision>" },
  "quality": "lossless",                    // or "compact"
  "tracks": [{
    "id": "mic", "role": "mic", "file": "mic.flac",
    "codec": "flac", "lossless": true,
    "sample_rate": 48000, "bits": 24, "channels": 1,
    "samples": 168837120, "sha256": "…",
    "start_host_ns": 123456789,             // alignment metadata
    "gaps": [{ "at_sample": 1234567, "length": 480, "reason": "overrun" }],
    "device": "MacBook Pro Microphone"
  }]
}
```

- The manifest uses file names relative to the folder and no absolute paths (ADR 0006 path scrubbing).
- It deliberately uses the vocabulary that SAGA-533 needs (recording id, tracks with role, codec, rate,
  hash, client kind) so the same record can describe a phone recording.
- **Write order:** the folder is created as `<name>.part`; `recording.json` with `state: "recording"` is
  written and fsynced first; tracks are appended. On a clean stop the writers finish, tracks are
  fsynced, the manifest is rewritten with `state: "complete"` through a temp file and rename, and the
  folder is renamed to its final name with the no-replace semantics of ADR 0008.
- **Recovery** (at app start, and `sagascript recordings recover`): a `.part` folder or a manifest still
  saying `recording` is finalized by scanning each FLAC (last frame gives the sample count; one decode
  pass, about 300x realtime, restores the MD5), then published normally and flagged `recovered: true`.
  Recovery never deletes audio.

### 4.6 Downstream use

- **Transcription:** a new `decode_tracks(recording)` returns each track decoded and resampled to 16 kHz
  with gaps and offsets applied (mono per track, so "me" and "others" stay separate for diarization).
  The existing single-file `decode_audio_file` is unchanged.
- **Playback in review:** phase 1 plays the mic track directly. For two tracks, the review view needs a
  mixdown stream; either serve a generated mixdown through `meeting-audio:` or play both tracks in sync.
  Decide in phase 2 after the webview FLAC check.
- **Export:** `recordings export --format flac|wav --layout tracks|stereo|mixdown` produces a single file
  on demand; stereo and mixdown resample to a common rate, which is stated in the output.

### 4.7 Surfaces

**CLI (names to be checked against `--help` and SAGA-533 before implementation):**

- `sagascript record --source mic|system|both [--app ID] --quality lossless|compact [--dir DIR]
  [--name NAME] [--duration SECONDS] [--transcribe] [--json]`.
- `sagascript recordings list|show|export|recover|delete`, the family SAGA-533 reserves.
- The existing `record -o file.wav` (16 kHz WAV, no transcription) keeps its meaning so scripts do not break.
- `sagascript doctor` / permissions report the microphone and system-audio permission (the draft #300
  already does this for system audio).

**Tauri (per `docs/recipes/add-tauri-command.md` and `add-event.md`):** `start_recording_session`,
`stop_recording_session`, `get_recording_session` (elapsed time, per-source level and status, gap count,
bytes written, free disk), `list_recordings`, plus a `recording-session-changed` event. Every failure
crosses IPC as `{ code, message, retryable }` (ADR 0006); new codes and rows in
`contracts/transport/command_errors.json`: `recording_permission_denied`, `recording_source_unavailable`,
`recording_disk_full`, `recording_device_changed`, `recording_recovery_needed`.

**Settings (per `add-setting.md`, ADR 0007):** `recording_source`, `recording_quality`,
`recording_directory` (default: a visible folder the user confirms on first use; never a hidden archive).

**UI:** a "Record" window opened from the tray, in line with the live-meeting view of #521: large start
and stop button, elapsed time, a meter per source, a source choice (microphone / microphone + computer
audio), the quality toggle, the folder, and after stopping: reveal in Finder, transcribe, delete. The
existing recording indicator is extended so that a system-audio source is visibly marked; choosing
system audio shows a consent reminder (recording other people may need their consent).

**Permissions:** microphone as today. System audio needs `NSAudioCaptureUsageDescription` (both drafts
add it) and, on macOS 14.4+, the System Audio Recording permission. No new entitlement per the drafts.

**Threads and locks:** the session state sits behind a leaf lock whose place in the lock order of
`ARCHITECTURE.md` §3 is fixed when it is implemented, locks use `lock_or_recover` (ADR 0005), no lock
(and never `SharedController`) is held across `.await`, and
macOS tap or permission calls that need the main thread go through `run_on_main_thread` (ADR 0003).

## 5. Fit with the existing drafts and the iPhone work

- **#300 and #521** are not merged, conflict with each other and each ship their own tap code. Neither
  stores a keepable original. Proposal: phase 1 ignores both. Before phase 2, choose one capture
  implementation: #521's tap-only aggregate avoids #300's refusal of headset microphones and has bounded
  blocks and a timeline, but is Apple Silicon and macOS 14.2+ only; #300 has the Windows loopback,
  per-app targeting and the `--source` CLI shape. Either way their storage code is replaced by this one.
- **SAGA-533 / 534 / 536 (iPhone):** one recording description (tracks with role, codec, rate, hash, client
  kind), the same FLAC files, and the same "save first, transcribe later" separation. The phone writes one
  `mic` track. A desktop recording can later be imported into the shared catalog without re-recording.
  iOS FLAC *encoding* is unverified and belongs in SAGA-532.
- **SAGA-519 (live transcription)** consumes the same blocks; it can run as a second consumer of the
  queue next to the writer so live text and a kept recording come from one capture.

## 6. Phases

| Phase | Scope | Depends on | Done when |
|---|---|---|---|
| **1a** | Portable core: manifest, `FlacEncoder`, store with `.part` write order, recovery. No hardware. | - | Round-trip and kill tests green on fixtures; `flac -t` accepts the output |
| **1b** | Mic capture path with pooled blocks and bounded queue, `RecordingSession`, gap handling, disk checks | 1a | A 2 h `FakeSource` soak keeps memory flat; overrun produces a gap marker |
| **1c** | CLI (`record` flags, `recordings`), Tauri commands and events, settings, Record window, indicator | 1b | App and CLI both record mic, lossless and compact; recording can be sent to transcription; CLI parity checked |
| **2** | `CaptureSource` for system audio (chosen implementation), two tracks, permissions flow, `decode_tracks`, review playback of two tracks | Owner decision D3; hardware OK from the owner for capture tests | Two aligned tracks, drift and device-change behaviour per SAGA-303, 30 min owner-run test |
| **3** | Optional: AAC tier with a crash-safe container; device-change auto-resume; AEC (SAGA-309) | Demand and licence review | - |
| **4** | Adopt desktop recordings into the SAGA-533/534 catalog and sync | SAGA-533 schema | A desktop recording appears on the phone |

Phase 1 delivers the first thing the requesting user asked for (record in the app, mic only, lossless
choice) without touching system-audio capture.

## 7. Test strategy (ADR 0004)

- Encoder: round trip is bit-exact against fixtures, decoded with `claxon` (dev-dependency) in unit tests;
  `flac -t` and a CoreAudio decode in the macOS acceptance script.
- Crash: truncate a written file at random points and require that recovery yields a playable file and a
  sample count within one block; kill a real process mid-write in an integration test.
- Session: `FakeSource` drives overrun, stall, device-change and disk-full cases; gap markers and manifest
  states are asserted.
- Manifest and IPC: a command-error row and test for each new code; fixtures never touch real settings
  (`SAGASCRIPT_SETTINGS_PATH`).
- Hardware: microphone and permission behaviour is checked with `cargo tauri build --debug`, and any test
  that captures audio on the owner's computers runs only after the owner's explicit OK.

## 8. Risks

| Risk | Mitigation |
|---|---|
| Real microphone FLAC ratios are worse than measured (wideband noise) | Measure on a real recording at the first owner-approved capture; sizes in the UI are estimates |
| FLAC playback in WKWebView / WebView2 unverified | Check in the installed app before phase 1c; `meeting_media.rs` already accepts `.flac` |
| `flacenc` is a single external crate | The encoder sits behind `TrackEncoder`; `libFLAC` bindings are the fallback (BSD, C) |
| "Lossless" claim | 24-bit integer FLAC from a device that delivers at most 24-bit data is exact; say so in the docs and do not apply gain or processing to the stored track |
| System-audio permission and headset (duplex) devices | Decided in D3; SAGA-303 tracks the duplex case |
| Disk fills during a long recording | Pre-flight estimate, polling, graceful stop, recovery |

## 9. Decisions

The owner approved D1, D2 and D4-D6 as recommended on 2026-11-06. D3 stays open.

| # | Decision | Outcome |
|---|---|---|
| D1 | Compact tier: reduced FLAC (16 kHz / 16-bit, 46 MB/h) or AAC (30 MB/h at 64 kbit/s) | **Decided:** reduced FLAC in v1; add AAC only if users ask for smaller files |
| D2 | One file per track + manifest, or one multichannel file | **Decided:** per track + manifest (no resampling, independent recovery, matches SAGA-533 "tracks"); offer a single-file export |
| D3 | Which capture code leads system audio: #521, #300, or a fresh `CaptureSource` that ports both | **Open.** Decide before phase 2; recommendation: lean on #521's tap and timeline and #300's Windows backend and CLI shape |
| D4 | Default folder | **Decided:** a visible folder (for example `~/Music/Sagascript`) confirmed on first use |
| D5 | Keep `record -o file.wav` unchanged | **Decided:** yes |
| D6 | Ship mic-only (phase 1) before system audio | **Decided:** yes |

## 10. Not done in this study

No audio was captured from a device, no draft PR or existing ticket text was changed, nothing was
published or pushed, and no product code was written. The throwaway `flacenc` crate lives outside the
repository.
