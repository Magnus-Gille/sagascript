use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use sagascript_core::audio::capture::AudioCaptureMetrics;
use sagascript_core::error::DictationError;
use sagascript_core::settings::{Settings, WhisperModel};
use sagascript_core::transcription::whisper_backend::DictationTimings;
use serde_json::{json, Value};
use tokio::sync::oneshot;
use tokio::task::{AbortHandle, JoinHandle};

use super::super::runtime::{DictationRuntime, InferenceRequest, InferenceTask, UiAction};
use super::{request_stop, run};
use crate::app_controller::{AppController, AppState, Capture};
use crate::events::{Event, UiState};

#[derive(Default)]
struct CaptureState {
    starts: usize,
    stops: usize,
    stop_result: Option<Result<Vec<f32>, DictationError>>,
}

struct FakeCapture(Arc<Mutex<CaptureState>>);

impl Capture for FakeCapture {
    fn start_capture(&mut self) -> Result<(), DictationError> {
        self.0.lock().unwrap().starts += 1;
        Ok(())
    }

    fn stop_capture(&mut self) -> Result<Vec<f32>, DictationError> {
        let mut state = self.0.lock().unwrap();
        state.stops += 1;
        state
            .stop_result
            .take()
            .unwrap_or_else(|| Ok(vec![0.25, -0.5, 0.75]))
    }

    fn stop_timings(&self) -> (Duration, Duration) {
        (Duration::ZERO, Duration::ZERO)
    }

