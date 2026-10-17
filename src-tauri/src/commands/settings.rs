//! Reading the settings and the setters that change one setting each.

use sagascript_core::settings::{FileModelPreference, Language, Settings, WhisperModel};
use sagascript_core::transcription::model;
use tauri::State;
use tracing::info;

use super::{
    acquire_hotkey_configuration, lock_controller, persist_and_sync, try_persist_and_sync,
    try_persist_and_sync_replacing, SharedController,
};

#[tauri::command]
pub async fn get_settings(controller: State<'_, SharedController>) -> Result<Settings, String> {
    let settings = lock_controller(&controller).settings().clone();
    Ok(settings_for_frontend(settings))
}

/// What `get_settings` sends: the settings with a legacy file's implicit
/// default profile made explicit, so the frontend always sees one. The
/// shared fixture contracts/transport/get_settings.json pins its shape (#347).
pub(crate) fn settings_for_frontend(mut settings: Settings) -> Settings {
    if settings.hotkey_profiles.is_empty() {
        settings.hotkey_profiles = settings.resolved_hotkey_profiles();
    }
    settings
}

#[tauri::command]
pub async fn set_language(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    language: Language,
) -> Result<Vec<String>, String> {
    let (persisted, notices) = persist_language(&controller, language)?;
    crate::update_profiles_menu(&app, &persisted.resolved_hotkey_profiles());
    info!("Language set to {:?}", language);
    Ok(notices)
}

/// The persistence half of `set_language`: the file and the controller, with
/// the notices about profile models the new language reset. The command adds
/// the profiles menu, which needs the app handle.
fn persist_language(
    controller: &SharedController,
    language: Language,
) -> Result<(Settings, Vec<String>), String> {
    let mut notices = Vec::new();
    let persisted = try_persist_and_sync(controller, |settings| {
        notices = apply_language_selection(settings, language)?;
        Ok::<(), String>(())
    })
    .map_err(|error| error.to_string())?;
    Ok((persisted, notices))
}

// Keep profile/dictionary validation ahead of all preset mutations.
fn apply_language_selection(
    settings: &mut Settings,
    language: Language,
) -> Result<Vec<String>, String> {
    let notices = sagascript_core::settings::ModelResetNotice::messages(
        &settings.set_default_profile_language(language)?,
    );
    settings.whisper_model = WhisperModel::recommended(language);
    settings.auto_select_model = true;
    Ok(notices)
}

#[tauri::command]
pub async fn set_onboarding_completed(
    controller: State<'_, SharedController>,
) -> Result<(), String> {
    persist_and_sync(&controller, |settings| {
        settings.has_completed_onboarding = true;
    })
    .map_err(|error| error.to_string())?;

    info!("Onboarding marked as completed");
    Ok(())
}

#[tauri::command]
pub async fn set_whisper_model(
    controller: State<'_, SharedController>,
    model: WhisperModel,
) -> Result<(), String> {
    let id = serde_json::to_value(model)
        .and_then(serde_json::from_value::<String>)
        .map_err(|error| error.to_string())?;
    let profile_id = lock_controller(&controller).settings().default_profile().id;
    set_profile_model(controller, profile_id, id).await
}

#[tauri::command]
pub async fn set_pianissimo_dictation(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    let profile_id = lock_controller(&controller).settings().default_profile().id;
    set_profile_model(
        controller,
        profile_id,
        if enabled { "pianissimo-sv" } else { "auto" }.into(),
    )
    .await
}

