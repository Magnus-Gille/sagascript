use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use sagascript_core::audio::capture::AudioCaptureMetrics;
use sagascript_core::error::DictationError;
use sagascript_core::settings::{
    FileModelPreference, HotkeyMode, HotkeyProfile, Language, Settings, WhisperModel,
};
use sagascript_core::transcription::whisper_backend::DictationTimings;
use serde_json::{json, Value};
use tokio::sync::oneshot;
use tokio::sync::Notify;
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
    start_result: Option<Result<(), DictationError>>,
}

struct FakeCapture(Arc<Mutex<CaptureState>>);

impl Capture for FakeCapture {
    fn start_capture(&mut self) -> Result<(), DictationError> {
        let mut state = self.0.lock().unwrap();
        state.starts += 1;
        state.start_result.take().unwrap_or(Ok(()))
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
    Panic,
}

#[derive(Debug, Clone, PartialEq)]
struct CapturedInferenceRequest {
    model: WhisperModel,
    language: Language,
    beam_size: u32,
    temperature_fallback: bool,
    prompt: Option<String>,
    audio: Vec<f32>,
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
    inference_requests: Vec<CapturedInferenceRequest>,
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
    inference_started: Arc<Notify>,
    inference_finished: Arc<Notify>,
    sleep_started: Arc<Notify>,
    sleep_calls: usize,
}

impl Default for Observed {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            actions: Vec::new(),
            focus_logs: Vec::new(),
            inference_audio: Vec::new(),
            inference_requests: Vec::new(),
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
            inference_started: Arc::new(Notify::new()),
            inference_finished: Arc::new(Notify::new()),
            sleep_started: Arc::new(Notify::new()),
            sleep_calls: 0,
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
    update_activity: Arc<crate::updater::activity::UpdateActivity>,
    log_path: Arc<std::path::PathBuf>,
    model_warm: Arc<AtomicBool>,
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
        let log_path = std::env::temp_dir()
            .join(format!("sagascript-lifecycle-{}", uuid::Uuid::new_v4()))
            .join("sagascript.log");
        let logging = crate::logging::LoggingService::new_with_path(log_path.clone());
        let mut controller = AppController::with_services(
            Settings {
                auto_paste,
                ..Settings::default()
            },
            FakeCapture(capture.clone()),
            logging,
        );
        let update_activity = Arc::new(crate::updater::activity::UpdateActivity::default());
        controller.set_update_activity(update_activity.clone());
        Self {
            controller: Arc::new(Mutex::new(controller)),
            capture,
            observed: Arc::new(Mutex::new(Observed::default())),
            inference: Arc::new(Mutex::new(inference)),
            paste: Arc::new(Mutex::new(paste)),
            update_activity,
            log_path: Arc::new(log_path),
            model_warm: Arc::new(AtomicBool::new(true)),
        }
    }

