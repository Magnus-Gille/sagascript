//! Pianissimo (KlangAI's Swedish ASR) on the persistent Core ML engine host.
//!
//! [`PianissimoBackend`] keeps its historical public API but is now a thin
//! adapter over a process-global [`EngineHostClient`]: one `sagascript-engine-host`
//! child per app (or CLI) process that keeps the model loaded between
//! utterances (see `docs/engine-host-protocol.md`). Long-audio windowing,
//! merging, progress and cancellation live in the `engine_host` module.
//!
//! Platform gate: Pianissimo needs macOS 14+ on Apple Silicon (Core ML host), or
//! Windows on ARM with the bundled ONNX Runtime host
//! ([`runtime_supported_on_this_os`]); everything else keeps using Whisper.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::error::DictationError;
use crate::settings::Settings;
use crate::transcription::engine_host::{
    CancelToken, ClientIdentity, EngineHostClient, EngineHostConfig, EngineHostError, LoadSpec,
    Transcription,
};
use crate::transcription::dev_overrides::env_override;
use crate::transcription::pianissimo_model;

/// Shown wherever Pianissimo is refused for platform reasons.
pub const UNSUPPORTED_MESSAGE: &str = "Pianissimo requires macOS 14+ on Apple Silicon or Windows on ARM (Snapdragon)";

/// Environment override (development builds only): path of the `sagascript-engine-host` binary.
pub const ENGINE_HOST_ENV: &str = "SAGASCRIPT_ENGINE_HOST";
/// Environment override: Core ML compute units (`ane`, `gpu`, `cpu`, `all`).
pub const COMPUTE_UNITS_ENV: &str = "SAGASCRIPT_ENGINE_COMPUTE_UNITS";
/// Neural Engine on macOS (Core ML host); the ONNX Runtime host on Windows is CPU-only.
const DEFAULT_COMPUTE_UNITS_MACOS: &str = "ane";
const DEFAULT_COMPUTE_UNITS_WINDOWS: &str = "cpu";

/// Bundle-relative location of the macOS host, below `Contents/`.
const BUNDLED_HOST: &str = "Resources/EngineHost/sagascript-engine-host";

/// Windows host and its ONNX Runtime library, in `engine-host\` beside the app executable.
const WINDOWS_HOST_DIR: &str = "engine-host";
const WINDOWS_HOST_EXE: &str = "sagascript-engine-host-ort.exe";
const WINDOWS_ORT_DLL: &str = "onnxruntime.dll";
/// Environment variable the ONNX Runtime host reads for its shared library.
const ORT_DYLIB_ENV: &str = "ORT_DYLIB_PATH";

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PianissimoWord {
    pub word: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PianissimoResult {
    pub text: String,
    pub words: Vec<PianissimoWord>,
}

/// What [`PianissimoBackend::warm_up`] found and did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WarmInfo {
    /// The model was already loaded (no spawn, no load).
    pub was_warm: bool,
    /// Wall time spent making the engine ready (about zero when warm).
    pub load_seconds: f64,
}

// ---- platform gate --------------------------------------------------------

fn macos_major(version: &str) -> Option<u32> {
    version.trim().split('.').next()?.parse().ok()
}

/// Pure gate: macOS >= 14 on aarch64 and nothing else. `macos_version` is the
/// `sw_vers -productVersion` string (only meaningful on macOS).
pub fn platform_supported(os: &str, arch: &str, macos_version: Option<&str>) -> bool {
    os == "macos"
        && arch == "aarch64"
        && macos_version.and_then(macos_major).is_some_and(|major| major >= 14)
}

/// Full platform gate: the macOS gate, or Windows on ARM once the bundled host
/// exists. Windows x64 and Linux are never supported.
pub fn platform_gate(os: &str, arch: &str, macos_version: Option<&str>, windows_host_present: bool) -> bool {
    platform_supported(os, arch, macos_version)
        || (os == "windows" && arch == "aarch64" && windows_host_present)
}

#[cfg(target_os = "macos")]
fn detect_macos_version() -> Option<String> {
    let output = std::process::Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
}

#[cfg(not(target_os = "macos"))]
fn detect_macos_version() -> Option<String> {
    None
}

