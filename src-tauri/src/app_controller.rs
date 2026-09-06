use std::time::{Duration, Instant};

use serde::Serialize;
use tracing::{info, warn};

use sagascript_core::audio::AudioCaptureService;
use sagascript_core::error::DictationError;
use crate::hotkey::HotkeyService;
use crate::logging::LoggingService;
use crate::logging::log_events;
use crate::paste::PasteService;
use sagascript_core::settings::{canonical_hotkey, HotkeyMode, HotkeyProfile, Settings};

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
    active_hotkey_profile: Option<HotkeyProfile>,
    active_hotkey_mode: Option<HotkeyMode>,
    presenter_owned: bool,
    recording_generation: u64,
    hotkey_configuration_changing: bool,
    training_recording: bool,
    session_data: Option<serde_json::Value>,
    release_started: Option<Instant>,
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
            active_hotkey_profile: None,
            active_hotkey_mode: None,
            presenter_owned: false,
            recording_generation: 0,
            hotkey_configuration_changing: false,
            training_recording: false,
            session_data: None,
            release_started: None,
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
        self.active_hotkey_profile
            .as_ref()
            .map(|profile| profile.language)
            .unwrap_or(self.settings.language)
    }

    pub fn active_hotkey_profile(&self) -> Option<&HotkeyProfile> {
        self.active_hotkey_profile.as_ref()
    }

    /// Handle hotkey down event
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn handle_hotkey_down(&mut self) -> Result<HotkeyDownResult, DictationError> {
        let profile = self
            .settings
            .resolved_hotkey_profiles()
            .into_iter()
            .find(|profile| profile.id == "default")
            .or_else(|| self.settings.resolved_hotkey_profiles().into_iter().next())
            .expect("resolved profiles always contains at least one profile");
        self.handle_hotkey_down_for_profile(profile)
    }

    pub fn handle_hotkey_down_for_profile(
        &mut self,
        profile: HotkeyProfile,
    ) -> Result<HotkeyDownResult, DictationError> {
        info!("Hotkey DOWN");

        match self.session_hotkey_mode() {
            HotkeyMode::PushToTalk => {
                // Only report StartedRecording if we actually started. Holding
                // PTT while a prior utterance is still Transcribing must be a
                // no-op — otherwise the overlay/tray shows a recording that
                // never happened and never hides (finding 1).
                if self.start_recording_for_profile(profile)? {
                    Ok(HotkeyDownResult::StartedRecording)
                } else {
                    Ok(HotkeyDownResult::NoOp)
                }
            }
            HotkeyMode::Toggle => {
                if self.state.is_recording()
                    && !self.training_recording
                    && self
                        .active_hotkey_profile
                        .as_ref()
                        .map(|active| active.id == profile.id)
                        .unwrap_or(true)
                {
                    Ok(HotkeyDownResult::StopRecording)
                } else if self.state == AppState::Idle {
                    if self.start_recording_for_profile(profile)? {
                        Ok(HotkeyDownResult::StartedRecording)
                    } else {
                        Ok(HotkeyDownResult::NoOp)
                    }
                } else {
                    Ok(HotkeyDownResult::NoOp)
                }
            }
            HotkeyMode::Presenter => {
                // Atomic Start is never a toggle. Finish has its own action;
                // repeated Start, training and in-flight transcription are no-ops.
                if self.start_recording_for_profile(profile)? {
                    Ok(HotkeyDownResult::StartedRecording)
                } else {
                    Ok(HotkeyDownResult::NoOp)
                }
            }
        }
    }

    fn session_hotkey_mode(&self) -> HotkeyMode {
        if self.state.is_busy() {
            self.active_hotkey_mode.unwrap_or(self.settings.hotkey_mode)
        } else {
            self.settings.hotkey_mode
        }
    }

    pub fn should_finish_presenter(&self) -> bool {
        self.is_presenter_session() && self.state.is_recording()
    }

    pub fn recording_generation(&self) -> u64 {
        self.recording_generation
    }

    pub fn is_presenter_session(&self) -> bool {
        self.state.is_busy()
            && self.presenter_owned
            && self.active_hotkey_mode == Some(HotkeyMode::Presenter)
            && !self.training_recording
    }

    /// Only the validated registration-failure fallback caller may use this.
    /// Keep the invalid saved configuration visible, but make this one capture
    /// releasable by the exact fallback key that actually started it.
    pub fn use_safe_fallback_lifecycle(&mut self) {
        if self.state.is_recording() && !self.training_recording {
            self.active_hotkey_mode = Some(HotkeyMode::PushToTalk);
            if let Some(profile) = &mut self.active_hotkey_profile {
                profile.shortcut = crate::SAFE_FALLBACK_HOTKEY.to_string();
            }
        }
    }

    pub fn stop_recording_generation(&mut self, generation: u64) -> StopRecordingOutcome {
        if self.recording_generation != generation {
            StopRecordingOutcome::NotRecording
        } else {
            self.stop_recording_guarded()
        }
    }

    /// Prevent a recording from starting halfway through an OS registration
    /// transaction, without holding the controller mutex across native calls.
    pub fn begin_hotkey_configuration_change(&mut self) -> bool {
        if self.state != AppState::Idle || self.hotkey_configuration_changing {
            return false;
        }
        self.hotkey_configuration_changing = true;
        true
    }

    pub fn end_hotkey_configuration_change(&mut self) {
        self.hotkey_configuration_changing = false;
    }

    /// Handle hotkey up event
    pub fn should_stop_on_key_up(&self) -> bool {
        self.session_hotkey_mode() == HotkeyMode::PushToTalk
            && self.state.is_recording()
            && !self.training_recording
    }

    pub fn should_stop_profile_on_key_up(&self, shortcut: &str) -> bool {
        self.should_stop_on_key_up()
            && self
                .active_hotkey_profile
                .as_ref()
                .and_then(|active| {
                    Some(canonical_hotkey(&active.shortcut).ok()? == canonical_hotkey(shortcut).ok()?)
                })
                .unwrap_or(false)
    }

    /// Start audio recording.
    ///
    /// Returns `Ok(true)` if recording actually started, `Ok(false)` if it was
    /// refused because the controller is not idle (e.g. a previous utterance is
    /// still transcribing). Callers use this to avoid reporting a recording that
    /// never happened (finding 1).
    pub fn start_recording(&mut self) -> Result<bool, DictationError> {
        let profile = self
            .settings
            .resolved_hotkey_profiles()
            .into_iter()
            .find(|profile| profile.id == "default")
            .or_else(|| self.settings.resolved_hotkey_profiles().into_iter().next())
            .expect("resolved profiles always contains at least one profile");
        self.start_recording_for_profile(profile)
    }

    pub fn start_recording_for_profile(
        &mut self,
        profile: HotkeyProfile,
    ) -> Result<bool, DictationError> {
        self.start_recording_for_profile_with_capture(profile, |audio| audio.start_capture())
    }

    /// Start recording using the supplied capture operation.
    ///
    /// The injected capture operation keeps the startup transition testable
    /// without requiring a microphone. It also makes sure a failure after the
    /// dictation session is opened goes through the same controller-owned
    /// cleanup path as other workflow errors.
    fn start_recording_for_profile_with_capture<F>(
        &mut self,
        profile: HotkeyProfile,
        start_capture: F,
    ) -> Result<bool, DictationError>
    where
        F: FnOnce(&mut AudioCaptureService) -> Result<(), DictationError>,
    {
        if self.state != AppState::Idle || self.hotkey_configuration_changing {
            warn!("Cannot start recording: state is {:?}", self.state);
            return Ok(false);
        }

        let next_generation = self.recording_generation.checked_add(1).ok_or_else(|| {
            DictationError::TranscriptionFailed("Recording generation exhausted; restart Sagascript.".into())
        })?;

        let session_id = self.logging.start_dictation_session();
        self.logging.log(
            "info",
            "App",
            log_events::session::DICTATION_STARTED,
            serde_json::json!({ "dictationSessionId": session_id }),
        );

        self.session_data = Some(serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "git_hash": env!("GIT_HASH"),
            "language": profile.language,
            "model": self.settings.effective_model_for(profile.language),
            "audio_ms": null,
            "phases_ms": {},
            "auto_paste": self.settings.auto_paste,
        }));
        self.release_started = None;
        if let Err(error) = start_capture(&mut self.audio) {
            let message = error.to_string();
            self.on_transcription_error(&message);
            return Err(error);
        }

        info!(
            profile_id = %profile.id,
            profile_name = %profile.name,
            language = %profile.language.display_name(),
            "Recording profile selected"
        );
        self.active_hotkey_profile = Some(profile);
        self.recording_generation = next_generation;
        self.active_hotkey_mode = Some(self.settings.hotkey_mode);
        self.presenter_owned = false;
        self.training_recording = false;
        self.state = AppState::Recording;
        self.recording_start = Some(Instant::now());
        self.last_error = None;

        info!("Recording started");
        Ok(true)
    }

    /// Only the native Presenter coordinator owns this completion pipeline.
    /// A GUI/manual recording must keep its normal lifecycle even when the
    /// configured global-hotkey mode happens to be Presenter.
    pub fn start_presenter_recording_for_profile(
        &mut self,
        profile: HotkeyProfile,
    ) -> Result<bool, DictationError> {
        self.start_presenter_recording_with_capture(profile, |audio| audio.start_capture())
    }

    fn start_presenter_recording_with_capture<F>(
        &mut self,
        profile: HotkeyProfile,
        start_capture: F,
    ) -> Result<bool, DictationError>
    where
        F: FnOnce(&mut AudioCaptureService) -> Result<(), DictationError>,
    {
        if self.settings.hotkey_mode != HotkeyMode::Presenter {
            return Ok(false);
        }
        let started = self.start_recording_for_profile_with_capture(profile, start_capture)?;
        if started {
            self.presenter_owned = true;
        }
        Ok(started)
    }

    /// Start a Teach Sagascript recording that only the Teach UI may stop.
    /// Global hotkey releases and toggle presses must not route this audio
    /// through the normal dictation/auto-paste path.
    pub fn start_training_recording_for_profile(
        &mut self,
        profile: HotkeyProfile,
    ) -> Result<bool, DictationError> {
        let started = self.start_recording_for_profile(profile)?;
        if started {
            self.training_recording = true;
        }
        Ok(started)
    }

    /// Stop recording and return the captured 16 kHz samples.
    ///
    /// Propagates a capture/resample failure (finding 4) instead of masking it
    /// as an empty buffer, so a real device/format error can reach the user
    /// rather than being reported as "No audio captured". On error the state is
    /// left as `Recording`; callers surface the error and return to Idle (see
    /// [`Self::stop_recording_guarded`]).
    pub fn stop_recording(&mut self) -> Result<Vec<f32>, DictationError> {
        self.mark_release();
        let samples = self.audio.stop_capture()?;
        let (finalization, conversion) = self.audio.stop_timings();
        self.record_phase("recording_finalization", finalization);
        self.record_phase("conversion", conversion);
        if let Some(data) = self.session_data.as_mut() {
            data["audio_ms"] = serde_json::json!(samples.len() as u64 / 16);
        }
        let duration = self
            .recording_start
            .map(|s| s.elapsed().as_millis())
            .unwrap_or(0);
        let metrics = self.audio.metrics();
        let recording_duration_ms = u64::try_from(duration).unwrap_or(u64::MAX);
        let audio_duration_ms = u64::try_from(
            (samples.len() as u128).saturating_mul(1_000) / 16_000,
        )
        .unwrap_or(u64::MAX);

        info!(
            "Recording stopped: {} samples ({duration}ms)",
            samples.len()
        );
        self.logging.log(
            "info",
            "Performance",
            log_events::audio::CAPTURE_STOPPED,
            serde_json::json!({
                "recordingDurationMs": recording_duration_ms,
                "audioDurationMs": audio_duration_ms,
                "audioSamples": samples.len(),
                "captureRequestToStreamPlayReturnMs": metrics.stream_play_return_ms,
                "captureRequestToFirstAudioCallbackMs": metrics.first_callback_ms,
                "deviceSampleRateHz": metrics.device_sample_rate_hz,
            }),
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
    pub fn preserve_transcription(&mut self, text: &str) {
        self.last_transcription = Some(text.to_string());
    }

    /// Called after transcription succeeds
    pub fn on_transcription_success(&mut self, text: &str) {
        self.presenter_owned = false;
        self.end_session("success");
        self.last_error = None;
        self.last_transcription = Some(text.to_string());
        self.audio.clear_last_captured();
        self.state = AppState::Idle;
        self.active_hotkey_profile = None;
        self.training_recording = false;
        self.logging.end_dictation_session();
    }

    /// Complete a quiet push-to-talk cancellation without replacing the last
    /// useful transcript, surfacing an error, or leaving retry audio behind.
    pub fn on_no_speech_detected(&mut self) {
        self.presenter_owned = false;
        self.end_session("no_speech");
        self.audio.clear_last_captured();
        self.state = AppState::Idle;
        self.active_hotkey_profile = None;
        self.training_recording = false;
        self.logging.log(
            "info",
            "Transcription",
            log_events::transcription::NO_SPEECH,
            serde_json::json!({}),
        );
        self.logging.end_dictation_session();
    }

    pub fn log_dictation_performance(&self, data: serde_json::Value) {
        self.logging.log(
            "info",
            "Performance",
            log_events::transcription::PHASE_TIMINGS,
            data,
        );
    }

    /// Called after transcription fails
    pub fn on_transcription_error(&mut self, error: &str) {
        self.presenter_owned = false;
        self.end_session("error");
        self.last_error = Some(error.to_string());
        self.state = AppState::Idle;
        self.active_hotkey_profile = None;
        self.training_recording = false;
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
                if text.trim().is_empty() {
                    self.on_no_speech_detected();
                } else {
                    self.on_transcription_success(&text);
                }
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
    pub fn complete_cancelled_recording(&mut self) {
        self.presenter_owned = false;
        self.end_session("cancelled");
        self.audio.clear_last_captured();
        self.state = AppState::Idle;
        self.active_hotkey_profile = None;
        self.active_hotkey_mode = None;
        self.training_recording = false;
        self.logging.end_dictation_session();
    }

    /// Cancel recording without transcribing
    pub fn cancel_recording(&mut self) {
        if self.state.is_recording() {
            let _ = self.audio.stop_capture();
            self.complete_cancelled_recording();
            info!("Recording cancelled");
        }
    }

    pub fn mark_release(&mut self) {
        self.release_started.get_or_insert_with(Instant::now);
    }

    pub fn record_phase(&mut self, phase: &'static str, duration: Duration) {
        if let Some(data) = self.session_data.as_mut() {
            data["phases_ms"][phase] = serde_json::json!(duration.as_secs_f64() * 1000.0);
        }
    }

    pub fn record_model_cache(&mut self, cached: bool) {
        if let Some(data) = self.session_data.as_mut() {
            data["model_cached"] = serde_json::json!(cached);
            data["context_profile"] = serde_json::json!("flash_attention");
        }
    }

    fn end_session(&mut self, outcome: &'static str) {
        if let Some(mut data) = self.session_data.take() {
            data["outcome"] = serde_json::json!(outcome);
            if let Some(start) = self.release_started.take() {
                data["key_up_to_completion_ms"] = serde_json::json!(start.elapsed().as_secs_f64() * 1000.0);
            }
            // Only typed identifiers, durations and outcomes. Never log text,
            // glossary entries, audio, window titles or free-form errors.
            self.logging.log("info", "Dictation", "dictation_session_finished", data);
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
    use std::cell::Cell;

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
    fn a_deferred_stop_cannot_stop_a_new_recording() {
        let mut ctrl = default_controller();
        let profile = ctrl.settings().resolved_hotkey_profiles()[0].clone();
        ctrl.start_recording_for_profile_with_capture(profile.clone(), |_| Ok(())).unwrap();
        let old_generation = ctrl.recording_generation();
        ctrl.cancel_recording();
        ctrl.start_recording_for_profile_with_capture(profile, |_| Ok(())).unwrap();
        assert_ne!(old_generation, ctrl.recording_generation());
        assert!(matches!(ctrl.stop_recording_generation(old_generation), StopRecordingOutcome::NotRecording));
        assert_eq!(ctrl.state(), AppState::Recording);
        ctrl.cancel_recording();
    }

    #[test]
    fn cancellation_during_transcription_requires_explicit_completion() {
        let mut ctrl = default_controller();
        ctrl.state = AppState::Transcribing;
        ctrl.last_transcription = Some("Previous useful result".to_string());

        ctrl.cancel_recording();
        assert_eq!(ctrl.state(), AppState::Transcribing);
        assert_eq!(ctrl.last_transcription(), Some("Previous useful result"));

        ctrl.complete_cancelled_recording();
        assert_eq!(ctrl.state(), AppState::Idle);
        assert_eq!(ctrl.last_transcription(), Some("Previous useful result"));
    }

    #[test]
    fn failed_capture_keeps_generation_available_for_next_start() {
        let mut ctrl = default_controller();
        let profile = ctrl.settings().resolved_hotkey_profiles()[0].clone();
        assert!(ctrl
            .start_recording_for_profile_with_capture(profile.clone(), |_| Ok(()))
            .unwrap());
        let first_generation = ctrl.recording_generation();
        ctrl.cancel_recording();

        let failed = ctrl.start_recording_for_profile_with_capture(profile.clone(), |_| {
            Err(DictationError::MicrophonePermissionDenied)
        });
        assert!(matches!(failed, Err(DictationError::MicrophonePermissionDenied)));
        assert_eq!(ctrl.recording_generation(), first_generation);
        assert_eq!(ctrl.state(), AppState::Idle);

        assert!(ctrl
            .start_recording_for_profile_with_capture(profile, |_| Ok(()))
            .unwrap());
        assert_eq!(ctrl.recording_generation(), first_generation + 1);
        ctrl.cancel_recording();
    }

    #[test]
    fn generation_exhaustion_fails_before_capture() {
        let mut ctrl = default_controller();
        ctrl.recording_generation = u64::MAX;
        let profile = ctrl.settings().resolved_hotkey_profiles()[0].clone();
        let capture_called = Cell::new(false);

        let result = ctrl.start_recording_for_profile_with_capture(profile, |_| {
            capture_called.set(true);
            Ok(())
        });

        assert!(matches!(
            result,
            Err(DictationError::TranscriptionFailed(message))
                if message.contains("generation exhausted")
        ));
        assert!(!capture_called.get());
        assert_eq!(ctrl.state(), AppState::Idle);
        assert_eq!(ctrl.recording_generation(), u64::MAX);
    }

    #[test]
    fn manual_recording_in_presenter_mode_does_not_own_presenter_pipeline() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().hotkey_mode = HotkeyMode::Presenter;
        let profile = ctrl.settings.resolved_hotkey_profiles()[0].clone();
        ctrl.start_recording_for_profile_with_capture(profile, |_| Ok(())).unwrap();
        assert!(!ctrl.is_presenter_session());
        assert!(!ctrl.should_finish_presenter());
        ctrl.state = AppState::Transcribing;
        assert!(!ctrl.is_presenter_session());
        ctrl.finish_transcription(Ok("manual result".into())).unwrap();
        assert_eq!(ctrl.state(), AppState::Idle);
        assert_eq!(ctrl.last_transcription(), Some("manual result"));
    }

    #[test]
    fn dedicated_presenter_start_owns_successful_capture_and_repeated_start_is_noop() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().hotkey_mode = HotkeyMode::Presenter;
        let profile = ctrl.settings.resolved_hotkey_profiles()[0].clone();

        assert!(ctrl
            .start_presenter_recording_with_capture(profile.clone(), |_| Ok(()))
            .unwrap());
        let generation = ctrl.recording_generation();
        assert!(ctrl.presenter_owned);
        assert!(ctrl.is_presenter_session());

        assert!(!ctrl
            .start_presenter_recording_with_capture(profile, |_| {
                panic!("a repeated presenter start must not invoke capture")
            })
            .unwrap());
        assert_eq!(ctrl.recording_generation(), generation);
        assert!(ctrl.presenter_owned);
        assert!(ctrl.is_presenter_session());
        ctrl.complete_cancelled_recording();
    }

    #[test]
    fn presenter_start_failure_block_and_wrong_mode_never_take_ownership() {
        let mut ctrl = default_controller();
        let profile = ctrl.settings.resolved_hotkey_profiles()[0].clone();

        assert!(!ctrl
            .start_presenter_recording_with_capture(profile.clone(), |_| {
                panic!("wrong-mode presenter start must not invoke capture")
            })
            .unwrap());
        assert!(!ctrl.presenter_owned);
        assert_eq!(ctrl.state(), AppState::Idle);

        ctrl.settings_mut().hotkey_mode = HotkeyMode::Presenter;
        ctrl.state = AppState::Transcribing;
        assert!(!ctrl
            .start_presenter_recording_with_capture(profile.clone(), |_| {
                panic!("blocked presenter start must not invoke capture")
            })
            .unwrap());
        assert!(!ctrl.presenter_owned);
        assert_eq!(ctrl.state(), AppState::Transcribing);

        ctrl.state = AppState::Idle;
        let failed = ctrl.start_presenter_recording_with_capture(profile, |_| {
            Err(DictationError::MicrophonePermissionDenied)
        });
        assert!(matches!(failed, Err(DictationError::MicrophonePermissionDenied)));
        assert!(!ctrl.presenter_owned);
        assert_eq!(ctrl.state(), AppState::Idle);
        assert!(ctrl.active_hotkey_profile().is_none());
    }

    #[test]
    fn presenter_ownership_clears_on_every_terminal_path_and_manual_restart_stays_normal() {
        for terminal in ["success", "error", "no_speech", "cancelled"] {
            let mut ctrl = default_controller();
            ctrl.settings_mut().hotkey_mode = HotkeyMode::Presenter;
            let profile = ctrl.settings.resolved_hotkey_profiles()[0].clone();
            assert!(ctrl
                .start_presenter_recording_with_capture(profile, |_| Ok(()))
                .unwrap());
            assert!(ctrl.presenter_owned);

            match terminal {
                "success" => ctrl.on_transcription_success("presenter result"),
                "error" => ctrl.on_transcription_error("presenter failed"),
                "no_speech" => ctrl.on_no_speech_detected(),
                "cancelled" => ctrl.complete_cancelled_recording(),
                _ => unreachable!(),
            }
            assert!(!ctrl.presenter_owned, "terminal path: {terminal}");
            assert_eq!(ctrl.state(), AppState::Idle);

            let profile = ctrl.settings.resolved_hotkey_profiles()[0].clone();
            assert!(ctrl
                .start_recording_for_profile_with_capture(profile, |_| Ok(()))
                .unwrap());
            assert!(!ctrl.presenter_owned, "manual restart: {terminal}");
            assert!(!ctrl.is_presenter_session(), "manual restart: {terminal}");
            ctrl.complete_cancelled_recording();
        }
    }

    #[test]
    fn training_capture_is_never_presenter_owned() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().hotkey_mode = HotkeyMode::Presenter;
        let profile = ctrl.settings.resolved_hotkey_profiles()[0].clone();
        assert!(ctrl
            .start_recording_for_profile_with_capture(profile, |_| Ok(()))
            .unwrap());
        ctrl.training_recording = true;
        assert!(!ctrl.presenter_owned);
        assert!(!ctrl.is_presenter_session());
        ctrl.complete_cancelled_recording();
    }

    #[test]
    fn training_recording_is_not_a_presenter_session() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().hotkey_mode = HotkeyMode::Presenter;
        ctrl.state = AppState::Recording;
        ctrl.active_hotkey_mode = Some(HotkeyMode::Presenter);
        ctrl.training_recording = true;
        assert!(!ctrl.is_presenter_session());
        assert!(!ctrl.should_finish_presenter());
        ctrl.complete_cancelled_recording();
    }

    #[test]
    fn pending_toggle_configuration_does_not_report_started_recording() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().hotkey_mode = HotkeyMode::Toggle;
        let profile = ctrl.settings().resolved_hotkey_profiles()[0].clone();
        assert!(ctrl.begin_hotkey_configuration_change());

        let result = ctrl.handle_hotkey_down_for_profile(profile);

        assert_eq!(result.unwrap(), HotkeyDownResult::NoOp);
        assert_eq!(ctrl.state(), AppState::Idle);
        ctrl.end_hotkey_configuration_change();
    }

    #[test]
    fn fallback_start_freezes_push_to_talk_without_rewriting_preferences() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().hotkey_mode = HotkeyMode::Presenter;
        let profile = ctrl.settings().resolved_hotkey_profiles()[0].clone();
        ctrl.start_recording_for_profile_with_capture(profile, |_| Ok(())).unwrap();
        ctrl.use_safe_fallback_lifecycle();
        assert!(ctrl.should_stop_profile_on_key_up(crate::SAFE_FALLBACK_HOTKEY));
        assert!(!ctrl.should_finish_presenter());
        assert_eq!(ctrl.settings().hotkey_mode, HotkeyMode::Presenter);
        ctrl.cancel_recording();
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
    fn finish_transcription_empty_audio_returns_idle() {
        let mut ctrl = default_controller();
        ctrl.state = AppState::Transcribing;

        let result = ctrl.finish_transcription(Err("No audio captured".to_string()));

        assert_eq!(result, Err("No audio captured".to_string()));
        assert_eq!(ctrl.last_error(), Some("No audio captured"));
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

    #[test]
    fn finish_transcription_empty_result_preserves_last_transcription() {
        let mut ctrl = default_controller();
        ctrl.state = AppState::Transcribing;
        ctrl.last_transcription = Some("Previous useful result".to_string());

        let result = ctrl.finish_transcription(Ok(String::new()));

        assert_eq!(result, Ok(String::new()));
        assert_eq!(ctrl.last_transcription(), Some("Previous useful result"));
        assert_eq!(ctrl.state(), AppState::Idle);
    }

    #[test]
    fn finish_transcription_whitespace_result_preserves_last_transcription() {
        let mut ctrl = default_controller();
        ctrl.state = AppState::Transcribing;
        ctrl.last_transcription = Some("Previous useful result".to_string());

        let result = ctrl.finish_transcription(Ok(" \n\t".to_string()));

        assert_eq!(result, Ok(" \n\t".to_string()));
        assert_eq!(ctrl.last_transcription(), Some("Previous useful result"));
        assert_eq!(ctrl.state(), AppState::Idle);
    }

    #[test]
    fn no_speech_returns_idle_without_replacing_last_transcription() {
        let mut ctrl = default_controller();
        ctrl.state = AppState::Transcribing;
        ctrl.last_transcription = Some("Previous useful result".to_string());
        ctrl.last_error = Some("stale error".to_string());
        ctrl.session_data = Some(serde_json::json!({
            "language": "en",
            "model": "base.en",
            "phases_ms": {},
        }));
        ctrl.release_started = Some(Instant::now());

        ctrl.on_no_speech_detected();

        assert_eq!(ctrl.state(), AppState::Idle);
        assert_eq!(ctrl.last_transcription(), Some("Previous useful result"));
        assert_eq!(ctrl.last_error(), Some("stale error"));
        assert!(ctrl.session_data.is_none());
        assert!(ctrl.release_started.is_none());

        // A repeated terminal callback has no active payload, so it cannot
        // emit a second typed terminal event for the same session.
        ctrl.on_no_speech_detected();
        assert!(ctrl.session_data.is_none());
        assert!(ctrl.release_started.is_none());
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

    #[test]
    fn presenter_start_and_release_never_implicitly_finish() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().hotkey_mode = HotkeyMode::Presenter;
        ctrl.active_hotkey_mode = Some(HotkeyMode::Presenter);
        ctrl.presenter_owned = true;
        for state in [AppState::Recording, AppState::Transcribing] {
            ctrl.state = state;
            assert_eq!(ctrl.handle_hotkey_down().unwrap(), HotkeyDownResult::NoOp);
            assert!(!ctrl.should_stop_on_key_up());
            assert_eq!(ctrl.should_finish_presenter(), state == AppState::Recording);
        }
        ctrl.state = AppState::Recording;
        ctrl.training_recording = true;
        assert!(!ctrl.should_finish_presenter());
    }

    #[test]
    fn mode_changes_do_not_reinterpret_active_recording() {
        let mut ctrl = default_controller();
        ctrl.state = AppState::Recording;
        ctrl.active_hotkey_mode = Some(HotkeyMode::Presenter);
        ctrl.settings_mut().hotkey_mode = HotkeyMode::PushToTalk;
        ctrl.presenter_owned = true;
        assert!(!ctrl.should_stop_on_key_up());
        assert!(ctrl.should_finish_presenter());
        assert_eq!(ctrl.handle_hotkey_down().unwrap(), HotkeyDownResult::NoOp);

        ctrl.active_hotkey_mode = Some(HotkeyMode::PushToTalk);
        ctrl.settings_mut().hotkey_mode = HotkeyMode::Presenter;
        assert!(ctrl.should_stop_on_key_up());
        assert!(!ctrl.should_finish_presenter());
    }

    #[test]
    fn shortcut_transaction_blocks_capture_without_holding_controller_lock() {
        let mut ctrl = default_controller();
        assert!(ctrl.begin_hotkey_configuration_change());
        assert!(!ctrl.begin_hotkey_configuration_change());
        let profile = ctrl.settings.resolved_hotkey_profiles()[0].clone();
        assert!(!ctrl.start_recording_for_profile_with_capture(profile, |_| {
            panic!("capture must not start while bindings are changing");
        }).unwrap());
        ctrl.end_hotkey_configuration_change();
        assert!(ctrl.begin_hotkey_configuration_change());
        ctrl.end_hotkey_configuration_change();
        for state in [AppState::Recording, AppState::Transcribing, AppState::Error] {
            ctrl.state = state;
            assert!(!ctrl.begin_hotkey_configuration_change());
        }
    }

    #[test]
    fn active_profile_freezes_language_and_release_identity() {
        let mut ctrl = default_controller();
        let profiles = vec![
            HotkeyProfile { id: "default".into(), name: "English".into(), shortcut: "Control+Shift+E".into(), language: sagascript_core::settings::Language::English },
            HotkeyProfile { id: "swedish".into(), name: "Swedish".into(), shortcut: "Option+Space".into(), language: sagascript_core::settings::Language::Swedish },
        ];
        ctrl.settings_mut().replace_hotkey_profiles(profiles.clone()).unwrap();
        ctrl.settings_mut().hotkey_mode = HotkeyMode::PushToTalk;
        ctrl.state = AppState::Recording;
        ctrl.active_hotkey_profile = Some(profiles[1].clone());

        assert_eq!(ctrl.language(), sagascript_core::settings::Language::Swedish);
        assert!(ctrl.should_stop_profile_on_key_up("Alt+Space"));
        assert!(!ctrl.should_stop_profile_on_key_up("Control+Shift+E"));
    }

    #[test]
    fn toggle_press_from_different_profile_does_not_stop_active_recording() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().hotkey_mode = HotkeyMode::Toggle;
        ctrl.state = AppState::Recording;
        ctrl.active_hotkey_profile = Some(HotkeyProfile { id: "english".into(), name: "English".into(), shortcut: "Control+Shift+E".into(), language: sagascript_core::settings::Language::English });
        let swedish = HotkeyProfile { id: "swedish".into(), name: "Swedish".into(), shortcut: "Option+Space".into(), language: sagascript_core::settings::Language::Swedish };

        assert_eq!(ctrl.handle_hotkey_down_for_profile(swedish).unwrap(), HotkeyDownResult::NoOp);
    }

    #[test]
    fn english_alt_e_profile_freezes_english_model_with_global_swedish() {
        let settings = Settings {
            language: sagascript_core::settings::Language::Swedish,
            auto_select_model: true,
            hotkey_profiles: vec![
                HotkeyProfile {
                    id: "default".into(),
                    name: "Swedish".into(),
                    shortcut: "Control+Shift+Space".into(),
                    language: sagascript_core::settings::Language::Swedish,
                },
                HotkeyProfile {
                    id: "english".into(),
                    name: "English".into(),
                    shortcut: "Alt+E".into(),
                    language: sagascript_core::settings::Language::English,
                },
            ],
            ..Default::default()
        };
        let mut ctrl = AppController::new(settings);
        let english = ctrl
            .settings()
            .hotkey_profile_for_shortcut("Alt+E")
            .expect("English Alt+E profile should resolve");

        assert!(ctrl
            .start_recording_for_profile_with_capture(english, |_| Ok(()))
            .unwrap());
        assert_eq!(ctrl.language(), sagascript_core::settings::Language::English);

        let session = ctrl
            .session_data
            .as_ref()
            .expect("recording should snapshot session metadata");
        assert_eq!(session["language"], serde_json::json!("en"));
        assert_eq!(session["model"], serde_json::json!("base.en"));

        // Changes to global settings during an active recording must not
        // replace the profile model selected when Alt+E started the session.
        ctrl.settings_mut().language = sagascript_core::settings::Language::Swedish;
        ctrl.settings_mut().whisper_model = sagascript_core::settings::WhisperModel::KbWhisperBase;
        assert_eq!(
            ctrl.session_data.as_ref().unwrap()["model"],
            serde_json::json!("base.en")
        );

        ctrl.cancel_recording();
    }

    #[test]
    fn terminal_session_payload_is_consumed_once_without_transcript_contents() {
        let mut ctrl = default_controller();
        ctrl.state = AppState::Transcribing;
        ctrl.session_data = Some(serde_json::json!({
            "language": "en",
            "model": "base.en",
            "phases_ms": {},
        }));

        ctrl.preserve_transcription("private transcript text");
        let payload_before_end = ctrl.session_data.as_ref().unwrap().to_string();
        assert!(!payload_before_end.contains("private transcript text"));

        ctrl.on_transcription_success("private transcript text");
        assert!(ctrl.session_data.is_none(), "the terminal event must consume the session payload");
        assert!(ctrl.release_started.is_none());

        // A repeated terminal callback has no active payload to emit, so one
        // dictation session can produce at most one finished-session event.
        ctrl.on_transcription_success("private transcript text");
        assert!(ctrl.session_data.is_none());
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
    fn training_recording_ignores_toggle_hotkey_stop() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().hotkey_mode = HotkeyMode::Toggle;
        ctrl.state = AppState::Recording;
        ctrl.training_recording = true;

        assert_eq!(
            ctrl.handle_hotkey_down().unwrap(),
            HotkeyDownResult::NoOp
        );
    }

    #[test]
    fn training_recording_ignores_push_to_talk_release() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().hotkey_mode = HotkeyMode::PushToTalk;
        ctrl.state = AppState::Recording;
        ctrl.training_recording = true;

        assert!(!ctrl.should_stop_on_key_up());
    }

    #[test]
    fn cancelling_training_recording_restores_normal_hotkey_lifecycle() {
        let mut ctrl = default_controller();
        ctrl.settings_mut().hotkey_mode = HotkeyMode::PushToTalk;
        ctrl.state = AppState::Recording;
        ctrl.training_recording = true;

        ctrl.cancel_recording();

        assert_eq!(ctrl.state(), AppState::Idle);
        assert!(!ctrl.training_recording);
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

    #[test]
    fn recording_start_failure_closes_session_and_restores_idle_without_capture() {
        let mut ctrl = default_controller();
        let profile = ctrl
            .settings()
            .resolved_hotkey_profiles()
            .into_iter()
            .next()
            .expect("default settings provide a hotkey profile");
        let expected_error = DictationError::MicrophonePermissionDenied;
        let expected_message = expected_error.to_string();

        let result = ctrl.start_recording_for_profile_with_capture(profile, |_| {
            Err(expected_error.clone())
        });

        assert!(matches!(result, Err(DictationError::MicrophonePermissionDenied)));
        assert_eq!(ctrl.last_error(), Some(expected_message.as_str()));
        assert_eq!(ctrl.state(), AppState::Idle);
        assert!(ctrl.active_hotkey_profile().is_none());
        assert!(!ctrl.training_recording);

        // The failed start must leave the controller reusable for a later
        // attempt; this still injects the capture result and never opens a mic.
        assert!(matches!(
            ctrl.start_recording_for_profile_with_capture(
                ctrl.settings()
                    .resolved_hotkey_profiles()
                    .into_iter()
                    .next()
                    .unwrap(),
                |_| Ok(()),
            ),
            Ok(true)
        ));
        assert_eq!(ctrl.state(), AppState::Recording);
        ctrl.cancel_recording();
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
