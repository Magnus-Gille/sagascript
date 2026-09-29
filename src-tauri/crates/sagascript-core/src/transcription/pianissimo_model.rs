//! Download metadata and lifecycle helpers for the Core ML conversion of
//! KlangAI's Swedish Pianissimo checkpoint (`pianissimo-sv-coreml`).
//!
//! The checkpoint is distributed by Klang AI AB under the Creative Commons
//! Attribution 4.0 International license (CC BY 4.0):
//! <https://creativecommons.org/licenses/by/4.0/>.
//! Source model card and attribution: <https://huggingface.co/KlangAI/pianissimo-sv>.
//!
//! The artifact is one zip archive with a pinned SHA-256. It is downloaded,
//! verified, extracted into a temporary directory next to the final location,
//! verified file by file against the manifest, and only then renamed to
//! `<models dir>/pianissimo-sv-coreml-<revision>/` so an interrupted install can
//! never leave a half-populated model directory behind.
//!
//! Developers can point `SAGASCRIPT_PIANISSIMO_MODEL_DIR` at an existing model
//! directory; downloads and hash verification are then skipped (the directory
//! must still contain the four Core ML packages and the vocabulary).

use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::download::{
    download_to_path, prepare_existing_artifact, verify_file_with_stamp, DownloadIntegrity,
    ExistingArtifact,
};
use crate::error::DictationError;
use crate::transcription::model::models_dir;

/// Stable model family id; the installed directory and the id sent to the
/// engine host append the revision (`pianissimo-sv-coreml-<revision>`).
pub const MODEL_ID: &str = "pianissimo-sv-coreml";

/// Environment override (development): use this existing model directory.
pub const MODEL_DIR_ENV: &str = "SAGASCRIPT_PIANISSIMO_MODEL_DIR";

/// Old NeMo-Speech.cpp artifacts removed on install and on explicit deletion.
const LEGACY_GGUF_FILENAME: &str = "pianissimo-sv-q8-melfix.gguf";
const LEGACY_NEMO_FILENAME: &str = "pianissimo-sv.nemo";

/// Directory (inside the model directory) holding verification stamps. Stamps
/// live outside the `.mlpackage` bundles so Core ML never sees foreign files.
const STAMP_DIR: &str = ".verified";

/// Core ML packages the engine host loads, plus the vocabulary. A model
/// directory must contain all of these to be usable.
pub const REQUIRED_ITEMS: [&str; 5] = [
    "Preprocessor.mlpackage",
    "Encoder.mlpackage",
    "Decoder.mlpackage",
    "JointDecisionv3.mlpackage",
    "parakeet_vocab.json",
];

/// One expected file inside the extracted archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManifestFile {
    /// Forward-slash path relative to the model directory.
    pub path: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

impl ManifestFile {
    pub const fn integrity(&self) -> DownloadIntegrity {
        DownloadIntegrity {
            sha256: self.sha256,
            size: self.size,
        }
    }
}

/// Immutable description of one published model artifact.
#[derive(Debug, Clone, Copy)]
pub struct ModelManifest {
    pub model_id: &'static str,
    pub revision: &'static str,
    pub url: &'static str,
    pub archive: DownloadIntegrity,
    pub files: &'static [ManifestFile],
}

impl ModelManifest {
    /// `pianissimo-sv-coreml-<revision>`: installed directory name and the
    /// `model_id` announced to the engine host.
    pub fn versioned_id(&self) -> String {
        format!("{}-{}", self.model_id, self.revision)
    }

    /// Installed (extracted) size in bytes.
    pub fn installed_size(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }

    /// True while the constants still hold the unpublished placeholder digests.
    pub fn is_placeholder(&self) -> bool {
        self.archive.sha256.bytes().all(|b| b == b'0')
    }
}

// TODO(conductor): fill from conversion output. Every constant below (revision,
// URL, archive size + SHA-256, per-file sizes + SHA-256) is a placeholder until
// the converted Core ML artifact is published. The all-zero digests make
// `download()` refuse to run instead of failing confusingly on a hash mismatch.
const ZERO_SHA: &str = "0000000000000000000000000000000000000000000000000000000000000000";

