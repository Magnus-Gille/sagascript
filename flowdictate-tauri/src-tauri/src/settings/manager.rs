use serde::{Deserialize, Serialize};

/// Supported transcription languages
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[serde(rename = "en")]
    English,
    #[serde(rename = "sv")]
    Swedish,
    #[serde(rename = "auto")]
    Auto,
}

impl Language {
    pub fn display_name(&self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Swedish => "Swedish",
            Language::Auto => "Auto-detect",
        }
    }

    /// Whisper language code (None for auto-detect)
    pub fn whisper_code(&self) -> Option<&'static str> {
        match self {
            Language::English => Some("en"),
            Language::Swedish => Some("sv"),
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
}

impl WhisperModel {
    pub fn display_name(&self) -> &'static str {
        match self {
            WhisperModel::TinyEn => "Tiny (English)",
            WhisperModel::Tiny => "Tiny (Multilingual)",
            WhisperModel::BaseEn => "Base (English)",
            WhisperModel::Base => "Base (Multilingual)",
            WhisperModel::KbWhisperTiny => "KB-Whisper Tiny (Swedish)",
            WhisperModel::KbWhisperBase => "KB-Whisper Base (Swedish)",
            WhisperModel::KbWhisperSmall => "KB-Whisper Small (Swedish)",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            WhisperModel::TinyEn => "Fastest, English only",
            WhisperModel::Tiny => "Fast, supports all languages",
            WhisperModel::BaseEn => "Balanced, English only",
            WhisperModel::Base => "Balanced, supports all languages",
            WhisperModel::KbWhisperTiny => "Fast, Swedish optimized (13% WER)",
            WhisperModel::KbWhisperBase => "Balanced, Swedish optimized (9% WER)",
            WhisperModel::KbWhisperSmall => "Accurate, Swedish optimized (7% WER)",
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
        }
    }

    /// Recommended model for a given language
    pub fn recommended(language: Language) -> WhisperModel {
        match language {
            Language::English => WhisperModel::BaseEn,
            Language::Swedish => WhisperModel::KbWhisperBase,
            Language::Auto => WhisperModel::Base,
        }
    }

    /// All standard (non-Swedish) models
    pub fn standard_models() -> &'static [WhisperModel] {
        &[
            WhisperModel::TinyEn,
            WhisperModel::Tiny,
            WhisperModel::BaseEn,
            WhisperModel::Base,
        ]
    }

    /// All Swedish-optimized models
    pub fn swedish_models() -> &'static [WhisperModel] {
        &[
            WhisperModel::KbWhisperTiny,
            WhisperModel::KbWhisperBase,
            WhisperModel::KbWhisperSmall,
        ]
    }
}

impl Default for WhisperModel {
    fn default() -> Self {
        WhisperModel::Base
    }
}

/// Transcription backend options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptionBackendType {
    Local,
    Remote,
}

impl TranscriptionBackendType {
    pub fn display_name(&self) -> &'static str {
        match self {
            TranscriptionBackendType::Local => "Local (whisper.cpp)",
            TranscriptionBackendType::Remote => "Remote (OpenAI)",
        }
    }
}

impl Default for TranscriptionBackendType {
    fn default() -> Self {
        TranscriptionBackendType::Local
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
    pub backend: TranscriptionBackendType,
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
            backend: TranscriptionBackendType::default(),
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
