# 0002. Privacy first: local by default, remote opt-in

- Status: Accepted (practice predates this record; recorded 2026-10-12)
- Related: [ADR 0001](0001-cli-first.md), [engine-host-protocol.md](../engine-host-protocol.md)

## Context

Dictation captures everything a user says, often in other people's documents. The product
promise is in README.md: "Audio and transcripts stay on your Mac. Sagascript connects to the
internet only when you choose a language that needs its speech engine, download another
model, or check for an update", and "no telemetry, cloud transcription, or transcript
upload". AGENTS.md: "local transcription is the default; remote/cloud behavior is opt-in";
CONTRIBUTING.md: "Remote/cloud features are always opt-in, never default."

Evidence in the code:

- In the Rust app and CLI, HTTP is confined to two files: `crates/sagascript-core/src/download.rs` (model
  downloads) and `src-tauri/src/updates.rs` (update checks, "deliberately only called from an
  explicit tray-menu action"), plus `tauri-plugin-updater` on the signed macOS update path.
  The test `normal_gui_launches_never_check_for_updates_implicitly` in `main_tests.rs` pins that.
- Engine hosts "never open network connections and never touch the microphone"
  ([engine-host-protocol.md](../engine-host-protocol.md)); release builds ignore
  `SAGASCRIPT_ENGINE_HOST` / `SAGASCRIPT_PIANISSIMO_MODEL_DIR` unless built with
  `dev-overrides`, so a signed build does not trust the environment for its sidecar or model.
- Transcripts reach disk only on request (comment on `save_transcription_text` in
  `src-tauri/src/commands/files.rs`: "privacy-first: transcripts reach disk only on explicit
  request"; [meeting-workflow.md](../meeting-workflow.md) says the same for meeting imports).
  An update the user starts keeps unsaved dictation as a local recovery draft
  (`prepare_update_recovery`). Diagnostics exclude content: `focus_diag.rs` logs only
  identifiers, pids and booleans; dictation latency logs carry "no audio, transcript text, or
  device names" ([configuration.md](../configuration.md)); the controller test
  `terminal_session_payload_is_consumed_once_without_transcript_contents` checks the log
  payload.
- File paths are handled case by case today, with no written rule: the engine-host client
  logs no paths because "the persistent log must not carry the user's home directory"
  (`start_and_handshake` in `engine_host/client.rs`), while settings warnings in
  `settings/store.rs` log the settings file path and the watcher start message in `main.rs`
  logs its directory.

## Decision

1. Transcription runs locally by default. No remote transcription, telemetry or analytics is
   on by default; any remote feature is an explicit opt-in setting that defaults to off, has a
   CLI key ([ADR 0001](0001-cli-first.md)) and is documented.
2. Network requests happen only as a direct result of a user action (download a model or
   engine, check for or install an update). New outbound code goes through the existing
   download or update paths or is called out in review.
3. Audio, transcripts and dictionary contents never go into logs, events that are logged, or
   error messages that are logged. Prefer counts, durations and booleans.
4. Data persists only where the user asked (settings, dictionaries, saved transcripts, update
   recovery drafts) and stays on the machine.
5. Tests and fixtures use synthetic data and never send it anywhere.

## Consequences

- Review checks every new `reqwest` use, URL constant or sidecar for an explicit trigger.
- Some features cost more (larger local models, local diarization) or are not offered.
- Error strings from native code can carry paths, for example absolute paths in Swift engine
  errors. `scrub_paths` removes them where an engine error enters Rust and where a command
  error is serialized ([ADR 0006](0006-typed-errors.md)); text from other sources is covered
  by the second place only.
