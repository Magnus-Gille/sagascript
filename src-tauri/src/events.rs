/// Tauri event names emitted from backend to frontend
pub mod event {
    /// App state changed (idle/recording/transcribing/error)
    pub const STATE_CHANGED: &str = "state-changed";
    /// Transcription result ready
    pub const TRANSCRIPTION_RESULT: &str = "transcription-result";
    /// Error occurred
    pub const ERROR: &str = "error";
    /// Model download progress
    pub const MODEL_DOWNLOAD_PROGRESS: &str = "model-download-progress";
    /// Model ready
    pub const MODEL_READY: &str = "model-ready";
    /// Transcription progress percentage (0–100)
    pub const TRANSCRIPTION_PROGRESS: &str = "transcription-progress";
    /// Hotkey registration health changed (registered OK <-> failed to
    /// register). Payload: `{ ok: bool, error: string | null, shortcut: string }`.
    pub const HOTKEY_REGISTRATION_CHANGED: &str = "hotkey-registration-changed";
}

#[cfg(test)]
mod tests {
    use super::event::*;

    #[test]
    fn event_names_are_kebab_case() {
        let events = [
            STATE_CHANGED,
            TRANSCRIPTION_RESULT,
            ERROR,
            MODEL_DOWNLOAD_PROGRESS,
            MODEL_READY,
            TRANSCRIPTION_PROGRESS,
            HOTKEY_REGISTRATION_CHANGED,
        ];
        for name in events {
            assert!(!name.is_empty());
            assert!(
                !name.contains('_'),
                "event '{name}' uses underscore instead of kebab-case"
            );
            assert!(
                !name.contains(' '),
                "event '{name}' contains spaces"
            );
        }
    }

    #[test]
    fn event_names_are_unique() {
        let events = [
            STATE_CHANGED,
            TRANSCRIPTION_RESULT,
            ERROR,
            MODEL_DOWNLOAD_PROGRESS,
            MODEL_READY,
            TRANSCRIPTION_PROGRESS,
            HOTKEY_REGISTRATION_CHANGED,
        ];
        for (i, a) in events.iter().enumerate() {
            for (j, b) in events.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "duplicate event name: {a}");
                }
            }
        }
    }
}
