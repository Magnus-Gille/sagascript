use serde::{Deserialize, Serialize};

/// Supported transcription languages
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[serde(rename = "en")]
    #[default]
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

/// Whisper model variants
/// All models use GGML format via whisper-rs (unified backend)
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WhisperModel {
    #[serde(rename = "tiny.en")]
    TinyEn,
    #[serde(rename = "tiny")]
    Tiny,
    #[serde(rename = "base.en")]
    BaseEn,
    #[serde(rename = "base")]
    #[default]
    Base,
    #[serde(rename = "kb-whisper-tiny")]
    KbWhisperTiny,
    #[serde(rename = "kb-whisper-base")]
    KbWhisperBase,
    #[serde(rename = "kb-whisper-small")]
    KbWhisperSmall,
    #[serde(rename = "kb-whisper-medium")]
    KbWhisperMedium,
    #[serde(rename = "kb-whisper-large")]
    KbWhisperLarge,
    #[serde(rename = "nb-whisper-tiny")]
    NbWhisperTiny,
    #[serde(rename = "nb-whisper-base")]
    NbWhisperBase,
    #[serde(rename = "nb-whisper-small")]
    NbWhisperSmall,
    #[serde(rename = "nb-whisper-medium")]
    NbWhisperMedium,
    #[serde(rename = "nb-whisper-large")]
    NbWhisperLarge,
    #[serde(rename = "small.en")]
    SmallEn,
    #[serde(rename = "small")]
    Small,
    #[serde(rename = "medium.en")]
    MediumEn,
    #[serde(rename = "medium")]
    Medium,
    #[serde(rename = "large-v3-turbo")]
    LargeV3Turbo,
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
            WhisperModel::KbWhisperMedium => "KB-Whisper Medium",
            WhisperModel::KbWhisperLarge => "KB-Whisper Large",
            WhisperModel::NbWhisperTiny => "NB-Whisper Tiny",
            WhisperModel::NbWhisperBase => "NB-Whisper Base",
            WhisperModel::NbWhisperSmall => "NB-Whisper Small",
            WhisperModel::NbWhisperMedium => "NB-Whisper Medium",
            WhisperModel::NbWhisperLarge => "NB-Whisper Large",
            WhisperModel::SmallEn => "Whisper Small (EN)",
            WhisperModel::Small => "Whisper Small",
            WhisperModel::MediumEn => "Whisper Medium (EN)",
            WhisperModel::Medium => "Whisper Medium",
            WhisperModel::LargeV3Turbo => "Whisper Large v3 Turbo",
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
            WhisperModel::KbWhisperSmall => "By KBLab. Swedish-optimized. More accurate, slower",
            WhisperModel::KbWhisperMedium => "By KBLab. Swedish-optimized. High accuracy, slow",
            WhisperModel::KbWhisperLarge => "By KBLab. Swedish-optimized. Highest accuracy, slowest",
            WhisperModel::NbWhisperTiny => "By NbAiLab. Norwegian-optimized. Fastest, less accurate",
            WhisperModel::NbWhisperBase => "By NbAiLab. Norwegian-optimized. Balanced speed and accuracy",
            WhisperModel::NbWhisperSmall => "By NbAiLab. Norwegian-optimized. More accurate, slower",
            WhisperModel::NbWhisperMedium => "By NbAiLab. Norwegian-optimized. High accuracy, slow",
            WhisperModel::NbWhisperLarge => "By NbAiLab. Norwegian-optimized. Highest accuracy, slowest",
            WhisperModel::SmallEn => "OpenAI Whisper, English-only. More accurate, slower",
            WhisperModel::Small => "OpenAI Whisper, multilingual. More accurate, slower",
            WhisperModel::MediumEn => "OpenAI Whisper, English-only. High accuracy, slow",
            WhisperModel::Medium => "OpenAI Whisper, multilingual. High accuracy, slow",
            WhisperModel::LargeV3Turbo => "OpenAI Whisper, multilingual. Highest accuracy, slowest",
        }
    }

    #[allow(dead_code)]
    pub fn is_english_only(&self) -> bool {
        matches!(self, WhisperModel::TinyEn | WhisperModel::BaseEn | WhisperModel::SmallEn | WhisperModel::MediumEn)
    }

    #[allow(dead_code)]
    pub fn is_swedish_optimized(&self) -> bool {
        matches!(
            self,
            WhisperModel::KbWhisperTiny | WhisperModel::KbWhisperBase | WhisperModel::KbWhisperSmall
                | WhisperModel::KbWhisperMedium | WhisperModel::KbWhisperLarge
        )
    }

    /// Optimal no-speech threshold per model.
    ///
    /// Smaller English-only models (small.en) aggressively classify speech as
    /// silence at moderate thresholds, causing large content deletions. Tiny
    /// models are prone to repetition loops at the default 0.6. Larger and
    /// language-optimised models are robust to any reasonable threshold.
    pub fn no_speech_threshold(&self) -> f32 {
        match self {
            // small.en drops content even at 0.3 — needs fully disabled filter
            WhisperModel::SmallEn => 0.0,
            // medium/large English models: conservative but safe
            WhisperModel::MediumEn | WhisperModel::Medium
                | WhisperModel::LargeV3Turbo => 0.3,
            // Everything else (tiny, base, kb-whisper, nb-whisper): 0.3 works
            _ => 0.3,
        }
    }

    #[allow(dead_code)]
    pub fn is_norwegian_optimized(&self) -> bool {
        matches!(
            self,
            WhisperModel::NbWhisperTiny | WhisperModel::NbWhisperBase | WhisperModel::NbWhisperSmall
                | WhisperModel::NbWhisperMedium | WhisperModel::NbWhisperLarge
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
            WhisperModel::KbWhisperMedium => "kb-whisper-medium-q5_0.bin",
            WhisperModel::KbWhisperLarge => "kb-whisper-large-q5_0.bin",
            WhisperModel::NbWhisperTiny => "nb-whisper-tiny-q5_0.bin",
            WhisperModel::NbWhisperBase => "nb-whisper-base-q5_0.bin",
            WhisperModel::NbWhisperSmall => "nb-whisper-small-q5_0.bin",
            WhisperModel::NbWhisperMedium => "nb-whisper-medium-q5_0.bin",
            WhisperModel::NbWhisperLarge => "nb-whisper-large-q5_0.bin",
            WhisperModel::SmallEn => "ggml-small.en.bin",
            WhisperModel::Small => "ggml-small.bin",
            WhisperModel::MediumEn => "ggml-medium.en.bin",
            WhisperModel::Medium => "ggml-medium.bin",
            WhisperModel::LargeV3Turbo => "ggml-large-v3-turbo.bin",
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
            WhisperModel::KbWhisperMedium => "https://huggingface.co/KBLab/kb-whisper-medium/resolve/main/ggml-model-q5_0.bin",
            WhisperModel::KbWhisperLarge => "https://huggingface.co/KBLab/kb-whisper-large/resolve/main/ggml-model-q5_0.bin",
            WhisperModel::NbWhisperTiny => "https://huggingface.co/NbAiLab/nb-whisper-tiny/resolve/main/ggml-model-q5_0.bin",
            WhisperModel::NbWhisperBase => "https://huggingface.co/NbAiLab/nb-whisper-base/resolve/main/ggml-model-q5_0.bin",
            WhisperModel::NbWhisperSmall => "https://huggingface.co/NbAiLab/nb-whisper-small/resolve/main/ggml-model-q5_0.bin",
            WhisperModel::NbWhisperMedium => "https://huggingface.co/NbAiLab/nb-whisper-medium/resolve/main/ggml-model-q5_0.bin",
            WhisperModel::NbWhisperLarge => "https://huggingface.co/NbAiLab/nb-whisper-large/resolve/main/ggml-model-q5_0.bin",
            WhisperModel::SmallEn => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin",
            WhisperModel::Small => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
            WhisperModel::MediumEn => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.en.bin",
            WhisperModel::Medium => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
            WhisperModel::LargeV3Turbo => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
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
            WhisperModel::KbWhisperMedium => 514,
            WhisperModel::KbWhisperLarge => 1031,
            WhisperModel::NbWhisperTiny => 30,
            WhisperModel::NbWhisperBase => 55,
            WhisperModel::NbWhisperSmall => 175,
            WhisperModel::NbWhisperMedium => 514,
            WhisperModel::NbWhisperLarge => 1031,
            WhisperModel::SmallEn => 466,
            WhisperModel::Small => 466,
            WhisperModel::MediumEn => 1530,
            WhisperModel::Medium => 1530,
            WhisperModel::LargeV3Turbo => 1620,
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
            Language::English => &[
                WhisperModel::TinyEn,
                WhisperModel::BaseEn,
                WhisperModel::SmallEn,
                WhisperModel::MediumEn,
            ],
            Language::Swedish => &[
                WhisperModel::KbWhisperTiny,
                WhisperModel::KbWhisperBase,
                WhisperModel::KbWhisperSmall,
                WhisperModel::KbWhisperMedium,
                WhisperModel::KbWhisperLarge,
            ],
            Language::Norwegian => &[
                WhisperModel::NbWhisperTiny,
                WhisperModel::NbWhisperBase,
                WhisperModel::NbWhisperSmall,
                WhisperModel::NbWhisperMedium,
                WhisperModel::NbWhisperLarge,
            ],
            Language::Auto => &[
                WhisperModel::Tiny,
                WhisperModel::Base,
                WhisperModel::Small,
                WhisperModel::Medium,
                WhisperModel::LargeV3Turbo,
            ],
        }
    }
}

