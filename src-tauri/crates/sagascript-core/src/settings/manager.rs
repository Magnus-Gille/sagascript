use std::collections::{BTreeMap, HashSet};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use super::{canonical_hotkey, validate_hotkey};

use crate::{download::DownloadIntegrity, transcription::Glossary};

#[cfg(target_os = "macos")]
const WHISPER_CPP_REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";

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
    #[serde(rename = "fi")]
    Finnish,
    #[serde(rename = "auto")]
    Auto,
}

impl Language {
    pub fn display_name(&self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Swedish => "Swedish",
            Language::Norwegian => "Norwegian",
            Language::Finnish => "Finnish",
            Language::Auto => "Auto-detect",
        }
    }

    /// Whisper language code (None for auto-detect)
    pub fn whisper_code(&self) -> Option<&'static str> {
        match self {
            Language::English => Some("en"),
            Language::Swedish => Some("sv"),
            Language::Norwegian => Some("no"),
            Language::Finnish => Some("fi"),
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
    /// DTW model preset for accurate attention-based token timestamps.
    /// KB-Whisper and NB-Whisper are fine-tunes of the corresponding base architecture,
    /// so they use the same alignment heads.
    #[cfg(feature = "diarization")]
    pub fn dtw_preset(&self) -> whisper_rs::DtwModelPreset {
        use whisper_rs::DtwModelPreset;
        match self {
            WhisperModel::TinyEn => DtwModelPreset::TinyEn,
            WhisperModel::Tiny
            | WhisperModel::FinnishWhisperTiny
            | WhisperModel::KbWhisperTiny
            | WhisperModel::NbWhisperTiny => DtwModelPreset::Tiny,
            WhisperModel::BaseEn => DtwModelPreset::BaseEn,
            WhisperModel::Base | WhisperModel::KbWhisperBase | WhisperModel::NbWhisperBase => {
                DtwModelPreset::Base
            }
            WhisperModel::SmallEn => DtwModelPreset::SmallEn,
            WhisperModel::Small | WhisperModel::KbWhisperSmall | WhisperModel::NbWhisperSmall => {
                DtwModelPreset::Small
            }
            WhisperModel::MediumEn => DtwModelPreset::MediumEn,
            WhisperModel::Medium
            | WhisperModel::KbWhisperMedium
            | WhisperModel::NbWhisperMedium => DtwModelPreset::Medium,
            WhisperModel::LargeV3Turbo | WhisperModel::LargeV3TurboQ8 => {
                DtwModelPreset::LargeV3Turbo
            }
            // KbWhisperLarge / NbWhisperLarge are large-v3 fine-tunes
            WhisperModel::KbWhisperLarge | WhisperModel::NbWhisperLarge => DtwModelPreset::LargeV3,
        }
    }

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

    /// Pinned download URL for the model artifact.
    pub fn download_url(&self) -> &'static str {
        match self {
            WhisperModel::TinyEn => "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-tiny.en.bin",
            WhisperModel::Tiny => "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-tiny.bin",
            WhisperModel::BaseEn => "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.en.bin",
            WhisperModel::Base => "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.bin",
            WhisperModel::FinnishWhisperTiny => "https://huggingface.co/Finnish-NLP/Finnish-finetuned-whisper-models-ggml-format/resolve/c58924b6deb4438756b3d38ecd67d65bdf20298d/ggml-model-fi-tiny.bin",
            WhisperModel::KbWhisperTiny => "https://huggingface.co/KBLab/kb-whisper-tiny/resolve/76d796af43a50fa34321efa562c9b9887a187463/ggml-model-q5_0.bin",
            WhisperModel::KbWhisperBase => "https://huggingface.co/KBLab/kb-whisper-base/resolve/1499d2d2f0c7ed545bd6f2eec85287cf8d8c8b38/ggml-model-q5_0.bin",
            WhisperModel::KbWhisperSmall => "https://huggingface.co/KBLab/kb-whisper-small/resolve/3564d61a42fc210ceaa55a22a96dd64478959c78/ggml-model-q5_0.bin",
            WhisperModel::KbWhisperMedium => "https://huggingface.co/KBLab/kb-whisper-medium/resolve/0abe10b9d7f75d0902656e5c06c5c4d549604dc5/ggml-model-q5_0.bin",
            WhisperModel::KbWhisperLarge => "https://huggingface.co/KBLab/kb-whisper-large/resolve/d5d5984b4d8f7c4847a8ea203f1976285fb28300/ggml-model-q5_0.bin",
            WhisperModel::NbWhisperTiny => "https://huggingface.co/NbAiLab/nb-whisper-tiny/resolve/8b38492d0e4111d5d6ad825e979cb082a2da013a/ggml-model-q5_0.bin",
            WhisperModel::NbWhisperBase => "https://huggingface.co/NbAiLab/nb-whisper-base/resolve/2ab372b6baa181a22f54f18030cae3703402c59e/ggml-model-q5_0.bin",
            WhisperModel::NbWhisperSmall => "https://huggingface.co/NbAiLab/nb-whisper-small/resolve/e9bb5cb83cb74c96239fd506163aa97cff2fce4c/ggml-model-q5_0.bin",
            WhisperModel::NbWhisperMedium => "https://huggingface.co/NbAiLab/nb-whisper-medium/resolve/0ed074d5985bd56ca4140159a9dbffbc3fb5117e/ggml-model-q5_0.bin",
            WhisperModel::NbWhisperLarge => "https://huggingface.co/NbAiLab/nb-whisper-large/resolve/8c6249fdeeb4dcd05e5735a4c39640607eb6e4ac/ggml-model-q5_0.bin",
            WhisperModel::SmallEn => "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-small.en.bin",
            WhisperModel::Small => "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-small.bin",
            WhisperModel::MediumEn => "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-medium.en.bin",
            WhisperModel::Medium => "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-medium.bin",
            WhisperModel::LargeV3Turbo => "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo.bin",
            WhisperModel::LargeV3TurboQ8 => "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo-q8_0.bin",
        }
    }

    /// Exact git-LFS metadata for the artifact at [`Self::download_url`].
    pub fn download_integrity(&self) -> DownloadIntegrity {
        match self {
            WhisperModel::TinyEn => DownloadIntegrity {
                sha256: "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f",
                size: 77_704_715,
            },
            WhisperModel::Tiny => DownloadIntegrity {
                sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
                size: 77_691_713,
            },
            WhisperModel::BaseEn => DownloadIntegrity {
                sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
                size: 147_964_211,
            },
            WhisperModel::Base => DownloadIntegrity {
                sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
                size: 147_951_465,
            },
            WhisperModel::FinnishWhisperTiny => DownloadIntegrity {
                sha256: "41cf309b7f50523cfca724ae90924fcd0e4794205de57a66abc3cce627103ce8",
                size: 77_691_730,
            },
            WhisperModel::KbWhisperTiny => DownloadIntegrity {
                sha256: "98d46b7d23e5528d006e8a42e29eb0cb39b44bed94e1329f10f57d1fd15c658b",
                size: 29_875_738,
            },
            WhisperModel::KbWhisperBase => DownloadIntegrity {
                sha256: "aead29b356bca8840e72a8dc2286e2d69e6702639751a1e60cb3c8eacefec546",
                size: 55_295_450,
            },
            WhisperModel::KbWhisperSmall => DownloadIntegrity {
                sha256: "6768836a51abc902e420c613153e6d418c90ea2774e913274d02ab23170225b7",
                size: 175_209_680,
            },
            WhisperModel::KbWhisperMedium => DownloadIntegrity {
                sha256: "7f8762e0ade9e0073674c0d5acae942a0b1ea98add9baa008ee89c94eaba43d0",
                size: 539_212_484,
            },
            WhisperModel::KbWhisperLarge => DownloadIntegrity {
                sha256: "6d2863812d7410322bb7d8647a5c7260761300fa946714c9ed66d22bb30bcb19",
                size: 1_081_140_203,
            },
            WhisperModel::NbWhisperTiny => DownloadIntegrity {
                sha256: "e5fb42192cdf31bea624a524d035e8895030b2bb4b31d4ea2a1ebf0ea8f57237",
                size: 29_875_738,
            },
            WhisperModel::NbWhisperBase => DownloadIntegrity {
                sha256: "dcb9f3ab963cd288974c826c1519ff73b78b2372e80d388a6ce94f29c6a5b40f",
                size: 55_295_450,
            },
            WhisperModel::NbWhisperSmall => DownloadIntegrity {
                sha256: "2a9025afb6e825fc4ae6a46671e0cb2f43e62f1dec87270deea6fe61b5285a20",
                size: 175_209_680,
            },
            WhisperModel::NbWhisperMedium => DownloadIntegrity {
                sha256: "18733de634af639a43b0f8c5f5a2ea0920de4c5b32a5570ec130981581c0e5e7",
                size: 539_212_484,
            },
            WhisperModel::NbWhisperLarge => DownloadIntegrity {
                sha256: "feb5951ae694a62cfeb81fb501f6cfa8cc50d96bcddb1e4e8215f7006bac23a2",
                size: 1_081_140_203,
            },
            WhisperModel::SmallEn => DownloadIntegrity {
                sha256: "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
                size: 487_614_201,
            },
            WhisperModel::Small => DownloadIntegrity {
                sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
                size: 487_601_967,
            },
            WhisperModel::MediumEn => DownloadIntegrity {
                sha256: "cc37e93478338ec7700281a7ac30a10128929eb8f427dda2e865faa8f6da4356",
                size: 1_533_774_781,
            },
            WhisperModel::Medium => DownloadIntegrity {
                sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
                size: 1_533_763_059,
            },
            WhisperModel::LargeV3Turbo => DownloadIntegrity {
                sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
                size: 1_624_555_275,
            },
            WhisperModel::LargeV3TurboQ8 => DownloadIntegrity {
                sha256: "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1",
                size: 874_188_075,
            },
        }
    }

    /// CoreML encoder basename whisper.cpp derives from the GGML filename: strip
    /// `.bin`, then strip a trailing `-qX_X` quantisation suffix (mirrors
    /// whisper.cpp's `whisper_get_coreml_path_encoder`). The CoreML encoder is
    /// FP16 and shared across quantisations of a model — so `large-v3-turbo-q8_0`
    /// reuses the same `ggml-large-v3-turbo-encoder.mlmodelc`. Returns `None` for
    /// models without a CoreML encoder: only the OpenAI models in the
    /// ggerganov/whisper.cpp repo ship one; the KB/NB fine-tunes live in their
    /// own repos and have none.
    #[cfg(target_os = "macos")]
    fn coreml_encoder_stem(&self) -> Option<&'static str> {
        if !self
            .download_url()
            .starts_with("https://huggingface.co/ggerganov/whisper.cpp/")
        {
            return None;
        }
        self.whisper_cpp_encoder_stem()
    }

    /// The encoder name whisper.cpp *probes* next to the GGML file, whether or
    /// not an encoder exists for this model (`.bin` -> `-encoder.mlmodelc`,
    /// with a trailing "-qX_X" removed, exactly as whisper.cpp does).
    #[cfg(target_os = "macos")]
    fn whisper_cpp_encoder_stem(&self) -> Option<&'static str> {
        let stem = self.ggml_filename().strip_suffix(".bin")?;
        // Strip a trailing "-qX_X" (e.g. "-q8_0"), exactly as whisper.cpp does.
        let stem = match stem.rfind('-') {
            Some(pos) => {
                let suffix = &stem.as_bytes()[pos..];
                if suffix.len() == 5 && suffix[1] == b'q' && suffix[3] == b'_' {
                    &stem[..pos]
                } else {
                    stem
                }
            }
            None => stem,
        };
        Some(stem)
    }

    /// Directory name whisper.cpp will try (and, for fine-tunes, fail) to load
    /// as a Core ML encoder, or `None` when this model has a real encoder (a
    /// failed load is then a genuine problem worth reporting).
    #[cfg(target_os = "macos")]
    pub fn unsupported_coreml_probe_dirname(&self) -> Option<String> {
        if self.coreml_encoder_stem().is_some() {
            return None;
        }
        Some(format!(
            "{}-encoder.mlmodelc",
            self.whisper_cpp_encoder_stem()?
        ))
    }

    /// Non-macOS: whisper.cpp is built without Core ML and never probes.
    #[cfg(not(target_os = "macos"))]
    pub fn unsupported_coreml_probe_dirname(&self) -> Option<String> {
        None
    }

    /// HuggingFace URL of the CoreML encoder bundle (`*-encoder.mlmodelc.zip`),
    /// or `None` if this model has no CoreML encoder.
    #[cfg(target_os = "macos")]
    pub fn coreml_encoder_url(&self) -> Option<String> {
        let stem = self.coreml_encoder_stem()?;
        Some(format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/{WHISPER_CPP_REVISION}/{stem}-encoder.mlmodelc.zip"
        ))
    }

    /// Exact git-LFS metadata for the CoreML encoder archive.
    #[cfg(target_os = "macos")]
    pub fn coreml_encoder_integrity(&self) -> Option<DownloadIntegrity> {
        match self {
            WhisperModel::TinyEn => Some(DownloadIntegrity {
                sha256: "82b32eef73c94bb0c432a776a047b757d9525c26d84038a15d8798d7c8d1ee58",
                size: 15_034_655,
            }),
            WhisperModel::Tiny => Some(DownloadIntegrity {
                sha256: "c88cbd2648e1f5415092bcf5256add463a0f19943e6938f46e8d4ffdebd47739",
                size: 15_037_446,
            }),
            WhisperModel::BaseEn => Some(DownloadIntegrity {
                sha256: "8cf860309e2449e2bdc8be834cf838ab2565747ecc8c0ef914ef5975115e192b",
                size: 37_950_917,
            }),
            WhisperModel::Base => Some(DownloadIntegrity {
                sha256: "7e6ab77041942572f239b5b602f8aaa1c3ed29d73e3d8f20abea03a773541089",
                size: 37_922_638,
            }),
            WhisperModel::SmallEn => Some(DownloadIntegrity {
                sha256: "b2ef1c506378b825b4b4341979a93e1656b5d6c129f17114cfb8fb78aabc2f89",
                size: 162_952_446,
            }),
            WhisperModel::Small => Some(DownloadIntegrity {
                sha256: "de43fb9fed471e95c19e60ae67575c2bf09e8fb607016da171b06ddad313988b",
                size: 163_083_239,
            }),
            WhisperModel::MediumEn => Some(DownloadIntegrity {
                sha256: "cdc44fee3c62b5743913e3147ed75f4e8ecfb52dd7a0f0f7387094b406ff0ee6",
                size: 566_993_085,
            }),
            WhisperModel::Medium => Some(DownloadIntegrity {
                sha256: "79b0b8d436d47d3f24dd3afc91f19447dd686a4f37521b2f6d9c30a642133fbd",
                size: 567_829_413,
            }),
            WhisperModel::LargeV3Turbo | WhisperModel::LargeV3TurboQ8 => Some(DownloadIntegrity {
                sha256: "84bedfe895bd7b5de6e8e89a0803dfc5addf8c0c5bc4c937451716bf7cf7988a",
                size: 1_173_393_014,
            }),
            WhisperModel::KbWhisperTiny
            | WhisperModel::KbWhisperBase
            | WhisperModel::KbWhisperSmall
            | WhisperModel::KbWhisperMedium
            | WhisperModel::KbWhisperLarge
            | WhisperModel::NbWhisperTiny
            | WhisperModel::NbWhisperBase
            | WhisperModel::NbWhisperSmall
            | WhisperModel::NbWhisperMedium
            | WhisperModel::NbWhisperLarge
            | WhisperModel::FinnishWhisperTiny => None,
        }
    }

    /// Directory name whisper.cpp expects the CoreML encoder to have next to the
    /// GGML file (`ggml-<name>-encoder.mlmodelc`). `None` if no CoreML encoder.
    #[cfg(target_os = "macos")]
    pub fn coreml_encoder_dirname(&self) -> Option<String> {
        let stem = self.coreml_encoder_stem()?;
        Some(format!("{stem}-encoder.mlmodelc"))
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

/// A file-transcription model choice. Unlike [`WhisperModel`], this can also
/// select the Swedish-only Pianissimo Original model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileModel {
    Whisper(WhisperModel),
    PianissimoOriginal,
}