/// Whether this machine can run Pianissimo: macOS 14+ on Apple Silicon, or Windows
/// on ARM with the bundled ONNX host. False on every other OS or architecture.
pub fn runtime_supported_on_this_os() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let version = if os == "macos" && arch == "aarch64" {
            detect_macos_version()
        } else {
            None
        };
        let windows_host = os == "windows"
            && resolve_host().is_some_and(|host| host.is_file());
        platform_gate(os, arch, version.as_deref(), windows_host)
    })
}

// ---- host resolution ------------------------------------------------------

/// `Sagascript.app/Contents/Resources/EngineHost/sagascript-engine-host` next to
/// `current_exe`. The path is canonicalized first so the installed CLI symlink
/// (`/usr/local/bin/sagascript` -> the app bundle executable) finds the bundle.
fn bundled_macos_host(current_exe: &Path) -> Option<PathBuf> {
    let current_exe = current_exe.canonicalize().ok()?;
    let contents = current_exe.parent()?.parent()?;
    let host = contents.join(BUNDLED_HOST);
    host.is_file().then_some(host)
}

/// `<exe dir>\engine-host\sagascript-engine-host-ort.exe` beside the app or CLI
/// executable (no canonicalization: it would add a `\\?\` prefix).
fn bundled_windows_host(current_exe: &Path) -> Option<PathBuf> {
    let host = current_exe.parent()?.join(WINDOWS_HOST_DIR).join(WINDOWS_HOST_EXE);
    host.is_file().then_some(host)
}

fn resolve_host_for(
    windows: bool,
    env_value: Option<OsString>,
    current_exe: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(path) = env_value.filter(|value| !value.is_empty()) {
        // Honoured as given even if missing, so the spawn error names the path.
        return Some(PathBuf::from(path));
    }
    current_exe.and_then(|exe| {
        if windows {
            bundled_windows_host(exe)
        } else {
            bundled_macos_host(exe)
        }
    })
}

/// Extra environment for the host process: on Windows the ONNX Runtime library
/// beside the host executable.
fn host_env(windows: bool, host: &Path) -> Vec<(String, String)> {
    if !windows {
        return Vec::new();
    }
    host.parent()
        .map(|dir| dir.join(WINDOWS_ORT_DLL))
        .map(|dll| vec![(ORT_DYLIB_ENV.to_string(), dll.display().to_string())])
        .unwrap_or_default()
}

/// Resolve the engine host: `SAGASCRIPT_ENGINE_HOST` (development builds only,
/// see [`super::dev_overrides`]), then the bundled host (`Resources/EngineHost`
/// in the macOS bundle, `engine-host\` beside the Windows executable), else `None`.
pub fn resolve_host() -> Option<PathBuf> {
    let current_exe = std::env::current_exe().ok();
    resolve_host_for(cfg!(windows), env_override(ENGINE_HOST_ENV), current_exe.as_deref())
}

// ---- identity and configuration ------------------------------------------

static IDENTITY: OnceLock<ClientIdentity> = OnceLock::new();

/// Register the build identity announced to the engine host. The app and the
/// CLI call this once with their generated build metadata (the same values as
/// the tray menu / `--version`). Later calls are ignored.
pub fn set_client_identity(identity: ClientIdentity) {
    let _ = IDENTITY.set(identity);
}

fn client_identity() -> ClientIdentity {
    IDENTITY.get().cloned().unwrap_or_default()
}

fn default_compute_units(windows: bool) -> &'static str {
    if windows {
        DEFAULT_COMPUTE_UNITS_WINDOWS
    } else {
        DEFAULT_COMPUTE_UNITS_MACOS
    }
}

fn compute_units() -> String {
    std::env::var(COMPUTE_UNITS_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default_compute_units(cfg!(windows)).to_string())
}

/// Idle policy from settings: unload after N minutes, shut the host down after 2N.
/// 0 disables both.
fn idle_policy(minutes: u32) -> (Option<Duration>, Option<Duration>) {
    if minutes == 0 {
        return (None, None);
    }
    let unload = Duration::from_secs(u64::from(minutes) * 60);
    (Some(unload), Some(unload * 2))
}

/// Engine-host configuration for an explicit host binary (no presence checks).
pub fn config_for_host(host: PathBuf, settings: &Settings) -> EngineHostConfig {
    let mut config = EngineHostConfig::new(
        host,
        LoadSpec {
            model_dir: pianissimo_model::model_dir(),
            model_id: pianissimo_model::model_id(),
            compute_units: compute_units(),
        },
    );
    config.identity = client_identity();
    config.env.extend(host_env(cfg!(windows), &config.host_path));
    let (unload, shutdown) = idle_policy(settings.engine_idle_unload_minutes);
    config.idle_unload = unload;
    config.idle_shutdown = shutdown;
    config
}