    fn metrics(&self) -> AudioCaptureMetrics {
        AudioCaptureMetrics {
            stream_play_return_ms: None,
            first_callback_ms: None,
            device_sample_rate_hz: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct RecordedEvent {
    name: &'static str,
    payload: Value,
}

#[derive(Debug, Clone)]
enum InferencePlan {
    Immediate(Result<String, DictationError>),
    PendingUntilAbort { finish_on_abort: bool },
}

#[derive(Debug, Clone)]
enum PastePlan {
    Return(Result<(), String>),
    DispatchFailure(String),
    DropCompletion,
    Queue(Result<(), String>),
}

type RuntimeCallback = Box<dyn FnOnce(FakeRuntime) + Send + 'static>;
type PendingInferenceSender = Arc<Mutex<Option<oneshot::Sender<Result<String, DictationError>>>>>;

struct Observed {
    events: Vec<RecordedEvent>,
    actions: Vec<UiAction>,
    focus_logs: Vec<(&'static str, Value)>,
    inference_audio: Vec<Vec<f32>>,
    paste_requests: Vec<String>,
    restore_paste_calls: usize,
    abort_calls: usize,
    spawned: Vec<JoinHandle<()>>,
    defer_ui: bool,
    queued_ui: Vec<RuntimeCallback>,
    queued_paste: Vec<RuntimeCallback>,
    paste_dispatched: Arc<tokio::sync::Notify>,
    inference_abort_handles: Vec<AbortHandle>,
    pending_inference_senders: Vec<PendingInferenceSender>,
    inference_completed: Vec<Arc<AtomicBool>>,
}

impl Default for Observed {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            actions: Vec::new(),
            focus_logs: Vec::new(),
            inference_audio: Vec::new(),
            paste_requests: Vec::new(),
            restore_paste_calls: 0,
            abort_calls: 0,
            spawned: Vec::new(),
            defer_ui: false,
            queued_ui: Vec::new(),
            queued_paste: Vec::new(),
            paste_dispatched: Arc::new(tokio::sync::Notify::new()),
            inference_abort_handles: Vec::new(),
            pending_inference_senders: Vec::new(),
            inference_completed: Vec::new(),
        }
    }
}

#[derive(Clone)]
struct FakeRuntime {
    controller: Arc<Mutex<AppController>>,
    capture: Arc<Mutex<CaptureState>>,
    observed: Arc<Mutex<Observed>>,
    inference: Arc<Mutex<InferencePlan>>,
    paste: Arc<Mutex<PastePlan>>,
}

impl FakeRuntime {
    fn new(
        auto_paste: bool,
        capture_result: Result<Vec<f32>, DictationError>,
        inference: InferencePlan,
        paste: PastePlan,
    ) -> Self {
        let capture = Arc::new(Mutex::new(CaptureState {
            stop_result: Some(capture_result),
            ..CaptureState::default()
        }));
        let controller = AppController::with_capture(
            Settings {
                auto_paste,
                ..Settings::default()
            },
            FakeCapture(capture.clone()),
        );
        Self {
            controller: Arc::new(Mutex::new(controller)),
            capture,
            observed: Arc::new(Mutex::new(Observed::default())),
            inference: Arc::new(Mutex::new(inference)),
            paste: Arc::new(Mutex::new(paste)),
        }
    }

    fn start_recording(&self) -> u64 {
        let mut controller = self.controller();
        let profile = controller.settings().resolved_hotkey_profiles()[0].clone();
        assert!(controller.start_recording_for_profile(profile).unwrap());
        assert_eq!(controller.state(), AppState::Recording);
        controller.recording_generation()
    }

    fn now_std(&self) -> Instant {
        self.now()
    }

    fn events(&self) -> Vec<RecordedEvent> {
        self.observed.lock().unwrap().events.clone()
    }

    fn actions(&self) -> Vec<UiAction> {
        self.observed.lock().unwrap().actions.clone()
    }

    fn defer_ui_dispatch(&self, defer: bool) {
        self.observed.lock().unwrap().defer_ui = defer;
    }

    fn drain_ui(&self) {
        let queued = {
            let mut observed = self.observed.lock().unwrap();
            observed.defer_ui = false;
            std::mem::take(&mut observed.queued_ui)
        };
        for task in queued {
            task(self.clone());
        }
    }

    async fn wait_for_paste_dispatch(&self) {
        let notified = self.observed.lock().unwrap().paste_dispatched.clone();
        notified.notified().await;
    }

    fn queued_paste_count(&self) -> usize {
        self.observed.lock().unwrap().queued_paste.len()
    }

    fn run_one_queued_paste(&self) {
        let task = self
            .observed
            .lock()
            .unwrap()
            .queued_paste
            .pop()
            .expect("one queued paste callback should be available");
        task(self.clone());
    }

    async fn join_spawned(&self) {
        let spawned = {
            let mut observed = self.observed.lock().unwrap();
            std::mem::take(&mut observed.spawned)
        };
        for task in spawned {
            task.await.expect("fake runtime task should complete");
        }
    }

    fn release_pending_inference(&self) {
        let mut observed = self.observed.lock().unwrap();
        observed.pending_inference_senders.clear();
        for abort in observed.inference_abort_handles.drain(..) {
            abort.abort();
        }
    }

    fn pending_inference_is_finished(&self) -> bool {
        self.observed
            .lock()
            .unwrap()
            .inference_completed
            .iter()
            .all(|completed| completed.load(Ordering::SeqCst))
    }

    fn capture_counts(&self) -> (usize, usize) {
        let capture = self.capture.lock().unwrap();
        (capture.starts, capture.stops)
    }

    fn pending_paste_count(&self) -> usize {
        self.queued_paste_count()
    }
}

impl DictationRuntime for FakeRuntime {
    fn controller(&self) -> MutexGuard<'_, AppController> {
        self.controller.lock().unwrap()
    }

    fn emit_event<E: Event>(&self, _event: E, payload: E::Payload<'_>) -> tauri::Result<()> {
        self.observed.lock().unwrap().events.push(RecordedEvent {
            name: E::NAME,
            payload: serde_json::to_value(payload).expect("event payload should serialize"),
        });
        Ok(())
    }

    fn dispatch(&self, task: impl FnOnce(Self) + Send + 'static) {
        let mut observed = self.observed.lock().unwrap();
        if observed.defer_ui {
            observed.queued_ui.push(Box::new(task));
        } else {
            drop(observed);
            task(self.clone());
        }
    }

    fn apply_ui(&self, action: UiAction) {
        self.observed.lock().unwrap().actions.push(action);
    }

    fn focus_log(&self, event: &'static str, data: Value) {
        self.observed.lock().unwrap().focus_logs.push((event, data));
    }

    fn now(&self) -> Instant {
        tokio::time::Instant::now().into_std()
    }

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(duration)
    }

    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        self.observed
            .lock()
            .unwrap()
            .spawned
            .push(tokio::spawn(task));
    }

    fn model_is_warm(&self, _model: WhisperModel, _use_pianissimo: bool) -> bool {
        true
    }

    fn start_inference(&self, request: InferenceRequest) -> InferenceTask {
        let plan = self.inference.lock().unwrap().clone();
        let audio = request.audio;
        let observed = self.observed.clone();
        observed.lock().unwrap().inference_audio.push(audio);

        match plan {
            InferencePlan::Immediate(result) => InferenceTask {
                task: tokio::spawn(async move { (result, DictationTimings::default()) }),
                abort: Box::new(|| {}),
            },
            InferencePlan::PendingUntilAbort { finish_on_abort } => {
                let (sender, receiver) = oneshot::channel();
                let completed = Arc::new(AtomicBool::new(false));
                let completed_task = completed.clone();
                let task = tokio::spawn(async move {
                    let result = receiver.await.unwrap_or(Err(DictationError::Cancelled));
                    completed_task.store(true, Ordering::SeqCst);
                    (result, DictationTimings::default())
                });
                let task_abort = task.abort_handle();
                let sender = Arc::new(Mutex::new(Some(sender)));
                {
                    let mut observed = observed.lock().unwrap();
                    observed.inference_abort_handles.push(task_abort);
                    observed.pending_inference_senders.push(sender.clone());
                    observed.inference_completed.push(completed);
                }
                let abort_observed = observed.clone();
                InferenceTask {
                    task,
                    abort: Box::new(move || {
                        abort_observed.lock().unwrap().abort_calls += 1;
                        if finish_on_abort {
                            if let Some(sender) = sender.lock().unwrap().take() {
                                let _ = sender.send(Err(DictationError::Cancelled));
                            }
                        }
                    }),
                }
            }
        }
    }

    fn restore_paste_target(&self) -> impl Future<Output = ()> + Send {
        let observed = self.observed.clone();
        async move {
            observed.lock().unwrap().restore_paste_calls += 1;
        }
    }

    fn dispatch_paste(&self, task: impl FnOnce(Self) + Send + 'static) -> Result<(), String> {
        let plan = self.paste.lock().unwrap().clone();
        match plan {
            PastePlan::DispatchFailure(error) => Err(error),
            PastePlan::DropCompletion => {
                drop(task);
                Ok(())
            }
            PastePlan::Queue(_) => {
                let mut observed = self.observed.lock().unwrap();
                let notify = observed.paste_dispatched.clone();
                observed.queued_paste.push(Box::new(task));
                drop(observed);
                notify.notify_one();
                Ok(())
            }
            PastePlan::Return(_) => {
                task(self.clone());
                Ok(())
            }
        }
    }

    fn paste(&self, text: &str) -> Result<(), String> {
        self.observed
            .lock()
            .unwrap()
            .paste_requests
            .push(text.to_string());
        match self.paste.lock().unwrap().clone() {
            PastePlan::Return(result) | PastePlan::Queue(result) => result,
            PastePlan::DispatchFailure(_) | PastePlan::DropCompletion => {
                panic!("fake paste should not run for this dispatch plan")
            }
        }
    }
}

