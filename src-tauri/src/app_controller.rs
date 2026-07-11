use std::time::{Duration, Instant};

use serde::Serialize;
use tracing::{info, warn};

use sagascript_core::audio::AudioCaptureService;
use sagascript_core::error::DictationError;
use crate::hotkey::HotkeyService;
use crate::logging::LoggingService;
use crate::logging::log_events;
use crate::paste::PasteService;
use sagascript_core::settings::{HotkeyMode, Settings};

/// Result of handling a hotkey-down event
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyDownResult {
    /// Recording was started
    StartedRecording,
    /// Toggle mode: recording should be stopped (second press)
    StopRecording,
    /// No action taken (e.g. already transcribing)
    NoOp,
}

/// Outcome of a guarded stop-recording request (see
/// [`AppController::stop_recording_guarded`]).
#[derive(Debug)]
pub enum StopRecordingOutcome {
    /// Not currently recording — the stop was ignored (guards against a
    /// duplicate/late stop racing an in-flight transcription). State and
    /// `last_error` are left untouched.
    NotRecording,
    /// Recording stopped; carries the captured 16 kHz samples (may be empty if
    /// the mic produced only silence).
    Stopped(Vec<f32>),
    /// The capture/resample failed. The controller has recorded the error and
    /// returned to Idle; the message is returned so the caller can surface it
    /// via the transcription-error event path.
    Failed(String),
}

/// Application state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AppState {
    Idle,
    Recording,
    Transcribing,
    #[allow(dead_code)]
    Error,
}

impl AppState {
    pub fn is_recording(&self) -> bool {
        matches!(self, AppState::Recording)
    }

    #[allow(dead_code)]
    pub fn is_busy(&self) -> bool {
        matches!(self, AppState::Recording | AppState::Transcribing)
    }
}

/// Central coordinator for the dictation workflow
pub struct AppController {
    state: AppState,
    audio: AudioCaptureService,
    #[allow(dead_code)]
    paste: PasteService,
    hotkey: HotkeyService,
    logging: LoggingService,
    settings: Settings,
    recording_start: Option<Instant>,
    last_transcription: Option<String>,
    last_error: Option<String>,
    model_ready: bool,
}

impl AppController {
    pub fn new(settings: Settings) -> Self {
        let logging = LoggingService::new();

        info!("Sagascript starting up...");
        logging.log(
            "info",
            "App",
            log_events::app::STARTED,
            serde_json::json!({ "appSessionId": logging.app_session_id }),
        );

        Self {
            state: AppState::Idle,
            audio: AudioCaptureService::new(),
            paste: PasteService::new(),
            hotkey: HotkeyService::new(),
            logging,
            settings,
            recording_start: None,
            last_transcription: None,
            last_error: None,
            model_ready: false,
        }
    }

