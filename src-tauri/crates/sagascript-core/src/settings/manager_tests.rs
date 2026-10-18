use super::*;

// These tests ask no machine anything (#372): the operations that depend on it are
// the `_gated` ones, given the answers a host with neither engine nor model has.
// The same on every OS, so they read alike on a Mac that runs Pianissimo and one
// that has models downloaded.
const PIANISSIMO_SUPPORTED: bool = false;

fn nothing_downloaded(_: FileModel) -> bool {
    false
}

#[test]
fn pianissimo_dictation_defaults_off_and_roundtrips() {
    let default = Settings::default();
    assert!(!default.pianissimo_dictation);

    let legacy: Settings = serde_json::from_str(r#"{"language":"sv"}"#).unwrap();
    assert!(!legacy.pianissimo_dictation);

    let enabled = Settings {
        pianissimo_dictation: true,
        ..Default::default()
    };
    let json = serde_json::to_string(&enabled).unwrap();
    let decoded: Settings = serde_json::from_str(&json).unwrap();
    assert!(decoded.pianissimo_dictation);
}

#[test]
fn pianissimo_dictation_is_swedish_only() {
    let disabled = Settings::default();
    assert!(!disabled.uses_pianissimo_for_dictation(Language::Swedish));

    let enabled = Settings {
        pianissimo_dictation: true,
        ..Default::default()
    };
    assert!(enabled.uses_pianissimo_for_dictation(Language::Swedish));
    for language in [
        Language::English,
        Language::Norwegian,
        Language::Finnish,
        Language::Auto,
    ] {
        assert!(!enabled.uses_pianissimo_for_dictation(language));
    }
}

// -- HotkeyMode --

#[test]
fn hotkey_mode_default_is_push_to_talk() {
    assert_eq!(HotkeyMode::default(), HotkeyMode::PushToTalk);
}

#[test]
fn hotkey_mode_serde() {
    let json = serde_json::to_string(&HotkeyMode::PushToTalk).unwrap();
    assert_eq!(json, "\"push\"");
    let json = serde_json::to_string(&HotkeyMode::Toggle).unwrap();
    assert_eq!(json, "\"toggle\"");
    let legacy: HotkeyMode = serde_json::from_str("\"presenter\"").unwrap();
    assert_eq!(legacy, HotkeyMode::PushToTalk);
    assert_eq!(serde_json::to_string(&legacy).unwrap(), "\"push\"");
}

#[test]
fn hotkey_mode_ids_are_the_serde_names_and_presenter_is_not_one() {
    for mode in HotkeyMode::ALL {
        assert_eq!(
            serde_json::to_value(mode).unwrap(),
            serde_json::Value::String(mode.id().to_owned())
        );
        assert_eq!(HotkeyMode::from_id(mode.id()), Some(mode));
    }
    assert_eq!(HotkeyMode::from_id("presenter"), None);
    assert_eq!(HotkeyMode::from_id("Push"), None);
    assert_eq!(HotkeyMode::from_id(""), None);
}

// -- Settings --

#[test]
fn settings_default_values() {
    let s = Settings::default();
    assert_eq!(s.language, Language::English);
    assert_eq!(s.whisper_model, WhisperModel::Base);
    assert_eq!(s.hotkey_mode, HotkeyMode::PushToTalk);
    assert!(s.show_overlay);
    assert!(s.auto_paste);
    assert!(s.auto_select_model);
    assert_eq!(s.hotkey, "Control+Shift+Space");
    assert_eq!(s.initial_prompt, "");
    assert!(s.profile_glossaries.is_empty());
    assert_eq!(s.beam_size, 0);
    assert!(s.temperature_fallback);
    assert!(!s.vad_enabled);
}

#[test]
fn legacy_settings_presenter_mode_migrates_to_push_without_changing_old_hotkey_fields() {
    let settings: Settings =
        serde_json::from_str(r#"{"hotkey_mode":"presenter","hotkey":"Option+Space"}"#).unwrap();
    assert_eq!(settings.hotkey_mode, HotkeyMode::PushToTalk);
    assert_eq!(settings.hotkey, "Option+Space");
}

#[test]
fn effective_glossary_combines_global_and_selected_profile_only() {
    let mut settings = Settings {
        initial_prompt: "Codex = code x".to_string(),
        hotkey_profiles: vec![
            HotkeyProfile {
                id: "swedish".to_string(),
                name: "Swedish".to_string(),
                shortcut: "Control+Shift+Space".to_string(),
                language: Language::Swedish,
                push_to_talk_shortcut: None,
                toggle_shortcut: None,
            },
            HotkeyProfile {
                id: "english".to_string(),
                name: "English".to_string(),
                shortcut: "Control+Option+Space".to_string(),
                language: Language::English,
                push_to_talk_shortcut: None,
                toggle_shortcut: None,
            },
        ],
        ..Default::default()
    };
    settings
        .profile_glossaries
        .insert("swedish".to_string(), "mergea = mördsa".to_string());
    settings
        .profile_glossaries
        .insert("english".to_string(), "Lovable = love a ball".to_string());

    assert_eq!(
        settings.effective_glossary_source(Some("swedish")),
        "Codex\nmergea = mördsa"
    );
    assert_eq!(
        settings.effective_glossary_source(Some("english")),
        "Codex\nLovable = love a ball"
    );
    assert_eq!(settings.effective_glossary_source(None), "Codex");
}

#[test]
fn global_aliases_are_hint_only_and_do_not_leak_between_profiles() {
    let mut settings = Settings {
        initial_prompt: "merge = merch".to_string(),
        hotkey_profiles: vec![
            HotkeyProfile {
                id: "swedish".to_string(),
                name: "Swedish".to_string(),
                shortcut: "Control+Shift+Space".to_string(),
                language: Language::Swedish,
                push_to_talk_shortcut: None,
                toggle_shortcut: None,
            },
            HotkeyProfile {
                id: "english".to_string(),
                name: "English".to_string(),
                shortcut: "Control+Option+Space".to_string(),
                language: Language::English,
                push_to_talk_shortcut: None,
                toggle_shortcut: None,
            },
        ],
        ..Default::default()
    };
    settings
        .profile_glossaries
        .insert("swedish".to_string(), "merge = merch".to_string());

    let swedish = Glossary::parse(&settings.effective_glossary_source(Some("swedish")));
    let english = Glossary::parse(&settings.effective_glossary_source(Some("english")));
    assert_eq!(swedish.correct_text("merch").0, "merge");
    assert_eq!(english.correct_text("merch").0, "merch");
    assert_eq!(settings.effective_glossary_source(Some("english")), "merge");
}

#[test]
fn effective_glossary_deduplicates_global_and_profile_canonicals() {
    let mut settings = Settings {
        initial_prompt: "Codex = code x\nmerge = merch".to_string(),
        hotkey_profiles: vec![HotkeyProfile {
            id: "swedish".to_string(),
            name: "Swedish".to_string(),
            shortcut: "Control+Shift+Space".to_string(),
            language: Language::Swedish,
            push_to_talk_shortcut: None,
            toggle_shortcut: None,
        }],
        ..Default::default()
    };
    settings.profile_glossaries.insert(
        "swedish".to_string(),
        "merge = merch\nOpenRouter = open router".to_string(),
    );

    let glossary = Glossary::parse(&settings.effective_glossary_source(Some("swedish")));
    assert_eq!(
        glossary.single_word_terms(),
        vec!["Codex", "merge", "OpenRouter"]
    );
    assert_eq!(
        glossary.decoder_prompt().as_deref(),
        Some("Codex, merge, OpenRouter")
    );
    assert_eq!(
        glossary.correct_text("code x merch open router").0,
        "code x merge OpenRouter"
    );
}

#[test]
fn effective_glossary_uses_scoped_source_without_legacy_global_entries() {
    let mut settings = Settings::default();
    settings.profile_glossaries.insert(
        "default".to_string(),
        "Magnus Gille = Magnus Jille".to_string(),
    );

    assert_eq!(
        settings.effective_glossary_source(Some("default")),
        "Magnus Gille = Magnus Jille"
    );
}

#[test]
fn effective_glossary_ignores_unknown_and_auto_profile_entries() {
    let mut settings = Settings {
        language: Language::Auto,
        initial_prompt: "Codex = code x".to_string(),
        ..Default::default()
    };
    settings
        .profile_glossaries
        .insert("default".to_string(), "merge = merch".to_string());
    settings
        .profile_glossaries
        .insert("removed".to_string(), "Lovable = love a ball".to_string());

    assert_eq!(settings.effective_glossary_source(Some("default")), "Codex");
    assert_eq!(settings.effective_glossary_source(Some("removed")), "Codex");
}

#[test]
fn one_run_prompt_replaces_global_hints_but_keeps_selected_profile_aliases() {
    let mut settings = Settings {
        initial_prompt: "Global = alias".to_string(),
        hotkey_profiles: vec![HotkeyProfile {
            id: "swedish".to_string(),
            name: "Swedish".to_string(),
            shortcut: "Control+Shift+Space".to_string(),
            language: Language::Swedish,
            push_to_talk_shortcut: None,
            toggle_shortcut: None,
        }],
        ..Default::default()
    };
    settings
        .profile_glossaries
        .insert("swedish".to_string(), "Profile = misheard".to_string());

    assert_eq!(
        settings.effective_glossary_source_with_prompt(Some("swedish"), Some("OneRun = spelling")),
        "OneRun\nProfile = misheard"
    );
    assert_eq!(
        settings.effective_glossary_source_with_prompt(Some("swedish"), Some("  \n")),
        "Global\nProfile = misheard"
    );
    assert_eq!(
        settings.effective_glossary_source_with_prompt(None, Some("OneRun = spelling")),
        "OneRun"
    );
    assert_eq!(settings.initial_prompt, "Global = alias");
    assert_eq!(
        settings
            .profile_glossaries
            .get("swedish")
            .map(String::as_str),
        Some("Profile = misheard")
    );
}

#[test]
fn removed_profile_glossary_stays_inert_and_reserves_its_old_id() {
    let mut settings = Settings::default();
    settings
        .profile_glossaries
        .insert("removed".to_string(), "Lovable = love a ball".to_string());

    let error = settings
        .replace_hotkey_profiles_gated(
            vec![
                HotkeyProfile::legacy_default("Control+Shift+Space".to_string(), Language::Swedish),
                HotkeyProfile {
                    id: "removed".to_string(),
                    name: "Reused".to_string(),
                    shortcut: "Control+Option+Space".to_string(),
                    language: Language::English,
                    push_to_talk_shortcut: None,
                    toggle_shortcut: None,
                },
            ],
            PIANISSIMO_SUPPORTED,
        )
        .unwrap_err()
        .to_string();

    assert!(error.contains("inactive personal dictionary"));
    assert_eq!(
        settings
            .profile_glossaries
            .get("removed")
            .map(String::as_str),
        Some("Lovable = love a ball")
    );
}

#[test]
fn empty_removed_profile_glossary_does_not_reserve_its_old_id() {
    let mut settings = Settings::default();
    settings
        .profile_glossaries
        .insert("removed".to_string(), "  \n".to_string());

    settings
        .replace_hotkey_profiles_gated(
            vec![
                HotkeyProfile::legacy_default("Control+Shift+Space".to_string(), Language::Swedish),
                HotkeyProfile {
                    id: "removed".to_string(),
                    name: "Reused".to_string(),
                    shortcut: "Control+Option+Space".to_string(),
                    language: Language::English,
                    push_to_talk_shortcut: None,
                    toggle_shortcut: None,
                },
            ],
            PIANISSIMO_SUPPORTED,
        )
        .unwrap();
}

#[test]
fn profile_with_dictionary_cannot_change_language_in_place() {
    let mut settings = Settings {
        language: Language::Swedish,
        ..Default::default()
    };
    settings
        .profile_glossaries
        .insert("default".to_string(), "mergea = mördsa".to_string());

    let error = settings
        .replace_hotkey_profiles_gated(
            vec![HotkeyProfile::legacy_default(
                "Control+Shift+Space".to_string(),
                Language::English,
            )],
            PIANISSIMO_SUPPORTED,
        )
        .unwrap_err()
        .to_string();

    assert!(error.contains("personal dictionary"));
    assert!(error.contains("language"));
    assert_eq!(settings.language, Language::Swedish);
}

#[test]
fn migrated_hint_only_dictionary_does_not_lock_profile_language() {
    let mut settings = Settings {
        language: Language::Swedish,
        ..Default::default()
    };
    settings.materialize_profile_models();
    settings
        .profile_glossaries
        .insert("default".into(), "OpenRouter".into());
    settings
        .set_default_profile_language_gated(Language::English, PIANISSIMO_SUPPORTED)
        .unwrap();
    assert_eq!(settings.default_profile().language, Language::English);
    assert_eq!(settings.profile_glossaries["default"], "OpenRouter");
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Auto
    );
}

#[test]
fn legacy_language_change_with_implicit_default_dictionary_is_atomic() {
    let mut settings = Settings {
        language: Language::Swedish,
        ..Default::default()
    };
    settings
        .profile_glossaries
        .insert("default".to_string(), "merge = merch".to_string());

    let error = settings
        .set_legacy_language(Language::English)
        .unwrap_err()
        .to_string();

    assert!(error.contains("personal dictionary"));
    assert_eq!(settings.language, Language::Swedish);
    assert!(settings.hotkey_profiles.is_empty());
    assert_eq!(
        settings
            .profile_glossaries
            .get("default")
            .map(String::as_str),
        Some("merge = merch")
    );
}

#[test]
fn legacy_language_change_with_explicit_default_dictionary_is_atomic() {
    let mut settings = Settings {
        language: Language::English,
        hotkey_profiles: vec![HotkeyProfile::legacy_default(
            "Control+Shift+Space".to_string(),
            Language::Swedish,
        )],
        ..Default::default()
    };
    settings
        .profile_glossaries
        .insert("default".to_string(), "merge = merch".to_string());

    let error = settings
        .set_legacy_language(Language::English)
        .unwrap_err()
        .to_string();

    assert!(error.contains("personal dictionary"));
    assert_eq!(settings.language, Language::English);
    assert_eq!(settings.hotkey_profiles[0].language, Language::Swedish);
}

#[test]
fn legacy_language_change_allows_orphan_dictionary_without_default_profile() {
    let mut settings = Settings {
        language: Language::Swedish,
        hotkey_profiles: vec![HotkeyProfile {
            id: "swedish".to_string(),
            name: "Swedish".to_string(),
            shortcut: "Control+Shift+Space".to_string(),
            language: Language::Swedish,
            push_to_talk_shortcut: None,
            toggle_shortcut: None,
        }],
        ..Default::default()
    };
    settings
        .profile_glossaries
        .insert("default".to_string(), "merge = merch".to_string());

    settings.set_legacy_language(Language::English).unwrap();

    assert_eq!(settings.language, Language::English);
    assert_eq!(settings.hotkey_profiles[0].language, Language::Swedish);
    assert_eq!(
        settings
            .profile_glossaries
            .get("default")
            .map(String::as_str),
        Some("merge = merch")
    );
}

#[test]
fn legacy_language_change_without_dictionary_updates_legacy_profile() {
    let mut settings = Settings {
        language: Language::Swedish,
        hotkey_profiles: vec![HotkeyProfile::legacy_default(
            "Control+Shift+Space".to_string(),
            Language::Swedish,
        )],
        ..Default::default()
    };

    settings.set_legacy_language(Language::English).unwrap();

    assert_eq!(settings.language, Language::English);
    assert_eq!(settings.hotkey_profiles[0].language, Language::English);
}

#[test]
fn settings_effective_model_with_auto_select() {
    let mut s = Settings {
        auto_select_model: true,
        ..Default::default()
    };

    s.language = Language::English;
    assert_eq!(s.effective_model(), WhisperModel::BaseEn);

    s.language = Language::Swedish;
    assert_eq!(s.effective_model(), WhisperModel::KbWhisperMedium);

    s.language = Language::Norwegian;
    assert_eq!(s.effective_model(), WhisperModel::NbWhisperBase);

    s.language = Language::Auto;
    assert_eq!(s.effective_model(), WhisperModel::Base);
}

#[test]
fn settings_effective_model_without_auto_select_uses_compatible_manual_model() {
    let s = Settings {
        auto_select_model: false,
        whisper_model: WhisperModel::KbWhisperSmall,
        language: Language::Swedish,
        ..Default::default()
    };

    assert_eq!(s.effective_model(), WhisperModel::KbWhisperSmall);
}

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

#[test]
fn profiles_choose_independent_compatible_dictation_models() {
    let mut settings = Settings {
        hotkey_profiles: vec![
            profile("swedish", "Super+S", Language::Swedish),
            profile("english", "Super+E", Language::English),
        ],
        ..Default::default()
    };
    settings
        .profile_models
        .insert("swedish".into(), FileModelPreference::PianissimoOriginal);
    settings.profile_models.insert(
        "english".into(),
        FileModelPreference::Whisper(WhisperModel::BaseEn),
    );

    assert_eq!(
        settings
            .dictation_model_for_profile_gated("swedish", PIANISSIMO_SUPPORTED)
            .unwrap(),
        FileModel::PianissimoOriginal
    );
    assert_eq!(
        settings
            .dictation_model_for_profile_gated("english", PIANISSIMO_SUPPORTED)
            .unwrap(),
        FileModel::Whisper(WhisperModel::BaseEn)
    );
    assert!(settings
        .set_profile_model_gated(
            "english",
            FileModelPreference::PianissimoOriginal,
            PIANISSIMO_SUPPORTED
        )
        .is_err());
    assert_eq!(
        settings
            .dictation_model_for_profile_gated("english", PIANISSIMO_SUPPORTED)
            .unwrap(),
        FileModel::Whisper(WhisperModel::BaseEn)
    );
}

#[test]
fn incompatible_language_change_resets_only_that_profile_to_auto_with_notice() {
    let mut swedish = profile("default", "Super+S", Language::Swedish);
    swedish.name = "Svenska".into();
    let mut settings = Settings {
        language: Language::Swedish,
        hotkey_profiles: vec![
            swedish.clone(),
            profile("other", "Super+O", Language::Swedish),
        ],
        ..Default::default()
    };
    settings
        .set_profile_model_gated("default", FileModelPreference::PianissimoOriginal, true)
        .unwrap();
    settings
        .set_profile_model_gated("other", FileModelPreference::PianissimoOriginal, true)
        .unwrap();
    let mut english = swedish;
    english.language = Language::English;
    let notices = settings
        .replace_hotkey_profiles_gated(
            vec![english, profile("other", "Super+O", Language::Swedish)],
            PIANISSIMO_SUPPORTED,
        )
        .unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].profile_id, "default");
    assert_eq!(
        notices[0].message,
        "Model for profile 'Svenska' changed to Recommended (Whisper Base (EN)) because Pianissimo only supports Swedish."
    );
    assert_eq!(settings.default_profile().language, Language::English);
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Auto
    );
    assert_eq!(
        settings
            .dictation_model_for_profile_gated("default", PIANISSIMO_SUPPORTED)
            .unwrap(),
        FileModel::Whisper(WhisperModel::BaseEn)
    );
    assert_eq!(
        settings.profile_models["other"],
        FileModelPreference::PianissimoOriginal
    );
}

