use serde::{Deserialize, Serialize};

use super::Language;

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
    #[serde(rename = "fi-whisper-tiny")]
    FinnishWhisperTiny,
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
    #[serde(rename = "large-v3-turbo-q8_0")]
    LargeV3TurboQ8,
}

impl WhisperModel {
    pub fn display_name(&self) -> &'static str {
        match self {
            WhisperModel::TinyEn => "Whisper Tiny (EN)",
            WhisperModel::Tiny => "Whisper Tiny",
            WhisperModel::BaseEn => "Whisper Base (EN)",
            WhisperModel::Base => "Whisper Base",
            WhisperModel::FinnishWhisperTiny => "Finnish-Whisper Tiny",
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
            WhisperModel::LargeV3TurboQ8 => "Whisper Large v3 Turbo (Q8_0)",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            WhisperModel::TinyEn => "OpenAI Whisper, English-only. Fastest, less accurate",
            WhisperModel::Tiny => "OpenAI Whisper, multilingual. Fastest, less accurate",
            WhisperModel::BaseEn => "OpenAI Whisper, English-only. Balanced speed and accuracy",
            WhisperModel::Base => "OpenAI Whisper, multilingual. Balanced speed and accuracy",
            WhisperModel::FinnishWhisperTiny => {
                "Finnish-NLP. Finnish fine-tune; optional evaluation model"
            }
            WhisperModel::KbWhisperTiny => "By KBLab. Swedish-optimized. Fastest, less accurate",
            WhisperModel::KbWhisperBase => {
                "By KBLab. Swedish-optimized. Balanced speed and accuracy"
            }
            WhisperModel::KbWhisperSmall => "By KBLab. Swedish-optimized. More accurate, slower",
            WhisperModel::KbWhisperMedium => "By KBLab. Swedish-optimized. High accuracy, slow",
            WhisperModel::KbWhisperLarge => "By KBLab. Swedish-optimized. Most accurate, slower",
            WhisperModel::NbWhisperTiny => {
                "By NbAiLab. Norwegian-optimized. Fastest, less accurate"
            }
            WhisperModel::NbWhisperBase => {
                "By NbAiLab. Norwegian-optimized. Balanced speed and accuracy"
            }
            WhisperModel::NbWhisperSmall => {
                "By NbAiLab. Norwegian-optimized. More accurate, slower"
            }
            WhisperModel::NbWhisperMedium => "By NbAiLab. Norwegian-optimized. High accuracy, slow",
            WhisperModel::NbWhisperLarge => {
                "By NbAiLab. Norwegian-optimized. Highest accuracy, slowest"
            }
            WhisperModel::SmallEn => "OpenAI Whisper, English-only. More accurate, slower",
            WhisperModel::Small => "OpenAI Whisper, multilingual. More accurate, slower",
            WhisperModel::MediumEn => "OpenAI Whisper, English-only. High accuracy, slow",
            WhisperModel::Medium => "OpenAI Whisper, multilingual. High accuracy, slow",
            WhisperModel::LargeV3Turbo => "OpenAI Whisper, multilingual. Highest accuracy, slowest",
            WhisperModel::LargeV3TurboQ8 => {
                "OpenAI Whisper large-v3-turbo, q8_0 quantised. High accuracy, multilingual, 834 MB"
            }
        }
    }

    pub fn is_english_only(&self) -> bool {
        matches!(
            self,
            WhisperModel::TinyEn
                | WhisperModel::BaseEn
                | WhisperModel::SmallEn
                | WhisperModel::MediumEn
        )
    }

    pub fn is_swedish_optimized(&self) -> bool {
        matches!(
            self,
            WhisperModel::KbWhisperTiny
                | WhisperModel::KbWhisperBase
                | WhisperModel::KbWhisperSmall
                | WhisperModel::KbWhisperMedium
                | WhisperModel::KbWhisperLarge
        )
    }

    pub fn is_finnish_optimized(&self) -> bool {
        matches!(self, WhisperModel::FinnishWhisperTiny)
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
            // All other models (tiny, base, kb-whisper, nb-whisper, medium, large): 0.3 works
            _ => 0.3,
        }
    }

    pub fn is_norwegian_optimized(&self) -> bool {
        matches!(
            self,
            WhisperModel::NbWhisperTiny
                | WhisperModel::NbWhisperBase
                | WhisperModel::NbWhisperSmall
                | WhisperModel::NbWhisperMedium
                | WhisperModel::NbWhisperLarge
        )
    }

    /// Fine-tuned language-specific models are intentionally biased toward
    /// their target language, so they must not be used to independently check
    /// whether the configured language matches an input file.
    pub fn is_language_optimized(&self) -> bool {
        self.is_swedish_optimized() || self.is_norwegian_optimized() || self.is_finnish_optimized()
    }

    /// Whether this model can honor an explicitly selected language without
    /// being biased toward a different language. Multilingual models support
    /// every explicit language and auto-detection; language-specific models
    /// support only their own language.
    pub fn is_compatible_with(&self, language: Language) -> bool {
        match language {
            Language::English => {
                !self.is_swedish_optimized()
                    && !self.is_norwegian_optimized()
                    && !self.is_finnish_optimized()
            }
            Language::Swedish => {
                !self.is_english_only()
                    && !self.is_norwegian_optimized()
                    && !self.is_finnish_optimized()
            }
            Language::Norwegian => {
                !self.is_english_only()
                    && !self.is_swedish_optimized()
                    && !self.is_finnish_optimized()
            }
            Language::Finnish => {
                self.is_finnish_optimized()
                    || (!self.is_english_only()
                        && !self.is_swedish_optimized()
                        && !self.is_norwegian_optimized())
            }
            Language::Auto => !self.is_english_only() && !self.is_language_optimized(),
        }
    }

    /// GGML model filename
    pub fn ggml_filename(&self) -> &'static str {
        match self {
            WhisperModel::TinyEn => "ggml-tiny.en.bin",
            WhisperModel::Tiny => "ggml-tiny.bin",
            WhisperModel::BaseEn => "ggml-base.en.bin",
            WhisperModel::Base => "ggml-base.bin",
            WhisperModel::FinnishWhisperTiny => "ggml-model-fi-tiny.bin",
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
            WhisperModel::LargeV3TurboQ8 => "ggml-large-v3-turbo-q8_0.bin",
        }
    }

    /// Approximate download size in MB
    pub fn size_mb(&self) -> u32 {
        match self {
            WhisperModel::TinyEn => 75,
            WhisperModel::Tiny => 75,
            WhisperModel::BaseEn => 142,
            WhisperModel::Base => 142,
            WhisperModel::FinnishWhisperTiny => 75,
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
            WhisperModel::LargeV3TurboQ8 => 834,
        }
    }

    /// Recommended model for a given language
    pub fn recommended(language: Language) -> WhisperModel {
        match language {
            Language::English => WhisperModel::BaseEn,
            // Whisper-family recommendation. Where Pianissimo runs, the Swedish
            // recommendation is Pianissimo instead: see
            // [`FileModel::recommended_for`], which every "Recommended" /
            // auto resolution goes through.
            Language::Swedish => WhisperModel::KbWhisperMedium,
            Language::Norwegian => WhisperModel::NbWhisperBase,
            Language::Finnish => WhisperModel::Base,
            Language::Auto => WhisperModel::Base,
        }
    }

    /// Swedish models that stay usable (CLI `--model`, existing settings) but
    /// are not offered in the app's pickers: Tiny collapses on long audio and
    /// Base/Small are beaten by Pianissimo and KB-Whisper Medium.
    pub fn is_hidden_in_app(&self) -> bool {
        matches!(
            self,
            WhisperModel::KbWhisperTiny
                | WhisperModel::KbWhisperBase
                | WhisperModel::KbWhisperSmall
        )
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
            Language::Finnish => &[
                WhisperModel::Tiny,
                WhisperModel::Base,
                WhisperModel::FinnishWhisperTiny,
                WhisperModel::Small,
                WhisperModel::Medium,
                WhisperModel::LargeV3Turbo,
                WhisperModel::LargeV3TurboQ8,
            ],
            Language::Auto => &[
                WhisperModel::Tiny,
                WhisperModel::Base,
                WhisperModel::Small,
                WhisperModel::Medium,
                WhisperModel::LargeV3Turbo,
                WhisperModel::LargeV3TurboQ8,
            ],
        }
    }
}

#[cfg(test)]
#[path = "whisper_tests.rs"]
mod tests;
