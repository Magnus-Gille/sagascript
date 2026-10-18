use super::*;
use sagascript_core::settings::HotkeyProfile;

#[cfg(test)]
fn reconcile_global_dictionary_after_conflict(
    settings: &mut Settings,
    expected_source: &str,
    persisted_source: &str,
) {
    if sagascript_core::settings::store::glossary_sources_match(
        &settings.initial_prompt,
        expected_source,
    ) {
        settings.initial_prompt = persisted_source.to_string();
    }
}

#[cfg(test)]
fn set_initial_prompt_in(
    settings: &mut Settings,
    prompt: &str,
    expected_source: Option<&str>,
) -> Result<(), CommandError> {
    ensure_expected_dictionary_source(&settings.initial_prompt, expected_source)?;
    settings.initial_prompt = prompt.to_string();
    Ok(())
}

#[cfg(test)]
mod dictionary_compare_and_set_tests {
    use super::*;

    #[test]
    fn dictionary_cas_matches_the_stores_trailing_newline_normalization() {
        assert!(ensure_expected_dictionary_source("OpenRouter", Some("OpenRouter\n\n")).is_ok());
        assert!(
            ensure_expected_dictionary_source("merge = merch", Some("merge = merch\r\n")).is_ok()
        );
        assert!(ensure_expected_dictionary_source("", Some("\r\n")).is_ok());
        assert!(ensure_expected_dictionary_source("OpenRouter ", Some("OpenRouter")).is_err());
        assert!(
            ensure_expected_dictionary_source("OpenRouter\nmerge", Some("OpenRouter merge"))
                .is_err()
        );
        let conflict =
            ensure_expected_dictionary_source("remote", Some("old")).expect_err("a stale source");
        assert!(matches!(conflict, CommandError::DictionaryChangedElsewhere));
        assert_eq!(conflict.code(), "dictionary_changed_elsewhere");
    }

    fn profile_settings() -> Settings {
        Settings {
            hotkey_profiles: vec![HotkeyProfile {
                id: "english".to_string(),
                name: "English".to_string(),
                shortcut: "Option+Space".to_string(),
                push_to_talk_shortcut: None,
                toggle_shortcut: None,
                language: Language::English,
            }],
            ..Settings::default()
        }
    }

    #[test]
    fn stale_global_source_is_rejected_without_mutation() {
        let mut settings = Settings {
            initial_prompt: "remote".to_string(),
            ..Settings::default()
        };
        let before = settings.clone();

        let error = set_initial_prompt_in(&mut settings, "local", Some("old")).unwrap_err();

        assert_eq!(error.code(), "dictionary_changed_elsewhere");
        assert!(error
            .to_string()
            .starts_with("Dictionary changed elsewhere:"));
        assert_eq!(settings.initial_prompt, before.initial_prompt);
        assert_eq!(settings.profile_glossaries, before.profile_glossaries);
    }

    #[test]
    fn stale_profile_source_is_rejected_without_mutation() {
        let mut settings = profile_settings();
        settings
            .profile_glossaries
            .insert("english".to_string(), "remote".to_string());
        let before = settings.clone();

        let error =
            set_profile_glossary_in(&mut settings, "english", "local".to_string(), Some("old"))
                .unwrap_err();

        assert_eq!(error.code(), "dictionary_changed_elsewhere");
        assert_eq!(settings.initial_prompt, before.initial_prompt);
        assert_eq!(settings.profile_glossaries, before.profile_glossaries);
    }

    #[test]
    fn matching_expected_sources_write_both_dictionary_scopes() {
        let mut settings = profile_settings();
        settings.initial_prompt = "global old".to_string();
        settings
            .profile_glossaries
            .insert("english".to_string(), "profile old".to_string());

        set_initial_prompt_in(&mut settings, "global new", Some("global old")).unwrap();
        set_profile_glossary_in(
            &mut settings,
            "english",
            "profile new".to_string(),
            Some("profile old"),
        )
        .unwrap();

        assert_eq!(settings.initial_prompt, "global new");
        assert_eq!(settings.profile_glossaries["english"], "profile new");
    }

    #[test]
    fn omitted_expected_source_preserves_legacy_writes() {
        let mut settings = profile_settings();
        settings.initial_prompt = "global old".to_string();
        settings
            .profile_glossaries
            .insert("english".to_string(), "profile old".to_string());

        set_initial_prompt_in(&mut settings, "global new", None).unwrap();
        set_profile_glossary_in(&mut settings, "english", "profile new".to_string(), None).unwrap();

        assert_eq!(settings.initial_prompt, "global new");
        assert_eq!(settings.profile_glossaries["english"], "profile new");
    }

