use super::*;
use crate::models::{Language, WhisperModel};

fn profile(id: &str, shortcut: &str, language: Language) -> HotkeyProfile {
    HotkeyProfile {
        id: id.to_string(),
        name: id.to_string(),
        shortcut: shortcut.to_string(),
        language,
        push_to_talk_shortcut: None,
        toggle_shortcut: None,
    }
}

/// Two profiles, the default Swedish one with an explicit model, in the shape a
/// migrated file has: profile models for both, the legacy fields not in step.
fn swedish_with_explicit_model() -> Settings {
    let mut settings = Settings {
        hotkey_profiles: vec![
            profile("default", "Super+S", Language::Swedish),
            profile("work", "Super+W", Language::English),
        ],
        language: Language::Swedish,
        whisper_model: WhisperModel::KbWhisperLarge,
        auto_select_model: false,
        ..Settings::default()
    };
    settings
        .set_profile_model_gated(
            "default",
            FileModelPreference::Whisper(WhisperModel::KbWhisperLarge),
            true,
        )
        .unwrap();
    settings
        .set_profile_model_gated(
            "work",
            FileModelPreference::Whisper(WhisperModel::BaseEn),
            true,
        )
        .unwrap();
    settings
}

fn as_json(settings: &Settings) -> serde_json::Value {
    serde_json::to_value(settings).expect("settings serialize")
}

// -- apply_language_selection --

#[test]
fn language_selection_restores_the_recommended_model_in_the_legacy_fields() {
    for language in Language::ALL {
        let mut settings = Settings {
            whisper_model: WhisperModel::Small,
            auto_select_model: false,
            ..Settings::default()
        };
        let notices = settings
            .apply_language_selection_gated(language, true)
            .unwrap();
        assert!(notices.is_empty(), "{language:?}: {notices:?}");
        assert_eq!(settings.default_profile().language, language);
        assert_eq!(settings.language, language);
        assert_eq!(settings.whisper_model, WhisperModel::recommended(language));
        assert!(settings.auto_select_model);
    }
}

#[test]
fn language_selection_resets_only_the_models_the_new_language_cannot_use() {
    let mut settings = swedish_with_explicit_model();
    let notices = settings
        .apply_language_selection_gated(Language::English, true)
        .unwrap();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].profile_id, "default");
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Auto
    );
    assert_eq!(
        settings.profile_models["work"],
        FileModelPreference::Whisper(WhisperModel::BaseEn)
    );
    assert_eq!(settings.whisper_model, WhisperModel::BaseEn);
    assert!(settings.auto_select_model);
}

#[test]
fn language_selection_keeps_a_compatible_profile_model_silently() {
    let mut settings = swedish_with_explicit_model();
    settings
        .set_profile_model_gated(
            "default",
            FileModelPreference::Whisper(WhisperModel::Base),
            true,
        )
        .unwrap();
    let notices = settings
        .apply_language_selection_gated(Language::Norwegian, true)
        .unwrap();
    assert!(notices.is_empty(), "{notices:?}");
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Whisper(WhisperModel::Base)
    );
}

#[test]
fn refused_language_selection_changes_nothing() {
    let mut settings = swedish_with_explicit_model();
    settings
        .profile_glossaries
        .insert("default".into(), "merge = merch".into());
    let before = as_json(&settings);
    let error = settings
        .apply_language_selection_gated(Language::English, true)
        .unwrap_err()
        .to_string();
    assert!(error.contains("personal dictionary"), "{error}");
    assert_eq!(as_json(&settings), before);
}

// -- the default profile's model --

#[test]
fn whisper_model_sets_the_profile_model_and_both_legacy_fields() {
    let mut settings = swedish_with_explicit_model();
    settings
        .set_whisper_model_gated(WhisperModel::KbWhisperMedium, true)
        .unwrap();
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Whisper(WhisperModel::KbWhisperMedium)
    );
    assert_eq!(settings.whisper_model, WhisperModel::KbWhisperMedium);
    assert!(!settings.auto_select_model);
    assert_eq!(
        settings.profile_models["work"],
        FileModelPreference::Whisper(WhisperModel::BaseEn),
        "another profile is not touched"
    );
}

#[test]
fn a_model_the_default_profile_cannot_use_is_refused_without_a_change() {
    let mut settings = swedish_with_explicit_model();
    let before = as_json(&settings);
    let error = settings
        .set_whisper_model_gated(WhisperModel::BaseEn, true)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("incompatible with profile 'default'"),
        "{error}"
    );
    assert_eq!(as_json(&settings), before);
}

#[test]
fn auto_select_model_on_is_auto_and_off_pins_the_legacy_model() {
    let mut settings = swedish_with_explicit_model();
    settings.set_auto_select_model_gated(true, true).unwrap();
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Auto
    );
    assert!(settings.auto_select_model);

    // Off keeps the model the legacy field names, which is why the model
    // operations keep that field in step.
    settings
        .set_whisper_model_gated(WhisperModel::KbWhisperSmall, true)
        .unwrap();
    settings.set_auto_select_model_gated(true, true).unwrap();
    settings.set_auto_select_model_gated(false, true).unwrap();
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Whisper(WhisperModel::KbWhisperSmall)
    );
    assert!(!settings.auto_select_model);
}

#[test]
fn auto_select_model_off_is_refused_when_the_legacy_model_does_not_fit() {
    let mut settings = swedish_with_explicit_model();
    settings.whisper_model = WhisperModel::BaseEn;
    let before = as_json(&settings);
    let error = settings
        .set_auto_select_model_gated(false, true)
        .unwrap_err()
        .to_string();
    assert!(error.contains("incompatible"), "{error}");
    assert_eq!(as_json(&settings), before);
}

