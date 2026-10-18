use super::*;
use crate::commands::apply_glossary;
use sagascript_core::settings::FileModelPreference;

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
            file_transcription_context(&settings, Some("nope"), None)
                .err()
                .map(|error| error.to_string()),
            Some("Unknown dictation profile 'nope'".to_string())
        );
        assert_eq!(
            file_transcription_language(&settings, Some("nope")).map_err(|error| error.to_string()),
            Err("Unknown dictation profile 'nope'".to_string())
        );
        assert_eq!(
            file_transcription_context(&settings, Some(""), None)
                .err()
                .map(|error| error.to_string()),
            Some("Unknown dictation profile ''".to_string())
        );

        // The profile is checked before the model, the model before any prompt.
        let english_only = Settings {
            file_transcription_model: FileModelPreference::Whisper(WhisperModel::BaseEn),
            ..migrated()
        };
        assert_eq!(
            file_transcription_context(&english_only, Some("nope"), None)
                .err()
                .map(|error| error.to_string()),
            Some("Unknown dictation profile 'nope'".to_string())
        );
        assert_eq!(
            file_transcription_context(&english_only, Some("swedish"), Some("Acme"))
                .err()
                .map(|error| error.to_string()),
            Some("Whisper model 'Whisper Base (EN)' is incompatible with Swedish".to_string())
        );

        let pianissimo = Settings {
            file_transcription_model: FileModelPreference::PianissimoOriginal,
            ..migrated()
        };
        assert_eq!(
            file_transcription_context(&pianissimo, Some("swedish"), None).err().map(|error| error.to_string()),
            Some("Pianissimo supports plain Swedish file transcription only; select a Whisper file model for meetings or diarization".to_string())
        );
    }
}

#[cfg(test)]
mod glossary_options_tests {
    use super::*;
    use crate::commands::test_fixtures::scoped_file_settings;

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
        assert!(error.to_string().contains("plain Swedish"));
        assert_eq!(error.code(), "pianissimo_unavailable");
        assert_eq!(
            settings.effective_model_for(Language::Swedish),
            WhisperModel::KbWhisperMedium
        );
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