/// Why Pianissimo cannot start right now, checked cheaply and in order.
fn precheck() -> Result<PathBuf, DictationError> {
    if !runtime_supported_on_this_os() {
        return Err(DictationError::TranscriptionFailed(UNSUPPORTED_MESSAGE.into()));
    }
    let host = resolve_host().ok_or_else(|| {
        DictationError::TranscriptionFailed(format!(
            "Pianissimo engine host not found: this build has no bundled engine host and \
             {ENGINE_HOST_ENV} is not set"
        ))
    })?;
    if !pianissimo_model::is_downloaded() {
        return Err(DictationError::TranscriptionFailed(
            "Pianissimo model is not downloaded. Run 'sagascript download-model pianissimo-sv'".into(),
        ));
    }
    Ok(host)
}

/// Engine-host configuration for the current machine and settings, after the
/// platform, host and model-presence checks (no model verification, no spawn).
pub fn engine_config() -> Result<EngineHostConfig, DictationError> {
    let host = precheck()?;
    Ok(config_for_host(host, &crate::settings::store::load()))
}

// ---- process-global client ------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct SharedKey {
    host: PathBuf,
    model_dir: PathBuf,
    model_id: String,
    compute_units: String,
    idle_minutes: u32,
}

struct Shared {
    key: SharedKey,
    client: EngineHostClient,
}

fn shared_slot() -> &'static Mutex<Option<Shared>> {
    static SHARED: OnceLock<Mutex<Option<Shared>>> = OnceLock::new();
    SHARED.get_or_init(|| Mutex::new(None))
}

/// The one [`EngineHostClient`] of this process. Created lazily (this does not
/// spawn the host); recreated when the host path, model or idle settings change.
/// The model artifact is verified once, when the client is created.
pub fn shared_client() -> Result<EngineHostClient, DictationError> {
    let host = precheck()?;
    let settings = crate::settings::store::load();
    let config = config_for_host(host, &settings);
    let key = SharedKey {
        host: config.host_path.clone(),
        model_dir: config.load.model_dir.clone(),
        model_id: config.load.model_id.clone(),
        compute_units: config.load.compute_units.clone(),
        idle_minutes: settings.engine_idle_unload_minutes,
    };
    let mut slot = shared_slot()
        .lock()
        .map_err(|_| DictationError::TranscriptionFailed("Pianissimo client lock was poisoned".into()))?;
    if let Some(shared) = slot.as_ref() {
        if shared.key == key {
            return Ok(shared.client.clone());
        }
    }
    pianissimo_model::verify_downloaded()?;
    if let Some(old) = slot.take() {
        old.client.shutdown();
    }
    let client = EngineHostClient::new(config);
    *slot = Some(Shared {
        key,
        client: client.clone(),
    });
    Ok(client)
}

/// Shut the shared host down (CLI exit, model deletion, app quit). No-op when
/// none was created.
pub fn shutdown_shared_client() {
    let shared = shared_slot().lock().ok().and_then(|mut slot| slot.take());
    if let Some(shared) = shared {
        shared.client.shutdown();
    }
}

static HOST_IDENTITY: Mutex<Option<(String, Option<String>)>> = Mutex::new(None);

fn remember_host_identity(snapshot: &super::engine_host::HostSnapshot) {
    if let (Some(host), Ok(mut cache)) = (snapshot.host.as_ref(), HOST_IDENTITY.lock()) {
        *cache = Some((host.version.clone(), host.git_sha.clone()));
    }
}

/// Engine-host build identity `(version, git_sha)` from the last successful
/// `hello` in this process. Never starts the host.
pub fn cached_host_identity() -> Option<(String, Option<String>)> {
    if let Some(shared) = shared_slot().lock().ok().and_then(|slot| slot.as_ref().map(|s| s.client.snapshot())) {
        remember_host_identity(&shared);
    }
    HOST_IDENTITY.lock().ok().and_then(|cache| cache.clone())
}

