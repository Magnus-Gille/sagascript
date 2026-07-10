use std::path::{Path, PathBuf};

use tracing::info;

use crate::download::{DownloadIntegrity, GGML_MAGIC, download_to_path, verify_file};
use crate::error::DictationError;
use crate::settings::WhisperModel;

/// Get the models directory for storing GGML files
pub fn models_dir() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    migrate_legacy_models_dir(&base)
}

/// Migrate the legacy FlowDictate models directory to Sagascript's, given a
/// base app-data directory (`dirs::data_dir()` in production; a tempdir in
/// tests). Pure with respect to the filesystem seam — no global state — so
/// it's fully unit-testable without touching the real
/// `~/Library/Application Support`.
///
/// - Fresh install (neither dir exists): no-op, just returns the new path.
/// - Legacy dir only: renamed into place; the now-empty legacy parent
///   (`<base>/FlowDictate`) is removed best-effort (`remove_dir` is a no-op
///   if it's not actually empty, e.g. the old app left other files there).
/// - Both dirs exist: the legacy dir is left untouched — never silently
///   clobber a Models dir that's already populated at the new location.
pub fn migrate_legacy_models_dir(base: &Path) -> PathBuf {
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
    std::fs::metadata(model_path(model))
        .is_ok_and(|metadata| metadata.len() == model.download_integrity().size)
}

/// Silero VAD model filename (used by whisper.cpp's built-in VAD).
pub const VAD_MODEL_FILENAME: &str = "ggml-silero-v5.1.2.bin";
const VAD_MODEL_URL: &str = "https://huggingface.co/ggml-org/whisper-vad/resolve/9ffd54a1e1ee413ddf265af9913beaf518d1639b/ggml-silero-v5.1.2.bin";
const VAD_MODEL_INTEGRITY: DownloadIntegrity = DownloadIntegrity {
    sha256: "29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf",
    size: 885_098,
};
#[cfg(target_os = "macos")]
const COREML_INTEGRITY_MARKER: &str = ".sagascript-archive-sha256";

/// Full path to the Silero VAD model in the models directory.
pub fn vad_model_path() -> PathBuf {
    models_dir().join(VAD_MODEL_FILENAME)
}

/// Whether the Silero VAD model has been downloaded.
pub fn is_vad_model_downloaded() -> bool {
    std::fs::metadata(vad_model_path())
        .is_ok_and(|metadata| metadata.len() == VAD_MODEL_INTEGRITY.size)
}

/// Download the Silero VAD model used by whisper.cpp's built-in VAD (~0.9 MB).
pub async fn download_vad_model(
    progress_callback: impl Fn(u64, u64) + Send + 'static,
) -> Result<PathBuf, DictationError> {
    let path = vad_model_path();
    if path.exists() {
        verify_vad_model(&path)?;
        return Ok(path);
    }

    info!("Downloading Silero VAD model from {VAD_MODEL_URL}");

    // The Silero VAD model has shipped in ggml format since whisper.cpp
    // v1.7.4, so the same magic check as the whisper models applies.
    download_to_path(
        VAD_MODEL_URL,
        &path,
        "bin",
        VAD_MODEL_INTEGRITY,
        Some(&GGML_MAGIC),
        progress_callback,
    )
    .await?;

    info!("VAD model downloaded: {}", path.display());
    Ok(path)
}

/// Verify the VAD artifact before whisper.cpp parses it.
pub fn verify_vad_model(path: &Path) -> Result<(), DictationError> {
    verify_file(path, VAD_MODEL_INTEGRITY)
}

/// Best-effort install of the CoreML encoder for an already-downloaded model —
/// backfills models obtained before CoreML support existed, so their encoder
/// runs on the Neural Engine. No-op off macOS, for models without a CoreML
/// encoder, or if the encoder is already installed.
#[cfg(target_os = "macos")]
pub async fn backfill_coreml_encoder(model: WhisperModel) -> Result<(), DictationError> {
    ensure_coreml_encoder(model, &models_dir()).await
}

/// Non-macOS stub (no CoreML).
#[cfg(not(target_os = "macos"))]
pub async fn backfill_coreml_encoder(_model: WhisperModel) -> Result<(), DictationError> {
    Ok(())
}

