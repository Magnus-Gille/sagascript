//! Shared model-download pipeline: stream a URL to a uniquely-named temp
//! file, validate the result, then atomically rename it into place. Used by
//! every model downloader in the crate (whisper GGML models, the Silero VAD
//! model, and the diarization ONNX models) so this hardening lives in one
//! place instead of three near-identical, independently-drifting copies.
//!
//! What this closes relative to the original triplicated code:
//! - every error path removes the temp file (no more orphaned multi-hundred-
//!   MB `.bin.tmp` left behind by a dropped connection or a full disk);
//! - the response body is validated (length + optional magic bytes) before
//!   the rename, so an HTML rate-limit page or a git-LFS pointer stub can
//!   never be renamed into a `ggml-*.bin` and silently make
//!   `is_model_downloaded()` report success forever;
//! - the temp filename is unique per invocation (previously a fixed
//!   `<name>.bin.tmp`), so two concurrent downloads of the same model can no
//!   longer interleave bytes into one corrupt file. Losing the fixed name
//!   also loses its incidental self-cleaning property (a retried download
//!   used to just overwrite its own leftover), so `download_to_path`
//!   opportunistically sweeps stale `.tmp` files from the destination
//!   directory before it starts.

use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[cfg(unix)]
use std::collections::HashMap;
#[cfg(unix)]
use std::sync::{Mutex, OnceLock};

use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::error::DictationError;

/// whisper.cpp's `GGML_FILE_MAGIC` (`0x67676d6c`, mnemonic "ggml") as it
/// actually appears on disk: the constant is written out as a little-endian
/// `u32`, so the file's first 4 bytes are `6c 6d 67 67` — not the ASCII
/// string "ggml". Verified directly against real downloaded models (both
/// stock ggerganov/whisper.cpp files and the KBLab/NbAiLab q5_0 quantized
/// fine-tunes — quantization only changes tensor payloads, never the
/// header) and against the Silero VAD model, which whisper.cpp has shipped
/// in ggml format since v1.7.4.
pub const GGML_MAGIC: [u8; 4] = [0x6c, 0x6d, 0x67, 0x67];

/// Number of leading response bytes buffered for the magic check. Only ever
/// need to compare against `GGML_MAGIC` (4 bytes) today; a little slack
/// keeps this working if a longer magic is added later without holding the
/// whole file in memory.
const MAGIC_PREFIX_LEN: usize = 8;

/// A stale temp file older than this is treated as an orphan from a
/// crashed/killed download rather than a concurrent, in-progress download of
/// a *different* model sharing the same directory (temp names are unique
/// per invocation, so a live download's own temp file is never the one a
/// given call is about to create — but another call's could still be
/// sitting alongside it). No real model download takes anywhere near this
/// long, so this is generous on purpose.
const ORPHAN_TMP_MAX_AGE: Duration = Duration::from_secs(60 * 60);

/// Network bounds for one download. Defaults are sized for multi-GB models on
/// slow lines; tests inject tiny values.
///
/// - `connect_timeout` (30 s): TCP/TLS connect must finish quickly.
/// - `idle_timeout` (60 s): no body bytes (and no response headers) for this
///   long aborts the transfer, so a stalled server cannot hold it open.
/// - total deadline: `total_base` (5 min) plus the pinned size divided by
///   `min_bytes_per_sec` (128 KiB/s). This scales with the manifest size, so a
///   multi-GB download on a slow but moving line is never cut short, while a
///   server trickling bytes below the floor still gets bounded (about 6.8 h
///   for a 3 GB model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadLimits {
    pub connect_timeout: Duration,
    pub idle_timeout: Duration,
    pub total_base: Duration,
    pub min_bytes_per_sec: u64,
}

impl Default for DownloadLimits {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(60),
            total_base: Duration::from_secs(5 * 60),
            min_bytes_per_sec: 128 * 1024,
        }
    }
}

impl DownloadLimits {
    /// Overall wall-clock deadline for an artifact of `size` bytes.
    pub fn total_deadline(&self, size: u64) -> Duration {
        self.total_base + Duration::from_secs(size / self.min_bytes_per_sec.max(1))
    }
}

/// Immutable integrity metadata for a downloadable model artifact. Values are
/// taken from the pinned Hugging Face revision's git-LFS metadata (`oid
/// sha256` and `size`), not from a mutable branch or a locally computed guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadIntegrity {
    pub sha256: &'static str,
    pub size: u64,
}

/// Result of preparing an app-managed artifact for a download attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingArtifact {
    Missing,
    Verified,
    RemovedInvalid,
}

enum VerificationFailure {
    IntegrityMismatch(String),
    Other(DictationError),
}

/// Stream `url` to `dest`, via a uniquely-named temp file in the same
/// directory so the final rename is atomic and same-filesystem.
///
/// - `tmp_ext` is a short marker folded into the temp filename purely for
///   readability when a leftover file is being debugged (e.g. `"bin"` or
///   `"onnx"`) — it plays no role in making the name unique.
/// - `expected_magic`, when `Some`, is checked against the first bytes of
///   the downloaded file before the rename. Pass `None` to skip the check
///   (e.g. ONNX files are bare protobuf with no fixed leading bytes, so a
///   magic check there risks false-rejecting a valid model).
/// - `progress_callback` is invoked after every chunk with
///   `(bytes_downloaded_so_far, total_size_or_0_if_unknown)`.
///
/// On any failure the temp file is removed best-effort before the error is
/// returned; `dest` itself is only ever touched by the final rename, so a
/// failed download can never leave a partial or invalid file in its place.
pub async fn download_to_path(
    url: &str,
    dest: &Path,
    tmp_ext: &str,
    integrity: DownloadIntegrity,
    expected_magic: Option<&[u8]>,
    progress_callback: impl Fn(u64, u64) + Send + 'static,
) -> Result<(), DictationError> {
    download_to_path_with_limits(
        url,
        dest,
        tmp_ext,
        integrity,
        expected_magic,
        DownloadLimits::default(),
        progress_callback,
    )
    .await
}