macro_rules! placeholder_file {
    ($path:literal) => {
        ManifestFile {
            path: $path,
            size: 0, // TODO(conductor): fill from conversion output
            sha256: ZERO_SHA,
        }
    };
}

/// The published artifact.
pub const MANIFEST: ModelManifest = ModelManifest {
    model_id: MODEL_ID,
    revision: "r0", // TODO(conductor): fill from conversion output
    // TODO(conductor): fill from conversion output
    url: "https://github.com/Magnus-Gille/sagascript/releases/download/TODO/pianissimo-sv-coreml.zip",
    archive: DownloadIntegrity {
        sha256: ZERO_SHA, // TODO(conductor): fill from conversion output
        size: 0,          // TODO(conductor): fill from conversion output
    },
    // TODO(conductor): replace with the real file list from the conversion output
    // (every file inside each .mlpackage, not just the samples below).
    files: &[
        placeholder_file!("Preprocessor.mlpackage/Manifest.json"),
        placeholder_file!("Preprocessor.mlpackage/Data/com.apple.CoreML/model.mlmodel"),
        placeholder_file!("Encoder.mlpackage/Manifest.json"),
        placeholder_file!("Encoder.mlpackage/Data/com.apple.CoreML/model.mlmodel"),
        placeholder_file!("Encoder.mlpackage/Data/com.apple.CoreML/weights/weight.bin"),
        placeholder_file!("Decoder.mlpackage/Manifest.json"),
        placeholder_file!("Decoder.mlpackage/Data/com.apple.CoreML/model.mlmodel"),
        placeholder_file!("Decoder.mlpackage/Data/com.apple.CoreML/weights/weight.bin"),
        placeholder_file!("JointDecisionv3.mlpackage/Manifest.json"),
        placeholder_file!("JointDecisionv3.mlpackage/Data/com.apple.CoreML/model.mlmodel"),
        placeholder_file!("JointDecisionv3.mlpackage/Data/com.apple.CoreML/weights/weight.bin"),
        placeholder_file!("parakeet_vocab.json"),
        placeholder_file!("manifest.json"),
        placeholder_file!("LICENSE-and-attribution.txt"),
    ],
};

// ---- locations ------------------------------------------------------------

