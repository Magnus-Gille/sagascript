//! macOS permission queries for onboarding (Accessibility, microphone).

#[tauri::command]
pub async fn check_accessibility_permission() -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        Ok(crate::platform::macos::is_accessibility_trusted())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(true)
    }
}

#[tauri::command]
pub async fn request_accessibility_permission() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        crate::focus_diag::activation(
            "command_request_accessibility_permission",
            "opens_system_settings",
        );
        crate::platform::macos::request_accessibility_permission()?;
    }
    Ok(())
}

#[tauri::command]
pub async fn open_accessibility_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        crate::focus_diag::activation(
            "command_open_accessibility_settings",
            "opens_system_settings",
        );
        crate::platform::macos::open_accessibility_settings()?;
    }
    Ok(())
}

/// Returns the microphone authorization status as a string.
/// Possible values: "authorized", "not_determined", "denied", "restricted", "unsupported".
/// "unsupported" is returned when the binary is not running from a proper .app bundle
/// (e.g. during `cargo run` or `cargo tauri dev` without a bundle).
#[tauri::command]
pub async fn microphone_status() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        if !macos_mic::is_in_app_bundle() {
            return Ok("unsupported".to_string());
        }
        crate::focus_diag::activation(
            "command_request_microphone_access",
            "may_show_system_prompt",
        );
        Ok(macos_mic::authorization_status_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok("authorized".to_string())
    }
}

/// Triggers AVCaptureDevice.requestAccessForMediaType:completionHandler: and returns
/// the new status string after the user responds (or immediately if already determined).
/// Possible return values: "authorized", "not_determined", "denied", "restricted", "unsupported".
#[tauri::command]
pub async fn request_microphone_access() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        if !macos_mic::is_in_app_bundle() {
            return Ok("unsupported".to_string());
        }
        // Use spawn_blocking to avoid starving the tokio runtime during the
        // up-to-60s wait for the user to respond to the permission dialog.
        tokio::task::spawn_blocking(|| {
            use std::sync::mpsc;
            let (tx, rx) = mpsc::channel();
            macos_mic::request_access(move |_granted| {
                let _ = tx.send(());
            });
            let _ = rx.recv_timeout(std::time::Duration::from_secs(60));
            macos_mic::authorization_status_string()
        })
        .await
        .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok("authorized".to_string())
    }
}

#[tauri::command]
pub async fn open_microphone_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        crate::focus_diag::activation("command_open_microphone_settings", "opens_system_settings");
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
            .spawn()
            .map_err(|e| format!("Failed to open System Settings: {}", e))?;
    }
    Ok(())
}

/// macOS-specific microphone permission helpers using AVCaptureDevice via objc.
#[cfg(target_os = "macos")]
mod macos_mic {
    use objc::runtime::{Class, Object};
    use objc::{msg_send, sel, sel_impl};
    use std::sync::Once;

    /// AVAuthorizationStatus values
    const AV_AUTH_STATUS_NOT_DETERMINED: isize = 0;
    const AV_AUTH_STATUS_DENIED: isize = 2;

    /// Returns true if the binary is running from inside a proper .app bundle.
    /// When running via `cargo run` or `cargo tauri dev` without a bundle,
    /// NSBundle.mainBundle.bundleIdentifier returns nil — TCC won't attribute
    /// permission requests correctly in that case.
    pub fn is_in_app_bundle() -> bool {
        unsafe {
            let ns_bundle_class = match Class::get("NSBundle") {
                Some(c) => c,
                None => return false,
            };
            let main_bundle: *mut Object = msg_send![ns_bundle_class, mainBundle];
            if main_bundle.is_null() {
                return false;
            }
            let bundle_id: *mut Object = msg_send![main_bundle, bundleIdentifier];
            if bundle_id.is_null() {
                return false;
            }
            // Check that the string is non-empty
            let len: usize = msg_send![bundle_id, length];
            len > 0
        }
    }

    /// Ensure AVFoundation framework is loaded (required for AVCaptureDevice class lookup).
    fn ensure_avfoundation_loaded() {
        static LOAD: Once = Once::new();
        LOAD.call_once(|| unsafe {
            let ns_bundle_class = Class::get("NSBundle").expect("NSBundle class");
            let path: *mut Object = msg_send![
                Class::get("NSString").expect("NSString"),
                stringWithUTF8String: c"/System/Library/Frameworks/AVFoundation.framework".as_ptr()
            ];
            let bundle: *mut Object = msg_send![ns_bundle_class, bundleWithPath: path];
            if !bundle.is_null() {
                let _loaded: bool = msg_send![bundle, load];
            }
        });
    }