/// Download a model from HuggingFace
pub async fn download_model(
    model: WhisperModel,
    progress_callback: impl Fn(u64, u64) + Send + 'static,
) -> Result<PathBuf, DictationError> {
    let dir = models_dir();
    let path = dir.join(model.ggml_filename());

    if path.exists() {
        verify_file(&path, model.download_integrity())?;
        info!("Model {} already exists at {}", model.display_name(), path.display());
        // Backfill the CoreML encoder for models downloaded before it was added.
        #[cfg(target_os = "macos")]
        if let Err(e) = ensure_coreml_encoder(model, &dir).await {
            tracing::warn!("CoreML encoder not installed for {}: {e}", model.display_name());
        }
        return Ok(path);
    }

    info!(
        "Downloading {} from {} (~{}MB)",
        model.display_name(),
        model.download_url(),
        model.size_mb()
    );

    download_to_path(
        model.download_url(),
        &path,
        "bin",
        model.download_integrity(),
        Some(&GGML_MAGIC),
        progress_callback,
    )
    .await?;

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
    let (Some(url), Some(dirname), Some(integrity)) = (
        model.coreml_encoder_url(),
        model.coreml_encoder_dirname(),
        model.coreml_encoder_integrity(),
    )
    else {
        return Ok(()); // this model has no CoreML encoder
    };

    let dest = dir.join(&dirname);
    if dest.exists() {
        quarantine_unverified_coreml_encoder_at(&dest, integrity)?;
        if dest.exists() {
            return Ok(()); // verified install already present
        }
    }

    info!(
        "Downloading CoreML encoder for {} from {url}",
        model.display_name()
    );

    // The archive is fully downloaded, exact-size checked, and SHA-256 checked
    // before `ditto` is allowed to parse it. The UUID also prevents concurrent
    // backfill attempts from sharing a staging path.
    let unique = uuid::Uuid::new_v4();
    let zip_path = dir.join(format!("{dirname}.{unique}.zip.tmp"));
    download_to_path(&url, &zip_path, "zip", integrity, None, |_, _| {}).await?;

    // Extract into a temp dir with ditto (macOS' canonical zip tool); the
    // archive contains the `.mlmodelc` directory at its root.
    let extract_dir = dir.join(format!(".{dirname}.{unique}.extract"));
    let _ = tokio::fs::remove_dir_all(&extract_dir).await; // clear any stale temp
    if let Err(e) = tokio::fs::create_dir_all(&extract_dir).await {
        let _ = tokio::fs::remove_file(&zip_path).await;
        return Err(DictationError::ModelDownloadFailed(format!(
            "CoreML temp dir failed: {e}"
        )));
    }

    // Extract, then move the `.mlmodelc` into place.
    let install = match run_ditto(&zip_path, &extract_dir).await {
        Ok(()) => {
            let extracted = extract_dir.join(&dirname);
            if extracted.exists() {
                let marker = std::fs::write(
                    extracted.join(COREML_INTEGRITY_MARKER),
                    format!("{}\n", integrity.sha256),
                )
                .map_err(|e| {
                    DictationError::ModelDownloadFailed(format!(
                        "CoreML integrity marker write failed: {e}"
                    ))
                });
                match marker {
                    Ok(()) => tokio::fs::rename(&extracted, &dest).await.map_err(|e| {
                        DictationError::ModelDownloadFailed(format!(
                            "CoreML move into place failed: {e}"
                        ))
                    }),
                    Err(e) => Err(e),
                }
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

/// Keep unverified CoreML bundles away from whisper.cpp's native CoreML
/// loader. Releases predating the integrity manifest have no archive marker;
/// those directories are reversibly quarantined and can be backfilled from a
/// verified archive without blocking ordinary Metal transcription.
#[cfg(target_os = "macos")]
pub fn quarantine_unverified_coreml_encoder(
    model: WhisperModel,
) -> Result<(), DictationError> {
    let (Some(dirname), Some(integrity)) = (
        model.coreml_encoder_dirname(),
        model.coreml_encoder_integrity(),
    ) else {
        return Ok(());
    };
    quarantine_unverified_coreml_encoder_at(&models_dir().join(dirname), integrity)
}

#[cfg(not(target_os = "macos"))]
pub fn quarantine_unverified_coreml_encoder(
    _model: WhisperModel,
) -> Result<(), DictationError> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn quarantine_unverified_coreml_encoder_at(
    dest: &Path,
    integrity: DownloadIntegrity,
) -> Result<(), DictationError> {
    if !dest.exists() {
        return Ok(());
    }
    let expected = format!("{}\n", integrity.sha256);
    if matches!(
        std::fs::read_to_string(dest.join(COREML_INTEGRITY_MARKER)),
        Ok(marker) if marker == expected
    ) {
        return Ok(());
    }

    let name = dest.file_name().and_then(|name| name.to_str()).unwrap_or("coreml");
    let quarantine = dest.with_file_name(format!(
        ".{name}.unverified.{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::rename(dest, &quarantine).map_err(|e| {
        DictationError::ModelDownloadFailed(format!(
            "CoreML encoder has no verified provenance and could not be quarantined ({}): {e}",
            dest.display()
        ))
    })?;
    tracing::warn!(
        "Quarantined unverified CoreML encoder {} at {}; a verified copy will be downloaded on the next backfill",
        dest.display(),
        quarantine.display()
    );
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

#[cfg(test)]
mod migrate_legacy_models_dir_tests {
    use super::*;

    fn temp_base() -> PathBuf {
        std::env::temp_dir().join(format!("sagascript-migrate-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn vad_source_is_immutable_and_manifest_is_plausible() {
        assert!(!VAD_MODEL_URL.contains("/resolve/main/"));
        assert_eq!(VAD_MODEL_INTEGRITY.sha256.len(), 64);
        assert!(
            VAD_MODEL_INTEGRITY
                .sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn coreml_install_requires_verified_archive_marker() {
        let base = temp_base();
        let unverified = base.join("ggml-base-encoder.mlmodelc");
        std::fs::create_dir_all(&unverified).unwrap();
        std::fs::write(unverified.join("model.mil"), b"native model").unwrap();
        let integrity = WhisperModel::Base.coreml_encoder_integrity().unwrap();

        quarantine_unverified_coreml_encoder_at(&unverified, integrity).unwrap();
        assert!(!unverified.exists());
        assert!(
            std::fs::read_dir(&base)
                .unwrap()
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().contains(".unverified."))
        );

        let verified = base.join("ggml-base.en-encoder.mlmodelc");
        std::fs::create_dir_all(&verified).unwrap();
        std::fs::write(
            verified.join(COREML_INTEGRITY_MARKER),
            format!("{}\n", integrity.sha256),
        )
        .unwrap();
        quarantine_unverified_coreml_encoder_at(&verified, integrity).unwrap();
        assert!(verified.exists());

        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn fresh_install_creates_nothing_and_returns_new_path() {
        let base = temp_base();
        // Deliberately don't create `base` itself — a fresh install has
        // neither the legacy nor the new dir yet.

        let result = migrate_legacy_models_dir(&base);

        assert_eq!(result, base.join("Sagascript").join("Models"));
        assert!(!base.join("FlowDictate").exists(), "must not create the legacy dir");
        assert!(!result.exists(), "migration itself must not create the new dir either");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn legacy_dir_with_file_is_moved_and_old_dir_removed() {
        let base = temp_base();
        let legacy_models = base.join("FlowDictate").join("Models");
        std::fs::create_dir_all(&legacy_models).unwrap();
        std::fs::write(legacy_models.join("ggml-base.bin"), b"dummy").unwrap();

        let result = migrate_legacy_models_dir(&base);

        assert_eq!(result, base.join("Sagascript").join("Models"));
        assert!(result.join("ggml-base.bin").exists(), "file must end up under Sagascript/Models");
        assert!(
            !base.join("FlowDictate").exists(),
            "now-empty legacy parent should be cleaned up"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn both_dirs_present_leaves_legacy_untouched() {
        let base = temp_base();

        let new_models = base.join("Sagascript").join("Models");
        std::fs::create_dir_all(&new_models).unwrap();
        std::fs::write(new_models.join("ggml-base.bin"), b"current").unwrap();

        let legacy_models = base.join("FlowDictate").join("Models");
        std::fs::create_dir_all(&legacy_models).unwrap();
        std::fs::write(legacy_models.join("ggml-tiny.bin"), b"legacy").unwrap();

        let result = migrate_legacy_models_dir(&base);

        assert_eq!(result, new_models);
        assert!(
            legacy_models.join("ggml-tiny.bin").exists(),
            "legacy dir must survive untouched when the new dir already exists"
        );
        assert!(
            new_models.join("ggml-base.bin").exists(),
            "existing new-dir contents must be untouched"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn sibling_file_in_legacy_parent_survives_migration() {
        let base = temp_base();
        let legacy_models = base.join("FlowDictate").join("Models");
        std::fs::create_dir_all(&legacy_models).unwrap();
        std::fs::write(legacy_models.join("ggml-base.bin"), b"dummy").unwrap();

        // A sibling file directly under FlowDictate/ (not under Models/) —
        // e.g. a leftover settings file from the old app. `remove_dir` only
        // removes empty directories, so this must survive the migration.
        let sibling = base.join("FlowDictate").join("settings.json");
        std::fs::write(&sibling, b"{}").unwrap();

        let result = migrate_legacy_models_dir(&base);

        assert_eq!(result, base.join("Sagascript").join("Models"));
        assert!(result.join("ggml-base.bin").exists());
        assert!(sibling.exists(), "sibling file in legacy parent must survive");
        assert!(
            base.join("FlowDictate").exists(),
            "legacy parent must survive since it's not empty"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