fn expected_event(name: &'static str, payload: Value) -> RecordedEvent {
    RecordedEvent { name, payload }
}

fn transcribing_then_idle() -> Vec<RecordedEvent> {
    vec![
        expected_event("state-changed", json!(UiState::Transcribing)),
        expected_event("state-changed", json!(UiState::Idle)),
    ]
}

async fn run_started(runtime: &FakeRuntime, generation: u64) {
    run(runtime.clone(), None, generation, runtime.now_std()).await;
}

#[tokio::test(start_paused = true)]
async fn successful_nonempty_transcript_without_autopaste_finishes_and_preserves_text() {
    let runtime = FakeRuntime::new(
        false,
        Ok(vec![0.25, -0.5, 0.75]),
        InferencePlan::Immediate(Ok("recognized words".into())),
        PastePlan::Return(Ok(())),
    );
    let generation = runtime.start_recording();

    run_started(&runtime, generation).await;

    assert_eq!(runtime.capture_counts(), (1, 1));
    let observed = runtime.observed.lock().unwrap();
    assert_eq!(observed.inference_audio, [vec![0.25, -0.5, 0.75]]);
    assert!(observed.paste_requests.is_empty());
    assert_eq!(observed.restore_paste_calls, 0);
    drop(observed);
    assert_eq!(
        runtime.events(),
        vec![
            expected_event("state-changed", json!(UiState::Transcribing)),
            expected_event("transcription-result", json!("recognized words")),
            expected_event("state-changed", json!(UiState::Idle)),
        ]
    );
    assert_eq!(
        runtime.actions(),
        vec![
            UiAction::Tray("transcribing"),
            UiAction::Finish {
                last_result: Some("recognized words".into()),
                open_copy_fallback: false,
            },
        ]
    );
    let controller = runtime.controller();
    assert_eq!(controller.state(), AppState::Idle);
    assert_eq!(controller.last_transcription(), Some("recognized words"));
    assert!(controller.is_update_result_pending("live-dictation"));
}