    #[test]
    fn unrelated_scope_change_does_not_block_compare_and_set() {
        let mut settings = profile_settings();
        settings.initial_prompt = "global changed elsewhere".to_string();
        settings
            .profile_glossaries
            .insert("english".to_string(), "profile old".to_string());

        set_profile_glossary_in(
            &mut settings,
            "english",
            "profile new".to_string(),
            Some("profile old"),
        )
        .unwrap();

        assert_eq!(settings.initial_prompt, "global changed elsewhere");
        assert_eq!(settings.profile_glossaries["english"], "profile new");
    }

    #[test]
    fn global_conflict_refreshes_stale_controller_field_without_touching_profiles() {
        let mut controller_settings = profile_settings();
        controller_settings.initial_prompt = "editor baseline".to_string();
        controller_settings
            .profile_glossaries
            .insert("english".to_string(), "unchanged profile".to_string());

        reconcile_global_dictionary_after_conflict(
            &mut controller_settings,
            "editor baseline",
            "fresh persisted source",
        );

        assert_eq!(controller_settings.initial_prompt, "fresh persisted source");
        assert_eq!(
            controller_settings.profile_glossaries["english"],
            "unchanged profile"
        );
    }

    #[test]
    fn global_conflict_does_not_clobber_a_newer_controller_field() {
        let mut controller_settings = profile_settings();
        controller_settings.initial_prompt = "newer controller value".to_string();

        reconcile_global_dictionary_after_conflict(
            &mut controller_settings,
            "editor baseline",
            "fresh persisted source",
        );

        assert_eq!(controller_settings.initial_prompt, "newer controller value");
    }

    #[test]
    fn profile_conflict_preserves_persisted_presence_and_rejects_newer_controller_value() {
        let mut stale_controller = profile_settings();
        stale_controller
            .profile_glossaries
            .insert("english".to_string(), "editor baseline".to_string());
        let missing = DictionarySourceSnapshot {
            present: false,
            source: String::new(),
        };
        reconcile_profile_dictionary_after_conflict(
            &mut stale_controller,
            "english",
            "editor baseline",
            &missing,
        );
        assert!(!stale_controller.profile_glossaries.contains_key("english"));

        stale_controller
            .profile_glossaries
            .insert("english".to_string(), "editor baseline".to_string());
        let explicitly_empty = DictionarySourceSnapshot {
            present: true,
            source: String::new(),
        };
        reconcile_profile_dictionary_after_conflict(
            &mut stale_controller,
            "english",
            "editor baseline",
            &explicitly_empty,
        );
        assert_eq!(
            stale_controller.profile_glossaries.get("english"),
            Some(&String::new())
        );

        stale_controller
            .profile_glossaries
            .insert("english".to_string(), "newer controller value".to_string());
        reconcile_profile_dictionary_after_conflict(
            &mut stale_controller,
            "english",
            "editor baseline",
            &DictionarySourceSnapshot {
                present: true,
                source: "fresh persisted source".to_string(),
            },
        );
        assert_eq!(
            stale_controller.profile_glossaries["english"],
            "newer controller value"
        );
    }
}

#[cfg(test)]
mod training_glossary_tests {
    use super::*;

    #[test]
    fn reviewed_candidates_are_saved_only_to_the_selected_profile() {
        let mut settings = Settings::default();
        let heard = "Jag heter Magnus Jille.";
        let corrected = "Jag heter Magnus Gille.";
        let candidates = suggest_glossary_candidates(heard, corrected, &Glossary::default());

        apply_reviewed_training_candidates(&mut settings, heard, corrected, "default", &candidates)
            .unwrap();

        assert_eq!(settings.initial_prompt, "");
        assert_eq!(
            settings
                .profile_glossaries
                .get("default")
                .map(String::as_str),
            Some("Gille = Jille")
        );
    }

    #[test]
    fn fabricated_candidate_fails_without_mutating_settings() {
        let mut settings = Settings::default();
        let fabricated = GlossarySuggestion {
            observed: "anything".to_string(),
            canonical: "dangerous".to_string(),
            kind: GlossarySuggestionKind::Alias,
            context: "Magnus Jille".to_string(),
        };

        let error = apply_reviewed_training_candidates(
            &mut settings,
            "Magnus Jille",
            "Magnus Gille",
            "default",
            &[fabricated],
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("review"));
        assert!(settings.profile_glossaries.is_empty());
    }

