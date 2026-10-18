use sagascript_core::settings::HotkeyProfile;
use sagascript_core::sync::lock_or_recover;
use tauri::{menu::MenuItem, Manager};
use tracing::error;

use super::{SharedProfileMenuState, PROFILE_MENU_LOCK};
use crate::commands::{lock_controller, SharedController};

fn format_menu_shortcut(shortcut: &str) -> String {
    let parts: Vec<(&str, String)> = shortcut
        .split('+')
        .map(|part| {
            let original = part.trim();
            let normalized = match original.to_ascii_lowercase().as_str() {
                "commandorcontrol" | "commandorctrl" | "cmdorctrl" | "cmdorcontrol" => {
                    if cfg!(target_os = "macos") {
                        "command"
                    } else {
                        "control"
                    }
                }
                _ => original,
            };
            (original, normalized.to_ascii_lowercase())
        })
        .collect();
    let mut label = String::new();
    for (aliases, symbol) in [
        (&["control", "ctrl"][..], "⌃"),
        (&["alt", "option"][..], "⌥"),
        (&["shift"][..], "⇧"),
        (&["super", "command", "cmd"][..], "⌘"),
    ] {
        if parts
            .iter()
            .any(|(_, normalized)| aliases.contains(&normalized.as_str()))
        {
            label.push_str(symbol);
        }
    }
    for (original, normalized) in parts {
        if matches!(
            normalized.as_str(),
            "control" | "ctrl" | "alt" | "option" | "shift" | "super" | "command" | "cmd"
        ) {
            continue;
        }
        label.push_str(match normalized.as_str() {
            "space" => "Space",
            "arrowup" | "up" => "↑",
            "arrowdown" | "down" => "↓",
            "arrowleft" | "left" => "←",
            "arrowright" | "right" => "→",
            _ => original,
        });
    }
    label
}

fn profile_menu_label(profile: &HotkeyProfile, selected: bool) -> String {
    format!(
        "{}{} — {} · {}",
        if selected { "✓ " } else { "" },
        profile.name,
        profile.language.display_name(),
        format_menu_shortcut(&profile.shortcut)
    )
}

pub(super) fn create_profile_menu_items(
    app: &impl tauri::Manager<tauri::Wry>,
    profiles: &[HotkeyProfile],
    selected_profile_id: Option<&str>,
) -> tauri::Result<Vec<MenuItem<tauri::Wry>>> {
    let mut items = Vec::with_capacity(profiles.len() + 1);
    for profile in profiles {
        items.push(MenuItem::with_id(
            app,
            format!("profile_shortcut_{}", profile.id),
            profile_menu_label(profile, selected_profile_id == Some(profile.id.as_str())),
            false,
            None::<&str>,
        )?);
    }
    items.push(MenuItem::with_id(
        app,
        "manage_profiles",
        "Edit Profiles…",
        true,
        None::<&str>,
    )?);
    Ok(items)
}

pub(crate) fn update_profiles_menu(app: &tauri::AppHandle, profiles: &[HotkeyProfile]) {
    let state: tauri::State<'_, SharedProfileMenuState> = app.state();
    let mut state = lock_or_recover(&state, PROFILE_MENU_LOCK);
    let Some(state) = state.as_mut() else {
        return;
    };

    let selected_profile_id = state
        .selected_profile_id
        .as_deref()
        .filter(|selected| profiles.iter().any(|profile| profile.id == *selected))
        .map(str::to_string)
        .or_else(|| profiles.first().map(|profile| profile.id.clone()));

    for item in state.items.drain(..) {
        if let Err(error) = state.submenu.remove(&item) {
            error!("Failed to remove stale tray profile item: {error}");
        }
    }
    match create_profile_menu_items(app, profiles, selected_profile_id.as_deref()) {
        Ok(items) => {
            for item in &items {
                if let Err(error) = state.submenu.append(item) {
                    error!("Failed to add tray profile item: {error}");
                }
            }
            state.items = items;
            state.selected_profile_id = selected_profile_id;
        }
        Err(error) => error!("Failed to rebuild tray profiles menu: {error}"),
    }
}

pub(crate) fn select_profile_menu(app: &tauri::AppHandle, profile: &HotkeyProfile) {
    let profiles = {
        let controller: tauri::State<'_, SharedController> = app.state();
        let profiles = lock_controller(&controller)
            .settings()
            .resolved_hotkey_profiles();
        profiles
    };
    {
        let state: tauri::State<'_, SharedProfileMenuState> = app.state();
        let mut state = lock_or_recover(&state, PROFILE_MENU_LOCK);
        if let Some(state) = state.as_mut() {
            state.selected_profile_id = Some(profile.id.clone());
        }
    }
    update_profiles_menu(app, &profiles);
}

#[cfg(test)]
#[path = "menu_tests.rs"]
mod tests;
