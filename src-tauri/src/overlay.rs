use tauri::Manager;
#[cfg(not(target_os = "linux"))]
use tracing::error;
use tracing::info;

/// True until the first overlay window of this app session has been created.
#[cfg(not(target_os = "linux"))]
static FIRST_OVERLAY_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

const OVERLAY_LABEL: &str = "overlay";

// The recording indicator is click-through and must never become the key
// window. In particular, Tauri's macOS `show` implementation calls
// `makeKeyAndOrderFront`, which would move focus away from the destination
// editor while a transcription is in flight.

/// Show the recording overlay window (create lazily on first call).
///
/// Disabled on Linux: creating the transparent, always-on-top overlay window
/// triggers an X11 window-lifecycle crash that terminates the app on several
/// compositors. Transcription and auto-paste still work — only the visual
/// recording indicator is suppressed. See issue #44.
pub fn show(app: &tauri::AppHandle) {
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        info!("Overlay disabled on Linux (avoids X11 window-lifecycle crash)");
    }
    #[cfg(not(target_os = "linux"))]
    {
        if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
            reposition(app, &window);
            present_existing_overlay(&window);
            info!("Overlay shown (existing window)");
            crate::focus_diag::log(
                "overlay_shown",
                serde_json::json!({ "created": false, "firstCreationInSession": false }),
            );
        } else {
            let first = FIRST_OVERLAY_PENDING.swap(false, std::sync::atomic::Ordering::AcqRel);
            match create_overlay(app, true) {
                Ok(_) => {
                    info!("Overlay created and shown");
                    crate::focus_diag::log(
                        "overlay_shown",
                        serde_json::json!({ "created": true, "firstCreationInSession": first }),
                    );
                }
                Err(e) => error!("Failed to create overlay: {e}"),
            }
        }
    }
}

/// Hide the recording overlay window
pub fn hide(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = window.hide();
        info!("Overlay hidden");
    }
}

