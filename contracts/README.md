# Rust–TypeScript transport contract

The webview and the Rust backend talk over Tauri IPC: commands (`invoke`) and events
(`listen`). Each side compiles on its own, so a field, enum value or name renamed on one side
used to reach the GUI unnoticed. This directory holds what both sides are tested against
([#347](https://github.com/Magnus-Gille/sagascript/issues/347)).

## What is checked, and where

| Contract | Rust side | Shared artifact | TypeScript side |
|---|---|---|---|
| Payload shapes | Tests that build real payloads with the producer's own code: `commands/settings_tests.rs` (`get_settings`), `meeting_jobs_tests.rs` (`get_meeting_job`, every job status), `events_tests.rs` (every event) | `transport/*.json` (written by Rust) | `src/test/transport.test.ts` parses each fixture with a schema from `src/test/transport-schemas.ts`; each schema is proven equal to its frontend type (`Settings`, `MeetingJobSnapshot`, `EventMap`) |
| Event names, consumers, scope | The catalogue in `src-tauri/src/events.rs`; emitting any other way is a clippy error (`disallowed-methods`) | `transport/events.json` | `EventMap` in `src/lib/events.ts` lists exactly the events the catalogue marks `gui` |
| Command errors | `src-tauri/src/command_error.rs`; `command_error_tests.rs` writes real serializations, including a Swift-style engine failure | `transport/command_errors.json` | `src/test/transport.test.ts` parses each with `BackendError`'s schema and checks the codes, the retry and download choices and that no message carries a path |
| Command names, argument keys and types | `src-tauri/src/contract/command_map.rs` reads `generate_handler!` and every `#[tauri::command]` signature | `src/lib/ipc-contract.generated.ts` (`CommandMap`, `COMMAND_ARGUMENTS`) | The IPC mock (`src/test/ipc.ts`) rejects a call whose argument keys Rust does not read, at run time in every test that goes through a wrapper; `src/lib/api.test.ts` type-checks each wrapper's arguments and result against `CommandMap` |

A change on one side fails a test:

- Rust renames or retypes a field, an enum value or an event: the Rust fixture test fails
  (`contracts/transport/… no longer matches what Rust sends`, with the JSON path).
- After the Rust fixture is regenerated, or when TypeScript changes instead: Vitest fails to
  parse the fixture (`$.field: missing`, `… sent but not in the TypeScript type`, `expected
  one of …`), or svelte-check fails because a schema no longer equals its type.
- A command's name, parameter or type changes: the app crate's tests fail until the generated
  file is regenerated; then svelte-check flags every wrapper row in `src/lib/api.test.ts` that
  no longer matches, and Vitest fails every test in which a wrapper still sends the old
  argument key ("invoked with arguments the Rust command does not read: missing …; no
  parameter …").
- A `#[tauri::command]` missing from `generate_handler!`, a Rust type without a TypeScript
  mapping, or a raw event name: the app crate's tests or clippy fail.
- A command the Playwright mock (`scripts/fixtures/transcription-browser-mock.js`) answers is
  not registered any more: `scripts/test-browser-mock-commands.mjs` (`ci` group) fails.

Everything runs in the existing gates, with no model, GUI or network: `cargo test` of the app
crate (test-macos, test-windows; `npm run verify`'s `test-workspace`) compares the fixtures and
the generated file, clippy of the app crate (test-linux, test-macos, test-windows) rejects a raw event name,
svelte-check (test-macos) checks each schema against its type, and Vitest (test-macos,
test-windows) parses the fixtures. The generated `CommandMap` and the fixtures need no check of
their own in `ci.yml` or `scripts/verify.mjs`: the app crate's tests are the "up to date"
check, so `scripts/test-verify-ci-parity.mjs` has no new command to match.

## After an intended change

From `src-tauri/`, regenerate the fixtures and the generated `CommandMap`, then review the diff
and run the tests again without the variable:

```bash
SAGASCRIPT_BLESS_CONTRACTS=1 cargo test -p sagascript contract
cargo test -p sagascript contract
```

Then make the TypeScript side agree: the type in `src/lib/` and its schema in
`src/test/transport-schemas.ts`, the wrapper in `src/lib/api.ts` and its row in
`src/lib/api.test.ts`. Bless from a build with the default features: the `reprocessing` field
and fixture variant exist only with `diarization`, and the test without it checks that
everything else is unchanged.

## Named exceptions

- **Internal events** (`Consumer::Internal` in `events.rs`, `"consumer": "internal"` in
  `events.json`): sent, but no frontend code listens, each with its reason
  (`transcription-progress`, `transcription-phase`). They are not in `EventMap` and need no
  listener.
- **`navigate_tab`** predates the kebab-case rule; renaming it would change the wire.
- **`state-changed` = `settings_reloaded`**: not a dictation state but a reload signal
  (`UiState::SettingsReloaded`). A new concern gets its own event.
- **`model-download-progress.model`** is the Rust variant name for Whisper downloads
  (`KbWhisperLarge`) and the model id for Pianissimo; the frontend reads only `progress`.
- **Untyped JSON** (`serde_json::Value`) in `get_engine_load_state`, `save_update_recovery` and
  `load_update_recovery`: `JSON_VALUES` in `command_map.rs` names the TypeScript type the
  frontend uses; the engine-load-state shape is pinned through `events.json`.
- **Ignored fields:** Rust sends the settings migration markers (`schema_version`,
  `model_lineup_version`, `engine_defaults_version`, `profile_glossary_migrated`) and the
  download byte counts; the schemas list them as `ignored`.
- **Registered commands without a caller** are listed with a reason in `src/lib/api.test.ts`
  (`COMMANDS_WITHOUT_A_CALLER`); they have no frontend wrapper.

The `listen("…")` call sites in the components still take their event name and payload type
from the component, not from `EventMap`; they become typed in
[#362](https://github.com/Magnus-Gille/sagascript/issues/362).

## Errors

A command rejects with a `BackendError` `{ code, message, retryable }` (`Result<T, CommandError>`,
[ADR 0006](../docs/adr/0006-typed-errors.md)); since #360 part 2a every command is typed.
The generator reads the error type of each signature: `CommandMap[C]["error"]` is `BackendError`
(or `string` for a `Result<T, String>` signature, which the generator still accepts), and
`BACKEND_ERROR_COMMANDS` lists the typed commands. The IPC fake (`src/test/ipc.ts`) rejects a
test whose fake throws the other shape, so use `backendError(code, message)` in a fake. The
frontend matches on `code` only (`isBackendError`).
