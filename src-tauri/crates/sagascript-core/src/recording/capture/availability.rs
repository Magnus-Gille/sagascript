//! Whether this computer can record system audio (decision D3, 2026-11-17).
//!
//! System audio needs macOS 14.4 or later on Apple Silicon. Older macOS and
//! Intel Macs record the microphone only. Windows uses WASAPI loopback and is
//! not version-gated here. The decision is a pure function so every row of it
//! is tested on any platform; the probe around it only reads the machine.

/// Lowest macOS version with the System Audio Recording permission (D3).
pub const MIN_MACOS: (u32, u32) = (14, 4);

/// The machine's operating system, as far as system audio cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// macOS with its `(major, minor)` version, if it could be read.
    MacOs(Option<(u32, u32)>),
    Windows,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemAudioSupport {
    Supported,
    /// macOS older than [`MIN_MACOS`], or a version that could not be read.
    OsTooOld,
    /// macOS on a CPU that is not Apple Silicon.
    UnsupportedArch,
    /// An operating system without a system-audio backend (Linux).
    UnsupportedOs,
}

impl SystemAudioSupport {
    pub fn is_supported(self) -> bool {
        self == Self::Supported
    }

    /// One line for the user: why system audio is off here.
    pub fn reason(self) -> &'static str {
        match self {
            Self::Supported => "system audio is available",
            Self::OsTooOld => "system audio needs macOS 14.4 or later",
            Self::UnsupportedArch => "system audio needs a Mac with Apple Silicon",
            Self::UnsupportedOs => "system audio is not available on this operating system",
        }
    }
}

/// The decision. `apple_silicon` is only read on macOS.
pub fn evaluate(os: Os, apple_silicon: bool) -> SystemAudioSupport {
    match os {
        Os::Windows => SystemAudioSupport::Supported,
        Os::Other => SystemAudioSupport::UnsupportedOs,
        Os::MacOs(_) if !apple_silicon => SystemAudioSupport::UnsupportedArch,
        Os::MacOs(Some(version)) if version >= MIN_MACOS => SystemAudioSupport::Supported,
        Os::MacOs(_) => SystemAudioSupport::OsTooOld,
    }
}

/// Parse `14.4`, `14.4.1` or `15` into `(major, minor)`.
pub fn parse_macos_version(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = match parts.next() {
        Some(part) => part.parse().ok()?,
        None => 0,
    };
    Some((major, minor))
}

/// Support on the machine this runs on.
pub fn probe() -> SystemAudioSupport {
    #[cfg(target_os = "macos")]
    {
        let version = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .and_then(|text| parse_macos_version(&text));
        evaluate(Os::MacOs(version), cfg!(target_arch = "aarch64"))
    }
    #[cfg(windows)]
    {
        evaluate(Os::Windows, false)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        evaluate(Os::Other, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_needs_14_4_and_apple_silicon() {
        let mac = |v| evaluate(Os::MacOs(v), true);
        assert_eq!(mac(Some((14, 4))), SystemAudioSupport::Supported);
        assert_eq!(mac(Some((14, 5))), SystemAudioSupport::Supported);
        assert_eq!(mac(Some((15, 0))), SystemAudioSupport::Supported);
        assert_eq!(mac(Some((14, 3))), SystemAudioSupport::OsTooOld);
        assert_eq!(mac(Some((13, 6))), SystemAudioSupport::OsTooOld);
        assert_eq!(mac(None), SystemAudioSupport::OsTooOld);
        assert_eq!(
            evaluate(Os::MacOs(Some((15, 0))), false),
            SystemAudioSupport::UnsupportedArch
        );
    }

    #[test]
    fn windows_is_supported_and_other_systems_are_not() {
        assert!(evaluate(Os::Windows, false).is_supported());
        assert_eq!(evaluate(Os::Other, true), SystemAudioSupport::UnsupportedOs);
    }

    #[test]
    fn versions_parse_with_or_without_a_patch() {
        assert_eq!(parse_macos_version("14.4\n"), Some((14, 4)));
        assert_eq!(parse_macos_version("14.4.1"), Some((14, 4)));
        assert_eq!(parse_macos_version("15"), Some((15, 0)));
        assert_eq!(parse_macos_version("sonoma"), None);
    }

    #[test]
    fn every_unsupported_case_explains_itself() {
        for s in [
            SystemAudioSupport::OsTooOld,
            SystemAudioSupport::UnsupportedArch,
            SystemAudioSupport::UnsupportedOs,
        ] {
            assert!(s.reason().starts_with("system audio"));
        }
    }
}
