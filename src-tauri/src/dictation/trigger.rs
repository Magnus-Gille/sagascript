use sagascript_core::settings::{canonical_hotkey, HotkeyMode};
use tauri::Manager;
use tracing::{error, info, warn};

use super::engine::{prewarm_engine_on_key_down, reemit_engine_load_state};
use super::pipeline::stop_recording_and_transcribe;
use crate::app_controller::HotkeyDownResult;
use crate::command_error::CommandError;
use crate::commands::{lock_controller, SharedController};
use crate::events::{self, EmitEvent, UiState};
#[cfg(target_os = "macos")]
use crate::platform;
use crate::tray::{select_profile_menu, update_tray_status};
use crate::{focus_diag, hotkey, overlay, SAFE_FALLBACK_HOTKEY};

fn should_use_safe_fallback(
    shortcut: &str,
    settings_valid: bool,
    operational_hotkey: &hotkey::OperationalHotkey,
) -> bool {
    if settings_valid {
        return false;
    }

    let (Ok(canonical_shortcut), Ok(canonical_fallback)) = (
        sagascript_core::settings::canonical_hotkey(shortcut),
        sagascript_core::settings::canonical_hotkey(SAFE_FALLBACK_HOTKEY),
    ) else {
        return false;
    };

    canonical_shortcut == canonical_fallback
        && operational_hotkey.matches(&[SAFE_FALLBACK_HOTKEY.to_string()])
}

pub(crate) fn handle_hotkey_event(
    app: &tauri::AppHandle,
    shortcut: &str,
    state: hotkey::BareHotkeyState,
) {
    let ctrl: tauri::State<'_, SharedController> = app.state();

    match state {
        hotkey::BareHotkeyState::Pressed => {
            info!("Hotkey pressed: {shortcut}");
            #[cfg(target_os = "macos")]
            let pressed_target = platform::macos::frontmost_pid();
            #[cfg(target_os = "macos")]
            let pressed_target_opt = pressed_target;
            #[cfg(not(target_os = "macos"))]
            let pressed_target_opt: Option<i32> = None;
            let health: tauri::State<'_, hotkey::HotkeyHealth> = app.state();
            let safe_fallback = {
                let c = lock_controller(&ctrl);
                should_use_safe_fallback(
                    shortcut,
                    c.settings().validate_shortcut_configuration().is_ok(),
                    &health.operational_hotkey(),
                )
            };
            let (result, active_profile) = {
                let mut c = lock_controller(&ctrl);
                let binding = c.settings().resolved_hotkey_bindings().into_iter().find(
                    |(_, configured, _)| {
                        canonical_hotkey(configured).ok() == canonical_hotkey(shortcut).ok()
                    },
                );
                let result = match binding {
                    Some((profile, configured_shortcut, mode)) => {
                        // A failed operational configuration always uses the
                        // dedicated fallback as PTT, even if the persisted
                        // shortcut happens to be an explicit toggle binding.
                        let (mode, shortcut) = if safe_fallback {
                            (HotkeyMode::PushToTalk, SAFE_FALLBACK_HOTKEY)
                        } else {
                            (mode, configured_shortcut.as_str())
                        };
                        match c.handle_hotkey_down_for_binding(profile, mode, shortcut) {
                            Ok(result) => {
                                if safe_fallback && result == HotkeyDownResult::StartedRecording {
                                    c.use_safe_fallback_lifecycle();
                                }
                                result
                            }
                            Err(error) => {
                                error!("Hotkey down error: {error}");
                                let _ = app
                                    .emit_event(events::event::ERROR, &CommandError::from(error));
                                HotkeyDownResult::NoOp
                            }
                        }
                    }
                    None if safe_fallback => {
                        let profile = c.settings().resolved_hotkey_profiles()[0].clone();
                        match c.handle_hotkey_down_for_binding(
                            profile,
                            HotkeyMode::PushToTalk,
                            SAFE_FALLBACK_HOTKEY,
                        ) {
                            Ok(result) => {
                                if result == HotkeyDownResult::StartedRecording {
                                    c.use_safe_fallback_lifecycle();
                                }
                                result
                            }
                            Err(error) => {
                                error!("Hotkey down error: {error}");
                                let _ = app
                                    .emit_event(events::event::ERROR, &CommandError::from(error));
                                HotkeyDownResult::NoOp
                            }
                        }
                    }
                    None => {
                        warn!("Ignoring unconfigured shortcut event: {shortcut}");
                        HotkeyDownResult::NoOp
                    }
                };
                (result, c.active_hotkey_profile().cloned())
            };
            match result {
                HotkeyDownResult::StartedRecording => {
                    #[cfg(target_os = "macos")]
                    platform::macos::remember_dictation_target(pressed_target);
                    focus_diag::log(
                        "hotkey_down",
                        serde_json::json!({ "pressedTargetPid": pressed_target_opt }),
                    );
                    prewarm_engine_on_key_down(&ctrl);
                    let show_overlay = {
                        let c = lock_controller(&ctrl);
                        c.settings().show_overlay
                    };
                    let _ = app.emit_event(events::event::STATE_CHANGED, UiState::Recording);
                    if let Some(profile) = active_profile {
                        select_profile_menu(app, &profile);
                        let _ =
                            app.emit_event(events::event::ACTIVE_HOTKEY_PROFILE_CHANGED, &profile);
                    }
                    update_tray_status(app, "recording");
                    if show_overlay {
                        overlay::show(app);
                        reemit_engine_load_state(app);
                        #[cfg(target_os = "macos")]
                        {
                            let restore = platform::macos::restore_dictation_target_if_stolen();
                            let (restored, restore_error) = match &restore {
                                Ok(restored) => (Some(*restored), None),
                                Err(error) => (None, Some(error.clone())),
                            };
                            focus_diag::log(
                                "focus_restore_after_overlay",
                                serde_json::json!({
                                    "restored": restored,
                                    "restoreFailed": restore_error.is_some(),
                                }),
                            );
                            if let Err(error) = restore {
                                warn!("Could not return focus after showing the recording overlay: {error}");
                            }
                        }
                    }
                }
                HotkeyDownResult::StopRecording => {
                    stop_recording_and_transcribe(app);
                }
                HotkeyDownResult::NoOp => {}
            }
        }
        hotkey::BareHotkeyState::Released => {
            info!("Hotkey released: {shortcut}");
            handle_hotkey_release(app, &ctrl, shortcut);
        }
    }
}

/// Handle hotkey release: stop recording for push-to-talk mode
fn handle_hotkey_release(
    app: &tauri::AppHandle,
    ctrl: &tauri::State<'_, SharedController>,
    shortcut: &str,
) {
    let should_stop = {
        let mut c = lock_controller(ctrl);
        let should_stop = c.should_stop_profile_on_key_up(shortcut);
        c.note_hotkey_release(shortcut);
        should_stop
    };

    if !should_stop {
        return;
    }

    stop_recording_and_transcribe(app);
}

#[cfg(test)]
#[path = "trigger_tests.rs"]
mod tests;
