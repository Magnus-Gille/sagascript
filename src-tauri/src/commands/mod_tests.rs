use super::*;

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
mod lineup_option_tests {
    use super::*;

    fn ids(supported: bool, language: Language, include: Option<FileModel>) -> Vec<String> {
        lineup_model_infos(language, supported, include, None)
            .into_iter()
            .map(|info| info.id)
            .collect()
    }

    #[test]
    fn swedish_pickers_show_only_the_supported_lineup() {
        assert_eq!(
            ids(true, Language::Swedish, None),
            ["pianissimo-sv", "kb-whisper-large"]
        );
        assert_eq!(
            ids(false, Language::Swedish, None),
            ["kb-whisper-medium", "kb-whisper-large"]
        );
        assert_eq!(
            ids(
                true,
                Language::Swedish,
                Some(FileModel::Whisper(WhisperModel::KbWhisperBase))
            ),
            ["pianissimo-sv", "kb-whisper-large", "kb-whisper-base"]
        );
        assert_eq!(
            ids(true, Language::English, None),
            ["tiny.en", "base.en", "small.en", "medium.en"]
        );
    }

    #[test]
    fn model_info_wire_keys_are_stable() {
        let info = lineup_model_infos(Language::Swedish, true, None, None).remove(0);
        let value = serde_json::to_value(info).unwrap();
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "active",
                "description",
                "display_name",
                "downloaded",
                "id",
                "size_mb"
            ]
        );
    }

    #[test]
    fn recommended_info_follows_the_platform_gate() {
        assert_eq!(
            model_info_for(FileModel::recommended_for(Language::Swedish, true), true).id,
            "pianissimo-sv"
        );
        assert_eq!(
            model_info_for(FileModel::recommended_for(Language::Swedish, false), true).id,
            "kb-whisper-medium"
        );
    }
}

#[cfg(test)]
mod glossary_options_tests {
    use super::*;

    #[test]
    fn live_options_prime_with_canonical_terms_only() {
        let settings = Settings {
            initial_prompt: "OpenRouter = open router | open vrouter\nmerge = merch".to_string(),
            ..Settings::default()
        };
        assert_eq!(
            build_transcribe_options(&settings).prompt.as_deref(),
            Some("OpenRouter, merge")
        );
    }

    #[test]
    fn live_options_include_only_the_selected_profile_glossary() {
        let mut settings = Settings {
            initial_prompt: "Codex = code x".to_string(),
            hotkey_profiles: vec![
                HotkeyProfile {
                    id: "svenska".to_string(),
                    name: "Svenska".to_string(),
                    shortcut: "Control+Shift+Space".to_string(),
                    push_to_talk_shortcut: None,
                    toggle_shortcut: None,
                    language: Language::Swedish,
                },
                HotkeyProfile {
                    id: "english".to_string(),
                    name: "English".to_string(),
                    shortcut: "Control+Option+Space".to_string(),
                    push_to_talk_shortcut: None,
                    toggle_shortcut: None,
                    language: Language::English,
                },
            ],
            ..Settings::default()
        };
        settings
            .profile_glossaries
            .insert("svenska".to_string(), "mergea = mördsa".to_string());
        settings
            .profile_glossaries
            .insert("english".to_string(), "Lovable = love a ball".to_string());

        assert_eq!(
            build_transcribe_options_for_profile(&settings, Some("svenska"))
                .prompt
                .as_deref(),
            Some("Codex, mergea")
        );
    }

    #[test]
    fn file_context_overrides_the_saved_dictionary_for_one_run() {
        let settings = Settings {
            initial_prompt: "OpenRouter = open router".to_string(),
            ..Settings::default()
        };
        let prompt = Some("Cloudflare = cloud flare".to_string());
        assert_eq!(
            build_file_transcribe_options(&settings, prompt)
                .prompt
                .as_deref(),
            Some("Cloudflare")
        );
    }

