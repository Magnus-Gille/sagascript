use super::SettingsError;
use crate::error::DictationError;
use crate::models::{Language, WhisperModel};
use crate::settings::{validate_hotkey, FileModelPreference, HotkeyProfile, Settings};

fn profile(id: &str, name: &str, shortcut: &str, language: Language) -> HotkeyProfile {
    HotkeyProfile {
        id: id.into(),
        name: name.into(),
        shortcut: shortcut.into(),
        language,
        push_to_talk_shortcut: None,
        toggle_shortcut: None,
    }
}

/// Every variant, with the exact text it replaced and the code it carries.
/// The codes are wire vocabulary (ADR 0006): a row changes only by adding one.
#[test]
fn every_variant_keeps_its_text_and_has_a_stable_snake_case_code() {
    let table: Vec<(SettingsError, &str, &str)> = vec![
        (
            SettingsError::UnknownModelPreference { id: "nope".into() },
            "Unknown file model preference 'nope'",
            "invalid_setting_value",
        ),
        (
            SettingsError::InvalidEnginePrewarm { value: "x".into() },
            "engine_prewarm must be off, on_app_start or on_key_down, got 'x'",
            "invalid_setting_value",
        ),
        (
            SettingsError::UnknownProfile { id: "p".into() },
            "Unknown profile 'p'",
            "unknown_profile",
        ),
        (
            SettingsError::UnknownDictationProfile { id: "p".into() },
            "Unknown dictation profile 'p'",
            "unknown_profile",
        ),
        (
            SettingsError::ModelIncompatibleWithProfile {
                model: "Whisper Tiny (EN)".into(),
                profile_id: "default".into(),
            },
            "Model 'Whisper Tiny (EN)' is incompatible with profile 'default'",
            "model_incompatible",
        ),
        (
            SettingsError::ModelIncompatibleWithLanguage {
                model: "KB-Whisper Tiny".into(),
                language: "English".into(),
            },
            "Whisper model 'KB-Whisper Tiny' is incompatible with English",
            "model_incompatible",
        ),
        (
            SettingsError::PianissimoSwedishProfileOnly {
                profile_id: "en".into(),
            },
            "Pianissimo is only available for Swedish profile 'en'",
            "pianissimo_unavailable",
        ),
        (
            SettingsError::PianissimoRequiresSwedishProfile,
            "Pianissimo requires a Swedish profile",
            "pianissimo_unavailable",
        ),
        (
            SettingsError::PianissimoSwedishFilesOnly,
            "Pianissimo Original is only available for Swedish file transcription",
            "pianissimo_unavailable",
        ),
        (
            SettingsError::PianissimoPlainFilesOnly,
            "Pianissimo supports plain Swedish file transcription only; select a Whisper file model for meetings or diarization",
            "pianissimo_unavailable",
        ),
        (
            SettingsError::PianissimoUnsupported,
            crate::models::PIANISSIMO_UNSUPPORTED_MESSAGE,
            "pianissimo_unavailable",
        ),
        (
            SettingsError::InvalidHotkey("Invalid hotkey: empty or malformed.".into()),
            "Invalid hotkey: empty or malformed.",
            "hotkey_invalid",
        ),
        (
            SettingsError::DuplicateHotkey {
                shortcut: "Control+A".into(),
            },
            "Duplicate hotkey 'Control+A'",
            "hotkey_invalid",
        ),
        (
            SettingsError::DuplicateHotkeyBindings,
            "Duplicate hotkey bindings",
            "hotkey_invalid",
        ),
        (
            SettingsError::EmptyProfileHotkey {
                profile_id: "p".into(),
            },
            "Profile 'p' has an empty hotkey",
            "hotkey_invalid",
        ),
        (
            SettingsError::NoProfiles,
            "At least one hotkey profile is required",
            "profile_invalid",
        ),
        (
            SettingsError::InvalidProfileId { id: "Bad Id".into() },
            "Invalid profile id 'Bad Id': use lowercase letters, numbers, '-' or '_'",
            "profile_invalid",
        ),
        (
            SettingsError::DuplicateProfileId { id: "p".into() },
            "Duplicate profile id 'p'",
            "profile_invalid",
        ),
        (
            SettingsError::ProfileNameMissing {
                profile_id: "p".into(),
            },
            "Profile 'p' must have a name",
            "profile_invalid",
        ),
        (
            SettingsError::ProfileNameTooLong {
                profile_id: "p".into(),
            },
            "Profile 'p' name must be 40 characters or fewer",
            "profile_invalid",
        ),
        (
            SettingsError::InvalidProfileFileId { id: "../x".into() },
            "Invalid profile id '../x' for personal dictionary file",
            "profile_invalid",
        ),
        (
            SettingsError::DictionaryBlocksLanguageChange {
                profile_id: "default".into(),
            },
            "Profile 'default' has a personal dictionary; clear it before changing the profile language",
            "dictionary_blocks_language_change",
        ),
        (
            SettingsError::InactiveProfileDictionary {
                profile_id: "p".into(),
            },
            "Profile id 'p' has an inactive personal dictionary; choose a new id to avoid reactivating old aliases",
            "inactive_profile_dictionary",
        ),
        (
            SettingsError::Storage("Failed to write settings: disk full".into()),
            "Failed to write settings: disk full",
            "settings_storage_failed",
        ),
    ];
    for (error, text, code) in table {
        assert_eq!(error.to_string(), text, "{error:?}");
        assert_eq!(error.code(), code, "{error:?}");
        assert!(
            code.bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'),
            "{code} is not snake_case"
        );
    }
}

