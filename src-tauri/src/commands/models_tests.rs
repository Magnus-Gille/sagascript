use super::*;

#[cfg(test)]
mod lineup_option_tests {
    use super::*;
    use sagascript_core::models;
    use sagascript_core::test_support::model_env::FixedModelEnv;

    /// A host that does (`supported`) or does not run Pianissimo, with nothing
    /// downloaded.
    fn host(supported: bool) -> FixedModelEnv {
        if supported {
            FixedModelEnv::with_pianissimo()
        } else {
            FixedModelEnv::new()
        }
    }

    /// The picker's wire entries for a host.
    fn lineup(
        supported: bool,
        language: Language,
        include: Option<FileModel>,
        active: Option<FileModel>,
    ) -> Vec<ModelInfo> {
        wire_picker(
            &models::app_picker(language, include, &host(supported)),
            active,
        )
    }

    fn ids(supported: bool, language: Language, include: Option<FileModel>) -> Vec<String> {
        lineup(supported, language, include, None)
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
        let info = lineup(true, Language::Swedish, None, None).remove(0);
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
        for (supported, expected) in [(true, "pianissimo-sv"), (false, "kb-whisper-medium")] {
            let choice = FileModel::recommended_for(Language::Swedish, supported);
            let info = wire_info(&models::model_info(choice, &host(supported)), true);
            assert_eq!(info.id, expected);
        }
    }

    /// The picker shows what a download transfers, `list-models` what the
    /// model occupies: Pianissimo's two sizes differ, and the picker keeps the
    /// download (#373 part 2).
    #[test]
    fn the_picker_shows_the_download_size_of_pianissimo() {
        let picker = lineup(true, Language::Swedish, None, None);
        assert_eq!(picker[0].id, "pianissimo-sv");
        assert_eq!(picker[0].size_mb, 1_000);
        assert!(picker[0]
            .description
            .starts_with("Swedish file transcription"));
        assert_eq!(picker[1].id, "kb-whisper-large");
        assert_eq!(picker[1].size_mb, WhisperModel::KbWhisperLarge.size_mb());
    }

    #[test]
    fn the_selected_model_is_marked_active_and_installed_models_downloaded() {
        let active = Some(FileModel::PianissimoOriginal);
        let env = FixedModelEnv {
            pianissimo_downloaded: true,
            downloaded: vec![WhisperModel::KbWhisperLarge],
            ..FixedModelEnv::with_pianissimo()
        };
        let picker = wire_picker(&models::app_picker(Language::Swedish, None, &env), active);
        let marks: Vec<(&str, bool, bool)> = picker
            .iter()
            .map(|info| (info.id.as_str(), info.active, info.downloaded))
            .collect();
        assert_eq!(
            marks,
            [
                ("pianissimo-sv", true, true),
                ("kb-whisper-large", false, true)
            ]
        );
    }
}
