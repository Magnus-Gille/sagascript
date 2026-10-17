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
