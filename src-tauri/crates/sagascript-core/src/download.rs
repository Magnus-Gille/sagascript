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
//!   directory before it starts;
//! - the temp file is fsynced before the rename, so a crash cannot leave the
//!   installed name (and a durable verification stamp) over unwritten data,
//!   and every blocking filesystem step runs on Tokio's blocking pool rather
//!   than on an async executor thread (#368).

use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

#[cfg(unix)]
use std::collections::HashMap;
#[cfg(unix)]
use std::sync::OnceLock;

use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::error::DictationError;
use crate::sync::lock_or_recover;

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
///   multi-GB download on a slow but moving line is allowed to finish, while a
///   server trickling bytes below the floor still gets bounded (about 6.8 h
///   for a 3 GB model). The guarantee is a minimum *average* rate of
///   `min_bytes_per_sec` plus `total_base`, not that arbitrarily slow
///   downloads always finish.
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
    download_with(
        url,
        dest,
        tmp_ext,
        integrity,
        expected_magic,
        progress_callback,
        &DownloadEnv::production(limits),
    )
    .await
}

/// File operations of a download that tests need to observe (#368): creating
/// the destination directory, making the temp file durable and the final
/// rename. [`RealFs`] is the only production implementation.
trait DownloadFs: Send + Sync {
    fn create_dir_all(&self, dir: &Path) -> std::io::Result<()>;
    fn sync_file(&self, file: &std::fs::File, path: &Path) -> std::io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()>;
}

struct RealFs;

impl DownloadFs for RealFs {
    fn create_dir_all(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)
    }

    fn sync_file(&self, file: &std::fs::File, _path: &Path) -> std::io::Result<()> {
        file.sync_all()
    }

    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::rename(from, to)
    }
}

/// Network bounds and injectable side effects of one download.
struct DownloadEnv {
    limits: DownloadLimits,
    clients: &'static ClientCache,
    fs: Arc<dyn DownloadFs>,
}

impl DownloadEnv {
    fn production(limits: DownloadLimits) -> Self {
        Self {
            limits,
            clients: &SHARED_CLIENTS,
            fs: Arc::new(RealFs),
        }
    }
}

static SHARED_CLIENTS: ClientCache = ClientCache::new();

/// One HTTP client per process for model downloads (#368), instead of one
/// per download: building a client loads the platform TLS configuration and
/// the system proxy settings.
///
/// The client keeps no idle connections ([`build_client`]). The CLI runs
/// downloads on short-lived Tokio runtimes (`download_vad_model` under a fresh
/// runtime's `block_on`), and a pooled connection is driven by a task on the
/// runtime that opened it, so a later runtime must never be handed one. Each
/// model download is one long transfer, so pooling would save nothing anyway.
struct ClientCache {
    /// One client per connect timeout (production only uses the default;
    /// tests add a short one). Each update is a single `push`, so the list
    /// stays consistent if a panic poisons the lock (ADR 0005).
    clients: Mutex<Vec<(Duration, reqwest::Client)>>,
    builds: AtomicUsize,
}

impl ClientCache {
    const fn new() -> Self {
        Self {
            clients: Mutex::new(Vec::new()),
            builds: AtomicUsize::new(0),
        }
    }

    async fn client(&self, connect_timeout: Duration) -> Result<reqwest::Client, DictationError> {
        if let Some(client) = self.cached(connect_timeout) {
            return Ok(client);
        }
        // Loading the TLS configuration reads system files: off the executor.
        let client = run_blocking(move || build_client(connect_timeout)).await?;
        let mut clients = lock_or_recover(&self.clients, "download.http_clients");
        // A concurrent first download may have stored one meanwhile; keep it.
        if let Some((_, existing)) = clients.iter().find(|(t, _)| *t == connect_timeout) {
            return Ok(existing.clone());
        }
        clients.push((connect_timeout, client.clone()));
        let builds = self.builds.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::debug!(builds, "Built an HTTP client for model downloads");
        Ok(client)
    }