impl FileModel {
    /// The engine behind "Recommended for <language>": Pianissimo for Swedish
    /// where it runs (`pianissimo_supported`), otherwise the language's
    /// recommended Whisper model. Single source of truth for every automatic
    /// resolution (dictation profiles, file transcription, CLI, UI labels).
    pub fn recommended_for(language: Language, pianissimo_supported: bool) -> FileModel {
        if language == Language::Swedish && pianissimo_supported {
            FileModel::PianissimoOriginal
        } else {
            FileModel::Whisper(WhisperModel::recommended(language))
        }
    }

    /// Models the app offers for `language`, after the "Recommended" entry.
    /// Swedish: Pianissimo + KB-Whisper Large where Pianissimo runs, otherwise
    /// KB-Whisper Medium + Large. `include` keeps an already-selected model
    /// (e.g. a legacy KB-Whisper Base) selectable instead of hiding the
    /// current choice.
    pub fn app_lineup(
        language: Language,
        pianissimo_supported: bool,
        include: Option<FileModel>,
    ) -> Vec<FileModel> {
        let mut models: Vec<FileModel> = if language == Language::Swedish {
            if pianissimo_supported {
                vec![
                    FileModel::PianissimoOriginal,
                    FileModel::Whisper(WhisperModel::KbWhisperLarge),
                ]
            } else {
                vec![
                    FileModel::Whisper(WhisperModel::KbWhisperMedium),
                    FileModel::Whisper(WhisperModel::KbWhisperLarge),
                ]
            }
        } else {
            WhisperModel::models_for_language(language)
                .iter()
                .copied()
                .map(FileModel::Whisper)
                .collect()
        };
        if let Some(extra) = include {
            let valid = match extra {
                FileModel::Whisper(model) => model.is_compatible_with(language),
                FileModel::PianissimoOriginal => {
                    language == Language::Swedish && pianissimo_supported
                }
            };
            if valid && !models.contains(&extra) {
                models.push(extra);
            }
        }
        models
    }
}

