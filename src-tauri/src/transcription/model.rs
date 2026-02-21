use std::path::PathBuf;

use tracing::info;

use crate::error::DictationError;
use crate::settings::WhisperModel;

/// Get the models directory for storing GGML files
pub fn models_dir() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    let new_dir = base.join("Sagascript").join("Models");

    // Migrate from legacy FlowDictate models directory
    if !new_dir.exists() {
        let legacy_dir = base.join("FlowDictate").join("Models");
        if legacy_dir.exists() {
            info!("Migrating models directory from FlowDictate to Sagascript");
            // Create parent dir and rename
            if let Some(parent) = new_dir.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::rename(&legacy_dir, &new_dir).is_ok() {
                info!("Models directory migrated successfully");
                // Clean up empty legacy parent
                let legacy_parent = base.join("FlowDictate");
                let _ = std::fs::remove_dir(&legacy_parent); // only removes if empty
            }
        }
    }

    new_dir
}

/// Get the full path to a model's GGML file
pub fn model_path(model: WhisperModel) -> PathBuf {
    models_dir().join(model.ggml_filename())
}

/// Check if a model is already downloaded
pub fn is_model_downloaded(model: WhisperModel) -> bool {
    model_path(model).exists()
}

/// Download a model from HuggingFace
pub async fn download_model(
    model: WhisperModel,
    progress_callback: impl Fn(u64, u64) + Send + 'static,
) -> Result<PathBuf, DictationError> {
    let path = model_path(model);

    if path.exists() {
        info!("Model {} already exists at {}", model.display_name(), path.display());
        return Ok(path);
    }

    // Ensure directory exists
    let dir = models_dir();
    std::fs::create_dir_all(&dir).map_err(|e| {
        DictationError::ModelDownloadFailed(format!("Failed to create models directory: {e}"))
    })?;

    info!(
        "Downloading {} from {} (~{}MB)",
        model.display_name(),
        model.download_url(),
        model.size_mb()
    );

    let client = reqwest::Client::new();
    let response = client
        .get(model.download_url())
        .send()
        .await
        .map_err(|e| DictationError::ModelDownloadFailed(format!("Download failed: {e}")))?;

    if !response.status().is_success() {
        return Err(DictationError::ModelDownloadFailed(format!(
            "HTTP {}: {}",
            response.status(),
            model.download_url()
        )));
    }

    let total_size = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;

    // Download to a temp file then rename (atomic)
    let tmp_path = path.with_extension("bin.tmp");
    let mut file = tokio::fs::File::create(&tmp_path).await.map_err(|e| {
        DictationError::ModelDownloadFailed(format!("Failed to create temp file: {e}"))
    })?;

    use tokio::io::AsyncWriteExt;
    let mut stream = response.bytes_stream();
    use futures_util::StreamExt;

    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|e| DictationError::ModelDownloadFailed(format!("Download error: {e}")))?;
        file.write_all(&chunk).await.map_err(|e| {
            DictationError::ModelDownloadFailed(format!("Write error: {e}"))
        })?;
        downloaded += chunk.len() as u64;
        progress_callback(downloaded, total_size);
    }

    file.flush().await.map_err(|e| {
        DictationError::ModelDownloadFailed(format!("Flush error: {e}"))
    })?;
    drop(file);

    // Rename temp to final
    tokio::fs::rename(&tmp_path, &path).await.map_err(|e| {
        DictationError::ModelDownloadFailed(format!("Failed to rename temp file: {e}"))
    })?;

    info!("Model downloaded: {}", path.display());
    Ok(path)
}