    fn get_av_capture_device_class() -> Option<&'static Class> {
        ensure_avfoundation_loaded();
        let cls = Class::get("AVCaptureDevice");
        if cls.is_none() {
            tracing::warn!("AVCaptureDevice class not found");
        }
        cls
    }

    /// AVMediaTypeAudio constant
    fn av_media_type_audio() -> *mut Object {
        let ns_string_class = Class::get("NSString").expect("NSString class");
        unsafe { msg_send![ns_string_class, stringWithUTF8String: c"soun".as_ptr()] }
    }

    pub(super) fn authorization_status_label(
        status: isize,
        denied_recheck_granted: bool,
    ) -> &'static str {
        match status {
            0 => "not_determined",
            1 => "restricted",
            2 if denied_recheck_granted => "authorized",
            2 => "denied",
            3 => "authorized",
            _ => "not_determined",
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    pub(super) enum AccessRequestDecision {
        QuerySystem,
        Complete(bool),
    }

    pub(super) fn access_request_decision(status: isize) -> AccessRequestDecision {
        match status {
            AV_AUTH_STATUS_NOT_DETERMINED | AV_AUTH_STATUS_DENIED => {
                AccessRequestDecision::QuerySystem
            }
            3 => AccessRequestDecision::Complete(true),
            _ => AccessRequestDecision::Complete(false),
        }
    }

    /// Return the raw authorization status as a string.
    /// AVCaptureDevice can cache `authorizationStatus` in-process, so when it
    /// reports "denied" we re-query via `requestAccess` which returns the
    /// current TCC state immediately (no dialog shown for already-determined
    /// states). This avoids opening a real audio stream as a side effect.
    pub fn authorization_status_string() -> String {
        let cls = match get_av_capture_device_class() {
            Some(c) => c,
            None => {
                tracing::warn!("AVCaptureDevice class not found — returning not_determined");
                return "not_determined".to_string();
            }
        };
        let media_type = av_media_type_audio();
        let status: isize = unsafe { msg_send![cls, authorizationStatusForMediaType: media_type] };
        // AVCaptureDevice may cache "denied" after a System Settings change.
        // The raw request API returns the current TCC state without a new dialog
        // for an already-determined permission.
        let denied_recheck_granted = status == 2 && recheck_access_granted();
        authorization_status_label(status, denied_recheck_granted).to_string()
    }

    /// Re-query TCC permission via `requestAccessForMediaType:completionHandler:`.
    /// For already-determined states, this returns immediately without showing a dialog.
    /// This defeats AVCaptureDevice's in-process cache of `authorizationStatus`.
    fn recheck_access_granted() -> bool {
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel();
        request_access_from_system(move |granted| {
            let _ = tx.send(granted);
        });
        rx.recv_timeout(std::time::Duration::from_secs(2))
            .unwrap_or(false)
    }

    /// Request microphone access using AVCaptureDevice.requestAccessForMediaType:completionHandler:.
    /// Calls `callback` once the user responds (or immediately if status is already determined).
    pub fn request_access<F: FnOnce(bool) + Send + 'static>(callback: F) {
        let cls = match get_av_capture_device_class() {
            Some(c) => c,
            None => {
                callback(false);
                return;
            }
        };
        let media_type = av_media_type_audio();

        let status: isize = unsafe { msg_send![cls, authorizationStatusForMediaType: media_type] };
        match access_request_decision(status) {
            AccessRequestDecision::QuerySystem => request_access_from_system(callback),
            AccessRequestDecision::Complete(granted) => callback(granted),
        }
    }

    /// Invoke AVFoundation's request API without consulting the potentially
    /// stale in-process `authorizationStatus` value first.
    fn request_access_from_system<F: FnOnce(bool) + Send + 'static>(callback: F) {
        let cls = match get_av_capture_device_class() {
            Some(c) => c,
            None => {
                callback(false);
                return;
            }
        };
        let media_type = av_media_type_audio();

        // Use AVCaptureDevice.requestAccessForMediaType:completionHandler: with an objc block.
        // This properly attributes the permission request to the app's bundle ID.
        let callback = std::sync::Mutex::new(Some(callback));
        let completion = block::ConcreteBlock::new(move |granted: bool| {
            if let Some(cb) =
                sagascript_core::sync::lock_or_recover(&callback, "mic_permission.callback").take()
            {
                cb(granted);
            }
        });
        // The block must be copied to the heap for async use by the framework
        let completion = completion.copy();

        unsafe {
            let _: () = msg_send![cls, requestAccessForMediaType: media_type completionHandler: &*completion];
        }
    }
}

#[cfg(test)]
#[path = "permissions_tests.rs"]
mod tests;
