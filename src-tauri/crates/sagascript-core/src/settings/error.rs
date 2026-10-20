//! Why a settings operation was refused or failed (ADR 0006, #360 part 2).
//!
//! The settings operations (`Settings::apply_language_selection`, the profile
//! and model setters, the store's `save` and `try_update`) used to return
//! `Result<_, String>`. A caller that wanted to tell "this language change is
//! blocked by a dictionary" from "the file could not be written" had to read
//! the text. Now each refusal is a variant with a stable [`code`](SettingsError::code),
//! and the text a user sees is the variant's `Display`, unchanged from the
//! strings it replaces.
//!
//! - The CLI wraps one as [`DictationError::Settings`](crate::error::DictationError),
//!   so the JSON error object of `sagascript transcribe --json` carries the code.
//! - The app wraps one as `CommandError::Settings`, so the webview receives
//!   `{ code, message, retryable }` and branches on `code`.
//!
//! Codes are snake_case, added and never renamed or reused for another meaning.
//! Several variants may share a code when a caller would treat them alike
//! (`model_incompatible`, `profile_invalid`).

use serde::Serialize;
use thiserror::Error;

/// A settings operation refused a change, or the settings could not be read or
/// written. `Clone` and `Serialize`, like [`DictationError`](crate::error::DictationError),
/// which carries it across the CLI; the I/O error behind a storage failure is
/// kept as text in its message.
#[derive(Error, Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "message")]
pub enum SettingsError {
    /// A model preference identifier that names no model (`config set
    /// file_transcription_model nope`).
    #[error("Unknown file model preference '{id}'")]
    UnknownModelPreference { id: String },

    /// `engine_prewarm` is none of `off`, `on_app_start`, `on_key_down`.
    #[error("engine_prewarm must be off, on_app_start or on_key_down, got '{value}'")]
    InvalidEnginePrewarm { value: String },

    /// No dictation profile has this id.
    #[error("Unknown profile '{id}'")]
    UnknownProfile { id: String },

    /// A dictation profile was asked for by a name the app's dictionary
    /// commands spell differently from [`Self::UnknownProfile`].
    #[error("Unknown dictation profile '{id}'")]
    UnknownDictationProfile { id: String },

    /// A model cannot serve the language of the profile it was chosen for.
    #[error("Model '{model}' is incompatible with profile '{profile_id}'")]
    ModelIncompatibleWithProfile {
        model: &'static str,
        profile_id: String,
    },

    /// A model cannot serve the language of the file run.
    #[error("Whisper model '{model}' is incompatible with {language}")]
    ModelIncompatibleWithLanguage {
        model: &'static str,
        language: &'static str,
    },

    /// Pianissimo only serves a Swedish profile, and this one is not.
    #[error("Pianissimo is only available for Swedish profile '{profile_id}'")]
    PianissimoSwedishProfileOnly { profile_id: String },

    /// Pianissimo was chosen for a profile that is not Swedish.
    #[error("Pianissimo requires a Swedish profile")]
    PianissimoRequiresSwedishProfile,

    /// Pianissimo Original was chosen for a file run in another language.
    #[error("Pianissimo Original is only available for Swedish file transcription")]
    PianissimoSwedishFilesOnly,