/// [`download_to_path`] with explicit network bounds.
pub async fn download_to_path_with_limits(
    url: &str,
    dest: &Path,
    tmp_ext: &str,
    integrity: DownloadIntegrity,
    expected_magic: Option<&[u8]>,
    limits: DownloadLimits,
    progress_callback: impl Fn(u64, u64) + Send + 'static,
) -> Result<(), DictationError> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| {
            DictationError::ModelDownloadFailed(format!("Failed to create models directory: {e}"))
        })?;
        sweep_orphaned_tmp_files(dir);
    }

    let tmp_path = unique_tmp_path(dest, tmp_ext);

    if let Err(e) = fetch_to_tmp(
        url,
        &tmp_path,
        integrity,
        expected_magic,
        limits,
        progress_callback,
    )
    .await
    {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }

    if let Err(e) = tokio::fs::rename(&tmp_path, dest).await {
        // The download itself succeeded and was already validated — only the
        // final move failed (e.g. a cross-device rename). Clean up rather
        // than leave a uniquely-named temp file that nothing will ever look
        // for again.
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(DictationError::ModelDownloadFailed(format!(
            "Failed to rename temp file: {e}"
        )));
    }

    // The streamed bytes were verified immediately above. Cache that result
    // against the installed file's filesystem fingerprint so normal model
    // loads do not rehash multi-gigabyte artifacts. The cache
    // is also persisted as a verification stamp so later launches skip the hash
    // while the file's identity is unchanged.
    cache_verified_file(dest, integrity);

    Ok(())
}

/// Do the actual network fetch + stream-to-file + validate, in one `Result`
/// so `download_to_path` can clean up the temp file with a single arm
/// instead of repeating `let _ = remove_file(...).await;` after every `?`.
async fn fetch_to_tmp(
    url: &str,
    tmp_path: &Path,
    integrity: DownloadIntegrity,
    expected_magic: Option<&[u8]>,
    limits: DownloadLimits,
    progress_callback: impl Fn(u64, u64) + Send + 'static,
) -> Result<(), DictationError> {
    let deadline = tokio::time::Instant::now() + limits.total_deadline(integrity.size);
    let total_err = || {
        DictationError::ModelDownloadFailed(format!(
            "Download exceeded the total time limit ({}s) for a {}-byte model",
            limits.total_deadline(integrity.size).as_secs(),
            integrity.size
        ))
    };
    let idle_err = || {
        DictationError::ModelDownloadFailed(format!(
            "Download stalled: no data received for {}s",
            limits.idle_timeout.as_secs()
        ))
    };

    let client = reqwest::Client::builder()
        .connect_timeout(limits.connect_timeout)
        .build()
        .map_err(|e| DictationError::ModelDownloadFailed(format!("Download failed: {e}")))?;
    let send = tokio::time::timeout(limits.idle_timeout, client.get(url).send());
    let response = match tokio::time::timeout_at(deadline, send).await {
        Err(_) => return Err(total_err()),
        Ok(Err(_)) => return Err(idle_err()),
        Ok(Ok(r)) => r
            .map_err(|e| DictationError::ModelDownloadFailed(format!("Download failed: {e}")))?,
    };

    if !response.status().is_success() {
        return Err(DictationError::ModelDownloadFailed(format!(
            "HTTP {}: {url}",
            response.status()
        )));
    }

    let content_length = response.content_length();
    if let Some(len) = content_length {
        if len != integrity.size {
            return Err(DictationError::ModelDownloadFailed(format!(
                "Server advertised {len} bytes but the pinned manifest expects {} bytes",
                integrity.size
            )));
        }
    }
    let total_size = content_length.unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut prefix: Vec<u8> = Vec::with_capacity(MAGIC_PREFIX_LEN);
    let mut hasher = Sha256::new();

    let mut file = tokio::fs::File::create(tmp_path).await.map_err(|e| {
        DictationError::ModelDownloadFailed(format!("Failed to create temp file: {e}"))
    })?;

    let mut stream = response.bytes_stream();
    loop {
        let next = tokio::time::timeout(limits.idle_timeout, stream.next());
        let chunk = match tokio::time::timeout_at(deadline, next).await {
            Err(_) => return Err(total_err()),
            Ok(Err(_)) => return Err(idle_err()),
            Ok(Ok(None)) => break,
            Ok(Ok(Some(c))) => c.map_err(|e| {
                DictationError::ModelDownloadFailed(format!("Download error: {e}"))
            })?,
        };
        if chunk.len() as u64 > integrity.size.saturating_sub(downloaded) {
            return Err(DictationError::ModelDownloadFailed(format!(
                "Download exceeds the pinned manifest size of {} bytes",
                integrity.size
            )));
        }
        if prefix.len() < MAGIC_PREFIX_LEN {
            let take = (MAGIC_PREFIX_LEN - prefix.len()).min(chunk.len());
            prefix.extend_from_slice(&chunk[..take]);
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| DictationError::ModelDownloadFailed(format!("Write error: {e}")))?;
        downloaded += chunk.len() as u64;
        hasher.update(&chunk);
        progress_callback(downloaded, total_size);
    }

    file.flush()
        .await
        .map_err(|e| DictationError::ModelDownloadFailed(format!("Flush error: {e}")))?;
    drop(file);

    let sha256 = format!("{:x}", hasher.finalize());
    validate_download(
        &prefix,
        downloaded,
        content_length,
        &sha256,
        integrity,
        expected_magic,
    )
        .map_err(DictationError::ModelDownloadFailed)?;

    Ok(())
}

