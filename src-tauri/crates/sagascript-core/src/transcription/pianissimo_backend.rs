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
use crate::path_scrub::scrub_paths;
use crate::settings::{EnginePrewarm, Settings};
use crate::sync::lock_or_recover;
use crate::transcription::dev_overrides::env_override;
use crate::transcription::engine_host::{
    CancelToken, ClientIdentity, EngineHostClient, EngineHostConfig, EngineHostError, ErrorCode,
    LoadEvent, LoadObserver, LoadSpec, Transcription, WindowTiming,
};
use crate::transcription::pianissimo_model;

/// Shown wherever Pianissimo is refused for platform reasons. Defined with the
/// model catalog, which settings need too (#372). Shim for the old path: remove
/// it, one release on, when its callers (the CLI and the app) use
/// `models::PIANISSIMO_UNSUPPORTED_MESSAGE`.
pub const UNSUPPORTED_MESSAGE: &str = crate::models::PIANISSIMO_UNSUPPORTED_MESSAGE;

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
const WINDOWS_ORT_PROVIDERS_DLL: &str = "onnxruntime_providers_shared.dll";

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
    /// Per-window host stage timings (preprocess/encode/decode ms and round trip).
    pub window_timings: Vec<WindowTiming>,
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
        && macos_version
            .and_then(macos_major)
            .is_some_and(|major| major >= 14)
}

/// Full platform gate: the macOS gate, or Windows on ARM once the bundled host
/// exists. Windows x64 and Linux are never supported.
pub fn platform_gate(
    os: &str,
    arch: &str,
    macos_version: Option<&str>,
    windows_host_present: bool,
) -> bool {
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
        let windows_host =
            os == "windows" && resolve_host().is_some_and(|host| windows_runtime_complete(&host));
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
    let host = current_exe
        .parent()?
        .join(WINDOWS_HOST_DIR)
        .join(WINDOWS_HOST_EXE);
    host.is_file().then_some(host)
}

/// The Windows host is only usable with the ONNX Runtime DLLs beside it.
fn windows_runtime_complete(host: &Path) -> bool {
    host.is_file()
        && host.parent().is_some_and(|dir| {
            [WINDOWS_ORT_DLL, WINDOWS_ORT_PROVIDERS_DLL]
                .iter()
                .all(|dll| dir.join(dll).is_file())
        })
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

/// Resolve the engine host: `SAGASCRIPT_ENGINE_HOST` (development builds only,
/// see [`super::dev_overrides`]), then the bundled host (`Resources/EngineHost`
/// in the macOS bundle, `engine-host\` beside the Windows executable), else `None`.
pub fn resolve_host() -> Option<PathBuf> {
    let current_exe = std::env::current_exe().ok();
    resolve_host_for(
        cfg!(windows),
        env_override(ENGINE_HOST_ENV),
        current_exe.as_deref(),
    )
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

/// What drove a model load: a background warm-up or a transcription request.
/// Only request-driven failures should surface as a dictation error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadSource {
    Warm,
    Request,
}

impl LoadSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Warm => "warm",
            Self::Request => "request",
        }
    }
}

thread_local! {
    /// Set on the pre-warm thread so loads it drives are tagged [`LoadSource::Warm`].
    static LOAD_SOURCE: std::cell::Cell<LoadSource> = const { std::cell::Cell::new(LoadSource::Request) };
}

type LoadListener = Box<dyn Fn(LoadEvent, LoadSource) + Send + Sync>;

static LOAD_LISTENER: OnceLock<LoadListener> = OnceLock::new();
static LAST_LOAD: Mutex<Option<(LoadEvent, LoadSource)>> = Mutex::new(None);

/// Register the process-wide listener told when the shared engine starts,
/// finishes, fails or cancels loading its model (the app forwards this to the
/// overlay). First registration wins.
pub fn set_load_listener(listener: impl Fn(LoadEvent, LoadSource) + Send + Sync + 'static) {
    let _ = LOAD_LISTENER.set(Box::new(listener));
}

/// Message the webview sees for a failed load. The detailed error (which may
/// contain filesystem paths) stays in the logs.
pub const GENERIC_LOAD_FAILURE: &str = "Model failed to load";

/// Webview payload for a load event: `{ state, source, loadMs?, message? }`
/// with `state` one of `loading | ready | failed | unknown`.
pub fn load_state_payload(event: Option<&(LoadEvent, LoadSource)>) -> serde_json::Value {
    match event {
        None => serde_json::json!({ "state": "unknown", "source": LoadSource::Request.as_str() }),
        Some((event, source)) => {
            let source = source.as_str();
            match event {
                LoadEvent::Started => serde_json::json!({ "state": "loading", "source": source }),
                LoadEvent::Ready { load_ms } => {
                    serde_json::json!({ "state": "ready", "source": source, "loadMs": load_ms })
                }
                LoadEvent::Failed(_) => {
                    serde_json::json!({ "state": "failed", "source": source, "message": GENERIC_LOAD_FAILURE })
                }
                LoadEvent::Cancelled => serde_json::json!({ "state": "unknown", "source": source }),
            }
        }
    }
}

/// Payload for the most recent load event (cached so a late-created overlay
/// can seed itself). `unknown` before any load.
pub fn current_load_state_payload() -> serde_json::Value {
    let last = LAST_LOAD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    load_state_payload(last.as_ref())
}

fn forward_load_event(event: LoadEvent) {
    let source = LOAD_SOURCE.with(std::cell::Cell::get);
    if let LoadEvent::Failed(error) = &event {
        tracing::warn!(source = source.as_str(), %error, "Pianissimo engine load failed");
    }
    *LAST_LOAD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((event.clone(), source));
    if let Some(listener) = LOAD_LISTENER.get() {
        listener(event, source);
    }
}