    fn session_logs(&self) -> Vec<Value> {
        std::fs::read_to_string(self.log_path.as_ref())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|entry| entry["event"] == "dictation_session_finished")
            .collect()
    }

    fn start_recording(&self) -> u64 {
        let mut controller = self.controller();
        let profile = controller.settings().resolved_hotkey_profiles()[0].clone();
        assert!(controller.start_recording_for_profile(profile).unwrap());
        assert_eq!(controller.state(), AppState::Recording);
        controller.recording_generation()
    }

    fn start_binding(&self, profile: HotkeyProfile, mode: HotkeyMode, shortcut: &str) -> u64 {
        assert_eq!(
            self.controller()
                .handle_hotkey_down_for_binding(profile, mode, shortcut)
                .unwrap(),
            crate::app_controller::HotkeyDownResult::StartedRecording
        );
        self.controller().recording_generation()
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

    async fn wait_for_inference_count(&self, count: usize) {
        loop {
            let notified = self.observed.lock().unwrap().inference_started.clone();
            let wait = notified.notified();
            tokio::pin!(wait);
            wait.as_mut().enable();
            if self.observed.lock().unwrap().inference_requests.len() >= count {
                return;
            }
            wait.await;
        }
    }

    async fn wait_for_sleep_count(&self, count: usize) {
        loop {
            let (notified, calls) = {
                let observed = self.observed.lock().unwrap();
                (observed.sleep_started.clone(), observed.sleep_calls)
            };
            let wait = notified.notified();
            tokio::pin!(wait);
            wait.as_mut().enable();
            if calls >= count {
                return;
            }
            wait.await;
        }
    }

    async fn wait_for_inference_completion_count(&self, count: usize) {
        loop {
            let (notified, completed) = {
                let observed = self.observed.lock().unwrap();
                (
                    observed.inference_finished.clone(),
                    observed
                        .inference_completed
                        .iter()
                        .filter(|flag| flag.load(Ordering::SeqCst))
                        .count(),
                )
            };
            let wait = notified.notified();
            tokio::pin!(wait);
            wait.as_mut().enable();
            if completed >= count {
                return;
            }
            wait.await;
        }
    }

    fn complete_pending_inference(&self, index: usize, result: Result<String, DictationError>) {
        let sender = self
            .observed
            .lock()
            .unwrap()
            .pending_inference_senders
            .get(index)
            .expect("pending inference sender should exist")
            .clone();
        let sender = sender.lock().unwrap().take();
        if let Some(sender) = sender {
            sender
                .send(result)
                .expect("pending inference receiver should remain open");
        }
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
        let observed = self.observed.clone();
        async move {
            let notify = {
                let mut observed = observed.lock().unwrap();
                observed.sleep_calls += 1;
                observed.sleep_started.clone()
            };
            notify.notify_one();
            tokio::time::sleep(duration).await;
        }
    }

    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        self.observed
            .lock()
            .unwrap()
            .spawned
            .push(tokio::spawn(task));
    }

    fn model_is_warm(&self, _model: WhisperModel, _use_pianissimo: bool) -> bool {
        self.model_warm.load(Ordering::SeqCst)
    }

    fn start_inference(&self, request: InferenceRequest) -> InferenceTask {
        let plan = self.inference.lock().unwrap().clone();
        let captured = CapturedInferenceRequest {
            model: request.model,
            language: request.language,
            beam_size: request.options.beam_size,
            temperature_fallback: request.options.temperature_fallback,
            prompt: request.options.prompt.clone(),
            audio: request.audio.clone(),
        };
        let audio = request.audio;
        let observed = self.observed.clone();
        let (started, completed_notify) = {
            let mut observed = observed.lock().unwrap();
            observed.inference_audio.push(audio.clone());
            observed.inference_requests.push(captured);
            (
                observed.inference_started.clone(),
                observed.inference_finished.clone(),
            )
        };
        started.notify_one();

        match plan {
            InferencePlan::Immediate(result) => InferenceTask {
                task: tokio::spawn(async move {
                    let output = (result, DictationTimings::default());
                    completed_notify.notify_one();
                    output
                }),
                abort: Box::new(|| {}),
            },
            InferencePlan::Panic => InferenceTask {
                task: tokio::spawn(async move { panic!("fake inference task panic") }),
                abort: Box::new(|| {}),
            },
            InferencePlan::PendingUntilAbort { finish_on_abort } => {
                let (sender, receiver) = oneshot::channel();
                let completed = Arc::new(AtomicBool::new(false));
                let completed_task = completed.clone();
                let finished_task = completed_notify.clone();
                let task = tokio::spawn(async move {
                    let result = receiver.await.unwrap_or(Err(DictationError::Cancelled));
                    completed_task.store(true, Ordering::SeqCst);
                    finished_task.notify_one();
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
async fn ptt_and_toggle_sessions_select_their_profile_request_with_each_autopaste_setting() {
    let profiles = vec![
        HotkeyProfile {
            id: "english".into(),
            name: "English".into(),
            shortcut: "Alt+E".into(),
            language: Language::English,
            push_to_talk_shortcut: Some("Alt+E".into()),
            toggle_shortcut: Some("Alt+Shift+E".into()),
        },
        HotkeyProfile {
            id: "norwegian".into(),
            name: "Norwegian".into(),
            shortcut: "Alt+N".into(),
            language: Language::Norwegian,
            push_to_talk_shortcut: None,
            toggle_shortcut: None,
        },
    ];
    for (profile_index, mode, auto_paste, expected_model, shortcut) in [
        (
            0,
            HotkeyMode::PushToTalk,
            false,
            WhisperModel::SmallEn,
            "Alt+E",
        ),
        (
            0,
            HotkeyMode::Toggle,
            true,
            WhisperModel::SmallEn,
            "Alt+Shift+E",
        ),
        (
            1,
            HotkeyMode::PushToTalk,
            true,
            WhisperModel::NbWhisperTiny,
            "Alt+N",
        ),
        (
            1,
            HotkeyMode::Toggle,
            false,
            WhisperModel::NbWhisperTiny,
            "Alt+N",
        ),
    ] {
        let runtime = FakeRuntime::new(
            auto_paste,
            Ok(vec![0.4, 0.5]),
            InferencePlan::Immediate(Ok("profile words".into())),
            PastePlan::Return(Ok(())),
        );
        {
            let mut controller = runtime.controller();
            let settings = controller.settings_mut();
            settings.hotkey_profiles = profiles.clone();
            settings.profile_models.insert(
                "english".into(),
                FileModelPreference::Whisper(WhisperModel::SmallEn),
            );
            settings.profile_models.insert(
                "norwegian".into(),
                FileModelPreference::Whisper(WhisperModel::NbWhisperTiny),
            );
            settings.auto_paste = auto_paste;
        }
        let generation = runtime.start_binding(profiles[profile_index].clone(), mode, shortcut);
        request_stop(runtime.clone());
        runtime.wait_for_sleep_count(1).await;
        tokio::time::advance(Duration::from_millis(301)).await;
        runtime.wait_for_inference_count(1).await;
        runtime.join_spawned().await;

        let observed = runtime.observed.lock().unwrap();
        assert_eq!(observed.inference_requests.len(), 1);
        let request = &observed.inference_requests[0];
        assert_eq!(request.model, expected_model);
        assert_eq!(request.language, profiles[profile_index].language);
        assert_eq!(request.audio, vec![0.4, 0.5]);
        assert_eq!(
            observed.paste_requests,
            if auto_paste {
                vec!["profile words"]
            } else {
                vec![]
            }
        );
        drop(observed);
        assert_eq!(
            runtime.controller().state(),
            AppState::Idle,
            "generation {generation}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn standalone_stop_is_noop_and_public_hotkey_edges_stop_each_mode_once() {
    let idle = FakeRuntime::new(
        false,
        Ok(vec![0.1]),
        InferencePlan::Immediate(Ok("unused".into())),
        PastePlan::Return(Ok(())),
    );
    request_stop(idle.clone());
    idle.wait_for_sleep_count(1).await;
    tokio::time::advance(Duration::from_millis(301)).await;
    idle.join_spawned().await;
    assert_eq!(idle.capture_counts(), (0, 0));
    assert!(idle.events().is_empty());

    let ptt = FakeRuntime::new(
        false,
        Ok(vec![0.2]),
        InferencePlan::Immediate(Ok("ptt".into())),
        PastePlan::Return(Ok(())),
    );
    let ptt_profile = ptt.controller().settings().resolved_hotkey_profiles()[0].clone();
    let generation = ptt.start_binding(
        ptt_profile.clone(),
        HotkeyMode::PushToTalk,
        &ptt_profile.shortcut,
    );
    assert_eq!(
        ptt.controller()
            .handle_hotkey_down_for_binding(
                ptt_profile.clone(),
                HotkeyMode::PushToTalk,
                &ptt_profile.shortcut
            )
            .unwrap(),
        crate::app_controller::HotkeyDownResult::NoOp
    );
    assert!(ptt
        .controller()
        .should_stop_profile_on_key_up(&ptt_profile.shortcut));
    ptt.controller().note_hotkey_release(&ptt_profile.shortcut);
    request_stop(ptt.clone());
    ptt.wait_for_sleep_count(1).await;
    tokio::time::advance(Duration::from_millis(301)).await;
    ptt.join_spawned().await;
    assert_eq!(ptt.capture_counts(), (1, 1));
    assert_eq!(ptt.observed.lock().unwrap().inference_requests.len(), 1);
    assert_eq!(ptt.controller().recording_generation(), generation);
    assert!(!ptt
        .controller()
        .should_stop_profile_on_key_up(&ptt_profile.shortcut));

    let toggle = FakeRuntime::new(
        false,
        Ok(vec![0.3]),
        InferencePlan::Immediate(Ok("toggle".into())),
        PastePlan::Return(Ok(())),
    );
    let profile = toggle.controller().settings().resolved_hotkey_profiles()[0].clone();
    assert_eq!(
        toggle.start_binding(profile.clone(), HotkeyMode::Toggle, &profile.shortcut),
        1
    );
    assert_eq!(
        toggle
            .controller()
            .handle_hotkey_down_for_binding(profile.clone(), HotkeyMode::Toggle, &profile.shortcut)
            .unwrap(),
        crate::app_controller::HotkeyDownResult::NoOp,
    );
    toggle.controller().note_hotkey_release(&profile.shortcut);
    assert_eq!(
        toggle
            .controller()
            .handle_hotkey_down_for_binding(profile.clone(), HotkeyMode::Toggle, &profile.shortcut)
            .unwrap(),
        crate::app_controller::HotkeyDownResult::StopRecording,
    );
    assert_eq!(
        toggle
            .controller()
            .handle_hotkey_down_for_binding(profile.clone(), HotkeyMode::Toggle, &profile.shortcut)
            .unwrap(),
        crate::app_controller::HotkeyDownResult::NoOp,
    );
    request_stop(toggle.clone());
    toggle.wait_for_sleep_count(1).await;
    tokio::time::advance(Duration::from_millis(301)).await;
    toggle.join_spawned().await;
    assert_eq!(toggle.capture_counts(), (1, 1));
    assert_eq!(toggle.observed.lock().unwrap().inference_requests.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn delayed_old_stop_cannot_stop_a_new_recording_generation() {
    let runtime = FakeRuntime::new(
        false,
        Ok(vec![0.1]),
        InferencePlan::Immediate(Ok("unused".into())),
        PastePlan::Return(Ok(())),
    );
    let old_generation = runtime.start_recording();
    request_stop(runtime.clone());
    runtime.wait_for_sleep_count(1).await;
    runtime.controller().cancel_recording();
    let new_generation = runtime.start_recording();
    assert_ne!(old_generation, new_generation);

    tokio::time::advance(Duration::from_millis(301)).await;
    runtime.join_spawned().await;
    assert_eq!(runtime.controller().state(), AppState::Recording);
    assert_eq!(runtime.controller().recording_generation(), new_generation);
    assert_eq!(runtime.capture_counts(), (2, 1));
    assert!(runtime.events().is_empty());
    runtime.controller().cancel_recording();
}

#[tokio::test(start_paused = true)]
async fn model_selection_refusal_and_inference_join_error_emit_exact_terminal_events() {
    let refusal = FakeRuntime::new(
        false,
        Ok(vec![0.2]),
        InferencePlan::Immediate(Ok("unused".into())),
        PastePlan::Return(Ok(())),
    );
    let generation = refusal.start_recording();
    {
        let mut controller = refusal.controller();
        controller.settings_mut().language = Language::Swedish;
        controller.settings_mut().profile_models.insert(
            "default".into(),
            FileModelPreference::Whisper(WhisperModel::TinyEn),
        );
    }
    request_stop(refusal.clone());
    refusal.wait_for_sleep_count(1).await;
    tokio::time::advance(Duration::from_millis(301)).await;
    refusal.join_spawned().await;
    assert_eq!(refusal.capture_counts(), (1, 1));
    assert!(refusal
        .observed
        .lock()
        .unwrap()
        .inference_requests
        .is_empty());
    assert_eq!(refusal.controller().state(), AppState::Idle);
    assert_eq!(refusal.events().len(), 3);
    assert_eq!(
        refusal.events()[0],
        expected_event("state-changed", json!(UiState::Transcribing))
    );
    assert_eq!(
        refusal.events()[1],
        expected_event("state-changed", json!(UiState::Idle))
    );
    assert_eq!(refusal.events()[2].name, "error");
    assert_eq!(refusal.events()[2].payload["code"], "model_incompatible");
    assert_eq!(
        refusal.actions(),
        vec![
            UiAction::Tray("transcribing"),
            UiAction::Finish {
                last_result: None,
                open_copy_fallback: false
            }
        ]
    );
    assert_eq!(refusal.controller().recording_generation(), generation);

    let failed_task = FakeRuntime::new(
        false,
        Ok(vec![0.2]),
        InferencePlan::Panic,
        PastePlan::Return(Ok(())),
    );
    failed_task.start_recording();
    request_stop(failed_task.clone());
    failed_task.wait_for_sleep_count(1).await;
    tokio::time::advance(Duration::from_millis(301)).await;
    failed_task.wait_for_inference_count(1).await;
    failed_task.join_spawned().await;
    let events = failed_task.events();
    assert_eq!(events.len(), 4);
    assert_eq!(
        events[0],
        expected_event("state-changed", json!(UiState::Transcribing))
    );
    assert_eq!(events[1].name, "error");
    assert_eq!(events[1].payload["code"], "task_failed");
    let message = events[1].payload["message"]
        .as_str()
        .expect("join error message should be a string");
    assert!(message.starts_with("Task join error: task "), "{message}");
    assert!(message.ends_with(" panicked with message \"fake inference task panic\""));
    assert_eq!(events[1].payload["retryable"], false);
    assert_eq!(
        events[2],
        expected_event("state-changed", json!(UiState::Error))
    );
    assert_eq!(
        events[3],
        expected_event("state-changed", json!(UiState::Idle))
    );
    assert_eq!(failed_task.controller().state(), AppState::Idle);
}

#[tokio::test(start_paused = true)]
async fn timed_out_old_inference_finishing_after_new_session_cannot_change_it() {
    let runtime = FakeRuntime::new(
        false,
        Ok(vec![0.2]),
        InferencePlan::PendingUntilAbort {
            finish_on_abort: false,
        },
        PastePlan::Return(Ok(())),
    );
    runtime.start_recording();
    request_stop(runtime.clone());
    runtime.wait_for_sleep_count(1).await;
    tokio::time::advance(Duration::from_millis(301)).await;
    runtime.wait_for_inference_count(1).await;
    tokio::time::advance(Duration::from_secs(65)).await;
    runtime.join_spawned().await;
    assert_eq!(runtime.controller().state(), AppState::Idle);
    assert_eq!(runtime.update_activity.active_work_count(), 0);
    let old_events = runtime.events();
    assert_eq!(old_events.len(), 4);
    assert_eq!(old_events[1].name, "error");
    assert_eq!(old_events[1].payload["code"], "transcription_timed_out");

    *runtime.inference.lock().unwrap() = InferencePlan::Immediate(Ok("new session text".into()));
    runtime.start_recording();
    request_stop(runtime.clone());
    runtime.wait_for_sleep_count(2).await;
    tokio::time::advance(Duration::from_millis(301)).await;
    runtime.wait_for_inference_count(2).await;
    runtime.join_spawned().await;
    assert_eq!(
        runtime.controller().last_transcription(),
        Some("new session text")
    );
    let before_late_completion = runtime.events();
    assert_eq!(before_late_completion.len(), 7);
    let leases_before_late_completion = (
        runtime.update_activity.active_work_count(),
        runtime.update_activity.pending_result_count(),
    );

    runtime.complete_pending_inference(0, Ok("stale old transcript".into()));
    runtime.wait_for_inference_completion_count(1).await;
    assert_eq!(runtime.controller().state(), AppState::Idle);
    assert_eq!(
        runtime.controller().last_transcription(),
        Some("new session text")
    );
    assert_eq!(runtime.events(), before_late_completion);
    assert_eq!(
        (
            runtime.update_activity.active_work_count(),
            runtime.update_activity.pending_result_count(),
        ),
        leases_before_late_completion
    );
    assert_eq!(
        runtime.actions().last(),
        Some(&UiAction::Finish {
            last_result: Some("new session text".into()),
            open_copy_fallback: false
        })
    );
    assert_eq!(runtime.capture_counts(), (2, 2));
}

#[tokio::test(start_paused = true)]
async fn pipeline_reads_model_options_after_release_and_autopaste_after_inference_while_binding_stays_frozen(
) {
    let runtime = FakeRuntime::new(
        true,
        Ok(vec![0.8]),
        InferencePlan::PendingUntilAbort {
            finish_on_abort: false,
        },
        PastePlan::Return(Ok(())),
    );
    let profile = HotkeyProfile::legacy_default("Alt+E".into(), Language::English);
    {
        let mut controller = runtime.controller();
        controller.settings_mut().hotkey_profiles = vec![profile.clone()];
        controller.settings_mut().profile_models.insert(
            "default".into(),
            FileModelPreference::Whisper(WhisperModel::BaseEn),
        );
        controller.settings_mut().initial_prompt = "old prompt".into();
    }
    runtime.start_binding(profile.clone(), HotkeyMode::PushToTalk, &profile.shortcut);
    request_stop(runtime.clone());
    runtime.wait_for_sleep_count(1).await;
    {
        let mut controller = runtime.controller();
        controller.settings_mut().profile_models.insert(
            "default".into(),
            FileModelPreference::Whisper(WhisperModel::SmallEn),
        );
        controller.settings_mut().beam_size = 4;
        controller.settings_mut().temperature_fallback = false;
        controller.settings_mut().initial_prompt = "updated prompt".into();
        controller.settings_mut().auto_paste = true;
        controller.settings_mut().hotkey = "Alt+X".into();
        controller.settings_mut().hotkey_profiles[0].shortcut = "Alt+X".into();
        assert!(controller.should_stop_profile_on_key_up("Alt+E"));
        assert!(!controller.should_stop_profile_on_key_up("Alt+X"));
    }
    tokio::time::advance(Duration::from_millis(301)).await;
    runtime.wait_for_inference_count(1).await;
    {
        let observed = runtime.observed.lock().unwrap();
        let request = &observed.inference_requests[0];
        assert_eq!(request.model, WhisperModel::SmallEn);
        assert_eq!(request.language, Language::English);
        assert_eq!(request.beam_size, 4);
        assert!(!request.temperature_fallback);
        assert_eq!(request.prompt.as_deref(), Some("updated prompt"));
    }
    runtime.controller().settings_mut().auto_paste = false;
    runtime.complete_pending_inference(0, Ok("late option text".into()));
    runtime.wait_for_inference_completion_count(1).await;
    runtime.join_spawned().await;
    assert!(runtime.observed.lock().unwrap().paste_requests.is_empty());
    assert_eq!(
        runtime.controller().last_transcription(),
        Some("late option text")
    );
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
        vec![UiAction::Finish {
            last_result: None,
            open_copy_fallback: false,
        },]
    );
    assert_eq!(runtime.controller().state(), AppState::Idle);
    assert_eq!(runtime.controller().recording_generation(), generation);
}

#[tokio::test(start_paused = true)]
async fn stale_queued_ui_cannot_hide_or_relabel_a_new_recording() {
    let runtime = FakeRuntime::new(
        false,
        Ok(vec![0.3]),
        InferencePlan::Immediate(Ok("first result".into())),
        PastePlan::Return(Ok(())),
    );
    runtime.defer_ui_dispatch(true);
    let first = runtime.start_recording();
    run_started(&runtime, first).await;
    let second = runtime.start_recording();
    assert_ne!(first, second);

    runtime.drain_ui();

    assert!(
        runtime.actions().is_empty(),
        "old queued tray/overlay work must be discarded"
    );
    assert_eq!(runtime.controller().state(), AppState::Recording);
    assert_eq!(runtime.capture_counts(), (2, 1));
}

#[tokio::test(start_paused = true)]
async fn stale_paste_callback_after_timeout_cannot_paste_into_a_new_session() {
    let runtime = FakeRuntime::new(
        true,
        Ok(vec![0.3]),
        InferencePlan::Immediate(Ok("old timed-out text".into())),
        PastePlan::Queue(Ok(())),
    );
    let first = runtime.start_recording();
    run_started(&runtime, first).await;
    assert_eq!(runtime.controller().state(), AppState::Idle);
    assert_eq!(runtime.queued_paste_count(), 1);
    let events = runtime.events();
    let second = runtime.start_recording();
    assert_ne!(first, second);

    runtime.run_one_queued_paste();

    assert!(
        runtime.observed.lock().unwrap().paste_requests.is_empty(),
        "a queued old paste must not begin after its deadline and a new start"
    );
    assert_eq!(runtime.events(), events);
    assert_eq!(runtime.controller().state(), AppState::Recording);
    assert_eq!(
        runtime.controller().last_transcription(),
        Some("old timed-out text")
    );
    assert!(runtime
        .controller()
        .is_update_result_pending("live-dictation"));
}

#[tokio::test(start_paused = true)]
async fn stale_error_overlay_callback_rechecks_when_main_thread_executes_it() {
    let runtime = FakeRuntime::new(
        false,
        Ok(vec![0.3]),
        InferencePlan::Immediate(Err(DictationError::TranscriptionFailed(
            "old failure".into(),
        ))),
        PastePlan::Return(Ok(())),
    );
    let first = runtime.start_recording();
    run_started(&runtime, first).await;
    runtime.observed.lock().unwrap().actions.clear();
    runtime.defer_ui_dispatch(true);
    runtime.join_spawned().await; // Timer expired while idle; native callback is still queued.
    let second = runtime.start_recording();
    assert_ne!(first, second);

    runtime.drain_ui();

    assert!(
        runtime.actions().is_empty(),
        "the timer's idle check cannot authorize a later native hide"
    );
    assert_eq!(runtime.controller().state(), AppState::Recording);
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

#[tokio::test(start_paused = true)]
async fn every_terminal_path_logs_once_releases_its_work_and_allows_the_next_session() {
    let cases = [
        (
            false,
            Ok(vec![0.1]),
            InferencePlan::Immediate(Ok("private test words".into())),
            PastePlan::Return(Ok(())),
            "success",
            true,
        ),
        (
            true,
            Ok(vec![0.1]),
            InferencePlan::Immediate(Ok("private test words".into())),
            PastePlan::Return(Ok(())),
            "success",
            false,
        ),
        (
            true,
            Ok(vec![0.1]),
            InferencePlan::Immediate(Ok("private test words".into())),
            PastePlan::Return(Err("private paste error".into())),
            "error",
            true,
        ),
        (
            false,
            Ok(vec![]),
            InferencePlan::Immediate(Ok("unused".into())),
            PastePlan::Return(Ok(())),
            "no_speech",
            false,
        ),
        (
            false,
            Ok(vec![0.1]),
            InferencePlan::Immediate(Ok(" \n ".into())),
            PastePlan::Return(Ok(())),
            "no_speech",
            false,
        ),
        (
            false,
            Ok(vec![0.1]),
            InferencePlan::Immediate(Err(DictationError::Cancelled)),
            PastePlan::Return(Ok(())),
            "cancelled",
            false,
        ),
        (
            false,
            Err(DictationError::AudioCaptureError(
                "private capture error".into(),
            )),
            InferencePlan::Immediate(Ok("unused".into())),
            PastePlan::Return(Ok(())),
            "error",
            false,
        ),
        (
            false,
            Ok(vec![0.1]),
            InferencePlan::Immediate(Err(DictationError::TranscriptionFailed(
                "private model error".into(),
            ))),
            PastePlan::Return(Ok(())),
            "error",
            false,
        ),
        (
            false,
            Ok(vec![0.1]),
            InferencePlan::PendingUntilAbort {
                finish_on_abort: true,
            },
            PastePlan::Return(Ok(())),
            "error",
            false,
        ),
    ];
    for (auto_paste, capture, inference, paste, outcome, pending) in cases {
        let runtime = FakeRuntime::new(auto_paste, capture, inference, paste);
        runtime.controller().settings_mut().initial_prompt = "private glossary context".into();
        let generation = runtime.start_recording();
        assert_eq!(runtime.update_activity.active_work_count(), 1);
        run_started(&runtime, generation).await;
        assert_eq!(runtime.controller().state(), AppState::Idle);
        assert!(runtime.controller().active_hotkey_profile().is_none());
        assert_eq!(runtime.update_activity.active_work_count(), 0, "{outcome}");
        assert_eq!(
            runtime.update_activity.pending_result_count(),
            usize::from(pending),
            "{outcome}"
        );
        let logs = runtime.session_logs();
        assert_eq!(
            logs.len(),
            1,
            "one actual JSONL session record for {outcome}"
        );
        assert_eq!(logs[0]["data"]["outcome"], outcome);
        assert!(logs[0]["dictationSession"]
            .as_str()
            .unwrap()
            .starts_with("dict-"));
        let serialized = serde_json::to_string(&logs[0]["data"]).unwrap();
        assert!(
            !serialized.contains("private"),
            "session payload leaked content: {serialized}"
        );
        let data = logs[0]["data"].as_object().unwrap();
        for key in data.keys() {
            assert!(
                [
                    "version",
                    "git_hash",
                    "language",
                    "model",
                    "audio_ms",
                    "phases_ms",
                    "auto_paste",
                    "outcome",
                    "model_cached",
                    "key_up_to_completion_ms"
                ]
                .contains(&key.as_str()),
                "unexpected session payload field {key}"
            );
        }
        let terminal_events = runtime.events();
        // A duplicate scheduled stop must not create another terminal log/event.
        run_started(&runtime, generation).await;
        assert_eq!(runtime.session_logs(), logs);
        assert_eq!(runtime.events(), terminal_events);
        let next = runtime.start_recording();
        assert_ne!(next, generation);
        assert_eq!(runtime.update_activity.active_work_count(), 1);
        runtime.controller().cancel_recording();
        assert_eq!(runtime.update_activity.active_work_count(), 0);
        assert_eq!(
            runtime.update_activity.pending_result_count(),
            usize::from(pending)
        );
        let after = runtime.session_logs();
        assert_eq!(after.len(), 2);
        assert_eq!(after[1]["data"]["outcome"], "cancelled");
        assert_ne!(after[0]["dictationSession"], after[1]["dictationSession"]);
        runtime.join_spawned().await;
    }
}

#[tokio::test(start_paused = true)]
async fn failed_capture_start_logs_once_without_consuming_generation_or_leaking_work() {
    let runtime = FakeRuntime::new(
        false,
        Ok(vec![0.1]),
        InferencePlan::Immediate(Ok("next session".into())),
        PastePlan::Return(Ok(())),
    );
    runtime.capture.lock().unwrap().start_result =
        Some(Err(DictationError::MicrophonePermissionDenied));
    assert!(matches!(
        runtime.controller().start_recording(),
        Err(DictationError::MicrophonePermissionDenied)
    ));
    assert_eq!(runtime.controller().recording_generation(), 0);
    assert_eq!(runtime.controller().state(), AppState::Idle);
    assert_eq!(runtime.update_activity.active_work_count(), 0);
    assert_eq!(runtime.session_logs().len(), 1);
    assert_eq!(runtime.session_logs()[0]["data"]["outcome"], "error");
    let generation = runtime.start_recording();
    assert_eq!(generation, 1);
    run_started(&runtime, generation).await;
    assert_eq!(runtime.capture_counts(), (2, 1));
    assert_eq!(runtime.session_logs().len(), 2);
    assert_eq!(runtime.session_logs()[1]["data"]["outcome"], "success");
    assert_eq!(runtime.update_activity.active_work_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn cold_model_loading_precedes_result_and_clears_the_loading_tray() {
    let runtime = FakeRuntime::new(
        false,
        Ok(vec![0.1]),
        InferencePlan::Immediate(Ok("cold model text".into())),
        PastePlan::Return(Ok(())),
    );
    runtime.model_warm.store(false, Ordering::SeqCst);
    let generation = runtime.start_recording();
    run_started(&runtime, generation).await;
    assert_eq!(
        runtime.events(),
        vec![
            expected_event("state-changed", json!(UiState::Transcribing)),
            expected_event("state-changed", json!(UiState::LoadingModel)),
            expected_event("transcription-result", json!("cold model text")),
            expected_event("state-changed", json!(UiState::Idle)),
        ]
    );
    assert_eq!(
        runtime.actions(),
        vec![
            UiAction::Tray("transcribing"),
            UiAction::Tray("loading_model"),
            UiAction::Finish {
                last_result: Some("cold model text".into()),
                open_copy_fallback: false
            }
        ]
    );
    assert_eq!(runtime.capture_counts(), (1, 1));
    assert_eq!(runtime.update_activity.active_work_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn old_terminal_ui_is_discarded_after_a_new_session_also_finishes() {
    let runtime = FakeRuntime::new(
        false,
        Ok(vec![0.1]),
        InferencePlan::Immediate(Ok("old result".into())),
        PastePlan::Return(Ok(())),
    );
    runtime.defer_ui_dispatch(true);
    let old = runtime.start_recording();
    run_started(&runtime, old).await;
    runtime.defer_ui_dispatch(false); // Leave the older callbacks queued.
    *runtime.inference.lock().unwrap() = InferencePlan::Immediate(Ok("new result".into()));
    let new = runtime.start_recording();
    run_started(&runtime, new).await;
    assert_eq!(runtime.controller().state(), AppState::Idle);
    assert_eq!(
        runtime.controller().last_transcription(),
        Some("new result")
    );
    let latest_ui = runtime.actions();
    runtime.drain_ui();
    assert_eq!(
        runtime.actions(),
        latest_ui,
        "old Finish must not overwrite the newer tray result even when both sessions are idle"
    );
    assert_eq!(runtime.session_logs().len(), 2);
}