#[tokio::test(start_paused = true)]
async fn successful_nonempty_transcript_autopastes_and_clears_pending_result() {
    let runtime = FakeRuntime::new(
        true,
        Ok(vec![1.0, 0.5]),
        InferencePlan::Immediate(Ok("paste this".into())),
        PastePlan::Return(Ok(())),
    );
    let generation = runtime.start_recording();

    run_started(&runtime, generation).await;

    let observed = runtime.observed.lock().unwrap();
    assert_eq!(observed.paste_requests, ["paste this"]);
    assert_eq!(observed.restore_paste_calls, 1);
    drop(observed);
    assert_eq!(
        runtime.events(),
        vec![
            expected_event("state-changed", json!(UiState::Transcribing)),
            expected_event("transcription-result", json!("paste this")),
            expected_event("state-changed", json!(UiState::Idle)),
        ]
    );
    assert_eq!(
        runtime.actions(),
        vec![
            UiAction::Tray("transcribing"),
            UiAction::Finish {
                last_result: Some("paste this".into()),
                open_copy_fallback: false,
            },
        ]
    );
    let controller = runtime.controller();
    assert_eq!(controller.state(), AppState::Idle);
    assert_eq!(controller.last_transcription(), Some("paste this"));
    assert!(!controller.is_update_result_pending("live-dictation"));
}

#[tokio::test(start_paused = true)]
async fn empty_capture_and_blank_transcript_are_quiet_no_speech_results() {
    let empty_capture = FakeRuntime::new(
        false,
        Ok(Vec::new()),
        InferencePlan::Immediate(Ok("unused".into())),
        PastePlan::Return(Ok(())),
    );
    let generation = empty_capture.start_recording();
    run_started(&empty_capture, generation).await;
    assert_eq!(empty_capture.events(), transcribing_then_idle());
    assert_eq!(
        empty_capture.actions(),
        vec![
            UiAction::Tray("transcribing"),
            UiAction::Finish {
                last_result: None,
                open_copy_fallback: false,
            },
        ]
    );
    assert_eq!(
        empty_capture.observed.lock().unwrap().inference_audio.len(),
        0
    );
    assert_eq!(empty_capture.controller().state(), AppState::Idle);

    let blank_transcript = FakeRuntime::new(
        false,
        Ok(vec![0.1]),
        InferencePlan::Immediate(Ok(" \n\t ".into())),
        PastePlan::Return(Ok(())),
    );
    let generation = blank_transcript.start_recording();
    run_started(&blank_transcript, generation).await;
    assert_eq!(blank_transcript.events(), transcribing_then_idle());
    assert_eq!(
        blank_transcript.actions(),
        vec![
            UiAction::Tray("transcribing"),
            UiAction::Finish {
                last_result: None,
                open_copy_fallback: false,
            },
        ]
    );
    assert_eq!(
        blank_transcript
            .observed
            .lock()
            .unwrap()
            .paste_requests
            .len(),
        0
    );
    assert_eq!(blank_transcript.controller().state(), AppState::Idle);
}