/// Real on-disk readiness of a file-transcription model.
fn real_file_model_ready(model: FileModel) -> bool {
    match model {
        FileModel::Whisper(m) => crate::transcription::model::is_model_downloaded(m),
        FileModel::PianissimoOriginal => crate::transcription::pianissimo_model::is_downloaded(),
    }
}

/// Resolve an `Auto` file-model preference against what is downloaded.
/// `allow_pianissimo` false yields Whisper models only.
fn auto_file_model(
    language: Language,
    pianissimo_supported: bool,
    allow_pianissimo: bool,
    is_ready: &dyn Fn(FileModel) -> bool,
) -> FileModel {
    let supported = pianissimo_supported && allow_pianissimo;
    let recommended = FileModel::recommended_for(language, supported);
    if is_ready(recommended) {
        return recommended;
    }
    let mut candidates = FileModel::app_lineup(language, supported, None);
    let mut rest: Vec<WhisperModel> = WhisperModel::models_for_language(language).to_vec();
    rest.sort_by_key(|m| std::cmp::Reverse(m.size_mb()));
    candidates.extend(rest.into_iter().map(FileModel::Whisper));
    candidates
        .into_iter()
        .filter(|m| allow_pianissimo || *m != FileModel::PianissimoOriginal)
        .find(|m| is_ready(*m))
        .unwrap_or(recommended)
}

/// Persisted preference for file transcription.
///
/// `Auto` chooses the requested language's recommended Whisper model,
/// independently of every live-dictation profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FileModelPreference {
    #[default]
    Auto,
    Whisper(WhisperModel),
    PianissimoOriginal,
}

impl FileModelPreference {
    /// Parse one of the stable persisted model identifiers.
    pub fn parse_id(id: &str) -> Result<Self, String> {
        match id {
            "auto" => Ok(Self::Auto),
            "pianissimo-sv" => Ok(Self::PianissimoOriginal),
            _ => serde_json::from_value::<WhisperModel>(serde_json::Value::String(id.to_owned()))
                .map(Self::Whisper)
                .map_err(|_| format!("Unknown file model preference '{id}'")),
        }
    }
}

impl Serialize for FileModelPreference {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Auto => serializer.serialize_str("auto"),
            Self::Whisper(model) => model.serialize(serializer),
            Self::PianissimoOriginal => serializer.serialize_str("pianissimo-sv"),
        }
    }
}

impl<'de> Deserialize<'de> for FileModelPreference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FileModelPreferenceVisitor;

        impl<'de> de::Visitor<'de> for FileModelPreferenceVisitor {
            type Value = FileModelPreference;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a file model preference identifier")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                FileModelPreference::parse_id(value).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(FileModelPreferenceVisitor)
    }
}

/// Hotkey activation mode
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HotkeyMode {
    #[serde(rename = "push", alias = "presenter")]
    #[default]
    PushToTalk,
    #[serde(rename = "toggle")]
    Toggle,
}

/// When the Pianissimo engine host loads its model ahead of the first utterance.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnginePrewarm {
    /// Load lazily when the first transcription needs the model.
    Off,
    /// Load in the background when the app starts, after system wake and when
    /// a profile switches to Pianissimo (Pianissimo active only). Default.
    #[default]
    OnAppStart,
    /// Load in the background when the push-to-talk key goes down.
    OnKeyDown,
}

impl EnginePrewarm {
    pub const ALL: [EnginePrewarm; 3] = [Self::Off, Self::OnAppStart, Self::OnKeyDown];

    /// Stable identifier used in the settings file and by `sagascript config`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::OnAppStart => "on_app_start",
            Self::OnKeyDown => "on_key_down",
        }
    }

    pub fn parse_id(value: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.id() == value)
            .ok_or_else(|| {
                format!("engine_prewarm must be off, on_app_start or on_key_down, got '{value}'")
            })
    }
}

/// Default minutes of inactivity before the engine host unloads its model.
pub const DEFAULT_ENGINE_IDLE_UNLOAD_MINUTES: u32 = 60;

/// Defaults that shipped before [`ENGINE_DEFAULTS_VERSION`] 1, used to tell
/// users still on the old defaults from users who chose something else.
pub const LEGACY_ENGINE_IDLE_UNLOAD_MINUTES: u32 = 10;

/// Current version of the engine warm-up defaults migration.
pub const ENGINE_DEFAULTS_VERSION: u32 = 1;

/// One global shortcut and the transcription language selected when it fires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyProfile {
    pub id: String,
    pub name: String,
    pub shortcut: String,
    pub language: Language,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_to_talk_shortcut: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toggle_shortcut: Option<String>,
}

impl HotkeyProfile {
    pub fn legacy_default(shortcut: String, language: Language) -> Self {
        Self {
            id: "default".to_string(),
            name: "Default".to_string(),
            shortcut,
            language,
            push_to_talk_shortcut: None,
            toggle_shortcut: None,
        }
    }