/// Developer override directory, when set and non-empty.
pub fn override_dir() -> Option<PathBuf> {
    std::env::var_os(MODEL_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Installed model directory for `manifest` under `models`.
pub fn installed_dir_in(manifest: &ModelManifest, models: &Path) -> PathBuf {
    models.join(manifest.versioned_id())
}

/// Directory the engine host should load: the override, else the installed
/// directory in the shared model cache.
pub fn model_dir() -> PathBuf {
    override_dir().unwrap_or_else(|| installed_dir_in(&MANIFEST, &models_dir()))
}

/// Alias kept for callers that prefer the older name.
pub fn path() -> PathBuf {
    model_dir()
}

/// Alias for callers that prefer an explicit model name.
pub fn model_path() -> PathBuf {
    model_dir()
}

/// Id announced to the engine host in `load` (`pianissimo-sv-coreml-<rev>`).
pub fn model_id() -> String {
    MANIFEST.versioned_id()
}

/// Bytes to download (the archive).
pub fn download_size_bytes() -> u64 {
    MANIFEST.archive.size
}

/// Bytes on disk once installed.
pub fn installed_size_bytes() -> u64 {
    MANIFEST.installed_size()
}

/// Whether the dev override is active (downloads/deletes are then no-ops).
pub fn is_overridden() -> bool {
    override_dir().is_some()
}

// ---- presence and verification -------------------------------------------

fn override_is_usable(dir: &Path) -> bool {
    REQUIRED_ITEMS.iter().all(|item| dir.join(item).exists())
}

fn is_downloaded_in(manifest: &ModelManifest, dir: &Path) -> bool {
    if manifest.is_placeholder() {
        return false;
    }
    manifest.files.iter().all(|file| {
        std::fs::metadata(dir.join(file.path))
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() == file.size)
    })
}

/// Fast presence check for model listings (existence + size of every manifest
/// file). Hash verification happens through [`verify_downloaded`].
pub fn is_downloaded() -> bool {
    match override_dir() {
        Some(dir) => override_is_usable(&dir),
        None => is_downloaded_in(&MANIFEST, &installed_dir_in(&MANIFEST, &models_dir())),
    }
}

fn stamp_path_for(dir: &Path, file: &ManifestFile) -> PathBuf {
    dir.join(STAMP_DIR)
        .join(format!("{}.verified.json", file.path.replace('/', "__")))
}

fn verify_in(
    manifest: &ModelManifest,
    dir: &Path,
    force: bool,
) -> Result<(), DictationError> {
    if manifest.is_placeholder() {
        return Err(DictationError::ModelDownloadFailed(
            "The Pianissimo Core ML model manifest has not been published yet".into(),
        ));
    }
    if dir.is_dir() {
        // Best effort: without the directory stamps are simply not persisted.
        let _ = std::fs::create_dir_all(dir.join(STAMP_DIR));
    }
    for file in manifest.files {
        let path = dir.join(file.path);
        verify_file_with_stamp(&path, &stamp_path_for(dir, file), file.integrity(), force)?;
    }
    Ok(())
}

/// Verify the installed model against the manifest. Files with a matching
/// persistent stamp are not re-hashed. With the dev override active only
/// presence of the required items is checked.
pub fn verify_downloaded() -> Result<(), DictationError> {
    verify_installed(false)
}

/// Like [`verify_downloaded`] but ignores every stamp and re-hashes all files
/// (`sagascript engine doctor --verify-model`).
pub fn verify_downloaded_forced() -> Result<(), DictationError> {
    verify_installed(true)
}

fn verify_installed(force: bool) -> Result<(), DictationError> {
    if let Some(dir) = override_dir() {
        return if override_is_usable(&dir) {
            Ok(())
        } else {
            Err(DictationError::ModelDownloadFailed(format!(
                "{MODEL_DIR_ENV}={} must contain {}",
                dir.display(),
                REQUIRED_ITEMS.join(", ")
            )))
        };
    }
    verify_in(&MANIFEST, &installed_dir_in(&MANIFEST, &models_dir()), force)
}

// ---- install --------------------------------------------------------------

fn install_err(message: impl Into<String>) -> DictationError {
    DictationError::ModelDownloadFailed(message.into())
}

/// Extract `archive` with path-traversal, symlink and size guards.
fn extract_archive(archive: &Path, dest: &Path, max_bytes: u64) -> Result<(), DictationError> {
    let file = std::fs::File::open(archive)
        .map_err(|e| install_err(format!("Cannot open model archive: {e}")))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| install_err(format!("Model archive is not a valid zip file: {e}")))?;
    let mut written_total: u64 = 0;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| install_err(format!("Corrupt model archive entry {index}: {e}")))?;
        let Some(relative) = entry.enclosed_name() else {
            return Err(install_err(format!(
                "Model archive entry {:?} escapes the extraction directory",
                entry.name()
            )));
        };
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(install_err(format!(
                "Model archive entry {:?} is a symlink, which is not allowed",
                entry.name()
            )));
        }
        let target = dest.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|e| install_err(format!("Cannot create {}: {e}", target.display())))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| install_err(format!("Cannot create {}: {e}", parent.display())))?;
        }
        let mut out = std::fs::File::create(&target)
            .map_err(|e| install_err(format!("Cannot create {}: {e}", target.display())))?;
        // Bounded copy: a lying header cannot exceed the manifest's total size.
        let remaining = max_bytes.saturating_sub(written_total);
        let copied = std::io::copy(&mut std::io::Read::take(&mut entry, remaining + 1), &mut out)
            .map_err(|e| install_err(format!("Cannot extract {}: {e}", target.display())))?;
        written_total += copied;
        if written_total > max_bytes {
            return Err(install_err(
                "Model archive expands beyond the size declared in the manifest",
            ));
        }
    }
    Ok(())
}