/// Pure validation of a completed-but-not-yet-renamed download. Kept free of
/// I/O so it is trivially unit-testable.
///
/// - When the server reported a non-zero `Content-Length`, the bytes
///   actually written must match it exactly — catches a truncated download
///   from a dropped connection or an early stream close.
/// - When `expected_magic` is `Some`, the file's leading bytes must match it
///   — catches an HTTP-200 body that isn't actually the model: a
///   HuggingFace git-LFS pointer stub, an HTML rate-limit/error page, or an
///   S3 XML error document would otherwise be renamed straight into
///   `ggml-*.bin` and `is_model_downloaded()` would report success forever.
pub fn validate_download(
    prefix_bytes: &[u8],
    bytes_written: u64,
    content_length: Option<u64>,
    actual_sha256: &str,
    integrity: DownloadIntegrity,
    expected_magic: Option<&[u8]>,
) -> Result<(), String> {
    if let Some(expected_len) = content_length {
        if expected_len != 0 && bytes_written != expected_len {
            return Err(format!(
                "downloaded {bytes_written} bytes but server reported Content-Length \
                 {expected_len} (truncated or interrupted download)"
            ));
        }
    }

    if bytes_written != integrity.size {
        return Err(format!(
            "downloaded {bytes_written} bytes but the pinned artifact must be exactly {} bytes",
            integrity.size
        ));
    }

    if !is_sha256(integrity.sha256) {
        return Err("internal model manifest contains an invalid SHA-256 digest".to_string());
    }

    if !actual_sha256.eq_ignore_ascii_case(integrity.sha256) {
        return Err(format!(
            "SHA-256 mismatch for downloaded model (expected {}, got {actual_sha256})",
            integrity.sha256
        ));
    }

    if let Some(magic) = expected_magic {
        if !prefix_bytes.starts_with(magic) {
            let got_len = magic.len().min(prefix_bytes.len());
            return Err(format!(
                "downloaded file does not start with the expected magic bytes {magic:02x?} \
                 (got {:02x?}) — this looks like an HTML page, a git-LFS pointer stub, or an \
                 error document rather than a model file",
                &prefix_bytes[..got_len]
            ));
        }
    }

    Ok(())
}

/// Verify an already-present artifact before handing it to native model
/// parsers. This also covers models downloaded by older Sagascript versions,
/// which predate the immutable integrity manifest.
///
/// Successful full hashes are remembered (in-process, and on unix in a
/// persistent `<file>.verified.json` stamp) so later calls skip the hash while
/// the file's identity and the manifest digest are unchanged.
pub fn verify_file(path: &Path, integrity: DownloadIntegrity) -> Result<(), DictationError> {
    verify_file_detailed(path, integrity).map_err(failure_into_error)
}

/// Like [`verify_file`] but ignores the in-process cache and the persistent
/// stamp, always re-hashing the file (and refreshing the stamp on success).
/// Intended for explicit user-requested verification such as a future
/// `sagascript engine doctor --verify-model`.
pub fn verify_file_forced(path: &Path, integrity: DownloadIntegrity) -> Result<(), DictationError> {
    verify_file_detailed_with(path, integrity, true, &sha256_of_file).map_err(failure_into_error)
}

/// [`verify_file`] / [`verify_file_forced`] with the persistent stamp stored at
/// `stamp_path` instead of beside the file. Used for files inside directory
/// packages (Core ML `.mlpackage`) that must not gain foreign files.
pub fn verify_file_with_stamp(
    path: &Path,
    stamp_path: &Path,
    integrity: DownloadIntegrity,
    force: bool,
) -> Result<(), DictationError> {
    verify_file_detailed_at(path, stamp_path, integrity, force, &sha256_of_file)
        .map_err(failure_into_error)
}

fn failure_into_error(failure: VerificationFailure) -> DictationError {
    match failure {
        VerificationFailure::IntegrityMismatch(message) => {
            DictationError::ModelDownloadFailed(message)
        }
        VerificationFailure::Other(error) => error,
    }
}

/// Verify an app-managed artifact and remove it only when its bytes are proven
/// not to match the immutable manifest. I/O failures and invalid manifest data
/// are returned without touching the file.
pub fn prepare_existing_artifact(
    path: &Path,
    integrity: DownloadIntegrity,
) -> Result<ExistingArtifact, DictationError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ExistingArtifact::Missing);
        }
        Err(error) => {
            return Err(DictationError::ModelDownloadFailed(format!(
                "Failed to inspect model before integrity verification ({}): {error}",
                path.display()
            )));
        }
    }

    match verify_file_detailed(path, integrity) {
        Ok(()) => Ok(ExistingArtifact::Verified),
        Err(VerificationFailure::Other(error)) => Err(error),
        Err(VerificationFailure::IntegrityMismatch(message)) => {
            std::fs::remove_file(path).map_err(|error| {
                DictationError::ModelDownloadFailed(format!(
                    "{message} The invalid artifact could not be removed ({}): {error}",
                    path.display()
                ))
            })?;
            remove_verification_stamp(path);
            tracing::warn!(
                "Removed invalid app-managed artifact {}; downloading a verified replacement",
                path.display()
            );
            Ok(ExistingArtifact::RemovedInvalid)
        }
    }
}

fn verify_file_detailed(
    path: &Path,
    integrity: DownloadIntegrity,
) -> Result<(), VerificationFailure> {
    verify_file_detailed_with(path, integrity, false, &sha256_of_file)
}

/// Hashing is injected so tests can observe whether a verification hashed.
type FileHasher<'a> = &'a dyn Fn(std::fs::File, &Path) -> Result<String, VerificationFailure>;

fn verify_file_detailed_with(
    path: &Path,
    integrity: DownloadIntegrity,
    force: bool,
    hasher: FileHasher<'_>,
) -> Result<(), VerificationFailure> {
    let stamp = verification_stamp_path(path);
    verify_file_detailed_at(path, &stamp, integrity, force, hasher)
}

