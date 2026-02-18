use std::time::{Duration, Instant};

use serde::Serialize;
use tracing::{info, warn};

use crate::audio::AudioCaptureService;
use crate::credentials::KeyringService;
use crate::error::DictationError;
use crate::hotkey::HotkeyService;
use crate::logging::LoggingService;
use crate::paste::PasteService;
use crate::settings::{HotkeyMode, Settings, TranscriptionBackendType};

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
    keyring: KeyringService,
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
            keyring: KeyringService::new(),
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

    pub fn keyring(&self) -> &KeyringService {
        &self.keyring
    }

    pub fn backend(&self) -> TranscriptionBackendType {
        self.settings.backend
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
