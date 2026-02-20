use serde::{Deserialize, Serialize};

/// Supported transcription languages
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[serde(rename = "en")]
    English,
    #[serde(rename = "sv")]
    Swedish,
    #[serde(rename = "no")]
    Norwegian,
    #[serde(rename = "auto")]
    Auto,
}

impl Language {
    pub fn display_name(&self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Swedish => "Swedish",
            Language::Norwegian => "Norwegian",
            Language::Auto => "Auto-detect",
        }
    }

    /// Whisper language code (None for auto-detect)
    pub fn whisper_code(&self) -> Option<&'static str> {
        match self {
            Language::English => Some("en"),
            Language::Swedish => Some("sv"),
            Language::Norwegian => Some("no"),
            Language::Auto => None,
        }
    }
}

impl Default for Language {
    fn default() -> Self {
        Language::English
    }
}

/// Whisper model variants
/// All models use GGML format via whisper-rs (unified backend)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WhisperModel {
    #[serde(rename = "tiny.en")]
    TinyEn,
    #[serde(rename = "tiny")]
    Tiny,
    #[serde(rename = "base.en")]
    BaseEn,
    #[serde(rename = "base")]
    Base,
    #[serde(rename = "kb-whisper-tiny")]
    KbWhisperTiny,
    #[serde(rename = "kb-whisper-base")]
    KbWhisperBase,
    #[serde(rename = "kb-whisper-small")]
    KbWhisperSmall,
    #[serde(rename = "nb-whisper-tiny")]
    NbWhisperTiny,
    #[serde(rename = "nb-whisper-base")]
    NbWhisperBase,
    #[serde(rename = "nb-whisper-small")]
    NbWhisperSmall,
}