#[test]
fn incompatible_whisper_model_is_reset_with_notice_naming_both_models() {
    let mut settings = Settings {
        language: Language::Swedish,
        hotkey_profiles: vec![profile("default", "Super+S", Language::Swedish)],
        ..Default::default()
    };
    settings
        .set_profile_model_gated(
            "default",
            FileModelPreference::Whisper(WhisperModel::KbWhisperBase),
            PIANISSIMO_SUPPORTED,
        )
        .unwrap();
    let notices = settings
        .set_default_profile_language_gated(Language::English, PIANISSIMO_SUPPORTED)
        .unwrap();
    assert_eq!(notices.len(), 1);
    assert!(
        notices[0]
            .message
            .contains("KB-Whisper Base does not support English"),
        "{}",
        notices[0].message
    );
    assert!(
        notices[0]
            .message
            .contains("Recommended (Whisper Base (EN))"),
        "{}",
        notices[0].message
    );
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Auto
    );
}

#[test]
fn compatible_language_change_keeps_explicit_model_without_notice() {
    let mut settings = Settings {
        hotkey_profiles: vec![profile("default", "Super+E", Language::English)],
        ..Default::default()
    };
    settings
        .set_profile_model_gated(
            "default",
            FileModelPreference::Whisper(WhisperModel::Base),
            PIANISSIMO_SUPPORTED,
        )
        .unwrap();
    let mut norwegian = profile("default", "Super+E", Language::Norwegian);
    norwegian.name = "default".into();
    let notices = settings
        .replace_hotkey_profiles_gated(vec![norwegian], PIANISSIMO_SUPPORTED)
        .unwrap();
    assert!(notices.is_empty());
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Whisper(WhisperModel::Base)
    );
}

