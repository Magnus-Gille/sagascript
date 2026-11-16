# Recorder: architecture proposal

- Ticket: SAGA-547. Related: SAGA-289 (system audio, draft PR #300), SAGA-519 (live meetings, draft PR
  #521), SAGA-303, SAGA-309, and the iPhone recorder epic SAGA-531 (SAGA-533 recording domain, SAGA-534
  storage, SAGA-536 app).
- Status: **D1 to D9 decided as recommended by the owner on 2026-11-06. D3 is decided in direction
  only: the minimum macOS version and Intel support are fixed before phase 2 starts.**
  Nothing here is implemented. Evidence:
  [recorder-formats-2026-11-05](research/recorder-formats-2026-11-05/README.md). Decision record:
  [ADR 0010](adr/0010-recording-storage.md) (Accepted).
- Date: 2026-11-05, base `dc6ef732`. Decisions recorded 2026-11-06.
- Review: independently reviewed on 2026-11-06 by Claude Opus 5.5 (API-only, text-only, two passes).
  What it found and what was done with each finding is in §11; several findings were reproduced with
  the spike and changed this design.

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
| Durable writes | `fsutil::publish_new` (no-replace publish of a *file* by hard link, with fsync) is the ADR 0008 primitive and `fsutil::sync_durably` the fsync policy; `record` uses neither | `sagascript-core/src/fsutil.rs` |

Consequences: capture needs a bounded, allocation-free callback path; both draft PRs save only the
transcription resolution; the decoder discards track separation.

## 4. Proposed architecture

```
 audio callbacks (realtime)            session / writer threads                 disk
 ┌────────────┐  Block   ┌────────────┐       ┌──────────────┐  frames   ┌───────────────────┐
 │ MicSource  │ ───────► │ bounded    │ ────► │ TrackWriter  │ ────────► │ <title> [id]/     │
 │ (cpal)     │  pooled  │ queue per  │       │ (FLAC enc.)  │           │   mic.flac        │
 ├────────────┤  blocks  │ track      │       │ + gap fill   │           │   mic.events.jsonl│
 │ SystemSrc  │ ───────► │            │       │ + meters     │           │   recording.json  │
 │ (phase 2)  │          └────────────┘       └──────┬───────┘           │   recording.lock  │
 └────────────┘                                      │ levels, status    └─────────┬─────────┘
                                                     │            stop/finalize:   │ manifest rewritten
                                                     ▼                             ▼
                                       RecordingSession (state machine) ──► events / CLI output
```

### 4.1 Principles

- **Capture the truth, derive the rest.** Store what the device delivered. Alignment, mixdown and the
  16 kHz transcription signal are computed from the files plus the manifest when needed.
- **The audio callback never allocates, locks or blocks.** It copies into a pre-allocated block and
  does a non-blocking push. This replaces the growing `Vec` behind a `Mutex` for the new path; the
  existing dictation path is left alone.
- **Lossless is a property of the pipeline, not a codec setting.** The lossless tier applies no
  resampling, gain, mixing or AEC to the stored track; its only conversion is float to 24-bit integer
  (§4.4). The compact tier is resampled by definition and the manifest says so.
- **Crash tolerance by construction.** Every format and write order is chosen so that a kill at any
  point leaves a playable file and enough metadata to finalize. Recovery is exact: it keeps every
  fully valid frame and reports what it kept.
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

- Each track records the host-clock time of its first frame (`start_host_ns`) and then counts frames.
  The clock domain is named in the manifest (`clock.domain`: macOS `mach_continuous_time`, which keeps
  counting through sleep, or Windows QPC), and the writer stores an anchor `(host_ns, frame_index)` at
  start, every 10 s and at every gap, so offset and drift can be measured later from metadata alone.
  The drift correction of SAGA-303 happens in the *derived* signal, never in the stored one. The
  existing `timeline.rs` in #521 does host-clock alignment and a drift check and is the starting
  point; whether `cpal`'s stream timestamps share its clock domain is checked in phase 2.