/// The archive may wrap everything in one top-level directory; return the
/// directory that holds the manifest files.
fn content_root(extracted: &Path, manifest: &ModelManifest) -> PathBuf {
    let has_first = |dir: &Path| {
        manifest
            .files
            .first()
            .is_some_and(|file| dir.join(file.path).exists())
    };
    if has_first(extracted) {
        return extracted.to_path_buf();
    }
    if let Ok(mut entries) = std::fs::read_dir(extracted) {
        if let (Some(Ok(only)), None) = (entries.next(), entries.next()) {
            if only.path().is_dir() && has_first(&only.path()) {
                return only.path();
            }
        }
    }
    extracted.to_path_buf()
}

/// Extract `archive`, verify every manifest file, and atomically move the result
/// to its final directory under `models`. The archive itself must already have
/// been verified by the caller. Returns the installed directory.
pub fn install_archive_in(
    manifest: &ModelManifest,
    archive: &Path,
    models: &Path,
) -> Result<PathBuf, DictationError> {
    std::fs::create_dir_all(models)
        .map_err(|e| install_err(format!("Failed to create models directory: {e}")))?;
    let staging = models.join(format!(".pianissimo-extract-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        std::fs::create_dir(&staging)
            .map_err(|e| install_err(format!("Cannot create staging directory: {e}")))?;
        extract_archive(archive, &staging, manifest.installed_size().saturating_add(1 << 20))?;
        let root = content_root(&staging, manifest);
        verify_in(manifest, &root, true)?;
        let final_dir = installed_dir_in(manifest, models);
        // Replace any previous (possibly corrupt) install without ever exposing
        // a partial directory under the final name.
        let trash = models.join(format!(".pianissimo-old-{}", uuid::Uuid::new_v4()));
        let had_old = final_dir.exists();
        if had_old {
            std::fs::rename(&final_dir, &trash)
                .map_err(|e| install_err(format!("Cannot replace existing model: {e}")))?;
        }
        if let Err(e) = std::fs::rename(&root, &final_dir) {
            if had_old {
                let _ = std::fs::rename(&trash, &final_dir);
            }
            return Err(install_err(format!("Cannot move model into place: {e}")));
        }
        if had_old {
            let _ = std::fs::remove_dir_all(&trash);
        }
        Ok(final_dir)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// Download the Core ML model archive, verify, extract and install it. Removes
/// the archive and any legacy NeMo/GGUF files afterwards.
pub async fn download(
    progress_callback: impl Fn(u64, u64) + Send + 'static,
) -> Result<PathBuf, DictationError> {
    if let Some(dir) = override_dir() {
        info!("Using {MODEL_DIR_ENV}={}; nothing to download", dir.display());
        return if override_is_usable(&dir) {
            Ok(dir)
        } else {
            Err(install_err(format!(
                "{MODEL_DIR_ENV}={} must contain {}",
                dir.display(),
                REQUIRED_ITEMS.join(", ")
            )))
        };
    }
    let models = models_dir();
    let manifest = &MANIFEST;
    let destination = installed_dir_in(manifest, &models);

    if is_downloaded_in(manifest, &destination) && verify_in(manifest, &destination, false).is_ok()
    {
        info!("Pianissimo model already exists at {}", destination.display());
        cleanup_superseded(&models);
        return Ok(destination);
    }
    if manifest.is_placeholder() {
        return Err(install_err(
            "The Pianissimo Core ML model has not been published yet (manifest placeholder)",
        ));
    }

    let archive = models.join(format!("{}.zip", manifest.versioned_id()));
    if prepare_existing_artifact(&archive, manifest.archive)? != ExistingArtifact::Verified {
        info!("Downloading Pianissimo model from {}", manifest.url);
        download_to_path(
            manifest.url,
            &archive,
            "zip",
            manifest.archive,
            Some(b"PK\x03\x04"),
            progress_callback,
        )
        .await?;
    }

    let installed = tokio::task::spawn_blocking({
        let archive = archive.clone();
        let models = models.clone();
        move || install_archive_in(&MANIFEST, &archive, &models)
    })
    .await
    .map_err(|e| install_err(format!("Model install task failed: {e}")))?;
    crate::download::remove_verification_stamp(&archive);
    let _ = std::fs::remove_file(&archive);
    let installed = installed?;

    cleanup_superseded(&models);
    info!("Pianissimo model installed: {}", installed.display());
    Ok(installed)
}

// ---- cleanup --------------------------------------------------------------

fn remove_file_if_present(path: &Path) -> Result<(), DictationError> {
    crate::download::remove_verification_stamp(path);
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(install_err(format!(
            "Failed to delete Pianissimo model file at {}: {error}",
            path.display()
        ))),
    }
}

fn remove_dir_if_present(path: &Path) -> Result<(), DictationError> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(install_err(format!(
            "Failed to delete Pianissimo model at {}: {error}",
            path.display()
        ))),
    }
}

