//! Read-only, process-local acceptance diagnostics. No capture, settings loader,
//! engine client, prompt, process discovery or network operation belongs here.
pub mod files;

use crate::audio::permission::{microphone_permission, MicrophonePermissionStatus};
use crate::hash::StreamingSha256;
use serde::Serialize;
use std::io::Read;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationStatus {
    Observed,
    Unknown,
    Unsupported,
    NotObserved,
    Invalid,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    GeneratedMetadata,
    CurrentProcess,
    PersistedFixture,
    LiveGui,
    NotObserved,
}

#[derive(Debug, Clone, Serialize)]
pub struct Observation<T> {
    pub status: ObservationStatus,
    pub source: Source,
    pub value: Option<T>,
}
impl<T> Observation<T> {
    pub fn observed(source: Source, value: T) -> Self {
        Self {
            status: ObservationStatus::Observed,
            source,
            value: Some(value),
        }
    }
    pub fn unavailable(source: Source, status: ObservationStatus) -> Self {
        Self {
            status,
            source,
            value: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessibilityStatus {
    Trusted,
    NotTrusted,
    Unsupported,
}

/// AXIsProcessTrusted is a boolean query; false does not distinguish denial
/// from a permission that has never been requested. Never call WithOptions here.
pub fn accessibility_permission() -> AccessibilityStatus {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "ApplicationServices", kind = "framework")]
        unsafe extern "C" {
            fn AXIsProcessTrusted() -> bool;
        }
        if unsafe { AXIsProcessTrusted() } {
            AccessibilityStatus::Trusted
        } else {
            AccessibilityStatus::NotTrusted
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        AccessibilityStatus::Unsupported
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessObservations {
    pub microphone: Observation<MicrophonePermissionStatus>,
    pub accessibility: Observation<AccessibilityStatus>,
    pub in_app_bundle: Observation<bool>,
    pub binary_sha256: Observation<String>,
}

pub fn observe_process() -> ProcessObservations {
    let microphone = microphone_permission();
    let mic_status = match microphone.status {
        MicrophonePermissionStatus::Unknown => ObservationStatus::Unknown,
        MicrophonePermissionStatus::Unsupported => ObservationStatus::Unsupported,
        _ => ObservationStatus::Observed,
    };
    let accessibility = accessibility_permission();
    let ax_status = if accessibility == AccessibilityStatus::Unsupported {
        ObservationStatus::Unsupported
    } else {
        ObservationStatus::Observed
    };
    let binary_sha256 = match hash_current_executable() {
        Ok(hash) => Observation::observed(Source::CurrentProcess, hash),
        Err(_) => Observation::unavailable(Source::CurrentProcess, ObservationStatus::Error),
    };
    ProcessObservations {
        microphone: Observation {
            status: mic_status,
            source: Source::CurrentProcess,
            value: Some(microphone.status),
        },
        accessibility: Observation {
            status: ax_status,
            source: Source::CurrentProcess,
            value: Some(accessibility),
        },
        in_app_bundle: if cfg!(target_os = "macos") {
            Observation::observed(Source::CurrentProcess, microphone.in_app_bundle)
        } else {
            Observation::unavailable(Source::CurrentProcess, ObservationStatus::Unsupported)
        },
        binary_sha256,
    }
}

fn hash_current_executable() -> std::io::Result<String> {
    let mut file = std::fs::File::open(std::env::current_exe()?)?;
    let mut digest = StreamingSha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest.finish_hex())
}

pub fn valid_revision(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|c| c.is_ascii_hexdigit())
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildObservation {
    pub version: String,
    pub revision: Observation<String>,
    pub dirty: Observation<bool>,
}
impl BuildObservation {
    pub fn new(version: &str, revision: &str, dirty: &str) -> Self {
        Self {
            version: version.into(),
            revision: if valid_revision(revision) {
                Observation::observed(Source::GeneratedMetadata, revision.to_ascii_lowercase())
            } else {
                Observation::unavailable(Source::GeneratedMetadata, ObservationStatus::Unknown)
            },
            dirty: match dirty {
                "true" => Observation::observed(Source::GeneratedMetadata, true),
                "false" => Observation::observed(Source::GeneratedMetadata, false),
                _ => {
                    Observation::unavailable(Source::GeneratedMetadata, ObservationStatus::Unknown)
                }
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Passed,
    Blocked,
}
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub status: CheckStatus,
}
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub status: CheckStatus,
    pub checks: Vec<Check>,
    pub remaining_manual_steps: Vec<&'static str>,
}
#[derive(Debug, Clone, Serialize)]
pub struct PreflightReport {
    pub schema_version: u8,
    pub scope: Source,
    pub expected_revision: String,
    pub platform: &'static str,
    pub architecture: &'static str,
    pub build: BuildObservation,
    pub process: ProcessObservations,
    pub onboarding: Observation<bool>,
    pub engine: Observation<()>,
    pub model: Observation<()>,
    pub gui_session: Observation<()>,
    pub summary: Summary,
}

/// Pure assembly shared by both entrypoints. Only an explicitly supplied
/// fixture or live controller makes onboarding a requested check.
pub fn assemble(
    expected: &str,
    build: BuildObservation,
    process: ProcessObservations,
    onboarding: Observation<bool>,
    scope: Source,
) -> PreflightReport {
    let observed = ObservationStatus::Observed;
    let mut checks = vec![
        (
            "source_revision",
            valid_revision(expected)
                && build.revision.status == observed
                && build.revision.value.as_deref() == Some(expected.to_ascii_lowercase().as_str()),
        ),
        (
            "clean_source",
            build.dirty.status == observed && build.dirty.value == Some(false),
        ),
        (
            "binary_sha256",
            process.binary_sha256.status == observed
                && process
                    .binary_sha256
                    .value
                    .as_ref()
                    .is_some_and(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit())),
        ),
        (
            "microphone",
            process.microphone.status == observed
                && process.microphone.value == Some(MicrophonePermissionStatus::Authorized),
        ),
        (
            "accessibility",
            process.accessibility.status == observed
                && process.accessibility.value == Some(AccessibilityStatus::Trusted),
        ),
    ];
    if onboarding.source != Source::NotObserved {
        checks.push((
            "onboarding",
            onboarding.status == observed && onboarding.value == Some(true),
        ));
    }
    let mut remaining = Vec::new();
    for (name, passed) in &checks {
        if !passed {
            remaining.push(*name);
        }
    }
    if onboarding.source == Source::NotObserved {
        remaining.push("onboarding_not_observed");
    }
    remaining.extend([
        "engine_and_model_not_observed",
        "physical_microphone_hotkey_paste_tests",
    ]);
    if scope == Source::CurrentProcess {
        remaining.push("live_gui_not_observed");
    }
    let status = if checks.iter().all(|(_, p)| *p) {
        CheckStatus::Passed
    } else {
        CheckStatus::Blocked
    };
    PreflightReport {
        schema_version: 1,
        scope,
        expected_revision: expected.to_ascii_lowercase(),
        platform: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        build,
        process,
        onboarding,
        engine: Observation::unavailable(Source::NotObserved, ObservationStatus::NotObserved),
        model: Observation::unavailable(Source::NotObserved, ObservationStatus::NotObserved),
        gui_session: Observation::unavailable(Source::NotObserved, ObservationStatus::NotObserved),
        summary: Summary {
            status,
            checks: checks
                .into_iter()
                .map(|(name, pass)| Check {
                    name,
                    status: if pass {
                        CheckStatus::Passed
                    } else {
                        CheckStatus::Blocked
                    },
                })
                .collect(),
            remaining_manual_steps: remaining,
        },
    }
}

#[cfg(test)]
mod tests;
