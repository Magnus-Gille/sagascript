/// Structured log event names emitted by the Rust app.
///
/// Every constant here has a call site. There are deliberately no dead-code
/// allows, so removing the last use of an event surfaces a warning instead of
/// leaving a stale name behind; add a constant together with the code that
/// logs it.
pub mod app {
    pub const STARTED: &str = "app_started";
}

pub mod audio {
    pub const CAPTURE_STOPPED: &str = "capture_stopped";
}

pub mod transcription {
    pub const NO_SPEECH: &str = "no_speech_detected";
    pub const PHASE_TIMINGS: &str = "dictation_phase_timings";
}

pub mod session {
    pub const DICTATION_STARTED: &str = "dictation_session_started";
}