    pub fn state(&self) -> AppState {
        self.state
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn settings_mut(&mut self) -> &mut Settings {
        &mut self.settings
    }

    pub fn hotkey_service_mut(&mut self) -> &mut HotkeyService {
        &mut self.hotkey
    }

    pub fn last_transcription(&self) -> Option<&str> {
        self.last_transcription.as_deref()
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn is_model_ready(&self) -> bool {
        self.model_ready
    }

    pub fn language(&self) -> sagascript_core::settings::Language {
        self.settings.language
    }

    /// Handle hotkey down event
    pub fn handle_hotkey_down(&mut self) -> Result<HotkeyDownResult, DictationError> {
        info!("Hotkey DOWN");

        match self.settings.hotkey_mode {
            HotkeyMode::PushToTalk => {
                // Only report StartedRecording if we actually started. Holding
                // PTT while a prior utterance is still Transcribing must be a
                // no-op — otherwise the overlay/tray shows a recording that
                // never happened and never hides (finding 1).
                if self.start_recording()? {
                    Ok(HotkeyDownResult::StartedRecording)
                } else {
                    Ok(HotkeyDownResult::NoOp)
                }
            }
            HotkeyMode::Toggle => {
                if self.state.is_recording() {
                    Ok(HotkeyDownResult::StopRecording)
                } else if self.state == AppState::Idle {
                    self.start_recording()?;
                    Ok(HotkeyDownResult::StartedRecording)
                } else {
                    Ok(HotkeyDownResult::NoOp)
                }
            }
        }
    }

    /// Handle hotkey up event
    pub fn should_stop_on_key_up(&self) -> bool {
        self.settings.hotkey_mode == HotkeyMode::PushToTalk && self.state.is_recording()
    }

    /// Start audio recording.
    ///
    /// Returns `Ok(true)` if recording actually started, `Ok(false)` if it was
    /// refused because the controller is not idle (e.g. a previous utterance is
    /// still transcribing). Callers use this to avoid reporting a recording that
    /// never happened (finding 1).
    pub fn start_recording(&mut self) -> Result<bool, DictationError> {
        if self.state != AppState::Idle {
            warn!("Cannot start recording: state is {:?}", self.state);
            return Ok(false);
        }

        let session_id = self.logging.start_dictation_session();
        self.logging.log(
            "info",
            "App",
            log_events::session::DICTATION_STARTED,
            serde_json::json!({ "dictationSessionId": session_id }),
        );

        self.audio.start_capture()?;
        self.state = AppState::Recording;
        self.recording_start = Some(Instant::now());
        self.last_error = None;

        info!("Recording started");
        Ok(true)
    }

    /// Stop recording and return the captured 16 kHz samples.
    ///
    /// Propagates a capture/resample failure (finding 4) instead of masking it
    /// as an empty buffer, so a real device/format error can reach the user
    /// rather than being reported as "No audio captured". On error the state is
    /// left as `Recording`; callers surface the error and return to Idle (see
    /// [`Self::stop_recording_guarded`]).
    pub fn stop_recording(&mut self) -> Result<Vec<f32>, DictationError> {
        let samples = self.audio.stop_capture()?;
        let duration = self
            .recording_start
            .map(|s| s.elapsed().as_millis())
            .unwrap_or(0);

        info!(
            "Recording stopped: {} samples ({duration}ms)",
            samples.len()
        );

        self.state = AppState::Transcribing;
        Ok(samples)
    }

    /// Stop recording only if currently recording, mapping a capture/resample
    /// failure onto the error state. Combines the finding-3 guard (a late or
    /// duplicate stop racing an in-flight transcription must not clobber state)
    /// and the finding-4 error surfacing (a real capture error is recorded and
    /// the controller returns to Idle so it reaches the user).
    pub fn stop_recording_guarded(&mut self) -> StopRecordingOutcome {
        if !self.state.is_recording() {
            return StopRecordingOutcome::NotRecording;
        }
        match self.stop_recording() {
            Ok(samples) => StopRecordingOutcome::Stopped(samples),
            Err(e) => {
                warn!("Recording stop failed: {e}");
                let msg = e.to_string();
                // Records last_error and returns to Idle.
                self.on_transcription_error(&msg);
                StopRecordingOutcome::Failed(msg)
            }
        }
    }

    /// Called after transcription succeeds
    pub fn on_transcription_success(&mut self, text: &str) {
        self.last_transcription = Some(text.to_string());
        self.audio.clear_last_captured();
        self.state = AppState::Idle;
        self.logging.end_dictation_session();
    }

    /// Called after transcription fails
    pub fn on_transcription_error(&mut self, error: &str) {
        self.last_error = Some(error.to_string());
        self.state = AppState::Idle;
        self.logging.end_dictation_session();
    }

    /// Complete a transcription attempt and restore the controller to Idle.
    ///
    /// Keeping this transition in the state machine prevents callers from
    /// accidentally returning early on model-load, task, or timeout failures
    /// while leaving the app permanently stuck in `Transcribing`.
    pub fn finish_transcription(
        &mut self,
        result: Result<String, String>,
    ) -> Result<String, String> {
        match result {
            Ok(text) => {
                self.on_transcription_success(&text);
                Ok(text)
            }
            Err(error) => {
                self.on_transcription_error(&error);
                Err(error)
            }
        }
    }

    /// Auto-paste text if enabled
    #[allow(dead_code)]
    pub fn auto_paste(&self, text: &str) -> Result<(), DictationError> {
        if !self.settings.auto_paste {
            return Ok(());
        }
        self.paste.paste(text)
    }

    /// Cancel recording without transcribing
    pub fn cancel_recording(&mut self) {
        if self.state.is_recording() {
            let _ = self.audio.stop_capture();
            self.state = AppState::Idle;
            self.logging.end_dictation_session();
            info!("Recording cancelled");
        }
    }

    /// How long we've been recording
    pub fn recording_elapsed(&self) -> Duration {
        self.recording_start
            .map(|s| s.elapsed())
            .unwrap_or(Duration::ZERO)
    }

    #[allow(dead_code)]
    pub fn set_model_ready(&mut self, ready: bool) {
        self.model_ready = ready;
    }

    /// Update settings
    pub fn update_settings(&mut self, settings: Settings) {
        self.settings = settings;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_controller() -> AppController {
        AppController::new(Settings::default())
    }

    // -- AppState --

    #[test]
    fn app_state_is_recording() {
        assert!(!AppState::Idle.is_recording());
        assert!(AppState::Recording.is_recording());
        assert!(!AppState::Transcribing.is_recording());
        assert!(!AppState::Error.is_recording());
    }

    #[test]
    fn app_state_is_busy() {
        assert!(!AppState::Idle.is_busy());
        assert!(AppState::Recording.is_busy());
        assert!(AppState::Transcribing.is_busy());
        assert!(!AppState::Error.is_busy());
    }

    #[test]
    fn app_state_serializes() {
        let json = serde_json::to_string(&AppState::Idle).unwrap();
        assert_eq!(json, "\"idle\"");
        assert_eq!(serde_json::to_string(&AppState::Recording).unwrap(), "\"recording\"");
        assert_eq!(serde_json::to_string(&AppState::Transcribing).unwrap(), "\"transcribing\"");
        assert_eq!(serde_json::to_string(&AppState::Error).unwrap(), "\"error\"");
    }

    // -- AppController initial state --

    #[test]
    fn initial_state_is_idle() {
        let ctrl = default_controller();
        assert_eq!(ctrl.state(), AppState::Idle);
    }

    #[test]
    fn initial_model_not_ready() {
        let ctrl = default_controller();
        assert!(!ctrl.is_model_ready());
    }

    #[test]
    fn initial_no_transcription() {
        let ctrl = default_controller();
        assert!(ctrl.last_transcription().is_none());
    }

    #[test]
    fn initial_no_error() {
        let ctrl = default_controller();
        assert!(ctrl.last_error().is_none());
    }

    #[test]
    fn initial_language_from_settings() {
        let settings = Settings { language: sagascript_core::settings::Language::Swedish, ..Default::default() };
        let ctrl = AppController::new(settings);
        assert_eq!(ctrl.language(), sagascript_core::settings::Language::Swedish);
    }

    // -- Settings --

    #[test]
    fn settings_getter() {
        let ctrl = default_controller();
        assert_eq!(ctrl.settings().language, sagascript_core::settings::Language::English);
    }

    #[test]
    fn settings_mut_modifiable() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().language = sagascript_core::settings::Language::Norwegian;
        assert_eq!(ctrl.settings().language, sagascript_core::settings::Language::Norwegian);
    }

    #[test]
    fn update_settings_replaces() {
        let mut ctrl = default_controller();
        let new_settings = Settings { auto_paste: false, language: sagascript_core::settings::Language::Swedish, ..Default::default() };
        ctrl.update_settings(new_settings);
        assert!(!ctrl.settings().auto_paste);
        assert_eq!(ctrl.settings().language, sagascript_core::settings::Language::Swedish);
    }

    // -- Model ready --

    #[test]
    fn set_model_ready() {
        let mut ctrl = default_controller();
        assert!(!ctrl.is_model_ready());
        ctrl.set_model_ready(true);
        assert!(ctrl.is_model_ready());
        ctrl.set_model_ready(false);
        assert!(!ctrl.is_model_ready());
    }

    // -- Transcription callbacks --

    #[test]
    fn on_transcription_success_stores_text() {
        let mut ctrl = default_controller();
        ctrl.state = AppState::Transcribing;
        ctrl.on_transcription_success("Hello world");
        assert_eq!(ctrl.last_transcription(), Some("Hello world"));
        assert_eq!(ctrl.state(), AppState::Idle);
    }

    #[test]
    fn on_transcription_error_stores_error() {
        let mut ctrl = default_controller();
        ctrl.state = AppState::Transcribing;
        ctrl.on_transcription_error("model crashed");
        assert_eq!(ctrl.last_error(), Some("model crashed"));
        assert_eq!(ctrl.state(), AppState::Idle);
    }

    #[test]
    fn finish_transcription_failure_returns_idle_and_preserves_error() {
        let mut ctrl = default_controller();
        ctrl.state = AppState::Transcribing;

        let result = ctrl.finish_transcription(Err("model failed to load".to_string()));

        assert_eq!(result, Err("model failed to load".to_string()));
        assert_eq!(ctrl.last_error(), Some("model failed to load"));
        assert_eq!(ctrl.state(), AppState::Idle);
    }

    #[test]
    fn finish_transcription_success_returns_idle_and_preserves_text() {
        let mut ctrl = default_controller();
        ctrl.state = AppState::Transcribing;

        let result = ctrl.finish_transcription(Ok("Hello again".to_string()));

        assert_eq!(result, Ok("Hello again".to_string()));
        assert_eq!(ctrl.last_transcription(), Some("Hello again"));
        assert_eq!(ctrl.state(), AppState::Idle);
    }

    // -- Recording elapsed --

    #[test]
    fn recording_elapsed_zero_when_not_recording() {
        let ctrl = default_controller();
        assert_eq!(ctrl.recording_elapsed(), Duration::ZERO);
    }

    // -- should_stop_on_key_up --

    #[test]
    fn should_stop_on_key_up_push_to_talk_recording() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().hotkey_mode = HotkeyMode::PushToTalk;
        ctrl.state = AppState::Recording;
        assert!(ctrl.should_stop_on_key_up());
    }

    #[test]
    fn should_not_stop_on_key_up_push_to_talk_idle() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().hotkey_mode = HotkeyMode::PushToTalk;
        ctrl.state = AppState::Idle;
        assert!(!ctrl.should_stop_on_key_up());
    }