#[tokio::test(start_paused = true)]
async fn cancelled_inference_is_quiet_and_keeps_the_previous_transcript() {
    let runtime = FakeRuntime::new(
        false,
        Ok(vec![0.2]),
        InferencePlan::Immediate(Err(DictationError::Cancelled)),
        PastePlan::Return(Ok(())),
    );
    runtime
        .controller()
        .preserve_transcription("previous result");
    let generation = runtime.start_recording();

    run_started(&runtime, generation).await;

    assert_eq!(runtime.events(), transcribing_then_idle());
    assert_eq!(
        runtime.actions(),
        vec![
            UiAction::Tray("transcribing"),
            UiAction::Finish {
                last_result: None,
                open_copy_fallback: false,
            },
        ]
    );
    let controller = runtime.controller();
    assert_eq!(controller.state(), AppState::Idle);
    assert_eq!(controller.last_error(), None);
    assert_eq!(controller.last_transcription(), Some("previous result"));
}

#[tokio::test(start_paused = true)]
async fn inference_failure_emits_typed_error_then_error_and_idle_states() {
    let runtime = FakeRuntime::new(
        false,
        Ok(vec![0.2]),
        InferencePlan::Immediate(Err(DictationError::TranscriptionFailed(
            "backend offline".into(),
        ))),
        PastePlan::Return(Ok(())),
    );
    let generation = runtime.start_recording();

    run_started(&runtime, generation).await;
    runtime.join_spawned().await;

    assert_eq!(
        runtime.events(),
        vec![
            expected_event("state-changed", json!(UiState::Transcribing)),
            expected_event(
                "error",
                json!({
                    "code": "transcription_failed",
                    "message": "Transcription failed: backend offline",
                    "retryable": false,
                })
            ),
            expected_event("state-changed", json!(UiState::Error)),
            expected_event("state-changed", json!(UiState::Idle)),
        ]
    );
    assert_eq!(
        runtime.actions(),
        vec![
            UiAction::Tray("transcribing"),
            UiAction::Tray("idle"),
            UiAction::HideOverlay
        ]
    );
    let controller = runtime.controller();
    assert_eq!(controller.state(), AppState::Idle);
    assert_eq!(
        controller.last_error(),
        Some("Transcription failed: backend offline")
    );
}

#[tokio::test(start_paused = true)]
async fn capture_stop_failure_emits_typed_error_without_starting_inference() {
    let runtime = FakeRuntime::new(
        false,
        Err(DictationError::AudioCaptureError(
            "device disappeared".into(),
        )),
        InferencePlan::Immediate(Ok("unused".into())),
        PastePlan::Return(Ok(())),
    );
    let generation = runtime.start_recording();

    run_started(&runtime, generation).await;

    assert_eq!(
        runtime.events(),
        vec![
            expected_event(
                "error",
                json!({
                    "code": "audio_capture_error",
                    "message": "Audio capture error: device disappeared",
                    "retryable": false,
                })
            ),
            expected_event("state-changed", json!(UiState::Idle)),
        ]
    );
    assert_eq!(
        runtime.actions(),
        vec![UiAction::Finish {
            last_result: None,
            open_copy_fallback: false,
        }]
    );
    assert_eq!(runtime.capture_counts(), (1, 1));
    assert!(runtime.observed.lock().unwrap().inference_audio.is_empty());
    assert_eq!(runtime.controller().state(), AppState::Idle);
}