- **Gaps:** if the queue overflows (`overrun`), the source stalls, or the device disappears, the writer
  inserts silence for the missing duration so the frame index still maps to time, and records
  `{at_frame, length, reason}`. Overrun lengths come from a dropped-frame counter that the callback
  increments, never from the clock; stall lengths come from the anchors. A watchdog declares a stall
  when no block has arrived for 500 ms. Gaps are written to the event journal (§4.5) the moment they
  happen, so a crash does not hide them, and they are shown to the user.
- **Format change, device change and sleep end the track.** A FLAC stream cannot change sample rate or
  channel count, so a changed device, a changed rate or channel count (for example a Bluetooth headset
  switching profile when a microphone client opens) or a system sleep closes the track with
  `ended_reason: device_changed | format_changed | system_sleep` (SAGA-303 finding 9); in phase 2 the
  other track continues. Filling a sleep with silence would write hours of nothing, so the recording
  ends instead. Auto-resume into a second segment file is later work, and the track id rule in §4.5
  leaves room for it.
- **Disk space** is checked at start (the uncompressed rate is the worst-case estimate) and polled; on
  a low-space threshold the session warns, at a critical threshold it stops and finalizes cleanly.
  Polling can miss the moment a disk fills, so a write error (`ENOSPC`) is a first-class case: the
  track is marked `failed: disk_full`, cut back to its last complete frame, its STREAMINFO is
  rewritten in place (which needs no new space), and a padded manifest is rewritten in place. If even
  that fails, the manifest keeps `state: recording` and recovery finishes the job once space exists.

### 4.4 Writer and encoders

```rust
pub trait TrackEncoder: Send {
    fn push(&mut self, interleaved: &[f32]) -> Result<(), RecordingError>;
    fn finish(self: Box<Self>) -> Result<TrackSummary, RecordingError>; // frames, audio_sha256, md5
}
```

- `FlacEncoder` (phase 1): `flacenc` frame API, block size 4096, written through a `BufWriter`; a
  placeholder STREAMINFO is written first and rewritten at finish (spike: bit-exact, 25 MB peak
  resident memory over an hour). Rules found by the spike and the review:
  - **Block sizes:** after the last frame, set STREAMINFO `min_blocksize = max_blocksize = 4096`.
    `flacenc` lowers the minimum to the length of a short final frame, which makes libFLAC warn
    "file might not be seekable" (reproduced, study §4). Real recordings almost never end on a block
    boundary, so this is the normal path.
  - **No fixed predictors:** `use_fixed = false`. With the default config `flacenc` 0.5.1 wrote a
    100 MB frame for one 4096-sample block of a full-scale 24-bit square wave, plus a STREAMINFO that
    `claxon` rejects (study §4). LPC-only is clean on every edge signal and costs 0.01 % on speech.
    The encoder also compares every frame with its raw size and stores the block verbatim if the
    encoded frame is larger.
  - **Sample conversion:** the lossless tier takes the capture API's `f32` samples, scales by 2^23,
    rounds to nearest, clamps to [-2^23, 2^23 - 1] and maps NaN to 0, and counts clipped samples in
    the manifest. A float sample of exactly +1.0 would otherwise be out of range. It always stores
    24-bit: `cpal` does not expose the device's physical bit depth, so storing 16-bit for a 16-bit
    device is not available in v1. A 16- or 24-bit integer device is exact, since `f32` carries 24 bits
    of mantissa; a 32-bit or float device is quantised, so "lossless" means lossless relative to the
    signal the capture API delivered. On Windows shared mode that is the OS mix after any audio
    processing objects, and the docs say so.
  - **Metadata blocks:** a PADDING block is reserved after STREAMINFO, and a SEEKTABLE is filled at
    finish and by recovery, so a multi-hour file seeks in a webview and later tags need no rewrite of
    900 MB. Whether `flacenc` writes these blocks is checked in 1a; if not, a small writer for them
    is part of the store.
