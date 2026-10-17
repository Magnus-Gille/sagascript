use tauri::Manager;
use tracing::{error, info, warn};

use crate::events::{self, EmitEvent};
use crate::focus_diag;
#[cfg(target_os = "macos")]
use crate::platform;

trait MainWindowVisibility {
    fn activate_app(&self);
    fn reset_to_normal_level(&self) -> Result<(), String>;
    fn unminimize(&self) -> Result<(), String>;
    fn show(&self) -> Result<(), String>;
    fn set_focus(&self) -> Result<(), String>;
}

impl MainWindowVisibility for tauri::WebviewWindow {
    fn activate_app(&self) {
        focus_diag::activation("main_window_reveal", "activate_app_queued");
        #[cfg(target_os = "macos")]
        if let Err(error) = self.app_handle().run_on_main_thread(|| {
            focus_diag::activation("main_window_reveal", "activate_ignoring_other_apps_before");
            platform::macos::activate_app_for_launch();
            focus_diag::activation("main_window_reveal", "activate_ignoring_other_apps_after");
        }) {
            warn!("Failed to schedule foreground application activation: {error}");
        }
    }

    fn reset_to_normal_level(&self) -> Result<(), String> {
        tauri::WebviewWindow::set_always_on_top(self, false).map_err(|error| error.to_string())
    }

    fn unminimize(&self) -> Result<(), String> {
        focus_diag::activation("main_window_reveal", "unminimize");
        tauri::WebviewWindow::unminimize(self).map_err(|error| error.to_string())
    }

    fn show(&self) -> Result<(), String> {
        focus_diag::activation("main_window_reveal", "show");
        tauri::WebviewWindow::show(self).map_err(|error| error.to_string())
    }

    fn set_focus(&self) -> Result<(), String> {
        focus_diag::activation("main_window_reveal", "set_focus");
        tauri::WebviewWindow::set_focus(self).map_err(|error| error.to_string())
    }
}

fn reveal_existing_main_window(window: &impl MainWindowVisibility) -> Result<(), String> {
    if let Err(error) = window.reset_to_normal_level() {
        warn!("Failed to restore normal main-window level: {error}");
    }
    window
        .unminimize()
        .map_err(|error| format!("failed to restore main window: {error}"))?;
    window
        .show()
        .map_err(|error| format!("failed to show main window: {error}"))?;
    window
        .set_focus()
        .map_err(|error| format!("failed to focus main window: {error}"))?;
    // Queue activation after presentation. During initial `.setup()` a direct
    // AppKit activation happens before Tauri's event loop is ready and macOS
    // leaves the onboarding window behind the previously active application.
    window.activate_app();
    Ok(())
}

/// Open or focus the main window, optionally navigating to a specific tab.
/// Errors are surfaced in the application log instead of being silently lost.
pub(crate) fn open_settings_window(app: &tauri::AppHandle, tab: Option<&str>, site: &'static str) {
    info!("Opening main window (tab: {:?})", tab);
    focus_diag::log(
        "settings_window_open_requested",
        serde_json::json!({
            "site": site,
            "tab": tab,
            "windowExists": app.get_webview_window("settings").is_some(),
        }),
    );

    if let Err(error) = try_open_settings_window(app, tab) {
        error!("Failed to open main window: {error}");
    }
}

fn try_open_settings_window(app: &tauri::AppHandle, tab: Option<&str>) -> Result<(), String> {
    // Build a URL with optional query parameter
    let url = match tab {
        Some("onboarding") => "index.html?onboarding=true".to_string(),
        Some(t) => format!("index.html?tab={t}"),
        None => "index.html".to_string(),
    };

    if let Some(window) = app.get_webview_window("settings") {
        // If switching tab on existing window, emit an event
        if let Some(t) = tab {
            if let Err(error) = window.emit_event(events::event::NAVIGATE_TAB, t) {
                warn!("Failed to navigate main window to '{t}': {error}");
            }
        }
        reveal_existing_main_window(&window)
    } else {
        // Cap default height to 80% of screen so it fits on small displays (e.g. 768p laptops)
        let default_height = if let Ok(Some(monitor)) = app.primary_monitor() {
            let logical_h = monitor.size().height as f64 / monitor.scale_factor();
            (logical_h * 0.8).min(660.0)
        } else {
            660.0
        };

        let window =
            tauri::WebviewWindowBuilder::new(app, "settings", tauri::WebviewUrl::App(url.into()))
                .title("Sagascript")
                .inner_size(500.0, default_height)
                .min_inner_size(500.0, 400.0)
                .resizable(true)
                .always_on_top(false)
                .center()
                .focused(true)
                .build()
                .map_err(|error| format!("failed to create main window: {error}"))?;

        reveal_existing_main_window(&window)
    }
}

#[cfg(test)]
#[path = "window_tests.rs"]
mod tests;