    /// Update the compatibility shortcut and whichever explicit binding is
    /// primary for this profile.
    pub fn set_primary_shortcut(&mut self, shortcut: String) {
        if self.push_to_talk_shortcut.is_some() {
            self.push_to_talk_shortcut = Some(shortcut.clone());
        } else if self.toggle_shortcut.is_some() {
            self.toggle_shortcut = Some(shortcut.clone());
        }
        self.shortcut = shortcut;
    }
}

/// All user-configurable settings, persisted as JSON
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub language: Language,
    pub whisper_model: WhisperModel,
    pub file_transcription_model: FileModelPreference,
    /// Use the experimental native Pianissimo runtime for Swedish live dictation.
    /// File transcription continues to use `file_transcription_model`.
    pub pianissimo_dictation: bool,
    /// When the engine host loads the Pianissimo model ahead of use.
    pub engine_prewarm: EnginePrewarm,
    /// Minutes of inactivity before the engine host unloads its model; the host
    /// process itself exits after twice this. 0 disables idle unloading.
    pub engine_idle_unload_minutes: u32,
    pub hotkey_mode: HotkeyMode,
    pub show_overlay: bool,
    pub auto_paste: bool,
    pub auto_select_model: bool,
    /// Hotkey shortcut string (e.g. "Control+Shift+Space")
    pub hotkey: String,
    /// Explicit dictation profiles. Empty means a legacy settings file; callers
    /// use `resolved_hotkey_profiles()` to synthesize its default profile.
    pub hotkey_profiles: Vec<HotkeyProfile>,
    /// Live model owned by each profile id. `Auto` selects the recommendation
    /// for that profile's language; file transcription has its own preference.
    pub profile_models: BTreeMap<String, FileModelPreference>,
    /// Optional initial prompt that primes the decoder with domain vocabulary
    /// (names, jargon, spellings) for more accurate transcription. Empty = none.
    pub initial_prompt: String,
    /// Additional personal dictionary entries scoped to a dictation profile.
    /// The legacy `initial_prompt` remains on disk only for rollback.
    pub profile_glossaries: BTreeMap<String, String>,
    /// Legacy global glossary has been copied into the profile dictionaries.
    /// The original file remains on disk for rollback, but no longer affects
    /// dictation once this marker is set.
    pub profile_glossary_migrated: bool,
    /// Beam search width. 0 = greedy decoding (fastest); >=2 enables beam search
    /// (more accurate on hard audio, several times slower).
    pub beam_size: u32,
    /// Allow whisper's temperature fallback (re-decode hard segments at higher
    /// temperature). true preserves robustness; false caps worst-case latency.
    pub temperature_fallback: bool,
    /// Skip non-speech regions with Silero VAD (reduces silence hallucination
    /// and speeds up clips with leading/trailing silence). Needs the VAD model.
    pub vad_enabled: bool,
    /// Whether the user has completed the first-launch onboarding
    #[serde(alias = "hasCompletedOnboarding")]
    pub has_completed_onboarding: bool,
    /// Version of the Swedish model-lineup migration that has run on this file
    /// (see [`Settings::migrate_swedish_lineup`]). A missing key means 0: files
    /// written before the lineup change are migrated once. New installs start
    /// at the current version.
    #[serde(default)]
    pub model_lineup_version: u32,
    /// Version of the engine warm-up defaults migration that has run on this
    /// file (see [`Settings::migrate_engine_defaults`]). A missing key means 0.
    /// New installs start at the current version.
    #[serde(default)]
    pub engine_defaults_version: u32,
}

/// Current version of the Swedish model-lineup migration.
pub const MODEL_LINEUP_VERSION: u32 = 1;

/// Machine state the lineup migration depends on, injected for testability.
pub struct LineupEnv<'a> {
    /// `pianissimo_backend::runtime_supported_on_this_os()`.
    pub pianissimo_supported: bool,
    /// `pianissimo_model::is_downloaded()`.
    pub pianissimo_downloaded: bool,
    /// `model::is_model_downloaded`.
    pub whisper_downloaded: &'a dyn Fn(WhisperModel) -> bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Language::default(),
            whisper_model: WhisperModel::default(),
            file_transcription_model: FileModelPreference::default(),
            pianissimo_dictation: false,
            engine_prewarm: EnginePrewarm::default(),
            engine_idle_unload_minutes: DEFAULT_ENGINE_IDLE_UNLOAD_MINUTES,
            hotkey_mode: HotkeyMode::default(),
            show_overlay: true,
            auto_paste: true,
            auto_select_model: true,
            hotkey: "Control+Shift+Space".to_string(),
            hotkey_profiles: Vec::new(),
            profile_models: BTreeMap::new(),
            initial_prompt: String::new(),
            profile_glossaries: BTreeMap::new(),
            profile_glossary_migrated: false,
            beam_size: 0,
            temperature_fallback: true,
            vad_enabled: false,
            has_completed_onboarding: false,
            model_lineup_version: MODEL_LINEUP_VERSION,
            engine_defaults_version: ENGINE_DEFAULTS_VERSION,
        }
    }
}

/// A profile's model was reset to Auto because a language change made it
/// incompatible. Structured so callers can filter by profile id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelResetNotice {
    pub profile_id: String,
    pub profile_name: String,
    pub message: String,
}

impl ModelResetNotice {
    /// User-facing strings, for wire formats that carry plain messages.
    pub fn messages(notices: &[ModelResetNotice]) -> Vec<String> {
        notices
            .iter()
            .map(|notice| notice.message.clone())
            .collect()
    }
}

impl Settings {
    /// One-time move of the engine warm-up settings to the cold-start defaults
    /// (load at app start, unload after 60 idle minutes), recorded by
    /// `engine_defaults_version`. Only values still equal to the previous
    /// defaults move: an explicit `off`, `on_app_start` or any other idle
    /// timeout is kept. (A user who explicitly picked the old default values
    /// is indistinguishable from one who never touched them and moves too.)
    /// Returns true when it ran (the caller must persist).
    pub fn migrate_engine_defaults(&mut self) -> bool {
        if self.engine_defaults_version >= ENGINE_DEFAULTS_VERSION {
            return false;
        }
        self.engine_defaults_version = ENGINE_DEFAULTS_VERSION;
        if self.engine_prewarm == EnginePrewarm::OnKeyDown {
            self.engine_prewarm = EnginePrewarm::OnAppStart;
        }
        if self.engine_idle_unload_minutes == LEGACY_ENGINE_IDLE_UNLOAD_MINUTES {
            self.engine_idle_unload_minutes = DEFAULT_ENGINE_IDLE_UNLOAD_MINUTES;
        }
        true
    }