    fn cached(&self, connect_timeout: Duration) -> Option<reqwest::Client> {
        lock_or_recover(&self.clients, "download.http_clients")
            .iter()
            .find(|(timeout, _)| *timeout == connect_timeout)
            .map(|(_, client)| client.clone())
    }
}

async fn download_with(
    url: &str,
    dest: &Path,
    tmp_ext: &str,
    integrity: DownloadIntegrity,
    expected_magic: Option<&[u8]>,
    progress_callback: impl Fn(u64, u64) + Send + 'static,
    env: &DownloadEnv,
) -> Result<(), DictationError> {
    if let Some(dir) = dest.parent() {
        let (fs, dir) = (Arc::clone(&env.fs), dir.to_path_buf());
        run_blocking(move || {
            fs.create_dir_all(&dir).map_err(|e| {
                DictationError::ModelDownloadFailed(format!(
                    "Failed to create models directory: {e}"
                ))
            })?;
            sweep_orphaned_tmp_files(&dir);
            Ok(())
        })
        .await?;
    }

    let client = env.clients.client(env.limits.connect_timeout).await?;
    let tmp_path = unique_tmp_path(dest, tmp_ext);
    // The guard is created together with the file (inside the blocking
    // creation task), so it removes the file on error, cancellation, panic, or
    // an abandoned creation; it moves into the install step below.
    let (file, guard) = fetch_to_tmp(
        &client,
        url,
        &tmp_path,
        integrity,
        expected_magic,
        env.limits,
        progress_callback,
    )
    .await?;

    let (fs, dest) = (Arc::clone(&env.fs), dest.to_path_buf());
    run_blocking_cancellable(move |cancel| {
        install_tmp_file(fs.as_ref(), file, guard, &dest, integrity, cancel)
    })
    .await
}

/// Make the validated temp file durable, then atomically move it into place
/// and record its verification stamp.
///
/// The order is the point (#368): the data is fsynced *before* the rename, so
/// a crash can never leave `dest` naming a file whose blocks were not written
/// yet, with a durable stamp (written last) vouching for it. If only the
/// final move fails (e.g. a cross-device rename), or the download was
/// cancelled before it, `guard` removes the temp file.
fn install_tmp_file(
    fs: &dyn DownloadFs,
    file: std::fs::File,
    mut guard: TmpFileGuard,
    dest: &Path,
    integrity: DownloadIntegrity,
    cancel: &CancelFlag,
) -> Result<(), DictationError> {
    fs.sync_file(&file, &guard.path).map_err(|e| {
        DictationError::ModelDownloadFailed(format!("Failed to sync temp file: {e}"))
    })?;
    drop(file);
    // The last point at which a cancelled download still installs nothing.
    cancel.check("Download")?;
    fs.rename(&guard.path, dest).map_err(|e| {
        DictationError::ModelDownloadFailed(format!("Failed to rename temp file: {e}"))
    })?;
    guard.disarm();

    // The streamed bytes were verified before the install. Cache that result
    // against the installed file's filesystem fingerprint so normal model
    // loads do not rehash multi-gigabyte artifacts. The cache
    // is also persisted as a verification stamp so later launches skip the hash
    // while the file's identity is unchanged.
    cache_verified_file(dest, integrity);
    Ok(())
}

/// Run blocking file work (directory scans, hashing, fsync, renames) on
/// Tokio's blocking pool so it never stalls an async executor thread (#368).
///
/// Dropping the returned future does not stop `work`; a blocking closure runs
/// to completion. Work that must notice cancellation uses
/// [`run_blocking_cancellable`].
pub(crate) async fn run_blocking<T, F>(work: F) -> Result<T, DictationError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, DictationError> + Send + 'static,
{
    tokio::task::spawn_blocking(work).await.map_err(|e| {
        DictationError::ModelDownloadFailed(format!("Background file task failed: {e}"))
    })?
}