    #[test]
    fn correction_helper_applies_explicit_aliases() {
        let glossary = Glossary::parse("merge = merch");
        assert_eq!(
            apply_glossary("Merch it".to_string(), &glossary),
            "merge it"
        );
    }

    #[test]
    fn gui_file_global_alias_does_not_rewrite_english() {
        let settings = Settings {
            initial_prompt: "merge = merch".to_string(),
            ..Settings::default()
        };
        let glossary = effective_file_glossary(&settings, None);
        assert_eq!(
            apply_glossary("company merch".into(), &glossary),
            "company merch"
        );
        assert_eq!(glossary.decoder_prompt().as_deref(), Some("merge"));
    }

    fn scoped_file_settings() -> Settings {
        let mut settings = Settings {
            language: Language::English,
            initial_prompt: "Codex = code x\nmerge = merch".into(),
            hotkey_profiles: vec![
                HotkeyProfile {
                    id: "swedish".into(),
                    name: "Swedish".into(),
                    shortcut: "Super+S".into(),
                    push_to_talk_shortcut: None,
                    toggle_shortcut: None,
                    language: Language::Swedish,
                },
                HotkeyProfile {
                    id: "english".into(),
                    name: "English".into(),
                    shortcut: "Super+E".into(),
                    push_to_talk_shortcut: None,
                    toggle_shortcut: None,
                    language: Language::English,
                },
                HotkeyProfile {
                    id: "automatic".into(),
                    name: "Automatic".into(),
                    shortcut: "Super+A".into(),
                    push_to_talk_shortcut: None,
                    toggle_shortcut: None,
                    language: Language::Auto,
                },
            ],
            ..Settings::default()
        };
        settings.profile_glossaries.insert(
            "swedish".into(),
            "merge = merch\nOpenRouter = open router".into(),
        );
        settings
            .profile_glossaries
            .insert("automatic".into(), "merge = merch".into());
        settings
    }

    #[test]
    fn gui_file_profile_pins_language_model_and_dictionary_together() {
        let settings = scoped_file_settings();
        let context = file_transcription_context(&settings, Some("swedish"), None).unwrap();
        assert_eq!(context.language, Language::Swedish);
        assert_eq!(
            context.model,
            settings.effective_model_for(Language::Swedish)
        );
        assert_eq!(
            apply_glossary("merch open router".into(), &context.glossary),
            "merge OpenRouter"
        );
        assert_eq!(context.options.prompt, context.glossary.decoder_prompt());
        let default = file_transcription_context(&settings, None, None).unwrap();
        assert_eq!(default.language, Language::Swedish);
        assert_eq!(
            apply_glossary("company merch".into(), &default.glossary),
            "company merge"
        );
        let english = file_transcription_context(&settings, Some("english"), None).unwrap();
        assert_eq!(english.language, Language::English);
        assert_eq!(
            apply_glossary("company merch".into(), &english.glossary),
            "company merch"
        );
    }

    #[test]
    fn gui_file_override_is_hint_only_but_keeps_explicit_profile_aliases() {
        let context = file_transcription_context(
            &scoped_file_settings(),
            Some("swedish"),
            Some("Cloudflare = cloud flare"),
        )
        .unwrap();
        assert_eq!(
            apply_glossary("cloud flare merch".into(), &context.glossary),
            "cloud flare merge"
        );
        assert_eq!(
            context.options.prompt.as_deref(),
            Some("Cloudflare, merge, OpenRouter")
        );
    }

    #[test]
    fn gui_file_rejects_unknown_but_accepts_auto_profile_before_io() {
        let settings = scoped_file_settings();
        assert!(file_transcription_context(&settings, Some("missing"), None).is_err());
        assert_eq!(
            file_transcription_context(&settings, Some("automatic"), None)
                .unwrap()
                .language,
            Language::Auto
        );
        let context = file_transcription_context(&settings, Some("automatic"), None).unwrap();
        assert_eq!(context.language, Language::Auto);
        assert_eq!(apply_glossary("merch".into(), &context.glossary), "merch");
    }

