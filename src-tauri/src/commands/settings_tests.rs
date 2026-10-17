use super::*;

#[cfg(test)]
mod language_selection_tests {
    use super::apply_language_selection;
    use sagascript_core::settings::{Language, Settings, WhisperModel};

    #[test]
    fn explicit_language_selection_restores_recommended_model() {
        for language in [
            Language::English,
            Language::Swedish,
            Language::Norwegian,
            Language::Finnish,
            Language::Auto,
        ] {
            let mut settings = Settings {
                whisper_model: WhisperModel::Small,
                auto_select_model: false,
                ..Default::default()
            };
            apply_language_selection(&mut settings, language).unwrap();
            assert_eq!(settings.language, language);
            assert_eq!(settings.whisper_model, WhisperModel::recommended(language));
            assert_eq!(
                settings.effective_model(),
                WhisperModel::recommended(language)
            );
            assert!(settings.auto_select_model);
        }
    }

    #[test]
    fn rejected_language_change_preserves_model_and_dictionary() {
        let mut settings = Settings {
            language: Language::Swedish,
            whisper_model: WhisperModel::KbWhisperSmall,
            auto_select_model: false,
            ..Default::default()
        };
        settings
            .profile_glossaries
            .insert("default".into(), "merge = merch".into());
        let error = apply_language_selection(&mut settings, Language::English).unwrap_err();
        assert!(error.contains("personal dictionary"));
        assert_eq!(settings.language, Language::Swedish);
        assert_eq!(settings.whisper_model, WhisperModel::KbWhisperSmall);
        assert!(!settings.auto_select_model);
        assert_eq!(settings.profile_glossaries["default"], "merge = merch");
    }
}

#[cfg(test)]
mod auto_paste_tests {
    use super::effective_auto_paste;

    #[test]
    fn enabling_never_persists_true_before_accessibility_is_trusted() {
        assert!(!effective_auto_paste(true, false));
        assert!(effective_auto_paste(true, true));
        assert!(!effective_auto_paste(false, true));
    }
}

/// The `get_settings` payload as the shared fixture
/// contracts/transport/get_settings.json (#347): Rust writes it, the
/// frontend's `Settings` type parses it (src/test/transport.test.ts).
#[cfg(test)]
mod transport_contract_tests {
    use super::*;
    use crate::contract;
    use sagascript_core::settings::{
        EnginePrewarm, FileModelPreference, HotkeyMode, HotkeyProfile, WhisperModel,
    };
    use serde_json::{json, Value};

    /// Non-default values in every field the frontend reads, one profile with
    /// explicit push-to-talk and toggle shortcuts and one without, and every
    /// form of profile model.
    fn customised() -> Settings {
        Settings {
            language: Language::Swedish,
            whisper_model: WhisperModel::KbWhisperSmall,
            file_transcription_model: FileModelPreference::PianissimoOriginal,
            pianissimo_dictation: true,
            engine_prewarm: EnginePrewarm::OnKeyDown,
            engine_idle_unload_minutes: 0,
            hotkey_mode: HotkeyMode::Toggle,
            show_overlay: false,
            auto_paste: false,
            auto_select_model: false,
            hotkey: "F13".into(),
            hotkey_profiles: vec![
                HotkeyProfile {
                    id: "swedish".into(),
                    name: "Svenska".into(),
                    shortcut: "F13".into(),
                    language: Language::Swedish,
                    push_to_talk_shortcut: Some("F13".into()),
                    toggle_shortcut: Some("F14".into()),
                },
                HotkeyProfile {
                    id: "finnish".into(),
                    name: "Suomi".into(),
                    shortcut: "Control+Shift+F".into(),
                    language: Language::Finnish,
                    push_to_talk_shortcut: None,
                    toggle_shortcut: None,
                },
            ],
            profile_models: [
                (
                    "swedish".to_string(),
                    FileModelPreference::Whisper(WhisperModel::KbWhisperMedium),
                ),
                ("finnish".to_string(), FileModelPreference::Auto),
            ]
            .into(),
            initial_prompt: "Astrid, Grimnir".into(),
            profile_glossaries: [("swedish".to_string(), "mergea = mördsa".to_string())].into(),
            profile_glossary_migrated: true,
            beam_size: 5,
            temperature_fallback: false,
            vad_enabled: true,
            has_completed_onboarding: true,
            ..Settings::default()
        }
    }

    /// `{ variant: payload }`. `default` is a fresh install, whose implicit
    /// legacy profile `get_settings` makes explicit.
    fn fixture() -> Value {
        json!({
            "default": contract::to_value(settings_for_frontend(Settings::default())),
            "customised": contract::to_value(settings_for_frontend(customised())),
        })
    }

    #[test]
    fn transport_contract_get_settings() {
        contract::assert_fixture("get_settings.json", &fixture());
    }

    /// Rust reads the fixture back unchanged: nothing in it is a value the
    /// frontend could send back that Rust would drop or rewrite.
    #[test]
    fn transport_contract_get_settings_reads_back() {
        if contract::blessing() {
            return; // the fixture is being rewritten; the next run reads it
        }
        let fixture = contract::read_fixture("get_settings.json");
        let variants = fixture.as_object().expect("variants by name");
        assert!(variants.contains_key("default") && variants.contains_key("customised"));
        for (name, payload) in variants {
            let settings: Settings = serde_json::from_value(payload.clone())
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(
                &contract::to_value(settings_for_frontend(settings)),
                payload,
                "{name}"
            );
        }
    }
}