#[test]
fn reapplying_or_compatibly_changing_default_language_keeps_selected_model() {
    let mut settings = Settings {
        language: Language::Swedish,
        hotkey_profiles: vec![profile("default", "Super+S", Language::Swedish)],
        ..Default::default()
    };
    settings
        .set_profile_model_gated(
            "default",
            FileModelPreference::Whisper(WhisperModel::KbWhisperBase),
            PIANISSIMO_SUPPORTED,
        )
        .unwrap();
    settings
        .set_default_profile_language_gated(Language::Swedish, PIANISSIMO_SUPPORTED)
        .unwrap();
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Whisper(WhisperModel::KbWhisperBase)
    );
    settings
        .set_profile_model_gated(
            "default",
            FileModelPreference::Whisper(WhisperModel::Base),
            PIANISSIMO_SUPPORTED,
        )
        .unwrap();
    settings
        .set_default_profile_language_gated(Language::English, PIANISSIMO_SUPPORTED)
        .unwrap();
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Whisper(WhisperModel::Base)
    );
}

#[test]
fn warm_model_plan_uses_profile_models_instead_of_legacy_global_choice() {
    let mut settings = Settings {
        hotkey_profiles: vec![
            profile("default", "Super+E", Language::English),
            profile("swedish", "Super+S", Language::Swedish),
        ],
        ..Default::default()
    };
    settings.profile_models.insert(
        "default".into(),
        FileModelPreference::Whisper(WhisperModel::SmallEn),
    );
    settings.profile_models.insert(
        "swedish".into(),
        FileModelPreference::Whisper(WhisperModel::KbWhisperSmall),
    );
    assert_eq!(
        settings.warm_model_plan_gated(2, 1000, false),
        vec![
            (WhisperModel::SmallEn, Language::English),
            (WhisperModel::KbWhisperSmall, Language::Swedish),
        ]
    );
}

#[test]
fn migrated_auto_profile_keeps_hints_without_cross_language_alias_replacement() {
    let mut settings = Settings {
        hotkey_profiles: vec![profile("auto", "Super+A", Language::Auto)],
        profile_glossary_migrated: true,
        ..Default::default()
    };
    settings
        .profile_glossaries
        .insert("auto".into(), "OpenRouter = open router".into());
    let source = settings.effective_glossary_source(Some("auto"));
    assert!(source.contains("OpenRouter"));
    assert_eq!(
        Glossary::parse(&source).correct_text("open router").0,
        "open router"
    );
}

#[test]
fn one_run_prompt_is_hint_only_while_profile_aliases_still_apply() {
    let mut settings = Settings {
        hotkey_profiles: vec![profile("default", "Super+S", Language::Swedish)],
        profile_glossary_migrated: true,
        ..Default::default()
    };
    settings
        .profile_glossaries
        .insert("default".into(), "merge = merch".into());
    let source = settings
        .effective_glossary_source_with_prompt(Some("default"), Some("Cloudflare = cloud flare"));
    let glossary = Glossary::parse(&source);
    assert_eq!(
        glossary.correct_text("cloud flare merch").0,
        "cloud flare merge"
    );
    assert!(glossary.decoder_prompt().unwrap().contains("Cloudflare"));
}

#[test]
fn repeated_migration_preserves_multiple_hints_without_duplicate_entries() {
    let mut settings = Settings {
        initial_prompt: "OpenRouter = open router\nCloudflare = cloud flare".into(),
        ..Default::default()
    };
    settings.materialize_profile_models();
    settings.migrate_global_glossary_to_profiles();
    let once = settings.profile_glossaries["default"].clone();
    let glossary = Glossary::parse(&once);
    assert_eq!(glossary.entries().len(), 2);
    assert!(glossary
        .entries()
        .iter()
        .all(|entry| entry.aliases.is_empty()));
    settings.profile_glossary_migrated = false; // an older version lost the new marker
    settings.migrate_global_glossary_to_profiles();
    assert_eq!(settings.profile_glossaries["default"], once);
}