    #[test]
    fn reviewed_alias_can_be_edited_or_converted_to_a_hint() {
        let heard = "Jag använder Love a ball idag.";
        let corrected = "Jag använder Lovable idag.";
        let original = suggest_glossary_candidates(heard, corrected, &Glossary::default())
            .into_iter()
            .next()
            .unwrap();

        let mut edited = original.clone();
        edited.canonical = "Lovable.dev".to_string();
        let mut settings = Settings::default();
        apply_reviewed_training_candidates(&mut settings, heard, corrected, "default", &[edited])
            .unwrap();
        assert_eq!(
            settings
                .profile_glossaries
                .get("default")
                .map(String::as_str),
            Some("Lovable.dev = Love a ball")
        );

        let mut as_hint = original;
        as_hint.kind = GlossarySuggestionKind::HintOnly;
        let mut settings = Settings::default();
        apply_reviewed_training_candidates(&mut settings, heard, corrected, "default", &[as_hint])
            .unwrap();
        assert_eq!(
            settings
                .profile_glossaries
                .get("default")
                .map(String::as_str),
            Some("Lovable")
        );
    }

    #[test]
    fn auto_language_profile_fails_closed_without_mutation() {
        let mut settings = Settings {
            language: Language::Auto,
            ..Default::default()
        };
        let heard = "Magnus Jille";
        let corrected = "Magnus Gille";
        let candidates = suggest_glossary_candidates(heard, corrected, &Glossary::default());

        let error = apply_reviewed_training_candidates(
            &mut settings,
            heard,
            corrected,
            "default",
            &candidates,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("explicit language"));
        assert!(settings.profile_glossaries.is_empty());
    }

    #[test]
    fn edited_canonical_cannot_shadow_an_existing_profile_alias() {
        let mut settings = Settings::default();
        settings
            .profile_glossaries
            .insert("default".into(), "Existing = dangerous".into());
        let original = settings.profile_glossaries.clone();
        let heard = "Magnus Jille";
        let corrected = "Magnus Gille";
        let mut candidate = suggest_glossary_candidates(
            heard,
            corrected,
            &Glossary::parse(&settings.effective_glossary_source(Some("default"))),
        )
        .into_iter()
        .next()
        .unwrap();
        candidate.canonical = "Dangerous".to_string();

        let error = apply_reviewed_training_candidates(
            &mut settings,
            heard,
            corrected,
            "default",
            &[candidate],
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("conflicts"));
        assert_eq!(settings.profile_glossaries, original);
    }

    #[test]
    fn inactive_global_alias_does_not_block_reviewed_profile_term() {
        let mut settings = Settings {
            initial_prompt: "Existing = dangerous".to_string(),
            ..Default::default()
        };
        let heard = "Magnus Jille";
        let corrected = "Magnus Gille";
        let mut candidate = suggest_glossary_candidates(
            heard,
            corrected,
            &Glossary::parse(&settings.effective_glossary_source(Some("default"))),
        )
        .into_iter()
        .next()
        .unwrap();
        candidate.canonical = "Dangerous".to_string();
        apply_reviewed_training_candidates(
            &mut settings,
            heard,
            corrected,
            "default",
            &[candidate],
        )
        .unwrap();
        assert_eq!(settings.initial_prompt, "Existing = dangerous");
        assert_eq!(settings.profile_glossaries["default"], "Dangerous = Jille");
    }
}

#[cfg(test)]
mod profile_dictionary_save_tests {
    use super::*;
    use crate::commands::test_fixtures::scoped_file_settings;

    #[test]
    fn profile_dictionary_save_validates_without_partial_mutation() {
        let mut settings = scoped_file_settings();
        let original = settings.profile_glossaries.clone();
        for profile in ["missing", "automatic"] {
            assert!(
                set_profile_glossary_in(&mut settings, profile, "new = old".into(), None).is_err()
            );
            assert_eq!(settings.profile_glossaries, original);
        }
        set_profile_glossary_in(&mut settings, "english", "OpenAI = open a i".into(), None)
            .unwrap();
        assert_eq!(settings.profile_glossaries["english"], "OpenAI = open a i");
        assert_eq!(settings.profile_glossaries["swedish"], original["swedish"]);
        assert_eq!(settings.initial_prompt, "Codex = code x\nmerge = merch");
    }
}