impl WhisperModel {
    pub fn display_name(&self) -> &'static str {
        match self {
            WhisperModel::TinyEn => "Whisper Tiny (EN)",
            WhisperModel::Tiny => "Whisper Tiny",
            WhisperModel::BaseEn => "Whisper Base (EN)",
            WhisperModel::Base => "Whisper Base",
            WhisperModel::KbWhisperTiny => "KB-Whisper Tiny",
            WhisperModel::KbWhisperBase => "KB-Whisper Base",
            WhisperModel::KbWhisperSmall => "KB-Whisper Small",
            WhisperModel::NbWhisperTiny => "NB-Whisper Tiny",
            WhisperModel::NbWhisperBase => "NB-Whisper Base",
            WhisperModel::NbWhisperSmall => "NB-Whisper Small",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            WhisperModel::TinyEn => "OpenAI Whisper, English-only. Fastest, less accurate",
            WhisperModel::Tiny => "OpenAI Whisper, multilingual. Fastest, less accurate",
            WhisperModel::BaseEn => "OpenAI Whisper, English-only. Balanced speed and accuracy",
            WhisperModel::Base => "OpenAI Whisper, multilingual. Balanced speed and accuracy",
            WhisperModel::KbWhisperTiny => "By KBLab. Swedish-optimized. Fastest, less accurate",
            WhisperModel::KbWhisperBase => "By KBLab. Swedish-optimized. Balanced speed and accuracy",
            WhisperModel::KbWhisperSmall => "By KBLab. Swedish-optimized. Most accurate, slower",
            WhisperModel::NbWhisperTiny => "By NbAiLab. Norwegian-optimized. Fastest, less accurate",
            WhisperModel::NbWhisperBase => "By NbAiLab. Norwegian-optimized. Balanced speed and accuracy",
            WhisperModel::NbWhisperSmall => "By NbAiLab. Norwegian-optimized. Most accurate, slower",
        }
    }

    pub fn is_english_only(&self) -> bool {
        matches!(self, WhisperModel::TinyEn | WhisperModel::BaseEn)
    }

    pub fn is_swedish_optimized(&self) -> bool {
        matches!(
            self,
            WhisperModel::KbWhisperTiny | WhisperModel::KbWhisperBase | WhisperModel::KbWhisperSmall
        )
    }

    pub fn is_norwegian_optimized(&self) -> bool {
        matches!(
            self,
            WhisperModel::NbWhisperTiny | WhisperModel::NbWhisperBase | WhisperModel::NbWhisperSmall
        )
    }

    /// GGML model filename
    pub fn ggml_filename(&self) -> &'static str {
        match self {
            WhisperModel::TinyEn => "ggml-tiny.en.bin",
            WhisperModel::Tiny => "ggml-tiny.bin",
            WhisperModel::BaseEn => "ggml-base.en.bin",
            WhisperModel::Base => "ggml-base.bin",
            WhisperModel::KbWhisperTiny => "kb-whisper-tiny-q5_0.bin",
            WhisperModel::KbWhisperBase => "kb-whisper-base-q5_0.bin",
            WhisperModel::KbWhisperSmall => "kb-whisper-small-q5_0.bin",
            WhisperModel::NbWhisperTiny => "nb-whisper-tiny-q5_0.bin",
            WhisperModel::NbWhisperBase => "nb-whisper-base-q5_0.bin",
            WhisperModel::NbWhisperSmall => "nb-whisper-small-q5_0.bin",
        }
    }

    /// HuggingFace download URL for model
    pub fn download_url(&self) -> &'static str {
        match self {
            WhisperModel::TinyEn => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin",
            WhisperModel::Tiny => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
            WhisperModel::BaseEn => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin",
            WhisperModel::Base => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
            WhisperModel::KbWhisperTiny => "https://huggingface.co/KBLab/kb-whisper-tiny/resolve/main/ggml-model-q5_0.bin",
            WhisperModel::KbWhisperBase => "https://huggingface.co/KBLab/kb-whisper-base/resolve/main/ggml-model-q5_0.bin",
            WhisperModel::KbWhisperSmall => "https://huggingface.co/KBLab/kb-whisper-small/resolve/main/ggml-model-q5_0.bin",
            WhisperModel::NbWhisperTiny => "https://huggingface.co/NbAiLab/nb-whisper-tiny/resolve/main/ggml-model-q5_0.bin",
            WhisperModel::NbWhisperBase => "https://huggingface.co/NbAiLab/nb-whisper-base/resolve/main/ggml-model-q5_0.bin",
            WhisperModel::NbWhisperSmall => "https://huggingface.co/NbAiLab/nb-whisper-small/resolve/main/ggml-model-q5_0.bin",
        }
    }

    /// Approximate download size in MB
    pub fn size_mb(&self) -> u32 {
        match self {
            WhisperModel::TinyEn => 75,
            WhisperModel::Tiny => 75,
            WhisperModel::BaseEn => 142,
            WhisperModel::Base => 142,
            WhisperModel::KbWhisperTiny => 40,
            WhisperModel::KbWhisperBase => 60,
            WhisperModel::KbWhisperSmall => 190,
            WhisperModel::NbWhisperTiny => 30,
            WhisperModel::NbWhisperBase => 55,
            WhisperModel::NbWhisperSmall => 175,
        }
    }

    /// Recommended model for a given language
    pub fn recommended(language: Language) -> WhisperModel {
        match language {
            Language::English => WhisperModel::BaseEn,
            Language::Swedish => WhisperModel::KbWhisperBase,
            Language::Norwegian => WhisperModel::NbWhisperBase,
            Language::Auto => WhisperModel::Base,
        }
    }

    /// Models available for a given language
    pub fn models_for_language(language: Language) -> &'static [WhisperModel] {
        match language {
            Language::English => &[WhisperModel::TinyEn, WhisperModel::BaseEn],
            Language::Swedish => &[
                WhisperModel::KbWhisperTiny,
                WhisperModel::KbWhisperBase,
                WhisperModel::KbWhisperSmall,
            ],
            Language::Norwegian => &[
                WhisperModel::NbWhisperTiny,
                WhisperModel::NbWhisperBase,
                WhisperModel::NbWhisperSmall,
            ],
            Language::Auto => &[WhisperModel::Tiny, WhisperModel::Base],
        }
    }
}