#[test]
fn pianissimo_dictation_needs_the_platform_and_a_swedish_profile() {
    let mut settings = swedish_with_explicit_model();
    let before = as_json(&settings);
    let unsupported = settings
        .set_pianissimo_dictation_gated(true, false)
        .unwrap_err();
    assert_eq!(unsupported, SettingsError::PianissimoUnsupported);
    assert_eq!(
        unsupported.to_string(),
        crate::models::PIANISSIMO_UNSUPPORTED_MESSAGE
    );
    assert_eq!(as_json(&settings), before);

    settings.set_pianissimo_dictation_gated(true, true).unwrap();
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::PianissimoOriginal
    );
    assert!(settings.pianissimo_dictation);

    settings
        .set_pianissimo_dictation_gated(false, true)
        .unwrap();
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Auto
    );
    assert!(!settings.pianissimo_dictation);

    let mut english = Settings::default();
    let error = english
        .set_pianissimo_dictation_gated(true, true)
        .unwrap_err();
    assert_eq!(error, SettingsError::PianissimoRequiresSwedishProfile);
    assert_eq!(error.to_string(), "Pianissimo requires a Swedish profile");
    assert!(!english.pianissimo_dictation);
}

// -- apply_profiles_change --

#[test]
fn profile_change_resets_a_model_the_new_language_cannot_use_with_a_notice() {
    let mut settings = swedish_with_explicit_model();
    let mut profiles = settings.resolved_hotkey_profiles();
    profiles[0].language = Language::English;
    let notices = settings
        .apply_profiles_change_gated(profiles, None, true)
        .unwrap();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Auto
    );
    assert_eq!(
        settings.whisper_model,
        WhisperModel::KbWhisperLarge,
        "a profile operation leaves the legacy fields to the default-profile operations"
    );
}

#[test]
fn profile_change_with_an_explicit_model_checks_it_against_the_new_language() {
    let mut settings = swedish_with_explicit_model();
    let mut profiles = settings.resolved_hotkey_profiles();
    profiles[0].language = Language::English;
    let choice = FileModelPreference::Whisper(WhisperModel::SmallEn);
    let notices = settings
        .apply_profiles_change_gated(profiles.clone(), Some(("default", choice)), true)
        .unwrap();
    assert!(
        notices.is_empty(),
        "no notice for the profile given a model"
    );
    assert_eq!(settings.profile_models["default"], choice);

    let mut settings = swedish_with_explicit_model();
    let before = as_json(&settings);
    let error = settings
        .apply_profiles_change_gated(
            profiles,
            Some(("default", FileModelPreference::PianissimoOriginal)),
            true,
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("Pianissimo"), "{error}");
    assert_eq!(
        as_json(&settings),
        before,
        "a refused change changes nothing"
    );
}

#[test]
fn profile_change_keeps_the_notice_of_every_other_profile() {
    let mut settings = Settings {
        hotkey_profiles: vec![
            profile("a", "Super+J", Language::Swedish),
            profile("b", "Super+K", Language::Swedish),
        ],
        ..Settings::default()
    };
    for id in ["a", "b"] {
        settings
            .set_profile_model_gated(id, FileModelPreference::PianissimoOriginal, true)
            .unwrap();
    }
    let mut profiles = settings.resolved_hotkey_profiles();
    for profile in &mut profiles {
        profile.language = Language::English;
    }
    let choice = FileModelPreference::Whisper(WhisperModel::BaseEn);
    let notices = settings
        .apply_profiles_change_gated(profiles, Some(("a", choice)), true)
        .unwrap();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].profile_id, "b");
    assert_eq!(settings.profile_models["a"], choice);
    assert_eq!(settings.profile_models["b"], FileModelPreference::Auto);
}

#[test]
fn removing_a_profile_through_a_profile_change_drops_its_model() {
    let mut settings = swedish_with_explicit_model();
    let profiles = vec![settings.resolved_hotkey_profiles().remove(0)];
    settings
        .apply_profiles_change_gated(profiles, None, true)
        .unwrap();
    assert!(!settings.profile_models.contains_key("work"));
    assert_eq!(settings.resolved_hotkey_profiles().len(), 1);
}

// -- auto-paste and VAD --

#[test]
fn auto_paste_is_stored_unless_the_permission_is_known_to_be_missing() {
    for (requested, accessibility, stored) in [
        (true, Accessibility::Granted, true),
        (true, Accessibility::Unknown, true),
        (true, Accessibility::Denied, false),
        (false, Accessibility::Granted, false),
        (false, Accessibility::Unknown, false),
        (false, Accessibility::Denied, false),
    ] {
        for start in [true, false] {
            let mut settings = Settings {
                auto_paste: start,
                ..Settings::default()
            };
            assert_eq!(
                settings.set_auto_paste(requested, accessibility),
                stored,
                "requested {requested}, {accessibility:?}"
            );
            assert_eq!(
                settings.auto_paste, stored,
                "requested {requested}, {accessibility:?}"
            );
        }
    }
}

#[test]
fn vad_enabled_is_only_the_stored_value() {
    let mut settings = Settings::default();
    assert!(!settings.vad_enabled);
    settings.set_vad_enabled(true);
    assert!(settings.vad_enabled);
    let mut expected = as_json(&settings);
    settings.set_vad_enabled(false);
    expected["vad_enabled"] = serde_json::Value::Bool(false);
    assert_eq!(as_json(&settings), expected, "no other field moves");
}
