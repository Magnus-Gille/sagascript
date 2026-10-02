//! Focus/activation diagnostics for issue #256. Logs category "Focus" events
//! into the shared app log. Privacy: only bundle identifiers, pids, booleans,
//! Sagascript window labels and step names are recorded.

use std::sync::OnceLock;

static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

/// Remember the app handle so call sites without one can log.
pub fn init(app: &tauri::AppHandle) {
    let _ = APP.set(app.clone());
}

#[cfg(target_os = "macos")]
fn windows_by_label(app: &tauri::AppHandle) -> Vec<(String, usize)> {
    use tauri::Manager;
    app.webview_windows()
        .into_iter()
        .filter_map(|(label, window)| window.ns_window().ok().map(|ptr| (label, ptr as usize)))
        .collect()
}

fn emit(event: &str, mut data: serde_json::Value, snapshot: Option<serde_json::Value>, deferred: bool) {
    if let (Some(snapshot), Some(map)) = (snapshot, data.as_object_mut()) {
        map.insert("focus".into(), snapshot);
        if deferred {
            map.insert("snapshotDeferredToMainThread".into(), true.into());
        }
    }
    crate::logging::log_global(level_for(event, &data), "Focus", event, data);
}

/// Only events needed to diagnose paste-target problems in the field are INFO;
/// everything else is DEBUG (written only with RUST_LOG=debug).
fn level_for(event: &str, data: &serde_json::Value) -> &'static str {
    let is_info = match event {
        "paste_guard_check" => data["decision"] == "reject",
        "focus_restore_before_paste" => data["attempted"] == true,
        "settings_window_open_requested" => data["site"] == "paste_failed_copy_fallback",
        "activation_call" => {
            data["site"] == "launch_activation" && data["step"] == "withdrawn_user_moved_on"
        }
        _ => false,
    };
    if is_info {
        "info"
    } else {
        "debug"
    }
}

/// Log a focus event with a snapshot of the frontmost app, app activation and
/// key window. Off the main thread the snapshot is taken on the main thread
/// shortly after (flagged `snapshotDeferredToMainThread`).
pub fn log(event: &'static str, data: serde_json::Value) {
    #[cfg(target_os = "macos")]
    {
        if objc2_foundation::MainThreadMarker::new().is_some() {
            let windows = APP.get().map(windows_by_label).unwrap_or_default();
            let snapshot = crate::platform::macos::focus_snapshot(&windows);
            emit(event, data, Some(snapshot), false);
        } else if let Some(app) = APP.get() {
            let handle = app.clone();
            let queued = data.clone();
            if app
                .run_on_main_thread(move || {
                    let windows = windows_by_label(&handle);
                    let snapshot = crate::platform::macos::focus_snapshot(&windows);
                    emit(event, queued, Some(snapshot), true);
                })
                .is_err()
            {
                emit(event, data, None, false);
            }
        } else {
            emit(event, data, None, false);
        }
    }
    #[cfg(not(target_os = "macos"))]
    emit(event, data, None, false);
}

/// Log a window/app activation call site.
pub fn activation(site: &'static str, step: &'static str) {
    log("activation_call", serde_json::json!({ "site": site, "step": step }));
}

#[cfg(test)]
mod tests {
    use super::level_for;
    use serde_json::json;

    #[test]
    fn field_diagnostic_events_stay_info() {
        assert_eq!(level_for("paste_guard_check", &json!({ "decision": "reject" })), "info");
        assert_eq!(level_for("focus_restore_before_paste", &json!({ "attempted": true })), "info");
        assert_eq!(
            level_for(
                "activation_call",
                &json!({ "site": "launch_activation", "step": "withdrawn_user_moved_on" })
            ),
            "info"
        );
        assert_eq!(
            level_for("settings_window_open_requested", &json!({ "site": "paste_failed_copy_fallback" })),
            "info"
        );
    }

    #[test]
    fn everything_else_is_debug() {
        assert_eq!(level_for("paste_guard_check", &json!({ "decision": "allow" })), "debug");
        assert_eq!(level_for("focus_restore_before_paste", &json!({ "attempted": false })), "debug");
        assert_eq!(level_for("settings_window_open_requested", &json!({ "site": "tray" })), "debug");
        assert_eq!(level_for("activation_call", &json!({ "site": "main_window_reveal", "step": "show" })), "debug");
        for e in ["hotkey_down", "overlay_shown", "focus_restore_after_overlay", "key_up_stop_requested",
                  "capture_stopped", "transcription_result_ready", "result_emitted_to_ui",
                  "engine_load_state_event", "startup_windows", "update_check_requested"] {
            assert_eq!(level_for(e, &json!({})), "debug", "{e}");
        }
    }
}
