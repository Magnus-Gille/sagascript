use std::time::{Duration, Instant};

use serde::Serialize;
use tracing::{info, warn};

use crate::audio::AudioCaptureService;
use crate::error::DictationError;
use crate::hotkey::HotkeyService;
use crate::logging::LoggingService;
use crate::paste::PasteService;
use crate::settings::{HotkeyMode, Settings};

/// Application state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AppState {
    Idle,
    Recording,
    Transcribing,
    Error,
}

impl AppState {
    pub fn is_recording(&self) -> bool {
        matches!(self, AppState::Recording)
    }

    pub fn is_busy(&self) -> bool {
        matches!(self, AppState::Recording | AppState::Transcribing)
    }
}

/// Central coordinator for the dictation workflow
pub struct AppController {
    state: AppState,
    audio: AudioCaptureService,
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

        info!("FlowDictate starting up...");
        logging.log(
            "info",
            "App",
            "app_started",
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

    pub fn last_transcription(&self) -> Option<&str> {
        self.last_transcription.as_deref()
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn is_model_ready(&self) -> bool {
        self.model_ready
    }

    pub fn language(&self) -> crate::settings::Language {
        self.settings.language
    }

    /// Handle hotkey down event
    pub fn handle_hotkey_down(&mut self) -> Result<(), DictationError> {
        info!("Hotkey DOWN");

        match self.settings.hotkey_mode {
            HotkeyMode::PushToTalk => self.start_recording(),
            HotkeyMode::Toggle => {
                if self.state.is_recording() {
                    Ok(())
                } else if self.state == AppState::Idle {
                    self.start_recording()
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Handle hotkey up event
    pub fn should_stop_on_key_up(&self) -> bool {
        self.settings.hotkey_mode == HotkeyMode::PushToTalk && self.state.is_recording()
    }

    /// Start audio recording
    pub fn start_recording(&mut self) -> Result<(), DictationError> {
        if self.state != AppState::Idle {
            warn!("Cannot start recording: state is {:?}", self.state);
            return Ok(());
        }

        let session_id = self.logging.start_dictation_session();
        self.logging.log(
            "info",
            "App",
            "dictation_session_started",
            serde_json::json!({ "dictationSessionId": session_id }),
        );

        self.audio.start_capture()?;
        self.state = AppState::Recording;
        self.recording_start = Some(Instant::now());
        self.last_error = None;

        info!("Recording started");
        Ok(())
    }

    /// Stop recording and return the audio samples
    pub fn stop_recording(&mut self) -> Vec<f32> {
        let samples = self.audio.stop_capture();
        let duration = self
            .recording_start
            .map(|s| s.elapsed().as_millis())
            .unwrap_or(0);

        info!(
            "Recording stopped: {} samples ({duration}ms)",
            samples.len()
        );

        self.state = AppState::Transcribing;
        samples
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

    /// Auto-paste text if enabled
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
        let mut settings = Settings::default();
        settings.language = crate::settings::Language::Swedish;
        let ctrl = AppController::new(settings);
        assert_eq!(ctrl.language(), crate::settings::Language::Swedish);
    }

    // -- Settings --

    #[test]
    fn settings_getter() {
        let ctrl = default_controller();
        assert_eq!(ctrl.settings().language, crate::settings::Language::English);
    }

    #[test]
    fn settings_mut_modifiable() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().language = crate::settings::Language::Norwegian;
        assert_eq!(ctrl.settings().language, crate::settings::Language::Norwegian);
    }

    #[test]
    fn update_settings_replaces() {
        let mut ctrl = default_controller();
        let mut new_settings = Settings::default();
        new_settings.auto_paste = false;
        new_settings.language = crate::settings::Language::Swedish;
        ctrl.update_settings(new_settings);
        assert!(!ctrl.settings().auto_paste);
        assert_eq!(ctrl.settings().language, crate::settings::Language::Swedish);
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
        let result = ctrl.start_recording();
        assert!(result.is_ok());
        assert_eq!(ctrl.state(), AppState::Transcribing); // unchanged
    }
}