#[test]
fn legacy_settings_resolve_to_one_default_hotkey_profile() {
    let settings: Settings =
        serde_json::from_str(r#"{"language":"sv","hotkey":"Option+Space"}"#).unwrap();
    let profiles = settings.resolved_hotkey_profiles();
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].id, "default");
    assert_eq!(profiles[0].shortcut, "Option+Space");
    assert_eq!(profiles[0].language, Language::Swedish);
}

#[test]
fn hotkey_profiles_roundtrip_and_lookup_aliases() {
    let mut settings = Settings::default();
    settings
        .replace_hotkey_profiles_gated(
            vec![
                profile("default", "Control+Shift+A", Language::English),
                profile("swedish", "Option+Space", Language::Swedish),
            ],
            PIANISSIMO_SUPPORTED,
        )
        .unwrap();
    let json = serde_json::to_string(&settings).unwrap();
    let loaded: Settings = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded.hotkey_profiles, settings.hotkey_profiles);
    assert_eq!(
        loaded.hotkey_profile_for_shortcut("Alt+Space").unwrap().id,
        "swedish"
    );
}

#[test]
fn legacy_profile_binding_uses_global_push_or_toggle_mode() {
    let mut settings = Settings::default();
    settings
        .replace_hotkey_profiles_gated(
            vec![profile("default", "Control+Shift+A", Language::English)],
            PIANISSIMO_SUPPORTED,
        )
        .unwrap();

    settings.hotkey_mode = HotkeyMode::PushToTalk;
    assert_eq!(settings.resolved_hotkey_bindings()[0].1, "Control+Shift+A");
    assert_eq!(
        settings.resolved_hotkey_bindings()[0].2,
        HotkeyMode::PushToTalk
    );

    settings.hotkey_mode = HotkeyMode::Toggle;
    assert_eq!(settings.resolved_hotkey_bindings()[0].2, HotkeyMode::Toggle);
}

#[test]
fn explicit_profile_bindings_resolve_both_modes_independent_of_global_mode() {
    let mut explicit = profile("default", "Control+Shift+A", Language::English);
    explicit.push_to_talk_shortcut = Some("Control+Shift+P".to_string());
    explicit.toggle_shortcut = Some("Control+Shift+T".to_string());
    let mut settings = Settings {
        hotkey_mode: HotkeyMode::Toggle,
        ..Settings::default()
    };
    settings
        .replace_hotkey_profiles_gated(vec![explicit], PIANISSIMO_SUPPORTED)
        .unwrap();

    let bindings = settings.resolved_hotkey_bindings();
    assert_eq!(bindings.len(), 2);
    assert_eq!(bindings[0].1, "Control+Shift+P");
    assert_eq!(bindings[0].2, HotkeyMode::PushToTalk);
    assert_eq!(bindings[1].1, "Control+Shift+T");
    assert_eq!(bindings[1].2, HotkeyMode::Toggle);
    assert_eq!(
        settings.resolved_shortcuts(),
        vec!["Control+Shift+P", "Control+Shift+T"]
    );
    assert_eq!(
        settings
            .hotkey_profile_for_shortcut("control+shift+t")
            .unwrap()
            .id,
        "default"
    );
    assert_eq!(settings.hotkey_profiles[0].shortcut, "Control+Shift+P");

    let json = serde_json::to_string(&settings).unwrap();
    let loaded: Settings = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded.hotkey_profiles, settings.hotkey_profiles);
    assert!(json.contains("push_to_talk_shortcut"));
    assert!(json.contains("toggle_shortcut"));
}

#[test]
fn explicit_profile_bindings_reject_empty_or_canonical_duplicates() {
    let mut empty = profile("default", "Control+Shift+E", Language::English);
    empty.push_to_talk_shortcut = Some("  ".to_string());
    assert!(Settings::validate_hotkey_profiles(&[empty]).is_err());

    let mut first = profile("first", "Control+Shift+F", Language::English);
    first.push_to_talk_shortcut = Some("Option+Shift+A".to_string());
    let mut second = profile("second", "Control+Shift+G", Language::Swedish);
    second.toggle_shortcut = Some("shift+alt+KeyA".to_string());
    assert!(Settings::validate_hotkey_profiles(&[first, second]).is_err());
}

#[test]
fn explicit_bindings_allow_an_additional_modifier() {
    let mut first = profile("first", "Control+Shift+P", Language::English);
    first.push_to_talk_shortcut = Some("Control+Shift+A".to_string());
    let mut second = profile("second", "Control+Shift+Q", Language::Swedish);
    second.push_to_talk_shortcut = Some("Control+Option+Shift+A".to_string());
    assert!(Settings::validate_hotkey_profiles(&[first, second]).is_ok());
}

#[test]
fn legacy_hotkey_update_tracks_explicit_primary_binding() {
    let mut push_to_talk = profile("default", "Control+Shift+P", Language::English);
    push_to_talk.push_to_talk_shortcut = Some("Control+Shift+Q".to_string());
    let mut settings = Settings::default();
    settings
        .replace_hotkey_profiles_gated(vec![push_to_talk], PIANISSIMO_SUPPORTED)
        .unwrap();
    settings
        .try_set_legacy_hotkey("Control+Shift+R".to_string())
        .unwrap();
    assert_eq!(settings.hotkey, "Control+Shift+R");
    assert_eq!(settings.hotkey_profiles[0].shortcut, "Control+Shift+R");
    assert_eq!(
        settings.hotkey_profiles[0].push_to_talk_shortcut.as_deref(),
        Some("Control+Shift+R")
    );

    let mut toggle = profile("default", "Control+Shift+P", Language::English);
    toggle.toggle_shortcut = Some("Control+Shift+Q".to_string());
    let mut settings = Settings::default();
    settings
        .replace_hotkey_profiles_gated(vec![toggle], PIANISSIMO_SUPPORTED)
        .unwrap();
    settings
        .try_set_legacy_hotkey("Control+Shift+R".to_string())
        .unwrap();
    assert_eq!(
        settings.hotkey_profiles[0].toggle_shortcut.as_deref(),
        Some("Control+Shift+R")
    );
}

#[test]
fn the_default_profile_is_the_one_called_default_else_the_first() {
    let profiles = [
        profile("english", "Super+E", Language::English),
        profile("default", "Super+D", Language::Swedish),
    ];
    assert_eq!(HotkeyProfile::default_index(&profiles), 1);
    assert_eq!(HotkeyProfile::default_index(&profiles[..1]), 0);
}

/// With explicit profiles and none called `default`, the legacy hotkey update
/// used to write `hotkey` alone, which no binding reads (#373). It now changes
/// the first profile, the one `default_profile()` returns, as the app's
/// `set_hotkey` always did.
#[test]
fn legacy_hotkey_update_edits_the_first_profile_when_none_is_called_default() {
    let mut settings = Settings::default();
    settings
        .replace_hotkey_profiles_gated(
            vec![
                profile("english", "Super+E", Language::English),
                profile("swedish", "Super+S", Language::Swedish),
            ],
            PIANISSIMO_SUPPORTED,
        )
        .unwrap();
    assert_eq!(settings.default_profile().id, "english");
    settings
        .try_set_legacy_hotkey("Control+Alt+D".to_string())
        .unwrap();
    assert_eq!(settings.hotkey, "Control+Alt+D");
    assert_eq!(settings.hotkey_profiles[0].shortcut, "Control+Alt+D");
    assert_eq!(settings.hotkey_profiles[1].shortcut, "Super+S");
    assert_eq!(
        settings.resolved_shortcuts(),
        vec!["Control+Alt+D".to_string(), "Super+S".to_string()]
    );
}

#[test]
fn legacy_hotkey_update_without_explicit_profiles_moves_the_implicit_one() {
    let mut settings = Settings::default();
    assert!(settings.hotkey_profiles.is_empty());
    settings
        .try_set_legacy_hotkey("Control+Alt+D".to_string())
        .unwrap();
    assert!(settings.hotkey_profiles.is_empty());
    assert_eq!(settings.resolved_shortcuts(), vec!["Control+Alt+D"]);
}