- Compact tier in phase 1 is the same encoder fed with a 16 kHz / 16-bit stream from a chunked
  `rubato` resampler. Today's `rubato` use is one batch at stop, so streaming use, its delay
  compensation in `start_host_ns`, and its flush at finish are new work. Rounding is to nearest without
  dither, the manifest records `resampled_from`, and gap positions are always in stored-track frames.
  There is still one codec, one crash model and no licence question.
- A lossy `AacEncoder` (AudioToolbox on macOS, Media Foundation on Windows) can implement the same trait
  later. It would need a crash-safe container plan (write ADTS as the journal and remux to M4A at stop,
  or fragmented MP4), because a plain M4A does not survive a kill (study §3), and a licence review.
- The writer thread drains the queue. In the spike the encoder used about 12-16 s of CPU per hour of
  audio (roughly 0.5 % of one core per mono track); wall-clock speed was 65-300x realtime depending on
  how busy the machine was. Disk stalls show up as overruns and gap markers, not as dropped audio
  without a trace.

### 4.5 On-disk layout, manifest and recovery

```
<recordings dir>/2026-11-05 14.02 Team meeting [a1b2c3]/
    mic.flac
    system.flac            # phase 2
    recording.json
    mic.events.jsonl       # append-only journal: start, clock anchors, gaps; kept after finish
    recording.lock         # held while a process owns the folder; removed at finish
```

```jsonc
{
  "schema_version": 1,
  "min_reader_version": 1,                  // readers ignore unknown fields and refuse only above their own version
  "recording_id": "uuid-v4",
  "title": "Team meeting",                  // free text; the folder name is a sanitised form of it
  "state": "complete",                      // "recording" while in progress
  "recovered": false,                       // true when recovery finalized it
  "started_at": "2026-11-05T14:02:11.482+01:00",
  "client": { "kind": "desktop", "app_version": "1.x", "build": "<revision>" },
  "quality": "lossless",                    // or "compact"
  "tracks": [{
    "id": "mic", "role": "mic", "file": "mic.flac",
    "codec": "flac", "lossless": true,
    "sample_rate": 48000, "bits": 24, "channels": 1,
    "frames": 168837120,                    // sample frames (per channel)
    "audio_sha256": "…",                    // over the FLAC frame bytes after the metadata blocks
    "clock": { "domain": "mach_continuous", "start_ns": 123456789,
               "anchors": [[123456789, 0]] },   // [host_ns, frame_index]; later anchors live in the journal
    "gaps": [{ "at_frame": 1234567, "length": 480, "reason": "overrun" }],
    "ended_reason": "stopped",              // or device_changed, format_changed, system_sleep, disk_full, interrupted
    "clipped_samples": 0,
    "resampled_from": null,                 // 48000 for the compact tier
    "device": "MacBook Pro Microphone"      // local detail: left out when a manifest is exported or synced
  }]
}
```

- The manifest uses file names relative to the folder and no absolute paths (ADR 0006 path scrubbing).
- It deliberately uses the vocabulary that SAGA-533 needs (recording id, tracks with role, codec, rate,
  hash, client kind) so the same record can describe a phone recording.
- **Manifest rules:** unknown fields are ignored and `min_reader_version` is the only thing that makes
  a reader refuse. `frames` counts sample frames, not interleaved samples. A track id is the role, and a
  later segment after a device change would be `mic.2`. Device names can hold personal names, so
  `device` is a local detail that exports and sync leave out. The vocabulary is aligned with SAGA-533
  before the schema is frozen in 1a.