fn verify_file_detailed_at(
    path: &Path,
    stamp_path: &Path,
    integrity: DownloadIntegrity,
    force: bool,
    hasher: FileHasher<'_>,
) -> Result<(), VerificationFailure> {
    // Validate trusted program metadata before inspecting or mutating a user
    // file. A packaging bug must never turn into an artifact deletion.
    if !is_sha256(integrity.sha256) {
        return Err(VerificationFailure::Other(
            DictationError::ModelDownloadFailed(
                "Internal model manifest contains an invalid SHA-256 digest".to_string(),
            ),
        ));
    }

    let file = std::fs::File::open(path).map_err(|e| {
        VerificationFailure::Other(DictationError::ModelDownloadFailed(format!(
            "Failed to open model for integrity verification ({}): {e}",
            path.display()
        )))
    })?;
    let metadata = file.metadata().map_err(|e| {
        VerificationFailure::Other(DictationError::ModelDownloadFailed(format!(
            "Failed to inspect model before integrity verification ({}): {e}",
            path.display()
        )))
    })?;
    if metadata.len() != integrity.size {
        return Err(VerificationFailure::IntegrityMismatch(format!(
            "Model integrity check failed for {}: expected {} bytes, got {}. Delete and re-download the model.",
            path.display(),
            integrity.size,
            metadata.len()
        )));
    }

    if !force
        && (verification_cache_matches(path, &metadata, integrity)
            || verification_stamp_matches(path, stamp_path, &metadata, integrity))
    {
        return Ok(());
    }

    let actual = hasher(file, path)?;
    if !actual.eq_ignore_ascii_case(integrity.sha256) {
        let _ = std::fs::remove_file(stamp_path);
        return Err(VerificationFailure::IntegrityMismatch(format!(
            "Model integrity check failed for {}: SHA-256 mismatch. Delete and re-download the model.",
            path.display()
        )));
    }
    // Record the identity captured *before* hashing: if the file changed
    // while it was being read, the stamp then simply fails to match later.
    cache_verified_metadata(path, &metadata, integrity);
    write_verification_stamp(path, stamp_path, &metadata, integrity);
    Ok(())
}

fn sha256_of_file(file: std::fs::File, path: &Path) -> Result<String, VerificationFailure> {
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    // Keep the multi-megabyte artifact hash buffer off the thread stack. The
    // CLI verifier runs on the process's main/Tokio thread on Windows, whose
    // stack is small enough for this allocation to trigger STATUS_STACK_OVERFLOW
    // (especially on ARM64).
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(|e| {
            VerificationFailure::Other(DictationError::ModelDownloadFailed(format!(
                "Failed while hashing model {}: {e}",
                path.display()
            )))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(unix)]
fn verification_cache() -> &'static Mutex<HashMap<PathBuf, String>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(unix)]
fn verification_cache_matches(
    path: &Path,
    metadata: &std::fs::Metadata,
    integrity: DownloadIntegrity,
) -> bool {
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    verification_cache()
        .lock()
        .is_ok_and(|cache| cache.get(&key) == Some(&integrity_cache_record(metadata, integrity)))
}

#[cfg(not(unix))]
fn verification_cache_matches(
    _path: &Path,
    _metadata: &std::fs::Metadata,
    _integrity: DownloadIntegrity,
) -> bool {
    // A size plus modification timestamp is not a reliable change token: on
    // Windows, a same-size overwrite can retain both values. Rehash on every
    // use rather than let mutable model bytes bypass integrity verification.
    false
}

/// Record a just-verified installed file (download path): stat it now and
/// remember it in-process and in the persistent stamp.
fn cache_verified_file(path: &Path, integrity: DownloadIntegrity) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    cache_verified_metadata(path, &metadata, integrity);
    write_verification_stamp(path, &verification_stamp_path(path), &metadata, integrity);
}

#[cfg(unix)]
fn cache_verified_metadata(path: &Path, metadata: &std::fs::Metadata, integrity: DownloadIntegrity) {
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if let Ok(mut cache) = verification_cache().lock() {
        cache.insert(key, integrity_cache_record(metadata, integrity));
    }
}

#[cfg(not(unix))]
fn cache_verified_metadata(
    _path: &Path,
    _metadata: &std::fs::Metadata,
    _integrity: DownloadIntegrity,
) {
    // See `verification_cache_matches`: without a trustworthy filesystem
    // identity/change token, caching a successful hash would be unsafe.
}

// ---- Persistent verification stamp -------------------------------------
//
// `<model>.verified.json` records the digest a file was verified against plus
// the filesystem identity (size, mtime, inode, ctime) observed at that time.
// A later process trusts the stamp only if every field still matches the
// file's current stat AND the stamp digest equals the manifest digest, so a
// replaced, truncated, touched or re-downloaded file always gets re-hashed.
// ctime cannot be set by ordinary userland, which is what makes a same-size,
// mtime-preserving overwrite detectable. Non-unix targets have no equivalent
// stable token in std (Windows overwrites can retain size and mtime), so they
// keep the always-hash behaviour and never write stamps.

#[cfg(unix)]
const STAMP_SCHEMA: u32 = 1;
const STAMP_SUFFIX: &str = ".verified.json";

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct VerificationStamp {
    schema: u32,
    sha256: String,
    size: u64,
    mtime_ns: u128,
    inode: u64,
    ctime_ns: i128,
    verified_by: String,
}

/// Path of the stamp that belongs to `model` (`<file name>.verified.json`).
pub fn verification_stamp_path(model: &Path) -> PathBuf {
    let mut name = model.as_os_str().to_os_string();
    name.push(STAMP_SUFFIX);
    PathBuf::from(name)
}

/// Best-effort removal of a model's verification stamp. Call whenever the
/// model file is deleted or replaced.
pub fn remove_verification_stamp(model: &Path) {
    match std::fs::remove_file(verification_stamp_path(model)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(
            "Could not remove verification stamp for {}: {error}",
            model.display()
        ),
    }
}

#[cfg(unix)]
fn stamp_for(metadata: &std::fs::Metadata, integrity: DownloadIntegrity) -> VerificationStamp {
    use std::os::unix::fs::MetadataExt;

    VerificationStamp {
        schema: STAMP_SCHEMA,
        sha256: integrity.sha256.to_ascii_lowercase(),
        size: metadata.len(),
        mtime_ns: metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or_default(),
        inode: metadata.ino(),
        ctime_ns: i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec()),
        verified_by: format!("sagascript-core {}", env!("CARGO_PKG_VERSION")),
    }
}

