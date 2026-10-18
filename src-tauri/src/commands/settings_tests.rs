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

/// What the controller takes over from a setter that the file may have moved
/// ahead of it in other ways: shortcuts nothing registered, a Pianissimo
/// selection this host demoted. The mirrors take the fields their setter writes
/// and no others (the table in `persist_tests.rs` runs the real setters).
#[cfg(test)]
mod mirror_tests {
    use super::{
        mirror_auto_select_model, mirror_language_selection, mirror_pianissimo_dictation,
        mirror_profile_models, mirror_whisper_model,
    };
    use sagascript_core::settings::{
        FileModelPreference, HotkeyMode, HotkeyProfile, Language, Settings, WhisperModel,
    };

    type Mirror = fn(&mut Settings, &Settings);

    fn profile(id: &str, shortcut: &str, language: Language) -> HotkeyProfile {
        HotkeyProfile {
            id: id.into(),
            name: id.into(),
            shortcut: shortcut.into(),
            push_to_talk_shortcut: None,
            toggle_shortcut: None,
            language,
        }
    }

    /// The keys of the settings JSON whose values differ between two copies.
    fn changed_keys(before: &Settings, after: &Settings) -> Vec<String> {
        let (before, after) = (
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(after).unwrap(),
        );
        let mut keys: Vec<String> = after
            .as_object()
            .unwrap()
            .iter()
            .filter(|(key, value)| before.get(key.as_str()) != Some(*value))
            .map(|(key, _)| key.clone())
            .collect();
        keys.sort();
        keys
    }

    /// A file that differs from `memory` in every field a mirror might take or
    /// must leave: models, legacy flags, dictionaries, and the shortcut state.
    fn written_ahead_of(memory: &Settings) -> Settings {
        let mut written = memory.clone();
        written.language = Language::Norwegian;
        written.whisper_model = WhisperModel::NbWhisperBase;
        written.auto_select_model = !memory.auto_select_model;
        written.pianissimo_dictation = !memory.pianissimo_dictation;
        written
            .profile_models
            .insert("default".into(), FileModelPreference::Auto);
        written
            .profile_glossaries
            .insert("default".into(), "Astrid = astrid".into());
        written.hotkey = "Control+Alt+K".into();
        written.hotkey_mode = HotkeyMode::Toggle;
        written.hotkey_profiles = vec![profile("default", "Control+Alt+K", Language::Norwegian)];
        written.beam_size = 7;
        written.file_transcription_model = FileModelPreference::PianissimoOriginal;
        written
    }

    #[test]
    fn each_model_mirror_takes_the_fields_its_setter_writes_and_no_others() {
        let memory = Settings::default();
        let written = written_ahead_of(&memory);
        let table: [(&str, Mirror, &[&str]); 4] = [
            (
                "set_profile_model",
                mirror_profile_models,
                &["profile_models"],
            ),
            (
                "set_whisper_model",
                mirror_whisper_model,
                &["auto_select_model", "profile_models", "whisper_model"],
            ),
            (
                "set_auto_select_model",
                mirror_auto_select_model,
                &["auto_select_model", "profile_models"],
            ),
            (
                "set_pianissimo_dictation",
                mirror_pianissimo_dictation,
                &["pianissimo_dictation", "profile_models"],
            ),
        ];
        for (setter, mirror, fields) in table {
            let mut next = memory.clone();
            mirror(&mut next, &written);
            assert_eq!(changed_keys(&memory, &next), fields, "{setter}");
        }
    }

    #[test]
    fn the_language_mirror_follows_the_default_profile_and_keeps_the_shortcuts() {
        let mut memory = Settings {
            hotkey_profiles: vec![
                profile("default", "Control+Shift+Space", Language::English),
                profile("sv", "Control+Alt+S", Language::Swedish),
            ],
            ..Settings::default()
        };
        memory
            .profile_models
            .insert("sv".into(), FileModelPreference::Auto);
        let mut written = written_ahead_of(&memory);
        written.hotkey_profiles = vec![
            profile("default", "Control+Alt+K", Language::Norwegian),
            profile("sv", "Control+Alt+L", Language::Finnish),
        ];

        let mut next = memory.clone();
        mirror_language_selection(&mut next, &written);

        // The default profile's language is taken, its shortcut is not; the
        // other profile is not the operation's to move at all.
        assert_eq!(
            next.hotkey_profiles,
            vec![
                profile("default", "Control+Shift+Space", Language::Norwegian),
                profile("sv", "Control+Alt+S", Language::Swedish),
            ]
        );
        assert_eq!(next.hotkey, memory.hotkey);
        assert_eq!(next.hotkey_mode, memory.hotkey_mode);
        assert_eq!(
            changed_keys(&memory, &next),
            [
                "auto_select_model",
                "hotkey_profiles",
                "language",
                "profile_models",
                "whisper_model"
            ]
        );
        assert_eq!(next.language, Language::Norwegian);
    }

    #[test]
    fn the_language_mirror_makes_an_implicit_profile_list_explicit_like_the_file() {
        let memory = Settings::default();
        assert!(memory.hotkey_profiles.is_empty());
        let written = written_ahead_of(&memory);

        let mut next = memory.clone();
        mirror_language_selection(&mut next, &written);

        assert_eq!(
            next.hotkey_profiles,
            vec![HotkeyProfile::legacy_default(
                memory.hotkey.clone(),
                Language::Norwegian
            )]
        );
        assert_eq!(next.resolved_shortcuts(), memory.resolved_shortcuts());
    }

    #[test]
    fn the_language_mirror_leaves_profiles_the_file_does_not_name() {
        let memory = Settings {
            hotkey_profiles: vec![profile("work", "Control+Alt+W", Language::English)],
            ..Settings::default()
        };
        let written = written_ahead_of(&memory);
        assert_eq!(written.default_profile().id, "default");

        let mut next = memory.clone();
        mirror_language_selection(&mut next, &written);

        assert_eq!(next.hotkey_profiles, memory.hotkey_profiles);
        assert_eq!(next.language, Language::English);
    }
}

#[cfg(test)]
mod auto_paste_tests {
    use sagascript_core::settings::{Accessibility, Settings};

    /// What the setter stores for a request, given what it found out about the
    /// permission: the shared operation the CLI calls as well (#373).
    fn stored(requested: bool, accessibility: Accessibility) -> bool {
        let mut settings = Settings::default();
        settings.set_auto_paste(requested, accessibility);
        settings.auto_paste
    }

    #[test]
    fn enabling_never_persists_true_before_accessibility_is_trusted() {
        assert!(!stored(true, Accessibility::Denied));
        assert!(stored(true, Accessibility::Granted));
        assert!(!stored(false, Accessibility::Granted));
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
