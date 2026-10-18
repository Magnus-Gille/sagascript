//! Reading the settings and the setters that change one setting each.

use sagascript_core::settings::{
    Accessibility, FileModelPreference, HotkeyProfile, Language, ModelResetNotice, Settings,
    WhisperModel,
};
use sagascript_core::transcription::model;
use tauri::State;
use tracing::info;

use super::{
    acquire_hotkey_configuration, lock_controller, persist_and_sync, try_persist_and_mirror,
    SharedController,
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
///
/// The controller takes what the language change wrote and nothing else
/// ([`mirror_language_selection`]): the default profile's language, the models
/// the new language resets, and the legacy model fields. Not the profiles'
/// shortcuts, which the file may hold unregistered. Returns the controller's
/// copy afterwards, which is what the app runs on.
pub(super) fn persist_language(
    controller: &SharedController,
    language: Language,
) -> Result<(Settings, Vec<String>), String> {
    let mut notices = Vec::new();
    let persisted = try_persist_and_mirror(
        controller,
        |settings| {
            notices = apply_language_selection(settings, language)?;
            Ok::<(), String>(())
        },
        mirror_language_selection,
    )
    .map_err(|error| error.to_string())?;
    Ok((persisted.current, notices))
}

/// What the controller takes over from `Settings::apply_language_selection`:
///
/// - `language`, `whisper_model`, `auto_select_model` and `profile_models`, all
///   written by the operation;
/// - the language of the default profile, which lives in `hotkey_profiles`. The
///   profile list stays the controller's (an implicit list becomes an explicit
///   one, as it does in the file), so the profiles keep the shortcuts the OS
///   registered, and `language` is the default profile's, as the operation
///   keeps it.
///
/// A profile list that has no profile of that id has nothing to follow.
fn mirror_language_selection(memory: &mut Settings, written: &Settings) {
    let default = written.default_profile();
    let mut profiles = memory.resolved_hotkey_profiles();
    if let Some(profile) = profiles.iter_mut().find(|profile| profile.id == default.id) {
        profile.language = default.language;
    }
    memory.language = profiles[HotkeyProfile::default_index(&profiles)].language;
    memory.hotkey_profiles = profiles;
    memory.whisper_model = written.whisper_model;
    memory.auto_select_model = written.auto_select_model;
    memory.profile_models = written.profile_models.clone();
}

/// The language operation both surfaces share (`Settings::apply_language_selection`,
/// #373), with the notices as the plain messages the command returns. It keeps
/// the profile and dictionary validation ahead of every other change.
fn apply_language_selection(
    settings: &mut Settings,
    language: Language,
) -> Result<Vec<String>, String> {
    settings
        .apply_language_selection(language)
        .map(|notices| ModelResetNotice::messages(&notices))
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
    change_profile_model(
        &controller,
        |settings| settings.set_whisper_model(model),
        mirror_whisper_model,
    )
}

#[tauri::command]
pub async fn set_pianissimo_dictation(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    change_profile_model(
        &controller,
        |settings| settings.set_pianissimo_dictation(enabled),
        mirror_pianissimo_dictation,
    )
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
    change_profile_model(
        &controller,
        |settings| settings.set_profile_model(&profile_id, preference),
        mirror_profile_models,
    )
}

#[tauri::command]
pub async fn set_auto_select_model(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    change_profile_model(
        &controller,
        |settings| settings.set_auto_select_model(enabled),
        mirror_auto_select_model,
    )
}

/// A change of a profile's model: the one place the commands above end, with
/// the operation they apply as `change` (the same `Settings` operations the
/// CLI's `config` uses, #373) and the fields it writes as `mirror`. Keeps what
/// `set_profile_model` always did around it: no change while a dictation is
/// recording, and the engine unloaded or warmed when the change needs it.
fn change_profile_model(
    controller: &SharedController,
    change: impl FnOnce(&mut Settings) -> Result<(), String>,
    mirror: impl FnOnce(&mut Settings, &Settings),
) -> Result<(), String> {
    let _recording_lease =
        acquire_hotkey_configuration(controller).map_err(|error| error.to_string())?;
    let changed = persist_profile_model(controller, change, mirror)?;
    // Outside the controller lock: an unload talks to the engine host.
    sagascript_core::transcription::pianissimo_backend::unload_if_unused(&changed.settings);
    if changed.warm {
        sagascript_core::transcription::pianissimo_backend::warm_in_background("profile_change");
    }
    Ok(())
}

/// What a change of a profile's model leaves for the command to act on.
pub(super) struct ProfileModelChange {
    /// The controller's settings after the change.
    pub(super) settings: Settings,
    /// Whether the change newly puts a profile on the engine host, so the
    /// engine should be loaded now.
    pub(super) warm: bool,
}

/// The persistence half of [`change_profile_model`]: the file and the
/// controller, and the decision about warming an engine. Nothing is started
/// here; the command does that with the answer.
///
/// The decision compares the controller's copy before and after the change.
/// Both are demoted for this host, so a host that cannot run Pianissimo never
/// sees a profile "newly" on it because the file selects it.
pub(super) fn persist_profile_model(
    controller: &SharedController,
    change: impl FnOnce(&mut Settings) -> Result<(), String>,
    mirror: impl FnOnce(&mut Settings, &Settings),
) -> Result<ProfileModelChange, String> {
    let persisted =
        try_persist_and_mirror(controller, change, mirror).map_err(|error| error.to_string())?;
    let warm = sagascript_core::transcription::pianissimo_backend::profile_change_needs_warm(
        &persisted.previous,
        &persisted.current,
    );
    Ok(ProfileModelChange {
        settings: persisted.current,
        warm,
    })
}

/// `set_profile_model` writes the model of one profile: `profile_models`.
pub(super) fn mirror_profile_models(memory: &mut Settings, written: &Settings) {
    memory.profile_models = written.profile_models.clone();
}

/// `set_whisper_model` also moves the legacy model fields that follow the
/// default profile's model.
fn mirror_whisper_model(memory: &mut Settings, written: &Settings) {
    mirror_profile_models(memory, written);
    memory.whisper_model = written.whisper_model;
    memory.auto_select_model = written.auto_select_model;
}

/// `set_auto_select_model` also moves the legacy flag.
fn mirror_auto_select_model(memory: &mut Settings, written: &Settings) {
    mirror_profile_models(memory, written);
    memory.auto_select_model = written.auto_select_model;
}

/// `set_pianissimo_dictation` also moves the legacy flag.
fn mirror_pianissimo_dictation(memory: &mut Settings, written: &Settings) {
    mirror_profile_models(memory, written);
    memory.pianissimo_dictation = written.pianissimo_dictation;
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

    let accessibility = if permission_granted {
        Accessibility::Granted
    } else {
        Accessibility::Denied
    };
    let persisted = persist_auto_paste(&controller, enabled, accessibility)?;
    info!("Auto-paste: {}", persisted.auto_paste);
    if enabled && !persisted.auto_paste {
        Err("Accessibility permission is required before auto-paste can be enabled".to_string())
    } else {
        Ok(())
    }
}

/// The persistence half of `set_auto_paste`: the operation both surfaces share
/// (`Settings::set_auto_paste`, #373) with what the command found out about the
/// permission. The command adds the OS probe and the refusal the user sees.
pub(super) fn persist_auto_paste(
    controller: &SharedController,
    enabled: bool,
    accessibility: Accessibility,
) -> Result<Settings, String> {
    persist_and_sync(controller, |settings| {
        settings.set_auto_paste(enabled, accessibility);
    })
    .map_err(|error| error.to_string())
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
pub(super) fn persist_show_overlay(
    controller: &SharedController,
    enabled: bool,
) -> Result<Settings, String> {
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
    persist_vad_enabled(&controller, enabled)?;
    // Fetch the Silero VAD model so it's ready for the next dictation. Done
    // after releasing the lock (no lock held across await). The CLI does not:
    // `transcribe --vad` fetches it on first use.
    if enabled {
        info!("Verifying or downloading VAD model...");
        model::download_vad_model(|_, _| {})
            .await
            .map_err(|e| format!("Failed to download VAD model: {e}"))?;
    }
    info!("VAD enabled: {enabled}");
    Ok(())
}

/// The persistence half of `set_vad_enabled` (`Settings::set_vad_enabled`,
/// #373); the command adds the model download, which needs the network.
pub(super) fn persist_vad_enabled(
    controller: &SharedController,
    enabled: bool,
) -> Result<Settings, String> {
    persist_and_sync(controller, |settings| {
        settings.set_vad_enabled(enabled);
    })
    .map_err(|error| error.to_string())
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
