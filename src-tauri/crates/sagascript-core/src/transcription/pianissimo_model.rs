//! Download metadata and lifecycle helpers for KlangAI's original Pianissimo
//! Swedish NeMo checkpoint.
//!
//! The checkpoint is distributed by Klang AI AB under the Creative Commons
//! Attribution 4.0 International license (CC BY 4.0):
//! <https://creativecommons.org/licenses/by/4.0/>.
//! Source model card and attribution: <https://huggingface.co/KlangAI/pianissimo-sv>.
//!
//! This module deliberately manages only the original `.nemo` checkpoint. It
//! does not convert the artifact or apply GGML-specific validation.

use std::path::PathBuf;

use tracing::info;

use crate::download::{
    download_to_path, prepare_existing_artifact, verify_file, DownloadIntegrity, ExistingArtifact,
};
use crate::error::DictationError;
use crate::transcription::model::models_dir;

/// Pinned Hugging Face revision containing the original checkpoint.
pub const PIANISSIMO_REVISION: &str = "8f1f6d8f8bd7482a5ea1d2bfaf6ef5be61597138";

/// Filename used for the original NeMo checkpoint in the app model directory.
pub const PIANISSIMO_FILENAME: &str = "pianissimo-sv.nemo";

/// Fully pinned URL for the original `.nemo` checkpoint.
pub const PIANISSIMO_URL: &str = "https://huggingface.co/KlangAI/pianissimo-sv/resolve/8f1f6d8f8bd7482a5ea1d2bfaf6ef5be61597138/pianissimo-sv.nemo";

/// Immutable integrity manifest for the original checkpoint.
pub const PIANISSIMO_INTEGRITY: DownloadIntegrity = DownloadIntegrity {
    sha256: "ca340b827dc9e18d2019341fa7b6dc163f00284d84066ce2cbfffcaa129920cd",
    size: 2_509_322_240,
};

/// Full path to the original Pianissimo checkpoint in the shared model cache.
pub fn path() -> PathBuf {
    models_dir().join(PIANISSIMO_FILENAME)
}

/// Alias for callers that prefer an explicit model name.
pub fn model_path() -> PathBuf {
    path()
}

/// Return whether the checkpoint exists and passes its exact size and SHA-256
/// integrity checks. Corrupt or truncated files are never reported as ready.
pub fn is_downloaded() -> bool {
    verify_file(&path(), PIANISSIMO_INTEGRITY).is_ok()
}

/// Download the original checkpoint into the shared model cache.
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

    // NeMo checkpoints are not GGML files, so skip the optional magic check.
    download_to_path(
        PIANISSIMO_URL,
        &destination,
        "nemo",
        PIANISSIMO_INTEGRITY,
        None,
        progress_callback,
    )
    .await?;

    info!("Pianissimo model downloaded: {}", destination.display());
    Ok(destination)
}

/// Remove the cached checkpoint. It is safe to call when no checkpoint exists.
pub fn delete() -> Result<(), DictationError> {
    match std::fs::remove_file(path()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DictationError::ModelDownloadFailed(format!(
            "Failed to delete Pianissimo model: {error}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::download::{prepare_existing_artifact, ExistingArtifact};

    #[test]
    fn manifest_is_pinned_to_original_checkpoint() {
        assert_eq!(PIANISSIMO_FILENAME, "pianissimo-sv.nemo");
        assert_eq!(
            PIANISSIMO_REVISION,
            "8f1f6d8f8bd7482a5ea1d2bfaf6ef5be61597138"
        );
        assert_eq!(
            PIANISSIMO_URL,
            format!(
                "https://huggingface.co/KlangAI/pianissimo-sv/resolve/{PIANISSIMO_REVISION}/{PIANISSIMO_FILENAME}"
            )
        );
        assert_eq!(PIANISSIMO_INTEGRITY.size, 2_509_322_240);
        assert_eq!(
            PIANISSIMO_INTEGRITY.sha256,
            "ca340b827dc9e18d2019341fa7b6dc163f00284d84066ce2cbfffcaa129920cd"
        );
    }

    #[test]
    fn path_uses_shared_models_directory_and_nemo_filename() {
        assert!(path().ends_with(PIANISSIMO_FILENAME));
        assert_eq!(model_path(), path());
    }

    #[test]
    fn invalid_existing_checkpoint_is_removed_before_redownload() {
        let directory = std::env::temp_dir().join(format!(
            "sagascript-pianissimo-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&directory).unwrap();
        let destination = directory.join(PIANISSIMO_FILENAME);
        fs::write(&destination, b"not a NeMo checkpoint").unwrap();

        let result = prepare_existing_artifact(&destination, PIANISSIMO_INTEGRITY).unwrap();

        assert_eq!(result, ExistingArtifact::RemovedInvalid);
        assert!(!destination.exists());
        fs::remove_dir(directory).unwrap();
    }
}
