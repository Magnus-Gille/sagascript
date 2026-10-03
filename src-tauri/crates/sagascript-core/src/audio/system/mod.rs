//! System-audio capture (issue #289): what the computer is playing, for
//! transcribing digital meetings. See `docs/system-audio.md` for the design.
//!
//! Nothing in this module prompts for a permission unless a capture is
//! explicitly started; [`system_audio_status`] only queries.

pub mod convert;
pub mod recorder;
pub mod twotrack;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows_backend;

use std::str::FromStr;

use serde::Serialize;

use crate::error::DictationError;

/// Where `record` takes its audio from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioSource {
    Mic,
    System,
    Both,
}

impl AudioSource {
    pub fn needs_mic(self) -> bool {
        matches!(self, AudioSource::Mic | AudioSource::Both)
    }
    pub fn needs_system(self) -> bool {
        matches!(self, AudioSource::System | AudioSource::Both)
    }
}

impl FromStr for AudioSource {
    type Err = DictationError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "mic" | "microphone" => Ok(AudioSource::Mic),
            "system" => Ok(AudioSource::System),
            "both" => Ok(AudioSource::Both),
            other => Err(DictationError::SettingsError(format!(
                "Unknown audio source '{other}'. Valid: mic, system, both"
            ))),
        }
    }
}

/// Which application's audio to capture (system source only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppSelector {
    Pid(u32),
    /// macOS bundle identifier, e.g. `us.zoom.xos`.
    BundleId(String),
    /// Executable name, e.g. `Teams.exe`.
    Exe(String),
}

impl AppSelector {
    /// Classify a `--app` value: all digits is a pid, a dotted name without a
    /// `.exe` suffix is a bundle id, anything else an executable name.
    pub fn parse(value: &str) -> Result<Self, DictationError> {
        let v = value.trim();
        if v.is_empty() {
            return Err(DictationError::SettingsError("--app must not be empty".into()));
        }
        if v.bytes().all(|b| b.is_ascii_digit()) {
            let pid = v.parse::<u32>().map_err(|_| {
                DictationError::SettingsError(format!("--app pid '{v}' is out of range"))
            })?;
            return Ok(AppSelector::Pid(pid));
        }
        let lower = v.to_ascii_lowercase();
        if v.contains('.') && !lower.ends_with(".exe") && !v.contains(['/', '\\']) {
            return Ok(AppSelector::BundleId(v.to_string()));
        }
        Ok(AppSelector::Exe(v.to_string()))
    }
}

/// Case-insensitive executable-name match; `Teams` matches `Teams.exe`.
pub fn exe_name_matches(candidate: &str, wanted: &str) -> bool {
    let base = candidate.rsplit(['/', '\\']).next().unwrap_or(candidate).to_ascii_lowercase();
    let want = wanted.rsplit(['/', '\\']).next().unwrap_or(wanted).to_ascii_lowercase();
    base == want || base.strip_suffix(".exe") == Some(want.as_str())
}

/// Process-tree roots among `(pid, parent_pid)` pairs of same-named processes:
/// those whose parent is not itself one of the matches, in ascending pid order.
/// Multi-process apps (Teams, Chrome) have one root with many same-exe children;
/// capturing the root's tree covers them all.
pub fn tree_roots(procs: &[(u32, u32)]) -> Vec<u32> {
    let pids: std::collections::HashSet<u32> = procs.iter().map(|(p, _)| *p).collect();
    let mut roots: Vec<u32> = procs
        .iter()
        .filter(|(pid, ppid)| !pids.contains(ppid) || pid == ppid)
        .map(|(p, _)| *p)
        .collect();
    roots.sort_unstable();
    roots.dedup();
    roots
}

/// What to capture from the system mix.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CaptureTarget {
    /// Everything the computer plays.
    #[default]
    All,
    App(AppSelector),
}

/// Permission state for system-audio capture. `Unknown` means the OS offers no
/// non-prompting query (or it failed); the first capture will then decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemAudioPermission {
    Granted,
    Denied,
    NotDetermined,
    /// No permission is needed (Windows loopback).
    NotRequired,
    Unknown,
    /// The platform cannot capture system audio at all.
    Unsupported,
}

/// Result of the non-prompting capability probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SystemAudioStatus {
    pub supported: bool,
    pub per_app_capture: bool,
    pub permission: SystemAudioPermission,
    /// Human-readable explanation, including what to do when unsupported/denied.
    pub detail: String,
    /// Backend identifier: `coreaudio-process-tap`, `wasapi-loopback`, or `none`.
    pub backend: &'static str,
}

/// Map the private `TCCAccessPreflight` result to a permission state. Only
/// 0 (authorized) and 1 (denied) are trusted; every other value, including the
/// "not determined" code we have not verified, is `Unknown` until the owner's
/// live test shows the raw values (logged at debug level by `doctor`).
pub fn map_tcc_preflight(result: i32) -> SystemAudioPermission {
    match result {
        0 => SystemAudioPermission::Granted,
        1 => SystemAudioPermission::Denied,
        _ => SystemAudioPermission::Unknown,
    }
}

/// Minimum macOS for the supported process-tap path (the audio-only
/// "System Audio Recording" permission arrived in 14.4).
pub const MACOS_MIN_VERSION: (u32, u32) = (14, 4);

pub fn macos_version_supported(major: u32, minor: u32) -> bool {
    (major, minor) >= MACOS_MIN_VERSION
}

/// Parse `14.4.1`-style versions into (major, minor).
pub fn parse_macos_version(v: &str) -> Option<(u32, u32)> {
    let mut parts = v.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |m| m.parse().ok())?;
    Some((major, minor))
}