/// Remove the NeMo-era GGUF (and the original `.nemo` checkpoint) plus stamps.
pub fn remove_legacy_files_in(models: &Path) -> Result<(), DictationError> {
    remove_file_if_present(&models.join(LEGACY_GGUF_FILENAME))?;
    remove_file_if_present(&models.join(LEGACY_NEMO_FILENAME))
}

/// Best-effort cleanup after a successful install: legacy files, older
/// revisions, and leftover staging directories.
fn cleanup_superseded(models: &Path) {
    if let Err(error) = remove_legacy_files_in(models) {
        warn!("Could not remove legacy Pianissimo files: {error}");
    }
    let current = MANIFEST.versioned_id();
    let prefix = format!("{MODEL_ID}-");
    let Ok(entries) = std::fs::read_dir(models) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let stale_revision = name.starts_with(&prefix) && name != current && entry.path().is_dir();
        let stale_staging = name.starts_with(".pianissimo-extract-") || name.starts_with(".pianissimo-old-");
        if stale_revision || stale_staging {
            if let Err(error) = remove_dir_if_present(&entry.path()) {
                warn!("{error}");
            }
        }
    }
}

fn delete_in(manifest: &ModelManifest, models: &Path) -> Result<(), DictationError> {
    remove_dir_if_present(&installed_dir_in(manifest, models))?;
    remove_file_if_present(&models.join(format!("{}.zip", manifest.versioned_id())))?;
    remove_legacy_files_in(models)
}

