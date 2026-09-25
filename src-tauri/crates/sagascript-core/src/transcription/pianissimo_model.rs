//! Download metadata and lifecycle helpers for the corrected Q8 conversion of
//! KlangAI's Swedish Pianissimo checkpoint.
//!
//! The checkpoint is distributed by Klang AI AB under the Creative Commons
//! Attribution 4.0 International license (CC BY 4.0):
//! <https://creativecommons.org/licenses/by/4.0/>.
//! Source model card and attribution: <https://huggingface.co/KlangAI/pianissimo-sv>.
//!
//! Conversion and its fidelity checks are documented in docs/research.

use std::path::{Path, PathBuf};

use tracing::info;

use crate::download::{
    download_to_path, prepare_existing_artifact, verify_file, DownloadIntegrity, ExistingArtifact,
};
use crate::error::DictationError;
use crate::transcription::model::models_dir;

/// Filename keeps the converted artifact separate from any earlier checkpoint.
pub const PIANISSIMO_FILENAME: &str = "pianissimo-sv-q8-melfix.gguf";
const LEGACY_PIANISSIMO_FILENAME: &str = "pianissimo-sv.nemo";

/// The converted model is an optional asset of the matching app release.
pub const PIANISSIMO_URL: &str = "https://github.com/Magnus-Gille/sagascript/releases/download/v1.3.2/pianissimo-sv-q8-melfix.gguf";

/// Immutable integrity manifest for the corrected conversion.
pub const PIANISSIMO_INTEGRITY: DownloadIntegrity = DownloadIntegrity {
    sha256: "56ed7a0199c2c6116b3254505296e36b8126275f5c2bce826613aeb1fca7b1b5",
    size: 714_456_704,
};

/// Full path to the converted Pianissimo model in the shared model cache.
pub fn path() -> PathBuf {
    models_dir().join(PIANISSIMO_FILENAME)
}

/// Alias for callers that prefer an explicit model name.
pub fn model_path() -> PathBuf {
    path()
}

/// Fast presence check for model listings. Full SHA-256 verification happens
/// before inference and whenever the download command is run.
pub fn is_downloaded() -> bool {
    std::fs::metadata(path()).is_ok_and(|metadata| metadata.len() == PIANISSIMO_INTEGRITY.size)
}

/// Verify the converted artifact before local inference.
pub fn verify_downloaded() -> Result<(), DictationError> {
    verify_file(&path(), PIANISSIMO_INTEGRITY)
}

/// Download the converted model into the shared model cache.
pub async fn download(
    progress_callback: impl Fn(u64, u64) + Send + 'static,
) -> Result<PathBuf, DictationError> {
    let destination = path();

    if prepare_existing_artifact(&destination, PIANISSIMO_INTEGRITY)? == ExistingArtifact::Verified
    {
        info!(
            "Pianissimo model already exists at {}",
            destination.display()
        );
        return Ok(destination);
    }

    info!("Downloading Pianissimo model from {PIANISSIMO_URL}");

    download_to_path(
        PIANISSIMO_URL,
        &destination,
        "gguf",
        PIANISSIMO_INTEGRITY,
        Some(b"GGUF"),
        progress_callback,
    )
    .await?;

    info!("Pianissimo model downloaded: {}", destination.display());
    Ok(destination)
}

fn remove_if_present(path: &Path) -> Result<(), DictationError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DictationError::ModelDownloadFailed(format!(
            "Failed to delete Pianissimo model at {}: {error}", path.display()
        ))),
    }
}

/// Remove both the current model and any original checkpoint on explicit
/// user-initiated deletion. Upgrade and download leave the old file intact.
pub fn delete() -> Result<(), DictationError> {
    remove_if_present(&path())?;
    remove_if_present(&models_dir().join(LEGACY_PIANISSIMO_FILENAME))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::download::{prepare_existing_artifact, ExistingArtifact};

    #[test]
    fn manifest_is_pinned_to_corrected_conversion() {
        assert_eq!(PIANISSIMO_FILENAME, "pianissimo-sv-q8-melfix.gguf");
        assert_eq!(PIANISSIMO_URL, "https://github.com/Magnus-Gille/sagascript/releases/download/v1.3.2/pianissimo-sv-q8-melfix.gguf");
        assert_eq!(PIANISSIMO_INTEGRITY.size, 714_456_704);
        assert_eq!(
            PIANISSIMO_INTEGRITY.sha256,
            "56ed7a0199c2c6116b3254505296e36b8126275f5c2bce826613aeb1fca7b1b5"
        );
    }

    #[test]
    fn path_uses_shared_models_directory_and_gguf_filename() {
        assert!(path().ends_with(PIANISSIMO_FILENAME));
        assert_eq!(model_path(), path());
    }

    #[test]
    fn invalid_existing_conversion_is_removed_before_redownload() {
        let directory = std::env::temp_dir().join(format!(
            "sagascript-pianissimo-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&directory).unwrap();
        let destination = directory.join(PIANISSIMO_FILENAME);
        fs::write(&destination, b"not a GGUF model").unwrap();

        let result = prepare_existing_artifact(&destination, PIANISSIMO_INTEGRITY).unwrap();

        assert_eq!(result, ExistingArtifact::RemovedInvalid);
        assert!(!destination.exists());
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn explicit_cleanup_removes_current_and_legacy_files() {
        let directory = std::env::temp_dir().join(format!(
            "sagascript-pianissimo-cleanup-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&directory).unwrap();
        for name in [PIANISSIMO_FILENAME, LEGACY_PIANISSIMO_FILENAME] {
            let file = directory.join(name);
            fs::write(&file, b"cached model").unwrap();
            remove_if_present(&file).unwrap();
            assert!(!file.exists());
            remove_if_present(&file).unwrap();
        }
        fs::remove_dir(directory).unwrap();
    }
}