- **Ownership and naming:** the folder is created with its **final name** by `create_dir`, which fails
  if the name exists (atomic, no replace), with mode `0700` (files `0600`) on Unix. The name is the
  sanitised title plus a short id: characters outside a portable set, path separators, Windows
  reserved names and trailing dots or spaces are removed, the length is capped, and a collision at
  start gets a new id. There is no `.part` rename. ADR 0008's `publish_new` hard-links a *file* and
  cannot publish a directory, an exclusive directory rename is a different primitive on each platform,
  and on Windows renaming a folder fails while a scanner or indexer holds a handle inside it. Instead
  the session holds an exclusive advisory lock on `recording.lock` for its whole life, and the
  manifest `state` says whether the recording is finished.
- **Write order** (every fsync is `fsutil::sync_durably`, which is `F_FULLFSYNC` on macOS):
  1. `create_dir`, fsync the parent, take the lock.
  2. Write `recording.json` with `state: "recording"` and fsync it; create the track files and the
     journal and fsync the folder.
  3. While recording, the writer appends frames, writes journal lines for the start, every anchor and
     every gap, and fsyncs tracks and journal every 10 s and at each gap.
  4. At stop: write the last frame, fsync, rewrite STREAMINFO (and SEEKTABLE) in place, fsync.
  5. Write `recording.json.tmp` with `state: "complete"`, fsync, rename it over `recording.json`
     (replacing our own file is the intent here), fsync the folder, delete the lock.

  The guarantee is stated twice. A **killed process** loses at most the bytes still in the
  `BufWriter`. A **power loss or OS crash** loses at most what was written since the last fsync,
  about 10 s.
- **Recovery** (at app start, and `sagascript recordings recover`) looks only at folders whose manifest
  says `recording` **and** whose lock it can take without blocking, so a recording owned by another
  live process (the CLI while the app starts, a second app instance) is never touched. For each track
  it scans frames forward, checking the header CRC-8 and frame CRC-16, keeps every valid frame, and
  moves any bytes after the last valid frame into `<track>.tail` instead of deleting them. A torn
  final frame is real: `flac -t` rejects it while ffmpeg and CoreAudio skip it (study §4). It then
  takes the exact frame count from the valid frames, recomputes the MD5 and `audio_sha256`, rewrites
  STREAMINFO and SEEKTABLE, rebuilds gaps and anchors from the journal, and writes the manifest with
  `recovered: true` and `ended_reason: "interrupted"`. Recovery never deletes audio.

### 4.6 Downstream use

- **Transcription:** a new `decode_tracks(recording)` yields each track decoded and resampled to 16 kHz
  with gaps and offsets applied (mono per track, so "me" and "others" stay separate for diarization).
  It is a chunked iterator, not a vector: three hours of one track is about 690 MB of `f32` at 16 kHz,
  and the 2 h soak test covers the transcription path too. The existing single-file
  `decode_audio_file` is unchanged.
- **Playback in review:** phase 1 plays the mic track directly, only when `state` is `complete` (a
  growing or unrecovered file reports an unknown duration), and only after the installed-app check that
  FLAC plays and seeks in WKWebView and WebView2. If either rejects 24-bit or high-rate FLAC, the
  fallback is a 16-bit WAV or transcode served through `meeting-audio:`; the storage format does not
  change. For two tracks, the review view needs a mixdown stream; either serve a generated mixdown
  through `meeting-audio:` or play both tracks in sync. Decide in phase 2.
- **Export:** `recordings export --format flac|wav --layout tracks|stereo|mixdown` produces a single file
  on demand; stereo and mixdown resample to a common rate, which is stated in the output.

### 4.7 Surfaces

**CLI (names to be checked against `--help` and SAGA-533 before implementation):**

- `sagascript record --source mic|system|both [--app ID] --quality lossless|compact [--dir DIR]
  [--name NAME] [--duration SECONDS] [--transcribe] [--json]`.