#[test]
fn replacing_profiles_syncs_legacy_fields_from_default() {
    let mut settings = Settings::default();
    settings
        .replace_hotkey_profiles_gated(
            vec![
                profile("swedish", "Option+Space", Language::Swedish),
                profile("default", "Control+Shift+E", Language::English),
            ],
            PIANISSIMO_SUPPORTED,
        )
        .unwrap();
    assert_eq!(settings.hotkey, "Control+Shift+E");
    assert_eq!(settings.language, Language::English);
}

#[test]
fn profile_validation_rejects_invalid_and_duplicate_values() {
    assert!(Settings::validate_hotkey_profiles(&[]).is_err());
    assert!(Settings::validate_hotkey_profiles(&[profile(
        "Bad ID",
        "Option+A",
        Language::English
    )])
    .is_err());
    assert!(
        Settings::validate_hotkey_profiles(&[profile("default", "Space", Language::English)])
            .is_err()
    );

    let mut blank = profile("default", "Option+A", Language::English);
    blank.name = "  ".to_string();
    assert!(Settings::validate_hotkey_profiles(&[blank]).is_err());

    let duplicate_ids = vec![
        profile("same", "Option+A", Language::English),
        profile("same", "Option+B", Language::Swedish),
    ];
    assert!(Settings::validate_hotkey_profiles(&duplicate_ids).is_err());

    let duplicate_aliases = vec![
        profile("one", "Option+Shift+A", Language::English),
        profile("two", "shift+alt+KeyA", Language::Swedish),
    ];
    assert!(Settings::validate_hotkey_profiles(&duplicate_aliases).is_err());

    let duplicate_bare_function_keys = vec![
        profile("one", "F13", Language::English),
        profile("two", "f13", Language::Swedish),
    ];
    assert!(Settings::validate_hotkey_profiles(&duplicate_bare_function_keys).is_err());
}

#[test]
fn effective_model_can_be_selected_for_profile_language() {
    let settings = Settings {
        auto_select_model: true,
        ..Default::default()
    };
    assert_eq!(
        settings.effective_model_for(Language::Swedish),
        WhisperModel::KbWhisperMedium
    );
    assert_eq!(
        settings.effective_model_for(Language::Norwegian),
        WhisperModel::NbWhisperBase
    );
}

#[test]
fn incompatible_manual_model_falls_back_for_profile_language() {
    let settings = Settings {
        auto_select_model: false,
        whisper_model: WhisperModel::KbWhisperBase,
        ..Default::default()
    };

    assert_eq!(
        settings.effective_model_for(Language::Swedish),
        WhisperModel::KbWhisperBase
    );
    assert_eq!(
        settings.effective_model_for(Language::English),
        WhisperModel::BaseEn
    );
    assert_eq!(
        settings.effective_model_for(Language::Norwegian),
        WhisperModel::NbWhisperBase
    );
}

#[test]
fn compatible_manual_multilingual_model_is_shared_across_profiles() {
    let settings = Settings {
        auto_select_model: false,
        whisper_model: WhisperModel::Medium,
        ..Default::default()
    };

    for language in [
        Language::English,
        Language::Swedish,
        Language::Norwegian,
        Language::Auto,
    ] {
        assert_eq!(settings.effective_model_for(language), WhisperModel::Medium);
    }
}