    /// One-time move of Swedish dictation off the retired KB-Whisper
    /// Tiny/Base/Small choices, recorded by `model_lineup_version` so a second
    /// run is a no-op. Returns true when it ran (the caller must persist).
    ///
    /// Safety rule: dictation must keep working. When a profile's model is not
    /// downloaded, the hotkey flow fails the utterance ("model not
    /// downloaded") and Settings shows a "<model> required" download prompt,
    /// so switching engines under a user who lacks the new one would break
    /// dictation until they download ~608 MB. Therefore a profile only moves
    /// when the recommended engine is already downloaded, or when its current
    /// model is not usable either (then the move costs nothing and Settings
    /// prompts for the recommended engine). Otherwise the old model is kept
    /// and Settings offers a one-click switch that downloads first.
    /// `Auto` profiles used to resolve to KB-Whisper Base; if Base is
    /// downloaded but the new recommendation is not, they are pinned to Base
    /// for the same reason. Explicit Medium/Large/Pianissimo are untouched.
    pub fn migrate_swedish_lineup(&mut self, env: &LineupEnv<'_>) -> bool {
        if self.model_lineup_version >= MODEL_LINEUP_VERSION {
            return false;
        }
        self.model_lineup_version = MODEL_LINEUP_VERSION;
        let target = FileModel::recommended_for(Language::Swedish, env.pianissimo_supported);
        let (target_pref, target_ready) = match target {
            FileModel::PianissimoOriginal => (
                FileModelPreference::PianissimoOriginal,
                env.pianissimo_downloaded,
            ),
            FileModel::Whisper(model) => (
                FileModelPreference::Whisper(model),
                (env.whisper_downloaded)(model),
            ),
        };
        let previous_auto = WhisperModel::KbWhisperBase;
        let swedish: Vec<String> = self
            .resolved_hotkey_profiles()
            .into_iter()
            .filter(|profile| profile.language == Language::Swedish)
            .map(|profile| profile.id)
            .collect();
        for id in swedish {
            let Some(current) = self.profile_models.get(&id).copied() else {
                continue;
            };
            match current {
                FileModelPreference::Whisper(old) if old.is_hidden_in_app() => {
                    if target_ready || !(env.whisper_downloaded)(old) {
                        tracing::info!(profile = %id, from = ?old, to = ?target_pref, "Swedish model lineup: moved profile to the recommended model");
                        self.profile_models.insert(id, target_pref);
                    } else {
                        tracing::info!(profile = %id, model = ?old, "Swedish model lineup: kept profile model because the recommended model is not downloaded");
                    }
                }
                FileModelPreference::Auto
                    if !target_ready && (env.whisper_downloaded)(previous_auto) =>
                {
                    tracing::info!(profile = %id, model = ?previous_auto, "Swedish model lineup: pinned Auto profile to its previous model because the recommended model is not downloaded");
                    self.profile_models
                        .insert(id, FileModelPreference::Whisper(previous_auto));
                }
                _ => {}
            }
        }
        // Legacy global choice (rollback compatibility): a Whisper field
        // cannot hold Pianissimo, so it only ever moves to KB-Whisper Medium.
        if !self.auto_select_model && self.whisper_model.is_hidden_in_app() {
            let medium = WhisperModel::KbWhisperMedium;
            if (env.whisper_downloaded)(medium) || !(env.whisper_downloaded)(self.whisper_model) {
                tracing::info!(from = ?self.whisper_model, to = ?medium, "Swedish model lineup: moved legacy global model");
                self.whisper_model = medium;
            }
        }
        true
    }

    pub fn default_profile(&self) -> HotkeyProfile {
        let profiles = self.resolved_hotkey_profiles();
        profiles
            .iter()
            .find(|profile| profile.id == "default")
            .unwrap_or(&profiles[0])
            .clone()
    }

    /// Preserve the effective live model of every existing profile before
    /// global model preferences stop being consulted for live dictation.
    pub fn materialize_profile_models(&mut self) {
        if self.hotkey_profiles.is_empty() {
            self.hotkey_profiles.push(HotkeyProfile::legacy_default(
                self.hotkey.clone(),
                self.language,
            ));
        }
        for profile in &self.hotkey_profiles {
            let preference = if self.pianissimo_dictation && profile.language == Language::Swedish {
                FileModelPreference::PianissimoOriginal
            } else if self.auto_select_model
                || !self.whisper_model.is_compatible_with(profile.language)
            {
                FileModelPreference::Auto
            } else {
                FileModelPreference::Whisper(self.whisper_model)
            };
            self.profile_models
                .entry(profile.id.clone())
                .or_insert(preference);
        }
    }

    pub fn migrate_global_glossary_to_profiles(&mut self) {
        if self.profile_glossary_migrated {
            return;
        }
        let hints: Vec<String> = Glossary::parse(&self.initial_prompt)
            .entries()
            .iter()
            .map(|entry| entry.canonical.clone())
            .collect();
        for profile in self.resolved_hotkey_profiles() {
            let source = self.profile_glossaries.entry(profile.id).or_default();
            let mut known: HashSet<String> = Glossary::parse(source)
                .entries()
                .iter()
                .map(|entry| entry.canonical.to_lowercase())
                .collect();
            for hint in &hints {
                if known.insert(hint.to_lowercase()) {
                    if !source.trim().is_empty() {
                        source.push('\n');
                    }
                    source.push_str(hint);
                }
            }
        }
        self.profile_glossary_migrated = true;
    }

    /// Resolve the live engine for one profile. Unmigrated settings retain
    /// their previous global selection until the profile migration is saved.
    pub fn dictation_model_for_profile(&self, profile_id: &str) -> Result<FileModel, String> {
        self.dictation_model_for_profile_gated(
            profile_id,
            crate::transcription::pianissimo_backend::runtime_supported_on_this_os(),
        )
    }

    /// [`Self::dictation_model_for_profile`] with the Pianissimo platform gate supplied by the caller.
    pub fn dictation_model_for_profile_gated(
        &self,
        profile_id: &str,
        pianissimo_supported: bool,
    ) -> Result<FileModel, String> {
        let profile = self
            .resolved_hotkey_profiles()
            .into_iter()
            .find(|profile| profile.id == profile_id)
            .ok_or_else(|| format!("Unknown profile '{profile_id}'"))?;
        let preference = self
            .profile_models
            .get(profile_id)
            .copied()
            .unwrap_or_else(|| {
                if self.uses_pianissimo_for_dictation(profile.language) {
                    FileModelPreference::PianissimoOriginal
                } else if self.auto_select_model
                    || !self.whisper_model.is_compatible_with(profile.language)
                {
                    FileModelPreference::Auto
                } else {
                    FileModelPreference::Whisper(self.whisper_model)
                }
            });
        match preference {
            FileModelPreference::Auto => Ok(FileModel::recommended_for(
                profile.language,
                pianissimo_supported,
            )),
            FileModelPreference::Whisper(model) if model.is_compatible_with(profile.language) => {
                Ok(FileModel::Whisper(model))
            }
            FileModelPreference::Whisper(model) => Err(format!(
                "Model '{}' is incompatible with profile '{}'",
                model.display_name(),
                profile_id
            )),
            FileModelPreference::PianissimoOriginal if profile.language == Language::Swedish => {
                Ok(FileModel::PianissimoOriginal)
            }
            FileModelPreference::PianissimoOriginal => Err(format!(
                "Pianissimo is only available for Swedish profile '{profile_id}'"
            )),
        }
    }

    pub fn live_model_for_profile(&self, profile_id: &str) -> Result<(WhisperModel, bool), String> {
        match self.dictation_model_for_profile(profile_id)? {
            FileModel::Whisper(model) => Ok((model, false)),
            // The Whisper slot is unused when the flag is set; keep the Whisper
            // recommendation as a harmless placeholder.
            FileModel::PianissimoOriginal => {
                Ok((WhisperModel::recommended(Language::Swedish), true))
            }
        }
    }

    /// True when `profile_id`'s live dictation model is Pianissimo.
    pub fn profile_uses_pianissimo(&self, profile_id: &str) -> bool {
        self.live_model_for_profile(profile_id)
            .is_ok_and(|(_, pianissimo)| pianissimo)
    }

    /// True when any dictation profile's live model is Pianissimo.
    pub fn any_profile_uses_pianissimo(&self) -> bool {
        self.resolved_hotkey_profiles()
            .iter()
            .any(|profile| self.profile_uses_pianissimo(&profile.id))
    }

    pub fn set_profile_model(
        &mut self,
        profile_id: &str,
        preference: FileModelPreference,
    ) -> Result<(), String> {
        self.set_profile_model_gated(
            profile_id,
            preference,
            crate::transcription::pianissimo_backend::runtime_supported_on_this_os(),
        )
    }

