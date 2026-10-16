use crate::download::DownloadIntegrity;

use super::WhisperModel;

#[cfg(target_os = "macos")]
const WHISPER_CPP_REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";

impl WhisperModel {
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
}
