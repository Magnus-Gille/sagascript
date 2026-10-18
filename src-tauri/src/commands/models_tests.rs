use super::*;

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

/// A model query for a profile that does not exist rejects with the code of
/// its settings error, and Pianissimo is refused with its own code where the
/// host cannot run it.
#[cfg(test)]
mod rejection_tests {
    use super::*;
    use crate::app_controller::AppController;
    use std::sync::Mutex;
    use tauri::Manager;

    #[test]
    fn a_profile_model_query_for_an_unknown_profile_is_unknown_profile() {
        let app = tauri::test::mock_app();
        app.manage::<SharedController>(Mutex::new(AppController::new(Settings::default())));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime");
        let error = runtime
            .block_on(get_profile_model_info(
                app.state::<SharedController>(),
                "nosuch".into(),
            ))
            .err()
            .expect("there is no such profile");
        assert_eq!(error.code(), "unknown_profile");
        assert_eq!(error.to_string(), "Unknown profile 'nosuch'");
    }

    #[test]
    fn downloading_pianissimo_is_refused_with_its_code_where_it_cannot_run() {
        assert!(ensure_pianissimo_runs(true).is_ok());
        let error = ensure_pianissimo_runs(false).expect_err("this host cannot run Pianissimo");
        assert_eq!(error.code(), "pianissimo_unavailable");
        assert_eq!(
            error.to_string(),
            sagascript_core::transcription::pianissimo_backend::UNSUPPORTED_MESSAGE
        );
    }
}
