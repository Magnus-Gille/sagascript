use super::*;
use sagascript_core::settings::{HotkeyProfile, Language};

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
    fn correction_helper_applies_explicit_aliases() {
        let glossary = Glossary::parse("merge = merch");
        assert_eq!(
            apply_glossary("Merch it".to_string(), &glossary),
            "merge it"
        );
    }
}
