# 0006. Typed errors across boundaries

- Status: Accepted (2026-10-14). Part 1 of [#360](https://github.com/Magnus-Gille/sagascript/issues/360)
  implements it for dictation, transcription, hotkeys, the dictionary and file transcription;
  part 2a (decisions 7 and 8) types the rest of the app crate and the settings; part 2b
  (decision 9) types the `error` event and wires the recovery; what it consciously leaves is
  listed under Consequences.
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
   `dictionary_changed_elsewhere`, `hotkey_invalid`, `hotkey_change_failed`, since part 2a the
   codes of decision 8 and, since part 2b, `paste_error` (the code `DictationError` already
   has, with the paste task's own text, for the `error` event). A cancellation has one representation (`cancelled`). The transitional
   `unclassified` of part 1 is gone: after part 2a nothing produced it, and a `String` no longer
   converts into a `CommandError`.
3. **The frontend matches only on `code`**: `isBackendError(error, code)`. `message` is shown
   (`toErrorMessage`) and never parsed. Every command is typed since part 2a; the generated
   `CommandMap` still says which shape a signature has (`error: string | BackendError`,
   `BACKEND_ERROR_COMMANDS`), and the IPC fake fails a test whose rejection has the other shape.
   The `error` event carries the same object (`EventMap.error` is a `BackendError`, decision 9).
4. **Engine failures** are `DictationError::Engine(EngineFailure)`, a struct with private
   `code`, `retryable` and `message`, built only by `EngineFailure::new`. A variant's fields are
   always public, so `Engine { code, retryable, message }` could be built unscrubbed; and
   `EngineFailure { kind, user_message, detail }` would keep a second, path-bearing `detail`
   that logs and the UI could reach. Only the scrubbed text is kept. `map_engine_error` keeps
   the host's code (`engine_model_missing`, `engine_busy`, …; host-side failures are
   `engine_crashed`, `engine_timeout`, `engine_unavailable`, `engine_io`) and its retry hint.
5. **Paths are scrubbed at the Rust boundary** by `sagascript_core::path_scrub::scrub_paths`:
   macOS and Linux account folders, volumes and temporary folders (matched ASCII
   case-insensitively, also as `/System/Volumes/Data/…` and after a run of slashes), `file://`
   URLs with or without a host, Windows drive, UNC and `\\?\` paths and the current home
   directory become `<path>`, keeping the file name below the account folder
   (`<path>/foo.mlmodelc`) and removing every folder before it. The Swift JSON encoder's `\/`
   spelling of a slash is read as a slash. It runs where foreign text enters an error type
   (`EngineFailure::new`), in the engine's log lines (load failure, host stderr cut to 4 KB,
   malformed host lines, pre-warm failure) and again when `CommandError` is serialized, so a
   path that arrives through a plain string is removed too. The Swift sources are unchanged.
   Privacy first: removing too much text is acceptable, keeping an account name is not. Three
   properties are tested, also on generated inputs: a path never extends past the start of
   another recognised root; the scan is linear (a megabyte takes milliseconds in a release build,
   so the stderr drain thread never stalls); and a second scrub changes nothing, which matters
   because the app scrubs twice.
6. **`retryable` means the user's action may be repeated** (record again, run the file again)
   and is expected to succeed: a busy engine, a model still loading, nothing captured, an engine
   host restart. It does not mean the same `invoke` may be repeated: `stop_and_transcribe` has
   consumed the audio, so a second call finds no recording and resolves with an empty text.
   Part 2 must not wire `backendErrorRecovery() === "retry"` to a bare re-invoke of
   `stop_and_transcribe`; "Try again" restarts the action. A timeout is not retryable: the same
   input under the same budget tends to time out again, and a file that always exceeds it would
   be offered a retry forever. `engine_model_missing` and `engine_model_load_failed` offer a model
   download instead (`backendErrorRecovery`).

7. **Settings failures are `SettingsError`** (`sagascript_core::settings::SettingsError`). The
   settings manager, store, operations (`ops`), machine wrappers and hotkey validation return
   it instead of a `String`; its `Display` is the text each refusal always had.
   `machine::try_update` (and the store beneath it) hands back the error type of the caller's
   closure (`E: From<SettingsError>`) and converts its own failures into it, so a typed refusal
   needs no side channel out of the settings lock. The CLI wraps it as `DictationError::Settings` (its JSON error object gets
   the specific code), the app as `CommandError::Settings`. Codes: `invalid_setting_value`,
   `unknown_profile`, `model_incompatible`, `pianissimo_unavailable`, `hotkey_invalid`,
   `profile_invalid`, `dictionary_blocks_language_change`, `inactive_profile_dictionary`,
   `settings_storage_failed`. The Settings window recognises a language change a personal
   dictionary blocks by `dictionary_blocks_language_change`, which removed the last text match.
8. **Command codes beyond the workflow errors.** `invalid_input` (the command refused its
   arguments or the user's choice), `not_found`, `unavailable` (not in this build, window or
   state), `io_error`, `clipboard_failed`, `export_exists` (exports never overwrite),
   `launch_failed`, `model_download_failed` and `accessibility_permission_denied` (the last
   two with the command's own text, sharing the code of the same condition elsewhere), plus
   `busy` for an update being applied or a job already running. The meeting error enums convert
   through `DictationError` (`meeting_invalid`, or `meeting_stale` when a revision or recording
   no longer matches what the caller holds; the diarization evaluation enums are
   `diarization_error`), matched exhaustively so a new variant needs a code before it compiles.
   Where a fixed message replaces a source error, `CommandError::because` scrubs the cause, logs
   it and keeps it as the error's `source()`; it never reaches the webview.

9. **Events, cancellation and recovery (part 2b).**
   - The `error` event's payload is the `{ code, message, retryable }` of a failed command
     (`&CommandError` in `events.rs`, `BackendError` in `EventMap`, fixture
     `contracts/transport/events.json`). The hotkey pipeline sends a dictation failure with its own
     code and retry hint, a profile-model refusal as `CommandError::Settings`, and a failed paste as
     `CommandError::PasteFailed` (`paste_error`). The message is the text it always was, with absolute
     paths removed.
   - A hotkey dictation whose transcription ends `cancelled` ends quietly: idle, overlay hidden, no
     `error` event, no `UiState::Error`, no last error and no latency record (`latency-report`
     reads three outcomes). The quiet branch is defensive: in the hotkey path `Cancelled` can come
     only from the pipeline's own timeout abort (reported as `transcription_timed_out`), Whisper's
     sticky cancel (set only by the CLI) and the engine host's `cancelled` (its job's own cancel
     token, or stdin EOF at app exit), so nothing reaches it in production today; it fixes the rule
     for when a cancel source is added. File transcription shows a backend `cancelled` as
     cancelled, with the wording of its Stop button ("Transcription was cancelled.").
   - `apply_hotkey_change` rejects a refusal of the settings with the `SettingsError`'s code
     (`dictionary_blocks_language_change`, `model_incompatible`, `inactive_profile_dictionary`,
     `profile_invalid`, …). Only a failure of the shortcuts themselves is `hotkey_invalid` (a
     shortcut that does not parse) or `hotkey_change_failed` (the OS refused, a rollback failed, the
     registration moved underneath the change, the settings could not be saved after the OS accepted
     the shortcuts).
   - The Dictate tab's "Try it out" error and a failed file in the Transcribe tab offer
     `backendErrorRecovery(error)`: "Try again" when the action may be repeated, "Download the model
     again" for `engine_model_missing` and `engine_model_load_failed`. "Try again" restarts the
     action (decision 6): a new recording, or the same file with its profile and prompt as a new job;
     it never repeats `stop_and_transcribe`. The download runs the Pianissimo model's download (it
     checks an installed model and fetches it again when it is missing or damaged), shows the
     window's usual progress and then offers "Try again"; a failed download offers itself again
     only where it can pass (`model_download_failed`, an `engine_model_*` code or a retryable
     failure), and a refusal such as `pianissimo_unavailable` or `invalid_input` shows its error and
     nothing else, so the offer cannot loop. Plain text (the restored last error) has no code and
     offers nothing. The button is `aria-disabled` and guarded while a download runs, one button
     carries both labels, and the focus is moved explicitly where a button goes away (a new
     recording's button, the new job's tab), so it never falls to the page; the errors are
     `role="alert"`.

## Consequences

- Rewording a message cannot change behaviour; a new failure that needs its own handling needs
  a code, a row in `command_errors.json` and a test.
- `DictationError` gains `Cancelled`, `Engine`, `TranscriptionTimedOut`, `TaskFailed` and
  `FileNotFound` (a missing file, from `ErrorKind::NotFound`). `PasteError`,
  `ApplicationLaunchError` and `AccessibilityPermissionDenied` stay: the paste service and
  `sagascript open` construct them.
- **The CLI's JSON `code` changes** for every failure whose cause is now a typed error. Part 1
  moved failures that were `transcription_failed` or `file_decode_error` (engine failures,
  cancellation, a missing file). Part 2a moves the settings refusals of core, which were
  `settings_error`, in every run that frames its errors as JSON (`transcribe --json|--jsonl`,
  `record --json`, `glossary suggest --json`; `engine status|doctor --json` reach none). The text
  and the exit code are unchanged:

  | Failure | Before | After |
  |---|---|---|
  | `--profile` names no dictation profile (`transcribe`, `record`, `glossary suggest`) | `settings_error` | `unknown_profile` |
  | `transcribe --model` names no model | `settings_error` | `invalid_setting_value` |
  | The model cannot serve the language: `transcribe --model` or the stored file model against `--language` or the profile's language; the profile's stored model for `record` and `glossary suggest` | `settings_error` | `model_incompatible` |
  | Pianissimo for a language or profile that is not Swedish | `settings_error` | `pianissimo_unavailable` |

  Other `settings_error` objects stay: they are the CLI's own wording for a flag or a
  combination (`--model auto`, an unknown `--language`, `--speakers`, Pianissimo with
  `--diarize`, a missing corrected transcript). `settings_storage_failed` appears in one JSON run:
  settings are read without failing, and the one JSON command that writes them,
  `glossary suggest --apply`, reported a store failure as `settings_error` until part 2b, which
  gives it the code of its `SettingsError` (same text). The plain-text commands (`config`,
  `glossary add|remove|clear`) print the same lines as before, except that `glossary remove|clear`
  with an unknown `--profile` printed "Settings error: " twice and now print it once; their JSON
  objects do not exist. `run_context_cli.rs`, `transcribe_contract.rs` and
  `output_contract.rs` pin the codes.
- The scrubber is a heuristic for text it does not control. It removes the account name from
  every path in a recognised spelling (decision 5). Known non-goals:
  - a root that does not start a word (`é/Users/zoe`, `-o/Users/zoe`), a percent-encoded path
    (`%2FUsers%2Fzoe`), `~zoe/`, relative paths, and a home directory the process does not know;
  - an account folder whose name has a space and ends its clause (`C:\Users\John Smith.` with no
    folder after it) keeps the words after the space, because they cannot be told from prose; with
    a folder after it (`C:\Users\John Smith\x.bin`) the whole name goes;
  - a comma, semicolon or colon followed by a space ends a path even inside a folder name, so the
    folder names after it stay visible (`/Users/zoe/Smith, John/x.wav` becomes
    `<path>/Smith, John/x.wav`); never the account folder, which comes first.

  Where it ends a path early it errs towards keeping prose readable after the file name (a word
  without a separator after a name with an extension, a word ending in a colon, a period or an
  exclamation mark, a URL), and towards removing more when a later word contains a separator
  (`and/or` after `a.wav` joins the path).
- **Part 2a** (done): the app crate's production `Result<_, String>` went from 125 lines to 14
  (191 to 76 counting tests, the CommandMap generator and its fixtures, and
  `paste/windows_clipboard.rs`, whose 17 lines never cross IPC). What stays, and why:
  `finish_snapshot`'s job result (`meeting_jobs.rs`) and `AppController::finish_transcription`
  (`app_controller.rs`) carry the failure text a job or a dictation records as data
  (`MeetingSnapshot.error`, `last_error`); `UpdatePreparation`'s channel (`updater/activity.rs`)
  and `paste/completion.rs` carry text the Settings window or the paste task reported;
  `platform/macos.rs`, `hotkey/macos_bare.rs`, `updater/signed.rs` and the macOS monitor reply in
  `commands/hotkey.rs` are macOS-only and are only logged or mapped to `launch_failed` (they can
  be compiled by the macOS runner alone). The settings internals that still used `String`
  (`settings/migrations.rs`, `settings/lenient.rs`, `store::decode_object`) fed
  `SettingsLoadError`, which is typed; part 2b types them too.
- **Part 2b** (done): decision 9, and the settings internals above (`lenient::decode` and
  `store::decode_object` return the `serde_json::Error`, the migration steps a `StepError`; the
  texts the store reports are unchanged and pinned), and the CLI's glossary editor
  (`try_update::<DictationError, _>` in `add`, `remove`, `clear` and `suggest --apply`, which gave
  the doubled "Settings error: " its single prefix). The app crate's production count stays at 14
  (decision 8's list). What it consciously leaves:
  - `validate_hotkey_for_platform` returns a `String`: it is the complete text of
    `SettingsError::InvalidHotkey`, and no caller tells its nine reasons apart, so an enum would only
    be formatted back to the same strings.
  - `SettingsError` converts into a `String` (`From`), for the settings test tables.
  - The meeting (diarized) import offers no recovery buttons: its failure arrives as the text of the
    job's snapshot (`MeetingSnapshot.error`, data on the wire), not as a rejection with a code.
  - The Dictate tab's own recording shows a `cancelled` rejection of `stop_and_transcribe` as the
    text "Transcription cancelled." and nothing else; ending it quietly there too is a UI choice
    this part did not make.
  - "Download the model again" downloads the Pianissimo model, the one whose host reports
    `engine_model_missing` and `engine_model_load_failed`; `backendErrorRecovery` maps no other
    code, so a missing Whisper model has no such button.