#[tauri::command]
pub async fn set_file_transcription_model(
    controller: State<'_, SharedController>,
    model_id: String,
) -> Result<(), String> {
    let choice = FileModelPreference::parse_id(&model_id)?;
    persist_and_sync(&controller, |settings| {
        settings.file_transcription_model = choice;
    })
    .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn set_profile_model(
    controller: State<'_, SharedController>,
    profile_id: String,
    model_id: String,
) -> Result<(), String> {
    let preference = FileModelPreference::parse_id(&model_id)?;
    let _recording_lease =
        acquire_hotkey_configuration(&controller).map_err(|error| error.to_string())?;
    let (previous, persisted) = try_persist_and_sync_replacing(&controller, |settings| {
        settings.set_profile_model(&profile_id, preference)
    })
    .map_err(|error| error.to_string())?;
    let warm = sagascript_core::transcription::pianissimo_backend::profile_change_needs_warm(
        &previous, &persisted,
    );
    // Outside the controller lock: an unload talks to the engine host.
    sagascript_core::transcription::pianissimo_backend::unload_if_unused(&persisted);
    if warm {
        sagascript_core::transcription::pianissimo_backend::warm_in_background("profile_change");
    }
    Ok(())
}

#[tauri::command]
pub async fn set_auto_select_model(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    let (profile_id, model_id) = {
        let ctrl = lock_controller(&controller);
        let settings = ctrl.settings();
        let model_id = if enabled {
            "auto".into()
        } else {
            serde_json::to_value(settings.whisper_model)
                .and_then(serde_json::from_value::<String>)
                .map_err(|error| error.to_string())?
        };
        (settings.default_profile().id, model_id)
    };
    set_profile_model(controller, profile_id, model_id).await
}

fn effective_auto_paste(requested: bool, permission_granted: bool) -> bool {
    requested && permission_granted
}

#[tauri::command]
pub async fn set_auto_paste(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let permission_granted = !enabled || crate::platform::macos::is_accessibility_trusted();
    #[cfg(not(target_os = "macos"))]
    let permission_granted = true;

    let effective = effective_auto_paste(enabled, permission_granted);
    persist_and_sync(&controller, |settings| {
        settings.auto_paste = effective;
    })
    .map_err(|error| error.to_string())?;
    info!("Auto-paste: {effective}");
    if enabled && !permission_granted {
        Err("Accessibility permission is required before auto-paste can be enabled".to_string())
    } else {
        Ok(())
    }
}

#[tauri::command]
pub async fn set_show_overlay(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    let persisted = persist_show_overlay(&controller, enabled)?;
    crate::overlay::on_show_overlay_changed(&app, persisted.show_overlay);
    info!("Show overlay: {enabled}");
    Ok(())
}

/// The persistence half of `set_show_overlay`; the command adds the overlay
/// window update, which needs the app handle.
fn persist_show_overlay(controller: &SharedController, enabled: bool) -> Result<Settings, String> {
    persist_and_sync(controller, |settings| {
        settings.show_overlay = enabled;
    })
    .map_err(|error| error.to_string())
}

pub(super) fn ensure_known_profile(settings: &Settings, profile_id: &str) -> Result<(), String> {
    settings
        .resolved_hotkey_profiles()
        .into_iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| format!("Unknown dictation profile '{profile_id}'"))?;
    Ok(())
}

#[tauri::command]
pub async fn set_beam_size(
    controller: State<'_, SharedController>,
    beam_size: u32,
) -> Result<(), String> {
    persist_and_sync(&controller, |settings| {
        settings.beam_size = beam_size;
    })
    .map_err(|error| error.to_string())?;
    info!("Beam size: {beam_size}");
    Ok(())
}

#[tauri::command]
pub async fn set_temperature_fallback(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    persist_and_sync(&controller, |settings| {
        settings.temperature_fallback = enabled;
    })
    .map_err(|error| error.to_string())?;
    info!("Temperature fallback: {enabled}");
    Ok(())
}

#[tauri::command]
pub async fn set_vad_enabled(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    persist_and_sync(&controller, |settings| {
        settings.vad_enabled = enabled;
    })
    .map_err(|error| error.to_string())?;
    // Fetch the Silero VAD model so it's ready for the next dictation. Done
    // after releasing the lock (no lock held across await).
    if enabled {
        info!("Verifying or downloading VAD model...");
        model::download_vad_model(|_, _| {})
            .await
            .map_err(|e| format!("Failed to download VAD model: {e}"))?;
    }
    info!("VAD enabled: {enabled}");
    Ok(())
}

/// `engine_prewarm`: `off`, `on_app_start` or `on_key_down`.
#[tauri::command]
pub async fn set_engine_prewarm(
    controller: State<'_, SharedController>,
    mode: String,
) -> Result<(), String> {
    let mode = sagascript_core::settings::EnginePrewarm::parse_id(&mode)?;
    persist_and_sync(&controller, |settings| {
        settings.engine_prewarm = mode;
    })
    .map_err(|error| error.to_string())?;
    info!("Engine prewarm: {}", mode.id());
    Ok(())
}

/// Minutes of inactivity before the engine host unloads its model (0 = never).
/// Takes effect the next time the engine client is created (next Pianissimo use
/// after the settings change).
#[tauri::command]
pub async fn set_engine_idle_unload_minutes(
    controller: State<'_, SharedController>,
    minutes: u32,
) -> Result<(), String> {
    persist_and_sync(&controller, |settings| {
        settings.engine_idle_unload_minutes = minutes;
    })
    .map_err(|error| error.to_string())?;
    info!("Engine idle unload: {minutes} min");
    Ok(())
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
