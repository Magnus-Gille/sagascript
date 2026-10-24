use std::time::Duration;

use sagascript_core::audio::capture::AudioCaptureMetrics;
use sagascript_core::audio::AudioCaptureService;
use sagascript_core::error::DictationError;

pub(crate) trait Capture: Send {
    fn start_capture(&mut self) -> Result<(), DictationError>;
    fn stop_capture(&mut self) -> Result<Vec<f32>, DictationError>;
    fn stop_timings(&self) -> (Duration, Duration);
    fn metrics(&self) -> AudioCaptureMetrics;
}

impl Capture for AudioCaptureService {
    fn start_capture(&mut self) -> Result<(), DictationError> {
        AudioCaptureService::start_capture(self)
    }

    fn stop_capture(&mut self) -> Result<Vec<f32>, DictationError> {
        AudioCaptureService::stop_capture(self)
    }

    fn stop_timings(&self) -> (Duration, Duration) {
        AudioCaptureService::stop_timings(self)
    }

    fn metrics(&self) -> AudioCaptureMetrics {
        AudioCaptureService::metrics(self)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::app_controller::{AppController, AppState, Settings, StopRecordingOutcome};

    #[derive(Default)]
    struct FakeState {
        starts: usize,
        stops: usize,
        start_result: Option<Result<(), DictationError>>,
        stop_result: Option<Result<Vec<f32>, DictationError>>,
    }

    struct FakeCapture(Arc<Mutex<FakeState>>);

    impl Capture for FakeCapture {
        fn start_capture(&mut self) -> Result<(), DictationError> {
            let mut state = self.0.lock().unwrap();
            state.starts += 1;
            state.start_result.take().unwrap_or(Ok(()))
        }

        fn stop_capture(&mut self) -> Result<Vec<f32>, DictationError> {
            let mut state = self.0.lock().unwrap();
            state.stops += 1;
            state.stop_result.take().unwrap_or_else(|| Ok(Vec::new()))
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

    fn controller(state: Arc<Mutex<FakeState>>) -> AppController {
        AppController::with_capture(Settings::default(), FakeCapture(state))
    }

    fn default_profile(controller: &AppController) -> sagascript_core::settings::HotkeyProfile {
        controller.settings().resolved_hotkey_profiles()[0].clone()
    }

    #[test]
    fn public_start_and_stop_use_capture_and_return_nonempty_samples() {
        let shared = Arc::new(Mutex::new(FakeState {
            stop_result: Some(Ok(vec![0.25, -0.5, 0.75])),
            ..FakeState::default()
        }));
        let mut controller = controller(shared.clone());
        let profile = default_profile(&controller);

        assert!(controller.start_recording_for_profile(profile).unwrap());
        assert_eq!(controller.recording_generation(), 1);
        assert!(matches!(
            controller.stop_recording_guarded(),
            StopRecordingOutcome::Stopped(samples) if samples == vec![0.25, -0.5, 0.75]
        ));

        let state = shared.lock().unwrap();
        assert_eq!(state.starts, 1);
        assert_eq!(state.stops, 1);
        assert_eq!(controller.state(), AppState::Transcribing);
    }

    #[test]
    fn public_start_failure_does_not_consume_generation() {
        let shared = Arc::new(Mutex::new(FakeState {
            start_result: Some(Err(DictationError::MicrophonePermissionDenied)),
            ..FakeState::default()
        }));
        let mut controller = controller(shared.clone());
        let profile = default_profile(&controller);

        assert!(matches!(
            controller.start_recording_for_profile(profile),
            Err(DictationError::MicrophonePermissionDenied)
        ));

        assert_eq!(controller.recording_generation(), 0);
        assert_eq!(controller.state(), AppState::Idle);
        assert_eq!(shared.lock().unwrap().starts, 1);
    }

    #[test]
    fn pending_permission_keeps_controller_idle_without_consuming_generation() {
        let shared = Arc::new(Mutex::new(FakeState {
            start_result: Some(Err(DictationError::MicrophonePermissionPending)),
            ..FakeState::default()
        }));
        let mut controller = controller(shared.clone());
        let profile = default_profile(&controller);

        assert!(matches!(
            controller.start_recording_for_profile(profile),
            Err(DictationError::MicrophonePermissionPending)
        ));

        assert_eq!(controller.recording_generation(), 0);
        assert_eq!(controller.state(), AppState::Idle);
        assert_eq!(shared.lock().unwrap().starts, 1);
    }

    #[test]
    fn public_stop_failure_cleans_up_recording() {
        let shared = Arc::new(Mutex::new(FakeState {
            stop_result: Some(Err(DictationError::TranscriptionFailed(
                "resample failed".into(),
            ))),
            ..FakeState::default()
        }));
        let mut controller = controller(shared.clone());
        let profile = default_profile(&controller);

        assert!(controller.start_recording_for_profile(profile).unwrap());
        assert!(matches!(
            controller.stop_recording_guarded(),
            StopRecordingOutcome::Failed(DictationError::TranscriptionFailed(message))
                if message == "resample failed"
        ));

        assert_eq!(controller.state(), AppState::Idle);
        assert!(controller.last_error().is_some());
        assert_eq!(shared.lock().unwrap().stops, 1);
    }

    #[test]
    fn duplicate_public_stop_does_not_call_capture_twice() {
        let shared = Arc::new(Mutex::new(FakeState::default()));
        let mut controller = controller(shared.clone());
        let profile = default_profile(&controller);

        assert!(controller.start_recording_for_profile(profile).unwrap());
        assert!(matches!(
            controller.stop_recording_guarded(),
            StopRecordingOutcome::Stopped(_)
        ));
        assert!(matches!(
            controller.stop_recording_guarded(),
            StopRecordingOutcome::NotRecording
        ));

        assert_eq!(shared.lock().unwrap().stops, 1);
    }

    #[test]
    fn public_cancel_uses_capture_stop_and_suppresses_stop_failure() {
        let shared = Arc::new(Mutex::new(FakeState {
            stop_result: Some(Err(DictationError::TranscriptionFailed(
                "ignored during cancellation".into(),
            ))),
            ..FakeState::default()
        }));
        let mut controller = controller(shared.clone());
        let profile = default_profile(&controller);

        assert!(controller.start_recording_for_profile(profile).unwrap());
        controller.cancel_recording();

        assert_eq!(controller.state(), AppState::Idle);
        assert!(controller.last_error().is_none());
        assert_eq!(shared.lock().unwrap().stops, 1);
    }
}
