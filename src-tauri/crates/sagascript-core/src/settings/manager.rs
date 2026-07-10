use serde::{Deserialize, Serialize};

use crate::download::DownloadIntegrity;

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
            WhisperModel::LargeV3TurboQ8 => "OpenAI Whisper large-v3-turbo, q8_0 quantised. High accuracy, multilingual, 834 MB",
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
    /// DTW model preset for accurate attention-based token timestamps.
    /// KB-Whisper and NB-Whisper are fine-tunes of the corresponding base architecture,
    /// so they use the same alignment heads.
    #[cfg(feature = "diarization")]
    pub fn dtw_preset(&self) -> whisper_rs::DtwModelPreset {
        use whisper_rs::DtwModelPreset;
        match self {
            WhisperModel::TinyEn => DtwModelPreset::TinyEn,
            WhisperModel::Tiny | WhisperModel::KbWhisperTiny | WhisperModel::NbWhisperTiny => DtwModelPreset::Tiny,
            WhisperModel::BaseEn => DtwModelPreset::BaseEn,
            WhisperModel::Base | WhisperModel::KbWhisperBase | WhisperModel::NbWhisperBase => DtwModelPreset::Base,
            WhisperModel::SmallEn => DtwModelPreset::SmallEn,
            WhisperModel::Small | WhisperModel::KbWhisperSmall | WhisperModel::NbWhisperSmall => DtwModelPreset::Small,
            WhisperModel::MediumEn => DtwModelPreset::MediumEn,
            WhisperModel::Medium | WhisperModel::KbWhisperMedium | WhisperModel::NbWhisperMedium => DtwModelPreset::Medium,
            WhisperModel::LargeV3Turbo | WhisperModel::LargeV3TurboQ8 => DtwModelPreset::LargeV3Turbo,
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
            WhisperModel::LargeV3TurboQ8 => "ggml-large-v3-turbo-q8_0.bin",
        }
    }

    /// HuggingFace download URL for model
    pub fn download_url(&self) -> &'static str {
        match self {
            WhisperModel::TinyEn => "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-tiny.en.bin",
            WhisperModel::Tiny => "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-tiny.bin",
            WhisperModel::BaseEn => "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.en.bin",
            WhisperModel::Base => "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.bin",
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
            WhisperModel::TinyEn => DownloadIntegrity { sha256: "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f", size: 77_704_715 },
            WhisperModel::Tiny => DownloadIntegrity { sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21", size: 77_691_713 },
            WhisperModel::BaseEn => DownloadIntegrity { sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002", size: 147_964_211 },
            WhisperModel::Base => DownloadIntegrity { sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe", size: 147_951_465 },
            WhisperModel::KbWhisperTiny => DownloadIntegrity { sha256: "98d46b7d23e5528d006e8a42e29eb0cb39b44bed94e1329f10f57d1fd15c658b", size: 29_875_738 },
            WhisperModel::KbWhisperBase => DownloadIntegrity { sha256: "aead29b356bca8840e72a8dc2286e2d69e6702639751a1e60cb3c8eacefec546", size: 55_295_450 },
            WhisperModel::KbWhisperSmall => DownloadIntegrity { sha256: "6768836a51abc902e420c613153e6d418c90ea2774e913274d02ab23170225b7", size: 175_209_680 },
            WhisperModel::KbWhisperMedium => DownloadIntegrity { sha256: "7f8762e0ade9e0073674c0d5acae942a0b1ea98add9baa008ee89c94eaba43d0", size: 539_212_484 },
            WhisperModel::KbWhisperLarge => DownloadIntegrity { sha256: "6d2863812d7410322bb7d8647a5c7260761300fa946714c9ed66d22bb30bcb19", size: 1_081_140_203 },
            WhisperModel::NbWhisperTiny => DownloadIntegrity { sha256: "e5fb42192cdf31bea624a524d035e8895030b2bb4b31d4ea2a1ebf0ea8f57237", size: 29_875_738 },
            WhisperModel::NbWhisperBase => DownloadIntegrity { sha256: "dcb9f3ab963cd288974c826c1519ff73b78b2372e80d388a6ce94f29c6a5b40f", size: 55_295_450 },
            WhisperModel::NbWhisperSmall => DownloadIntegrity { sha256: "2a9025afb6e825fc4ae6a46671e0cb2f43e62f1dec87270deea6fe61b5285a20", size: 175_209_680 },
            WhisperModel::NbWhisperMedium => DownloadIntegrity { sha256: "18733de634af639a43b0f8c5f5a2ea0920de4c5b32a5570ec130981581c0e5e7", size: 539_212_484 },
            WhisperModel::NbWhisperLarge => DownloadIntegrity { sha256: "feb5951ae694a62cfeb81fb501f6cfa8cc50d96bcddb1e4e8215f7006bac23a2", size: 1_081_140_203 },
            WhisperModel::SmallEn => DownloadIntegrity { sha256: "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d", size: 487_614_201 },
            WhisperModel::Small => DownloadIntegrity { sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b", size: 487_601_967 },
            WhisperModel::MediumEn => DownloadIntegrity { sha256: "cc37e93478338ec7700281a7ac30a10128929eb8f427dda2e865faa8f6da4356", size: 1_533_774_781 },
            WhisperModel::Medium => DownloadIntegrity { sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208", size: 1_533_763_059 },
            WhisperModel::LargeV3Turbo => DownloadIntegrity { sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69", size: 1_624_555_275 },
            WhisperModel::LargeV3TurboQ8 => DownloadIntegrity { sha256: "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1", size: 874_188_075 },
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
            WhisperModel::TinyEn => Some(DownloadIntegrity { sha256: "82b32eef73c94bb0c432a776a047b757d9525c26d84038a15d8798d7c8d1ee58", size: 15_034_655 }),
            WhisperModel::Tiny => Some(DownloadIntegrity { sha256: "c88cbd2648e1f5415092bcf5256add463a0f19943e6938f46e8d4ffdebd47739", size: 15_037_446 }),
            WhisperModel::BaseEn => Some(DownloadIntegrity { sha256: "8cf860309e2449e2bdc8be834cf838ab2565747ecc8c0ef914ef5975115e192b", size: 37_950_917 }),
            WhisperModel::Base => Some(DownloadIntegrity { sha256: "7e6ab77041942572f239b5b602f8aaa1c3ed29d73e3d8f20abea03a773541089", size: 37_922_638 }),
            WhisperModel::SmallEn => Some(DownloadIntegrity { sha256: "b2ef1c506378b825b4b4341979a93e1656b5d6c129f17114cfb8fb78aabc2f89", size: 162_952_446 }),
            WhisperModel::Small => Some(DownloadIntegrity { sha256: "de43fb9fed471e95c19e60ae67575c2bf09e8fb607016da171b06ddad313988b", size: 163_083_239 }),
            WhisperModel::MediumEn => Some(DownloadIntegrity { sha256: "cdc44fee3c62b5743913e3147ed75f4e8ecfb52dd7a0f0f7387094b406ff0ee6", size: 566_993_085 }),
            WhisperModel::Medium => Some(DownloadIntegrity { sha256: "79b0b8d436d47d3f24dd3afc91f19447dd686a4f37521b2f6d9c30a642133fbd", size: 567_829_413 }),
            WhisperModel::LargeV3Turbo | WhisperModel::LargeV3TurboQ8 => Some(DownloadIntegrity { sha256: "84bedfe895bd7b5de6e8e89a0803dfc5addf8c0c5bc4c937451716bf7cf7988a", size: 1_173_393_014 }),
            WhisperModel::KbWhisperTiny
            | WhisperModel::KbWhisperBase
            | WhisperModel::KbWhisperSmall
            | WhisperModel::KbWhisperMedium
            | WhisperModel::KbWhisperLarge
            | WhisperModel::NbWhisperTiny
            | WhisperModel::NbWhisperBase
            | WhisperModel::NbWhisperSmall
            | WhisperModel::NbWhisperMedium
            | WhisperModel::NbWhisperLarge => None,
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
                WhisperModel::LargeV3TurboQ8,
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
    /// Optional initial prompt that primes the decoder with domain vocabulary
    /// (names, jargon, spellings) for more accurate transcription. Empty = none.
    pub initial_prompt: String,
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
            initial_prompt: String::new(),
            beam_size: 0,
            temperature_fallback: true,
            vad_enabled: false,
            has_completed_onboarding: false,
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

    #[cfg(target_os = "macos")]
    #[test]
    fn coreml_encoder_derivation() {
        // OpenAI models: URL + on-disk dir name must match whisper.cpp's
        // `.bin` → `-encoder.mlmodelc` derivation (verified against the
        // ggerganov/whisper.cpp HF repo).
        assert_eq!(
            WhisperModel::Base.coreml_encoder_url().as_deref(),
            Some("https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base-encoder.mlmodelc.zip")
        );
        assert_eq!(
            WhisperModel::Base.coreml_encoder_dirname().as_deref(),
            Some("ggml-base-encoder.mlmodelc")
        );
        // `.en` is part of the name, not a quant suffix — must be preserved.
        assert_eq!(
            WhisperModel::BaseEn.coreml_encoder_dirname().as_deref(),
            Some("ggml-base.en-encoder.mlmodelc")
        );
        assert_eq!(
            WhisperModel::LargeV3Turbo.coreml_encoder_dirname().as_deref(),
            Some("ggml-large-v3-turbo-encoder.mlmodelc")
        );
        // Quantised turbo strips the `-q8_0` suffix and reuses the SAME FP16
        // CoreML encoder as the f16 turbo (matches whisper.cpp's derivation).
        assert_eq!(
            WhisperModel::LargeV3TurboQ8.coreml_encoder_dirname().as_deref(),
            Some("ggml-large-v3-turbo-encoder.mlmodelc")
        );
        assert_eq!(
            WhisperModel::LargeV3TurboQ8.coreml_encoder_url().as_deref(),
            Some("https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo-encoder.mlmodelc.zip")
        );
        // KB/NB fine-tunes live in other repos and have no CoreML encoder.
        assert_eq!(WhisperModel::KbWhisperBase.coreml_encoder_url(), None);
        assert_eq!(WhisperModel::NbWhisperSmall.coreml_encoder_dirname(), None);
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
            WhisperModel::LargeV3TurboQ8,
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
            WhisperModel::LargeV3TurboQ8,
        ];
        for m in models {
            let url = m.download_url();
            assert!(url.starts_with("https://huggingface.co/"), "{:?}: {}", m, url);
            assert!(url.contains(".bin"), "{:?}: {}", m, url);
            assert!(!url.contains("/resolve/main/"), "mutable URL for {m:?}: {url}");
            let integrity = m.download_integrity();
            assert_eq!(integrity.sha256.len(), 64, "invalid SHA-256 for {m:?}");
            assert!(integrity.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(
                integrity.size >= 20 * 1024 * 1024,
                "implausibly small Whisper artifact for {m:?}: {} bytes",
                integrity.size
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn all_coreml_archives_are_immutable_and_have_exact_metadata() {
        let models = [
            WhisperModel::TinyEn,
            WhisperModel::Tiny,
            WhisperModel::BaseEn,
            WhisperModel::Base,
            WhisperModel::SmallEn,
            WhisperModel::Small,
            WhisperModel::MediumEn,
            WhisperModel::Medium,
            WhisperModel::LargeV3Turbo,
            WhisperModel::LargeV3TurboQ8,
        ];
        for model in models {
            let url = model.coreml_encoder_url().unwrap();
            assert!(!url.contains("/resolve/main/"), "mutable URL: {url}");
            let integrity = model.coreml_encoder_integrity().unwrap();
            assert_eq!(integrity.sha256.len(), 64);
            assert!(integrity.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(
                integrity.size >= 10 * 1024 * 1024,
                "implausibly small CoreML archive for {model:?}"
            );
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
            WhisperModel::LargeV3TurboQ8,
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
        assert_eq!(auto.len(), 6);
        assert!(auto.contains(&WhisperModel::Tiny));
        assert!(auto.contains(&WhisperModel::Base));
        assert!(auto.contains(&WhisperModel::Small));
        assert!(auto.contains(&WhisperModel::Medium));
        assert!(auto.contains(&WhisperModel::LargeV3Turbo));
        assert!(auto.contains(&WhisperModel::LargeV3TurboQ8));
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
            (WhisperModel::LargeV3TurboQ8, "\"large-v3-turbo-q8_0\""),
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
        assert_eq!(s.initial_prompt, "");
        assert_eq!(s.beam_size, 0);
        assert!(s.temperature_fallback);
        assert!(!s.vad_enabled);
    }

    #[test]
    fn settings_effective_model_with_auto_select() {
        let mut s = Settings { auto_select_model: true, ..Default::default() };

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
        let s = Settings { auto_select_model: false, whisper_model: WhisperModel::KbWhisperSmall, language: Language::English, ..Default::default() }; // language shouldn't matter

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
        assert_eq!(deserialized.initial_prompt, original.initial_prompt);
        assert_eq!(deserialized.beam_size, original.beam_size);
        assert_eq!(deserialized.temperature_fallback, original.temperature_fallback);
        assert_eq!(deserialized.vad_enabled, original.vad_enabled);
        assert_eq!(
            deserialized.has_completed_onboarding,
            original.has_completed_onboarding
        );
    }

    #[test]
    fn settings_accepts_legacy_camel_case_onboarding_key() {
        let settings: Settings =
            serde_json::from_str(r#"{"hasCompletedOnboarding":true}"#).unwrap();
        assert!(settings.has_completed_onboarding);
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
    fn no_speech_threshold_small_en_fully_disabled() {
        assert_eq!(WhisperModel::SmallEn.no_speech_threshold(), 0.0);
    }

    #[test]
    fn no_speech_threshold_other_models_at_0_3() {
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
            WhisperModel::Small,
            WhisperModel::MediumEn,
            WhisperModel::Medium,
            WhisperModel::LargeV3Turbo,
            WhisperModel::LargeV3TurboQ8,
        ];
        for m in models {
            assert_eq!(
                m.no_speech_threshold(),
                0.3,
                "{:?} should have threshold 0.3",
                m
            );
        }
    }

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