#[tokio::test(start_paused = true)]
async fn duplicate_request_stop_runs_only_one_capture_stop_and_inference() {
    let runtime = FakeRuntime::new(
        false,
        Ok(vec![0.1, 0.2]),
        InferencePlan::Immediate(Ok("once".into())),
        PastePlan::Return(Ok(())),
    );
    runtime.start_recording();

    request_stop(runtime.clone());
    request_stop(runtime.clone());
    tokio::time::advance(Duration::from_millis(301)).await;
    runtime.join_spawned().await;

    assert_eq!(runtime.capture_counts(), (1, 1));
    assert_eq!(runtime.observed.lock().unwrap().inference_audio.len(), 1);
    assert_eq!(
        runtime.events(),
        vec![
            expected_event("state-changed", json!(UiState::Transcribing)),
            expected_event("transcription-result", json!("once")),
            expected_event("state-changed", json!(UiState::Idle)),
        ]
    );
    assert_eq!(runtime.controller().state(), AppState::Idle);
}

#[tokio::test(start_paused = true)]
async fn paste_failures_keep_recognized_text_and_choose_copy_fallback_by_outcome() {
    let cases = [
        (
            PastePlan::Return(Err("Accessibility permission denied".into())),
            "failed",
            "Accessibility permission denied",
            true,
            true,
        ),
        (
            PastePlan::DispatchFailure("main thread unavailable".into()),
            "dispatch_failed",
            "Could not dispatch automatic paste. Copy the recognized text from Dictate.",
            true,
            false,
        ),
        (
            PastePlan::DropCompletion,
            "completion_dropped",
            "Automatic paste did not report a result. Copy the recognized text from Dictate.",
            true,
            false,
        ),
        (
            PastePlan::Queue(Ok(())),
            "timed_out",
            "Automatic paste did not finish in time and may still complete. The recognized text is available in Dictate; check the editor before pasting again.",
            false,
            false,
        ),
    ];

    for (paste, outcome, error_message, fallback, paste_began) in cases {
        let runtime = FakeRuntime::new(
            true,
            Ok(vec![0.6]),
            InferencePlan::Immediate(Ok("recognized and retained".into())),
            paste,
        );
        runtime.controller().preserve_transcription("older text");
        let generation = runtime.start_recording();

        run_started(&runtime, generation).await;

        assert_eq!(
            runtime.events(),
            vec![
                expected_event("state-changed", json!(UiState::Transcribing)),
                expected_event("transcription-result", json!("recognized and retained")),
                expected_event(
                    "error",
                    json!({
                        "code": "paste_error",
                        "message": error_message,
                        "retryable": false,
                    })
                ),
                expected_event("state-changed", json!(UiState::Idle)),
            ],
            "paste outcome {outcome}"
        );
        assert_eq!(
            runtime.actions(),
            vec![
                UiAction::Tray("transcribing"),
                UiAction::Finish {
                    last_result: None,
                    open_copy_fallback: fallback,
                },
            ],
            "paste outcome {outcome}"
        );
        let observed = runtime.observed.lock().unwrap();
        assert_eq!(
            observed.paste_requests,
            if paste_began {
                vec!["recognized and retained".to_string()]
            } else {
                Vec::new()
            }
        );
        assert_eq!(observed.restore_paste_calls, 1);
        assert_eq!(
            observed
                .focus_logs
                .iter()
                .filter(|(name, _)| *name == "transcription_result_ready")
                .count(),
            1
        );
        drop(observed);
        assert_eq!(
            runtime.pending_paste_count(),
            if outcome == "timed_out" { 1 } else { 0 }
        );
        let controller = runtime.controller();
        assert_eq!(controller.state(), AppState::Idle);
        assert_eq!(
            controller.last_transcription(),
            Some("recognized and retained")
        );
        assert_eq!(controller.last_error(), Some(error_message));
        assert!(controller.is_update_result_pending("live-dictation"));
    }
}

#[tokio::test(start_paused = true)]
async fn queued_ui_callbacks_can_drain_while_the_same_generation_is_idle() {
    let runtime = FakeRuntime::new(
        false,
        Ok(Vec::new()),
        InferencePlan::Immediate(Ok("unused".into())),
        PastePlan::Return(Ok(())),
    );
    runtime.defer_ui_dispatch(true);
    let generation = runtime.start_recording();

    run_started(&runtime, generation).await;

    assert!(runtime.actions().is_empty());
    assert_eq!(runtime.controller().state(), AppState::Idle);
    assert_eq!(runtime.controller().recording_generation(), generation);

    runtime.drain_ui();

    assert_eq!(
        runtime.actions(),
        vec![
            UiAction::Tray("transcribing"),
            UiAction::Finish {
                last_result: None,
                open_copy_fallback: false,
            },
        ]
    );
    assert_eq!(runtime.controller().state(), AppState::Idle);
    assert_eq!(runtime.controller().recording_generation(), generation);
}