    #[test]
    fn meeting_context_rejects_pianissimo_instead_of_switching_to_whisper() {
        let mut settings = scoped_file_settings();
        settings.file_transcription_model = FileModelPreference::PianissimoOriginal;
        let error = file_transcription_context(&settings, Some("swedish"), None)
            .err()
            .expect("meeting path must reject Pianissimo");
        assert!(error.contains("plain Swedish"));
        assert_eq!(
            settings.effective_model_for(Language::Swedish),
            WhisperModel::KbWhisperMedium
        );
    }

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

    #[test]
    fn scoped_file_glossary_preserves_cross_fragment_phrase_matching() {
        let context =
            file_transcription_context(&scoped_file_settings(), Some("swedish"), None).unwrap();
        let (fragments, corrections) = context
            .glossary
            .correct_fragments(&["open", " router merch"]);
        assert_eq!(fragments.concat(), "OpenRouter merge");
        assert_eq!(corrections.len(), 2);
    }
}

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
mod controller_poison_tests {
    use super::*;

    /// Poison `mutex` the way production would: another thread panics while
    /// holding the guard (#359).
    fn poison<T: Send>(mutex: &Mutex<T>) {
        std::thread::scope(|scope| {
            let panicked = scope
                .spawn(|| {
                    let _guard = mutex.lock().unwrap();
                    panic!("injected panic while holding the lock (#359)");
                })
                .join();
            assert!(panicked.is_err());
        });
        assert!(mutex.is_poisoned());
    }

    fn controller() -> SharedController {
        Mutex::new(AppController::new(Settings::default()))
    }

    /// A panic under the controller lock must not turn every later command
    /// into a panic: the next real command path gets the controller.
    #[test]
    fn commands_keep_working_after_a_panic_poisoned_the_controller() {
        let controller = controller();
        poison(&controller);

        let lease = acquire_hotkey_configuration(&controller)
            .expect("a poisoned controller must be recovered, not propagated");
        let refused = acquire_hotkey_configuration(&controller)
            .err()
            .expect("the recovered controller still enforces the single-change lease");
        assert_eq!(refused.code(), "busy");
        assert!(refused.retryable());
        drop(lease);
        assert!(acquire_hotkey_configuration(&controller).is_ok());
        assert!(!controller.is_poisoned(), "recovery clears the poison flag");
    }

    /// The lease's Drop runs during unwinding when a settings change panics.
    /// Re-panicking on the poisoned controller there would abort the process.
    #[test]
    fn hotkey_configuration_lease_drop_tolerates_a_poisoned_controller() {
        let controller = controller();
        let lease = acquire_hotkey_configuration(&controller).unwrap();
        poison(&controller);

        let dropped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(lease)));
        assert!(
            dropped.is_ok(),
            "Drop must not panic on a poisoned controller"
        );
        assert!(
            acquire_hotkey_configuration(&controller).is_ok(),
            "the lease was released despite the poison"
        );
    }
}

#[cfg(test)]
mod gui_recording_tests {
    use super::gui_start_recording_result;
    use sagascript_core::error::DictationError;

    #[test]
    fn gui_start_while_transcribing_returns_busy_error() {
        let error = gui_start_recording_result(Ok(false)).unwrap_err();

        assert_eq!(error.code(), "busy");
        assert!(error.retryable(), "waiting and trying again works");
        let message = error.to_string();
        assert!(message.contains("busy"));
        assert!(message.contains("current recording"));
    }

    #[test]
    fn a_failed_start_keeps_the_dictation_error_code() {
        let error = gui_start_recording_result(Err(DictationError::MicrophonePermissionDenied))
            .unwrap_err();
        assert_eq!(error.code(), "microphone_permission_denied");
        assert!(gui_start_recording_result(Ok(true)).is_ok());
    }
}

#[cfg(test)]
mod training_stop_tests {
    use super::not_recording_result;