#[test]
fn legacy_settings_migrate_file_model_preference_to_auto() {
    let settings: Settings =
        serde_json::from_str(r#"{"language":"sv","whisper_model":"kb-whisper-small"}"#).unwrap();

    assert_eq!(settings.file_transcription_model, FileModelPreference::Auto);
}

#[test]
fn auto_file_model_preference_inherits_live_effective_model() {
    let settings = Settings {
        auto_select_model: true,
        file_transcription_model: FileModelPreference::Auto,
        ..Default::default()
    };

    assert_eq!(
        settings.effective_file_model_for_gated(Language::Swedish, false, &|_| true),
        Ok(FileModel::Whisper(WhisperModel::KbWhisperMedium))
    );
}

#[test]
fn selected_file_model_must_match_language() {
    let settings = Settings {
        file_transcription_model: FileModelPreference::Whisper(WhisperModel::KbWhisperBase),
        ..Default::default()
    };
    assert!(settings
        .effective_file_model_for_gated(
            Language::English,
            PIANISSIMO_SUPPORTED,
            &nothing_downloaded
        )
        .is_err());

    let settings = Settings {
        file_transcription_model: FileModelPreference::PianissimoOriginal,
        ..Default::default()
    };
    assert_eq!(
        settings.effective_file_model_for_gated(
            Language::Swedish,
            PIANISSIMO_SUPPORTED,
            &nothing_downloaded
        ),
        Ok(FileModel::PianissimoOriginal)
    );
    assert!(settings
        .effective_file_model_for_gated(
            Language::Norwegian,
            PIANISSIMO_SUPPORTED,
            &nothing_downloaded
        )
        .is_err());
}

#[test]
fn file_model_preference_does_not_change_live_model_resolution() {
    let settings = Settings {
        auto_select_model: false,
        whisper_model: WhisperModel::Medium,
        file_transcription_model: FileModelPreference::PianissimoOriginal,
        ..Default::default()
    };

    assert_eq!(
        settings.effective_model_for(Language::English),
        WhisperModel::Medium
    );
    assert_eq!(
        settings.effective_file_model_for_gated(
            Language::Swedish,
            PIANISSIMO_SUPPORTED,
            &nothing_downloaded
        ),
        Ok(FileModel::PianissimoOriginal)
    );
}

#[test]
fn file_model_preference_serde_roundtrip_and_unknown_ids() {
    let pairs = [
        (FileModelPreference::Auto, "\"auto\""),
        (
            FileModelPreference::Whisper(WhisperModel::KbWhisperSmall),
            "\"kb-whisper-small\"",
        ),
        (FileModelPreference::PianissimoOriginal, "\"pianissimo-sv\""),
    ];

    for (preference, expected) in pairs {
        let json = serde_json::to_string(&preference).unwrap();
        assert_eq!(json, expected);
        assert_eq!(
            serde_json::from_str::<FileModelPreference>(&json).unwrap(),
            preference
        );
    }
    assert!(FileModelPreference::parse_id("unknown-model").is_err());
    assert!(serde_json::from_str::<FileModelPreference>("\"unknown-model\"").is_err());
}

#[test]
fn file_model_preference_ids_are_the_model_ids() {
    for model in WhisperModel::ALL {
        let expected = FileModelPreference::Whisper(model);
        assert_eq!(FileModelPreference::parse_id(model.id()), Ok(expected));
        assert_eq!(
            serde_json::to_value(expected).unwrap(),
            serde_json::Value::String(model.id().to_owned())
        );
    }
    assert_eq!(
        FileModelPreference::parse_id(FileModel::PianissimoOriginal.id()),
        Ok(FileModelPreference::PianissimoOriginal)
    );
    assert_eq!(
        serde_json::to_value(FileModelPreference::PianissimoOriginal).unwrap(),
        serde_json::Value::String(FileModel::PIANISSIMO_ID.to_owned())
    );
    assert_eq!(
        FileModelPreference::parse_id("auto"),
        Ok(FileModelPreference::Auto)
    );
    for preference in [
        FileModelPreference::Auto,
        FileModelPreference::Whisper(WhisperModel::KbWhisperSmall),
        FileModelPreference::PianissimoOriginal,
    ] {
        assert_eq!(
            FileModelPreference::parse_id(preference.id()),
            Ok(preference)
        );
    }
    assert_eq!(
        FileModelPreference::parse_id("large-v3"),
        Err(SettingsError::UnknownModelPreference {
            id: "large-v3".to_owned()
        })
    );
}

#[test]
fn warm_model_plan_includes_swedish_and_english_base_models() {
    let mut settings = Settings {
        auto_select_model: false,
        whisper_model: WhisperModel::KbWhisperBase,
        ..Default::default()
    };
    settings
        .replace_hotkey_profiles_gated(
            vec![
                profile("default", "Super+S", Language::Swedish),
                profile("english", "Super+E", Language::English),
            ],
            PIANISSIMO_SUPPORTED,
        )
        .unwrap();
    settings
        .set_profile_model_gated(
            "default",
            FileModelPreference::Whisper(WhisperModel::KbWhisperBase),
            false,
        )
        .unwrap();

    assert_eq!(
        settings.warm_model_plan_gated(2, 384, false),
        vec![
            (WhisperModel::KbWhisperBase, Language::Swedish),
            (WhisperModel::BaseEn, Language::English),
        ]
    );
}

#[test]
fn warm_model_plan_skips_pianissimo_profiles_but_keeps_whisper_profiles() {
    let mut settings = Settings {
        pianissimo_dictation: true,
        auto_select_model: false,
        whisper_model: WhisperModel::KbWhisperBase,
        ..Default::default()
    };
    settings
        .replace_hotkey_profiles_gated(
            vec![
                profile("default", "Super+S", Language::Swedish),
                profile("english", "Super+E", Language::English),
            ],
            PIANISSIMO_SUPPORTED,
        )
        .unwrap();
    settings
        .set_profile_model_gated("default", FileModelPreference::PianissimoOriginal, true)
        .unwrap();

    assert_eq!(
        settings.warm_model_plan_gated(2, 384, false),
        vec![(WhisperModel::BaseEn, Language::English)]
    );

    settings
        .replace_hotkey_profiles_gated(
            vec![profile("default", "Super+S", Language::Swedish)],
            PIANISSIMO_SUPPORTED,
        )
        .unwrap();
    assert!(settings.warm_model_plan_gated(2, 384, false).is_empty());
}

#[test]
fn warm_model_plan_deduplicates_multilingual_model() {
    let mut settings = Settings {
        auto_select_model: false,
        whisper_model: WhisperModel::Base,
        ..Default::default()
    };
    settings
        .replace_hotkey_profiles_gated(
            vec![
                profile("default", "Super+S", Language::Swedish),
                profile("english", "Super+E", Language::English),
            ],
            PIANISSIMO_SUPPORTED,
        )
        .unwrap();
    settings
        .set_profile_model_gated(
            "default",
            FileModelPreference::Whisper(WhisperModel::Base),
            PIANISSIMO_SUPPORTED,
        )
        .unwrap();
    settings
        .set_profile_model_gated(
            "english",
            FileModelPreference::Whisper(WhisperModel::Base),
            PIANISSIMO_SUPPORTED,
        )
        .unwrap();

    assert_eq!(
        settings.warm_model_plan_gated(2, 384, false),
        vec![(WhisperModel::Base, Language::Swedish)]
    );
}

#[test]
fn warm_model_plan_moves_default_profile_to_front() {
    let mut settings = Settings::default();
    settings
        .replace_hotkey_profiles_gated(
            vec![
                profile("english", "Super+E", Language::English),
                profile("default", "Super+S", Language::Swedish),
            ],
            PIANISSIMO_SUPPORTED,
        )
        .unwrap();
    settings
        .set_profile_model_gated(
            "default",
            FileModelPreference::Whisper(WhisperModel::KbWhisperBase),
            false,
        )
        .unwrap();

    assert_eq!(
        settings.warm_model_plan_gated(2, 384, false),
        vec![
            (WhisperModel::KbWhisperBase, Language::Swedish),
            (WhisperModel::BaseEn, Language::English),
        ]
    );
}

#[test]
fn warm_model_plan_budget_and_zero_capacity_keep_primary() {
    let mut settings = Settings::default();
    settings
        .replace_hotkey_profiles_gated(
            vec![
                profile("default", "Super+S", Language::Swedish),
                profile("english", "Super+E", Language::English),
            ],
            PIANISSIMO_SUPPORTED,
        )
        .unwrap();
    settings
        .set_profile_model_gated(
            "default",
            FileModelPreference::Whisper(WhisperModel::KbWhisperBase),
            false,
        )
        .unwrap();

    let primary = vec![(WhisperModel::KbWhisperBase, Language::Swedish)];
    assert_eq!(settings.warm_model_plan_gated(2, 100, false), primary);
    assert_eq!(settings.warm_model_plan_gated(0, 1_000, false), primary);
}

#[test]
fn english_only_manual_model_is_not_reused_for_other_languages() {
    let settings = Settings {
        auto_select_model: false,
        whisper_model: WhisperModel::MediumEn,
        ..Default::default()
    };

    assert_eq!(
        settings.effective_model_for(Language::English),
        WhisperModel::MediumEn
    );
    assert_eq!(
        settings.effective_model_for(Language::Swedish),
        WhisperModel::KbWhisperMedium
    );
    assert_eq!(
        settings.effective_model_for(Language::Norwegian),
        WhisperModel::NbWhisperBase
    );
    assert_eq!(
        settings.effective_model_for(Language::Auto),
        WhisperModel::Base
    );
}

#[test]
fn settings_serde_roundtrip() {
    let original = Settings::default();
    let json = serde_json::to_string(&original).unwrap();
    assert!(json.contains(r#""hotkey_mode":"push""#));
    assert!(!json.contains("presenter"));
    let deserialized: Settings = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.language, original.language);
    assert_eq!(deserialized.whisper_model, original.whisper_model);
    assert_eq!(deserialized.hotkey_mode, original.hotkey_mode);
    assert_eq!(deserialized.show_overlay, original.show_overlay);
    assert_eq!(deserialized.auto_paste, original.auto_paste);
    assert_eq!(deserialized.auto_select_model, original.auto_select_model);
    assert_eq!(deserialized.hotkey, original.hotkey);
    assert_eq!(deserialized.initial_prompt, original.initial_prompt);
    assert_eq!(deserialized.beam_size, original.beam_size);
    assert_eq!(
        deserialized.temperature_fallback,
        original.temperature_fallback
    );
    assert_eq!(deserialized.vad_enabled, original.vad_enabled);
    assert_eq!(
        deserialized.has_completed_onboarding,
        original.has_completed_onboarding
    );
}

#[test]
fn settings_accepts_legacy_camel_case_onboarding_key() {
    let settings: Settings = serde_json::from_str(r#"{"hasCompletedOnboarding":true}"#).unwrap();
    assert!(settings.has_completed_onboarding);
}

#[test]
fn settings_deserialized_with_missing_fields_uses_defaults() {
    // serde(default) should fill in missing fields
    let json = r#"{"language":"sv"}"#;
    let s: Settings = serde_json::from_str(json).unwrap();
    assert_eq!(s.language, Language::Swedish);
    // All other fields should be defaults
    assert_eq!(s.whisper_model, WhisperModel::Base);
    assert!(s.auto_paste);
    assert_eq!(s.hotkey, "Control+Shift+Space");
}

// -- Model consistency --

#[test]
fn engine_defaults_migration_moves_only_users_still_on_the_old_defaults() {
    let old = |prewarm, minutes| Settings {
        engine_prewarm: prewarm,
        engine_idle_unload_minutes: minutes,
        engine_defaults_version: 0,
        ..Default::default()
    };
    let mut untouched = old(EnginePrewarm::OnKeyDown, 10);
    assert!(untouched.migrate_engine_defaults());
    assert_eq!(untouched.engine_prewarm, EnginePrewarm::OnAppStart);
    assert_eq!(untouched.engine_idle_unload_minutes, 60);
    assert_eq!(untouched.engine_defaults_version, ENGINE_DEFAULTS_VERSION);

    let mut chose_off = old(EnginePrewarm::Off, 10);
    assert!(chose_off.migrate_engine_defaults());
    assert_eq!(chose_off.engine_prewarm, EnginePrewarm::Off);
    assert_eq!(chose_off.engine_idle_unload_minutes, 60);

    let mut chose_timeout = old(EnginePrewarm::OnAppStart, 3);
    chose_timeout.migrate_engine_defaults();
    assert_eq!(chose_timeout.engine_prewarm, EnginePrewarm::OnAppStart);
    assert_eq!(chose_timeout.engine_idle_unload_minutes, 3);

    let mut never_unload = old(EnginePrewarm::OnKeyDown, 0);
    never_unload.migrate_engine_defaults();
    assert_eq!(never_unload.engine_idle_unload_minutes, 0);
}

#[test]
fn engine_defaults_migration_runs_once() {
    let mut settings = Settings {
        engine_prewarm: EnginePrewarm::OnKeyDown,
        engine_idle_unload_minutes: 10,
        engine_defaults_version: 0,
        ..Default::default()
    };
    assert!(settings.migrate_engine_defaults());
    // The user later picks the old values again on purpose: kept.
    settings.engine_prewarm = EnginePrewarm::OnKeyDown;
    settings.engine_idle_unload_minutes = 10;
    assert!(!settings.migrate_engine_defaults());
    assert_eq!(settings.engine_prewarm, EnginePrewarm::OnKeyDown);
    assert_eq!(settings.engine_idle_unload_minutes, 10);
    assert_eq!(
        Settings::default().engine_defaults_version,
        ENGINE_DEFAULTS_VERSION
    );
    let legacy: Settings = serde_json::from_str(r#"{"engine_prewarm":"on_key_down"}"#).unwrap();
    assert_eq!(legacy.engine_defaults_version, 0);
}

#[test]
fn engine_settings_default_roundtrip_and_tolerate_missing_keys() {
    let defaults = Settings::default();
    assert_eq!(defaults.engine_prewarm, EnginePrewarm::OnAppStart);
    assert_eq!(defaults.engine_idle_unload_minutes, 60);
    let legacy: Settings = serde_json::from_str(r#"{"language":"sv"}"#).unwrap();
    assert_eq!(legacy.engine_prewarm, EnginePrewarm::OnAppStart);
    assert_eq!(legacy.engine_idle_unload_minutes, 60);
    let json = serde_json::to_value(Settings {
        engine_prewarm: EnginePrewarm::OnAppStart,
        engine_idle_unload_minutes: 3,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(json["engine_prewarm"], "on_app_start");
    assert_eq!(json["engine_idle_unload_minutes"], 3);
    for mode in EnginePrewarm::ALL {
        assert_eq!(EnginePrewarm::parse_id(mode.id()).unwrap(), mode);
        assert_eq!(serde_json::to_value(mode).unwrap(), mode.id());
    }
    assert!(EnginePrewarm::parse_id("always").is_err());
}

#[test]
fn unsupported_system_demotes_every_pianissimo_selection_to_recommended_whisper() {
    let mut settings = Settings {
        language: Language::Swedish,
        pianissimo_dictation: true,
        file_transcription_model: FileModelPreference::PianissimoOriginal,
        hotkey_profiles: vec![
            profile("default", "Super+S", Language::Swedish),
            profile("second", "Super+T", Language::Swedish),
        ],
        ..Default::default()
    };
    settings.materialize_profile_models();
    settings.profile_models.insert(
        "second".into(),
        FileModelPreference::Whisper(WhisperModel::KbWhisperBase),
    );
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::PianissimoOriginal
    );

    assert!(
        !settings.clone().demote_unsupported_pianissimo(true),
        "supported: untouched"
    );
    assert!(settings.demote_unsupported_pianissimo(false));

    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Auto
    );
    assert_eq!(
        settings.profile_models["second"],
        FileModelPreference::Whisper(WhisperModel::KbWhisperBase)
    );
    assert_eq!(settings.file_transcription_model, FileModelPreference::Auto);
    assert!(!settings.pianissimo_dictation);
    assert_eq!(
        settings
            .dictation_model_for_profile_gated("default", false)
            .unwrap(),
        FileModel::Whisper(WhisperModel::recommended(Language::Swedish))
    );
    assert!(!settings.demote_unsupported_pianissimo(false), "idempotent");
}

#[test]
fn the_quiet_demotion_leaves_what_the_logging_one_leaves() {
    let mut loud = Settings {
        language: Language::Swedish,
        pianissimo_dictation: true,
        file_transcription_model: FileModelPreference::PianissimoOriginal,
        hotkey_profiles: vec![profile("default", "Super+S", Language::Swedish)],
        ..Default::default()
    };
    loud.materialize_profile_models();
    let mut quiet = loud.clone();

    assert!(!quiet.clone().demote_unsupported_pianissimo_quietly(true));
    assert!(loud.demote_unsupported_pianissimo(false));
    assert!(quiet.demote_unsupported_pianissimo_quietly(false));

    assert_eq!(
        serde_json::to_value(&quiet).unwrap(),
        serde_json::to_value(&loud).unwrap()
    );
    assert!(
        !quiet.demote_unsupported_pianissimo_quietly(false),
        "idempotent"
    );
}

#[test]
fn selecting_pianissimo_is_refused_when_the_platform_gate_is_closed() {
    let mut settings = Settings {
        language: Language::Swedish,
        hotkey_profiles: vec![profile("default", "Super+S", Language::Swedish)],
        ..Default::default()
    };
    let error = settings
        .set_profile_model_gated("default", FileModelPreference::PianissimoOriginal, false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("macOS 14"), "{error}");
    assert_ne!(
        settings.profile_models.get("default"),
        Some(&FileModelPreference::PianissimoOriginal)
    );
}

// -- Swedish model lineup --

#[test]
fn recommended_swedish_resolves_by_platform_everywhere() {
    assert_eq!(
        FileModel::recommended_for(Language::Swedish, true),
        FileModel::PianissimoOriginal
    );
    assert_eq!(
        FileModel::recommended_for(Language::Swedish, false),
        FileModel::Whisper(WhisperModel::KbWhisperMedium)
    );
    assert_eq!(
        WhisperModel::recommended(Language::Swedish),
        WhisperModel::KbWhisperMedium
    );
    assert_eq!(
        FileModel::recommended_for(Language::English, true),
        FileModel::Whisper(WhisperModel::BaseEn)
    );

    let mut settings = Settings {
        language: Language::Swedish,
        ..Default::default()
    };
    settings.materialize_profile_models();
    assert_eq!(settings.file_transcription_model, FileModelPreference::Auto);
    for (gate, expected) in [
        (true, FileModel::PianissimoOriginal),
        (false, FileModel::Whisper(WhisperModel::KbWhisperMedium)),
    ] {
        assert_eq!(
            settings
                .dictation_model_for_profile_gated("default", gate)
                .unwrap(),
            expected
        );
        assert_eq!(
            settings
                .effective_file_model_for_gated(Language::Swedish, gate, &|_| true)
                .unwrap(),
            expected
        );
    }
    // Whisper-only work (meetings, diarization) never gets Pianissimo from Auto.
    assert_eq!(
        settings
            .effective_whisper_file_model_for_gated(Language::Swedish, &|_| true)
            .unwrap(),
        FileModel::Whisper(WhisperModel::KbWhisperMedium)
    );
}

fn ready_set(models: &[FileModel]) -> impl Fn(FileModel) -> bool {
    let models = models.to_vec();
    move |m| models.contains(&m)
}

#[test]
fn auto_file_model_falls_back_to_downloaded_models() {
    let auto = Settings {
        file_transcription_model: FileModelPreference::Auto,
        ..Default::default()
    };
    let base = FileModel::Whisper(WhisperModel::KbWhisperBase);
    let medium = FileModel::Whisper(WhisperModel::KbWhisperMedium);
    let pian = FileModel::PianissimoOriginal;

    // Only Base downloaded: upgrading user keeps working, on both platforms.
    assert_eq!(
        auto.effective_file_model_for_gated(Language::Swedish, true, &ready_set(&[base])),
        Ok(base)
    );
    assert_eq!(
        auto.effective_file_model_for_gated(Language::Swedish, false, &ready_set(&[base])),
        Ok(base)
    );
    // Pianissimo downloaded wins.
    assert_eq!(
        auto.effective_file_model_for_gated(Language::Swedish, true, &ready_set(&[base, pian])),
        Ok(pian)
    );
    // Unsupported Mac, Medium + Base downloaded: Medium.
    assert_eq!(
        auto.effective_file_model_for_gated(Language::Swedish, false, &ready_set(&[base, medium])),
        Ok(medium)
    );
    // Supported, Medium + Base downloaded (no Pianissimo): lineup order gives Large, then Medium.
    assert_eq!(
        auto.effective_file_model_for_gated(Language::Swedish, true, &ready_set(&[base, medium])),
        Ok(medium)
    );
    // Nothing downloaded: the recommended model, so the error names its download.
    assert_eq!(
        auto.effective_file_model_for_gated(Language::Swedish, true, &|_| false),
        Ok(pian)
    );
    assert_eq!(
        auto.effective_file_model_for_gated(Language::Swedish, false, &|_| false),
        Ok(medium)
    );
}

#[test]
fn auto_whisper_file_model_never_returns_pianissimo() {
    let auto = Settings {
        file_transcription_model: FileModelPreference::Auto,
        ..Default::default()
    };
    let base = FileModel::Whisper(WhisperModel::KbWhisperBase);
    let medium = FileModel::Whisper(WhisperModel::KbWhisperMedium);
    assert_eq!(
        auto.effective_whisper_file_model_for_gated(
            Language::Swedish,
            &ready_set(&[FileModel::PianissimoOriginal, base])
        ),
        Ok(base)
    );
    assert_eq!(
        auto.effective_whisper_file_model_for_gated(Language::Swedish, &|m| m
            != FileModel::PianissimoOriginal
            && m == base),
        Ok(base)
    );
    assert_eq!(
        auto.effective_whisper_file_model_for_gated(Language::Swedish, &|_| false),
        Ok(medium)
    );
    assert_eq!(
        auto.effective_whisper_file_model_for_gated(Language::Swedish, &|_| true),
        Ok(medium)
    );
}

#[test]
fn auto_file_model_english_and_explicit_preferences_unchanged() {
    let auto = Settings {
        file_transcription_model: FileModelPreference::Auto,
        ..Default::default()
    };
    let base_en = FileModel::Whisper(WhisperModel::BaseEn);
    assert_eq!(
        auto.effective_file_model_for_gated(Language::English, true, &ready_set(&[base_en])),
        Ok(base_en)
    );
    assert_eq!(
        auto.effective_file_model_for_gated(Language::English, true, &|_| false),
        Ok(base_en)
    );

    let explicit = Settings {
        file_transcription_model: FileModelPreference::Whisper(WhisperModel::KbWhisperLarge),
        ..Default::default()
    };
    assert_eq!(
        explicit.effective_file_model_for_gated(Language::Swedish, true, &|_| false),
        Ok(FileModel::Whisper(WhisperModel::KbWhisperLarge))
    );
    assert!(explicit
        .effective_file_model_for_gated(Language::English, true, &|_| true)
        .is_err());
    let pian = Settings {
        file_transcription_model: FileModelPreference::PianissimoOriginal,
        ..Default::default()
    };
    assert_eq!(
        pian.effective_whisper_file_model_for_gated(Language::Swedish, &|_| false),
        Ok(FileModel::PianissimoOriginal)
    );
}

fn swedish_settings(profile: FileModelPreference) -> Settings {
    let mut settings = Settings {
        language: Language::Swedish,
        model_lineup_version: 0,
        ..Default::default()
    };
    settings.materialize_profile_models();
    settings.profile_models.insert("default".into(), profile);
    settings
}

fn migrate(
    settings: &mut Settings,
    supported: bool,
    pianissimo_downloaded: bool,
    downloaded: &[WhisperModel],
) -> bool {
    let downloaded = downloaded.to_vec();
    let check = move |model: WhisperModel| downloaded.contains(&model);
    settings.migrate_swedish_lineup(&LineupEnv {
        pianissimo_supported: supported,
        pianissimo_downloaded,
        whisper_downloaded: &check,
    })
}

#[test]
fn lineup_migration_matrix() {
    use WhisperModel::*;
    for old in [KbWhisperTiny, KbWhisperBase, KbWhisperSmall] {
        for supported in [true, false] {
            for target_ready in [true, false] {
                for old_ready in [true, false] {
                    let mut downloaded = vec![];
                    if old_ready {
                        downloaded.push(old);
                    }
                    if !supported && target_ready {
                        downloaded.push(KbWhisperMedium);
                    }
                    let mut settings = swedish_settings(FileModelPreference::Whisper(old));
                    assert!(migrate(
                        &mut settings,
                        supported,
                        supported && target_ready,
                        &downloaded
                    ));
                    let target = if supported {
                        FileModelPreference::PianissimoOriginal
                    } else {
                        FileModelPreference::Whisper(KbWhisperMedium)
                    };
                    let expected = if target_ready || !old_ready {
                        target
                    } else {
                        FileModelPreference::Whisper(old)
                    };
                    assert_eq!(
                        settings.profile_models["default"], expected,
                        "old={old:?} supported={supported} target_ready={target_ready} old_ready={old_ready}"
                    );
                    assert_eq!(settings.model_lineup_version, MODEL_LINEUP_VERSION);
                    // Never left without a resolvable engine.
                    assert!(settings
                        .dictation_model_for_profile_gated("default", supported)
                        .is_ok());
                }
            }
        }
    }
}

#[test]
fn lineup_migration_keeps_working_old_model_until_recommended_is_downloaded() {
    let mut settings = swedish_settings(FileModelPreference::Whisper(WhisperModel::KbWhisperSmall));
    migrate(&mut settings, true, false, &[WhisperModel::KbWhisperSmall]);
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Whisper(WhisperModel::KbWhisperSmall)
    );
}

#[test]
fn lineup_migration_pins_auto_to_previous_model_only_when_that_keeps_dictation_working() {
    // Auto used to mean KB-Whisper Base. Base downloaded, Pianissimo missing: pin.
    let mut settings = swedish_settings(FileModelPreference::Auto);
    migrate(&mut settings, true, false, &[WhisperModel::KbWhisperBase]);
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Whisper(WhisperModel::KbWhisperBase)
    );
    // Pianissimo already there: Auto simply follows the new recommendation.
    let mut settings = swedish_settings(FileModelPreference::Auto);
    migrate(&mut settings, true, true, &[WhisperModel::KbWhisperBase]);
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Auto
    );
    // Nothing downloaded: pinning would not help, Auto prompts for the recommendation.
    let mut settings = swedish_settings(FileModelPreference::Auto);
    migrate(&mut settings, true, false, &[]);
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Auto
    );
    // Unsupported platform: Medium is the recommendation.
    let mut settings = swedish_settings(FileModelPreference::Auto);
    migrate(&mut settings, false, false, &[WhisperModel::KbWhisperBase]);
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Whisper(WhisperModel::KbWhisperBase)
    );
}