#[cfg(test)]
fn reset_last_load_for_test() {
    *LAST_LOAD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
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
    config.load_observer = Some(LoadObserver(std::sync::Arc::new(forward_load_event)));
    let (unload, shutdown) = idle_policy(settings.engine_idle_unload_minutes);
    config.idle_unload = unload;
    config.idle_shutdown = shutdown;
    config
}

/// Why Pianissimo cannot start right now, checked cheaply and in order.
fn precheck() -> Result<PathBuf, DictationError> {
    if !runtime_supported_on_this_os() {
        return Err(DictationError::TranscriptionFailed(
            UNSUPPORTED_MESSAGE.into(),
        ));
    }
    let host = resolve_host().ok_or_else(|| {
        DictationError::TranscriptionFailed(format!(
            "Pianissimo engine host not found: this build has no bundled engine host and \
             {ENGINE_HOST_ENV} is not set"
        ))
    })?;
    if !pianissimo_model::is_downloaded() {
        return Err(DictationError::TranscriptionFailed(
            "Pianissimo model is not downloaded. Run 'sagascript download-model pianissimo-sv'"
                .into(),
        ));
    }
    Ok(host)
}

/// Engine-host configuration for the current machine and `settings`, after the
/// platform, host and model-presence checks (no model verification, no spawn).
/// The caller loads the settings (`machine::load`): transcription does not read
/// the settings file (#372).
pub fn engine_config(settings: &Settings) -> Result<EngineHostConfig, DictationError> {
    let host = precheck()?;
    Ok(config_for_host(host, settings))
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
/// spawn the host); recreated when the host path, model or the idle setting of
/// `settings` (`engine_idle_unload_minutes`, the only field read) changes between
/// two calls: the next call then verifies the model again, shuts the old client
/// down and creates a new one (see `client_in_slot`). The model artifact is
/// verified once per client, when it is created.
///
/// The caller loads the settings (`machine::load`) when it asks, as this
/// function itself did before (#372).
pub fn shared_client(settings: &Settings) -> Result<EngineHostClient, DictationError> {
    let host = precheck()?;
    let config = config_for_host(host, settings);
    let key = shared_key(&config, settings);
    client_in_slot(
        shared_slot(),
        key,
        pianissimo_model::verify_downloaded,
        |old| {
            if let Some(old) = old {
                old.client.shutdown();
            }
            // Audio a killed app or CLI left in the PCM temp dir (once per process).
            super::engine_host::pipeline::sweep_stale_pcm_dirs_in_background(
                config.temp_dir.clone().unwrap_or_else(std::env::temp_dir),
            );
            EngineHostClient::new(config)
        },
    )
}

/// What identifies the client a `config` and the idle setting would create.
fn shared_key(config: &EngineHostConfig, settings: &Settings) -> SharedKey {
    SharedKey {
        host: config.host_path.clone(),
        model_dir: config.load.model_dir.clone(),
        model_id: config.load.model_id.clone(),
        compute_units: config.load.compute_units.clone(),
        idle_minutes: settings.engine_idle_unload_minutes,
    }
}

/// The decision of [`shared_client`] on its `slot`, apart from where the key
/// and the client come from:
///
/// - the slot holds a client with this `key`: that client, nothing else runs;
/// - otherwise `verify_model` runs first, and when it fails the slot keeps the
///   client it had (the error is the caller's);
/// - then `replace` is given the client being retired (`None` for the first
///   one) and returns the new client, which goes into the slot.
fn client_in_slot(
    slot: &Mutex<Option<Shared>>,
    key: SharedKey,
    verify_model: impl FnOnce() -> Result<(), DictationError>,
    replace: impl FnOnce(Option<Shared>) -> EngineHostClient,
) -> Result<EngineHostClient, DictationError> {
    let mut slot = slot.lock().map_err(|_| {
        DictationError::TranscriptionFailed("Pianissimo client lock was poisoned".into())
    })?;
    if let Some(shared) = slot.as_ref() {
        if shared.key == key {
            return Ok(shared.client.clone());
        }
    }
    verify_model()?;
    let client = replace(slot.take());
    *slot = Some(Shared {
        key,
        client: client.clone(),
    });
    Ok(client)
}

/// Shut the shared host down (CLI exit, model deletion, app quit). No-op when
/// none was created.
pub fn shutdown_shared_client() {
    shutdown_slot(shared_slot());
}

/// Recovered as-is (ADR 0005): the slot holds either the previous complete
/// `Shared` or the next one, whichever critical section a panic interrupted.
const SHARED_SLOT_LOCK: &str = "pianissimo.shared_slot";

/// Takes the client out of `slot` and shuts it down; whether there was one.
/// Recovers a poisoned slot: the CLI shuts the host down after a command that
/// panicked, which can be what poisoned it, and the host must not be orphaned.
fn shutdown_slot(slot: &Mutex<Option<Shared>>) -> bool {
    // The guard is gone before the (slow) shutdown.
    let Some(shared) = lock_or_recover(slot, SHARED_SLOT_LOCK).take() else {
        return false;
    };
    shared.client.shutdown();
    true
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
    if let Some(shared) = shared_slot()
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(|s| s.client.snapshot()))
    {
        remember_host_identity(&shared);
    }
    HOST_IDENTITY.lock().ok().and_then(|cache| cache.clone())
}

