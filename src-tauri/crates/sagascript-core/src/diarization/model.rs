use std::path::PathBuf;

use tracing::info;

use crate::download::{
    DownloadIntegrity, ExistingArtifact, download_to_path, prepare_existing_artifact,
};
use crate::error::DictationError;

/// ONNX models used for speaker diarization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiarizationModel {
    /// Pyannote segmentation 3.0 — speaker activity detection (~6 MB)
    PyannoteSegmentation3,
    /// WeSpeaker ResNet34-LM — speaker embedding extraction (~27 MB)
    WeSpeakerResNet34LM,
}

impl DiarizationModel {
    /// All diarization models (both are required for the pipeline).
    pub const ALL: &[DiarizationModel] = &[
        DiarizationModel::PyannoteSegmentation3,
        DiarizationModel::WeSpeakerResNet34LM,
    ];

    /// ONNX filename stored on disk.
    pub fn filename(&self) -> &'static str {
        match self {
            Self::PyannoteSegmentation3 => "pyannote-segmentation-3.0.onnx",
            Self::WeSpeakerResNet34LM => "wespeaker-resnet34-lm.onnx",
        }
    }

    /// HuggingFace download URL for the ONNX model.
    pub fn download_url(&self) -> &'static str {
        match self {
            Self::PyannoteSegmentation3 => {
                "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/9403a6902bb58e3d5ae8c7e77c3422de279db2e0/model.onnx"
            }
            Self::WeSpeakerResNet34LM => {
                "https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34-LM/resolve/f0c48c298fd835726c27956a5d617bad7115627e/voxceleb_resnet34_LM.onnx"
            }
        }
    }

    /// Exact git-LFS metadata for the pinned ONNX artifact.
    pub fn download_integrity(&self) -> DownloadIntegrity {
        match self {
            Self::PyannoteSegmentation3 => DownloadIntegrity {
                sha256: "220ad67ca923bef2fa91f2390c786097bf305bceb5e261d4af67b38e938e1079",
                size: 5_992_913,
            },
            Self::WeSpeakerResNet34LM => DownloadIntegrity {
                sha256: "7bb2f06e9df17cdf1ef14ee8a15ab08ed28e8d0ef5054ee135741560df2ec068",
                size: 26_530_309,
            },
        }
    }

    /// Approximate model size in MB.
    pub fn size_mb(&self) -> u32 {
        match self {
            Self::PyannoteSegmentation3 => 6,
            Self::WeSpeakerResNet34LM => 27,
        }
    }

    /// Human-readable display name.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::PyannoteSegmentation3 => "Pyannote Segmentation 3.0",
            Self::WeSpeakerResNet34LM => "WeSpeaker ResNet34-LM",
        }
    }

    /// CLI model ID used in `download-model` and `list-models`.
    pub fn model_id(&self) -> &'static str {
        match self {
            Self::PyannoteSegmentation3 => "pyannote-segmentation",
            Self::WeSpeakerResNet34LM => "wespeaker-embedding",
        }
    }

    /// Parse a CLI model ID string into a DiarizationModel.
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "pyannote-segmentation" => Some(Self::PyannoteSegmentation3),
            "wespeaker-embedding" => Some(Self::WeSpeakerResNet34LM),
            "diarization" => None, // special case handled by caller (downloads both)
            _ => None,
        }
    }

    /// Check if the given string is the special "diarization" meta-ID
    /// that means "download all diarization models".
    pub fn is_meta_id(id: &str) -> bool {
        id == "diarization"
    }
}

/// Get the full path to a diarization model's ONNX file.
pub fn model_path(model: DiarizationModel) -> PathBuf {
    crate::transcription::model::models_dir().join(model.filename())
}

/// Check if a diarization model is already downloaded.
pub fn is_model_downloaded(model: DiarizationModel) -> bool {
    std::fs::metadata(model_path(model))
        .is_ok_and(|metadata| metadata.len() == model.download_integrity().size)
}

/// Check if all diarization models are downloaded.
pub fn all_models_downloaded() -> bool {
    DiarizationModel::ALL.iter().all(|m| is_model_downloaded(*m))
}

/// Download a diarization model from HuggingFace.
pub async fn download_model(
    model: DiarizationModel,
    progress_callback: impl Fn(u64, u64) + Send + 'static,
) -> Result<PathBuf, DictationError> {
    let path = model_path(model);

    if prepare_existing_artifact(&path, model.download_integrity())? == ExistingArtifact::Verified {
        info!(
            "Diarization model {} already exists at {}",
            model.display_name(),
            path.display()
        );
        return Ok(path);
    }

    info!(
        "Downloading {} from {} (~{}MB)",
        model.display_name(),
        model.download_url(),
        model.size_mb()
    );

    // ONNX files are bare protobuf with no fixed leading bytes (the first
    // bytes vary with ir_version and the producer-name string length), so no
    // magic check is applied here — only the Content-Length check. A magic
    // check risks false-rejecting a valid model, which the hardening this is
    // part of explicitly must never do.
    download_to_path(
        model.download_url(),
        &path,
        "onnx",
        model.download_integrity(),
        None,
        progress_callback,
    )
    .await?;

    info!("Diarization model downloaded: {}", path.display());
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_models_have_unique_filenames() {
        let mut filenames = std::collections::HashSet::new();
        for m in DiarizationModel::ALL {
            assert!(
                filenames.insert(m.filename()),
                "duplicate filename: {}",
                m.filename()
            );
        }
    }

    #[test]
    fn all_models_have_unique_ids() {
        let mut ids = std::collections::HashSet::new();
        for m in DiarizationModel::ALL {
            assert!(ids.insert(m.model_id()), "duplicate ID: {}", m.model_id());
        }
    }

    #[test]
    fn model_id_roundtrip() {
        for m in DiarizationModel::ALL {
            let parsed = DiarizationModel::from_id(m.model_id());
            assert_eq!(parsed, Some(*m), "roundtrip failed for {}", m.model_id());
        }
    }

    #[test]
    fn from_id_unknown_returns_none() {
        assert_eq!(DiarizationModel::from_id("nonexistent"), None);
    }

    #[test]
    fn meta_id_check() {
        assert!(DiarizationModel::is_meta_id("diarization"));
        assert!(!DiarizationModel::is_meta_id("pyannote-segmentation"));
        assert!(!DiarizationModel::is_meta_id(""));
    }

    #[test]
    fn filenames_end_with_onnx() {
        for m in DiarizationModel::ALL {
            assert!(
                m.filename().ends_with(".onnx"),
                "{} filename should end with .onnx",
                m.display_name()
            );
        }
    }

    #[test]
    fn download_urls_are_https() {
        for m in DiarizationModel::ALL {
            assert!(
                m.download_url().starts_with("https://"),
                "{} URL should be HTTPS",
                m.display_name()
            );
            assert!(
                !m.download_url().contains("/resolve/main/"),
                "{} URL must pin an immutable revision",
                m.display_name()
            );
            let integrity = m.download_integrity();
            assert_eq!(integrity.sha256.len(), 64);
            assert!(integrity.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(
                integrity.size >= 5 * 1024 * 1024,
                "{} artifact is implausibly small",
                m.display_name()
            );
        }
    }

    #[test]
    fn size_mb_is_positive() {
        for m in DiarizationModel::ALL {
            assert!(m.size_mb() > 0, "{} size should be > 0", m.display_name());
        }
    }

    #[test]
    fn model_path_uses_models_dir() {
        for m in DiarizationModel::ALL {
            let path = model_path(*m);
            assert!(
                path.ends_with(m.filename()),
                "path should end with filename"
            );
        }
    }
}
