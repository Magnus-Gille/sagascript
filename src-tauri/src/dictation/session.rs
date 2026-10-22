//! One generation's terminal transition. Consuming the owner keeps each branch
//! on the same cleanup path; the controller check also rejects superseded work.

use sagascript_core::error::DictationError;
use sagascript_core::settings::SettingsError;
use tracing::{error, warn};

use super::{
    dictation_failure_event, dictation_failure_text, dispatch_ui, hide_overlay_after_error,
};
use crate::app_controller::{AppController, AppState};
use crate::command_error::CommandError;
use crate::dictation::runtime::{DictationRuntime, UiAction};
use crate::events::{self, UiState};
use crate::paste_completion;

pub(super) struct LiveSession {
    generation: u64,
}

pub(super) enum TerminalOutcome {
    /// Capture already restored Idle before returning its error.
    CaptureFailed(DictationError),
    EmptyAudio,
    NoSpeech,
    ModelSelectionFailed(SettingsError),
    Recognized {
        text: String,
        paste_outcome: &'static str,
        paste_error: Option<String>,
    },
    Cancelled,
    Failed(DictationError),
}

impl LiveSession {
    pub(super) fn new(generation: u64) -> Self {
        Self { generation }
    }

    pub(super) fn is_processing(&self, controller: &AppController) -> bool {
        controller.recording_generation() == self.generation
            && controller.state() == AppState::Transcribing
            && controller.active_hotkey_profile().is_some()
    }

    fn is_finished(&self, controller: &AppController) -> bool {
        controller.recording_generation() == self.generation && controller.state() == AppState::Idle
    }

    // Catalogue emits only enqueue their wire payload. Keep this short group
    // under the controller lock so a new start cannot interleave its state
    // event with an old result/error/idle sequence. Native UI calls stay outside.
    fn emit_terminal(&self, runtime: &impl DictationRuntime, emit: impl FnOnce()) {
        let controller = runtime.controller();
        if self.is_finished(&controller) {
            emit();
        }
    }

    pub(super) fn finish(
        self,
        runtime: &impl DictationRuntime,
        outcome: TerminalOutcome,
        performance: Option<serde_json::Value>,
    ) {
        let mut controller = runtime.controller();
        let owns_transition = if matches!(outcome, TerminalOutcome::CaptureFailed(_)) {
            self.is_finished(&controller)
        } else {
            self.is_processing(&controller)
        };
        if !owns_transition {
            return;
        }
        if let Some(performance) = performance {
            controller.log_dictation_performance(performance);
        }
        match &outcome {
            TerminalOutcome::CaptureFailed(_) => {}
            TerminalOutcome::EmptyAudio | TerminalOutcome::NoSpeech => {
                controller.on_no_speech_detected()
            }
            TerminalOutcome::ModelSelectionFailed(refusal) => {
                if let Err(error) = controller.finish_transcription(Err(refusal.to_string())) {
                    error!("Profile-model failure completion: {error}");
                }
            }
            TerminalOutcome::Recognized {
                text,
                paste_outcome,
                paste_error,
            } => {
                if let Some(message) = paste_error {
                    controller.preserve_transcription(text);
                    if let Err(error) = controller.set_update_result_pending("live-dictation", true)
                    {
                        warn!("Could not mark failed paste result for recovery: {error}");
                    }
                    controller.on_transcription_error(message);
                } else {
                    controller.on_transcription_success(text);
                    if *paste_outcome == "succeeded" {
                        if let Err(error) =
                            controller.set_update_result_pending("live-dictation", false)
                        {
                            warn!(
                                "Could not clear successfully pasted result from recovery: {error}"
                            );
                        }
                    }
                }
            }
            TerminalOutcome::Cancelled => controller.on_transcription_cancelled(),
            TerminalOutcome::Failed(error) => {
                controller.on_transcription_error(&dictation_failure_text(error))
            }
        }
        drop(controller);

        let quiet_finish = || {
            dispatch_ui(
                runtime,
                self.generation,
                UiAction::Finish {
                    last_result: None,
                    open_copy_fallback: false,
                },
            )
        };
        match outcome {
            TerminalOutcome::CaptureFailed(error) => {
                quiet_finish();
                self.emit_terminal(runtime, || {
                    let _ =
                        runtime.emit_event(events::event::ERROR, &dictation_failure_event(&error));
                    let _ = runtime.emit_event(events::event::STATE_CHANGED, UiState::Idle);
                });
            }
            TerminalOutcome::ModelSelectionFailed(refusal) => {
                quiet_finish();
                self.emit_terminal(runtime, || {
                    let _ = runtime.emit_event(events::event::STATE_CHANGED, UiState::Idle);
                    let _ =
                        runtime.emit_event(events::event::ERROR, &CommandError::Settings(refusal));
                });
            }
            TerminalOutcome::EmptyAudio => {
                quiet_finish();
                self.emit_terminal(runtime, || {
                    let _ = runtime.emit_event(events::event::STATE_CHANGED, UiState::Idle);
                });
            }
            TerminalOutcome::NoSpeech | TerminalOutcome::Cancelled => {
                self.emit_terminal(runtime, || {
                    let _ = runtime.emit_event(events::event::STATE_CHANGED, UiState::Idle);
                });
                quiet_finish();
            }
            TerminalOutcome::Recognized {
                text,
                paste_outcome,
                paste_error,
            } => {
                if let Some(message) = paste_error {
                    runtime.focus_log("result_emitted_to_ui", serde_json::json!({ "outcome": "paste_error", "pasteOutcome": paste_outcome }));
                    self.emit_terminal(runtime, || {
                        let _ = runtime.emit_event(events::event::TRANSCRIPTION_RESULT, &text);
                        let _ = runtime
                            .emit_event(events::event::ERROR, &CommandError::PasteFailed(message));
                        let _ = runtime.emit_event(events::event::STATE_CHANGED, UiState::Idle);
                    });
                    dispatch_ui(
                        runtime,
                        self.generation,
                        UiAction::Finish {
                            last_result: None,
                            open_copy_fallback: paste_completion::should_open_copy_fallback(
                                paste_outcome,
                            ),
                        },
                    );
                } else {
                    self.emit_terminal(runtime, || {
                        let _ = runtime.emit_event(events::event::TRANSCRIPTION_RESULT, &text);
                        let _ = runtime.emit_event(events::event::STATE_CHANGED, UiState::Idle);
                    });
                    dispatch_ui(
                        runtime,
                        self.generation,
                        UiAction::Finish {
                            last_result: Some(text),
                            open_copy_fallback: false,
                        },
                    );
                }
            }
            TerminalOutcome::Failed(error) => {
                self.emit_terminal(runtime, || {
                    let _ =
                        runtime.emit_event(events::event::ERROR, &dictation_failure_event(&error));
                    let _ = runtime.emit_event(events::event::STATE_CHANGED, UiState::Error);
                    let _ = runtime.emit_event(events::event::STATE_CHANGED, UiState::Idle);
                });
                dispatch_ui(runtime, self.generation, UiAction::Tray("idle"));
                hide_overlay_after_error(runtime.clone(), self.generation);
            }
        }
    }
}