impl Default for WhisperModel {
    fn default() -> Self {
        WhisperModel::Base
    }
}

/// Hotkey activation mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HotkeyMode {
    #[serde(rename = "push")]
    PushToTalk,
    #[serde(rename = "toggle")]
    Toggle,
}

impl HotkeyMode {
    pub fn display_name(&self) -> &'static str {
        match self {
            HotkeyMode::PushToTalk => "Push-to-talk",
            HotkeyMode::Toggle => "Toggle",
        }
    }
}

impl Default for HotkeyMode {
    fn default() -> Self {
        HotkeyMode::PushToTalk
    }
}

/// All user-configurable settings, persisted as JSON
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub language: Language,
    pub whisper_model: WhisperModel,
    pub hotkey_mode: HotkeyMode,
    pub show_overlay: bool,
    pub auto_paste: bool,
    pub auto_select_model: bool,
    /// Hotkey shortcut string (e.g. "Control+Shift+Space")
    pub hotkey: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Language::default(),
            whisper_model: WhisperModel::default(),
            hotkey_mode: HotkeyMode::default(),
            show_overlay: true,
            auto_paste: true,
            auto_select_model: true,
            hotkey: "Control+Shift+Space".to_string(),
        }
    }
}

impl Settings {
    /// Returns the effective model considering auto-selection
    pub fn effective_model(&self) -> WhisperModel {
        if self.auto_select_model {
            WhisperModel::recommended(self.language)
        } else {
            self.whisper_model
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Language --

    #[test]
    fn language_default_is_english() {
        assert_eq!(Language::default(), Language::English);
    }

    #[test]
    fn language_display_names() {
        assert_eq!(Language::English.display_name(), "English");
        assert_eq!(Language::Swedish.display_name(), "Swedish");
        assert_eq!(Language::Norwegian.display_name(), "Norwegian");
        assert_eq!(Language::Auto.display_name(), "Auto-detect");
    }

    #[test]
    fn language_whisper_codes() {
        assert_eq!(Language::English.whisper_code(), Some("en"));
        assert_eq!(Language::Swedish.whisper_code(), Some("sv"));
        assert_eq!(Language::Norwegian.whisper_code(), Some("no"));
        assert_eq!(Language::Auto.whisper_code(), None);
    }

    #[test]
    fn language_serde_roundtrip() {
        let lang = Language::Swedish;
        let json = serde_json::to_string(&lang).unwrap();
        assert_eq!(json, "\"sv\"");
        let deserialized: Language = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, lang);
    }

    #[test]
    fn language_serde_all_variants() {
        let pairs = [
            (Language::English, "\"en\""),
            (Language::Swedish, "\"sv\""),
            (Language::Norwegian, "\"no\""),
            (Language::Auto, "\"auto\""),
        ];
        for (lang, expected) in pairs {
            let json = serde_json::to_string(&lang).unwrap();
            assert_eq!(json, expected, "serialize {:?}", lang);
            let back: Language = serde_json::from_str(&json).unwrap();
            assert_eq!(back, lang, "deserialize {:?}", lang);
        }
    }

    // -- WhisperModel --

    #[test]
    fn whisper_model_default_is_base() {
        assert_eq!(WhisperModel::default(), WhisperModel::Base);
    }

    #[test]
    fn english_only_models() {
        assert!(WhisperModel::TinyEn.is_english_only());
        assert!(WhisperModel::BaseEn.is_english_only());
        assert!(!WhisperModel::Tiny.is_english_only());
        assert!(!WhisperModel::Base.is_english_only());
        assert!(!WhisperModel::KbWhisperTiny.is_english_only());
        assert!(!WhisperModel::NbWhisperBase.is_english_only());
    }

    #[test]
    fn swedish_optimized_models() {
        assert!(WhisperModel::KbWhisperTiny.is_swedish_optimized());
        assert!(WhisperModel::KbWhisperBase.is_swedish_optimized());
        assert!(WhisperModel::KbWhisperSmall.is_swedish_optimized());
        assert!(!WhisperModel::TinyEn.is_swedish_optimized());
        assert!(!WhisperModel::NbWhisperTiny.is_swedish_optimized());
    }

    #[test]
    fn norwegian_optimized_models() {
        assert!(WhisperModel::NbWhisperTiny.is_norwegian_optimized());
        assert!(WhisperModel::NbWhisperBase.is_norwegian_optimized());
        assert!(WhisperModel::NbWhisperSmall.is_norwegian_optimized());
        assert!(!WhisperModel::TinyEn.is_norwegian_optimized());
        assert!(!WhisperModel::KbWhisperTiny.is_norwegian_optimized());
    }

    #[test]
    fn all_models_have_ggml_filenames() {
        let models = [
            WhisperModel::TinyEn,
            WhisperModel::Tiny,
            WhisperModel::BaseEn,
            WhisperModel::Base,
            WhisperModel::KbWhisperTiny,
            WhisperModel::KbWhisperBase,
            WhisperModel::KbWhisperSmall,
            WhisperModel::NbWhisperTiny,
            WhisperModel::NbWhisperBase,
            WhisperModel::NbWhisperSmall,
        ];
        for m in models {
            let filename = m.ggml_filename();
            assert!(filename.ends_with(".bin"), "{:?} filename: {}", m, filename);
            assert!(!filename.is_empty());
        }
    }

    #[test]
    fn all_models_have_download_urls() {
        let models = [
            WhisperModel::TinyEn,
            WhisperModel::Tiny,
            WhisperModel::BaseEn,
            WhisperModel::Base,
            WhisperModel::KbWhisperTiny,
            WhisperModel::KbWhisperBase,
            WhisperModel::KbWhisperSmall,
            WhisperModel::NbWhisperTiny,
            WhisperModel::NbWhisperBase,
            WhisperModel::NbWhisperSmall,
        ];
        for m in models {
            let url = m.download_url();
            assert!(url.starts_with("https://huggingface.co/"), "{:?}: {}", m, url);
            assert!(url.contains(".bin"), "{:?}: {}", m, url);
        }
    }

    #[test]
    fn all_models_have_nonzero_size() {
        let models = [
            WhisperModel::TinyEn,
            WhisperModel::Tiny,
            WhisperModel::BaseEn,
            WhisperModel::Base,
            WhisperModel::KbWhisperTiny,
            WhisperModel::KbWhisperBase,
            WhisperModel::KbWhisperSmall,
            WhisperModel::NbWhisperTiny,
            WhisperModel::NbWhisperBase,
            WhisperModel::NbWhisperSmall,
        ];
        for m in models {
            assert!(m.size_mb() > 0, "{:?} has 0 size", m);
        }
    }

    #[test]
    fn recommended_model_per_language() {
        assert_eq!(WhisperModel::recommended(Language::English), WhisperModel::BaseEn);
        assert_eq!(WhisperModel::recommended(Language::Swedish), WhisperModel::KbWhisperBase);
        assert_eq!(WhisperModel::recommended(Language::Norwegian), WhisperModel::NbWhisperBase);
        assert_eq!(WhisperModel::recommended(Language::Auto), WhisperModel::Base);
    }

    #[test]
    fn models_for_language_returns_correct_sets() {
        let en = WhisperModel::models_for_language(Language::English);
        assert_eq!(en.len(), 2);
        assert!(en.contains(&WhisperModel::TinyEn));
        assert!(en.contains(&WhisperModel::BaseEn));

        let sv = WhisperModel::models_for_language(Language::Swedish);
        assert_eq!(sv.len(), 3);
        assert!(sv.contains(&WhisperModel::KbWhisperTiny));
        assert!(sv.contains(&WhisperModel::KbWhisperBase));
        assert!(sv.contains(&WhisperModel::KbWhisperSmall));

        let no = WhisperModel::models_for_language(Language::Norwegian);
        assert_eq!(no.len(), 3);
        assert!(no.contains(&WhisperModel::NbWhisperTiny));

        let auto = WhisperModel::models_for_language(Language::Auto);
        assert_eq!(auto.len(), 2);
        assert!(auto.contains(&WhisperModel::Tiny));
        assert!(auto.contains(&WhisperModel::Base));
    }

    #[test]
    fn whisper_model_serde_roundtrip() {
        let model = WhisperModel::KbWhisperSmall;
        let json = serde_json::to_string(&model).unwrap();
        assert_eq!(json, "\"kb-whisper-small\"");
        let back: WhisperModel = serde_json::from_str(&json).unwrap();
        assert_eq!(back, model);
    }

    #[test]
    fn whisper_model_serde_all_variants() {
        let pairs = [
            (WhisperModel::TinyEn, "\"tiny.en\""),
            (WhisperModel::Tiny, "\"tiny\""),
            (WhisperModel::BaseEn, "\"base.en\""),
            (WhisperModel::Base, "\"base\""),
            (WhisperModel::KbWhisperTiny, "\"kb-whisper-tiny\""),
            (WhisperModel::KbWhisperBase, "\"kb-whisper-base\""),
            (WhisperModel::KbWhisperSmall, "\"kb-whisper-small\""),
            (WhisperModel::NbWhisperTiny, "\"nb-whisper-tiny\""),
            (WhisperModel::NbWhisperBase, "\"nb-whisper-base\""),
            (WhisperModel::NbWhisperSmall, "\"nb-whisper-small\""),
        ];
        for (model, expected) in pairs {
            let json = serde_json::to_string(&model).unwrap();
            assert_eq!(json, expected, "serialize {:?}", model);
            let back: WhisperModel = serde_json::from_str(&json).unwrap();
            assert_eq!(back, model, "deserialize {:?}", model);
        }
    }

    // -- HotkeyMode --

    #[test]
    fn hotkey_mode_default_is_push_to_talk() {
        assert_eq!(HotkeyMode::default(), HotkeyMode::PushToTalk);
    }

    #[test]
    fn hotkey_mode_display_names() {
        assert_eq!(HotkeyMode::PushToTalk.display_name(), "Push-to-talk");
        assert_eq!(HotkeyMode::Toggle.display_name(), "Toggle");
    }

    #[test]
    fn hotkey_mode_serde() {
        let json = serde_json::to_string(&HotkeyMode::PushToTalk).unwrap();
        assert_eq!(json, "\"push\"");
        let json = serde_json::to_string(&HotkeyMode::Toggle).unwrap();
        assert_eq!(json, "\"toggle\"");
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
    }

    #[test]
    fn settings_effective_model_with_auto_select() {
        let mut s = Settings::default();
        s.auto_select_model = true;

        s.language = Language::English;
        assert_eq!(s.effective_model(), WhisperModel::BaseEn);

        s.language = Language::Swedish;
        assert_eq!(s.effective_model(), WhisperModel::KbWhisperBase);

        s.language = Language::Norwegian;
        assert_eq!(s.effective_model(), WhisperModel::NbWhisperBase);

        s.language = Language::Auto;
        assert_eq!(s.effective_model(), WhisperModel::Base);
    }

    #[test]
    fn settings_effective_model_without_auto_select() {
        let mut s = Settings::default();
        s.auto_select_model = false;
        s.whisper_model = WhisperModel::KbWhisperSmall;
        s.language = Language::English; // shouldn't matter

        assert_eq!(s.effective_model(), WhisperModel::KbWhisperSmall);
    }

    #[test]
    fn settings_serde_roundtrip() {
        let original = Settings::default();
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.language, original.language);
        assert_eq!(deserialized.whisper_model, original.whisper_model);
        assert_eq!(deserialized.hotkey_mode, original.hotkey_mode);
        assert_eq!(deserialized.show_overlay, original.show_overlay);
        assert_eq!(deserialized.auto_paste, original.auto_paste);
        assert_eq!(deserialized.auto_select_model, original.auto_select_model);
        assert_eq!(deserialized.hotkey, original.hotkey);
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
}