    /// [`Self::set_profile_model`] with the Pianissimo platform gate supplied by the caller.
    pub fn set_profile_model_gated(
        &mut self,
        profile_id: &str,
        preference: FileModelPreference,
        pianissimo_supported: bool,
    ) -> Result<(), String> {
        if preference == FileModelPreference::PianissimoOriginal && !pianissimo_supported {
            return Err(crate::transcription::pianissimo_backend::UNSUPPORTED_MESSAGE.into());
        }
        let profile = self
            .resolved_hotkey_profiles()
            .into_iter()
            .find(|profile| profile.id == profile_id)
            .ok_or_else(|| format!("Unknown profile '{profile_id}'"))?;
        match preference {
            FileModelPreference::Whisper(model) if !model.is_compatible_with(profile.language) => {
                return Err(format!(
                    "Model '{}' is incompatible with profile '{profile_id}'",
                    model.display_name()
                ));
            }
            FileModelPreference::PianissimoOriginal if profile.language != Language::Swedish => {
                return Err("Pianissimo requires a Swedish profile".to_string());
            }
            _ => {}
        }
        self.profile_models
            .insert(profile_id.to_string(), preference);
        Ok(())
    }

    /// Settings migration for systems that cannot run Pianissimo (anything but
    /// macOS 14+ on Apple Silicon or Windows on ARM): every selection of it falls back to the
    /// recommended Whisper model (`Auto`). Returns true when anything changed.
    pub fn demote_unsupported_pianissimo(&mut self, pianissimo_supported: bool) -> bool {
        if pianissimo_supported {
            return false;
        }
        let mut changed = false;
        for (profile, preference) in self.profile_models.iter_mut() {
            if *preference == FileModelPreference::PianissimoOriginal {
                tracing::warn!(
                    "Pianissimo is not supported on this system; profile '{profile}' now uses the recommended Whisper model"
                );
                *preference = FileModelPreference::Auto;
                changed = true;
            }
        }
        if self.file_transcription_model == FileModelPreference::PianissimoOriginal {
            tracing::warn!(
                "Pianissimo is not supported on this system; file transcription now uses the recommended Whisper model"
            );
            self.file_transcription_model = FileModelPreference::Auto;
            changed = true;
        }
        if self.pianissimo_dictation {
            tracing::warn!("Pianissimo is not supported on this system; disabling the legacy Pianissimo dictation flag");
            self.pianissimo_dictation = false;
            changed = true;
        }
        changed
    }

    /// Change the default profile's language. A model the new language cannot
    /// use is reset to Auto; the returned notices describe every such reset.
    pub fn set_default_profile_language(
        &mut self,
        language: Language,
    ) -> Result<Vec<ModelResetNotice>, String> {
        let mut candidate = self.clone();
        let mut profiles = candidate.resolved_hotkey_profiles();
        let index = profiles
            .iter()
            .position(|profile| profile.id == "default")
            .unwrap_or(0);
        profiles[index].language = language;
        let notices = candidate.replace_hotkey_profiles(profiles)?;
        *self = candidate;
        Ok(notices)
    }

    /// Reset (to Auto) the model of every profile whose language differs from
    /// `previous` and whose explicit model cannot serve the new language.
    /// Other profiles are never touched. Returns one notice per reset.
    fn reset_models_incompatible_after_language_change(
        &mut self,
        previous: &[HotkeyProfile],
    ) -> Vec<ModelResetNotice> {
        let supported = crate::transcription::pianissimo_backend::runtime_supported_on_this_os();
        let mut notices = Vec::new();
        for profile in self.resolved_hotkey_profiles() {
            let changed = previous
                .iter()
                .any(|old| old.id == profile.id && old.language != profile.language);
            if !changed {
                continue;
            }
            let reason = match self.profile_models.get(&profile.id) {
                Some(FileModelPreference::Whisper(model))
                    if !model.is_compatible_with(profile.language) =>
                {
                    format!(
                        "{} does not support {}",
                        model.display_name(),
                        profile.language.display_name()
                    )
                }
                Some(FileModelPreference::PianissimoOriginal)
                    if profile.language != Language::Swedish =>
                {
                    "Pianissimo only supports Swedish".to_string()
                }
                _ => continue,
            };
            let recommended = match FileModel::recommended_for(profile.language, supported) {
                FileModel::Whisper(model) => model.display_name(),
                FileModel::PianissimoOriginal => "Pianissimo",
            };
            self.profile_models
                .insert(profile.id.clone(), FileModelPreference::Auto);
            notices.push(ModelResetNotice {
                profile_id: profile.id.clone(),
                profile_name: profile.name.clone(),
                message: format!(
                    "Model for profile '{}' changed to Recommended ({recommended}) because {reason}.",
                    profile.name
                ),
            });
        }
        notices
    }

    /// Resolve the selected profile's dictionary. Unmigrated in-memory legacy
    /// settings keep their old hint-only global behavior until first load.
    pub fn effective_glossary_source(&self, profile_id: Option<&str>) -> String {
        self.effective_glossary_source_with_prompt(profile_id, None)
    }

    /// Compose the glossary source for one transcription without mutating
    /// stored settings. A non-empty one-run prompt replaces the saved global
    /// hint source; the selected known, explicit-language profile remains the
    /// only source of deterministic alias replacements.
    pub fn effective_glossary_source_with_prompt(
        &self,
        profile_id: Option<&str>,
        prompt: Option<&str>,
    ) -> String {
        if self.profile_glossary_migrated {
            let id = profile_id
                .map(str::to_string)
                .or_else(|| Some(self.default_profile().id));
            let scoped = id
                .as_deref()
                .and_then(|id| self.profile_glossaries.get(id))
                .map(String::as_str)
                .unwrap_or("");
            let auto_language = id.as_deref().is_some_and(|id| {
                self.resolved_hotkey_profiles()
                    .iter()
                    .any(|profile| profile.id == id && profile.language == Language::Auto)
            });
            // Auto-detect cannot safely apply deterministic replacements in an
            // unknown language, but its dictionary hints remain available.
            let auto_hints =
                auto_language.then(|| Glossary::parse(scoped).decoder_prompt().unwrap_or_default());
            let scoped = auto_hints.as_deref().unwrap_or(scoped);
            let prompt_hints = prompt
                .filter(|source| !source.trim().is_empty())
                .and_then(|source| Glossary::parse(source).decoder_prompt());
            return match prompt_hints.as_deref() {
                Some(source) if scoped.trim().is_empty() => source.to_string(),
                Some(source) => format!("{source}\n{scoped}"),
                None => scoped.to_string(),
            };
        }
        let base_source = prompt
            .filter(|candidate| !candidate.trim().is_empty())
            .unwrap_or(self.initial_prompt.as_str());
        let global = Glossary::parse(base_source)
            .decoder_prompt()
            .unwrap_or_default();
        let scoped_profile_id = profile_id.filter(|requested_id| {
            self.resolved_hotkey_profiles()
                .iter()
                .any(|profile| profile.id == **requested_id && profile.language != Language::Auto)
        });
        let scoped = scoped_profile_id
            .and_then(|id| self.profile_glossaries.get(id))
            .map(String::as_str)
            .map(str::trim)
            .unwrap_or_default();

        match (global.trim().is_empty(), scoped.is_empty()) {
            (true, true) => String::new(),
            (false, true) => global,
            (true, false) => scoped.to_string(),
            (false, false) => format!("{}\n{scoped}", global.trim()),
        }
    }

    /// Legacy model helper for settings predating profile-owned model choices.
    pub fn effective_model(&self) -> WhisperModel {
        self.effective_model_for(self.language)
    }

