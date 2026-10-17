# Sagascript architecture

A map for agents and humans: what lives where, how the four main flows move through the
code, which threads and locks are involved, which feature flags exist, and where tests run.
It describes the code as it is; lines starting with **Planned:** link the issue that will
change a mechanism. Read [AGENTS.md](../AGENTS.md) first for the invariants and the
verification commands, [docs/recipes/](recipes/README.md) for step-by-step changes and
[docs/adr/](adr/README.md) for the reasons behind the rules.

## 1. Repository map

| Path | Responsibility |
|---|---|
| `src/` | Svelte 5 frontend (runes). `App.svelte` picks the view from the URL: `?overlay`, `?onboarding`, otherwise Settings. |
| `src/lib/api.ts`, `src/lib/meeting-reprocessing-api.ts` | The only `invoke` wrappers, plus hand-written TypeScript copies of Rust DTOs (`Settings`, `HotkeyProfile`, …). |
| `src/lib/ipc-contract.generated.ts`, `src/lib/events.ts` | The IPC surface: `CommandMap`, generated from the Rust command signatures, and `EventMap`, the payload of every event the frontend listens for ([contracts/README.md](../contracts/README.md)). |
| `src/lib/*.svelte` | `Settings.svelte` (Dictate / Transcribe / Settings tabs), `Onboarding.svelte`, `Overlay.svelte`, `FileTranscription.svelte`, `Meeting*.svelte`, `TranscriptionStages.svelte`. |
| `src/lib/*.ts`, `src/lib/*.js` | DOM-free logic (reducers, queues, update recovery), tested by Vitest next to the module and by the remaining `scripts/test-*.mjs`. |
| `src/lib/tauri-events.svelte.ts`, `models.svelte.ts`, `permissions.ts`, `platform.ts` | Shared by the windows. Tauri events are only listened to through `useTauriEvent`/`useDragDropEvent`/`subscribeTauriEvent` (they own the unlisten; ESLint forbids raw `listen()` in `src/`) and take their payload types from `EventMap` in `events.ts`; `models.svelte.ts` holds `PIANISSIMO_ID` and `downloadModelById`; `permissions.ts` the macOS Accessibility request and wait; `platform.ts` one `get_platform` lookup per window. |
| `src-tauri/src/` | App crate `sagascript`: CLI dispatch, Tauri shell, tray, hotkeys, paste, overlay, updater, Tauri commands. |
| `src-tauri/crates/sagascript-core/` | Library: audio decode/resample/capture, Whisper and Pianissimo transcription, engine-host client, model catalog (`models/`), settings store, model downloads, diarization, meeting documents, `fsutil` (publish a new file without ever replacing one, [ADR 0008](adr/0008-publishing-new-files.md)). |
| `src-tauri/crates/sagascript-cli/` | Library + headless `sagascript` binary: every clap subcommand. Also holds meeting/diarization pipelines that the app imports. |
| `src-tauri/crates/sagascript-engine-protocol/` | Wire types for the engine-host JSON Lines protocol ([spec](engine-host-protocol.md)); used by the core client, the ORT host and the fake host. |
| `src-tauri/crates/sagascript-testsupport/` | Dev-only test helpers shared by core, CLI and app tests: `TempDir` (unique, removed on drop), `fake_settings` (a settings file alone in its own `TempDir`), `sha256_hex`, `isolated_cli_env` (spawned CLI without inherited credentials, with `SAGASCRIPT_SETTINGS_PATH`). Never linked into a shipped binary. |
| `src-tauri/engine-host/coreml/` | Swift package: Core ML host `sagascript-engine-host` (Pianissimo on macOS 14+ Apple silicon). |
| `src-tauri/engine-host/ort/` | Standalone Cargo workspace (own `Cargo.lock`): ONNX Runtime CPU host `sagascript-engine-host-ort`, shipped for Windows on ARM. |
| `src-tauri/engine-host/test-vectors/` | Vectors shared by both hosts' unit tests: `protocol/` (the golden JSONL transcripts and the scripted `fake-engine.json` that define the wire contract, also decoded by `sagascript-engine-protocol`; [spec](engine-host-protocol.md#golden-transcripts)) and `context-biasing.json`. |
| `src-tauri/tauri.conf.json`, `src-tauri/capabilities/` | App config (`frontendDist: ../dist`) and plugin permissions. |
| Meeting modules | Core `meeting.rs`, `meeting_media.rs`, `meeting_review.rs`, `meeting_reprocess*.rs`; CLI `meeting.rs`, `meeting_proposal.rs`, `meeting_reprocessing*.rs` and `transcribe/meeting.rs`; app `meeting_jobs.rs`, `meeting_media*.rs`, `meeting_review_commands.rs`, `meeting_reprocessing_commands.rs`. Docs: `docs/meeting-*.md`. |
| Run context (#346) | Which profile, language and dictionary a file, recording or reprocessing run uses: core `run_context.rs` (`RunSelection` = profile and language, `RunContext` = those plus the dictionary; pure, no I/O; also `file_beam_size`, the beam default of a file run) and CLI `run_request.rs` (`RunRequest`: `--profile`, `--language`, `--prompt`, `--prompt-file` as typed; parses the language, reads the prompt file). `transcribe`, `record`, `meeting reprocess` and the app's file transcription (`commands/files.rs` `file_transcription_context`, which has no language override) all use it. The model is not resolved there: each caller chooses its own from the resolved language, which also fixes its order of errors (`tests/run_context_cli.rs`). |
| Transcribe command (#343) | CLI `transcribe/`: `mod.rs` plans a run (`run`, `run_inner`) and does one input (`transcribe_file`); `args.rs` (the command line), `inputs.rs` (directory expansion), `resolve.rs` (language and model IDs, which model a run gets, the one-run glossary), `vocab_correction.rs`, `language_detection.rs` (the language check; it asks for its detectors through `DetectorLoader`, naming an `EncoderPolicy`, and `InstalledDetectors::load` is the one place that touches the models directory, local files only today; a new policy is a deliberate change, but the types do not stop code in the loader from fetching, so review and `language_detection_never_downloads_an_encoder` have to) and `language_regions.rs`, `diarize.rs`, `meeting.rs` (entry points the app and reprocessing call), `pianissimo.rs`, `progress.rs` (`--progress-json`), `output.rs`. `transcribe/mod.rs` re-exports the paths other code imports. Tests: the sibling `transcribe_tests.rs`, and the whole command's observable contract (streams, exit status, error `code`, cache hits, cancellation) in `tests/transcribe_contract.rs`. |
| `scripts/` | Build/release/CI helpers (`*.sh`, `*.mjs`, `*.ps1`), Node tests `test-*.mjs`, Playwright suites `qa-*.mjs`, browser IPC mock `fixtures/transcription-browser-mock.js`, Python tooling `dictation_eval/` and `diarization_sv/`, Core ML conversion `pianissimo-coreml/`. |
| `tests/` | Code-health contract tests (`tests/scripts/`) and fixtures (`tests/fixtures/code-health/`). |
| `contracts/` | The Rust–TypeScript transport contract: shared payload fixtures in `transport/` that Rust writes and Vitest parses ([README](../contracts/README.md), #347). |
| `tooling/code-health/` | Pinned `knip`/TypeScript used by `.github/workflows/code-health.yml`. |
| `site/` | Separate marketing site (React, vinext, Cloudflare `wrangler`); no workflow builds or lints it. |
| `test-audio/` | Small audio fixtures for smoke tests and benchmarks. |

App crate modules (`src-tauri/src/`):

| Module | Role |
|---|---|
| `main.rs` | `main()`: CLI dispatch, Tauri builder and `.setup`, tray and update menu, signed updater, live dictation pipeline, settings watcher, window handling. |
| `commands/` | Most `#[tauri::command]`s, one file per domain (`settings`, `hotkey`, `state`, `transcribe`, `transcribe_file`, `files`, `models`, `dictionary`, `update_recovery`, `permissions`, `system`), re-exported by `mod.rs` so `generate_handler!` names `commands::name`. `mod.rs` also holds `SharedController` / `SharedWhisper` / `lock_controller`; `persist.rs` holds `persist_and_sync`, the only way a command writes settings (file and controller copy together); `main_thread.rs` waits for the main thread without blocking a worker. |
| `app_controller.rs` | `AppController`: dictation state (`AppState`), audio capture, in-memory `Settings` copy, session logging. |
| `events.rs` | Event catalogue: every event's name, payload type, producer and consumer (`events::event::*`); emit with `EmitEvent::emit_event`, since a raw `tauri::Emitter::emit` is a clippy error. |
| `contract/` | Tests only: the shared fixture helper and the `CommandMap` generator (`command_map.rs`, parses `generate_handler!` and the command signatures with `syn`; a registered `commands::name` is looked up in `commands/mod.rs` and the files below it). |
| `hotkey/` | Shortcut registration, `HotkeyHealth`, bare F-key monitor `macos_bare.rs`. |
| `paste/` | `PasteService` (clipboard + simulated paste: enigo on macOS/Windows, `xdotool` on Linux). `paste/completion.rs` (completion wait) is mounted from `main.rs` via `#[path]` as `crate::paste_completion`. |
| `platform/`, `overlay.rs`, `focus_diag.rs`, `logging/` | Per-OS helpers (`platform::mic_permission()`: the macOS microphone permission and its `unsafe` AVFoundation calls; `platform/macos.rs`), recording indicator window, focus diagnostics, app log file. |
| `plain_file_jobs.rs`, `meeting_*.rs` | File-transcription and meeting job slots, cancel, deadlines. |
| `updates.rs`, `update_activity.rs`, `update_recovery.rs` | Update checks, work/exclusivity accounting, recovery drafts. |

**Dependency direction:** `sagascript-engine-protocol` ← `sagascript-core` ← `sagascript-cli`
← app. The app uses the CLI crate for CLI dispatch (`sagascript_cli::try_parse`,
`sagascript_cli::run`) and for meeting pipelines (`sagascript_cli::transcribe`,
`sagascript_cli::meeting_reprocessing`, `sagascript_cli::meeting_proposal`).
**Planned:** [#374](https://github.com/Magnus-Gille/sagascript/issues/374) moves those
pipelines into core.

`sagascript-testsupport` is only a dev-dependency (of core, the CLI and the app) and depends
on none of them; fixtures made of core types (`meeting_transcript()`) live in
`sagascript_core::test_support` behind `cfg(test)` or core's `test-support` feature, which
the CLI and the app enable as a dev-dependency.

**Binaries:** the app crate and the CLI crate both build `sagascript`; a bare app invocation
starts the GUI, a subcommand runs the CLI and exits. The headless CLI prints help and exits 2
without a subcommand. `target/<profile>/sagascript` is whichever was built last. Core also
builds `sagascript-fake-engine-host` (`crates/sagascript-core/tests/support/fake_engine_host.rs`)
for tests.

**CLI output and exit:** both binaries call `sagascript_cli::run(cli) -> ExitCode` and return
it from `main`. `run_with(cli, &Out)` dispatches the subcommand and returns an `exit::Exit`
without exiting the process, so unit tests call it with `Out::capture()`. Every command writes
through `output::Out` (`outln!`/`errln!`, `Out::json`/`jsonl`; the crate denies
`println!`/`eprintln!`): a closed stdout pipe ends the run quietly with exit 0, and
`exit::conclude` maps the result to the exit status and, for a `--json`/`--jsonl` run that
failed before writing anything, writes one `{"status":"error","code","error"}` object on
stdout. `transcribe`'s three output modes live in one `BatchSink` (`transcribe/output.rs`). The statuses are listed in
[add-cli-subcommand.md](recipes/add-cli-subcommand.md#4-exit-codes).

**Build facts that bite:**

- Bare `cargo build/test/clippy` in `src-tauri/` covers only the app (`default-members = ["."]`);
  use `--workspace` or `-p`.
- `tauri::generate_context!()` needs `../dist`. Run `npm run build` before any cargo command
  that compiles the app crate (check, test, clippy), or it fails at compile time;
  `npm run verify` reports a missing `dist/` before running those checks.
- `swift test` needs `Sources/EngineHostCore/BuildInfo.swift`, which is gitignored and written
  by `scripts/build-engine-host.sh` (CI runs `scripts/stage-engine-host.sh` first;
  `npm run verify -- --only swift` runs it before `swift test`).
- `rust-toolchain.toml` pins the development compiler; `rust-version` in `src-tauri/Cargo.toml`
  is the minimum supported version (MSRV), checked by CI's `msrv-linux` job.
- Build identity comes from `src-tauri/build.rs` (`GIT_HASH`, `BUILD_DATE`, overridable with
  `SAGASCRIPT_GIT_HASH` / `SAGASCRIPT_BUILD_DATE`) and feeds the tray `BUILD_IDENTITY`, the
  `get_build_info` command and the engine-host handshake.

### 1.1 Layers inside `sagascript-core`

The layers below are *textual*: the imports of `models/`, `glossary.rs` and `settings/` point
downward, with one exception (below), and `crates/sagascript-core/tests/module_layering.rs`
checks that from the source text (every path the code names, `use` groups and renames
expanded) for the rules marked "tested". The rest is convention. At run time one edge still
closes a loop, through a compatibility shim.

| Layer | Modules | Rule |
|---|---|---|
| Leaves | `models/`, `glossary.rs` | `models/` is the model catalog: `WhisperModel` (21 models, ids via `id()` / `from_id()` / `FromStr`, `ALL`), its download URLs and SHA-256 in `sources.rs` (pinned revisions; `scripts/generate-third-party-notices.mjs` only asserts that six reviewed URL prefixes still appear in the model sources, `sources.rs` among them), `FileModel` (a Whisper model or Pianissimo) and `Language`. Imports neither `settings` nor `transcription` (tested) and returns no `whisper_rs` type: `transcription::whisper_backend` maps a model to its DTW preset. `glossary.rs` (personal dictionary parser) imports nothing from the crate (tested). |
| Settings | `settings/` | Reads the catalog. Imports nothing from `transcription` (tested): what is downloaded and whether Pianissimo can run arrive as arguments, as `LineupEnv` / `MachineProbe` into the store and as the `pianissimo_supported` and readiness arguments of the `*_gated` methods of `Settings`. |
| Runtime | `transcription/` (Whisper, Pianissimo, engine host, model downloads), `audio/`, `download.rs`, `meeting*.rs`, `diarization*` | `transcription` reads `models` and, in `pianissimo_backend` alone, `settings` (the engine config). `transcription::model` and `pianissimo_model` answer whether a model is on disk; `pianissimo_backend::runtime_supported_on_this_os` answers whether Pianissimo runs. |
| Machine | `machine.rs` | The only place that feeds the runtime's answers to settings: `ThisMachine` (a `MachineProbe`), `load` / `update` / `try_update`, and the `Settings` methods without a `_gated` suffix (`dictation_model_for_profile`, `effective_file_model_for`, `warm_model_plan`, ...), each its `_gated` sibling plus this machine's probe. |

Old paths stay for one release (#372): `settings::{WhisperModel, FileModel, Language}`
re-export `models`, `transcription::{glossary, Glossary, ...}` re-export `glossary`, and
`settings::store::{load, update, try_update}` re-export `machine`.

**The one edge that goes up, and the loop behind it.** The last re-export is a line in
`settings/store.rs` that names `machine`, which imports transcription: textually the only
upward import in `settings/` (tested; the test lists the three paths it allows). At run time
it closes a loop: `transcription::pianissimo_backend::engine_config` and `shared_client`
call `settings::store::load()`, which is that shim, so transcription calls `machine::load()`,
which calls transcription's probes. It is harmless today (the probes do not call back into
settings) but it is not a layering. Removing it takes, in order: (1) the CLI and the app call
`machine::{load, update, try_update}`; (2) `engine_config` and `shared_client` receive the
`Settings` they read instead of loading them, which changes *when* the settings are read, so
it is a behaviour change of its own and not part of this move; (3) delete the re-export line
and empty the expected list in the test.

**Planned:** the rest of [#372](https://github.com/Magnus-Gille/sagascript/issues/372):
`meeting/`, an always compiled `diarization/`, `pub(crate)` narrowing and a `testing` module.

## 2. Data flows

### 2.1 Dictation: hotkey → record → transcribe → paste

```mermaid
sequenceDiagram
    autonumber
    participant OS as OS hotkey
    participant HK as Hotkey callback
    participant Main as Main thread
    participant Ctrl as AppController mutex
    participant Task as Async task
    participant Work as Blocking worker
    participant UI as Webviews and tray
    Note over OS,HK: callback thread depends on the platform, see below
    OS->>HK: key down (global-shortcut handler or macos_bare monitor)
    HK->>Ctrl: handle_hotkey_down_for_binding starts audio capture
    HK->>UI: emit state-changed recording, overlay::show, tray status
    OS->>HK: key up (push-to-talk) or second press (toggle)
    HK->>Task: stop_recording_and_transcribe spawns a task
    Task->>Ctrl: stop_recording_generation returns 16 kHz samples
    Task->>Main: dispatch_to_main updates tray to transcribing
    Task->>Work: spawn_blocking LiveDictationBackend::transcribe
    Note over Task,Work: 60 s timeout, then request_abort and a 5 s grace
    Work-->>Task: text or DictationError
    Task->>Task: apply_glossary
    opt auto_paste enabled
        Task->>Main: run_on_main_thread paste closure (macOS)
        Main->>Main: PasteService::paste_checked uses enigo
        Main-->>Task: result over a oneshot, paste_completion::wait
    end
    Task->>Ctrl: on_transcription_success or on_transcription_error
    Task->>UI: emit transcription-result and state-changed idle
    Task->>Main: dispatch_to_main hides overlay, updates tray
```

- Code: `handle_hotkey_event`, `handle_hotkey_release`, `stop_recording_and_transcribe` and
  `restore_target_before_paste` in `src-tauri/src/main.rs`; capture in `AppController`
  (`app_controller.rs`) over `sagascript_core::audio::AudioCaptureService`; engine choice in
  `crates/sagascript-core/src/transcription/live_dictation.rs` (Whisper via
  `whisper_backend.rs`, Pianissimo via `pianissimo_backend.rs` and the engine host, 2.4).
- Callback thread: `tauri-plugin-global-shortcut` calls `handle_hotkey_event` inline from
  `global-hotkey`. On macOS that is the main thread (Carbon handler; the `macos_bare` NSEvent
  monitors too); on Windows key-down arrives on the main thread and key-up on a spawned
  `GetAsyncKeyState` polling thread; on Linux (X11) both arrive on the library's event thread,
  so the direct `overlay::show`, `select_profile_menu` and `update_tray_status` calls there
  break [ADR 0003](adr/0003-main-thread-native-apis.md) (known violation).
- Main-thread hops: the handler must not block (the 300 ms `MIN_RECORDING_MS` top-up is a
  `tokio::time::sleep` inside the task); tray and overlay updates from the task go through
  `dispatch_to_main`; on macOS `restore_target_before_paste` waits on the blocking pool and
  sends each AppKit call to the main thread with a 500 ms timeout, then the paste closure runs
  via `run_on_main_thread`. On Windows and Linux the paste closure runs on `spawn_blocking`.
- The Dictate tab's test recording (`commands::stop_and_transcribe`) repeats the
  timeout/abort transaction with its own `TRANSCRIPTION_TIMEOUT_SECS` / `ABORT_GRACE_SECS`.
  **Planned:** [#344](https://github.com/Magnus-Gille/sagascript/issues/344) makes the
  lifecycle testable outside Tauri.

### 2.2 Settings layers: disk ↔ store ↔ AppController ↔ watcher ↔ frontend

```mermaid
sequenceDiagram
    autonumber
    participant UI as Settings.svelte
    participant Cmd as Tauri setter command
    participant Store as settings store
    participant Disk as settings file
    participant Ctrl as AppController
    participant CLI as sagascript config
    participant W as Settings watcher thread
    UI->>Cmd: api.ts wrapper invokes set_x
    Cmd->>Store: persist_and_sync (one writer at a time) calls store::try_update
    Store->>Disk: lock json.lock, load and migrate, mutate, atomic save_to
    Store-->>Cmd: persisted Settings
    Cmd->>Ctrl: lock, the same change on the controller's copy (a few setters replace the whole copy)
    Cmd-->>UI: Ok, then applySetting calls get_settings
    CLI->>Store: config set uses store::try_update
    Store->>Disk: same lock and atomic write
    Disk-->>W: file event (polled every 1 s on macOS)
    W->>Store: load_settings_with_permission_gate
    W->>W: re-register hotkeys if bindings changed
    W->>Ctrl: update_settings replaces the whole copy
    W->>UI: emit state-changed settings_reloaded
    UI->>Cmd: get_settings
```

| Layer | Where | Notes |
|---|---|---|
| Disk | `${XDG_CONFIG_HOME:-~/.config}/sagascript/sagascript-settings.json`, `glossary.txt`, `glossaries/*.txt` | See [configuration.md](configuration.md). `SAGASCRIPT_SETTINGS_PATH` selects an exact file and disables legacy migration (tests, isolated CLI runs). |
| Model | `Settings` in `crates/sagascript-core/src/settings/manager.rs` | `#[serde(default)]` on the struct: missing keys take `Default` values. The model types it names (`WhisperModel`, `FileModel`, `Language`) live in `models/` (section 1.1). |
| Store | `crates/sagascript-core/src/settings/store.rs` | `load_with`, `load_from`, `save`, `update_with`, `try_update_with` take the machine state as a `MachineProbe`; `machine::{load, update, try_update}` are them with this machine's probes (`store::{load, update, try_update}` re-export those for one release); cross-process `fs2` lock on `<file>.json.lock`; read-merge-write keeps unknown keys; atomic rename. |
| AppController | `app_controller.rs` | In-memory copy: `settings()`, `update_settings()` (`settings_mut()` in tests only). Dictation reads it at use time. It can differ from the file on purpose: the watcher keeps the registered shortcuts after a failed reload, and a host that cannot run Pianissimo demotes a selection of it in this copy only. |
| Watcher | `start_settings_watcher` in `main.rs` | `PollWatcher` on macOS, `RecommendedWatcher` elsewhere; filter `configuration_event_may_affect`; waits while a dictation records (`acquire_hotkey_configuration`). Fires for the app's own writes too (harmless reload). |
| Frontend | `getSettings()` / setters in `src/lib/api.ts`; `saveSetting()`, `applySetting()` and the `settings_reloaded` listener in `Settings.svelte` | Re-reads after a write. A rejected setter saved nothing; a saved change whose re-read fails is reported as saved, never repeated and never rolled back: the window shows a Reload settings notice, disables the controls that save a setting and sends no further change until a read succeeds. Reads are settled in the order they were issued, so a late answer, success or failure, cannot replace or hide a newer one ([#341](https://github.com/Magnus-Gille/sagascript/issues/341)). |

Rules: write through `store::update`/`try_update` (never `save()` a possibly stale clone);
release the file lock before taking the controller lock; the watcher replays only hotkey
registration, Pianissimo warm/unload and the profiles menu, so side effects that live in a
setter (for example `overlay::on_show_overlay_changed`, the VAD model download in
`set_vad_enabled`) do not run for CLI edits. A value that does not read resets only its field,
with a warning, and a save over such a file first preserves its bytes as `.bak`. Migrations are
the ordered steps in `settings/migrations.rs`, recorded as `schema_version` plus the legacy
markers, and the copy from legacy locations ([ADR 0007](adr/0007-settings-file-evolution.md)).
**Planned:** [#373](https://github.com/Magnus-Gille/sagascript/issues/373) (one implementation
per CLI/app operation).

### 2.3 Updater phases

```mermaid
sequenceDiagram
    autonumber
    actor User
    participant Tray as Tray menu
    participant Upd as Update task
    participant Act as UpdateActivity
    participant Set as Settings window
    User->>Tray: Check for Updates, Install and Restart, or sagascript update
    Tray->>Upd: check_for_updates_with_intent
    alt macOS build with SAGASCRIPT_UPDATER_PUBKEY
        Upd->>Upd: updates::check_signed_update (signed latest.json)
    else other builds
        Upd->>Upd: updates::check_for_update (GitHub releases API)
        Upd->>Tray: show result, release page opens on click
    end
    opt install requested and update available (macOS signed path)
        Upd->>Upd: update.download verifies the signature
        loop every 250 ms for at most 600 s
            Upd->>Act: try_exclusive waits for active work
        end
        Upd->>Set: emit_to settings update-preparing with nonce
        Set->>Upd: save_update_recovery, then complete_update_preparation
        Note over Upd,Set: without a Settings window the pending dictation goes to update-recovery.json
        Upd->>Upd: update.install
        Upd->>Tray: dispatch_to_main runs app.restart
    end
```

- Code: `check_for_updates_with_intent`, `install_signed_update`, `prepare_update_recovery`
  in `main.rs`; `updates.rs`; `UpdateActivity` / `UpdatePreparation` in `update_activity.rs`;
  `update_recovery.rs`. Checks are never implicit (test
  `normal_gui_launches_never_check_for_updates_implicitly`). `sagascript update` (macOS)
  opens the app with `GUI_UPDATE_ARG` (`--install-update`), handled by
  `second_instance_requests_update` / `should_install_update_at_launch`.
- A failure in any phase resets the menu to "Try Again…"; failures during preparation or
  install also emit `update-aborted` to Settings. The `updater-smoke-test` feature points the
  manifest at a loopback URL (section 4).
- **Planned:** [#376](https://github.com/Magnus-Gille/sagascript/issues/376) puts the
  orchestration behind traits; [#369](https://github.com/Magnus-Gille/sagascript/issues/369)
  moves it out of `main.rs`.

### 2.4 Engine-host process lifecycle

```mermaid
sequenceDiagram
    autonumber
    participant Caller as Dictation, file job or CLI
    participant PB as pianissimo_backend
    participant Client as EngineHostClient
    participant Host as Engine host process
    Caller->>PB: shared_client()
    PB->>PB: precheck platform, host path and model, verify the model once
    PB-->>Caller: process-global client, host not spawned yet
    Caller->>Client: warm or submit_window
    Client->>Host: spawn with --protocol 1, then hello (10 s)
    Host-->>Client: protocol, build identity, capabilities
    Client->>Host: load model_dir (180 s)
    Host-->>Client: LoadResult with effective window
    Client->>Host: transcribe_window, priority interactive or batch
    Host-->>Client: tokens, merged in window order
    Note over Client,Host: supervisor thread unloads after idle_unload, shuts down after idle_shutdown
    Host--xClient: crash or EOF
    Client->>Host: respawn with backoff, more than max_restarts in restart_window marks Failed for failed_cooldown
    Caller->>PB: shutdown_shared_client at app exit or CLI end
    PB->>Host: shutdown, 2 s grace, then kill
```

- Code: `crates/sagascript-core/src/transcription/engine_host/` (`process.rs` transport,
  `client.rs` handshake/lifecycle/restart budget, `pipeline.rs` long-audio windows) and
  `pianissimo_backend.rs` (`resolve_host`, `runtime_supported_on_this_os`, `config_for_host`,
  `shared_client`, `warm_in_background`, `shutdown_shared_client`). The client API is
  blocking; each host has a stdout reader and a stderr drain thread, and a client gets a
  supervisor thread only when an idle policy is set.
- Settings: `engine_idle_unload_minutes` → `idle_policy` (unload after N minutes, exit after
  2N, 0 = never); `engine_prewarm` (`off`, `on_app_start`, `on_key_down`). Hosts are
  resolved from the bundle (`Resources/EngineHost/sagascript-engine-host`,
  `engine-host\sagascript-engine-host-ort.exe`); `SAGASCRIPT_ENGINE_HOST` only in development
  builds (`dev-overrides`).
- Recovery ([#366](https://github.com/Magnus-Gille/sagascript/issues/366)): `Failed` lasts
  `failed_cooldown` (60 s), then one fresh start (half-open); a window that times out returns at
  once, and a detached `engine-host-wedge-watch` thread kills the host if it sends no terminal
  response within `wedge_grace` (30 s) after `cancel`; stdin writes are bounded
  (`Timeouts::write`); the restart backoff is slept unlocked and is cancel-aware.
  `EngineHostClient::reset()` clears `Failed` at once but has no production caller: settings that
  change the engine (host, model, compute units, idle policy) already replace the shared client
  (`shared_client`), model deletion shuts it down, and the CLI `engine doctor` uses its own client.
- PCM temp dirs hold a locked `owner.lock`; `pipeline::sweep_stale_pcm_dirs` (app startup and
  shared-client creation, background, once per process; `engine doctor` on demand) removes the
  dirs of dead owners, and dirs whose owner cannot be proven dead (pre-#366, or on a file system
  that cannot lock) once untouched for 24 h.
- **Planned:** [#367](https://github.com/Magnus-Gille/sagascript/issues/367) (machine-checked
  protocol).

## 3. Threads and locks

| Thread | What runs there |
|---|---|
| Main (UI) thread | Tauri setup, tray/window/overlay APIs, global-shortcut callbacks on macOS (and key-down on Windows), `macos_bare` callbacks, `run_on_main_thread` closures (macOS paste, AppKit focus calls), sync (non-`async`) Tauri commands. |
| Tokio runtime | `async` Tauri commands, tasks from `tauri::async_runtime::spawn` (dictation stop pipeline, update flow). |
| Blocking pool | `spawn_blocking`: inference, paste on Windows/Linux, focus-restore wait, recovery file I/O, and the file work of model downloads (`download::run_blocking`: directory preparation, hashing existing artifacts, fsync, rename and directory sync on install, Pianissimo staging and promotion). |
| Dedicated threads | Settings watcher, startup model warm-up, engine-host reader/stderr/stdin-writer/supervisor/wedge-watch threads and the PCM temp dir sweep, audio capture worker and cpal callback, `global-hotkey` event thread (Linux) and key-up polling thread (Windows). |
| CLI | `sagascript_cli::run` builds its own Tokio runtime; inference commands run on a worker thread (16 MiB stack on Windows, `run_inference_command`). |

| Lock | Where | Notes |
|---|---|---|
| `SharedController = Mutex<AppController>` | `commands/mod.rs`, managed in `main()` | Taken from commands, the hotkey handler, the watcher, the updater and meeting jobs, always via `lock_controller`. Audio start/stop and log writes happen under it. Never hold it across `.await` or inference; `SharedWhisper` lives outside it for that reason. |
| `<settings>.json.lock` | `with_settings_lock` in `settings/store.rs` | Exclusive file lock shared by app and CLI processes, held for read-merge-write only. |
| `HotkeyHealth { state, transition }` | `hotkey/health.rs` | `transition` serializes OS (un)registration between commands and the watcher; kept out of the controller on purpose (see its doc comment). |
| `UpdateActivity`, `UpdatePreparation` | `update_activity.rs` | Work leases and the exclusive update permit. |
| Tray state (`SharedUpdateMenuState`, `SharedStatusItem`, `SharedProfileMenuState`) | `main.rs` | Short critical sections. |
| Job slots (`MeetingJobs`, `SharedMeetingAudio`, `PlainFileJobs` fields) | `meeting_jobs.rs`, `meeting_media.rs`, `plain_file_jobs.rs` | One active job each. |
| `LoggingService` (`log_file`, `dictation_session_id`) | `logging/service.rs` | Taken inside controller methods. |
| `WhisperBackend` (`load_lock`, `state`, `context`, `loaded_runtime`, `cached_runtime`) | `transcription/whisper_backend.rs` | `load_lock` and `state` are taken with a bounded wait (`lock_load_bounded`, `lock_state_bounded`, `WARM_STATE_GRACE`), returning `ModelBusy` instead of blocking. |
| Engine host (`lifecycle`, `state`, `boost`; process `routing`, `stdin`, `child`) | `engine_host/client.rs`, `process.rs` | `lifecycle` serializes start/load/unload/shutdown; it is released while a restart backoff is slept. `stdin` only guards the writer thread's queue, never a write. |
| Pianissimo `shared_slot` and load-state statics | `pianissimo_backend.rs` | Process-global client. |
| Others | `AudioCaptureService` buffers (`audio/capture.rs`), `GLOBAL_DICTATION` (`logging/service.rs`), the download `verification_cache` and `download.http_clients` client cache (`download.rs`), `LiveDictationBackend.active_job`, `PianissimoBackend.active`, `HostProcess` `stderr_tail`/`reader`, the CLI `Canceller` (`cancel.rs`) | Leaf locks. The capture buffers are taken under `SharedController` by start/stop and without it by the capture worker and cpal callback. |

Observed acquisition order (outer → inner). Nothing enforces it; keep new code consistent:

1. `HotkeyHealth.transition` is held for a whole hotkey transaction (`apply_hotkey_change`,
   settings watcher). Inside it the controller lock and the settings file lock are each taken
   and released in turn, never nested. The configuration lease (`acquire_hotkey_configuration`)
   is a flag, not a lock: `apply_hotkey_change` takes `transition` then the lease, the watcher
   the lease then `transition`. No deadlock (the lease is try-acquire), but a GUI hotkey change
   that races a watcher reload fails with "Finish or cancel the active dictation…" while idle.
2. Setters take the settings file lock first, release it, then take the controller lock.
3. `MeetingJobs` slot → `SharedController` (`begin_meeting_file`).
4. `SharedController` → `UpdateActivity` state (work leases dropped in controller methods)
   and → `LoggingService` locks.
   - (4b) `SharedController` → `HotkeyHealth.state`: `handle_hotkey_event` reads
     `health.operational_hotkey()` under the controller guard; no reverse order exists.
5. `PlainFileJobs.active` → `pending_cancels`.
6. Engine host `lifecycle` → `state`.
7. Whisper `load_lock` → `state` → `context` → `loaded_runtime` → `cached_runtime` (comment in
   `WhisperBackend::load_model_inner`). Both outer locks are try-locks with a grace period, so
   an inversion surfaces as `ModelBusy`, not as a deadlock.

**Poison policy ([ADR 0005](adr/0005-mutex-poison-policy.md)):** `[profile.release]` deliberately
does not set `panic = "abort"`, so a panic while a lock is held poisons it. Locks recover instead
of propagating: `sagascript_core::sync::lock_or_recover` (and `commands::lock_controller` for
`SharedController`) take the data, clear the poison flag and log `error!` with the lock name and
the last panic message (`install_panic_recorder`). Where a panic can leave state stale, recovery
restores a safe state first (`lock_or_recover_with`): the engine-host client forgets `loaded` so
the next request re-validates the host, and the whisper warm state is discarded. The whisper
`load_lock` deliberately still fails closed with its restart message; Whisper dictation holds it
across inference, so a panic mid-inference leaves that backend unavailable until restart. Every
`SharedController` lock, including those in `meeting_jobs.rs`,
`meeting_reprocessing_commands.rs` and the pre-warm `try_lock`, goes through `lock_controller`
(or recovers `TryLockError::Poisoned`). `Drop` impls always use the tolerant helpers. The
settings watcher runs each event under `catch_unwind`; its health is
`HotkeyStatus.settings_watcher`. The meeting-job slot and the meeting audio attachment
(`meeting_media.rs`) are separate mutexes that still fail closed with an error message.

## 4. Feature flags

| Crate | Feature | Default | Gates |
|---|---|---|---|
| app `sagascript` | `custom-protocol` | on | `tauri/custom-protocol` (serve the bundled frontend). |
| app | `diarization` | on | Forwards to core and CLI; without it the meeting import commands return "This build has no speaker diarization support." |
| app | `updater-smoke-test` | off | `SIGNED_UPDATE_MANIFEST_URL` → `http://127.0.0.1:34827/latest.json`; enabled only by `.github/workflows/test-build.yml`. |
| app, core, CLI | `dev-overrides` | off | Release builds honor `SAGASCRIPT_ENGINE_HOST` / `SAGASCRIPT_PIANISSIMO_MODEL_DIR` (`transcription/dev_overrides.rs`; debug builds always do). |
| core | `record` | off | `audio::capture` / `AudioCaptureService` (cpal; ALSA on Linux). The app always enables it. |
| core | `diarization` | off | `ort`, `rustfft`, `kodama`, `ndarray`; the `diarization` module; the `diarize_eval` example; DTW word timestamps in `whisper_backend.rs`. |
| CLI | `record` | on | The `record` subcommand. |
| CLI | `diarization` | on | `--diarize`, `meeting_reprocessing*`, `speaker_args`, `diarization_cache`. |
| ORT host | `dev-overrides` | off | Release builds honor `ORT_DYLIB_PATH`. |

Feature unification: `cargo test --workspace` builds core with `record` and `diarization`
(enabled by the app and CLI). CI therefore also runs core and CLI per package to keep the
no-feature builds honest, and asserts that `cargo tree -p sagascript-cli --no-default-features`
contains no cpal/ALSA.

## 5. Where tests live and how CI runs them

Shared fixtures: `sagascript_testsupport` (`TempDir`, `fake_settings`, `sha256_hex`, `isolated_cli_env`) and
`sagascript_core::test_support::meeting_transcript()`; use them instead of hand-written
`std::env::temp_dir()` paths, cleanup guards or env-scrub loops.

| Layer | Location | Local command | CI |
|---|---|---|---|
| Rust unit, app | sibling `foo_tests.rs` files, declared last in `foo.rs` as `#[cfg(test)] #[path = "foo_tests.rs"] mod tests;` (`commands/settings_tests.rs` and the other `commands/*_tests.rs`, `main_tests.rs`, `meeting_jobs_tests.rs`, `paste/windows_clipboard_tests.rs`); inline `#[cfg(test)]` modules in smaller files | `cargo test` (app only), `cargo test --workspace` (everything) | every OS lane (`cargo test --workspace`) |
| Rust unit, core/CLI/protocol | inline, or sibling `foo_tests.rs` as in the app: core `download_tests.rs`, `settings/manager_tests.rs`, `settings/store_tests.rs`, `transcription/whisper_backend_tests.rs`; CLI `transcribe_tests.rs`, `meeting_reprocessing_tests.rs`; `sagascript-engine-protocol/src/tests.rs` | `cargo test -p <crate>`; release-only guard: `cargo test --release -p sagascript-core --lib dev_overrides` | `cargo test --workspace` and the per-crate feature-subset gates on every OS; the release guard in test-linux |
| Rust integration | `sagascript-core/tests/*.rs` (engine host against the fake host, chunk-merge golden, Pianissimo adapter); `sagascript-cli/tests/*.rs` (spawn `CARGO_BIN_EXE_sagascript`) | `cargo test -p sagascript-core`, `-p sagascript-cli` | macOS, Linux, Windows |
| ORT host | `src-tauri/engine-host/ort/src/**` | `cargo test` and `cargo clippy --all-targets -- -D warnings` in `engine-host/ort/` | test-linux (fmt, test, clippy); test-windows (test, clippy) |
| Engine-host protocol | golden transcripts `src-tauri/engine-host/test-vectors/protocol/*.jsonl` with the scripted `fake-engine.json` ([contract](engine-host-protocol.md#golden-transcripts)); replayed by `sagascript-engine-protocol/src/vector_tests.rs`, `engine-host/ort/src/server/golden_tests.rs` and `ProtocolServerTests.swift` | `cargo test -p sagascript-engine-protocol`; `cargo test --locked` in `engine-host/ort/`; `swift test` | `cargo test --workspace` in every OS lane (protocol crate); test-linux, test-windows (ORT host); test-macos (Swift) |
| MSRV | whole workspace, lean CLI, ORT host | `RUSTUP_TOOLCHAIN=1.88 cargo check --workspace --locked` (see CONTRIBUTING.md) | msrv-linux |
| Swift | `src-tauri/engine-host/coreml/Tests/EngineHostCoreTests/` | `scripts/build-engine-host.sh`, then `swift test` (`npm run verify -- --only swift`) | test-macos |
| Frontend unit and component (Vitest) | `src/**/*.test.ts` next to the module or component, in happy-dom; typed IPC mock, setup and component backends in `src/test/` ([docs/testing/frontend.md](testing/frontend.md)) | `npm run test:unit` (`test:unit:coverage` for a report) | test-macos, test-windows; release-quality-gate, windows-package |
| Node | `test-*.mjs` and `test_*.mjs` anywhere under `scripts/` except `fixtures/` (`node:test`), each in one group of `scripts/node-test-groups.mjs`: `ui` (component source-text tests until #361 replaces them), `ci` (workflows, tooling), `release`, `codeHealth` | `npm run test:frontend` (all), `npm run test:ui` / `test:ci` / `test:release` | test-macos, test-windows |
| Code health | `scripts/test-code-health-*.mjs`, `tests/scripts/*.mjs` | `npm run test:code-health` | `code-health.yml`; the `scripts/` part also in test-macos, test-windows |
| Browser | `scripts/qa-*.mjs` with the mocked IPC in `scripts/fixtures/transcription-browser-mock.js` | `npx playwright install chromium && npm run test:browser` | test-macos |
| Python | `scripts/dictation_eval/test_*.py`, `scripts/diarization_sv/test_*.py` | `python -m unittest discover -s scripts/dictation_eval -p 'test_*.py'` | all OS (dictation_eval); Linux and macOS (diarization_sv) |
| Types | Svelte/TS, including the Vitest files | `npx svelte-check --tsconfig ./tsconfig.json` | test-macos |
| Transport contract | Rust: `transport_contract*` tests in `events_tests.rs`, `commands/settings_tests.rs`, `meeting_jobs_tests.rs` and `contract/` write or compare `contracts/transport/*.json` and `src/lib/ipc-contract.generated.ts`; TS: `src/test/transport.test.ts`, `src/lib/api.test.ts` ([contracts/README.md](../contracts/README.md)) | `cargo test contract` in `src-tauri/` (`SAGASCRIPT_BLESS_CONTRACTS=1` regenerates), `npm run test:unit`, svelte-check | with the app tests and the frontend suites |
| Formatting | Prettier over `src/`, `scripts/`, `tests/` (not `.svelte` yet; `.prettierignore`) | `npm run format:check` (`npm run format` fixes) | test-macos, test-windows |
| Lint | ESLint over `src/` (TypeScript, Svelte), `scripts/`, `tests/` (`eslint.config.js`, baseline in `eslint-suppressions.json`) | `npm run lint` | test-macos |
| CLI smoke | release binary in `ci.yml` build jobs, `scripts/ci/smoke-cli.mjs` (one script for every OS; `basic` tier without a model: help, `config`, `list-models`, `formats`, completions of every shell; `model` tier: download + transcribe, `--json`, `--jsonl`, a closed pipe and the error object through `scripts/verify-json-cli-streams.py`) | `node scripts/ci/smoke-cli.mjs basic --binary <path>` | build-macos, build-windows (Windows tolerates the `model` tier, #357) |
| Conformance | `scripts/engine-host-conformance.py <host> --no-model` against a real host process; with `--model-dir <dir>` the full run | model-free: `npm run verify -- --only engine-host-conformance` (macOS); full run by hand with a downloaded model | model-free subset: the staged Core ML host in test-macos and the signed host in `release-build-macos.yml`; full run (release gate, each time right after a Pianissimo smoke): `release-build-macos.yml` against the host in the installed signed app (prebuild and fallback paths), `scripts/verify-release-draft.sh` against the host in the downloaded draft (promote path in CI, fallback path by hand) and the manual `test-build.yml` ([spec](engine-host-protocol.md#conformance)) |

CI (`.github/workflows/ci.yml`) has a test lane per OS (test-macos, test-linux, test-windows), a native
build lane with the CLI smoke tests on macOS and Windows, an MSRV lane on Linux, and the `check-*`
aggregators that branch protection reads (`check-macos` and `check-windows` are required). The parts
every lane shares live once, outside the workflow (#358): `.github/actions/setup` (Python, the pinned
Rust toolchain, Node.js, the Rust cache, `npm ci`), `scripts/ci/cargo-gates.mjs` (the one list of cargo
commands, run by every lane with `--os`; the MSRV lane runs its `msrv` profile), `scripts/ci/smoke-cli.mjs`
and `scripts/ci/assert-lanes.mjs` (what an aggregator asserts about the lanes it needs). The same cargo
commands run on every OS except the gates marked `os:` in the list, each with its reason: formatting,
`cargo deny`, the core feature subsets (`diarization`, `record`), the lean-CLI build for the Python
suites, the ALSA tree check and the release-profile guard run on Linux only, the ORT host gates on
Linux and Windows. Everything else is `cargo clippy` and `cargo test` over the workspace, over core
and over the lean CLI (`--no-default-features`). Besides the cargo gates test-linux runs the clippy policy
counts (`scripts/clippy-policy.mjs`, which must not rise) and the offline diarization evaluation;
msrv-linux runs `cargo check --workspace --locked`, the lean CLI and the ORT host on the `rust-version`
toolchain (1.88), and check-linux requires it; test-macos runs `npm audit --audit-level=high`, the
frontend suites (Prettier, ESLint, Vitest, Node), Svelte types, the Swift tests and the engine-host
conformance subset; test-windows runs the same frontend suites except ESLint and the Windows build
identity gates. test-windows owns the Windows tests of every pull request:
`windows-package.yml` (on PRs touching `src/`, `src-tauri/`, `scripts/`, package files,
`.gitattributes` or the workflow itself) repeats `cargo test --workspace` and clippy only for
releases (`prebuild-release.yml`) and manual runs, on both architectures;
`release-quality-gate.yml` runs them for a release. A PR that only touches
`README.md`, `docs/release-readiness.md` or `docs/research/**/*.md` skips the native lanes
(`scripts/ci-docs-scope.mjs`). Locally, `npm run verify` runs the test and lint suites above
(`scripts/verify.mjs`, documented in [CONTRIBUTING.md](../CONTRIBUTING.md#running-tests));
`scripts/test-verify-ci-parity.mjs` keeps its check list in step with the commands the test lanes run,
gate list and setup action included. The `scripts/test-ci-*.mjs` tests read the workflows as parsed
YAML (`scripts/lib/ci-workflow.mjs`), not as text.
**Planned:** [#361](https://github.com/Magnus-Gille/sagascript/issues/361) phase B
replaces the remaining source-text tests in the `ui` group with component tests.

## 6. Known drift and planned changes

- Event and payload exceptions are named in the catalogue rather than removed
  ([#347](https://github.com/Magnus-Gille/sagascript/issues/347), [contracts/README.md](../contracts/README.md)):
  `state-changed` also carries `settings_reloaded`; `navigate_tab` is snake_case;
  `transcription-progress` and `transcription-phase` have no GUI listener; the
  `model-download-progress` `model` field is a Debug name for Whisper. Twenty registered
  commands have no frontend caller (`COMMANDS_WITHOUT_A_CALLER` in `src/lib/api.test.ts`);
  removing them is undecided.
- Commands for dictation, transcription, hotkeys, the dictionary and file transcription return
  `Result<T, CommandError>` (`{ code, message, retryable }`, ADR 0006) and the frontend matches
  on `code`. The other commands still return `Result<_, String>`, and one frontend check still
  matches message text (`defaultDictionaryLanguageBlock` in `Settings.svelte`). **Planned:**
  part 2 of [#360](https://github.com/Magnus-Gille/sagascript/issues/360).
- Only some CLI subcommands offer `--json` (`list-models`, `config list`, `glossary list`,
  `formats` and `config profiles list` do not), `--progress-json` events share stderr with prose
  and `tracing`, and a second Ctrl-C still force-quits from the signal thread
  (`cancel.rs`). Follow-ups of [#364](https://github.com/Magnus-Gille/sagascript/issues/364).
- Whisper and Pianissimo are selected by hand (`LiveDictationBackend::new(whisper, pianissimo)`);
  there is no engine trait. **Planned:** [#378](https://github.com/Magnus-Gille/sagascript/issues/378).
- `main.rs` will be split by domain
  ([#369](https://github.com/Magnus-Gille/sagascript/issues/369)); update this map when files
  move. `commands.rs` was split in [#370](https://github.com/Magnus-Gille/sagascript/issues/370).
- Most `async` Tauri commands in `commands/` have no `.await` and call synchronous helpers
  that write the settings file (`persist_and_sync` → `store::update`, under the settings file
  lock) on a Tokio worker. Model downloads and Pianissimo installs already do their file work
  on the blocking pool ([#368](https://github.com/Magnus-Gille/sagascript/issues/368)). **Not
  done in #370:** the setters stay `async fn`s that write on the worker. They cannot become
  plain `fn`s (a sync command runs on the main thread, ADR 0003), and `spawn_blocking` needs an
  owned handle on the controller, which Tauri manages by reference, so moving the write off the
  worker is a separate change.