    #[test]
    fn should_not_stop_on_key_up_toggle_mode() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().hotkey_mode = HotkeyMode::Toggle;
        ctrl.state = AppState::Recording;
        assert!(!ctrl.should_stop_on_key_up());
    }

    // -- handle_hotkey_down --

    #[test]
    fn toggle_mode_returns_stop_when_recording() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().hotkey_mode = HotkeyMode::Toggle;
        ctrl.state = AppState::Recording;
        let result = ctrl.handle_hotkey_down().unwrap();
        assert_eq!(result, HotkeyDownResult::StopRecording);
    }

    #[test]
    fn toggle_mode_returns_noop_when_transcribing() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().hotkey_mode = HotkeyMode::Toggle;
        ctrl.state = AppState::Transcribing;
        let result = ctrl.handle_hotkey_down().unwrap();
        assert_eq!(result, HotkeyDownResult::NoOp);
    }

    // Finding 1: in push-to-talk mode a hotkey-down while a prior utterance is
    // still transcribing must NOT report StartedRecording (start_recording
    // refuses when state != Idle) — otherwise the overlay/tray shows a recording
    // that never happened and never hides.
    #[test]
    fn push_to_talk_down_when_transcribing_is_noop() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().hotkey_mode = HotkeyMode::PushToTalk;
        ctrl.state = AppState::Transcribing;
        let result = ctrl.handle_hotkey_down().unwrap();
        assert_eq!(result, HotkeyDownResult::NoOp);
    }

    // -- auto_paste --

    #[test]
    fn auto_paste_disabled_is_noop() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().auto_paste = false;
        // Should return Ok without attempting paste
        assert!(ctrl.auto_paste("test").is_ok());
    }

    // -- cancel_recording --

    #[test]
    fn cancel_recording_when_not_recording_is_noop() {
        let mut ctrl = default_controller();
        ctrl.cancel_recording();
        assert_eq!(ctrl.state(), AppState::Idle);
    }

    // -- start_recording when not idle --

    #[test]
    fn start_recording_when_transcribing_is_noop() {
        let mut ctrl = default_controller();
        ctrl.state = AppState::Transcribing;
        // Finding 1: refused start reports `false` (did not actually start).
        let started = ctrl.start_recording().unwrap();
        assert!(!started);
        assert_eq!(ctrl.state(), AppState::Transcribing); // unchanged
    }

    // -- stop_recording_guarded --

    // Finding 3: a stop that races an in-flight transcription (state !=
    // Recording) must be a no-op — it must not transition state nor set
    // last_error, so the running transcription is not clobbered.
    #[test]
    fn stop_recording_guarded_when_not_recording_is_noop() {
        let mut ctrl = default_controller();
        ctrl.state = AppState::Transcribing;
        let outcome = ctrl.stop_recording_guarded();
        assert!(matches!(outcome, StopRecordingOutcome::NotRecording));
        assert_eq!(ctrl.state(), AppState::Transcribing); // unchanged
        assert!(ctrl.last_error().is_none());
    }

    // Finding 4: a guarded stop from the Recording state returns the captured
    // samples (here empty — no real capture in the test) and transitions to
    // Transcribing. Exercises the Result plumbing added for finding 4.
    #[test]
    fn stop_recording_guarded_from_recording_returns_stopped() {
        let mut ctrl = default_controller();
        ctrl.state = AppState::Recording;
        match ctrl.stop_recording_guarded() {
            StopRecordingOutcome::Stopped(samples) => assert!(samples.is_empty()),
            other => panic!("expected Stopped, got {other:?}"),
        }
        assert_eq!(ctrl.state(), AppState::Transcribing);
    }
}