- `sagascript recordings list|show|export|recover|delete`, the family SAGA-533 reserves.
- The existing `record -o file.wav` (16 kHz WAV, no transcription) keeps its meaning so scripts do not break.
- Recording is never silent, in the CLI too: it prints a start banner and a status line to stderr every
  10 s (elapsed time, level, gaps, free disk), and no fully silent headless capture is offered. `--name`
  is a title, sanitised for the folder name (§4.5).
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
existing recording indicator is extended so that a system-audio source is visibly marked. The first
recording on a computer, and every system-audio recording, shows a consent reminder (recording other
people may need their consent; a microphone picks up a room as much as system audio does).

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
| **1a** | Portable core: manifest and journal, `FlacEncoder` with the §4.4 rules, store with the §4.5 write order and lock, recovery. No hardware. | - | Round trip bit-exact on the §7 corpus (short final block, edge signals, 16 kHz compact config) with `flac -t` free of warnings; kill and truncation tests recover exactly the valid frames |
| **1b** | Mic capture path with pooled blocks and bounded queue, `RecordingSession`, gap handling, watchdog, format-change, sleep and disk-full handling | 1a | A 2 h `FakeSource` soak keeps memory flat; overrun produces a gap marker; each ending reason of §4.3 is asserted in a test |
| **1c** | CLI (`record` flags, `recordings`), Tauri commands and events, settings, Record window, indicator | 1b | App and CLI both record mic, lossless and compact; recording can be sent to transcription; CLI parity checked; FLAC plays and seeks in the installed app on macOS and Windows, or the §4.6 fallback is in |
| **2** | `CaptureSource` for system audio (chosen implementation), two tracks, permissions flow, `decode_tracks`, review playback of two tracks | Owner decision D3; hardware OK from the owner for capture tests | Two aligned tracks, drift and device-change behaviour per SAGA-303, 30 min owner-run test |
| **3** | Optional: AAC tier with a crash-safe container; device-change auto-resume; AEC (SAGA-309) | Demand and licence review | - |
| **4** | Adopt desktop recordings into the SAGA-533/534 catalog and sync | SAGA-533 schema | A desktop recording appears on the phone |

Phase 1 delivers the first thing the requesting user asked for (record in the app, mic only, lossless
choice) without touching system-audio capture.

## 7. Test strategy (ADR 0004)

- Encoder: round trip is bit-exact against a fixture corpus, decoded with `claxon` or `symphonia` in
  unit tests (`claxon` would be dev-only; `symphonia` is already a dependency and may replace it);
  `flac -t` and a CoreAudio decode in the macOS acceptance script. The corpus has lengths that are and
  are not block multiples, 44.1 / 48 / 96 kHz, 1 / 2 / 8 channels, 16- and 24-bit, and the edge
  signals of the study (digital silence, DC at both extremes, full-scale square, white noise,
  alternating extremes, a single impulse). No frame may be larger than its raw size. `flacenc` is
  pinned to an exact version.
- Crash: truncate at random byte offsets, frame-aligned and mid-frame, and require that recovery keeps
  exactly the fully valid frames (the frame count equals the sum of their blocks and the torn bytes
  land in `.tail`); `SIGKILL` a real process mid-write without any flush; run the CLI recording and
  app-start recovery at the same time and require that recovery leaves the live recording alone.