/// Set once the async caller of a blocking step has gone away (its future was
/// dropped), so the blocking work can stop before an irreversible step.
#[derive(Clone, Default)]
struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// `Err` once cancelled. Nobody awaits a cancelled step, so the message
    /// (`what` names the step) only matters for logs and tests.
    fn check(&self, what: &str) -> Result<(), DictationError> {
        if self.is_cancelled() {
            return Err(DictationError::ModelDownloadFailed(format!(
                "{what} was cancelled"
            )));
        }
        Ok(())
    }
}

/// Sets the flag when dropped, i.e. when the awaiting future completes or is
/// cancelled. Setting it after the work finished is harmless.
struct CancelOnDrop(CancelFlag);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// [`run_blocking`] for work that should stop early when its caller is
/// cancelled: `work` receives a [`CancelFlag`] that is set as soon as the
/// returned future is dropped.
async fn run_blocking_cancellable<T, F>(work: F) -> Result<T, DictationError>
where
    T: Send + 'static,
    F: FnOnce(&CancelFlag) -> Result<T, DictationError> + Send + 'static,
{
    let cancel = CancelFlag::default();
    let _cancel_on_drop = CancelOnDrop(cancel.clone());
    run_blocking(move || work(&cancel)).await
}

/// Maximum number of HTTP redirects followed (matches reqwest's default).
const MAX_REDIRECTS: usize = 10;

/// Owns the temporary download file: removes it on drop (error return,
/// cancellation of the future, or panic unwinding) unless disarmed after a
/// successful rename. Removal is a synchronous best-effort `remove_file`; on
/// Unix an in-flight blocking write to the unlinked inode is harmless.
struct TmpFileGuard {
    path: PathBuf,
    armed: bool,
}

impl TmpFileGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TmpFileGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn total_err(limits: DownloadLimits, size: u64) -> DictationError {
    DictationError::ModelDownloadFailed(format!(
        "Download exceeded the total time limit ({}s) for a {size}-byte model",
        limits.total_deadline(size).as_secs(),
    ))
}

/// Do the actual network fetch + stream-to-file + validate. The temp file is
/// cleaned up by the caller's [`TmpFileGuard`]. The total deadline wraps every
/// asynchronous step (headers, redirects, body, writes, flush) and is also
/// checked explicitly between chunks and before validation, because the
/// synchronous progress callback and hashing cannot be pre-empted.
async fn fetch_to_tmp(
    client: &reqwest::Client,
    url: &str,
    tmp_path: &Path,
    integrity: DownloadIntegrity,
    expected_magic: Option<&[u8]>,
    limits: DownloadLimits,
    progress_callback: impl Fn(u64, u64) + Send + 'static,
) -> Result<(std::fs::File, TmpFileGuard), DictationError> {
    let deadline = tokio::time::Instant::now() + limits.total_deadline(integrity.size);
    let fetch = async {
        let response = open_response(client, url, limits).await?;
        stream_to_tmp(
            response,
            tmp_path,
            integrity,
            expected_magic,
            limits,
            deadline,
            progress_callback,
        )
        .await
    };
    match tokio::time::timeout_at(deadline, fetch).await {
        Ok(result) => result,
        Err(_) => Err(total_err(limits, integrity.size)),
    }
}

fn download_failed(e: reqwest::Error) -> DictationError {
    DictationError::ModelDownloadFailed(format!("Download failed: {e}"))
}

fn idle_err(limits: DownloadLimits) -> DictationError {
    DictationError::ModelDownloadFailed(format!(
        "Download stalled: no data received for {}s",
        limits.idle_timeout.as_secs()
    ))
}

/// HTTP client for model downloads. Redirects are followed manually (see
/// [`open_response`]); no connection is kept idle (see [`ClientCache`]).
fn build_client(connect_timeout: Duration) -> Result<reqwest::Client, DictationError> {
    reqwest::Client::builder()
        // The platform TLS stack, whatever else in the binary enables
        // rustls (the macOS updater does); see this crate's Cargo.toml.
        .tls_backend_native()
        .connect_timeout(connect_timeout)
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(0)
        .build()
        .map_err(download_failed)
}

