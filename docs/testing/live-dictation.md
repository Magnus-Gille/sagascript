# Live dictation lifecycle evidence

The hotkey pipeline executes through `DictationRuntime`. Production still uses the
real `AppController`, capture service, local backend transaction, focus restoration,
main-thread UI dispatch and paste service. Tests replace those external boundaries;
they do not implement a second pipeline. `LiveSession` consumes a generation-bound
terminal outcome and checks that the controller still owns the processing session.

Queued native callbacks validate both generation and state when they execute. A queued
paste whose completion receiver has already expired does not begin. A paste that has
already begun may still complete after the two-second wait; the existing timeout warning,
retained text and decision not to move focus to a copy window remain in effect.

## Executed contract map

Run `cargo test -p sagascript dictation::pipeline` from `src-tauri/`. Tests are in
`src/dictation/pipeline_lifecycle_tests.rs` and invoke the production `request_stop`/`run`.
The capture-adapter tests additionally exercise the real controller's public methods.

| Contract | Representative test/evidence |
|---|---|
| PTT/toggle, profile and auto-paste | `ptt_and_toggle_sessions_select_their_profile_request_with_each_autopaste_setting`; success tests assert text, pending recovery and exact events. |
| Duplicate edges/stops and idle stop | `standalone_stop_is_noop_and_public_hotkey_edges_stop_each_mode_once`, `duplicate_request_stop_runs_only_one_capture_stop_and_inference`. |
| Minimum duration and old delayed stop | `delayed_old_stop_cannot_stop_a_new_recording_generation`: real cancel/start before the delayed stop runs, no private state fabrication. |
| Capture start/stop/resample and empty audio | `failed_capture_start_logs_once_without_consuming_generation_or_leaking_work`, `capture_stop_failure_emits_typed_error_without_starting_inference`, empty/blank test; capture adapter pins the resample error boundary. |
| Model selection, cold load, inference and join errors | `model_selection_refusal_and_inference_join_error_emit_exact_terminal_events`, `cold_model_loading_precedes_result_and_clears_the_loading_tray`, typed inference failure test. |
| No speech and cancellation | Quiet-result tests preserve previous useful text and distinguish cancellation/no-speech from errors. |
| Deadline, abort and grace | Both `inference_timeout_*` tests assert one abort, 60-second deadline and the 5-second grace outcome; generic `inference_wait_tests` retain boundary tests. |
| Paste success, failure, dispatch failure, closed channel, timeout | `paste_failures_keep_recognized_text_and_choose_copy_fallback_by_outcome` and queued-paste tests assert distinct errors, retained text, pending result and fallback behavior. |
| Late responses | `timed_out_old_inference_finishing_after_new_session_cannot_change_it`; stale paste callback test checks the new recording and retained result. |
| Queued UI and delayed overlay | `stale_queued_ui_cannot_hide_or_relabel_a_new_recording`, `stale_error_overlay_callback_rechecks_when_main_thread_executes_it`, `old_terminal_ui_is_discarded_after_a_new_session_also_finishes`. The last case requires the generation guard even when both sessions have reached Idle. |
| Settings read timing and frozen shortcut | `pipeline_reads_model_options_after_release_and_autopaste_after_inference_while_binding_stays_frozen` changes settings across actual async boundaries. |
| Cleanup and private session metadata | `every_terminal_path_logs_once_releases_its_work_and_allows_the_next_session` reads actual JSONL, counts session records and work/result leases, rejects unexpected payload keys/content, retries a duplicate stop and starts the next session. |

The GUI Dictate/Teach and training-file waits use the same bounded inference-wait helper.
Their existing typed errors, training deadline calculation, glossary processing and final
state handling are preserved. The file/batch CLI orchestration is outside this refactor.

## Scope of evidence

Paused Tokio time, oneshot completions and explicit dispatch barriers make the races
reproducible. Four isolated behavior mutations remove the UI generation check, duplicate
terminal event delivery, omit abort, or discard recognized text after paste failure;
each must compile and fail the relevant assertion before the originals are restored.
Compilation failures are not successful mutation evidence.

Characterization required adding injectable capture/runtime boundaries before the full-flow
tests could run. It is not claimed that these new tests ran against unmodified original
source. The extraction snapshot precedes session consolidation and the stale-callback fixes;
those fixes have recorded failing regressions followed by passing results.

These tests do not establish macOS TCC, AppKit, actual global registration, native paste,
real microphone inference or latency. Native acceptance must bind the exact tested app
bundle and source tree, exercise its shared app/CLI binary, and preserve personal data.
Signing and native execution require their separately authorized prerequisites. A Linux
pass or a mocked browser pass does not close that acceptance boundary.