pub fn unsupported_message() -> String {
    "System-audio capture is not supported on this platform (macOS 14.4+ and Windows only).".into()
}

/// Capability probe only (OS version, API presence). Does not query the
/// permission, so capture start never touches the private TCC preflight.
pub fn system_audio_capability() -> SystemAudioStatus {
    #[cfg(target_os = "macos")]
    {
        macos::capability()
    }
    #[cfg(target_os = "windows")]
    {
        windows_backend::status()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        system_audio_status()
    }
}

/// Capability plus the best-effort, non-prompting permission query. Used by
/// `doctor` only. Never starts a capture.
pub fn system_audio_status() -> SystemAudioStatus {
    #[cfg(target_os = "macos")]
    {
        macos::status()
    }
    #[cfg(target_os = "windows")]
    {
        windows_backend::status()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        SystemAudioStatus {
            supported: false,
            per_app_capture: false,
            permission: SystemAudioPermission::Unsupported,
            detail: unsupported_message(),
            backend: "none",
        }
    }
}

/// Callback receiving native-rate interleaved f32 frames. Runs on a capture
/// thread; it must be cheap and must not block.
pub type ChunkSink = Box<dyn FnMut(&[f32]) + Send>;

/// Format of the chunks passed to the sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

/// A running system-audio capture. Dropping it stops the capture.
pub struct SystemAudioCapture {
    #[cfg(target_os = "macos")]
    inner: macos::Capture,
    #[cfg(target_os = "windows")]
    inner: windows_backend::Capture,
    format: NativeFormat,
}

impl SystemAudioCapture {
    /// Start capturing. This is the call that may trigger the OS permission
    /// prompt (macOS); callers must have shown the consent/indicator first.
    pub fn start(target: &CaptureTarget, sink: ChunkSink) -> Result<Self, DictationError> {
        #[cfg(target_os = "macos")]
        {
            let (inner, format) = macos::Capture::start(target, sink)?;
            Ok(Self { inner, format })
        }
        #[cfg(target_os = "windows")]
        {
            let (inner, format) = windows_backend::Capture::start(target, sink)?;
            Ok(Self { inner, format })
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = (target, sink);
            Err(DictationError::AudioCaptureError(unsupported_message()))
        }
    }

    pub fn format(&self) -> NativeFormat {
        self.format
    }

    /// Stop capturing. Returns the first backend capture error, if any occurred.
    pub fn stop(self) -> Result<(), DictationError> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.inner.stop()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_parses_and_rejects_unknown() {
        assert_eq!("mic".parse::<AudioSource>().unwrap(), AudioSource::Mic);
        assert_eq!("system".parse::<AudioSource>().unwrap(), AudioSource::System);
        assert_eq!("both".parse::<AudioSource>().unwrap(), AudioSource::Both);
        assert!("speakers".parse::<AudioSource>().is_err());
        assert!(AudioSource::Both.needs_mic() && AudioSource::Both.needs_system());
        assert!(!AudioSource::System.needs_mic());
        assert!(!AudioSource::Mic.needs_system());
    }

    #[test]
    fn app_selector_classification() {
        assert_eq!(AppSelector::parse("1234").unwrap(), AppSelector::Pid(1234));
        assert_eq!(
            AppSelector::parse("us.zoom.xos").unwrap(),
            AppSelector::BundleId("us.zoom.xos".into())
        );
        assert_eq!(AppSelector::parse("Teams.exe").unwrap(), AppSelector::Exe("Teams.exe".into()));
        assert_eq!(AppSelector::parse("Teams").unwrap(), AppSelector::Exe("Teams".into()));
        assert_eq!(
            AppSelector::parse("C:\\x\\a.b").unwrap(),
            AppSelector::Exe("C:\\x\\a.b".into())
        );
        assert!(AppSelector::parse(" ").is_err());
        assert!(AppSelector::parse("99999999999").is_err());
    }

    #[test]
    fn tree_roots_pick_parentless_matches() {
        // 10 is the root; 11 and 12 are same-exe children; 20 is a separate instance.
        assert_eq!(tree_roots(&[(11, 10), (10, 1), (12, 10), (20, 2)]), vec![10, 20]);
        assert_eq!(tree_roots(&[(5, 5)]), vec![5]);
        assert!(tree_roots(&[]).is_empty());
    }

    #[test]
    fn exe_matching() {
        assert!(exe_name_matches("Teams.exe", "teams"));
        assert!(exe_name_matches("C:\\Apps\\Zoom.exe", "zoom.exe"));
        assert!(!exe_name_matches("Zoomer.exe", "zoom"));
    }

    #[test]
    fn tcc_mapping() {
        assert_eq!(map_tcc_preflight(0), SystemAudioPermission::Granted);
        assert_eq!(map_tcc_preflight(1), SystemAudioPermission::Denied);
        assert_eq!(map_tcc_preflight(2), SystemAudioPermission::Unknown);
        assert_eq!(map_tcc_preflight(-1), SystemAudioPermission::Unknown);
    }

    #[test]
    fn macos_version_gate() {
        assert_eq!(parse_macos_version("14.4.1"), Some((14, 4)));
        assert_eq!(parse_macos_version("15"), Some((15, 0)));
        assert_eq!(parse_macos_version("x"), None);
        assert!(macos_version_supported(14, 4));
        assert!(macos_version_supported(15, 0));
        assert!(!macos_version_supported(14, 3));
        assert!(!macos_version_supported(13, 6));
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn linux_reports_unsupported() {
        let s = system_audio_status();
        assert!(!s.supported);
        assert_eq!(s.permission, SystemAudioPermission::Unsupported);
        assert!(SystemAudioCapture::start(&CaptureTarget::All, Box::new(|_| {})).is_err());
    }
}