    /// The file model is Pianissimo, which only transcribes plain Swedish files,
    /// and the run is a meeting import or diarization (the app's wording; the
    /// CLI says it in its own words).
    #[error(
        "Pianissimo supports plain Swedish file transcription only; select a Whisper file model for meetings or diarization"
    )]
    PianissimoPlainFilesOnly,

    /// The file model is Pianissimo and the run asks for diarization.
    #[error(
        "Pianissimo supports plain Swedish file transcription only; choose a Whisper file model for diarization"
    )]
    PianissimoNotForDiarization,

    /// This system cannot run Pianissimo (selecting it, or downloading its
    /// model).
    #[error("{}", crate::models::PIANISSIMO_UNSUPPORTED_MESSAGE)]
    PianissimoUnsupported,

    /// A shortcut that is malformed or not allowed on this system. The text
    /// says why.
    #[error("{0}")]
    InvalidHotkey(String),

    /// Two bindings claim the same shortcut.
    #[error("Duplicate hotkey '{shortcut}'")]
    DuplicateHotkey { shortcut: String },

    /// The profiles' resolved bindings collide.
    #[error("Duplicate hotkey bindings")]
    DuplicateHotkeyBindings,

    /// A profile has a blank shortcut.
    #[error("Profile '{profile_id}' has an empty hotkey")]
    EmptyProfileHotkey { profile_id: String },

    /// A profile list with no profile in it.
    #[error("At least one hotkey profile is required")]
    NoProfiles,

    /// A profile id that is not lowercase letters, numbers, `-` or `_`.
    #[error("Invalid profile id '{id}': use lowercase letters, numbers, '-' or '_'")]
    InvalidProfileId { id: String },

    /// Two profiles share an id.
    #[error("Duplicate profile id '{id}'")]
    DuplicateProfileId { id: String },

    /// A profile without a name.
    #[error("Profile '{profile_id}' must have a name")]
    ProfileNameMissing { profile_id: String },

    /// A profile name over 40 characters.
    #[error("Profile '{profile_id}' name must be 40 characters or fewer")]
    ProfileNameTooLong { profile_id: String },

    /// A profile id that cannot name a personal-dictionary file.
    #[error("Invalid profile id '{id}' for personal dictionary file")]
    InvalidProfileFileId { id: String },

    /// The profile's personal dictionary has aliases for its current language,
    /// so changing the language would orphan them. The user clears the
    /// dictionary first.
    #[error(
        "Profile '{profile_id}' has a personal dictionary; clear it before changing the profile language"
    )]
    DictionaryBlocksLanguageChange { profile_id: String },

    /// A new profile reuses the id of a removed profile whose personal
    /// dictionary is still stored.
    #[error(
        "Profile id '{profile_id}' has an inactive personal dictionary; choose a new id to avoid reactivating old aliases"
    )]
    InactiveProfileDictionary { profile_id: String },

    /// The settings file, its lock, a personal-dictionary file or a recovery
    /// copy could not be read, written, locked or installed. The text names the
    /// step and carries the operating system's reason.
    #[error("{0}")]
    Storage(String),
}

/// The display text, for a caller that reports plain text: today the test
/// helpers (`test_support`, the app's persist tests), which compare refusals by
/// their text. Production code keeps the typed error (ADR 0006). Like
/// `From<DictationError> for String`.
impl From<SettingsError> for String {
    fn from(error: SettingsError) -> String {
        error.to_string()
    }
}

impl SettingsError {
    /// The stable machine-readable code (ADR 0006). Never renamed or reused for
    /// another meaning.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownModelPreference { .. } | Self::InvalidEnginePrewarm { .. } => {
                "invalid_setting_value"
            }
            Self::UnknownProfile { .. } | Self::UnknownDictationProfile { .. } => "unknown_profile",
            Self::ModelIncompatibleWithProfile { .. }
            | Self::ModelIncompatibleWithLanguage { .. } => "model_incompatible",
            Self::PianissimoSwedishProfileOnly { .. }
            | Self::PianissimoRequiresSwedishProfile
            | Self::PianissimoSwedishFilesOnly
            | Self::PianissimoPlainFilesOnly
            | Self::PianissimoNotForDiarization
            | Self::PianissimoUnsupported => "pianissimo_unavailable",
            Self::InvalidHotkey(_)
            | Self::DuplicateHotkey { .. }
            | Self::DuplicateHotkeyBindings
            | Self::EmptyProfileHotkey { .. } => "hotkey_invalid",
            Self::NoProfiles
            | Self::InvalidProfileId { .. }
            | Self::DuplicateProfileId { .. }
            | Self::ProfileNameMissing { .. }
            | Self::ProfileNameTooLong { .. }
            | Self::InvalidProfileFileId { .. } => "profile_invalid",
            Self::DictionaryBlocksLanguageChange { .. } => "dictionary_blocks_language_change",
            Self::InactiveProfileDictionary { .. } => "inactive_profile_dictionary",
            Self::Storage(_) => "settings_storage_failed",
        }
    }

    /// A storage failure with its complete user-facing text.
    pub(crate) fn storage(message: impl Into<String>) -> Self {
        Self::Storage(message.into())
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
