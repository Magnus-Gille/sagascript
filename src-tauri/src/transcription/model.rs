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
    let dir = models_dir();
    let path = dir.join(model.ggml_filename());

    if path.exists() {
        info!("Model {} already exists at {}", model.display_name(), path.display());
        // Backfill the CoreML encoder for models downloaded before it was added.
        #[cfg(target_os = "macos")]
        if let Err(e) = ensure_coreml_encoder(model, &dir).await {
            tracing::warn!("CoreML encoder not installed for {}: {e}", model.display_name());
        }
        return Ok(path);
    }

    // Ensure directory exists
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

    // Best-effort: fetch the CoreML encoder so whisper.cpp runs the encoder on
    // the Neural Engine instead of falling back to the Metal encoder. Non-fatal
    // — transcription still works (just slower) if this fails.
    #[cfg(target_os = "macos")]
    if let Err(e) = ensure_coreml_encoder(model, &dir).await {
        tracing::warn!("CoreML encoder not installed for {}: {e}", model.display_name());
    }

    Ok(path)
}

/// Download and install the CoreML encoder (`ggml-<name>-encoder.mlmodelc`) next
/// to the GGML file so whisper.cpp uses the Neural Engine for the encoder. The
/// archive is streamed to a temp file, extracted with macOS' `ditto`, and the
/// resulting `.mlmodelc` directory is moved into place atomically. Idempotent:
/// returns early if the model has no CoreML encoder or it is already installed.
#[cfg(target_os = "macos")]
async fn ensure_coreml_encoder(
    model: WhisperModel,
    dir: &std::path::Path,
) -> Result<(), DictationError> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let (Some(url), Some(dirname)) =
        (model.coreml_encoder_url(), model.coreml_encoder_dirname())
    else {
        return Ok(()); // this model has no CoreML encoder
    };

    let dest = dir.join(&dirname);
    if dest.exists() {
        return Ok(()); // already installed
    }

    info!(
        "Downloading CoreML encoder for {} from {url}",
        model.display_name()
    );

    let client = reqwest::Client::new();
    let response = client.get(&url).send().await.map_err(|e| {
        DictationError::ModelDownloadFailed(format!("CoreML download failed: {e}"))
    })?;
    if !response.status().is_success() {
        return Err(DictationError::ModelDownloadFailed(format!(
            "CoreML HTTP {}: {url}",
            response.status()
        )));
    }

    // Stream the zip to a temp file (encoders can be hundreds of MB).
    let zip_path = dir.join(format!("{dirname}.zip.tmp"));
    {
        let mut file = tokio::fs::File::create(&zip_path).await.map_err(|e| {
            DictationError::ModelDownloadFailed(format!("CoreML temp create failed: {e}"))
        })?;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                DictationError::ModelDownloadFailed(format!("CoreML download error: {e}"))
            })?;
            file.write_all(&chunk).await.map_err(|e| {
                DictationError::ModelDownloadFailed(format!("CoreML write error: {e}"))
            })?;
        }
        file.flush().await.map_err(|e| {
            DictationError::ModelDownloadFailed(format!("CoreML flush error: {e}"))
        })?;
    }

    // Extract into a temp dir with ditto (macOS' canonical zip tool); the
    // archive contains the `.mlmodelc` directory at its root.
    let extract_dir = dir.join(format!(".{dirname}.extract"));
    let _ = tokio::fs::remove_dir_all(&extract_dir).await; // clear any stale temp
    tokio::fs::create_dir_all(&extract_dir).await.map_err(|e| {
        DictationError::ModelDownloadFailed(format!("CoreML temp dir failed: {e}"))
    })?;

    // Extract, then move the `.mlmodelc` into place.
    let install = match run_ditto(&zip_path, &extract_dir).await {
        Ok(()) => {
            let extracted = extract_dir.join(&dirname);
            if extracted.exists() {
                tokio::fs::rename(&extracted, &dest).await.map_err(|e| {
                    DictationError::ModelDownloadFailed(format!(
                        "CoreML move into place failed: {e}"
                    ))
                })
            } else {
                Err(DictationError::ModelDownloadFailed(format!(
                    "CoreML archive did not contain {dirname}"
                )))
            }
        }
        Err(e) => Err(e),
    };

    // Clean up the temp zip and extraction dir regardless of outcome.
    let _ = tokio::fs::remove_file(&zip_path).await;
    let _ = tokio::fs::remove_dir_all(&extract_dir).await;

    install?;
    info!("CoreML encoder installed: {}", dest.display());
    Ok(())
}

/// Run `ditto -x -k <zip> <dest_dir>` to expand a PKZip archive.
#[cfg(target_os = "macos")]
async fn run_ditto(
    zip: &std::path::Path,
    dest_dir: &std::path::Path,
) -> Result<(), DictationError> {
    let status = tokio::process::Command::new("/usr/bin/ditto")
        .arg("-x")
        .arg("-k")
        .arg(zip)
        .arg(dest_dir)
        .status()
        .await
        .map_err(|e| DictationError::ModelDownloadFailed(format!("ditto failed to run: {e}")))?;
    if !status.success() {
        return Err(DictationError::ModelDownloadFailed(format!(
            "ditto exited unsuccessfully ({status})"
        )));
    }
    Ok(())
}