    #[test]
    fn duplicate_normal_stop_remains_a_noop() {
        let result = not_recording_result(true).unwrap();
        assert!(result.raw_text.is_empty());
        assert!(result.effective_text.is_empty());
    }

    #[test]
    fn training_stop_without_active_recording_is_an_error() {
        let error = not_recording_result(false).unwrap_err();
        assert_eq!(error.code(), "not_recording");
        assert!(error.to_string().contains("No training recording"));
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
        .unwrap_err();

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
        .unwrap_err();

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
        .unwrap_err();

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
mod recovery_payload_tests {
    use super::update_recovery_payload;
    use crate::update_recovery::RecoverySnapshot;
    use serde_json::{json, Value};

    #[test]
    fn missing_snapshot_is_the_only_absent_draft() {
        assert_eq!(update_recovery_payload(None).unwrap(), None);
        for payload in [Value::Null, json!(false), json!(1), json!(""), json!([])] {
            let snapshot = RecoverySnapshot {
                schema_version: 1,
                timestamp: 1,
                payload,
            };
            assert!(update_recovery_payload(Some(snapshot)).is_err());
        }
    }

    #[test]
    fn unsupported_envelope_version_is_not_an_absent_draft() {
        for schema_version in [0, 2] {
            let snapshot = RecoverySnapshot {
                schema_version,
                timestamp: 1,
                payload: json!({"schema_version": 1}),
            };
            assert!(update_recovery_payload(Some(snapshot)).is_err());
        }
    }

    #[test]
    fn object_payload_is_preserved_for_strict_frontend_validation() {
        let payload = json!({"schema_version": 1, "dictation": null, "future_field": [1, 2]});
        let snapshot = RecoverySnapshot {
            schema_version: 1,
            timestamp: 1,
            payload: payload.clone(),
        };
        assert_eq!(
            update_recovery_payload(Some(snapshot)).unwrap(),
            Some(payload)
        );
    }
}

#[cfg(test)]
mod save_name_tests {
    use super::sanitize_save_file_name;

    #[test]
    fn save_default_strips_directories_and_keeps_txt() {
        assert_eq!(sanitize_save_file_name("/a/b/talk.m4a"), "talk.m4a");
        assert_eq!(sanitize_save_file_name("C:\\audio\\talk"), "talk.txt");
        assert_eq!(sanitize_save_file_name(""), "transcription.txt");
        assert_eq!(sanitize_save_file_name("   "), "transcription.txt");
        assert_eq!(sanitize_save_file_name("notes"), "notes.txt");
    }
}

/// The pure AVFoundation status mappings of `macos_mic`, whose other code
/// needs a real macOS bundle.
#[cfg(target_os = "macos")]
mod macos_mic_tests {
    use crate::commands::macos_mic::{
        access_request_decision, authorization_status_label, AccessRequestDecision,
    };

    #[test]
    fn maps_avfoundation_authorization_constants_correctly() {
        assert_eq!(authorization_status_label(0, false), "not_determined");
        assert_eq!(authorization_status_label(1, false), "restricted");
        assert_eq!(authorization_status_label(2, false), "denied");
        assert_eq!(authorization_status_label(3, false), "authorized");
    }

    #[test]
    fn denied_cache_can_be_refreshed_without_remapping_authorized() {
        assert_eq!(authorization_status_label(2, true), "authorized");
        assert_eq!(authorization_status_label(3, false), "authorized");
    }

    #[test]
    fn request_decision_live_queries_undetermined_and_cached_denied() {
        assert_eq!(
            access_request_decision(0),
            AccessRequestDecision::QuerySystem
        );
        assert_eq!(
            access_request_decision(2),
            AccessRequestDecision::QuerySystem
        );
    }

    #[test]
    fn request_decision_completes_known_restricted_and_authorized_states() {
        assert_eq!(
            access_request_decision(1),
            AccessRequestDecision::Complete(false)
        );
        assert_eq!(
            access_request_decision(3),
            AccessRequestDecision::Complete(true)
        );
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

/// The app's file context resolves profile, language and dictionary through
/// the run resolver of core, the same one the CLI's request type is checked
/// against on the same table (`run_context_table`, #346). So the app and the
/// CLI are equal through the resolver, and the app does not depend on the CLI
/// for it. The tables' settings carry an explicit file model, so resolving it
/// never looks at the models downloaded on the machine running the test
/// (ADR 0004).
#[cfg(test)]
mod run_context_parity_tests {
    use super::{file_transcription_context, file_transcription_language};
    use sagascript_core::run_context::{RunOverrides, RunSelection};
    use sagascript_core::settings::{FileModelPreference, Settings, WhisperModel};
    use sagascript_core::test_support::run_context_table::{entries, migrated, shapes, PROMPTS};

    #[test]
    fn the_app_resolves_what_the_core_resolver_resolves_for_the_same_profile_and_prompt() {
        // The app has no language override (that is #373), so the reference is
        // the core resolver with a profile, a prompt and `language: None`.
        for shape in shapes() {
            for profile in &shape.profiles {
                for prompt in PROMPTS {
                    let label = format!("{}, profile {profile:?}, prompt {prompt:?}", shape.name);
                    let core = RunSelection::resolve(
                        &shape.settings,
                        RunOverrides {
                            profile: *profile,
                            language: None,
                        },
                    )
                    .unwrap_or_else(|error| panic!("{label}: core: {error}"))
                    .into_context(&shape.settings, prompt);
                    let app = file_transcription_context(&shape.settings, *profile, prompt)
                        .unwrap_or_else(|error| panic!("{label}: app: {error}"));
                    assert_eq!(app.language, core.language, "{label}: language");
                    assert_eq!(
                        entries(&app.glossary),
                        entries(&core.glossary),
                        "{label}: dictionary"
                    );
                    assert_eq!(
                        app.glossary.decoder_prompt(),
                        core.glossary.decoder_prompt(),
                        "{label}: decoder prompt"
                    );
                    assert_eq!(
                        app.options.prompt,
                        core.glossary.decoder_prompt(),
                        "{label}: options prompt"
                    );
                    assert_eq!(
                        file_transcription_language(&shape.settings, *profile).unwrap(),
                        core.language,
                        "{label}: language helper"
                    );
                }
            }
        }
    }

    #[test]
    fn the_apps_errors_keep_their_own_text() {
        // The app reports plain strings, without the CLI's "Settings error: ".
        let settings = migrated();
        assert_eq!(
            file_transcription_context(&settings, Some("nope"), None).err(),
            Some("Unknown dictation profile 'nope'".to_string())
        );
        assert_eq!(
            file_transcription_language(&settings, Some("nope")),
            Err("Unknown dictation profile 'nope'".to_string())
        );
        assert_eq!(
            file_transcription_context(&settings, Some(""), None).err(),
            Some("Unknown dictation profile ''".to_string())
        );

        // The profile is checked before the model, the model before any prompt.
        let english_only = Settings {
            file_transcription_model: FileModelPreference::Whisper(WhisperModel::BaseEn),
            ..migrated()
        };
        assert_eq!(
            file_transcription_context(&english_only, Some("nope"), None).err(),
            Some("Unknown dictation profile 'nope'".to_string())
        );
        assert_eq!(
            file_transcription_context(&english_only, Some("swedish"), Some("Acme")).err(),
            Some("Whisper model 'Whisper Base (EN)' is incompatible with Swedish".to_string())
        );

        let pianissimo = Settings {
            file_transcription_model: FileModelPreference::PianissimoOriginal,
            ..migrated()
        };
        assert_eq!(
            file_transcription_context(&pianissimo, Some("swedish"), None).err(),
            Some("Pianissimo supports plain Swedish file transcription only; select a Whisper file model for meetings or diarization".to_string())
        );
    }
}
