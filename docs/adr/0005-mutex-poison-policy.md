# ADR 0005: Recover poisoned mutexes instead of panicking every later caller

## Status

Accepted (issue #359).

## Context

- `SharedController = Mutex<AppController>` (`src-tauri/src/commands.rs`) was locked with
  `.lock().unwrap()` at 77 sites in `commands.rs` and `main.rs`, including the global-shortcut
  handler. Audio start/stop and synchronous JSONL log writes run under that lock.
- The workspace deliberately does not set `panic = "abort"`, so panics unwind. A panic while the
  guard is held poisons the mutex, and every later command and hotkey press then panicked
  until the app was restarted.
- `Drop` impls panicked on a poisoned lock: `HotkeyConfigurationLease` (`commands.rs`),
  `WorkLease` and `ExclusiveLease` (`update_activity.rs`), `Lease` (`plain_file_jobs.rs`), `Slot`,
  `Reservation` and `Inner` (`engine_host/client.rs`), `HostProcess` (`engine_host/process.rs`).
  They run while unwinding, so poison meant a second panic and a process abort ("panic in a
  destructor during cleanup", reproduced by the engine-host regression test).
- The settings watcher thread (`main.rs`) had no `catch_unwind`: one panic silently ended
  settings hot reload for the rest of the session.
- Core code panicked on poison too: `engine_host/{client,process}.rs`, `audio/capture.rs` (also in
  the realtime input callback) and `whisper_backend.rs`, whose `lock_state_bounded` panicked by
  design. Other modules already tolerated poison (`dev_overrides.rs`, `native_log.rs`, parts of
  `pianissimo_backend.rs`) or failed the command (`meeting_jobs.rs`, `meeting_media.rs`).
- No panic hook was installed. The default hook prints to stderr, which, like the GUI's tracing
  output on stdout, never reaches the JSONL app log; it is only visible from a terminal.

## Decision

1. **Recover.** Lock through `sagascript_core::sync::lock_or_recover(&mutex, "lock.name")`, or
   `commands::lock_controller` for the controller. On poison the helper takes the data
   (`PoisonError::into_inner`), clears the poison flag so each incident is reported once, and
   logs `tracing::error!` with the lock name and the latest panic message. Both binaries call
   `install_panic_recorder()` first; it chains to the previous hook, so stderr output is unchanged.
2. **Recover only consistent data.** Each lock documents why its data survives an interrupted
   update (usually: every critical section is one assignment or collection operation with no
   panic point between related fields). Where a panic can leave state stale, restore a safe state
   before handing it out (`lock_or_recover_with`, `recover_poisoned`):
   - whisper `state`: discard the warm `WhisperState`, as after an abort; it is rebuilt lazily.
   - engine-host client `state`/`lifecycle`: clear `loaded`, so the next request re-validates the
     host (restart within the crash budget) and re-sends `load`, a no-op for a loaded model.
     Slot counters are left alone because live slot guards still decrement them.
3. **Fail closed** when the lock guards a multi-step native transaction that cannot be
   re-validated: whisper `load_lock` keeps returning its actionable "restart Sagascript" error.
   `transcribe_dictation` and `with_model` hold `load_lock` across inference, so a panic
   mid-inference poisons it and that `WhisperBackend` (for live dictation, the shared one) stays
   unavailable until restart, which a regression test pins. Plain-file and meeting imports use
   their own backends; the rest of the app, including Pianissimo dictation, keeps working.
4. `Drop` impls always use the tolerant helpers. `try_lock` callers treat only `WouldBlock` as
   busy and recover `Poisoned` like `lock()`. Long-lived loops isolate each iteration with
   `catch_unwind`: the settings watcher (`run_settings_watcher_loop`) logs the panic, counts it in
   `HotkeyStatus.settings_watcher.recovered_panics` (`hotkey_status` command) and keeps watching;
   a panic during its unregister/register step records the registration as unknown.
5. `lock().unwrap()` remains acceptable in test code only.

## Consequences

- One panic no longer turns every later command, hotkey press or lease drop into a panic or an
  abort; it is logged once with the lock and its cause, and regression tests poison real locks
  to keep it that way. The fail-closed whisper `load_lock` (decision 3) is the exception: a panic
  during Whisper inference still requires a restart before Whisper works again.
- A bug may now show up as a degraded feature instead of a crash. The `error!` lines and the
  watcher's `recovered_panics` counter are the signal; recovery never replaces fixing the panic.
- Do not recover when nothing re-validates data whose cross-field invariants a panic can break,
  when the lock guards native or FFI state that may be mid-operation, or when continuing could
  write corrupt user data to disk. Restore a safe state, or fail the operation with an error.
- Every new lock needs a name and a one-line consistency argument next to it.
- Every `SharedController` lock goes through `lock_controller`, including the meeting paths
  (`meeting_jobs.rs`, `meeting_reprocessing_commands.rs`) and the non-blocking pre-warm. The
  meeting-job slot (`meeting_jobs.rs`) and the meeting audio attachment (`meeting_media.rs`)
  are separate mutexes that still fail closed: poison turns into an error message asking for a
  restart.
- Panic and recovery logs still go to stdout/stderr, not the JSONL app log.