#[test]
fn lineup_migration_leaves_explicit_and_other_language_choices_alone() {
    for preference in [
        FileModelPreference::Whisper(WhisperModel::KbWhisperMedium),
        FileModelPreference::Whisper(WhisperModel::KbWhisperLarge),
        FileModelPreference::PianissimoOriginal,
    ] {
        let mut settings = swedish_settings(preference);
        migrate(&mut settings, true, true, &[WhisperModel::KbWhisperMedium]);
        assert_eq!(settings.profile_models["default"], preference);
    }
    let mut settings = Settings {
        language: Language::English,
        model_lineup_version: 0,
        ..Default::default()
    };
    settings.materialize_profile_models();
    settings.profile_models.insert(
        "default".into(),
        FileModelPreference::Whisper(WhisperModel::SmallEn),
    );
    migrate(&mut settings, true, true, &[]);
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Whisper(WhisperModel::SmallEn)
    );
}

#[test]
fn lineup_migration_moves_legacy_global_model_only_to_a_downloaded_medium() {
    let mut settings = swedish_settings(FileModelPreference::Auto);
    settings.auto_select_model = false;
    settings.whisper_model = WhisperModel::KbWhisperSmall;
    migrate(&mut settings, true, true, &[WhisperModel::KbWhisperSmall]);
    assert_eq!(settings.whisper_model, WhisperModel::KbWhisperSmall);
    let mut settings = swedish_settings(FileModelPreference::Auto);
    settings.auto_select_model = false;
    settings.whisper_model = WhisperModel::KbWhisperSmall;
    migrate(
        &mut settings,
        true,
        true,
        &[WhisperModel::KbWhisperSmall, WhisperModel::KbWhisperMedium],
    );
    assert_eq!(settings.whisper_model, WhisperModel::KbWhisperMedium);
}

#[test]
fn lineup_migration_is_idempotent_and_new_installs_skip_it() {
    let mut settings = swedish_settings(FileModelPreference::Whisper(WhisperModel::KbWhisperBase));
    assert!(migrate(&mut settings, true, true, &[]));
    let after_first = settings.profile_models.clone();
    // A later manual choice of a retired model (via CLI) is not undone.
    settings.profile_models.insert(
        "default".into(),
        FileModelPreference::Whisper(WhisperModel::KbWhisperTiny),
    );
    assert!(!migrate(&mut settings, true, true, &[]));
    assert_eq!(
        settings.profile_models["default"],
        FileModelPreference::Whisper(WhisperModel::KbWhisperTiny)
    );
    assert_ne!(after_first["default"], settings.profile_models["default"]);
    assert_eq!(
        Settings::default().model_lineup_version,
        MODEL_LINEUP_VERSION
    );
    let old_file: Settings = serde_json::from_str(r#"{"language":"sv"}"#).unwrap();
    assert_eq!(
        old_file.model_lineup_version, 0,
        "files without the marker are migrated"
    );
}