#[tokio::test(start_paused = true)]
async fn queued_paste_can_start_and_complete_before_its_deadline() {
    let runtime = FakeRuntime::new(
        true,
        Ok(vec![0.3, 0.4]),
        InferencePlan::Immediate(Ok("queued text".into())),
        PastePlan::Queue(Ok(())),
    );
    let generation = runtime.start_recording();
    let run_runtime = runtime.clone();
    let run_task = tokio::spawn(async move {
        run(run_runtime.clone(), None, generation, run_runtime.now_std()).await;
    });

    runtime.wait_for_paste_dispatch().await;
    assert_eq!(runtime.queued_paste_count(), 1);
    assert!(runtime.observed.lock().unwrap().paste_requests.is_empty());

    runtime.run_one_queued_paste();
    run_task.await.expect("pipeline task should finish");

    assert_eq!(runtime.queued_paste_count(), 0);
    assert_eq!(
        runtime.observed.lock().unwrap().paste_requests,
        ["queued text"]
    );
    assert_eq!(
        runtime.events(),
        vec![
            expected_event("state-changed", json!(UiState::Transcribing)),
            expected_event("transcription-result", json!("queued text")),
            expected_event("state-changed", json!(UiState::Idle)),
        ]
    );
    assert_eq!(
        runtime.controller().last_transcription(),
        Some("queued text")
    );
    assert_eq!(runtime.controller().state(), AppState::Idle);
}

#[tokio::test(start_paused = true)]
async fn inference_timeout_requests_abort_and_reports_timed_out_after_unwind() {
    let runtime = FakeRuntime::new(
        false,
        Ok(vec![0.8]),
        InferencePlan::PendingUntilAbort {
            finish_on_abort: true,
        },
        PastePlan::Return(Ok(())),
    );
    let generation = runtime.start_recording();

    run_started(&runtime, generation).await;
    runtime.join_spawned().await;

    assert_eq!(runtime.observed.lock().unwrap().abort_calls, 1);
    assert!(runtime.pending_inference_is_finished());
    assert_eq!(
        runtime.events(),
        vec![
            expected_event("state-changed", json!(UiState::Transcribing)),
            expected_event(
                "error",
                json!({
                    "code": "transcription_timed_out",
                    "message": "Transcription timed out after 60s (inference aborted)",
                    "retryable": false,
                })
            ),
            expected_event("state-changed", json!(UiState::Error)),
            expected_event("state-changed", json!(UiState::Idle)),
        ]
    );
    assert_eq!(runtime.controller().state(), AppState::Idle);
}

#[tokio::test(start_paused = true)]
async fn inference_timeout_reports_when_abort_does_not_unwind_in_grace() {
    let runtime = FakeRuntime::new(
        false,
        Ok(vec![0.8]),
        InferencePlan::PendingUntilAbort {
            finish_on_abort: false,
        },
        PastePlan::Return(Ok(())),
    );
    let generation = runtime.start_recording();

    run_started(&runtime, generation).await;

    assert_eq!(runtime.observed.lock().unwrap().abort_calls, 1);
    assert!(!runtime.pending_inference_is_finished());
    assert_eq!(
        runtime.events(),
        vec![
            expected_event("state-changed", json!(UiState::Transcribing)),
            expected_event(
                "error",
                json!({
                    "code": "transcription_timed_out",
                    "message": "Transcription timed out after 60s (inference aborted)",
                    "retryable": false,
                })
            ),
            expected_event("state-changed", json!(UiState::Error)),
            expected_event("state-changed", json!(UiState::Idle)),
        ]
    );
    assert_eq!(runtime.controller().state(), AppState::Idle);

    runtime.release_pending_inference();
    runtime.join_spawned().await;
}