- Names and manifest: a table test for title sanitisation (separators, `..`, Windows reserved names,
  trailing dots, long titles, collisions); an old reader on a manifest with unknown fields; disk-full
  injected on a small disk image; compact-tier alignment after the resampler delay; webview playback
  and seeking in the installed app.
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
| `flacenc` is a single 0.x crate with a reproduced edge-case bug (100 MB frame from the fixed predictors, study §4) | Pin the version; `use_fixed = false`; per-frame size guard with a verbatim fallback; the edge-signal corpus runs in CI; the encoder sits behind `TrackEncoder`. Fallbacks: AudioToolbox on macOS (a native FLAC encoder, verified by the study) and `libFLAC` bindings elsewhere, which would give up the pure-Rust portable core |
| "Lossless" claim | 24-bit integer FLAC from a device that delivers at most 24-bit data is exact; a 32-bit or float device is quantised to 24 bits, and Windows shared mode delivers the OS mix. Say so in the docs and apply no gain or processing to the stored track |
| Folder on FAT32 / exFAT, a network volume or a cloud-synced folder (OneDrive, iCloud) | Detect the file-system class at start and warn; roll over to a new segment before 4 GiB on FAT (about 14 h of one 48 kHz / 24-bit mono speech track at 295 MB/h, far less for noisy or multichannel audio); cloud sync of a growing folder can lock or upload it, so warn there too (D9) |
| Titles in folder names reach Spotlight, backups and shell history | Say so where the title is entered; folder mode `0700`; an id-only name is a later option |
| System-audio permission and headset (duplex) devices | Decided in D3; SAGA-303 tracks the duplex case |
| Disk fills during a long recording | Pre-flight estimate, polling, graceful stop, recovery |

## 9. Decisions

