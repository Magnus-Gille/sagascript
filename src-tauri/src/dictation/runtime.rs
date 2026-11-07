//! External effects of the live session. The pipeline uses this same boundary
//! with the desktop adapters and with deterministic capture/inference/queue tests.

use std::future::Future;
use std::sync::MutexGuard;
use std::time::{Duration, Instant};

use sagascript_core::error::DictationError;
use sagascript_core::settings::{Language, WhisperModel};
use sagascript_core::transcription::whisper_backend::DictationTimings;
use sagascript_core::transcription::TranscribeOptions;
use tauri::Manager;
use tracing::{error, info};

use crate::app_controller::AppController;
use crate::commands::{lock_controller, SharedController, SharedWhisper};
use crate::events::{EmitEvent, Event};
use crate::launch::open_settings_window;
use crate::tray::{update_tray_last_result, update_tray_status};
use crate::{dispatch_to_main, focus_diag, overlay};

pub(super) struct InferenceRequest {
    pub model: WhisperModel,
    pub audio: Vec<f32>,
    pub language: Language,
    pub options: TranscribeOptions,
    pub use_pianissimo: bool,
}

pub(super) struct InferenceTask {
    pub task: tokio::task::JoinHandle<(Result<String, DictationError>, DictationTimings)>,
    pub abort: Box<dyn FnOnce() + Send>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum UiAction {
    Tray(&'static str),
    Finish {
        last_result: Option<String>,
        open_copy_fallback: bool,
    },
    HideOverlay,
}

pub(super) trait DictationRuntime: Clone + Send + Sync + 'static {
    fn controller(&self) -> MutexGuard<'_, AppController>;
    fn emit_event<E: Event>(&self, event: E, payload: E::Payload<'_>) -> tauri::Result<()>;
    fn dispatch(&self, task: impl FnOnce(Self) + Send + 'static);
    /// Called on the UI thread while the controller guard protects the checked
    /// generation. Must not reacquire that guard or wait for a worker.
    fn apply_ui(&self, action: UiAction);
    fn focus_log(&self, event: &'static str, data: serde_json::Value);
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send;
    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static);
    fn model_is_warm(&self, model: WhisperModel, use_pianissimo: bool) -> bool;
    fn start_inference(&self, request: InferenceRequest) -> InferenceTask;
    fn restore_paste_target(&self) -> impl Future<Output = ()> + Send;
    fn dispatch_paste(&self, task: impl FnOnce(Self) + Send + 'static) -> Result<(), String>;
    fn paste(&self, text: &str) -> Result<(), String>;
}

#[derive(Clone)]
pub(super) struct DesktopRuntime(pub tauri::AppHandle);

impl DictationRuntime for DesktopRuntime {
    fn controller(&self) -> MutexGuard<'_, AppController> {
        lock_controller(self.0.state::<SharedController>().inner())
    }

    fn emit_event<E: Event>(&self, event: E, payload: E::Payload<'_>) -> tauri::Result<()> {
        self.0.emit_event(event, payload)
    }

    fn dispatch(&self, task: impl FnOnce(Self) + Send + 'static) {
        dispatch_to_main(&self.0, move |app| task(DesktopRuntime(app.clone())));
    }

    fn apply_ui(&self, action: UiAction) {
        let app = &self.0;
        match action {
            UiAction::Tray(status) => update_tray_status(app, status),
            UiAction::Finish {
                last_result,
                open_copy_fallback,
            } => {
                overlay::hide(app);
                update_tray_status(app, "idle");
                if let Some(text) = last_result {
                    update_tray_last_result(app, &text);
                }
                if open_copy_fallback {
                    open_settings_window(app, Some("dictate"), "paste_failed_copy_fallback");
                }
            }
            UiAction::HideOverlay => overlay::hide(app),
        }
    }

    fn focus_log(&self, event: &'static str, data: serde_json::Value) {
        focus_diag::log(event, data);
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }

    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        tauri::async_runtime::spawn(task);
    }

    fn model_is_warm(&self, model: WhisperModel, use_pianissimo: bool) -> bool {
        if use_pianissimo {
            sagascript_core::transcription::pianissimo_backend::engine_is_warm()
        } else {
            !self.0.state::<SharedWhisper>().needs_reload(model)
        }
    }

    fn start_inference(&self, request: InferenceRequest) -> InferenceTask {
        let backend = std::sync::Arc::new(
            sagascript_core::transcription::live_dictation::LiveDictationBackend::new(
                self.0.state::<SharedWhisper>().inner().clone(),
                request.use_pianissimo,
                sagascript_core::machine::load,
            ),
        );
        let task_backend = backend.clone();
        let task = tokio::task::spawn_blocking(move || {
            let mut timings = DictationTimings::default();
            let result = task_backend.transcribe(
                request.model,
                &request.audio,
                request.language,
                &request.options,
                &mut timings,
            );
            (result, timings)
        });
        InferenceTask {
            task,
            abort: Box::new(move || backend.request_abort()),
        }
    }

    async fn restore_paste_target(&self) {
        #[cfg(target_os = "macos")]
        super::pipeline::restore_target_before_paste(&self.0).await;
    }

    fn dispatch_paste(&self, task: impl FnOnce(Self) + Send + 'static) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            let app = self.0.clone();
            self.0
                .run_on_main_thread(move || task(DesktopRuntime(app)))
                .map_err(|error| error.to_string())?;
        }
        #[cfg(not(target_os = "macos"))]
        {
            // Preserve the existing blocking-worker boundary on Windows/Linux.
            let runtime = self.clone();
            tokio::task::spawn_blocking(move || task(runtime));
        }
        Ok(())
    }

    fn paste(&self, text: &str) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        let result = crate::paste::PasteService::new()
            .paste_checked(text, || {
                let check = crate::platform::macos::dictation_paste_guard_check();
                focus_diag::log(
                    "paste_guard_check",
                    serde_json::json!({
                        "decision": if check.valid { "allow" } else { "reject" },
                        "reason": check.reason,
                        "targetPid": check.target_pid,
                        "currentFrontmostPid": check.current_pid,
                        "ownPid": check.own_pid,
                        "frontmostIsSagascript": check.current_pid == Some(check.own_pid),
                        "frontmostMatchesTarget": check.current_pid == Some(check.target_pid),
                    }),
                );
                check.valid
            })
            .map_err(|error| error.to_string());
        #[cfg(not(target_os = "macos"))]
        let result = crate::paste::PasteService::new()
            .paste(text)
            .map_err(|error| error.to_string());
        match &result {
            Ok(()) => info!("Auto-paste completed successfully"),
            Err(e) => error!("Auto-paste failed: {e}"),
        }
        result
    }
}