#[cfg(unix)]
fn verification_stamp_matches(
    path: &Path,
    stamp_path: &Path,
    metadata: &std::fs::Metadata,
    integrity: DownloadIntegrity,
) -> bool {
    let bytes = match std::fs::read(stamp_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(error) => {
            tracing::debug!("Cannot read verification stamp {}: {error}", stamp_path.display());
            return false;
        }
    };
    let Ok(stamp) = serde_json::from_slice::<VerificationStamp>(&bytes) else {
        tracing::debug!("Ignoring corrupt verification stamp {}", stamp_path.display());
        return false;
    };
    let current = stamp_for(metadata, integrity);
    let matches = stamp.schema == STAMP_SCHEMA
        && stamp.sha256.eq_ignore_ascii_case(integrity.sha256)
        && stamp.size == current.size
        && stamp.mtime_ns == current.mtime_ns
        && stamp.inode == current.inode
        && stamp.ctime_ns == current.ctime_ns;
    if matches {
        // Promote to the in-process cache so this process never re-reads it.
        cache_verified_metadata(path, metadata, integrity);
    }
    matches
}

#[cfg(not(unix))]
fn verification_stamp_matches(
    _path: &Path,
    _stamp_path: &Path,
    _metadata: &std::fs::Metadata,
    _integrity: DownloadIntegrity,
) -> bool {
    false
}

/// Atomically (temp file + rename in the same directory) write the stamp.
/// Never fails verification: errors are logged and swallowed.
#[cfg(unix)]
fn write_verification_stamp(
    _path: &Path,
    stamp_path: &Path,
    metadata: &std::fs::Metadata,
    integrity: DownloadIntegrity,
) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let result = (|| -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(&stamp_for(metadata, integrity))
            .map_err(std::io::Error::other)?;
        let tmp = unique_tmp_path(stamp_path, "json");
        let write = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            file.write_all(&json)?;
            file.sync_all()
        })();
        if let Err(error) = write.and_then(|()| std::fs::rename(&tmp, stamp_path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(error);
        }
        Ok(())
    })();
    if let Err(error) = result {
        tracing::debug!(
            "Could not write verification stamp {}: {error}",
            stamp_path.display()
        );
    }
}

#[cfg(not(unix))]
fn write_verification_stamp(
    _path: &Path,
    _stamp_path: &Path,
    _metadata: &std::fs::Metadata,
    _integrity: DownloadIntegrity,
) {
}