/// Hotkey activation mode
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HotkeyMode {
    #[serde(rename = "push")]
    #[default]
    PushToTalk,
    #[serde(rename = "toggle")]
    Toggle,
}

impl HotkeyMode {
    #[allow(dead_code)]
    pub fn display_name(&self) -> &'static str {
        match self {
            HotkeyMode::PushToTalk => "Push-to-talk",
            HotkeyMode::Toggle => "Toggle",
        }
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
        assert!(WhisperModel::SmallEn.is_english_only());
        assert!(WhisperModel::MediumEn.is_english_only());
        assert!(!WhisperModel::Tiny.is_english_only());
        assert!(!WhisperModel::Base.is_english_only());
        assert!(!WhisperModel::Small.is_english_only());
        assert!(!WhisperModel::Medium.is_english_only());
        assert!(!WhisperModel::LargeV3Turbo.is_english_only());
        assert!(!WhisperModel::KbWhisperTiny.is_english_only());
        assert!(!WhisperModel::NbWhisperBase.is_english_only());
    }

    #[test]
    fn swedish_optimized_models() {
        assert!(WhisperModel::KbWhisperTiny.is_swedish_optimized());
        assert!(WhisperModel::KbWhisperBase.is_swedish_optimized());
        assert!(WhisperModel::KbWhisperSmall.is_swedish_optimized());
        assert!(WhisperModel::KbWhisperMedium.is_swedish_optimized());
        assert!(WhisperModel::KbWhisperLarge.is_swedish_optimized());
        assert!(!WhisperModel::TinyEn.is_swedish_optimized());
        assert!(!WhisperModel::NbWhisperTiny.is_swedish_optimized());
    }

    #[test]
    fn norwegian_optimized_models() {
        assert!(WhisperModel::NbWhisperTiny.is_norwegian_optimized());
        assert!(WhisperModel::NbWhisperBase.is_norwegian_optimized());
        assert!(WhisperModel::NbWhisperSmall.is_norwegian_optimized());
        assert!(WhisperModel::NbWhisperMedium.is_norwegian_optimized());
        assert!(WhisperModel::NbWhisperLarge.is_norwegian_optimized());
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
            WhisperModel::KbWhisperMedium,
            WhisperModel::KbWhisperLarge,
            WhisperModel::NbWhisperTiny,
            WhisperModel::NbWhisperBase,
            WhisperModel::NbWhisperSmall,
            WhisperModel::NbWhisperMedium,
            WhisperModel::NbWhisperLarge,
            WhisperModel::SmallEn,
            WhisperModel::Small,
            WhisperModel::MediumEn,
            WhisperModel::Medium,
            WhisperModel::LargeV3Turbo,
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
            WhisperModel::KbWhisperMedium,
            WhisperModel::KbWhisperLarge,
            WhisperModel::NbWhisperTiny,
            WhisperModel::NbWhisperBase,
            WhisperModel::NbWhisperSmall,
            WhisperModel::NbWhisperMedium,
            WhisperModel::NbWhisperLarge,
            WhisperModel::SmallEn,
            WhisperModel::Small,
            WhisperModel::MediumEn,
            WhisperModel::Medium,
            WhisperModel::LargeV3Turbo,
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
            WhisperModel::KbWhisperMedium,
            WhisperModel::KbWhisperLarge,
            WhisperModel::NbWhisperTiny,
            WhisperModel::NbWhisperBase,
            WhisperModel::NbWhisperSmall,
            WhisperModel::NbWhisperMedium,
            WhisperModel::NbWhisperLarge,
            WhisperModel::SmallEn,
            WhisperModel::Small,
            WhisperModel::MediumEn,
            WhisperModel::Medium,
            WhisperModel::LargeV3Turbo,
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
        assert_eq!(en.len(), 4);
        assert!(en.contains(&WhisperModel::TinyEn));
        assert!(en.contains(&WhisperModel::BaseEn));
        assert!(en.contains(&WhisperModel::SmallEn));
        assert!(en.contains(&WhisperModel::MediumEn));

        let sv = WhisperModel::models_for_language(Language::Swedish);
        assert_eq!(sv.len(), 5);
        assert!(sv.contains(&WhisperModel::KbWhisperTiny));
        assert!(sv.contains(&WhisperModel::KbWhisperBase));
        assert!(sv.contains(&WhisperModel::KbWhisperSmall));
        assert!(sv.contains(&WhisperModel::KbWhisperMedium));
        assert!(sv.contains(&WhisperModel::KbWhisperLarge));

        let no = WhisperModel::models_for_language(Language::Norwegian);
        assert_eq!(no.len(), 5);
        assert!(no.contains(&WhisperModel::NbWhisperTiny));
        assert!(no.contains(&WhisperModel::NbWhisperBase));
        assert!(no.contains(&WhisperModel::NbWhisperSmall));
        assert!(no.contains(&WhisperModel::NbWhisperMedium));
        assert!(no.contains(&WhisperModel::NbWhisperLarge));

        let auto = WhisperModel::models_for_language(Language::Auto);
        assert_eq!(auto.len(), 5);
        assert!(auto.contains(&WhisperModel::Tiny));
        assert!(auto.contains(&WhisperModel::Base));
        assert!(auto.contains(&WhisperModel::Small));
        assert!(auto.contains(&WhisperModel::Medium));
        assert!(auto.contains(&WhisperModel::LargeV3Turbo));
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
            (WhisperModel::KbWhisperMedium, "\"kb-whisper-medium\""),
            (WhisperModel::KbWhisperLarge, "\"kb-whisper-large\""),
            (WhisperModel::NbWhisperTiny, "\"nb-whisper-tiny\""),
            (WhisperModel::NbWhisperBase, "\"nb-whisper-base\""),
            (WhisperModel::NbWhisperSmall, "\"nb-whisper-small\""),
            (WhisperModel::NbWhisperMedium, "\"nb-whisper-medium\""),
            (WhisperModel::NbWhisperLarge, "\"nb-whisper-large\""),
            (WhisperModel::SmallEn, "\"small.en\""),
            (WhisperModel::Small, "\"small\""),
            (WhisperModel::MediumEn, "\"medium.en\""),
            (WhisperModel::Medium, "\"medium\""),
            (WhisperModel::LargeV3Turbo, "\"large-v3-turbo\""),
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

    // -- Model consistency --

    #[test]
    fn recommended_model_is_in_models_for_language() {
        let languages = [Language::English, Language::Swedish, Language::Norwegian, Language::Auto];
        for lang in languages {
            let recommended = WhisperModel::recommended(lang);
            let models = WhisperModel::models_for_language(lang);
            assert!(
                models.contains(&recommended),
                "recommended model {:?} for {:?} is not in models_for_language: {:?}",
                recommended,
                lang,
                models
            );
        }
    }
}