/// Send the request and follow redirects up to a successful response.
async fn open_response(
    client: &reqwest::Client,
    url: &str,
    limits: DownloadLimits,
) -> Result<reqwest::Response, DictationError> {
    // Redirects are followed manually so the header-phase idle timeout applies
    // per hop rather than to the whole chain; the total deadline is shared.
    let mut current = reqwest::Url::parse(url)
        .map_err(|e| DictationError::ModelDownloadFailed(format!("Invalid download URL: {e}")))?;
    let mut hops = 0;
    let response = loop {
        let response =
            tokio::time::timeout(limits.idle_timeout, client.get(current.clone()).send())
                .await
                .map_err(|_| idle_err(limits))?
                .map_err(download_failed)?;
        if !response.status().is_redirection() {
            break response;
        }
        if !is_followed_redirect(response.status()) {
            return Err(DictationError::ModelDownloadFailed(format!(
                "Unsupported redirect status {} from {current}",
                response.status()
            )));
        }
        let Some(location) = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
        else {
            break response;
        };
        hops += 1;
        if hops > MAX_REDIRECTS {
            return Err(DictationError::ModelDownloadFailed(format!(
                "Too many redirects (more than {MAX_REDIRECTS})"
            )));
        }
        current =
            redirect_target(&current, location).map_err(DictationError::ModelDownloadFailed)?;
    };

    if !response.status().is_success() {
        return Err(DictationError::ModelDownloadFailed(format!(
            "HTTP {}: {url}",
            response.status()
        )));
    }
    Ok(response)
}

