# 0006. Typed errors across boundaries

- Status: Accepted (2026-10-14). Part 1 of [#360](https://github.com/Magnus-Gille/sagascript/issues/360)
  implements it for dictation, transcription, hotkeys, the dictionary and file transcription;
  part 2 finishes the migration.
- Related: [#347](https://github.com/Magnus-Gille/sagascript/issues/347) (generated `CommandMap`),
  [#364](https://github.com/Magnus-Gille/sagascript/issues/364) (CLI error object),
  [ADR 0002](0002-privacy-first.md)

## Context

- Commands returned `Result<_, String>` (191 in the app crate, 56 in core, 31 in the CLI at
  a69d5d1). `DictationError` was flattened to a string at the command boundary, so a timeout, a
  busy model and a join error read alike, and `main.rs` repacked them as `TranscriptionFailed`.
- The frontend branched on message text: `startsWith("Dictionary changed elsewhere:")` (six
  places) and `includes("failed to open file:")`. Rewording a message silently broke behaviour.
- Cancelling Pianissimo was the string `"Pianissimo transcription cancelled"`, and
  `map_engine_error` dropped the host's `code` and `retryable`.
- The Swift host reports absolute paths (`Failed to load model at /Users/<name>/Library/…`);
  they reached the app log and the UI, against the rule that logs carry no home directory.
- About thirty copies of `typeof e === "string" ? e : e?.message || …`, three `errorText`
  helpers and 26 explicit `any` (mostly `catch (e: any)`).

## Decision

1. **Wire shape.** A command that returns `Result<T, CommandError>` rejects `invoke()` with
   `{ "code": string, "message": string, "retryable": boolean }`, serialized by hand in
   `src-tauri/src/command_error.rs`. `#[serde(tag = "code", content = "message")]` was not used:
   it drops `message` on unit variants and cannot carry `retryable`. The TypeScript mirror is
   `BackendError` in `src/lib/errors.ts`; `contracts/transport/command_errors.json` is written by
   Rust from real errors and parsed by Vitest.
2. **Codes are stable snake_case**, added and never renamed or reused. `DictationError::code()`
   is the one list for workflow errors (the variant name in snake_case, or an engine failure's
   own `engine_*` code); the CLI's JSON error object uses the same function, so there is one
   vocabulary. `CommandError` adds `busy`, `not_recording`, `cancelled`,
   `dictionary_changed_elsewhere`, `hotkey_invalid`, `hotkey_change_failed`, and the
   transitional `unclassified`. A cancellation has one representation (`cancelled`).
3. **The frontend matches only on `code`**: `isBackendError(error, code)`. `message` is shown
   (`toErrorMessage`) and never parsed. A command not yet typed still rejects with a string; the
   generated `CommandMap` says which (`error: string | BackendError`, `BACKEND_ERROR_COMMANDS`),
   and the IPC fake fails a test whose rejection has the other shape.
4. **Engine failures** are `DictationError::Engine(EngineFailure)`, a struct with private
   `code`, `retryable` and `message`, built only by `EngineFailure::new`. A variant's fields are
   always public, so `Engine { code, retryable, message }` could be built unscrubbed; and
   `EngineFailure { kind, user_message, detail }` would keep a second, path-bearing `detail`
   that logs and the UI could reach. Only the scrubbed text is kept. `map_engine_error` keeps
   the host's code (`engine_model_missing`, `engine_busy`, …; host-side failures are
   `engine_crashed`, `engine_timeout`, `engine_unavailable`, `engine_io`) and its retry hint.
5. **Paths are scrubbed at the Rust boundary** by `sagascript_core::path_scrub::scrub_paths`:
   macOS and Linux account folders, volumes and temporary folders, Windows drive and UNC
   paths, `file://` URLs and the current home directory become `<path>`, keeping the file name
   below the account folder (`<path>/foo.mlmodelc`). It runs where foreign text enters an error
   type (`EngineFailure::new`), in the engine's log lines (load failure, host stderr, pre-warm
   failure) and again when `CommandError` is serialized, so a path that arrives through a plain
   string is removed too. The Swift sources are unchanged.
6. **Retry or download** is decided from the error, not the text: `retryable` offers another
   attempt, `engine_model_missing` and `engine_model_load_failed` offer a model download
   (`backendErrorRecovery`).

## Consequences

- Rewording a message cannot change behaviour; a new failure that needs its own handling needs
  a code, a row in `command_errors.json` and a test.
- `DictationError` gains `Cancelled`, `Engine`, `TranscriptionTimedOut`, `TaskFailed` and
  `FileNotFound` (a missing file, from `ErrorKind::NotFound`). `PasteError`,
  `ApplicationLaunchError` and `AccessibilityPermissionDenied` stay: the paste service and
  `sagascript open` construct them. The CLI's `code` changes only for failures that were
  `transcription_failed` or `file_decode_error` before (engine failures, cancellation, a missing
  file).
- The scrubber is a heuristic for text it does not control. A path with several spaces in a
  folder name may keep a trailing word; it never keeps the account name.
- **Part 2** ([#360](https://github.com/Magnus-Gille/sagascript/issues/360)): convert the rest
  of the app crate's `Result<_, String>` (the target is at most 20, each justified), give every
  `Unclassified` producer a code, type `SettingsError` (the one remaining text match,
  `defaultDictionaryLanguageBlock` in `Settings.svelte`, needs it), add `From` impls for the nine
  `Meeting*`/`Reference`/`Proposal` error enums, keep the source errors in
  `meeting_reprocessing_commands.rs`, and wire "Try again" and "Download the model again" into
  the transcription error views.