/// Start the host if needed and complete `hello` only (no model load) so the
/// build identity is known. Blocking; call from a blocking context.
pub fn refresh_host_identity(settings: &Settings) -> Option<(String, Option<String>)> {
    if let Ok(client) = shared_client(settings) {
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

/// An event that may warrant loading the engine ahead of the next utterance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarmTrigger {
    /// The app just started.
    AppStart,
    /// The push-to-talk key went down (the active profile is passed in).
    KeyDown,
    /// The system woke from sleep (the model may have been evicted).
    Wake,
    /// A profile's settings changed (see [`profile_change_needs_warm`]).
    ProfileChange,
}

impl WarmTrigger {
    pub fn reason(self) -> &'static str {
        match self {
            Self::AppStart => "app_start",
            Self::KeyDown => "key_down",
            Self::Wake => "wake",
            Self::ProfileChange => "profile_change",
        }
    }
}

/// Whether `trigger` should load the engine under `settings`. `active_profile`
/// is only consulted for [`WarmTrigger::KeyDown`].
///
/// App start, wake and profile changes follow `engine_prewarm = on_app_start`
/// and need some profile on Pianissimo. Key-down warms in both `on_app_start`
/// and `on_key_down` (an additional trigger after an idle unload) when the
/// active profile uses Pianissimo. `off` never warms.
pub fn warm_wanted(
    settings: &Settings,
    trigger: WarmTrigger,
    active_profile: Option<&str>,
) -> bool {
    warm_wanted_gated(
        settings,
        trigger,
        active_profile,
        runtime_supported_on_this_os(),
    )
}

/// [`warm_wanted`] with the platform gate given instead of asked of this
/// machine, for a caller that decides against a gate of its own (the app's
/// tests simulate a host that cannot run Pianissimo).
pub fn warm_wanted_gated(
    settings: &Settings,
    trigger: WarmTrigger,
    active_profile: Option<&str>,
    pianissimo_supported: bool,
) -> bool {
    match (settings.engine_prewarm, trigger) {
        (EnginePrewarm::Off, _) => false,
        (_, WarmTrigger::KeyDown) => {
            let id = active_profile
                .map(str::to_owned)
                .unwrap_or_else(|| settings.default_profile().id);
            settings.profile_uses_pianissimo_gated(&id, pianissimo_supported)
        }
        (EnginePrewarm::OnKeyDown, _) => false,
        (EnginePrewarm::OnAppStart, _) => {
            settings.any_profile_uses_pianissimo_gated(pianissimo_supported)
        }
    }
}

/// Call `warm(reason)` when [`warm_wanted`]. The hook is injected so tests need
/// no engine; production passes [`warm_in_background`].
pub fn warm_on(
    settings: &Settings,
    trigger: WarmTrigger,
    active_profile: Option<&str>,
    warm: impl FnOnce(&'static str),
) -> bool {
    let wanted = warm_wanted(settings, trigger, active_profile);
    if wanted {
        warm(trigger.reason());
    }
    wanted
}

/// True when a settings change newly puts a profile on Pianissimo (or newly
/// enables `on_app_start` while one is), so the engine should load now.
pub fn profile_change_needs_warm(old: &Settings, new: &Settings) -> bool {
    profile_change_needs_warm_gated(old, new, runtime_supported_on_this_os())
}

/// [`profile_change_needs_warm`] with the platform gate given instead of asked
/// of this machine (see [`warm_wanted_gated`]).
pub fn profile_change_needs_warm_gated(
    old: &Settings,
    new: &Settings,
    pianissimo_supported: bool,
) -> bool {
    if !warm_wanted_gated(new, WarmTrigger::ProfileChange, None, pianissimo_supported) {
        return false;
    }
    if !warm_wanted_gated(old, WarmTrigger::ProfileChange, None, pianissimo_supported) {
        return true;
    }
    new.resolved_hotkey_profiles().iter().any(|profile| {
        new.profile_uses_pianissimo_gated(&profile.id, pianissimo_supported)
            && !old.profile_uses_pianissimo_gated(&profile.id, pianissimo_supported)
    })
}

/// After a memory-pressure unload, key-down and wake warms are skipped for
/// this long so the engine is not reloaded straight into the same pressure.
pub const PRESSURE_WARM_COOLDOWN: Duration = Duration::from_secs(5 * 60);

static LAST_PRESSURE_UNLOAD: Mutex<Option<Instant>> = Mutex::new(None);

/// True when a warm for `reason` should be skipped because of a recent
/// memory-pressure unload. App start and profile changes are never skipped.
fn warm_in_pressure_cooldown(reason: &str, last_unload: Option<Instant>, now: Instant) -> bool {
    matches!(reason, "key_down" | "wake")
        && last_unload.is_some_and(|at| now.saturating_duration_since(at) < PRESSURE_WARM_COOLDOWN)
}

fn shared_client_if_created() -> Option<EngineHostClient> {
    shared_slot()
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(|shared| shared.client.clone()))
}

/// Free the engine's model memory when the system reports memory pressure and
/// no transcription is running. The next utterance reloads (and shows the
/// loading state). Never starts anything. Returns whether it unloaded, and
/// starts the [`PRESSURE_WARM_COOLDOWN`] when it did.
pub fn unload_if_idle_for_memory_pressure() -> bool {
    let unloaded = shared_client_if_created().is_some_and(|client| client.unload_if_idle());
    if unloaded {
        *LAST_PRESSURE_UNLOAD
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Instant::now());
        tracing::info!("Pianissimo engine unloaded under memory pressure");
    }
    unloaded
}

/// The model is only worth keeping resident while some profile uses Pianissimo.
/// (Pre-warm `off` is not a reason to unload: it only skips eager loading.)
pub fn should_unload_when_unused(settings: &Settings) -> bool {
    !settings.any_profile_uses_pianissimo()
}

