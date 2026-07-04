/// Log event name constants matching the Swift app's LogEvent structure.
///
/// Not every constant here is wired up yet from the Rust side — some mirror
/// events the Swift companion app emits but the Rust app_controller doesn't
/// (yet) log. Those carry their own `#[allow(dead_code)]` rather than a
/// blanket module-level one, so newly-wired constants (see `app::STARTED`,
/// `session::DICTATION_STARTED`) surface a real dead-code warning if their
/// call site is ever removed without removing the constant too.
pub mod app {
    pub const STARTED: &str = "app_started";
    #[allow(dead_code)]
    pub const READY: &str = "app_ready";
    #[allow(dead_code)]
    pub const TERMINATED: &str = "app_terminated";
}

#[allow(dead_code)]
pub mod hotkey {
    pub const REGISTERED: &str = "hotkey_registered";
    pub const UNREGISTERED: &str = "hotkey_unregistered";
    pub const KEY_DOWN: &str = "key_down";
    pub const KEY_UP: &str = "key_up";
}

#[allow(dead_code)]
pub mod audio {
    pub const CAPTURE_STARTED: &str = "capture_started";
    pub const CAPTURE_STOPPED: &str = "capture_stopped";
    pub const PERMISSION_GRANTED: &str = "permission_granted";
    pub const PERMISSION_DENIED: &str = "permission_denied";
}

#[allow(dead_code)]
pub mod transcription {
    pub const STARTED: &str = "transcription_started";
    pub const COMPLETED: &str = "transcription_completed";
    pub const FAILED: &str = "transcription_failed";
    pub const MODEL_LOADING: &str = "model_loading";
    pub const MODEL_LOADED: &str = "model_loaded";
}

#[allow(dead_code)]
pub mod paste {
    pub const ATTEMPTED: &str = "paste_attempted";
    pub const SUCCEEDED: &str = "paste_succeeded";
    pub const FAILED: &str = "paste_failed";
}

pub mod session {
    pub const DICTATION_STARTED: &str = "dictation_session_started";
    #[allow(dead_code)]
    pub const DICTATION_COMPLETE: &str = "dictation_session_complete";
    #[allow(dead_code)]
    pub const STATE_CHANGED: &str = "state_changed";
}