    pub fn effective_model_for(&self, language: Language) -> WhisperModel {
        if self.auto_select_model || !self.whisper_model.is_compatible_with(language) {
            WhisperModel::recommended(language)
        } else {
            self.whisper_model
        }
    }

    /// Legacy pre-migration Pianissimo switch, consulted only when a profile
    /// has not yet received its own model preference.
    pub fn uses_pianissimo_for_dictation(&self, language: Language) -> bool {
        self.pianissimo_dictation && language == Language::Swedish
    }

    /// Resolve the model for file transcription without changing the live
    /// dictation model preference. An `Auto` preference prefers the
    /// recommended model when it is downloaded, else the best downloaded
    /// compatible model (see [`Self::effective_file_model_for_gated`]).
    pub fn effective_file_model_for(&self, language: Language) -> Result<FileModel, String> {
        self.effective_file_model_for_gated(
            language,
            crate::transcription::pianissimo_backend::runtime_supported_on_this_os(),
            &real_file_model_ready,
        )
    }

    /// [`Self::effective_file_model_for`] with the Pianissimo platform gate and
    /// the model-readiness probe supplied by the caller.
    ///
    /// `Auto`: the recommended model if `is_ready`; otherwise the first ready
    /// model in the language's app lineup order, then other compatible Whisper
    /// models from largest to smallest; if none is ready, the recommended model
    /// (so "not downloaded" errors name the recommended download).
    pub fn effective_file_model_for_gated(
        &self,
        language: Language,
        pianissimo_supported: bool,
        is_ready: &dyn Fn(FileModel) -> bool,
    ) -> Result<FileModel, String> {
        match self.file_transcription_model {
            FileModelPreference::Auto => Ok(auto_file_model(
                language,
                pianissimo_supported,
                true,
                is_ready,
            )),
            _ => self.explicit_file_model_for(language),
        }
    }

    fn explicit_file_model_for(&self, language: Language) -> Result<FileModel, String> {
        match self.file_transcription_model {
            FileModelPreference::Auto => unreachable!("Auto is resolved by the caller"),
            FileModelPreference::Whisper(model) if model.is_compatible_with(language) => {
                Ok(FileModel::Whisper(model))
            }
            FileModelPreference::Whisper(model) => Err(format!(
                "Whisper model '{}' is incompatible with {}",
                model.display_name(),
                language.display_name()
            )),
            FileModelPreference::PianissimoOriginal if language == Language::Swedish => {
                Ok(FileModel::PianissimoOriginal)
            }
            FileModelPreference::PianissimoOriginal => Err(
                "Pianissimo Original is only available for Swedish file transcription".to_string(),
            ),
        }
    }

    /// Like [`Self::effective_file_model_for`], but for work that needs a
    /// Whisper model (meetings, diarization, decoder hints, beam search): an
    /// `Auto` preference never resolves to Pianissimo. An explicit Pianissimo
    /// choice is returned as-is so callers can report it clearly.
    pub fn effective_whisper_file_model_for(
        &self,
        language: Language,
    ) -> Result<FileModel, String> {
        self.effective_whisper_file_model_for_gated(language, &real_file_model_ready)
    }

    /// [`Self::effective_whisper_file_model_for`] with the readiness probe supplied by the caller.
    pub fn effective_whisper_file_model_for_gated(
        &self,
        language: Language,
        is_ready: &dyn Fn(FileModel) -> bool,
    ) -> Result<FileModel, String> {
        match self.file_transcription_model {
            FileModelPreference::Auto => Ok(auto_file_model(language, false, false, is_ready)),
            _ => self.explicit_file_model_for(language),
        }
    }

    pub fn resolved_hotkey_profiles(&self) -> Vec<HotkeyProfile> {
        if self.hotkey_profiles.is_empty() {
            vec![HotkeyProfile::legacy_default(
                self.hotkey.clone(),
                self.language,
            )]
        } else {
            self.hotkey_profiles.clone()
        }
    }

    /// Return the profile bindings that should be registered at runtime.
    /// Explicit push-to-talk/toggle bindings replace the legacy binding for a
    /// profile.
    pub fn resolved_hotkey_bindings(&self) -> Vec<(HotkeyProfile, String, HotkeyMode)> {
        self.resolved_hotkey_profiles()
            .into_iter()
            .flat_map(|profile| {
                let mut bindings = Vec::with_capacity(2);
                if let Some(shortcut) = &profile.push_to_talk_shortcut {
                    bindings.push((profile.clone(), shortcut.clone(), HotkeyMode::PushToTalk));
                }
                if let Some(shortcut) = &profile.toggle_shortcut {
                    bindings.push((profile.clone(), shortcut.clone(), HotkeyMode::Toggle));
                }
                if bindings.is_empty() {
                    bindings.push((profile.clone(), profile.shortcut.clone(), self.hotkey_mode));
                }
                bindings
            })
            .collect()
    }

    /// Return all shortcuts that the active hotkey mode may register.
    pub fn resolved_shortcuts(&self) -> Vec<String> {
        self.resolved_hotkey_bindings()
            .into_iter()
            .map(|(_, shortcut, _)| shortcut)
            .collect()
    }

    pub fn validate_hotkey_profiles(profiles: &[HotkeyProfile]) -> Result<(), String> {
        if profiles.is_empty() {
            return Err("At least one hotkey profile is required".to_string());
        }
        let mut ids = HashSet::new();
        let mut shortcuts = HashSet::new();
        for profile in profiles {
            if profile.id.is_empty()
                || profile.id.len() > 32
                || !profile.id.starts_with(|c: char| c.is_ascii_alphanumeric())
                || !profile
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            {
                return Err(format!(
                    "Invalid profile id '{}': use lowercase letters, numbers, '-' or '_'",
                    profile.id
                ));
            }
            if !ids.insert(profile.id.clone()) {
                return Err(format!("Duplicate profile id '{}'", profile.id));
            }
            if profile.name.trim().is_empty() {
                return Err(format!("Profile '{}' must have a name", profile.id));
            }
            if profile.name.chars().count() > 40 {
                return Err(format!(
                    "Profile '{}' name must be 40 characters or fewer",
                    profile.id
                ));
            }
            validate_hotkey(&profile.shortcut)?;
            let has_explicit_bindings =
                profile.push_to_talk_shortcut.is_some() || profile.toggle_shortcut.is_some();
            let active_shortcuts = if has_explicit_bindings {
                profile
                    .push_to_talk_shortcut
                    .iter()
                    .chain(profile.toggle_shortcut.iter())
                    .map(String::as_str)
                    .collect::<Vec<_>>()
            } else {
                vec![profile.shortcut.as_str()]
            };
            for shortcut in active_shortcuts {
                if shortcut.trim().is_empty() {
                    return Err(format!("Profile '{}' has an empty hotkey", profile.id));
                }
                validate_hotkey(shortcut)?;
                let canonical = canonical_hotkey(shortcut)?;
                if !shortcuts.insert(canonical) {
                    return Err(format!("Duplicate hotkey '{}'", shortcut));
                }
            }
        }
        Ok(())
    }

    /// Validate the complete shortcut configuration.
    pub fn validate_shortcut_configuration(&self) -> Result<(), String> {
        let profiles = self.resolved_hotkey_profiles();
        Self::validate_hotkey_profiles(&profiles)?;
        let shortcuts = self
            .resolved_hotkey_bindings()
            .into_iter()
            .map(|(_, shortcut, _)| canonical_hotkey(&shortcut))
            .collect::<Result<HashSet<_>, _>>()?;
        if shortcuts.len() != self.resolved_hotkey_bindings().len() {
            return Err("Duplicate hotkey bindings".to_string());
        }
        Ok(())
    }

    pub fn replace_hotkey_mode(&mut self, hotkey_mode: HotkeyMode) -> Result<(), String> {
        let mut candidate = self.clone();
        candidate.hotkey_mode = hotkey_mode;
        candidate.validate_shortcut_configuration()?;
        self.hotkey_mode = hotkey_mode;
        Ok(())
    }