/// Start the host if needed and complete `hello` only (no model load) so the
/// build identity is known. Blocking; call from a blocking context.
pub fn refresh_host_identity() -> Option<(String, Option<String>)> {
    if let Ok(client) = shared_client() {
        if let Ok(snapshot) = client.connect() {
            remember_host_identity(&snapshot);
        }
    }
    cached_host_identity()
}

/// Whether the shared engine currently has the model loaded (never starts anything).
pub fn engine_is_warm() -> bool {
    shared_slot()
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(|shared| shared.client.snapshot().loaded))
        .unwrap_or(false)
}

/// Load the model in the background so the next utterance starts warm. Returns
/// immediately; never blocks the caller (hotkey path, main thread). Silently
/// does nothing when Pianissimo is unsupported or not installed.
pub fn warm_in_background(reason: &'static str) {
    static IN_FLIGHT: AtomicBool = AtomicBool::new(false);
    if !runtime_supported_on_this_os() || !pianissimo_model::is_downloaded() {
        return;
    }
    if IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("pianissimo-warm".into())
        .spawn(move || {
            let started = Instant::now();
            match shared_client().map_err(|e| e.to_string()).and_then(|client| {
                if client.snapshot().loaded {
                    return Ok(false);
                }
                client.warm().map(|_| true).map_err(|e| e.to_string())
            }) {
                Ok(true) => tracing::info!(reason, ms = started.elapsed().as_millis() as u64, "Pianissimo engine pre-warmed"),
                Ok(false) => tracing::debug!(reason, "Pianissimo engine already warm"),
                Err(error) => tracing::warn!(reason, %error, "Pianissimo engine pre-warm failed"),
            }
            IN_FLIGHT.store(false, Ordering::SeqCst);
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not start Pianissimo pre-warm thread");
        IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}

// ---- adapter --------------------------------------------------------------

pub(crate) fn map_engine_error(error: EngineHostError) -> DictationError {
    if error.is_cancelled() {
        return cancelled_error();
    }
    DictationError::TranscriptionFailed(format!("Pianissimo engine: {error}"))
}

pub(crate) fn cancelled_error() -> DictationError {
    DictationError::TranscriptionFailed("Pianissimo transcription cancelled".into())
}

pub(crate) fn to_result(transcription: Transcription) -> PianissimoResult {
    PianissimoResult {
        text: transcription.text,
        words: transcription
            .words
            .into_iter()
            .map(|word| PianissimoWord {
                word: word.word,
                start: word.start,
                end: word.end,
            })
            .collect(),
    }
}

/// Make the engine ready and report whether it already was.
pub(crate) fn warm_client(
    client: &EngineHostClient,
    cancel: &CancelToken,
) -> Result<WarmInfo, DictationError> {
    let was_warm = client.snapshot().loaded;
    let started = Instant::now();
    client.warm_with_cancel(cancel).map_err(map_engine_error)?;
    Ok(WarmInfo {
        was_warm,
        load_seconds: if was_warm { 0.0 } else { started.elapsed().as_secs_f64() },
    })
}

/// Run `job` while a scoped watcher forwards a caller-owned `AtomicBool` to the
/// job's [`CancelToken`] (the engine client polls tokens, callers own flags).
pub(crate) fn with_cancel_bridge<T>(
    cancelled: &AtomicBool,
    token: &CancelToken,
    job: impl FnOnce() -> T,
) -> T {
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::SeqCst) {
                if cancelled.load(Ordering::SeqCst) {
                    token.cancel();
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let result = job();
        done.store(true, Ordering::SeqCst);
        result
    })
}

/// Adapter with the pre-sidecar public API over an [`EngineHostClient`].
pub struct PianissimoBackend {
    client: EngineHostClient,
    active: Mutex<Option<CancelToken>>,
}

impl PianissimoBackend {
    pub fn start() -> Result<Self, DictationError> {
        static NEVER_CANCEL: AtomicBool = AtomicBool::new(false);
        Self::start_with_cancel(&NEVER_CANCEL)
    }

    /// Attach to the process-global engine client. Does not load the model; the
    /// first transcription (or [`Self::warm_up`]) does.
    pub fn start_with_cancel(cancelled: &AtomicBool) -> Result<Self, DictationError> {
        if cancelled.load(Ordering::SeqCst) {
            return Err(cancelled_error());
        }
        let client = shared_client()?;
        if cancelled.load(Ordering::SeqCst) {
            return Err(cancelled_error());
        }
        Ok(Self::with_client(client))
    }

    /// Use an explicit client (tests, benchmarks).
    pub fn with_client(client: EngineHostClient) -> Self {
        Self {
            client,
            active: Mutex::new(None),
        }
    }

    pub fn client(&self) -> &EngineHostClient {
        &self.client
    }

    /// Start the host and load the model now, reporting whether it was warm.
    pub fn warm_up(&self) -> Result<WarmInfo, DictationError> {
        warm_client(&self.client, &CancelToken::new())
    }

    pub fn transcribe(
        &self,
        samples: &[f32],
        progress: impl Fn(u8),
    ) -> Result<PianissimoResult, DictationError> {
        static NEVER_CANCEL: AtomicBool = AtomicBool::new(false);
        self.transcribe_with_cancel(samples, progress, &NEVER_CANCEL)
    }

    /// Transcribe 16 kHz mono samples as a file job (batch priority, windows
    /// merged in order). `progress` receives real 0..=100 percentages as windows
    /// are merged; `cancelled` is observed continuously.
    pub fn transcribe_with_cancel(
        &self,
        samples: &[f32],
        progress: impl Fn(u8),
        cancelled: &AtomicBool,
    ) -> Result<PianissimoResult, DictationError> {
        if cancelled.load(Ordering::SeqCst) {
            return Err(cancelled_error());
        }
        let token = CancelToken::new();
        {
            let mut active = self.active.lock().map_err(|_| {
                DictationError::TranscriptionFailed("Pianissimo operation lock was poisoned".into())
            })?;
            *active = Some(token.clone());
        }
        progress(0);
        let outcome = with_cancel_bridge(cancelled, &token, || {
            let mut report = |merged: usize, total: usize| {
                let percent = (merged * 100).checked_div(total).unwrap_or(100).min(100);
                progress(percent as u8);
            };
            self.client
                .transcribe_file_samples(samples, &token, Some(&mut report))
        });
        if let Ok(mut active) = self.active.lock() {
            let _ = active.take();
        }
        let transcription = outcome.map_err(map_engine_error)?;
        progress(100);
        Ok(to_result(transcription))
    }

    /// Cancel the running transcription (best effort, non-sticky).
    pub fn request_abort(&self) {
        if let Ok(active) = self.active.lock() {
            if let Some(token) = active.as_ref() {
                token.cancel();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_matrix_allows_only_macos_14_on_apple_silicon() {
        // (os, arch, macOS version, expected)
        let cases = [
            ("macos", "aarch64", Some("14.0"), true),
            ("macos", "aarch64", Some("14.7.6\n"), true),
            ("macos", "aarch64", Some("26.6.2"), true),
            ("macos", "aarch64", Some("13.7.6"), false),
            ("macos", "aarch64", Some("12.6.9"), false),
            ("macos", "aarch64", Some("unknown"), false),
            ("macos", "aarch64", None, false),
            ("macos", "x86_64", Some("15.0"), false),
            ("windows", "x86_64", None, false),
            ("windows", "aarch64", None, false),
            ("windows", "aarch64", Some("14.0"), false),
            ("linux", "x86_64", None, false),
            ("linux", "aarch64", Some("14.0"), false),
        ];
        for (os, arch, version, expected) in cases {
            assert_eq!(
                platform_supported(os, arch, version),
                expected,
                "{os}/{arch}/{version:?}"
            );
        }
    }

    #[test]
    fn full_gate_adds_windows_arm_only_with_the_bundled_host() {
        // (os, arch, macOS version, windows host present, expected)
        let cases = [
            ("windows", "aarch64", None, true, true),
            ("windows", "aarch64", None, false, false),
            ("windows", "x86_64", None, true, false),
            ("linux", "aarch64", None, true, false),
            ("linux", "x86_64", None, true, false),
            ("macos", "aarch64", Some("14.0"), false, true),
            ("macos", "aarch64", Some("13.0"), true, false),
            ("macos", "x86_64", Some("15.0"), true, false),
        ];
        for (os, arch, version, host, expected) in cases {
            assert_eq!(
                platform_gate(os, arch, version, host),
                expected,
                "{os}/{arch}/{version:?}/host={host}"
            );
        }
    }

    #[test]
    fn windows_host_resolves_beside_the_executable() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("sagascript.exe");
        assert_eq!(bundled_windows_host(&exe), None);
        let dir = root.path().join("engine-host");
        std::fs::create_dir_all(&dir).unwrap();
        let host = dir.join("sagascript-engine-host-ort.exe");
        std::fs::write(&host, b"x").unwrap();
        assert_eq!(bundled_windows_host(&exe), Some(host.clone()));
        assert_eq!(resolve_host_for(true, None, Some(&exe)), Some(host.clone()));
        // The macOS layout is not consulted on Windows and vice versa.
        assert_eq!(resolve_host_for(false, None, Some(&exe)), None);
        // An explicit override wins, even when missing.
        assert_eq!(
            resolve_host_for(true, Some("C:\\dev\\host.exe".into()), Some(&exe)),
            Some(PathBuf::from("C:\\dev\\host.exe"))
        );
    }

    #[test]
    fn ort_library_is_passed_only_on_windows_and_lives_beside_the_host() {
        let host = Path::new("app").join("engine-host").join("sagascript-engine-host-ort.exe");
        let env = host_env(true, &host);
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, "ORT_DYLIB_PATH");
        assert_eq!(
            Path::new(&env[0].1),
            Path::new("app").join("engine-host").join("onnxruntime.dll")
        );
        assert!(host_env(false, &host).is_empty());
    }

    #[test]
    fn compute_units_default_per_platform() {
        assert_eq!(default_compute_units(false), "ane");
        assert_eq!(default_compute_units(true), "cpu");
    }

    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    #[test]
    fn unsupported_hosts_report_unsupported() {
        assert!(!runtime_supported_on_this_os());
    }

    #[test]
    fn idle_policy_shuts_down_after_twice_the_unload_time() {
        assert_eq!(idle_policy(0), (None, None));
        assert_eq!(
            idle_policy(10),
            (Some(Duration::from_secs(600)), Some(Duration::from_secs(1200)))
        );
    }

    fn app_layout(root: &Path) -> (PathBuf, PathBuf) {
        let contents = root.join("Sagascript.app/Contents");
        let host_dir = contents.join("Resources/EngineHost");
        let app_executable = contents.join("MacOS/sagascript");
        std::fs::create_dir_all(&host_dir).unwrap();
        std::fs::create_dir_all(app_executable.parent().unwrap()).unwrap();
        let host = host_dir.join("sagascript-engine-host");
        std::fs::write(&host, b"").unwrap();
        std::fs::write(&app_executable, b"").unwrap();
        (host, app_executable)
    }

    #[test]
    fn bundled_host_resolves_relative_to_app_executable() {
        let root = tempfile::tempdir().unwrap();
        let (host, app_executable) = app_layout(root.path());
        assert_eq!(bundled_macos_host(&app_executable).unwrap(), host.canonicalize().unwrap());
    }

    #[test]
    fn bundled_host_is_none_when_missing() {
        let root = tempfile::tempdir().unwrap();
        let (host, app_executable) = app_layout(root.path());
        std::fs::remove_file(host).unwrap();
        assert!(bundled_macos_host(&app_executable).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn bundled_host_resolves_from_cli_symlink_outside_app_bundle() {
        let root = tempfile::tempdir().unwrap();
        let (host, app_executable) = app_layout(root.path());
        let cli_dir = root.path().join("usr-local-bin");
        std::fs::create_dir_all(&cli_dir).unwrap();
        let cli_symlink = cli_dir.join("sagascript");
        std::os::unix::fs::symlink(&app_executable, &cli_symlink).unwrap();

        assert_eq!(bundled_macos_host(&cli_symlink).unwrap(), host.canonicalize().unwrap());
    }

    #[test]
    fn env_override_wins_over_bundle_and_empty_env_is_ignored() {
        let root = tempfile::tempdir().unwrap();
        let (host, app_executable) = app_layout(root.path());
        assert_eq!(
            resolve_host_for(false, Some("/dev/host".into()), Some(&app_executable)),
            Some(PathBuf::from("/dev/host"))
        );
        assert_eq!(
            resolve_host_for(false, Some("".into()), Some(&app_executable)).unwrap(),
            host.canonicalize().unwrap()
        );
        assert_eq!(resolve_host_for(false, None, Some(&root.path().join("nope"))), None);
        assert_eq!(resolve_host_for(false, None, None), None);
    }
}
