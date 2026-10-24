//! Process-local microphone authorization and recording preflight.
//! The public diagnostic query never prompts or opens input.
//! A CLI observation describes that process, not another running GUI process.
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MicrophonePermissionStatus {
    Authorized,
    Denied,
    Restricted,
    NotDetermined,
    Unsupported,
    Unknown,
}

impl MicrophonePermissionStatus {
    /// Only an explicit denial is a new capture veto. Undetermined status must
    /// retain the platform's existing first-use permission flow.
    pub fn capture_denied(self) -> bool {
        matches!(self, Self::Denied | Self::Restricted)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MicrophonePermissionReport {
    pub platform: String,
    pub status: MicrophonePermissionStatus,
    pub in_app_bundle: bool,
}

pub fn microphone_permission() -> MicrophonePermissionReport {
    #[cfg(target_os = "macos")]
    let (status, in_app_bundle) = macos::query();
    #[cfg(not(target_os = "macos"))]
    let (status, in_app_bundle) = (MicrophonePermissionStatus::Unsupported, false);
    MicrophonePermissionReport {
        platform: std::env::consts::OS.into(),
        status,
        in_app_bundle,
    }
}

/// Recording preflight may refresh an already-denied, cached AVFoundation
/// result. Read-only diagnostics deliberately never call this function.
#[cfg(feature = "record")]
pub(crate) fn capture_permission_status(
    raw: MicrophonePermissionStatus,
) -> MicrophonePermissionStatus {
    #[cfg(target_os = "macos")]
    {
        refresh_denied(raw, macos::recheck_denied)
    }
    #[cfg(not(target_os = "macos"))]
    {
        raw
    }
}

#[cfg(any(target_os = "macos", test))]
fn from_av_status(value: isize) -> MicrophonePermissionStatus {
    match value {
        0 => MicrophonePermissionStatus::NotDetermined,
        1 => MicrophonePermissionStatus::Restricted,
        2 => MicrophonePermissionStatus::Denied,
        3 => MicrophonePermissionStatus::Authorized,
        _ => MicrophonePermissionStatus::Unknown,
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{from_av_status, MicrophonePermissionStatus};
    use objc::runtime::{Class, Object};
    use objc::{msg_send, sel, sel_impl};

    // Link frameworks instead of dynamically loading bundles from paths.
    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {}
    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {}

    // Match the existing GUI recovery behavior: requestAccess rechecks an
    // already-determined denial without opening a stream. Never call this for
    // not_determined: its initial prompting remains owned by the capture flow.
    #[cfg(feature = "record")]
    pub(super) fn recheck_denied() -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        let callback = std::sync::Mutex::new(Some(tx));
        let completion = block::ConcreteBlock::new(move |granted: bool| {
            if let Some(tx) = crate::sync::lock_or_recover(&callback, "microphone.recheck").take() {
                let _ = tx.send(granted);
            }
        })
        .copy();
        unsafe {
            let (Some(device), Some(string)) =
                (Class::get("AVCaptureDevice"), Class::get("NSString"))
            else {
                return false;
            };
            // A standalone CLI has no app usage description. Do not introduce
            // a requestAccess call that macOS cannot authorize for that process.
            let Some(bundle) = Class::get("NSBundle") else {
                return false;
            };
            let main: *mut Object = msg_send![bundle, mainBundle];
            if main.is_null() {
                return false;
            }
            let key: *mut Object = msg_send![string, alloc];
            if key.is_null() {
                return false;
            }
            let key: *mut Object =
                msg_send![key, initWithUTF8String: c"NSMicrophoneUsageDescription".as_ptr()];
            if key.is_null() {
                return false;
            }
            let description: *mut Object = msg_send![main, objectForInfoDictionaryKey: key];
            let _: () = msg_send![key, release];
            if description.is_null() {
                return false;
            }
            let is_string: bool = msg_send![description, isKindOfClass: string];
            if !is_string {
                return false;
            }
            let length: usize = msg_send![description, length];
            if length == 0 {
                return false;
            }
            let media: *mut Object = msg_send![string, alloc];
            if media.is_null() {
                return false;
            }
            let media: *mut Object = msg_send![media, initWithUTF8String: c"soun".as_ptr()];
            if media.is_null() {
                return false;
            }
            let _: () =
                msg_send![device, requestAccessForMediaType: media completionHandler: &*completion];
            let _: () = msg_send![media, release];
        }
        rx.recv_timeout(std::time::Duration::from_secs(2))
            .unwrap_or(false)
    }

    pub(super) fn query() -> (MicrophonePermissionStatus, bool) {
        // Class methods and immutable Foundation queries only. No requestAccess,
        // TCC database access, input device or audio stream is involved.
        unsafe {
            let (Some(device), Some(string), Some(bundle)) = (
                Class::get("AVCaptureDevice"),
                Class::get("NSString"),
                Class::get("NSBundle"),
            ) else {
                return (MicrophonePermissionStatus::Unknown, false);
            };
            let media: *mut Object = msg_send![string, alloc];
            if media.is_null() {
                return (MicrophonePermissionStatus::Unknown, false);
            }
            let media: *mut Object = msg_send![media, initWithUTF8String: c"soun".as_ptr()];
            if media.is_null() {
                return (MicrophonePermissionStatus::Unknown, false);
            }
            let raw: isize = msg_send![device, authorizationStatusForMediaType: media];
            let _: () = msg_send![media, release];
            let main: *mut Object = msg_send![bundle, mainBundle];
            let in_bundle = if main.is_null() {
                false
            } else {
                let id: *mut Object = msg_send![main, bundleIdentifier];
                if id.is_null() {
                    false
                } else {
                    let length: usize = msg_send![id, length];
                    length > 0
                }
            };
            (from_av_status(raw), in_bundle)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn av_status_mapping_preserves_unknown_and_undetermined() {
        assert_eq!(from_av_status(0), MicrophonePermissionStatus::NotDetermined);
        assert_eq!(from_av_status(1), MicrophonePermissionStatus::Restricted);
        assert_eq!(from_av_status(2), MicrophonePermissionStatus::Denied);
        assert_eq!(from_av_status(3), MicrophonePermissionStatus::Authorized);
        assert_eq!(from_av_status(99), MicrophonePermissionStatus::Unknown);
    }
    #[test]
    fn only_explicit_denial_blocks_existing_capture_flow() {
        for status in [
            MicrophonePermissionStatus::Denied,
            MicrophonePermissionStatus::Restricted,
        ] {
            assert!(status.capture_denied());
        }
        for status in [
            MicrophonePermissionStatus::Authorized,
            MicrophonePermissionStatus::NotDetermined,
            MicrophonePermissionStatus::Unsupported,
            MicrophonePermissionStatus::Unknown,
        ] {
            assert!(!status.capture_denied());
        }
    }
}

#[cfg(any(all(target_os = "macos", feature = "record"), test))]
fn refresh_denied(
    status: MicrophonePermissionStatus,
    refresh: impl FnOnce() -> bool,
) -> MicrophonePermissionStatus {
    if status == MicrophonePermissionStatus::Denied && refresh() {
        MicrophonePermissionStatus::Authorized
    } else {
        status
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn cached_denial_recovers_when_system_recheck_grants() {
        assert_eq!(
            refresh_denied(MicrophonePermissionStatus::Denied, || true),
            MicrophonePermissionStatus::Authorized
        );
    }
    #[test]
    fn ungranted_denial_remains_denied() {
        assert_eq!(
            refresh_denied(MicrophonePermissionStatus::Denied, || false),
            MicrophonePermissionStatus::Denied
        );
    }
    #[test]
    fn other_states_do_not_request_access() {
        for status in [
            MicrophonePermissionStatus::Authorized,
            MicrophonePermissionStatus::Restricted,
            MicrophonePermissionStatus::NotDetermined,
            MicrophonePermissionStatus::Unknown,
            MicrophonePermissionStatus::Unsupported,
        ] {
            assert_eq!(
                refresh_denied(status, || panic!("unexpected permission request")),
                status
            );
        }
    }
}