#[test]
fn the_cli_wraps_a_settings_error_with_its_own_code_and_the_old_text() {
    let error = DictationError::from(SettingsError::UnknownModelPreference { id: "nope".into() });
    assert_eq!(error.code(), "invalid_setting_value");
    assert_eq!(
        error.to_string(),
        "Settings error: Unknown file model preference 'nope'",
        "the CLI printed this text before the error was typed"
    );
    assert!(!error.retryable());
    // The CLI's own wording for a bad flag keeps the generic code.
    assert_eq!(
        DictationError::SettingsError("--speakers: x".into()).code(),
        "settings_error"
    );
}

#[test]
fn the_operations_refuse_with_the_variant_a_caller_can_match() {
    // A language change blocked by a personal dictionary: the one refusal the
    // Settings window recognises (it shows the dictionary and offers to clear it).
    let mut settings = Settings {
        language: Language::Swedish,
        ..Settings::default()
    };
    settings
        .profile_glossaries
        .insert("default".into(), "Foo = bar".into());
    let blocked = settings.set_legacy_language(Language::English);
    assert_eq!(
        blocked,
        Err(SettingsError::DictionaryBlocksLanguageChange {
            profile_id: "default".into()
        })
    );

    let default = Settings::default();
    let unknown = default.dictation_model_for_profile("nosuch").unwrap_err();
    assert_eq!(unknown.code(), "unknown_profile");

    assert_eq!(
        FileModelPreference::parse_id("nope").unwrap_err().code(),
        "invalid_setting_value"
    );

    let mut english = Settings {
        language: Language::English,
        ..Settings::default()
    };
    let incompatible = english
        .set_profile_model(
            "default",
            FileModelPreference::Whisper(WhisperModel::KbWhisperTiny),
        )
        .unwrap_err();
    assert_eq!(incompatible.code(), "model_incompatible");

    assert_eq!(
        Settings::validate_hotkey_profiles(&[]).unwrap_err(),
        SettingsError::NoProfiles
    );
    let duplicate = [
        profile("a", "A", "Control+Shift+A", Language::English),
        profile("a", "B", "Control+Shift+B", Language::English),
    ];
    assert_eq!(
        Settings::validate_hotkey_profiles(&duplicate).unwrap_err(),
        SettingsError::DuplicateProfileId { id: "a".into() }
    );
    assert_eq!(
        validate_hotkey("Bogus+").unwrap_err().code(),
        "hotkey_invalid"
    );
}
