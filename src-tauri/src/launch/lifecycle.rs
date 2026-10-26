use tauri::Manager;
use tracing::info;

use super::mode::{
    second_instance_requests_settings, second_instance_requests_update, should_reveal_for_reopen,
};
use super::window::open_settings_window;
use crate::commands::{lock_controller, SharedController};
use crate::dispatch_to_main;
use crate::updater::check_for_updates_and_install;

/// A second launch of the app reached the running instance (single-instance
/// plugin). `args` is the second process's argv.
pub(crate) fn handle_second_instance(app: &tauri::AppHandle, args: Vec<String>) {
    let update_requested = second_instance_requests_update(&args);
    if !update_requested && !second_instance_requests_settings(&args) {
        info!("Background second-instance launch ignored");
        return;
    }

    // The callback is delivered by the single-instance transport,
    // which is not guaranteed to be the Tauri main thread (notably on
    // macOS). Queue all state/UI work onto the main thread. This also
    // makes the Windows WM_COPYDATA callback return promptly instead
    // of keeping the secondary process blocked on a window operation.
    let app = app.clone();
    dispatch_to_main(&app, move |app| {
        if update_requested {
            info!("CLI requested the in-app updater");
            check_for_updates_and_install(app.clone());
            return;
        }
        let should_reveal = app
            .try_state::<SharedController>()
            .map(|ctrl| should_reveal_for_reopen(lock_controller(&ctrl).state()))
            .unwrap_or(true);
        if should_reveal {
            info!("Second-instance launch requested Settings");
            open_settings_window(app, None, "second_instance_launch");
        } else {
            info!("Ignoring second-instance launch while dictation is active");
        }
    });
}

/// The Tauri event loop's run-event hook: keep the tray-only app alive, stop the
/// engine host on exit and reveal Settings on a macOS reopen.
pub(crate) fn handle_run_event(_app_handle: &tauri::AppHandle, event: tauri::RunEvent) {
    match event {
        // Prevent app from exiting when all windows are closed (tray-only app),
        // but allow explicit exit requests (e.g. from tray "Quit" menu)
        tauri::RunEvent::ExitRequested {
            api, code: None, ..
        } => api.prevent_exit(),
        tauri::RunEvent::ExitRequested {
            api, code: Some(_), ..
        } if _app_handle
            .try_state::<crate::live_meeting_commands::SharedLiveMeetings>()
            .is_some_and(|jobs| jobs.has_pending_session()) =>
        {
            api.prevent_exit();
            open_settings_window(_app_handle, Some("live"), "pending_live_meeting_on_quit");
        }
        // Stop the Pianissimo engine host with the app instead of leaving it to stdin EOF.
        tauri::RunEvent::Exit => {
            if let Some(jobs) =
                _app_handle.try_state::<crate::live_meeting_commands::SharedLiveMeetings>()
            {
                jobs.shutdown();
            }
            sagascript_core::transcription::pianissimo_backend::shutdown_shared_client()
        }
        // Finder/Spotlight sends Reopen when the already-running app is
        // launched again. A miniaturized NSWindow may still count as
        // "visible", so always run the full reveal path.
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => {
            let state = {
                let ctrl: tauri::State<'_, SharedController> = _app_handle.state();
                let state = lock_controller(&ctrl).state();
                state
            };
            if should_reveal_for_reopen(state) {
                info!("Application reopen requested");
                open_settings_window(_app_handle, None, "app_reopen_event");
            } else {
                info!("Ignoring application reopen while dictation is {state:?}");
            }
        }
        _ => {}
    }
}