#[cfg(not(target_os = "linux"))]
fn create_overlay(app: &tauri::AppHandle, present: bool) -> Result<(), Box<dyn std::error::Error>> {
    let (x, y) = overlay_position(app);

    let window = tauri::WebviewWindowBuilder::new(
        app,
        OVERLAY_LABEL,
        tauri::WebviewUrl::App("index.html?overlay=true".into()),
    )
    .title("")
    .inner_size(OVERLAY_WIDTH, 60.0)
    .position(x, y)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .visible(false)
    .focusable(false)
    .focused(false)
    .resizable(false)
    .skip_taskbar(true)
    .build()?;

    #[cfg(target_os = "macos")]
    configure_macos_window(&window);

    // Click-through: cross-platform via Tauri API
    let _ = window.set_ignore_cursor_events(true);

    if present {
        present_existing_overlay(&window);
    }

    Ok(())
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
const OVERLAY_WIDTH: f64 = 220.0;
#[cfg_attr(target_os = "linux", allow(dead_code))]
const OVERLAY_TOP: f64 = 80.0;
#[cfg_attr(target_os = "linux", allow(dead_code))]
const FALLBACK_SCREEN_WIDTH: f64 = 1200.0;

/// Horizontal origin (logical points) that centres the overlay on a screen.
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn overlay_x_for_screen_width(logical_screen_width: f64) -> f64 {
    logical_screen_width / 2.0 - OVERLAY_WIDTH / 2.0
}

/// Current target position, computed from the primary monitor at call time.
#[cfg(not(target_os = "linux"))]
fn overlay_position(app: &tauri::AppHandle) -> (f64, f64) {
    let width = match app.primary_monitor() {
        Ok(Some(monitor)) => monitor.size().width as f64 / monitor.scale_factor(),
        _ => FALLBACK_SCREEN_WIDTH,
    };
    (overlay_x_for_screen_width(width), OVERLAY_TOP)
}

/// Move the (possibly pre-created) overlay to the current screen before it is shown.
#[cfg(not(target_os = "linux"))]
fn reposition(app: &tauri::AppHandle, window: &tauri::WebviewWindow) {
    let (x, y) = overlay_position(app);
    let _ = window.set_position(tauri::LogicalPosition::new(x, y));
}

/// Whether a settings change should pre-create the overlay now.
pub fn should_precreate_on_toggle(enabled: bool, exists: bool) -> bool {
    enabled && !exists
}

/// Settings toggle hook: build the hidden overlay on the main thread when enabled.
pub fn on_show_overlay_changed(app: &tauri::AppHandle, enabled: bool) {
    let exists = app.get_webview_window(OVERLAY_LABEL).is_some();
    if should_precreate_on_toggle(enabled, exists) {
        let handle = app.clone();
        if let Err(e) = app.run_on_main_thread(move || precreate_hidden(&handle)) {
            tracing::warn!("Could not schedule overlay pre-creation: {e}");
        }
    }
}

/// Whether the overlay window should be built (hidden) during app startup.
///
/// The first `WebviewWindowBuilder::build` after launch is where the Settings
/// window was observed coming forward during the first dictation (#291). On
/// macOS tao's launch path activates the app and re-orders visible windows key
/// (`tao::platform_impl::macos::app_state::launched` ->
/// `activateIgnoringOtherApps` + `window_activation_hack`), and a queued
/// activation from `main_window_reveal` lands around the same time. Building the
/// window at startup, while Sagascript is still frontmost, means the first
/// dictation only runs `orderFront` on an existing window, which is the path
/// later dictations take and never steals focus. Linux never creates it (#44).
pub fn should_precreate_at_startup(show_overlay: bool, target_os_supported: bool) -> bool {
    show_overlay && target_os_supported
}

/// Build the overlay window hidden (never ordered front, never key/main) so the
/// first dictation reuses it. No-op if it already exists or on Linux.
pub fn precreate_hidden(app: &tauri::AppHandle) {
    #[cfg(not(target_os = "linux"))]
    {
        if app.get_webview_window(OVERLAY_LABEL).is_some() {
            return;
        }
        match create_overlay(app, false) {
            Ok(()) => {
                FIRST_OVERLAY_PENDING.store(false, std::sync::atomic::Ordering::Release);
                info!("Overlay pre-created hidden at startup");
                crate::focus_diag::log("overlay_precreated", serde_json::json!({}));
            }
            Err(e) => error!("Failed to pre-create overlay: {e}"),
        }
    }
    #[cfg(target_os = "linux")]
    let _ = app;
}

#[cfg(target_os = "macos")]
fn present_existing_overlay(window: &tauri::WebviewWindow) {
    macos_show_without_focus(window);
}

#[cfg(target_os = "windows")]
fn present_existing_overlay(window: &tauri::WebviewWindow) {
    let _ = window.show();
}

/// macOS-specific: configure NSWindow for overlay behaviour
#[cfg(target_os = "macos")]
#[allow(deprecated, unexpected_cfgs)]
fn configure_macos_window(window: &tauri::WebviewWindow) {
    use cocoa::appkit::NSWindow;
    use cocoa::base::{id, NO};

    let ns_window: id = window.ns_window().unwrap() as id;

    unsafe {
        // NSStatusWindowLevel (25) — above normal windows but below screen saver
        ns_window.setLevel_(25);

        // canJoinAllSpaces (1) | stationary (16) | ignoresCycle (64) | fullScreenAuxiliary (256)
        let behavior: u64 = 1 | 16 | 64 | 256;
        let _: () = objc::msg_send![ns_window, setCollectionBehavior: behavior];

        // Transparent chrome
        ns_window.setOpaque_(NO);
        let clear_color: id = objc::msg_send![objc::class!(NSColor), clearColor];
        ns_window.setBackgroundColor_(clear_color);

        // No shadow (CSS provides its own)
        ns_window.setHasShadow_(NO);
    }
}

/// macOS-specific: bring window to front without making it the key window.
#[cfg(target_os = "macos")]
#[allow(deprecated, unexpected_cfgs)]
fn macos_show_without_focus(window: &tauri::WebviewWindow) {
    use cocoa::base::{id, nil};

    let ns_window: id = window.ns_window().unwrap() as id;
    unsafe {
        let _: () = objc::msg_send![ns_window, orderFront: nil];
    }
}

#[cfg(test)]
mod tests {
    use super::should_precreate_at_startup;

    #[test]
    fn centres_overlay_on_screen_width() {
        assert_eq!(super::overlay_x_for_screen_width(1440.0), 610.0);
        assert_eq!(super::overlay_x_for_screen_width(1200.0), 490.0);
    }

    #[test]
    fn precreate_on_toggle_only_when_enabled_and_missing() {
        assert!(super::should_precreate_on_toggle(true, false));
        assert!(!super::should_precreate_on_toggle(true, true));
        assert!(!super::should_precreate_on_toggle(false, false));
    }

    #[test]
    fn precreates_only_when_enabled_and_supported() {
        assert!(should_precreate_at_startup(true, true));
        assert!(!should_precreate_at_startup(false, true));
        assert!(!should_precreate_at_startup(true, false));
        assert!(!should_precreate_at_startup(false, false));
    }
}