/// Stream a successful response into a new temp file at `tmp_path` and
/// validate it. Returns the open, flushed file and the guard that owns it.
async fn stream_to_tmp(
    response: reqwest::Response,
    tmp_path: &Path,
    integrity: DownloadIntegrity,
    expected_magic: Option<&[u8]>,
    limits: DownloadLimits,
    deadline: tokio::time::Instant,
    // Called synchronously after every chunk; it must be cheap and
    // non-blocking, since it holds up the transfer (and cannot be pre-empted).
    progress_callback: impl Fn(u64, u64) + Send + 'static,
) -> Result<(std::fs::File, TmpFileGuard), DictationError> {
    let expired = || tokio::time::Instant::now() >= deadline;
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

    // Create the file and its cleanup guard together on the blocking pool. If
    // this future is cancelled while creation is queued, the abandoned result
    // (guard included) is dropped once creation finishes and removes the file.
    let create_path = tmp_path.to_path_buf();
    let (std_file, guard) = tokio::task::spawn_blocking(move || {
        let file = std::fs::File::create(&create_path)?;
        Ok::<_, std::io::Error>((file, TmpFileGuard::new(create_path)))
    })
    .await
    .map_err(|e| DictationError::ModelDownloadFailed(format!("Temp file task failed: {e}")))?
    .map_err(|e| DictationError::ModelDownloadFailed(format!("Failed to create temp file: {e}")))?;
    let mut file = tokio::fs::File::from_std(std_file);

    let mut stream = response.bytes_stream();
    loop {
        let chunk = match tokio::time::timeout(limits.idle_timeout, stream.next())
            .await
            .map_err(|_| idle_err(limits))?
        {
            None => break,
            Some(c) => {
                c.map_err(|e| DictationError::ModelDownloadFailed(format!("Download error: {e}")))?
            }
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
        if expired() {
            return Err(total_err(limits, integrity.size));
        }
    }

    file.flush()
        .await
        .map_err(|e| DictationError::ModelDownloadFailed(format!("Flush error: {e}")))?;
    let file = file.into_std().await;
    if expired() {
        return Err(total_err(limits, integrity.size));
    }

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

    Ok((file, guard))
}

/// Only these statuses are followed; any other 3xx is an error.
fn is_followed_redirect(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}

/// Resolve a redirect `Location` against the current URL, refusing an
/// HTTPS-to-HTTP downgrade (pinned model URLs are HTTPS; a downgrade would
/// expose which models are downloaded).
fn redirect_target(current: &reqwest::Url, location: &str) -> Result<reqwest::Url, String> {
    let next = current
        .join(location)
        .map_err(|e| format!("Invalid redirect target: {e}"))?;
    if current.scheme() == "https" && next.scheme() != "https" {
        return Err(format!(
            "Refusing HTTPS-to-{} redirect downgrade from {} to {}",
            next.scheme().to_uppercase(),
            current.host_str().unwrap_or("?"),
            next.host_str().unwrap_or("?")
        ));
    }
    Ok(next)
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
///
/// Blocking: without a matching verification stamp this hashes the whole
/// file, which can be gigabytes. Async code uses
/// [`prepare_existing_artifact_async`].
pub fn prepare_existing_artifact(
    path: &Path,
    integrity: DownloadIntegrity,
) -> Result<ExistingArtifact, DictationError> {
    prepare_existing_artifact_with(path, integrity, &sha256_of_file)
}

/// [`prepare_existing_artifact`] for async callers (#368): the check, and the
/// hash it may need, run on Tokio's blocking pool. Dropping the future stops
/// the hash before its next 1 MiB read; a hash stopped that way never deletes
/// the file.
pub async fn prepare_existing_artifact_async(
    path: &Path,
    integrity: DownloadIntegrity,
) -> Result<ExistingArtifact, DictationError> {
    prepare_existing_artifact_async_with(path.to_path_buf(), integrity, sha256_of_file_until).await
}

async fn prepare_existing_artifact_async_with<H>(
    path: PathBuf,
    integrity: DownloadIntegrity,
    hasher: H,
) -> Result<ExistingArtifact, DictationError>
where
    H: Fn(std::fs::File, &Path, &CancelFlag) -> Result<String, VerificationFailure>
        + Send
        + 'static,
{
    run_blocking_cancellable(move |cancel| {
        prepare_existing_artifact_with(&path, integrity, &|file, p| hasher(file, p, cancel))
    })
    .await
}

fn prepare_existing_artifact_with(
    path: &Path,
    integrity: DownloadIntegrity,
    hasher: FileHasher<'_>,
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

    match verify_file_detailed_with(path, integrity, false, hasher) {
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
    sha256_of_file_until(file, path, &CancelFlag::default())
}

/// Hash `file`, giving up between reads once `cancel` is set. A cancelled
/// hash is an `Other` failure, never an integrity mismatch, so it can never
/// get the file deleted.
fn sha256_of_file_until(
    file: std::fs::File,
    path: &Path,
    cancel: &CancelFlag,
) -> Result<String, VerificationFailure> {
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    // Keep the multi-megabyte artifact hash buffer off the thread stack. The
    // CLI verifier runs on the process's main/Tokio thread on Windows, whose
    // stack is small enough for this allocation to trigger STATUS_STACK_OVERFLOW
    // (especially on ARM64).
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        if cancel.is_cancelled() {
            return Err(VerificationFailure::Other(
                DictationError::ModelDownloadFailed(format!(
                    "Integrity verification of {} was cancelled",
                    path.display()
                )),
            ));
        }
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
fn cache_verified_metadata(
    path: &Path,
    metadata: &std::fs::Metadata,
    integrity: DownloadIntegrity,
) {
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
            tracing::debug!(
                "Cannot read verification stamp {}: {error}",
                stamp_path.display()
            );
            return false;
        }
    };
    let Ok(stamp) = serde_json::from_slice::<VerificationStamp>(&bytes) else {
        tracing::debug!(
            "Ignoring corrupt verification stamp {}",
            stamp_path.display()
        );
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
#[path = "download_tests.rs"]
mod tests;