The owner approved D1 to D9 as recommended on 2026-11-06. D3 is decided in direction (a fresh shared
`CaptureSource` that takes the best of #521 and #300); the minimum macOS version and Intel support are
still to be fixed before phase 2 starts. D7 applies to each track on its own: the system-audio track
of phase 2 is a track, not a channel of the microphone track.

| # | Decision | Outcome |
|---|---|---|
| D1 | Compact tier: reduced FLAC (16 kHz / 16-bit, 46 MB/h) or AAC (30 MB/h at 64 kbit/s) | **Decided:** reduced FLAC in v1; add AAC only if users ask for smaller files |
| D2 | One file per track + manifest, or one multichannel file | **Decided:** per track + manifest (no resampling, independent recovery, matches SAGA-533 "tracks"); offer a single-file export |
| D3 | Which capture code leads system audio: #521, #300, or a fresh `CaptureSource` that ports both. Includes the minimum macOS version and Intel support: #521 is Apple Silicon and macOS 14.2+ only, while the System Audio Recording permission in this design is cited for 14.4+ | **Decided in direction (2026-11-06):** a fresh `CaptureSource` that leans on #521's tap and timeline and #300's Windows backend and CLI shape; reconcile the OS versions (minimum macOS, Intel) before phase 2 starts |
| D4 | Default folder | **Decided:** a visible folder (for example `~/Music/Sagascript`) confirmed on first use |
| D5 | Keep `record -o file.wav` unchanged | **Decided:** yes |
| D6 | Ship mic-only (phase 1) before system audio | **Decided:** yes |
| D7 | Channel policy (added by the review): the mic today is averaged to mono; FLAC holds at most 8 channels and a multi-input interface has more | **Decided:** per track, store 1-2 channels as delivered; above 2 the user picks a pair (default 1-2); the compact tier is a mono downmix and says so; size estimates multiply by the channel count |
| D8 | Deleting a recording (added by the review) | **Decided:** the app moves the folder to the Trash / Recycle Bin; `recordings delete` needs `--yes` and removes it |
| D9 | File-system class and per-OS default folder (added by the review), extending D4 | **Decided:** `~/Music/Sagascript` on macOS; on Windows `Music\Sagascript` unless that folder is cloud-synced, then ask; warn on FAT, network and cloud-synced folders |

## 10. Not done in this study

No audio was captured from a device, no draft PR (#300, #521) or existing ticket text was changed, and
no product code was written. The throwaway `flacenc` crate lives outside the repository. The study and
this design are published as the documentation-only draft PR #548.

## 11. Independent review (2026-11-06)

Reviewer: Claude Opus 5.5 (`claude-opus-5-5`) through the API-only `claude-feedback` route, text only,
no repository or tools. The scoped material was this design, ADR 0010, the study README and the spike
source (about 47 kB; no credentials). The first pass (effort high) was cut off by its output limit
after finding 11 (19.0 k input and 24.0 k output tokens); a second pass (effort medium, 19.0 k input
and 10.8 k output tokens) produced findings 12 to 26. I treated the findings as hypotheses. Where the
spike could test one it did, which changed three of them from opinion to fact.

| # | Finding | Result |
|---|---|---|
| 1 | Recovery can finalize a live recording | Accepted: `recording.lock`, recovery takes it non-blocking (§4.5) |
| 2 | Gaps and start time lost after a crash | Accepted: append-only journal, fsynced at gaps (§4.3, §4.5) |
| 3 | The spike's crash is not a real kill; torn tail | **Reproduced:** a mid-frame cut decodes in ffmpeg and CoreAudio but fails `flac -t`; recovery truncates to the last valid frame and keeps the rest as `.tail` |
| 4 | Short final block untested | **Reproduced:** bit-exact, but STREAMINFO min block 251 gave a libFLAC "not seekable" warning; fixed by restoring min = max = 4096 (§4.4) |
| 5 | Durability order incomplete | Accepted: full sequence in §4.5; `fsutil::sync_durably` already does `F_FULLFSYNC` with a fallback |
| 6 | Directory publish, Windows rename | **Confirmed in the code:** `publish_new` hard-links a file; the `.part` rename is dropped for `create_dir` with the final name plus a lock |
| 7 | Disk full is a write error | Accepted (§4.3) |
| 8 | Clock model and domain | Accepted: `clock.domain`, anchors (§4.3, §4.5); the `cpal` clock is checked in phase 2 |
| 9 | Format change, sleep, stall | Accepted: they end the track; watchdog (§4.3) |
| 10 | Channel policy | Accepted as D7 |
| 11 | Float conversion and the lossless claim | Accepted: conversion rule, always 24-bit, claim narrowed (§4.4, §8) |
| 12 | Names, collisions, reserved names | Accepted (§4.5, §7) |
| 13 | Privacy of titles and device, permissions | Accepted: `0700` / `0600`, `device` stays local, titles documented; id-only names later (§8) |
| 14 | CLI is not "never silent" | Accepted: stderr banner and status line, consent reminder on first use (§4.7) |
| 15 | No seek table or padding | Accepted: PADDING and SEEKTABLE; whether `flacenc` writes them is checked in 1a |
| 16 | Compact tier contradicts "never resampled" | Accepted: wording and streaming resampler work (§4.1, §4.4) |
| 17 | Manifest forward compatibility | Accepted: `min_reader_version`, `frames`, id rule (§4.5); aligned with SAGA-533 in 1a |
| 18 | `sha256` coverage | Accepted: `audio_sha256` over the frame bytes, computable while streaming |
| 19 | `decode_tracks` unbounded memory | Accepted: chunked iterator (§4.6) |
| 20 | File-system class | Accepted as risk and D9; segment rollover is a 1b task |
| 21 | `flacenc` risk, edge signals | **Reproduced a real bug:** a 100 MB frame from the fixed predictors on a full-scale square wave; `use_fixed = false` and a frame-size guard (§4.4, §8) |
| 22 | Numeric wording | Partly accepted: peak resident memory and CPU wording corrected. Declined: the claimed 299 MB/h, which used 58 min instead of the 3517 s soak (289 MB over 3517 s is 296 MB/h) |
| 23 | Soak loops one clip | Deferred: 1a adds a non-repeating wideband soak; the edge corpus covers the predictor paths |
| 24 | No playback fallback, in-progress file | Accepted (§4.6, phase 1c) |
| 25 | Missing decisions, ADR acceptance | Accepted: D7 to D9, OS versions in D3; ADR 0010 states the webview condition |
| 26 | Test oracle and missing tests | Accepted (§7) |

Not judged by the reviewer, so still open: WKWebView and WebView2 behaviour with 24-bit FLAC through the
custom protocol, `cpal` clock behaviour, TCC for `~/Music` and removable volumes, the SAGA-533 schema.