/// Unload the resident model now if no profile uses Pianissimo any more and
/// nothing is transcribing; a busy engine is left to the idle timer. Never
/// starts anything. Returns whether it unloaded.
pub fn unload_if_unused(settings: &Settings) -> bool {
    if !should_unload_when_unused(settings) {
        return false;
    }
    let unloaded = shared_client_if_created().is_some_and(|client| client.unload_if_idle());
    if unloaded {
        tracing::info!("Pianissimo engine unloaded: no profile uses it any more");
    }
    unloaded
}

/// Load the model in the background so the next utterance starts warm. Returns
/// immediately; never blocks the caller (hotkey path, main thread). Silently
/// does nothing when Pianissimo is unsupported or not installed.
///
/// `load_settings` (`machine::load`) is called on the warm-up thread, when it
/// attaches to the shared engine, not by the caller: the settings file is not
/// read on the hotkey path, and the read happens as late as it always did.
pub fn warm_in_background(reason: &'static str, load_settings: fn() -> Settings) {
    static IN_FLIGHT: AtomicBool = AtomicBool::new(false);
    if !runtime_supported_on_this_os() || !pianissimo_model::is_downloaded() {
        return;
    }
    let last_unload = *LAST_PRESSURE_UNLOAD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if warm_in_pressure_cooldown(reason, last_unload, Instant::now()) {
        tracing::debug!(
            reason,
            "Pianissimo pre-warm skipped: recent memory-pressure unload"
        );
        return;
    }
    if IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("pianissimo-warm".into())
        .spawn(move || {
            LOAD_SOURCE.with(|source| source.set(LoadSource::Warm));
            let started = Instant::now();
            match shared_client(&load_settings())
                .map_err(|e| e.to_string())
                .and_then(|client| {
                    if client.snapshot().loaded {
                        return Ok(false);
                    }
                    client.warm().map(|_| true).map_err(|e| e.to_string())
                }) {
                Ok(true) => tracing::info!(
                    reason,
                    ms = started.elapsed().as_millis() as u64,
                    "Pianissimo engine pre-warmed"
                ),
                Ok(false) => tracing::debug!(reason, "Pianissimo engine already warm"),
                Err(error) => tracing::warn!(
                    reason,
                    error = %scrub_paths(&error),
                    "Pianissimo engine pre-warm failed"
                ),
            }
            IN_FLIGHT.store(false, Ordering::SeqCst);
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not start Pianissimo pre-warm thread");
        IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}

// ---- adapter --------------------------------------------------------------

/// Maps an engine-host failure to the error the callers see. A cancellation
/// stays a [`DictationError::Cancelled`]; anything else keeps the host's code
/// and retry hint (`engine_*`, see [`engine_code`]) and its text with absolute
/// paths removed: the Swift host names model files by absolute path
/// (ADR 0006, #360).
pub(crate) fn map_engine_error(error: EngineHostError) -> DictationError {
    if error.is_cancelled() {
        return cancelled_error();
    }
    let (code, retryable) = engine_code(&error);
    DictationError::engine(code, retryable, &format!("Pianissimo engine: {error}"))
}

pub(crate) fn cancelled_error() -> DictationError {
    DictationError::Cancelled
}

/// The stable wire code and retry hint of an engine-host failure. A remote
/// error keeps the host's own code and flag (`busy` is the one the protocol
/// marks retryable); the host-side failures have codes of their own. Unknown
/// remote codes collapse to `engine_failed` so a newer host cannot put free
/// text into a code.
fn engine_code(error: &EngineHostError) -> (&'static str, bool) {
    match error {
        EngineHostError::Remote {
            code, retryable, ..
        } => {
            let code = match code {
                ErrorCode::Protocol => "engine_protocol",
                ErrorCode::BadRequest => "engine_bad_request",
                ErrorCode::Unsupported => "engine_unsupported",
                ErrorCode::NotLoaded => "engine_not_loaded",
                ErrorCode::ModelMissing => "engine_model_missing",
                ErrorCode::ModelLoadFailed => "engine_model_load_failed",
                ErrorCode::Busy => "engine_busy",
                ErrorCode::Internal => "engine_internal",
                ErrorCode::Cancelled | ErrorCode::Engine | ErrorCode::Unknown(_) => "engine_failed",
            };
            (code, *retryable)
        }
        // A restart is attempted on the next request, within the crash budget.
        EngineHostError::Crashed { .. } => ("engine_crashed", true),
        // Like a transcription timeout: the same input tends to time out again.
        EngineHostError::Timeout { .. } => ("engine_timeout", false),
        EngineHostError::Spawn { .. }
        | EngineHostError::Handshake(_)
        | EngineHostError::Failed(_)
        | EngineHostError::NotRunning => ("engine_unavailable", false),
        EngineHostError::Protocol(_) => ("engine_protocol", false),
        EngineHostError::InvalidInput(_) => ("engine_bad_request", false),
        EngineHostError::Io(_) => ("engine_io", false),
        EngineHostError::Cancelled => ("engine_failed", false),
    }
}

pub(crate) fn to_result(transcription: Transcription) -> PianissimoResult {
    PianissimoResult {
        window_timings: transcription.windows,
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
        load_seconds: if was_warm {
            0.0
        } else {
            started.elapsed().as_secs_f64()
        },
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
    pub fn start(settings: &Settings) -> Result<Self, DictationError> {
        static NEVER_CANCEL: AtomicBool = AtomicBool::new(false);
        Self::start_with_cancel(&NEVER_CANCEL, settings)
    }

    /// Attach to the process-global engine client. Does not load the model; the
    /// first transcription (or [`Self::warm_up`]) does. `settings` are the
    /// current ones (see [`shared_client`]).
    pub fn start_with_cancel(
        cancelled: &AtomicBool,
        settings: &Settings,
    ) -> Result<Self, DictationError> {
        if cancelled.load(Ordering::SeqCst) {
            return Err(cancelled_error());
        }
        let client = shared_client(settings)?;
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

    /// Steer decoding with dictionary terms (context biasing); `None` clears it.
    pub fn set_boost(&self, boost: Option<super::engine_host::BoostSpec>) {
        self.client.set_boost(boost);
    }

    pub fn client(&self) -> &EngineHostClient {
        &self.client
    }

    /// Engine name the host reported in `hello` (`coreml`, `onnx`, ...), once connected.
    pub fn engine_name(&self) -> Option<String> {
        self.client.snapshot().host.map(|host| host.engine)
    }

    /// Start the host and load the model now, reporting whether it was warm.
    pub fn warm_up(&self) -> Result<WarmInfo, DictationError> {
        warm_client(&self.client, &CancelToken::new())
    }

    /// [`Self::warm_up`] that returns promptly when `cancelled` is set. Stops the
    /// wait only: the host keeps loading, so the next job reuses the model.
    pub fn warm_up_with_cancel(&self, cancelled: &AtomicBool) -> Result<WarmInfo, DictationError> {
        if cancelled.load(Ordering::SeqCst) {
            return Err(cancelled_error());
        }
        let token = CancelToken::new();
        with_cancel_bridge(cancelled, &token, || warm_client(&self.client, &token))
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

    fn idle_client() -> EngineHostClient {
        // Never started (the host does not exist) and without idle supervision.
        let mut config = EngineHostConfig::new(
            "/nonexistent/sagascript-engine-host",
            LoadSpec {
                model_dir: "/nonexistent/models".into(),
                model_id: "test-model".into(),
                compute_units: "cpu".into(),
            },
        );
        config.idle_unload = None;
        config.idle_shutdown = None;
        EngineHostClient::new(config)
    }

    fn shared_for_test() -> Shared {
        Shared {
            key: SharedKey {
                host: "/nonexistent/sagascript-engine-host".into(),
                model_dir: "/nonexistent/models".into(),
                model_id: "test-model".into(),
                compute_units: "cpu".into(),
                idle_minutes: 0,
            },
            client: idle_client(),
        }
    }

    fn remote(code: ErrorCode, retryable: bool, message: &str) -> EngineHostError {
        EngineHostError::Remote {
            code,
            message: message.into(),
            retryable,
        }
    }

    #[test]
    fn cancellation_is_the_cancelled_error_not_a_string() {
        assert!(matches!(cancelled_error(), DictationError::Cancelled));
        assert!(matches!(
            map_engine_error(EngineHostError::Cancelled),
            DictationError::Cancelled
        ));
        assert!(matches!(
            map_engine_error(remote(ErrorCode::Cancelled, false, "stopped")),
            DictationError::Cancelled
        ));
    }

    #[test]
    fn engine_errors_keep_the_hosts_code_and_retry_hint() {
        let cases = [
            (ErrorCode::ModelMissing, false, "engine_model_missing"),
            (
                ErrorCode::ModelLoadFailed,
                false,
                "engine_model_load_failed",
            ),
            (ErrorCode::NotLoaded, false, "engine_not_loaded"),
            (ErrorCode::Busy, true, "engine_busy"),
            (ErrorCode::Unsupported, false, "engine_unsupported"),
            (ErrorCode::BadRequest, false, "engine_bad_request"),
            (ErrorCode::Protocol, false, "engine_protocol"),
            (ErrorCode::Engine, false, "engine_failed"),
            (ErrorCode::Internal, false, "engine_internal"),
            (
                ErrorCode::Unknown("quantum_flux".into()),
                false,
                "engine_failed",
            ),
        ];
        for (code, retryable, expected) in cases {
            let mapped = map_engine_error(remote(code.clone(), retryable, "text"));
            assert_eq!(mapped.code(), expected, "{code:?}");
            assert_eq!(mapped.retryable(), retryable, "{code:?}");
            assert!(matches!(mapped, DictationError::Engine(_)), "{code:?}");
        }
    }

    #[test]
    fn host_side_failures_have_their_own_codes() {
        let crash = map_engine_error(EngineHostError::Crashed {
            status: "signal: 11".into(),
            stderr_tail: "boom".into(),
        });
        assert_eq!((crash.code(), crash.retryable()), ("engine_crashed", true));
        let timeout = map_engine_error(EngineHostError::Timeout {
            op: "transcribe",
            after: Duration::from_secs(5),
        });
        assert_eq!(
            (timeout.code(), timeout.retryable()),
            ("engine_timeout", false)
        );
        for unavailable in [
            EngineHostError::NotRunning,
            EngineHostError::Failed("restart budget spent".into()),
            EngineHostError::Handshake("bad hello".into()),
            EngineHostError::Spawn {
                path: "/nonexistent/host".into(),
                source: std::io::Error::other("denied"),
            },
        ] {
            let mapped = map_engine_error(unavailable);
            assert_eq!(
                (mapped.code(), mapped.retryable()),
                ("engine_unavailable", false)
            );
        }
        assert_eq!(
            map_engine_error(EngineHostError::Protocol("bad line".into())).code(),
            "engine_protocol"
        );
        assert_eq!(
            map_engine_error(EngineHostError::InvalidInput("empty".into())).code(),
            "engine_bad_request"
        );
        assert_eq!(
            map_engine_error(EngineHostError::Io(std::io::Error::other("pipe"))).code(),
            "engine_io"
        );
    }

    /// The Swift host formats `Failed to load model at <absolute path>: …`
    /// (CoreMLEngine.swift, CompiledModelCache.swift). The text must reach the
    /// UI and the log without the user's home directory (ADR 0006).
    #[test]
    fn a_swift_error_with_a_user_path_loses_the_path() {
        let swift = "Failed to load model at /Users/x/Library/Application Support/Sagascript/models/foo.mlmodelc: The file couldn't be opened.";
        let mapped = map_engine_error(remote(ErrorCode::ModelLoadFailed, false, swift));
        let shown = mapped.to_string();
        assert!(!shown.contains("/Users/x"), "{shown}");
        assert!(shown.contains("foo.mlmodelc"), "{shown}");
        assert!(shown.starts_with("Pianissimo engine: "), "{shown}");
        assert!(!format!("{mapped:?}").contains("/Users/x"));

        let crashed = map_engine_error(EngineHostError::Crashed {
            status: "exit status: 1".into(),
            stderr_tail:
                "fatal: cannot open /Users/x/Library/Caches/Sagascript/a.bin\nat /home/x/b".into(),
        });
        let shown = crashed.to_string();
        assert!(
            !shown.contains("/Users/x") && !shown.contains("/home/x"),
            "{shown}"
        );
    }

    #[test]
    fn shutdown_takes_the_client_out_of_the_slot() {
        let slot = Mutex::new(Some(shared_for_test()));
        assert!(shutdown_slot(&slot));
        assert!(slot.lock().unwrap().is_none());
        assert!(!shutdown_slot(&slot), "nothing left to shut down");
    }

    /// The CLI shuts the host down after a command that panicked, and a
    /// panic can be what poisoned the slot (ADR 0005): the host must still
    /// be stopped, not orphaned.
    #[test]
    fn shutdown_still_runs_when_a_panic_poisoned_the_slot() {
        let slot = Mutex::new(Some(shared_for_test()));
        std::thread::scope(|scope| {
            let panicked = scope
                .spawn(|| {
                    let _guard = slot.lock().unwrap();
                    panic!("panic while holding the shared engine slot");
                })
                .join();
            assert!(panicked.is_err());
        });
        assert!(slot.is_poisoned());

        assert!(shutdown_slot(&slot), "the client was not shut down");
        assert!(slot.lock().is_ok_and(|slot| slot.is_none()));
    }

    // ---- the shared client and the settings it was made from ----------------
    //
    // `shared_client` caches one client per process, keyed by the host path, the
    // model, the compute units and `engine_idle_unload_minutes`. These tests pin
    // what a change of that setting between two calls does: the next call finds a
    // different key, verifies the model again, retires the old client (the caller
    // shuts it down) and creates a new one; an unchanged key hands out the same
    // client without any of that.

    fn key_with_idle_minutes(idle_minutes: u32) -> SharedKey {
        SharedKey {
            idle_minutes,
            ..shared_for_test().key
        }
    }

    /// A slot plus counters for what `client_in_slot` ran.
    #[derive(Default)]
    struct Calls {
        verified: std::cell::Cell<u32>,
        created: std::cell::Cell<u32>,
        retired: std::cell::RefCell<Vec<EngineHostClient>>,
    }

    fn client_for(
        slot: &Mutex<Option<Shared>>,
        idle_minutes: u32,
        calls: &Calls,
    ) -> EngineHostClient {
        client_in_slot(
            slot,
            key_with_idle_minutes(idle_minutes),
            || {
                calls.verified.set(calls.verified.get() + 1);
                Ok(())
            },
            |old| {
                calls.created.set(calls.created.get() + 1);
                calls
                    .retired
                    .borrow_mut()
                    .extend(old.map(|shared| shared.client));
                idle_client()
            },
        )
        .expect("verification passes")
    }

    #[test]
    fn the_same_settings_give_the_same_client_without_verifying_again() {
        let slot = Mutex::new(None);
        let calls = Calls::default();
        let first = client_for(&slot, 10, &calls);
        let second = client_for(&slot, 10, &calls);
        assert!(std::sync::Arc::ptr_eq(&first.inner, &second.inner));
        assert_eq!(
            (calls.verified.get(), calls.created.get()),
            (1, 1),
            "the model is verified once, when the client is created"
        );
        assert!(calls.retired.borrow().is_empty());
    }

    #[test]
    fn a_changed_idle_setting_between_calls_replaces_the_client() {
        let slot = Mutex::new(None);
        let calls = Calls::default();
        let before = client_for(&slot, 10, &calls);
        let after = client_for(&slot, 3, &calls);
        assert!(
            !std::sync::Arc::ptr_eq(&before.inner, &after.inner),
            "a new idle setting is a new client"
        );
        assert_eq!(
            (calls.verified.get(), calls.created.get()),
            (2, 2),
            "the replacement verifies the model again"
        );
        let retired = calls.retired.borrow();
        assert_eq!(
            retired.len(),
            1,
            "the old client is handed back to be shut down"
        );
        assert!(std::sync::Arc::ptr_eq(&retired[0].inner, &before.inner));
        // And the slot now holds the new one: asking again changes nothing.
        let again = client_for(&slot, 3, &calls);
        assert!(std::sync::Arc::ptr_eq(&again.inner, &after.inner));
        assert_eq!(calls.created.get(), 2);
    }

    #[test]
    fn going_back_to_an_earlier_setting_is_a_replacement_too() {
        // The slot holds one client, not one per setting.
        let slot = Mutex::new(None);
        let calls = Calls::default();
        let a = client_for(&slot, 10, &calls);
        let _b = client_for(&slot, 3, &calls);
        let c = client_for(&slot, 10, &calls);
        assert!(!std::sync::Arc::ptr_eq(&a.inner, &c.inner));
        assert_eq!(calls.created.get(), 3);
        assert_eq!(calls.retired.borrow().len(), 2);
    }

    #[test]
    fn a_failed_model_check_keeps_the_client_the_slot_had() {
        let slot = Mutex::new(None);
        let calls = Calls::default();
        let kept = client_for(&slot, 10, &calls);
        let refused = client_in_slot(
            &slot,
            key_with_idle_minutes(3),
            || {
                Err(DictationError::TranscriptionFailed(
                    "model changed on disk".into(),
                ))
            },
            |_| unreachable!("nothing is replaced when the model check fails"),
        );
        assert!(refused.is_err());
        let still = client_for(&slot, 10, &calls);
        assert!(std::sync::Arc::ptr_eq(&kept.inner, &still.inner));
        assert!(calls.retired.borrow().is_empty());
    }

    #[test]
    fn the_key_takes_the_idle_minutes_from_the_settings_it_is_given() {
        let config = |minutes| {
            let settings = Settings {
                engine_idle_unload_minutes: minutes,
                ..Settings::default()
            };
            let config = config_for_host("/nonexistent/host".into(), &settings);
            shared_key(&config, &settings)
        };
        assert_eq!(config(7).idle_minutes, 7);
        assert_ne!(config(7), config(0));
        assert_eq!(config(7), config(7));
    }

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
    fn windows_gate_requires_host_and_both_runtime_dlls() {
        let root = tempfile::tempdir().unwrap();
        let host = root.path().join("sagascript-engine-host-ort.exe");
        assert!(!windows_runtime_complete(&host));
        std::fs::write(&host, b"x").unwrap();
        assert!(!windows_runtime_complete(&host));
        std::fs::write(root.path().join("onnxruntime.dll"), b"x").unwrap();
        assert!(!windows_runtime_complete(&host));
        std::fs::write(root.path().join("onnxruntime_providers_shared.dll"), b"x").unwrap();
        assert!(windows_runtime_complete(&host));
        std::fs::remove_file(root.path().join("onnxruntime.dll")).unwrap();
        assert!(!windows_runtime_complete(&host));
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
            (
                Some(Duration::from_secs(600)),
                Some(Duration::from_secs(1200))
            )
        );
    }

    fn pianissimo_settings(prewarm: EnginePrewarm) -> Settings {
        let mut settings = Settings {
            language: crate::models::Language::Swedish,
            engine_prewarm: prewarm,
            ..Default::default()
        };
        settings
            .set_profile_model_gated(
                "default",
                crate::settings::FileModelPreference::PianissimoOriginal,
                true,
            )
            .unwrap();
        settings
    }

    fn whisper_settings(prewarm: EnginePrewarm) -> Settings {
        let mut settings = Settings {
            language: crate::models::Language::Swedish,
            engine_prewarm: prewarm,
            ..Default::default()
        };
        settings
            .set_profile_model_gated(
                "default",
                crate::settings::FileModelPreference::Whisper(
                    crate::models::WhisperModel::KbWhisperMedium,
                ),
                true,
            )
            .unwrap();
        settings
    }

    #[test]
    fn warm_triggers_follow_prewarm_mode_and_pianissimo_use() {
        let on_start = pianissimo_settings(EnginePrewarm::OnAppStart);
        assert!(on_start.any_profile_uses_pianissimo());
        for trigger in [
            WarmTrigger::AppStart,
            WarmTrigger::Wake,
            WarmTrigger::ProfileChange,
            WarmTrigger::KeyDown,
        ] {
            assert!(warm_wanted(&on_start, trigger, None), "{trigger:?}");
        }
        let on_key = pianissimo_settings(EnginePrewarm::OnKeyDown);
        assert!(warm_wanted(&on_key, WarmTrigger::KeyDown, None));
        for trigger in [
            WarmTrigger::AppStart,
            WarmTrigger::Wake,
            WarmTrigger::ProfileChange,
        ] {
            assert!(!warm_wanted(&on_key, trigger, None), "{trigger:?}");
        }
        let off = pianissimo_settings(EnginePrewarm::Off);
        for trigger in [
            WarmTrigger::AppStart,
            WarmTrigger::Wake,
            WarmTrigger::ProfileChange,
            WarmTrigger::KeyDown,
        ] {
            assert!(!warm_wanted(&off, trigger, None), "{trigger:?}");
        }
    }

    #[test]
    fn warm_triggers_do_nothing_without_a_pianissimo_profile() {
        let settings = whisper_settings(EnginePrewarm::OnAppStart);
        assert!(!settings.any_profile_uses_pianissimo());
        for trigger in [
            WarmTrigger::AppStart,
            WarmTrigger::Wake,
            WarmTrigger::ProfileChange,
            WarmTrigger::KeyDown,
        ] {
            assert!(!warm_wanted(&settings, trigger, None), "{trigger:?}");
        }
    }

    #[test]
    fn wake_and_key_down_call_the_injected_warm_hook_with_their_reason() {
        let settings = pianissimo_settings(EnginePrewarm::OnAppStart);
        assert!(settings.any_profile_uses_pianissimo());
        let calls = std::cell::RefCell::new(Vec::new());
        assert!(warm_on(&settings, WarmTrigger::Wake, None, |r| calls
            .borrow_mut()
            .push(r)));
        assert!(warm_on(
            &settings,
            WarmTrigger::KeyDown,
            Some("default"),
            |r| calls.borrow_mut().push(r)
        ));
        assert_eq!(*calls.borrow(), vec!["wake", "key_down"]);
        let off = pianissimo_settings(EnginePrewarm::Off);
        assert!(!warm_on(&off, WarmTrigger::Wake, None, |_| panic!(
            "must not warm when off"
        )));
    }

    #[test]
    fn switching_a_profile_to_pianissimo_warms_but_other_changes_do_not() {
        let before = whisper_settings(EnginePrewarm::OnAppStart);
        let after = pianissimo_settings(EnginePrewarm::OnAppStart);
        assert!(after.any_profile_uses_pianissimo() && !before.any_profile_uses_pianissimo());
        assert!(profile_change_needs_warm(&before, &after));
        // Already on Pianissimo: an unrelated edit does not re-warm.
        assert!(!profile_change_needs_warm(&after, &after.clone()));
        // Switching away does not warm.
        assert!(!profile_change_needs_warm(&after, &before));
        // Turning pre-warm on while a profile is on Pianissimo warms once.
        let mut was_off = after.clone();
        was_off.engine_prewarm = EnginePrewarm::Off;
        assert!(profile_change_needs_warm(&was_off, &after));
        // With pre-warm off nothing warms.
        assert!(!profile_change_needs_warm(&before, &was_off));
    }

    #[test]
    fn the_warm_decision_asks_the_gate_it_is_given() {
        let selected = pianissimo_settings(EnginePrewarm::OnAppStart);
        let mut demoted = selected.clone();
        assert!(demoted.demote_unsupported_pianissimo(false));
        let other = whisper_settings(EnginePrewarm::OnAppStart);

        // A host that cannot run it: a demoted copy never wants the engine, and
        // a pair of demoted copies is no change; the file's selection set
        // against the demoted copy would be one.
        assert!(!warm_wanted_gated(
            &demoted,
            WarmTrigger::ProfileChange,
            None,
            false
        ));
        assert!(!profile_change_needs_warm_gated(
            &demoted,
            &demoted.clone(),
            false
        ));
        assert!(!profile_change_needs_warm_gated(&other, &demoted, false));
        assert!(profile_change_needs_warm_gated(&demoted, &selected, false));
        // A host that can: Auto for a Swedish profile is Pianissimo there.
        assert!(warm_wanted_gated(
            &demoted,
            WarmTrigger::ProfileChange,
            None,
            true
        ));
        assert!(!profile_change_needs_warm_gated(
            &demoted,
            &demoted.clone(),
            true
        ));
        // The ungated functions ask this machine and decide the same way.
        let supported = runtime_supported_on_this_os();
        assert_eq!(
            profile_change_needs_warm(&other, &selected),
            profile_change_needs_warm_gated(&other, &selected, supported)
        );
    }

    #[test]
    fn model_unloads_when_no_profile_uses_pianissimo_even_with_prewarm_off() {
        for prewarm in [
            EnginePrewarm::Off,
            EnginePrewarm::OnAppStart,
            EnginePrewarm::OnKeyDown,
        ] {
            assert!(
                should_unload_when_unused(&whisper_settings(prewarm)),
                "{prewarm:?}"
            );
            assert!(
                !should_unload_when_unused(&pianissimo_settings(prewarm)),
                "{prewarm:?}"
            );
        }
        // Nothing created yet: nothing to unload, and it never starts anything.
        assert!(!unload_if_unused(&whisper_settings(EnginePrewarm::Off)));
    }

    #[test]
    fn pressure_cooldown_skips_only_key_down_and_wake_warms() {
        let now = Instant::now();
        let recent = Some(now);
        assert!(warm_in_pressure_cooldown(
            "key_down",
            recent,
            now + Duration::from_secs(10)
        ));
        assert!(warm_in_pressure_cooldown(
            "wake",
            recent,
            now + Duration::from_secs(10)
        ));
        assert!(!warm_in_pressure_cooldown("app_start", recent, now));
        assert!(!warm_in_pressure_cooldown("profile_change", recent, now));
        assert!(!warm_in_pressure_cooldown(
            "key_down",
            recent,
            now + PRESSURE_WARM_COOLDOWN
        ));
        assert!(!warm_in_pressure_cooldown("key_down", None, now));
    }

    #[test]
    fn load_state_payload_is_cached_tagged_and_generic() {
        let _guard = LOAD_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_last_load_for_test();
        assert_eq!(current_load_state_payload()["state"], "unknown");
        forward_load_event(LoadEvent::Started);
        let p = current_load_state_payload();
        assert_eq!(
            (p["state"].as_str(), p["source"].as_str()),
            (Some("loading"), Some("request"))
        );
        forward_load_event(LoadEvent::Failed("/Users/x/secret/path: boom".into()));
        let p = current_load_state_payload();
        assert_eq!(p["state"], "failed");
        assert_eq!(p["message"], GENERIC_LOAD_FAILURE);
        assert!(!p.to_string().contains("secret"), "{p}");
        forward_load_event(LoadEvent::Cancelled);
        assert_eq!(current_load_state_payload()["state"], "unknown");
        forward_load_event(LoadEvent::Ready { load_ms: 42 });
        assert_eq!(current_load_state_payload()["loadMs"], 42);
        // Warm-thread loads are tagged as warm.
        std::thread::spawn(|| {
            LOAD_SOURCE.with(|source| source.set(LoadSource::Warm));
            forward_load_event(LoadEvent::Failed("x".into()));
        })
        .join()
        .unwrap();
        let p = current_load_state_payload();
        assert_eq!(
            (p["state"].as_str(), p["source"].as_str()),
            (Some("failed"), Some("warm"))
        );
        reset_last_load_for_test();
    }

    static LOAD_TEST_LOCK: Mutex<()> = Mutex::new(());

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
        assert_eq!(
            bundled_macos_host(&app_executable).unwrap(),
            host.canonicalize().unwrap()
        );
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

        assert_eq!(
            bundled_macos_host(&cli_symlink).unwrap(),
            host.canonicalize().unwrap()
        );
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
        assert_eq!(
            resolve_host_for(false, None, Some(&root.path().join("nope"))),
            None
        );
        assert_eq!(resolve_host_for(false, None, None), None);
    }
}