    /// Replace the profile list. When a profile's language changes and its explicit
    /// model no longer fits, that profile's model is reset to Auto and a notice
    /// is returned instead of an error.
    pub fn replace_hotkey_profiles(
        &mut self,
        profiles: Vec<HotkeyProfile>,
    ) -> Result<Vec<ModelResetNotice>, String> {
        let mut profiles = profiles;
        for profile in &mut profiles {
            if let Some(shortcut) = profile
                .push_to_talk_shortcut
                .as_ref()
                .or(profile.toggle_shortcut.as_ref())
            {
                profile.shortcut = shortcut.clone();
            }
        }
        Self::validate_hotkey_profiles(&profiles)?;
        let current_profiles = self.resolved_hotkey_profiles();
        if let Some(profile) = profiles.iter().find(|profile| {
            current_profiles.iter().any(|current| {
                current.id == profile.id
                    && current.language != profile.language
                    && self
                        .profile_glossaries
                        .get(&profile.id)
                        .is_some_and(|source| {
                            Glossary::parse(source)
                                .entries()
                                .iter()
                                .any(|entry| !entry.aliases.is_empty())
                        })
            })
        }) {
            return Err(format!(
                "Profile '{}' has a personal dictionary; clear it before changing the profile language",
                profile.id
            ));
        }
        let current_ids: HashSet<&str> = current_profiles
            .iter()
            .map(|profile| profile.id.as_str())
            .collect();
        if let Some(profile) = profiles.iter().find(|profile| {
            !current_ids.contains(profile.id.as_str())
                && self
                    .profile_glossaries
                    .get(&profile.id)
                    .is_some_and(|source| !source.trim().is_empty())
        }) {
            return Err(format!(
                "Profile id '{}' has an inactive personal dictionary; choose a new id to avoid reactivating old aliases",
                profile.id
            ));
        }
        let legacy = profiles
            .iter()
            .find(|profile| profile.id == "default")
            .unwrap_or(&profiles[0]);
        let mut candidate = self.clone();
        candidate.hotkey = legacy.shortcut.clone();
        candidate.language = legacy.language;
        candidate.hotkey_profiles = profiles;
        let active_ids: HashSet<&str> = candidate
            .hotkey_profiles
            .iter()
            .map(|profile| profile.id.as_str())
            .collect();
        candidate
            .profile_models
            .retain(|id, _| active_ids.contains(id.as_str()));
        for profile in &candidate.hotkey_profiles {
            candidate
                .profile_models
                .entry(profile.id.clone())
                .or_insert(FileModelPreference::Auto);
        }
        let notices = candidate.reset_models_incompatible_after_language_change(&current_profiles);
        for profile in &candidate.hotkey_profiles {
            candidate.dictation_model_for_profile(&profile.id)?;
        }
        candidate.validate_shortcut_configuration()?;
        self.hotkey = candidate.hotkey;
        self.language = candidate.language;
        self.hotkey_profiles = candidate.hotkey_profiles;
        self.profile_models = candidate.profile_models;
        Ok(notices)
    }

    pub fn hotkey_profile_for_shortcut(&self, shortcut: &str) -> Option<HotkeyProfile> {
        let target = canonical_hotkey(shortcut).ok()?;
        self.resolved_hotkey_bindings()
            .into_iter()
            .find(|(_, binding, _)| {
                canonical_hotkey(binding).ok().as_deref() == Some(target.as_str())
            })
            .map(|(profile, _, _)| profile)
    }

    pub fn set_legacy_language(&mut self, language: Language) -> Result<(), String> {
        let current_legacy_language = if self.hotkey_profiles.is_empty() {
            Some(self.language)
        } else {
            self.hotkey_profiles
                .iter()
                .find(|profile| profile.id == "default")
                .map(|profile| profile.language)
        };
        if current_legacy_language.is_some_and(|current| current != language)
            && self
                .profile_glossaries
                .get("default")
                .is_some_and(|source| {
                    Glossary::parse(source)
                        .entries()
                        .iter()
                        .any(|entry| !entry.aliases.is_empty())
                })
        {
            return Err(
                "Profile 'default' has a personal dictionary; clear it before changing the profile language"
                    .to_string(),
            );
        }
        self.language = language;
        if let Some(profile) = self
            .hotkey_profiles
            .iter_mut()
            .find(|profile| profile.id == "default")
        {
            profile.language = language;
        }
        Ok(())
    }

    /// Try to update the legacy/default profile shortcut atomically.
    pub fn try_set_legacy_hotkey(&mut self, shortcut: String) -> Result<(), String> {
        let mut candidate = self.clone();
        candidate.hotkey = shortcut.clone();
        if let Some(profile) = candidate
            .hotkey_profiles
            .iter_mut()
            .find(|profile| profile.id == "default")
        {
            profile.set_primary_shortcut(shortcut);
        }
        candidate.validate_shortcut_configuration()?;
        self.hotkey = candidate.hotkey;
        self.hotkey_profiles = candidate.hotkey_profiles;
        Ok(())
    }

    /// Checked legacy/default profile shortcut update. Invalid or colliding
    /// updates leave the complete settings value unchanged.
    pub fn set_legacy_hotkey(&mut self, shortcut: String) -> Result<(), String> {
        self.try_set_legacy_hotkey(shortcut)
    }

    /// Build the ordered set of profile models worth loading during GUI
    /// startup. Pianissimo profiles are excluded. The primary remaining Whisper
    /// profile is always first and included. Additional distinct models must fit both
    /// the resident-entry limit and advertised model-size budget.
    pub fn warm_model_plan(
        &self,
        max_models: usize,
        max_total_mb: u32,
    ) -> Vec<(WhisperModel, Language)> {
        self.warm_model_plan_gated(
            max_models,
            max_total_mb,
            crate::transcription::pianissimo_backend::runtime_supported_on_this_os(),
        )
    }

    /// [`Self::warm_model_plan`] with the Pianissimo platform gate supplied by the caller.
    pub fn warm_model_plan_gated(
        &self,
        max_models: usize,
        max_total_mb: u32,
        pianissimo_supported: bool,
    ) -> Vec<(WhisperModel, Language)> {
        let profiles: Vec<_> = self
            .resolved_hotkey_profiles()
            .into_iter()
            .filter(|profile| {
                matches!(
                    self.dictation_model_for_profile_gated(&profile.id, pianissimo_supported),
                    Ok(FileModel::Whisper(_))
                )
            })
            .collect();
        let Some(primary_index) = profiles
            .iter()
            .position(|profile| profile.id == "default")
            .or_else(|| (!profiles.is_empty()).then_some(0))
        else {
            return Vec::new();
        };

        let capacity = max_models.max(1);
        let ordered_indices = std::iter::once(primary_index)
            .chain((0..profiles.len()).filter(|index| *index != primary_index));
        let mut plan = Vec::with_capacity(capacity.min(profiles.len()));
        let mut total_mb = 0u32;

        for index in ordered_indices {
            let profile = &profiles[index];
            let Ok(FileModel::Whisper(model)) =
                self.dictation_model_for_profile_gated(&profile.id, pianissimo_supported)
            else {
                continue;
            };
            if plan.iter().any(|(resident, _)| *resident == model) {
                continue;
            }

            let is_primary = plan.is_empty();
            let next_total = total_mb.saturating_add(model.size_mb());
            if !is_primary && (plan.len() >= capacity || next_total > max_total_mb) {
                continue;
            }

            plan.push((model, profile.language));
            total_mb = next_total;
        }

        plan
    }
}

#[cfg(test)]
#[path = "manager_tests.rs"]
mod tests;