/// Remove the installed model, any partial archive, and the legacy GGUF on
/// explicit user-initiated deletion. A dev override directory is never touched.
pub fn delete() -> Result<(), DictationError> {
    // A running host may hold the model open; stop it before removing files.
    crate::transcription::pianissimo_backend::shutdown_shared_client();
    let models = models_dir();
    if is_overridden() {
        warn!("{MODEL_DIR_ENV} is set; leaving the override directory untouched");
    }
    delete_in(&MANIFEST, &models)?;
    cleanup_superseded(&models);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;

    use sha2::{Digest, Sha256};

    use super::*;

    /// Fixture files: (relative path, contents).
    const FIXTURE: [(&str, &[u8]); 6] = [
        ("Preprocessor.mlpackage/Manifest.json", b"{\"pre\":1}"),
        ("Encoder.mlpackage/Data/com.apple.CoreML/weights/weight.bin", b"encoder-weights"),
        ("Decoder.mlpackage/Manifest.json", b"{\"dec\":1}"),
        ("JointDecisionv3.mlpackage/Manifest.json", b"{\"joint\":1}"),
        ("parakeet_vocab.json", b"[\"a\"]"),
        ("LICENSE-and-attribution.txt", b"CC BY 4.0"),
    ];

    fn sha(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn leak(value: String) -> &'static str {
        Box::leak(value.into_boxed_str())
    }

    fn build_zip(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = dir.join("model.zip");
        let mut writer = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, bytes) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
        path
    }

    fn manifest_for(entries: &[(&str, &[u8])], archive: &Path) -> ModelManifest {
        let files: Vec<ManifestFile> = entries
            .iter()
            .map(|(path, bytes)| ManifestFile {
                path: leak((*path).to_string()),
                size: bytes.len() as u64,
                sha256: leak(sha(bytes)),
            })
            .collect();
        let bytes = fs::read(archive).unwrap();
        ModelManifest {
            model_id: "pianissimo-sv-coreml",
            revision: "test1",
            url: "http://invalid.example/model.zip",
            archive: DownloadIntegrity {
                sha256: leak(sha(&bytes)),
                size: bytes.len() as u64,
            },
            files: Box::leak(files.into_boxed_slice()),
        }
    }

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn production_manifest_is_named_and_marked_as_placeholder() {
        assert_eq!(MANIFEST.versioned_id(), format!("pianissimo-sv-coreml-{}", MANIFEST.revision));
        assert!(MANIFEST.is_placeholder(), "update this test when the real artifact lands");
        for required in REQUIRED_ITEMS {
            assert!(
                MANIFEST.files.iter().any(|f| f.path.starts_with(required) || f.path == required),
                "{required} missing from manifest"
            );
        }
        for extra in ["manifest.json", "LICENSE-and-attribution.txt"] {
            assert!(MANIFEST.files.iter().any(|f| f.path == extra));
        }
    }

    #[test]
    fn install_extracts_verifies_and_renames_atomically() {
        let root = tmp();
        let models = root.path().join("Models");
        fs::create_dir_all(&models).unwrap();
        let archive = build_zip(root.path(), &FIXTURE);
        let manifest = manifest_for(&FIXTURE, &archive);

        let dir = install_archive_in(&manifest, &archive, &models).unwrap();

        assert_eq!(dir, models.join("pianissimo-sv-coreml-test1"));
        assert!(is_downloaded_in(&manifest, &dir));
        verify_in(&manifest, &dir, false).unwrap();
        // Stamps live outside the mlpackages.
        assert!(dir.join(STAMP_DIR).is_dir());
        assert!(!dir.join("Encoder.mlpackage").join("Data").join("com.apple.CoreML").join("weights").join("weight.bin.verified.json").exists());
        // No staging leftovers.
        let leftovers: Vec<_> = fs::read_dir(&models)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn install_accepts_single_top_level_directory_in_archive() {
        let root = tmp();
        let models = root.path().join("Models");
        let wrapped: Vec<(String, &[u8])> = FIXTURE
            .iter()
            .map(|(p, b)| (format!("wrapper/{p}"), *b))
            .collect();
        let refs: Vec<(&str, &[u8])> = wrapped.iter().map(|(p, b)| (p.as_str(), *b)).collect();
        let archive = build_zip(root.path(), &refs);
        let manifest = manifest_for(&FIXTURE, &archive);
        let dir = install_archive_in(&manifest, &archive, &models).unwrap();
        assert!(is_downloaded_in(&manifest, &dir));
    }

    #[test]
    fn corrupt_file_in_archive_leaves_no_install() {
        let root = tmp();
        let models = root.path().join("Models");
        fs::create_dir_all(&models).unwrap();
        let good = build_zip(root.path(), &FIXTURE);
        let manifest = manifest_for(&FIXTURE, &good);
        // Same sizes, different bytes in one file.
        let mut bad_entries = FIXTURE.to_vec();
        bad_entries[1] = (FIXTURE[1].0, b"ENCODER-WEIGHTS");
        let bad = build_zip(root.path(), &bad_entries);

        let error = install_archive_in(&manifest, &bad, &models).unwrap_err();

        assert!(error.to_string().contains("integrity"), "{error}");
        assert!(!installed_dir_in(&manifest, &models).exists());
        assert_eq!(fs::read_dir(&models).unwrap().count(), 0, "staging dir cleaned up");
    }

    #[test]
    fn truncated_or_garbage_archive_is_rejected_without_touching_existing_install() {
        let root = tmp();
        let models = root.path().join("Models");
        fs::create_dir_all(&models).unwrap();
        let good = build_zip(root.path(), &FIXTURE);
        let manifest = manifest_for(&FIXTURE, &good);
        let dir = install_archive_in(&manifest, &good, &models).unwrap();

        let garbage = root.path().join("garbage.zip");
        fs::write(&garbage, b"PK\x03\x04 definitely not a zip").unwrap();
        let error = install_archive_in(&manifest, &garbage, &models).unwrap_err();

        assert!(error.to_string().contains("zip"), "{error}");
        assert!(is_downloaded_in(&manifest, &dir), "previous install intact");
    }

    #[test]
    fn missing_manifest_file_in_archive_is_rejected() {
        let root = tmp();
        let models = root.path().join("Models");
        let good = build_zip(root.path(), &FIXTURE);
        let manifest = manifest_for(&FIXTURE, &good);
        let partial = build_zip(root.path(), &FIXTURE[..4]);
        assert!(install_archive_in(&manifest, &partial, &models).is_err());
        assert!(!installed_dir_in(&manifest, &models).exists());
    }

    #[test]
    fn path_traversal_entries_are_rejected() {
        let root = tmp();
        let models = root.path().join("Models");
        let mut entries = FIXTURE.to_vec();
        entries.push(("../escape.txt", b"x"));
        let archive = build_zip(root.path(), &entries);
        let manifest = manifest_for(&FIXTURE, &archive);
        let error = install_archive_in(&manifest, &archive, &models).unwrap_err();
        assert!(error.to_string().contains("escapes"), "{error}");
        assert!(!root.path().join("escape.txt").exists());
    }

    #[test]
    fn reinstall_replaces_a_corrupt_previous_directory() {
        let root = tmp();
        let models = root.path().join("Models");
        let archive = build_zip(root.path(), &FIXTURE);
        let manifest = manifest_for(&FIXTURE, &archive);
        let dir = install_archive_in(&manifest, &archive, &models).unwrap();
        fs::write(dir.join("parakeet_vocab.json"), b"[\"X\"]").unwrap(); // same size, wrong bytes
        assert!(verify_in(&manifest, &dir, false).is_err());

        let dir = install_archive_in(&manifest, &archive, &models).unwrap();

        verify_in(&manifest, &dir, false).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn second_verification_uses_stamps_and_forced_rehashes() {
        let root = tmp();
        let models = root.path().join("Models");
        let archive = build_zip(root.path(), &FIXTURE);
        let manifest = manifest_for(&FIXTURE, &archive);
        let dir = install_archive_in(&manifest, &archive, &models).unwrap();
        let stamps: Vec<_> = fs::read_dir(dir.join(STAMP_DIR)).unwrap().flatten().collect();
        assert_eq!(stamps.len(), FIXTURE.len());
        verify_in(&manifest, &dir, false).unwrap();
        verify_in(&manifest, &dir, true).unwrap();
        // Forced verification catches a same-size in-place edit.
        fs::write(dir.join("parakeet_vocab.json"), b"[\"X\"]").unwrap();
        assert!(verify_in(&manifest, &dir, true).is_err());
    }

    #[test]
    fn delete_removes_install_archive_and_legacy_gguf() {
        let root = tmp();
        let models = root.path().join("Models");
        let archive = build_zip(root.path(), &FIXTURE);
        let manifest = manifest_for(&FIXTURE, &archive);
        let dir = install_archive_in(&manifest, &archive, &models).unwrap();
        fs::write(models.join(LEGACY_GGUF_FILENAME), b"gguf").unwrap();
        fs::write(models.join(format!("{LEGACY_GGUF_FILENAME}.verified.json")), b"{}").unwrap();
        fs::write(models.join(LEGACY_NEMO_FILENAME), b"nemo").unwrap();

        delete_in(&manifest, &models).unwrap();
        delete_in(&manifest, &models).unwrap(); // idempotent

        assert!(!dir.exists());
        assert_eq!(fs::read_dir(&models).unwrap().count(), 0);
    }

    #[test]
    fn override_requires_all_items() {
        let root = tmp();
        assert!(!override_is_usable(root.path()));
        for item in REQUIRED_ITEMS {
            let path = root.path().join(item);
            if item.ends_with(".mlpackage") {
                fs::create_dir_all(path).unwrap();
            } else {
                fs::write(path, b"[]").unwrap();
            }
        }
        assert!(override_is_usable(root.path()));
    }

    #[test]
    fn placeholder_manifest_is_never_reported_as_downloaded() {
        let root = tmp();
        assert!(!is_downloaded_in(&MANIFEST, root.path()));
        assert!(verify_in(&MANIFEST, root.path(), false).is_err());
    }
}