#[cfg(unix)]
fn integrity_cache_record(metadata: &std::fs::Metadata, integrity: DownloadIntegrity) -> String {
    use std::os::unix::fs::MetadataExt;

    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();

    format!(
        "v1\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
        integrity.sha256,
        metadata.len(),
        modified,
        metadata.dev(),
        metadata.ino(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

/// Build a temp-file path in the same directory as `dest`, unique per call
/// so two concurrent downloads (e.g. the user queues two models back to
/// back) never write into the same temp file and interleave bytes into a
/// corrupt result. `tmp_ext` (e.g. `"bin"`/`"onnx"`) is folded in purely so a
/// human staring at the directory can tell what a leftover file was for.
fn unique_tmp_path(dest: &Path, tmp_ext: &str) -> PathBuf {
    let unique = uuid::Uuid::new_v4();
    dest.with_extension(format!("{tmp_ext}.{unique}.tmp"))
}

/// Best-effort removal of `.tmp` files left behind by a crashed or killed
/// download. Needed because unique-per-invocation temp names (above) lose
/// the old fixed-name behavior where a retried download would just
/// overwrite/truncate its own stale leftover — now nothing reclaims an
/// orphan on its own. Only sweeps files older than `ORPHAN_TMP_MAX_AGE`, so
/// a fresh temp file belonging to a *different*, concurrently-running
/// download in the same models directory is never touched. Errors (missing
/// dir, permissions) are swallowed: this is opportunistic housekeeping, not
/// something a download should fail over.
fn sweep_orphaned_tmp_files(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("tmp") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age > ORPHAN_TMP_MAX_AGE {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SHA: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    fn integrity(size: u64) -> DownloadIntegrity {
        DownloadIntegrity {
            sha256: TEST_SHA,
            size,
        }
    }

    fn temp_test_dir() -> PathBuf {
        std::env::temp_dir().join(format!("sagascript-download-test-{}", uuid::Uuid::new_v4()))
    }

    // -- validate_download --

    #[test]
    fn accepts_valid_ggml_prefix_with_matching_length() {
        let prefix = [0x6c, 0x6d, 0x67, 0x67, 0x0a, 0x00, 0x00, 0x00];
        assert!(
            validate_download(
                &prefix,
                142_000_000,
                Some(142_000_000),
                TEST_SHA,
                integrity(142_000_000),
                Some(&GGML_MAGIC),
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_html_rate_limit_page() {
        let prefix = b"<!DOCTYPE html><html><head><title>429</title>";
        let err =
            validate_download(
                prefix,
                prefix.len() as u64,
                Some(prefix.len() as u64),
                TEST_SHA,
                integrity(prefix.len() as u64),
                Some(&GGML_MAGIC),
            )
            .unwrap_err();
        assert!(err.contains("magic"), "error should mention magic bytes: {err}");
    }

    #[test]
    fn rejects_git_lfs_pointer_file() {
        let prefix =
            b"version https://git-lfs.github.com/spec/v1\noid sha256:deadbeef\nsize 142000000\n";
        let err = validate_download(
            prefix,
            prefix.len() as u64,
            None,
            TEST_SHA,
            integrity(prefix.len() as u64),
            Some(&GGML_MAGIC),
        )
        .unwrap_err();
        assert!(err.contains("magic"), "error should mention magic bytes: {err}");
    }

    #[test]
    fn rejects_length_mismatch_even_without_magic_check() {
        // Simulates a dropped connection: server promised 1000 bytes, only 400 arrived.
        let prefix = b"partial-body-bytes";
        let err = validate_download(prefix, 400, Some(1000), TEST_SHA, integrity(400), None)
            .unwrap_err();
        assert!(
            err.contains("400") && err.contains("1000"),
            "error should name both byte counts: {err}"
        );
    }

    #[test]
    fn skips_length_check_when_content_length_unknown_or_zero() {
        let prefix = GGML_MAGIC;
        assert!(
            validate_download(&prefix, 999, None, TEST_SHA, integrity(999), Some(&GGML_MAGIC))
                .is_ok()
        );
        assert!(
            validate_download(
                &prefix,
                999,
                Some(0),
                TEST_SHA,
                integrity(999),
                Some(&GGML_MAGIC),
            )
            .is_ok()
        );
    }

    #[test]
    fn skips_magic_check_when_none_e_g_onnx() {
        // ONNX files are protobuf-encoded with no fixed magic; only length is checked.
        let onnx_like_prefix = [0x08, 0x07, 0x12, 0x07];
        assert!(
            validate_download(
                &onnx_like_prefix,
                27_000_000,
                Some(27_000_000),
                TEST_SHA,
                integrity(27_000_000),
                None,
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_exact_size_mismatch_even_when_server_length_matches() {
        let err = validate_download(b"model", 512, Some(512), TEST_SHA, integrity(1024), None)
            .unwrap_err();
        assert!(err.contains("exactly 1024"), "error: {err}");
    }

    #[test]
    fn rejects_sha256_mismatch() {
        let err = validate_download(
            b"model",
            512,
            Some(512),
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            integrity(512),
            None,
        )
        .unwrap_err();
        assert!(err.contains("SHA-256 mismatch"), "error: {err}");
    }

    #[test]
    fn verify_file_hashes_existing_artifact_before_use() {
        let dir = temp_test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model.bin");
        std::fs::write(&path, b"verified model").unwrap();
        let expected = DownloadIntegrity {
            sha256: "6c736b3dfa943bf4e7c61df78d1dfcad9a3d8b56369f0559670497b19127e74d",
            size: 14,
        };
        assert!(verify_file(&path, expected).is_ok());
        std::fs::write(&path, b"tampered model").unwrap();
        assert!(verify_file(&path, expected).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    // -- persistent verification stamp (unix only) --

    #[cfg(unix)]
    mod stamp {
        use super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        const BODY: &[u8] = b"verified model";
        const SHA: &str = "6c736b3dfa943bf4e7c61df78d1dfcad9a3d8b56369f0559670497b19127e74d";
        const INTEGRITY: DownloadIntegrity = DownloadIntegrity { sha256: SHA, size: 14 };

        /// Run one verification with a counting hasher; returns hash count.
        fn run(path: &Path, integrity: DownloadIntegrity, force: bool) -> (usize, bool) {
            let count = AtomicUsize::new(0);
            let hasher = |file, p: &Path| {
                count.fetch_add(1, Ordering::SeqCst);
                sha256_of_file(file, p)
            };
            let ok = verify_file_detailed_with(path, integrity, force, &hasher).is_ok();
            (count.load(Ordering::SeqCst), ok)
        }

        /// Fresh dir + model file. Each test uses its own path, so the
        /// process-wide cache (keyed by path) never leaks between tests; to
        /// exercise the *stamp* we clear that cache entry before each run.
        fn setup() -> (PathBuf, PathBuf) {
            let dir = temp_test_dir();
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("model.bin");
            std::fs::write(&path, BODY).unwrap();
            (dir, path)
        }

        fn forget_process_cache(path: &Path) {
            let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            verification_cache().lock().unwrap().remove(&key);
        }

        fn stamped(path: &Path) -> (PathBuf, PathBuf) {
            let (count, ok) = run(path, INTEGRITY, false);
            assert!((count, ok) == (1, true));
            forget_process_cache(path);
            (path.to_path_buf(), verification_stamp_path(path))
        }

        #[test]
        fn first_verify_hashes_and_writes_stamp_then_skips() {
            use std::os::unix::fs::PermissionsExt;
            let (dir, path) = setup();
            assert_eq!(run(&path, INTEGRITY, false), (1, true));
            let stamp_path = verification_stamp_path(&path);
            let stamp: VerificationStamp =
                serde_json::from_slice(&std::fs::read(&stamp_path).unwrap()).unwrap();
            assert_eq!(stamp.schema, 1);
            assert_eq!(stamp.sha256, SHA);
            assert_eq!(stamp.size, 14);
            let mode = std::fs::metadata(&stamp_path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);

            // Same process: in-process cache. New "process": stamp only.
            assert_eq!(run(&path, INTEGRITY, false), (0, true));
            forget_process_cache(&path);
            assert_eq!(run(&path, INTEGRITY, false), (0, true));
            // Forced verification always hashes.
            assert_eq!(run(&path, INTEGRITY, true), (1, true));
            assert!(verify_file_forced(&path, INTEGRITY).is_ok());
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn touched_mtime_rehashes() {
            let (dir, path) = setup();
            stamped(&path);
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_modified(SystemTime::now() + Duration::from_secs(5)).unwrap();
            drop(file);
            assert_eq!(run(&path, INTEGRITY, false), (1, true));
            forget_process_cache(&path);
            assert_eq!(run(&path, INTEGRITY, false), (0, true), "stamp was rewritten");
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn changed_size_rehashes_and_fails() {
            let (dir, path) = setup();
            stamped(&path);
            std::fs::write(&path, b"longer verified model").unwrap();
            // Size mismatch is rejected before hashing.
            assert_eq!(run(&path, INTEGRITY, false), (0, false));
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn replaced_file_new_inode_rehashes() {
            let (dir, path) = setup();
            stamped(&path);
            let replacement = dir.join("other.bin");
            std::fs::write(&replacement, BODY).unwrap();
            std::fs::rename(&replacement, &path).unwrap();
            assert_eq!(run(&path, INTEGRITY, false), (1, true));
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn same_size_tampering_is_caught() {
            let (dir, path) = setup();
            stamped(&path);
            std::fs::write(&path, b"tampered model").unwrap();
            assert_eq!(run(&path, INTEGRITY, false), (1, false));
            assert!(!verification_stamp_path(&path).exists(), "stale stamp removed");
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn stamp_for_a_different_expected_sha_is_ignored() {
            let (dir, path) = setup();
            stamped(&path);
            let other = DownloadIntegrity {
                sha256: "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                size: 14,
            };
            assert_eq!(run(&path, other, false), (1, false));
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn corrupt_stamp_rehashes_and_is_rewritten() {
            let (dir, path) = setup();
            let (_, stamp_path) = stamped(&path);
            std::fs::write(&stamp_path, b"{not json").unwrap();
            assert_eq!(run(&path, INTEGRITY, false), (1, true));
            forget_process_cache(&path);
            assert_eq!(run(&path, INTEGRITY, false), (0, true));
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn unwritable_directory_does_not_fail_verification() {
            use std::os::unix::fs::PermissionsExt;
            let (dir, path) = setup();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
            let result = run(&path, INTEGRITY, false);
            let wrote = verification_stamp_path(&path).exists();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(result, (1, true));
            // (Running as root would bypass the mode bits.)
            assert!(!wrote || unsafe_is_root());
            let _ = std::fs::remove_dir_all(dir);
        }

        fn unsafe_is_root() -> bool {
            std::env::var("USER").is_ok_and(|user| user == "root")
        }

        #[test]
        fn concurrent_verifiers_leave_a_valid_stamp() {
            let (dir, path) = setup();
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let path = path.clone();
                    std::thread::spawn(move || {
                        // Bypass the shared in-process cache so all race on the stamp.
                        verify_file_forced(&path, INTEGRITY).is_ok()
                    })
                })
                .collect();
            for handle in handles {
                assert!(handle.join().unwrap());
            }
            forget_process_cache(&path);
            assert_eq!(run(&path, INTEGRITY, false), (0, true));
            let leftovers = std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
                .count();
            assert_eq!(leftovers, 0);
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn removing_a_stamp_is_idempotent() {
            let (dir, path) = setup();
            stamped(&path);
            remove_verification_stamp(&path);
            remove_verification_stamp(&path);
            assert!(!verification_stamp_path(&path).exists());
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn verify_file_works_on_a_small_stack() {
        let dir = temp_test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model.bin");
        std::fs::write(&path, b"verified model").unwrap();
        let expected = DownloadIntegrity {
            sha256: "6c736b3dfa943bf4e7c61df78d1dfcad9a3d8b56369f0559670497b19127e74d",
            size: 14,
        };

        // This is intentionally below the verifier's former 1 MiB stack
        // buffer. A regression therefore fails on Windows with a stack
        // overflow instead of depending on the host process stack size.
        let result = std::thread::Builder::new()
            .name("sagascript-small-stack-verification".to_string())
            .stack_size(256 * 1024)
            .spawn({
                let path = path.clone();
                move || verify_file(&path, expected)
            })
            .unwrap()
            .join()
            .unwrap();

        assert!(result.is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn prepare_existing_artifact_keeps_verified_file() {
        let dir = temp_test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model.bin");
        std::fs::write(&path, b"verified model").unwrap();
        let expected = DownloadIntegrity {
            sha256: "6c736b3dfa943bf4e7c61df78d1dfcad9a3d8b56369f0559670497b19127e74d",
            size: 14,
        };

        assert_eq!(
            prepare_existing_artifact(&path, expected).unwrap(),
            ExistingArtifact::Verified
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"verified model");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn prepare_existing_artifact_removes_same_size_hash_mismatch() {
        let dir = temp_test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model.bin");
        std::fs::write(&path, b"tampered model").unwrap();
        let expected = DownloadIntegrity {
            sha256: "6c736b3dfa943bf4e7c61df78d1dfcad9a3d8b56369f0559670497b19127e74d",
            size: 14,
        };

        assert_eq!(
            prepare_existing_artifact(&path, expected).unwrap(),
            ExistingArtifact::RemovedInvalid
        );
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn prepare_existing_artifact_preserves_file_for_invalid_manifest() {
        let dir = temp_test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model.bin");
        std::fs::write(&path, b"untrusted bytes").unwrap();
        let invalid_manifest = DownloadIntegrity {
            sha256: "not-a-sha256",
            size: 1,
        };

        assert!(prepare_existing_artifact(&path, invalid_manifest).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"untrusted bytes");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn prepare_existing_artifact_preserves_path_on_io_error() {
        let dir = temp_test_dir();
        let path = dir.join("model.bin");
        std::fs::create_dir_all(&path).unwrap();
        let expected = DownloadIntegrity {
            sha256: TEST_SHA,
            size: std::fs::metadata(&path).unwrap().len(),
        };

        assert!(prepare_existing_artifact(&path, expected).is_err());
        assert!(path.is_dir(), "generic I/O failure must not delete the path");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn prepare_existing_artifact_preserves_dangling_symlink() {
        use std::os::unix::fs::symlink;

        let dir = temp_test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model.bin");
        symlink(dir.join("missing-target"), &path).unwrap();

        assert!(prepare_existing_artifact(&path, integrity(1)).is_err());
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink(),
            "an open failure must not remove or replace the existing path"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn serve_once(body: &'static [u8]) -> String {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });
        format!("http://{address}/model.bin")
    }

    #[tokio::test]
    async fn invalid_existing_artifact_is_replaced_by_verified_download() {
        let dir = temp_test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model.bin");
        std::fs::write(&path, b"tampered model").unwrap();
        let expected = DownloadIntegrity {
            sha256: "6c736b3dfa943bf4e7c61df78d1dfcad9a3d8b56369f0559670497b19127e74d",
            size: 14,
        };

        assert_eq!(
            prepare_existing_artifact(&path, expected).unwrap(),
            ExistingArtifact::RemovedInvalid
        );
        download_to_path(
            &serve_once(b"verified model"),
            &path,
            "bin",
            expected,
            None,
            |_, _| {},
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"verified model");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn failed_replacement_never_installs_partial_artifact() {
        let dir = temp_test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model.bin");
        std::fs::write(&path, b"tampered model").unwrap();
        let expected = DownloadIntegrity {
            sha256: "6c736b3dfa943bf4e7c61df78d1dfcad9a3d8b56369f0559670497b19127e74d",
            size: 14,
        };

        prepare_existing_artifact(&path, expected).unwrap();
        assert!(
            download_to_path(
                &serve_once(b"partial"),
                &path,
                "bin",
                expected,
                None,
                |_, _| {},
            )
            .await
            .is_err()
        );
        assert!(!path.exists());
        assert!(
            std::fs::read_dir(&dir).unwrap().next().is_none(),
            "failed replacement must clean its unique temp file"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    // -- bounded downloads --

    #[derive(Clone, Copy)]
    enum Serve {
        /// Content-Length header (None = omitted), then body, then optional stall.
        Body { declared: Option<usize>, body_len: usize, stall: Duration },
    }

    fn serve(mode: Serve) -> String {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let Serve::Body { declared, body_len, stall } = mode;
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            let mut head = String::from("HTTP/1.1 200 OK\r\nConnection: close\r\n");
            if let Some(d) = declared {
                head.push_str(&format!("Content-Length: {d}\r\n"));
            }
            head.push_str("\r\n");
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&vec![b'a'; body_len]);
            let _ = stream.flush();
            std::thread::sleep(stall);
        });
        format!("http://{address}/model.bin")
    }

    fn fast_limits() -> DownloadLimits {
        DownloadLimits {
            connect_timeout: Duration::from_millis(500),
            idle_timeout: Duration::from_millis(300),
            total_base: Duration::from_secs(5),
            min_bytes_per_sec: 1024,
        }
    }

    async fn bounded(mode: Serve, size: u64) -> (Result<(), DictationError>, Duration, PathBuf) {
        let dir = temp_test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model.bin");
        let integrity = DownloadIntegrity {
            // Never reached on the failure paths below.
            sha256: "6c736b3dfa943bf4e7c61df78d1dfcad9a3d8b56369f0559670497b19127e74d",
            size,
        };
        let start = std::time::Instant::now();
        let result = download_to_path_with_limits(
            &serve(mode),
            &path,
            "bin",
            integrity,
            None,
            fast_limits(),
            |_, _| {},
        )
        .await;
        (result, start.elapsed(), dir)
    }

    fn assert_clean(dir: &Path) {
        assert!(
            std::fs::read_dir(dir).unwrap().next().is_none(),
            "failed download must leave no partial or temp file"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn oversized_body_without_length_fails_promptly_and_leaves_nothing() {
        let (r, took, dir) = bounded(
            Serve::Body { declared: None, body_len: 4 * 1024 * 1024, stall: Duration::from_secs(3) },
            14,
        )
        .await;
        let err = r.unwrap_err().to_string();
        assert!(err.contains("exceeds the pinned manifest size"), "{err}");
        assert!(took < Duration::from_secs(2), "{took:?}");
        assert_clean(&dir);
    }

    #[tokio::test]
    async fn wrong_content_length_is_rejected_before_any_write() {
        let (r, took, dir) = bounded(
            Serve::Body { declared: Some(1_000_000), body_len: 14, stall: Duration::from_secs(3) },
            14,
        )
        .await;
        let err = r.unwrap_err().to_string();
        assert!(err.contains("Server advertised 1000000"), "{err}");
        assert!(took < Duration::from_secs(2), "{took:?}");
        assert_clean(&dir);
    }

    #[tokio::test]
    async fn stalled_body_fails_within_idle_deadline_and_leaves_nothing() {
        let (r, took, dir) = bounded(
            Serve::Body { declared: Some(14), body_len: 5, stall: Duration::from_secs(4) },
            14,
        )
        .await;
        let err = r.unwrap_err().to_string();
        assert!(err.contains("stalled"), "{err}");
        assert!(took < Duration::from_secs(2), "{took:?}");
        assert_clean(&dir);
    }

    #[tokio::test]
    async fn valid_pinned_download_succeeds_with_limits() {
        let dir = temp_test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("model.bin");
        let expected = DownloadIntegrity {
            sha256: "6c736b3dfa943bf4e7c61df78d1dfcad9a3d8b56369f0559670497b19127e74d",
            size: 14,
        };
        download_to_path_with_limits(
            &serve_once(b"verified model"),
            &path,
            "bin",
            expected,
            None,
            fast_limits(),
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"verified model");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn total_deadline_scales_with_size() {
        let l = DownloadLimits::default();
        assert!(l.total_deadline(3 * 1024 * 1024 * 1024) > Duration::from_secs(6 * 3600));
        assert!(l.total_deadline(0) >= Duration::from_secs(300));
    }

    // -- unique_tmp_path --

    #[test]
    fn unique_tmp_path_is_unique_and_carries_marker() {
        let dest = Path::new("/models/ggml-base.bin");
        let a = unique_tmp_path(dest, "bin");
        let b = unique_tmp_path(dest, "bin");
        assert_ne!(a, b, "two calls must never collide");
        for p in [&a, &b] {
            assert_eq!(p.parent(), dest.parent());
            let name = p.file_name().unwrap().to_str().unwrap();
            assert!(name.starts_with("ggml-base.bin."), "name: {name}");
            assert!(name.ends_with(".tmp"), "name: {name}");
        }
    }

    // -- sweep_orphaned_tmp_files --

    #[test]
    fn sweep_removes_only_stale_tmp_files() {
        let dir = temp_test_dir();
        std::fs::create_dir_all(&dir).unwrap();

        let stale = dir.join("stale.bin.tmp");
        std::fs::write(&stale, b"leftover").unwrap();
        // Backdate mtime well past the sweep threshold using std's stable
        // `File::set_times` (no extra dependency needed).
        let ancient = SystemTime::now() - (ORPHAN_TMP_MAX_AGE + Duration::from_secs(60));
        let file = std::fs::OpenOptions::new().write(true).open(&stale).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(ancient)).unwrap();

        let fresh = dir.join("fresh.bin.tmp");
        std::fs::write(&fresh, b"in progress").unwrap();

        let keep = dir.join("keep.bin"); // not a .tmp — must never be touched
        std::fs::write(&keep, b"real model").unwrap();

        sweep_orphaned_tmp_files(&dir);

        assert!(!stale.exists(), "stale tmp file should be swept");
        assert!(
            fresh.exists(),
            "fresh tmp file must survive — it might belong to a concurrent download"
        );
        assert!(keep.exists(), "non-tmp file must never be touched");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweep_tolerates_missing_directory() {
        // Must not panic if the models dir doesn't exist yet (fresh install).
        sweep_orphaned_tmp_files(&temp_test_dir());
    }
}
