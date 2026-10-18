# 0003. Native input and UI APIs run on the main thread

- Status: Accepted (practice predates this record; recorded 2026-10-12)
- Related: [#344](https://github.com/Magnus-Gille/sagascript/issues/344),
  [ARCHITECTURE.md §2.1 and §3](../ARCHITECTURE.md#3-threads-and-locks)

## Context

Auto-paste simulates a keystroke with `enigo`, which on macOS uses Text Input Source
(TIS/HIToolbox) APIs. Those APIs, AppKit, and Tauri's tray and window handles are only safe
on the main thread. Calling them from a Tokio worker crashes the process. The repo records
this in several places, with slightly different signal names:

- AGENTS.md and CONTRIBUTING.md ("Calling these APIs from a tokio worker thread will cause a
  SIGTRAP crash").
- `src-tauri/src/commands/transcribe.rs`, Dictate test transcription: "enigo's macOS TIS APIs crash if
  called from a tokio worker thread (SIGTRAP in dispatch_assert_queue)".
- `stop_recording_and_transcribe` in `src-tauri/src/dictation/pipeline.rs`: "Auto-paste MUST run on the main
  thread — enigo's macOS TIS APIs crash (SIGABRT) if called from a tokio worker thread."
- `dispatch_to_main` in `main.rs`: "Native tray/window APIs must not be touched from a
  transport or worker thread"; the single-instance callback (`launch/lifecycle.rs`) queues its UI work the same way
  because it "is not guaranteed to be the Tauri main thread (notably on macOS)".

The opposite constraint matters as much: on macOS (and for key-down on Windows) the main
thread delivers hotkey events, so blocking it stalls redraws and later key events (comment on
the minimum-duration top-up in `stop_recording_and_transcribe`).

## Decision

1. `enigo`, TIS/HIToolbox, AppKit/`NSWorkspace` calls and Tauri tray, menu, window and
   overlay calls run on the main thread. From async or worker code use
   `AppHandle::run_on_main_thread`; for fire-and-forget UI updates use `dispatch_to_main`.
2. Never block the main thread: no sleeps, inference, network or file waits there. The hotkey
   handler does only short work (controller state change, capture start, UI updates) and hands
   the rest to a task.
3. When the caller needs the result, send it back over a channel and wait off the main thread
   with a timeout: `paste_completion::wait` for paste, `on_main` (500 ms) inside
   `restore_target_before_paste` for AppKit focus calls.
4. Platform scope: the paste rule is mandatory on macOS. On Windows the paste closure runs on
   `spawn_blocking` because clipboard access can block the UI thread; Linux pastes by running
   `xdotool`. Tray, menu, window and overlay calls belong on the main thread on every platform:
   code that may run elsewhere goes through `dispatch_to_main`. Do not assume a callback runs
   on the main thread; check where the event source delivers it (see Consequences).
5. Only a signed build exercises paste permissions realistically: use
   `cargo tauri build --debug`, not `cargo tauri dev`, for Accessibility and microphone testing.

## Consequences

- Async code cannot call `PasteService` directly; every paste path needs a dispatch and a
  completion wait. Today there are several variants (the hardened live path in
  `dictation/pipeline.rs`, the fire-and-forget `run_on_main_thread` paste after file
  transcription in `commands/transcribe_file.rs`, and
  the `on_main` helper). #344 consolidates them behind one adapter and makes the lifecycle
  testable with fakes; until then, copy the hardened path in `stop_recording_and_transcribe`.
- Main-thread code paths cannot run in plain unit tests; keep decisions in pure functions
  (`should_use_safe_fallback`, `auto_paste_permitted`, `paste_completion` helpers) and test
  those.
- Sync `#[tauri::command]` functions also run on the main thread, so commands that do I/O are
  `async`.
- Known violation: global-shortcut callbacks are not main-thread everywhere.
  `tauri-plugin-global-shortcut` 2.3.2 calls the app handler inline from the `global-hotkey`
  event source (pinned rev `35b81e5`): on macOS from a Carbon handler on the application event
  target (main thread; the `hotkey/macos_bare.rs` NSEvent monitors are main-thread too); on
  Windows `Pressed` from `WM_HOTKEY` on the plugin's message window (main thread) but
  `Released` from a spawned `GetAsyncKeyState` polling thread; on Linux (X11) both from the
  library's own event thread. So on Linux `handle_hotkey_event` in `dictation/trigger.rs` calls
  `select_profile_menu`, `update_tray_status` and `overlay::show` off the main thread, and on
  Windows `handle_hotkey_release` runs on the polling thread (it locks the controller, logs and
  spawns a task, so this is harmless today). A fix routes those UI calls through
  `dispatch_to_main`.
- On macOS `handle_hotkey_event` takes the `SharedController` lock on the main thread. That is
  fine only while every holder keeps its critical section short; code moved during the
  `main.rs` split ([#369](https://github.com/Magnus-Gille/sagascript/issues/369)) must not add
  I/O or waits under that lock.
