# SAGA-564 working spec (phase 1c) - delete before merge

Worktree: this one. Read `AGENTS.md`, `docs/recorder-design.md` §4.5-4.7, §9, the recipes in `docs/recipes/`,
ADR 0001/0003-0007, `docs/ARCHITECTURE.md` (recording section), `src-tauri/crates/sagascript-core/src/recording/`.
Hard rules: NEVER open a real microphone/output device in a test or command; never launch the app; no new crates;
no `.env*`/`secrets/**`; do not push; run ONE cargo command at a time (the Mac is shared); commit in small
conventional commits ending with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`;
restore `src-tauri/gen/schemas/*` if a build regenerates them. Always `cd` to the worktree root with an absolute path.

## Core (sagascript-core)

### Settings (add-setting.md, ADR 0007)
- `recording_quality: RecordingQuality` (`Lossless` default | `Compact`; snake_case; `id()/parse_id()`), CLI/Tauri convert to `recording::Quality`.
- `recording_directory: String` (empty = default folder). `fn Settings::recordings_dir(&self) -> PathBuf`:
  empty -> `recording::default_recordings_dir()` = `dirs::audio_dir()` (macOS `~/Music`, Windows `Music`) joined `Sagascript`,
  falling back to `dirs::home_dir()/Sagascript`.
- `recording_channel_pair: [u16; 2]` default `[1, 2]` (1-based, `[n, n]` = mono channel n; maps to `MicConfig::channel_pair`).
  CLI value syntax `1-2` / `3-3`; reject 0.
- internal markers (INTERNAL_FIELDS): `recording_folder_confirmed: bool`, `recording_consent_acknowledged: bool` (first-use, D4 + design consent reminder).
- No `recording_source` setting (mic only).
- Each user field gets a CLI `config` key; migration/default tests; existing files without the keys load defaults.

### `recording/decode.rs` - `decode_tracks`
`pub fn decode_tracks(folder: &Path) -> Result<TrackDecoder, RecordingError>`; `TrackDecoder: Iterator<Item = Result<TrackChunk, RecordingError>>`,
`TrackChunk { track_id: String, role: String, start_sample: u64 /*16 kHz index within the recording timeline*/, samples: Vec<f32> /*16 kHz mono, <= 32_000 samples*/ }`.
Tracks are yielded one after another (all chunks of track 1, then track 2). Offsets: a track's `clock.start_ns` relative to the earliest
track start becomes leading silence in 16 kHz samples (emit as zero chunks, no giant allocation). Gaps are already silence inside the stored FLAC;
verify against the manifest (`frames` / gaps) and surface a typed error (`CorruptTrack`) if the decoded frame count is below the manifest's.
Use symphonia (already a dependency) + `audio::resample::StreamingResampler`; downmix 2 channels to mono. Never a whole-track Vec.
Only for `state == complete` or recovered; a `recording` state -> `RecordingError::NotRecording`-style typed error (add a variant only if needed; new codes need rows later).

### `recording/library.rs`
`RecordingSummary { folder_name, recording_id, title, state, recovered, started_at, quality, duration_seconds, bytes, track_count, gap_count, gap_seconds, ended_reason: Option<String>, codec, sample_rate, channels, needs_recovery: bool, playable: bool /* state complete */ }` (Serialize).
`list_recordings(root) -> Vec<RecordingSummary>` newest first (by started_at, then name); folders without a readable manifest are skipped (log, no path).
`read_recording(root, name_or_id) -> Result<(PathBuf, Manifest)>` where `name_or_id` is a folder name or a recording-id prefix (>= 6 chars, unique);
folder name must be a single path component (no separators, no `..`).
`delete_recording_folder(root, name_or_id)`: refuses a folder that has no `recording.json`, whose lock is held (`RecordingError::Locked`), and anything not directly under root. Removes the folder tree (CLI `--yes` path; the app trashes instead).
`audio_track_path(folder, manifest, "mic")` helper returning the FLAC path for playback.

### `recording/export.rs`
`export_recording(folder, &ExportOptions { format: Flac|Wav, layout: Tracks|Stereo|Mixdown, out: PathBuf /*dir for tracks, file otherwise*/ }) -> ExportReport { files: Vec<PathBuf>, derived: bool, resampled_to: Option<u32>, note: Option<String> }`.
`tracks`: FLAC = byte copy of the stored file; WAV = streamed decode of the stored track to 24-bit PCM WAV at native rate (write header then patch sizes; stream, no whole-track Vec).
`stereo` (track 1 left, track 2 right; one track -> same signal on both) and `mixdown` (mean of tracks): resample to the highest native rate (or the rate if one), `derived = true`, `note` says "derived and resampled from X Hz" and for mixdown "tracks were summed". Refuse to overwrite an existing file (`publish_new`/create_new). Output never inside the recording folder.

### Test source seam (`recording/capture/synthetic.rs`)
`SyntheticSource`: a `CaptureSource` that delivers a sine in **real time** on its own thread (10 ms blocks stamped with `capture::clock::host_now_ns()`), format 48 kHz mono (configurable), device name "Synthetic microphone". Real code, no hardware. The CLI honours `SAGASCRIPT_TEST_SOURCE=synthetic[:<hz>]` ONLY when `cfg!(debug_assertions) || cfg!(feature = "dev-overrides")` (same pattern as `SAGASCRIPT_ENGINE_HOST`); in a release build the variable is ignored. The app does not read it.

### Error codes (command_errors.json rows + tests live in the app crate)
Existing `RecordingError::code()` values already include `recording_permission_denied`, `recording_source_unavailable`, `recording_disk_full`.
Still to add as core variants or app-level mappings: `recording_device_changed` (a track `ended_reason` device_changed/format_changed: surfaced via status, also an error code when `stop` finds the session ended early so the UI can say so)
and `recording_recovery_needed` (start refused because unrecovered recordings exist? NO: it is raised when the user opens/plays a `needs_recovery` recording; the UI offers recovery).
Also map the 1b codes `recording_io, recording_locked, recording_name_unavailable, recording_not_recording, recording_manifest_invalid, recording_manifest_too_new, recording_unsupported_format, recording_invalid_input, recording_unknown_track, recording_track_finished, recording_corrupt_track, recording_encoder, recording_session_failed` through one `From<RecordingError> for CommandError`.

## IPC (app crate, Tauri)

Commands (all `Result<T, CommandError>`; codes above):
- `start_recording_session(title: Option<String>) -> RecordingSessionInfo` (quality/folder/channel pair from settings; refuses when dictation is active or a session is already running; mic permission checked via `platform::mic_permission()` -> `recording_permission_denied`; MicSource::open on the blocking pool, `run_on_main_thread` where macOS needs it).
- `stop_recording_session() -> RecordingFinished`.
- `get_recording_session() -> RecordingSessionInfo`.
- `list_recordings() -> Vec<RecordingSummary>`; `delete_recording(folder_name)` moves to Trash/Recycle Bin (macOS NSFileManager `trashItemAtURL` via the existing `objc2-foundation`; Windows `SHFileOperationW` + `FOF_ALLOWUNDO` via the existing `windows-sys` with one more feature; no new crate); `reveal_recording(folder_name)`; `recover_recordings() -> Vec<RecoveryNote>`;
  `transcribe_recording(folder_name)` -> hands the recording to the existing file transcription path (use `decode_tracks` through a seam if one exists in `plain_file_jobs`/`files.rs`, otherwise pass the mic track path and report the deviation); `get_recording_folder_info(path: Option<String>) -> RecordingFolderInfo { path, exists, free_bytes: Option<u64>, warnings: Vec<String> /* fat32, exfat, network_volume, cloud_synced */, is_default }`;
  `set_recording_quality(quality)`, `set_recording_directory(path)`, `set_recording_channel_pair(first, second)`, `confirm_recording_folder()`, `acknowledge_recording_consent()`; all setters via `persist_and_sync` with rows in `persist_tests.rs`.
- Event `recording-session-changed` (payload = `RecordingSessionInfo`), emitted on every state change and every 250 ms while a session is Recording/Finalizing (poll thread, not the audio callback). Tray status shows a recording state while a session runs (the always-visible indicator), and the overlay shows "Recording" if the overlay setting is on.
- Recovery at app start: after setup, `recover_pending` on the configured directory in a background task; if anything was recovered, emit event `recordings-recovered` (payload `Vec<RecoveryNote>`) and keep the notes for `recover_recordings`.
- Tray menu item "Record..." opens window label `record` (URL `index.html?window=record`, ~460x640).

```ts
type RecordingSessionState = "idle" | "starting" | "recording" | "finalizing" | "done" | "failed";
interface RecordingSessionInfo {
  state: RecordingSessionState;
  recording_id: string | null; folder_name: string | null; title: string | null;
  quality: "lossless" | "compact" | null;
  elapsed_ms: number; level: number /* 0..1 peak, max over channels */; clipped: boolean;
  gap_count: number; gap_ms: number; stalled: boolean; device_lost: boolean;
  bytes_written: number; free_disk_bytes: number | null;
  warnings: string[];                 // "low_disk", "fat32", "exfat", "network_volume", "cloud_synced"
  ended_early: string | null;         // "device_changed" | "format_changed" | "system_sleep" | "disk_full" | ...
  error_code: string | null;
}
interface RecordingFinished { summary: RecordingSummary; ended_early: string | null }
interface RecordingSummary { folder_name; recording_id; title; state: "recording"|"complete"; recovered; started_at; quality; duration_seconds; bytes; track_count; gap_count; gap_seconds; ended_reason: string|null; needs_recovery; playable }
interface RecoveryNote { folder_name; title; outcome: "recovered"|"already_complete"|"skipped_locked"|"failed"; frames_kept: number; tail_bytes: number; message: string }
```
`events.ts`, `eventPayloads`, `command_errors.json`, `contracts/transport/*.json` fixtures regenerate with `SAGASCRIPT_BLESS_CONTRACTS=1 cargo test -p sagascript contract`.

## CLI

`sagascript record` keeps its meaning without the new flags; **recorder mode** is entered by `--quality`, `--dir`, `--name` or `--transcribe`
(these conflict with `-o`, `--clipboard`). New flags: `--quality lossless|compact`, `--dir DIR`, `--name TITLE`, `--transcribe`; `--duration`/`--json` already exist.
Mic only (no `--source`). Recorder mode: stderr banner (folder, quality, device, consent reminder), a status line every 10 s
(elapsed, level dB, gaps, free disk), Ctrl+C/SIGTERM/`--duration` stop and finish; stdout: folder path (plain) or one JSON object with `--json`
(`recording_id, folder, title, quality, duration_seconds, bytes, tracks[], gap_count, ended_early, recovered:false, transcript?`). Never silent.
`sagascript recordings list|show|export|recover|delete` (`--dir`, `--json`; `delete` needs `--yes`, removes the folder; `export --format flac|wav --layout tracks|stereo|mixdown -o PATH`).
`doctor` (or `permissions`) reports the microphone permission for recording. Integration tests spawn the binary with `SAGASCRIPT_TEST_SOURCE=synthetic`, `SAGASCRIPT_SETTINGS_PATH`, a temp `--dir`.
`--transcribe`: after saving, run the existing file transcription of the saved recording through `decode_tracks` (mic track) and print the text like `record` does.

## Frontend (Svelte 5 runes)
`src/lib/Record.svelte` (window `?window=record`), DOM-free state machine `src/lib/record-state.ts` (reducer over `RecordingSessionInfo` + user actions: idle/starting/recording/stopping/finished/error, first-use steps: folder confirm, consent reminder),
wrappers in `src/lib/api.ts`, events in `events.ts`; browser mock cases; Vitest unit tests for the state machine and component tests (docs/testing/frontend.md; no regex over `.svelte`).
UI: big start/stop, elapsed, level meter, quality toggle with estimated size/hour (lossless ~ 295 MB/h x channels at 48 kHz mono; compact ~ 46 MB/h), folder + change + warnings, red unmistakable indicator, gaps/errors, after stop: reveal, transcribe, move to Trash; recent list; recovery note; playback of the mic track (only `playable`) behind a clearly marked fallback seam `RECORDING_PLAYBACK_FALLBACK` (WebView FLAC playback unverified; owner checklist).
