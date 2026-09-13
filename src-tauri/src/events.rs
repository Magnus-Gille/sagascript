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
    /// File-decode progress as a byte fraction 0–100 (#237). An I/O
    /// fraction, not a time fraction; 100 snaps on clean EOF. Emitted only
    /// while decoding; the inference stream uses TRANSCRIPTION_PROGRESS.
    pub const TRANSCRIPTION_DECODE: &str = "transcription-decode";
    /// Plain file-transcription phase (#237): "decoding" | "loading" |
    /// "preparing". Emitted as the backend moves through the silent
    /// pre-inference work so the UI can name the stall instead of showing a
    /// frozen 1%. The progress stream takes over once inference starts.
    pub const TRANSCRIPTION_PHASE: &str = "transcription-phase";
    /// Hotkey registration health changed (registered OK <-> failed to
    /// register). Payload: `{ ok: bool, error: string | null, shortcut: string }`.
    pub const HOTKEY_REGISTRATION_CHANGED: &str = "hotkey-registration-changed";
    /// Dictation profile selected by the shortcut that started recording.
    pub const ACTIVE_HOTKEY_PROFILE_CHANGED: &str = "active-hotkey-profile-changed";
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
            TRANSCRIPTION_DECODE,
            TRANSCRIPTION_PHASE,
            HOTKEY_REGISTRATION_CHANGED,
            ACTIVE_HOTKEY_PROFILE_CHANGED,
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
            TRANSCRIPTION_DECODE,
            TRANSCRIPTION_PHASE,
            HOTKEY_REGISTRATION_CHANGED,
            ACTIVE_HOTKEY_PROFILE_CHANGED,
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
