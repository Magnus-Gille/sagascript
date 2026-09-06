use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::State;
use tracing::{error, info, warn};

/// Maximum time to wait for whisper inference before aborting (seconds)
const TRANSCRIPTION_TIMEOUT_SECS: u64 = 60;

/// After a timeout fires and we request an abort, how long to wait for the
/// blocking inference to actually unwind and release the warm-state lock before
/// logging that it is still stuck. The real abort callback returns within a
/// compute step or two, so this rarely elapses.
const ABORT_GRACE_SECS: u64 = 5;

use crate::app_controller::{AppController, AppState, StopRecordingOutcome};
use crate::hotkey::{HotkeyHealth, HotkeyStatus, OperationalHotkey};
use crate::hotkey::configuration::HotkeyChange;
use sagascript_core::audio::decoder;
use sagascript_core::settings::{
    validate_hotkey, HotkeyMode, HotkeyProfile, Language, PresenterConfig, Settings, WhisperModel,
};
use sagascript_core::transcription::{
    model, recommended_parallel_chunks, ContextProfile, TranscribeOptions, WhisperBackend,
    FILE_TRANSCRIBE_BEAM,
};
use sagascript_core::transcription::{
    suggest_glossary_candidates, Glossary, GlossarySuggestion, GlossarySuggestionKind,
};

/// Build the per-transcription options from the current settings. Resolves the
/// VAD model path only when VAD is enabled and the model is present (otherwise
/// VAD is silently skipped — whisper would fail on a missing model).
pub(crate) fn build_transcribe_options(settings: &Settings) -> TranscribeOptions {
    build_transcribe_options_for_profile(settings, None)
}

pub(crate) fn build_transcribe_options_for_profile(
    settings: &Settings,
    profile_id: Option<&str>,
) -> TranscribeOptions {
    let prompt = Glossary::parse(&settings.effective_glossary_source(profile_id)).decoder_prompt();
    let vad_model_path = if settings.vad_enabled {
        let p = model::vad_model_path();
        if p.exists() {
            p.to_str().map(str::to_string)
        } else {
            tracing::warn!("VAD enabled but model not downloaded — skipping VAD");
            None
        }
    } else {
        None
    };
    TranscribeOptions {
        prompt,
        beam_size: settings.beam_size,
        temperature_fallback: settings.temperature_fallback,
        vad_model_path,
        segment_timestamps: false,
        parallel_chunks: 1,
    }
}

/// Like [`build_transcribe_options`] but for file transcription: defaults to
/// beam search for quality (unless the user explicitly set a beam width), and
/// uses the file dialog's prompt when provided (otherwise the saved prompt).
pub(crate) fn build_file_transcribe_options(
    settings: &Settings,
    prompt: Option<String>,
) -> TranscribeOptions {
    let mut opts = build_transcribe_options(settings);
    if opts.beam_size < 2 {
        opts.beam_size = FILE_TRANSCRIBE_BEAM;
    }
    opts.prompt = effective_file_glossary(settings, prompt.as_deref()).decoder_prompt();
    opts
}

pub(crate) fn effective_file_glossary(settings: &Settings, prompt: Option<&str>) -> Glossary {
    Glossary::parse(&settings.effective_glossary_source_with_prompt(None, prompt))
}

struct FileTranscriptionContext {
    language: Language,
    model: WhisperModel,
    glossary: Glossary,
    options: TranscribeOptions,
}

/// Freeze language, model and dictionary together before file decoding begins.
/// A missing/Auto profile is rejected rather than silently borrowing aliases.
fn file_transcription_context(
    settings: &Settings,
    profile_id: Option<&str>,
    prompt: Option<&str>,
) -> Result<FileTranscriptionContext, String> {
    let language = if let Some(id) = profile_id {
        ensure_known_profile(settings, id)?;
        settings
            .resolved_hotkey_profiles()
            .into_iter()
            .find(|profile| profile.id == id)
            .ok_or_else(|| format!("Unknown dictation profile '{id}'"))?
            .language
    } else {
        settings.language
    };
    let glossary =
        Glossary::parse(&settings.effective_glossary_source_with_prompt(profile_id, prompt));
    let mut options = build_file_transcribe_options(settings, prompt.map(str::to_owned));
    options.prompt = glossary.decoder_prompt();
    Ok(FileTranscriptionContext {
        language,
        model: settings.effective_model_for(language),
        glossary,
        options,
    })
}

pub(crate) fn apply_glossary(text: String, glossary: &Glossary) -> String {
    let (corrected, corrections) = glossary.correct_text(&text);
    if !corrections.is_empty() {
        info!(
            correction_count = corrections.len(),
            "Applied personal glossary corrections"
        );
    }
    corrected
}

#[cfg(test)]
mod glossary_options_tests {
    use super::*;

    #[test]
    fn live_options_prime_with_canonical_terms_only() {
        let settings = Settings {
            initial_prompt: "OpenRouter = open router | open vrouter\nmerge = merch".to_string(),
            ..Settings::default()
        };
        assert_eq!(
            build_transcribe_options(&settings).prompt.as_deref(),
            Some("OpenRouter, merge")
        );
    }

    #[test]
    fn live_options_include_only_the_selected_profile_glossary() {
        let mut settings = Settings {
            initial_prompt: "Codex = code x".to_string(),
            hotkey_profiles: vec![
                HotkeyProfile {
                    id: "svenska".to_string(),
                    name: "Svenska".to_string(),
                    shortcut: "Control+Shift+Space".to_string(),
                    language: Language::Swedish,
                },
                HotkeyProfile {
                    id: "english".to_string(),
                    name: "English".to_string(),
                    shortcut: "Control+Option+Space".to_string(),
                    language: Language::English,
                },
            ],
            ..Settings::default()
        };
        settings
            .profile_glossaries
            .insert("svenska".to_string(), "mergea = mördsa".to_string());
        settings
            .profile_glossaries
            .insert("english".to_string(), "Lovable = love a ball".to_string());

        assert_eq!(
            build_transcribe_options_for_profile(&settings, Some("svenska"))
                .prompt
                .as_deref(),
            Some("Codex, mergea")
        );
    }

    #[test]
    fn file_context_overrides_the_saved_dictionary_for_one_run() {
        let settings = Settings {
            initial_prompt: "OpenRouter = open router".to_string(),
            ..Settings::default()
        };
        let prompt = Some("Cloudflare = cloud flare".to_string());
        assert_eq!(
            build_file_transcribe_options(&settings, prompt)
                .prompt
                .as_deref(),
            Some("Cloudflare")
        );
    }

    #[test]
    fn correction_helper_applies_explicit_aliases() {
        let glossary = Glossary::parse("merge = merch");
        assert_eq!(
            apply_glossary("Merch it".to_string(), &glossary),
            "merge it"
        );
    }

    #[test]
    fn gui_file_global_alias_does_not_rewrite_english() {
        let settings = Settings {
            initial_prompt: "merge = merch".to_string(),
            ..Settings::default()
        };
        let glossary = effective_file_glossary(&settings, None);
        assert_eq!(
            apply_glossary("company merch".into(), &glossary),
            "company merch"
        );
        assert_eq!(glossary.decoder_prompt().as_deref(), Some("merge"));
    }

    fn scoped_file_settings() -> Settings {
        let mut settings = Settings {
            language: Language::English,
            initial_prompt: "Codex = code x\nmerge = merch".into(),
            hotkey_profiles: vec![
                HotkeyProfile {
                    id: "swedish".into(),
                    name: "Swedish".into(),
                    shortcut: "Super+S".into(),
                    language: Language::Swedish,
                },
                HotkeyProfile {
                    id: "english".into(),
                    name: "English".into(),
                    shortcut: "Super+E".into(),
                    language: Language::English,
                },
                HotkeyProfile {
                    id: "automatic".into(),
                    name: "Automatic".into(),
                    shortcut: "Super+A".into(),
                    language: Language::Auto,
                },
            ],
            ..Settings::default()
        };
        settings.profile_glossaries.insert(
            "swedish".into(),
            "merge = merch\nOpenRouter = open router".into(),
        );
        settings
            .profile_glossaries
            .insert("automatic".into(), "merge = merch".into());
        settings
    }

    #[test]
    fn gui_file_profile_pins_language_model_and_dictionary_together() {
        let settings = scoped_file_settings();
        let context = file_transcription_context(&settings, Some("swedish"), None).unwrap();
        assert_eq!(context.language, Language::Swedish);
        assert_eq!(
            context.model,
            settings.effective_model_for(Language::Swedish)
        );
        assert_eq!(
            apply_glossary("merch open router".into(), &context.glossary),
            "merge OpenRouter"
        );
        assert_eq!(context.options.prompt, context.glossary.decoder_prompt());
        for profile in [None, Some("english")] {
            let context = file_transcription_context(&settings, profile, None).unwrap();
            assert_eq!(context.language, Language::English);
            assert_eq!(
                apply_glossary("company merch".into(), &context.glossary),
                "company merch"
            );
        }
    }

    #[test]
    fn gui_file_override_is_hint_only_but_keeps_explicit_profile_aliases() {
        let context = file_transcription_context(
            &scoped_file_settings(),
            Some("swedish"),
            Some("Cloudflare = cloud flare"),
        )
        .unwrap();
        assert_eq!(
            apply_glossary("cloud flare merch".into(), &context.glossary),
            "cloud flare merge"
        );
        assert_eq!(
            context.options.prompt.as_deref(),
            Some("Cloudflare, merge, OpenRouter")
        );
    }

    #[test]
    fn gui_file_rejects_unknown_and_auto_profile_before_io() {
        let settings = scoped_file_settings();
        assert!(file_transcription_context(&settings, Some("missing"), None).is_err());
        assert!(file_transcription_context(&settings, Some("automatic"), None).is_err());
        let auto = Settings {
            language: Language::Auto,
            ..settings
        };
        let context = file_transcription_context(&auto, None, None).unwrap();
        assert_eq!(context.language, Language::Auto);
        assert_eq!(apply_glossary("merch".into(), &context.glossary), "merch");
    }

    #[test]
    fn profile_dictionary_save_validates_without_partial_mutation() {
        let mut settings = scoped_file_settings();
        let original = settings.profile_glossaries.clone();
        for profile in ["missing", "automatic"] {
            assert!(
                set_profile_glossary_in(&mut settings, profile, "new = old".into(), None).is_err()
            );
            assert_eq!(settings.profile_glossaries, original);
        }
        set_profile_glossary_in(&mut settings, "english", "OpenAI = open a i".into(), None)
            .unwrap();
        assert_eq!(settings.profile_glossaries["english"], "OpenAI = open a i");
        assert_eq!(settings.profile_glossaries["swedish"], original["swedish"]);
        assert_eq!(settings.initial_prompt, "Codex = code x\nmerge = merch");
    }

    #[test]
    fn scoped_file_glossary_preserves_cross_fragment_phrase_matching() {
        let context =
            file_transcription_context(&scoped_file_settings(), Some("swedish"), None).unwrap();
        let (fragments, corrections) = context
            .glossary
            .correct_fragments(&["open", " router merch"]);
        assert_eq!(fragments.concat(), "OpenRouter merge");
        assert_eq!(corrections.len(), 2);
    }
}

/// Shared app state type — uses std::sync::Mutex (not tokio) because
/// cpal::Stream is !Send and we need sync access from Tauri commands
pub type SharedController = Mutex<AppController>;

/// Shared whisper backend — separate from AppController to avoid holding
/// the controller lock during blocking transcription
pub type SharedWhisper = Arc<WhisperBackend>;

#[tauri::command]
pub async fn get_active_hotkey_profile(
    controller: State<'_, SharedController>,
) -> Result<Option<HotkeyProfile>, String> {
    Ok(controller.lock().unwrap().active_hotkey_profile().cloned())
}

// -- State queries --

#[tauri::command]
pub async fn get_state(controller: State<'_, SharedController>) -> Result<AppState, String> {
    let ctrl = controller.lock().unwrap();
    Ok(ctrl.state())
}

#[tauri::command]
pub async fn get_settings(controller: State<'_, SharedController>) -> Result<Settings, String> {
    let ctrl = controller.lock().unwrap();
    let mut settings = ctrl.settings().clone();
    if settings.hotkey_profiles.is_empty() {
        settings.hotkey_profiles = settings.resolved_hotkey_profiles();
    }
    Ok(settings)
}

#[tauri::command]
pub async fn get_last_transcription(
    controller: State<'_, SharedController>,
) -> Result<Option<String>, String> {
    let ctrl = controller.lock().unwrap();
    Ok(ctrl.last_transcription().map(|s| s.to_string()))
}

#[tauri::command]
pub async fn get_last_error(
    controller: State<'_, SharedController>,
) -> Result<Option<String>, String> {
    let ctrl = controller.lock().unwrap();
    Ok(ctrl.last_error().map(|s| s.to_string()))
}

#[tauri::command]
pub async fn is_model_ready(controller: State<'_, SharedController>) -> Result<bool, String> {
    let ctrl = controller.lock().unwrap();
    Ok(ctrl.is_model_ready())
}

/// Returns the display name of the currently loaded (or effective) model
#[tauri::command]
pub async fn get_loaded_model(
    controller: State<'_, SharedController>,
    whisper: State<'_, SharedWhisper>,
) -> Result<LoadedModelInfo, String> {
    let ctrl = controller.lock().unwrap();
    let effective = ctrl.settings().effective_model();
    let loaded = whisper.loaded_model();
    Ok(LoadedModelInfo {
        effective_model: effective.display_name().to_string(),
        effective_model_id: serde_json::to_value(effective)
            .and_then(serde_json::from_value::<String>)
            .unwrap_or_else(|_| format!("{:?}", effective)),
        loaded_model: loaded.map(|m| m.display_name().to_string()),
        is_loaded: loaded == Some(effective),
        is_downloaded: model::is_model_downloaded(effective),
    })
}

// -- Settings mutations --

#[tauri::command]
pub async fn set_language(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    language: Language,
) -> Result<(), String> {
    set_language_for_controller(&controller, &app, language)
}

fn set_language_for_controller(
    controller: &State<'_, SharedController>,
    app: &tauri::AppHandle,
    language: Language,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::try_update(|settings| {
        settings.set_legacy_language(language)
    })?;
    let mut ctrl = controller.lock().unwrap();
    ctrl.update_settings(persisted.clone());
    drop(ctrl);
    crate::update_profiles_menu(app, &persisted.resolved_hotkey_profiles());
    info!("Language set to {:?}", language);
    Ok(())
}

#[tauri::command]
pub async fn set_onboarding_completed(
    controller: State<'_, SharedController>,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.has_completed_onboarding = true;
    })?;
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().has_completed_onboarding = persisted.has_completed_onboarding;
    drop(ctrl);

    info!("Onboarding marked as completed");
    Ok(())
}

#[tauri::command]
pub async fn set_whisper_model(
    controller: State<'_, SharedController>,
    model: WhisperModel,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.whisper_model = model;
        settings.auto_select_model = false;
    })?;
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().whisper_model = persisted.whisper_model;
    ctrl.settings_mut().auto_select_model = persisted.auto_select_model;
    info!("Model set to {:?}", model);
    Ok(())
}

#[tauri::command]
pub async fn set_auto_select_model(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.auto_select_model = enabled;
    })?;
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().auto_select_model = persisted.auto_select_model;
    info!("Auto-select model: {enabled}");
    Ok(())
}

#[tauri::command]
pub async fn set_hotkey_mode(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    health: State<'_, HotkeyHealth>,
    mode: HotkeyMode,
) -> Result<(), String> {
    apply_hotkey_change(app, controller, health, HotkeyChange::Mode(mode))
}

#[tauri::command]
pub async fn set_presenter_config(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    health: State<'_, HotkeyHealth>,
    config: PresenterConfig,
) -> Result<(), String> {
    apply_hotkey_change(app, controller, health, HotkeyChange::Presenter(config))
}

#[tauri::command]
pub async fn set_hotkey(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    health: State<'_, HotkeyHealth>,
    shortcut: String,
) -> Result<(), String> {
    validate_hotkey(&shortcut)?;
    let mut profiles = controller
        .lock()
        .unwrap()
        .settings()
        .resolved_hotkey_profiles();
    let profile_index = profiles
        .iter()
        .position(|profile| profile.id == "default")
        .unwrap_or(0);
    profiles[profile_index].shortcut = shortcut;
    set_hotkey_profiles(app, controller, health, profiles).await
}

#[tauri::command]
pub async fn set_hotkey_profiles(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    health: State<'_, HotkeyHealth>,
    profiles: Vec<HotkeyProfile>,
) -> Result<(), String> {
    apply_hotkey_change(app, controller, health, HotkeyChange::Profiles(profiles))
}

pub(crate) struct HotkeyConfigurationLease<'a>(&'a SharedController);

impl Drop for HotkeyConfigurationLease<'_> {
    fn drop(&mut self) {
        self.0.lock().unwrap().end_hotkey_configuration_change();
    }
}

pub(crate) fn acquire_hotkey_configuration(
    controller: &SharedController,
) -> Result<HotkeyConfigurationLease<'_>, String> {
    if !controller.lock().unwrap().begin_hotkey_configuration_change() {
        return Err("Finish or cancel the active dictation before changing shortcuts".into());
    }
    Ok(HotkeyConfigurationLease(controller))
}

fn apply_hotkey_change(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    health: State<'_, HotkeyHealth>,
    update: HotkeyChange,
) -> Result<(), String> {
    use tauri::Emitter;
    let _transition = health.transition_guard();
    let _recording_lease = acquire_hotkey_configuration(&controller)?;
    let old_settings = controller.lock().unwrap().settings().clone();
    let candidate = update.prepare(&sagascript_core::settings::store::load())?;
    let new_shortcuts = candidate.resolved_shortcuts();
    let new_primary = candidate.hotkey.clone();
    let old_shortcut = old_settings.hotkey.clone();
    let old_operational = health.operational_hotkey();
    if old_operational == OperationalHotkey::Unknown {
        return Err(
            "Hotkey registration state is unknown after an earlier OS error; restart Sagascript before changing profiles"
                .to_string(),
        );
    }

    let old_shortcuts = old_settings.resolved_shortcuts();
    if new_shortcuts == old_shortcuts
        && old_operational.matches(&new_shortcuts)
        && !health.is_failed()
    {
        let persisted = sagascript_core::settings::store::try_update(|settings| {
            update.apply_registered(settings, &new_shortcuts)
        })?;
        controller
            .lock()
            .unwrap()
            .update_settings(persisted.clone());
        let change = health.record(&new_primary, None, old_operational);
        if change.changed {
            let _ = app.emit(
                crate::events::event::HOTKEY_REGISTRATION_CHANGED,
                &change.status,
            );
        }
        crate::update_profiles_menu(&app, &persisted.resolved_hotkey_profiles());
        return Ok(());
    }

    if let OperationalHotkey::Registered(old_shortcuts) = &old_operational {
        if let Err(error) = crate::hotkey::unregister_shortcuts(&app, old_shortcuts) {
            error!("Failed to unregister operational hotkeys: {error}");
            let change = health.record(
                &old_shortcut,
                Some(format!(
                    "failed to unregister active hotkeys: {error}; operational state is unknown"
                )),
                OperationalHotkey::Unknown,
            );
            if change.changed {
                let _ = app.emit(
                    crate::events::event::HOTKEY_REGISTRATION_CHANGED,
                    &change.status,
                );
            }
            return Err(format!("Failed to unregister active hotkeys: {error}"));
        }
    }

    if let Err(error) = crate::hotkey::register_shortcuts(&app, &new_shortcuts) {
        if let Err(cleanup_error) = crate::hotkey::unregister_shortcuts(&app, &new_shortcuts) {
            let change = health.record(
                &old_shortcut,
                Some(format!("new registration failed: {error}; partial registration cleanup failed: {cleanup_error}; operational state is unknown")),
                OperationalHotkey::Unknown,
            );
            if change.changed {
                let _ = app.emit(
                    crate::events::event::HOTKEY_REGISTRATION_CHANGED,
                    &change.status,
                );
            }
            return Err(format!(
                "Failed to register hotkey profiles: {error}; cleanup failed: {cleanup_error}"
            ));
        }
        let change = match &old_operational {
            OperationalHotkey::Registered(old_shortcuts) => match crate::hotkey::register_shortcuts(&app, old_shortcuts) {
                Ok(()) => health.record(&old_shortcut, None, old_operational.clone()),
                Err(rollback_error) => {
                    health.record(
                        &old_shortcut,
                        Some(format!("new registration failed: {error}; restoring previous hotkeys also failed: {rollback_error}")),
                        OperationalHotkey::Inactive,
                    )
                }
            },
            OperationalHotkey::Inactive => health.record(
                &old_shortcut,
                Some("no previous hotkey was active".to_string()),
                OperationalHotkey::Inactive,
            ),
            OperationalHotkey::Unknown => unreachable!("unknown state returned above"),
        };
        if change.changed {
            let _ = app.emit(
                crate::events::event::HOTKEY_REGISTRATION_CHANGED,
                &change.status,
            );
        }
        return Err(format!("Failed to register hotkey profiles: {error}"));
    }

    health.record(
        &old_shortcut,
        Some("hotkey profile change is pending persistence".to_string()),
        OperationalHotkey::registered_many(&new_shortcuts),
    );

    let persisted = match sagascript_core::settings::store::try_update(|settings| {
        update.apply_registered(settings, &new_shortcuts)
    }) {
        Ok(settings) => settings,
        Err(save_error) => {
            let unregister_error = crate::hotkey::unregister_shortcuts(&app, &new_shortcuts).err();
            let rollback_error = if unregister_error.is_none() {
                match &old_operational {
                    OperationalHotkey::Registered(old_shortcuts) => {
                        crate::hotkey::register_shortcuts(&app, old_shortcuts).err()
                    }
                    OperationalHotkey::Inactive => None,
                    OperationalHotkey::Unknown => unreachable!("unknown state returned above"),
                }
            } else {
                None
            };
            let unknown = unregister_error.is_some();
            let restored_operational = if unknown {
                OperationalHotkey::Unknown
            } else if rollback_error.is_some() {
                OperationalHotkey::Inactive
            } else {
                old_operational.clone()
            };
            let health_error = unknown
                .then(|| {
                    "failed to unregister unpersisted hotkeys; operational state is unknown"
                        .to_string()
                })
                .or_else(|| {
                    rollback_error
                        .as_ref()
                        .map(|error| format!("failed to restore previous hotkeys: {error}"))
                });
            let change = health.record(&old_shortcut, health_error, restored_operational);
            if change.changed {
                let _ = app.emit(
                    crate::events::event::HOTKEY_REGISTRATION_CHANGED,
                    &change.status,
                );
            }
            return Err(format!("Failed to persist hotkey profiles: {save_error}"));
        }
    };

    {
        let mut ctrl = controller.lock().unwrap();
        ctrl.update_settings(persisted.clone());
        ctrl.hotkey_service_mut().set_shortcut(&persisted.hotkey);
    }

    let change = health.record(
        &new_primary,
        None,
        OperationalHotkey::registered_many(&new_shortcuts),
    );
    if change.changed {
        let _ = app.emit(
            crate::events::event::HOTKEY_REGISTRATION_CHANGED,
            &change.status,
        );
    }

    crate::update_profiles_menu(&app, &persisted.resolved_hotkey_profiles());

    info!(
        "Hotkey profiles changed: {} registered",
        new_shortcuts.len()
    );
    Ok(())
}

/// Retry the shortcuts currently persisted on disk.
///
/// This is used after macOS grants Accessibility while the app is already
/// running. Reading the file again matters for CLI-driven changes: the file
/// can contain a requested bare F-key while the controller deliberately keeps
/// the previous operational shortcut after registration failed closed.
///
/// If the AppKit event monitor itself was never installed (setup failure),
/// reinstall it here on the macOS main thread before re-registering, so a
/// retry can recover without an app restart. Install and registration
/// failures still surface through the normal health path below.
#[tauri::command]
pub async fn retry_hotkey_registration(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    health: State<'_, HotkeyHealth>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    if !crate::hotkey::bare_function_key_monitor_installed() {
        let app_for_install = app.clone();
        let (installed_tx, installed_rx) = std::sync::mpsc::channel();
        app.run_on_main_thread(move || {
            let result = crate::hotkey::install_bare_function_key_monitor(&app_for_install);
            let _ = installed_tx.send(result);
        })
        .map_err(|error| error.to_string())?;
        // The main thread runs the install promptly; a disconnected channel
        // means the app is shutting down, so fall through and let the
        // registration attempt below report the monitor as unavailable.
        if let Ok(Err(error)) = installed_rx.recv() {
            tracing::warn!("F13-F24 event monitor reinstall failed: {error}");
        }
    }
    let profiles = sagascript_core::settings::store::load().resolved_hotkey_profiles();
    set_hotkey_profiles(app, controller, health, profiles).await
}

/// Current hotkey registration health — whether the last registration
/// attempt (at startup, from this command, or from the settings-file
/// watcher's hot-reload) actually succeeded. Reads the process-wide flag
/// rather than querying the global-shortcut plugin's `is_registered()`,
/// which only tells you *a* shortcut is bound, not whether *our* most recent
/// attempt to bind it succeeded.
#[tauri::command]
pub async fn hotkey_status(health: State<'_, HotkeyHealth>) -> Result<HotkeyStatus, String> {
    Ok(health.status())
}

// -- Recording --

#[tauri::command]
pub async fn start_recording(controller: State<'_, SharedController>) -> Result<(), String> {
    let mut ctrl = controller.lock().unwrap();
    gui_start_recording_result(ctrl.start_recording())
}

#[tauri::command]
pub async fn start_training_recording(
    controller: State<'_, SharedController>,
    profile_id: String,
) -> Result<(), String> {
    let mut ctrl = controller.lock().unwrap();
    ensure_known_profile(ctrl.settings(), &profile_id)?;
    let profile = ctrl
        .settings()
        .resolved_hotkey_profiles()
        .into_iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| format!("Unknown dictation profile '{profile_id}'"))?;
    gui_start_recording_result(ctrl.start_training_recording_for_profile(profile))
}

fn gui_start_recording_result(
    result: Result<bool, sagascript_core::error::DictationError>,
) -> Result<(), String> {
    match result {
        Ok(true) => Ok(()),
        Ok(false) => Err(
            "Cannot start recording while Sagascript is busy. Wait for the current transcription to finish."
                .to_string(),
        ),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod gui_recording_tests {
    use super::gui_start_recording_result;

    #[test]
    fn gui_start_while_transcribing_returns_busy_error() {
        let error = gui_start_recording_result(Ok(false)).unwrap_err();

        assert!(error.contains("busy"));
        assert!(error.contains("current transcription"));
    }
}

#[tauri::command]
pub async fn stop_and_transcribe(
    controller: State<'_, SharedController>,
    whisper: State<'_, SharedWhisper>,
) -> Result<String, String> {
    stop_and_transcribe_impl(controller, whisper, true)
        .await
        .map(|transcript| transcript.effective_text)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TrainingTranscript {
    pub raw_text: String,
    pub effective_text: String,
}

#[tauri::command]
pub async fn stop_and_transcribe_training(
    controller: State<'_, SharedController>,
    whisper: State<'_, SharedWhisper>,
) -> Result<TrainingTranscript, String> {
    stop_and_transcribe_impl(controller, whisper, false).await
}

fn not_recording_result(allow_empty: bool) -> Result<TrainingTranscript, String> {
    if allow_empty {
        Ok(TrainingTranscript {
            raw_text: String::new(),
            effective_text: String::new(),
        })
    } else {
        Err("No training recording is active".to_string())
    }
}

async fn stop_and_transcribe_impl(
    controller: State<'_, SharedController>,
    whisper: State<'_, SharedWhisper>,
    allow_empty_not_recording: bool,
) -> Result<TrainingTranscript, String> {
    let (audio, language, effective_model, opts, glossary) = {
        let mut ctrl = controller.lock().unwrap();
        // A late/duplicate normal stop racing the hotkey path remains an
        // Ok-empty no-op so it cannot clobber an in-flight transcription.
        // Teach owns its recording lifecycle, so a missing recording there is
        // an actionable UI error instead of a silently accepted empty result.
        let audio = match ctrl.stop_recording_guarded() {
            StopRecordingOutcome::NotRecording => {
                return not_recording_result(allow_empty_not_recording)
            }
            // Capture/resample failure (finding 4): the controller already
            // recorded the error and returned to Idle; surface the real error.
            StopRecordingOutcome::Failed(msg) => return Err(msg),
            StopRecordingOutcome::Stopped(audio) => audio,
        };
        let language = ctrl.language();
        let effective_model = ctrl.settings().effective_model_for(language);
        let profile_id = ctrl
            .active_hotkey_profile()
            .map(|profile| profile.id.as_str());
        let opts = build_transcribe_options_for_profile(ctrl.settings(), profile_id);
        let glossary = Glossary::parse(&ctrl.settings().effective_glossary_source(profile_id));
        (audio, language, effective_model, opts, glossary)
    };

    if audio.is_empty() {
        let error = "No audio captured".to_string();
        let completion = controller
            .lock()
            .unwrap()
            .finish_transcription(Err(error.clone()));
        return Err(completion.err().unwrap_or(error));
    }

    // Every outcome after recording stops must flow through
    // `finish_transcription`: stop_recording_guarded has already moved the
    // controller to Transcribing, so returning early would wedge subsequent
    // recording attempts until the app restarts.
    let result = {
        // Run blocking transcription on a separate thread with a timeout. On timeout
        // we now trigger a REAL abort (the whisper-rs abort callback wired in
        // WhisperBackend): request_abort() flips the flag whisper.cpp checks between
        // compute steps, so the blocking task returns promptly and releases the warm
        // state instead of running to completion and wedging the pipeline. The handle
        // is kept borrowed (`&mut fut`) across the timeout so we can await its actual
        // exit after abort and log whether the lock was released.
        let whisper_ref = whisper.inner().clone();
        let mut fut = tokio::task::spawn_blocking(move || {
            let mut timings =
                sagascript_core::transcription::whisper_backend::DictationTimings::default();
            let result = whisper_ref.transcribe_live_dictation(
                effective_model,
                &audio,
                language,
                &opts,
                &mut timings,
            );
            (result, timings)
        });

        let timeout = Duration::from_secs(TRANSCRIPTION_TIMEOUT_SECS);
        match tokio::time::timeout(timeout, &mut fut).await {
            Ok(Ok((result, timings))) => {
                let mut ctrl = controller.lock().unwrap();
                if timings.model_acquisition_started {
                    ctrl.record_phase(
                        "model_acquisition",
                        Duration::from_secs_f64(timings.model_ms / 1000.0),
                    );
                    ctrl.record_model_cache(timings.model_cached);
                }
                if timings.inference_started {
                    ctrl.record_phase(
                        "inference",
                        Duration::from_secs_f64(timings.inference_ms / 1000.0),
                    );
                }
                result.map_err(|error| error.to_string())
            }
            Ok(Err(error)) => Err(format!("Transcription task failed: {error}")),
            Err(_) => {
                warn!("Transcription timed out after {TRANSCRIPTION_TIMEOUT_SECS}s — requesting abort");
                whisper.request_abort();
                // Give the aborted inference a brief grace to unwind, and log which
                // outcome occurred so a genuine hang is distinguishable from a clean
                // abort.
                match tokio::time::timeout(Duration::from_secs(ABORT_GRACE_SECS), &mut fut).await {
                    Ok(_) => info!("Aborted transcription task exited — warm-state lock released"),
                    Err(_) => error!(
                        "Transcription task still running {ABORT_GRACE_SECS}s after abort — the \
                         warm state may stay locked until it unwinds; further transcriptions will \
                         report ModelBusy rather than block forever"
                    ),
                }
                Err(format!(
                    "Transcription timed out after {TRANSCRIPTION_TIMEOUT_SECS}s (inference aborted)"
                ))
            }
        }
    };

    let training_result = result.map(|raw_text| TrainingTranscript {
        effective_text: apply_glossary(raw_text.clone(), &glossary),
        raw_text,
    });

    // NOTE: auto-paste is NOT done here — enigo's macOS TIS APIs crash if
    // called from a tokio worker thread (SIGTRAP in dispatch_assert_queue).
    // The hotkey path in main.rs handles paste via run_on_main_thread(). This
    // command returns the text to the frontend for display instead.
    let completion = training_result
        .as_ref()
        .map(|transcript| transcript.effective_text.clone())
        .map_err(Clone::clone);
    controller
        .lock()
        .unwrap()
        .finish_transcription(completion)?;
    training_result
}

#[tauri::command]
pub async fn transcribe_training_file(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    whisper: State<'_, SharedWhisper>,
    file_path: String,
    profile_id: String,
) -> Result<TrainingTranscript, String> {
    use tauri::Emitter;

    let path = std::path::PathBuf::from(file_path);
    let audio = tokio::task::spawn_blocking(move || decoder::decode_audio_file(&path))
        .await
        .map_err(|error| format!("Decode task failed: {error}"))?
        .map_err(|error| error.to_string())?;
    if audio.is_empty() {
        return Err("No audio decoded from file".to_string());
    }

    let (language, effective_model, opts, glossary) = {
        let ctrl = controller.lock().unwrap();
        ensure_known_profile(ctrl.settings(), &profile_id)?;
        let profile = ctrl
            .settings()
            .resolved_hotkey_profiles()
            .into_iter()
            .find(|profile| profile.id == profile_id)
            .expect("validated profile must exist");
        (
            profile.language,
            ctrl.settings().effective_model_for(profile.language),
            build_transcribe_options_for_profile(ctrl.settings(), Some(&profile_id)),
            Glossary::parse(&ctrl.settings().effective_glossary_source(Some(&profile_id))),
        )
    };

    if whisper.needs_reload(effective_model) {
        let _ = app.emit(crate::events::event::STATE_CHANGED, "loading_model");
    }
    let _ = app.emit(crate::events::event::STATE_CHANGED, "transcribing");

    let whisper_ref = whisper.inner().clone();
    let progress_app = app.clone();
    let duration_seconds = (audio.len() / 16_000) as u64;
    let timeout = Duration::from_secs((duration_seconds * 6).max(TRANSCRIPTION_TIMEOUT_SECS));
    let mut task = tokio::task::spawn_blocking(move || {
        whisper_ref.with_model(effective_model, ContextProfile::FlashAttention, |backend| {
            backend.transcribe_sync_with_options(&audio, language, &opts, move |progress| {
                let _ = progress_app.emit("transcription-progress", progress);
            })
        })
    });

    let result = match tokio::time::timeout(timeout, &mut task).await {
        Ok(Ok(result)) => result.map_err(|error| error.to_string()),
        Ok(Err(error)) => Err(format!("Transcription task failed: {error}")),
        Err(_) => {
            whisper.request_abort();
            let _ = tokio::time::timeout(Duration::from_secs(ABORT_GRACE_SECS), &mut task).await;
            Err(format!(
                "Training transcription timed out after {}s",
                timeout.as_secs()
            ))
        }
    };
    let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");

    result.map(|raw_text| TrainingTranscript {
        effective_text: apply_glossary(raw_text.clone(), &glossary),
        raw_text,
    })
}

#[cfg(test)]
mod training_stop_tests {
    use super::not_recording_result;

    #[test]
    fn duplicate_normal_stop_remains_a_noop() {
        let result = not_recording_result(true).unwrap();
        assert!(result.raw_text.is_empty());
        assert!(result.effective_text.is_empty());
    }

    #[test]
    fn training_stop_without_active_recording_is_an_error() {
        let error = not_recording_result(false).unwrap_err();
        assert!(error.contains("No training recording"));
    }
}

#[tauri::command]
pub async fn cancel_recording(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
) -> Result<(), String> {
    if controller.lock().unwrap().is_presenter_session() {
        let handle = app.clone();
        app.run_on_main_thread(move || crate::presenter::cancel(&handle))
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    let mut ctrl = controller.lock().unwrap();
    ctrl.cancel_recording();
    drop(ctrl);
    crate::overlay::hide(&app);
    Ok(())
}

// -- Model management --

#[tauri::command]
pub async fn is_model_downloaded(whisper_model: WhisperModel) -> Result<bool, String> {
    Ok(model::is_model_downloaded(whisper_model))
}

#[tauri::command]
pub async fn get_model_info(
    controller: State<'_, SharedController>,
) -> Result<Vec<ModelInfo>, String> {
    let ctrl = controller.lock().unwrap();
    let language = ctrl.settings().language;
    let effective = ctrl.settings().effective_model();
    let models = WhisperModel::models_for_language(language);

    Ok(models
        .iter()
        .map(|m| ModelInfo {
            id: serde_json::to_value(m)
                .and_then(serde_json::from_value::<String>)
                .unwrap_or_else(|_| format!("{:?}", m)),
            display_name: m.display_name().to_string(),
            description: m.description().to_string(),
            size_mb: m.size_mb(),
            downloaded: model::is_model_downloaded(*m),
            active: *m == effective,
        })
        .collect())
}

/// Return the effective speech engine for one profile language. This keeps the
/// normal Dictate surface model-name-free while still giving it enough state to
/// offer a direct download when an upgraded or newly added profile is not ready.
#[tauri::command]
pub async fn get_effective_model_info(
    controller: State<'_, SharedController>,
    language: Language,
) -> Result<ModelInfo, String> {
    let ctrl = controller.lock().unwrap();
    let model = ctrl.settings().effective_model_for(language);
    Ok(ModelInfo {
        id: serde_json::to_value(model)
            .and_then(serde_json::from_value::<String>)
            .unwrap_or_else(|_| format!("{model:?}")),
        display_name: model.display_name().to_string(),
        description: model.description().to_string(),
        size_mb: model.size_mb(),
        downloaded: model::is_model_downloaded(model),
        active: true,
    })
}

// -- Model download --

#[tauri::command]
pub async fn download_model(
    app: tauri::AppHandle,
    whisper_model: WhisperModel,
) -> Result<(), String> {
    use tauri::Emitter;
    let app_handle = app.clone();
    model::download_model(whisper_model, move |downloaded, total| {
        let progress = if total > 0 {
            (downloaded as f64 / total as f64 * 100.0) as u32
        } else {
            0
        };
        let _ = app_handle.emit(
            crate::events::event::MODEL_DOWNLOAD_PROGRESS,
            serde_json::json!({
                "model": format!("{:?}", whisper_model),
                "downloaded": downloaded,
                "total": total,
                "progress": progress,
            }),
        );
    })
    .await
    .map_err(|e| e.to_string())?;

    let _ = app.emit(crate::events::event::MODEL_READY, ());
    Ok(())
}

// -- Settings toggles --

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
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.auto_paste = effective;
    })?;
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().auto_paste = persisted.auto_paste;
    info!("Auto-paste: {effective}");
    if enabled && !permission_granted {
        Err("Accessibility permission is required before auto-paste can be enabled".to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod auto_paste_tests {
    use super::effective_auto_paste;

    #[test]
    fn enabling_never_persists_true_before_accessibility_is_trusted() {
        assert!(!effective_auto_paste(true, false));
        assert!(effective_auto_paste(true, true));
        assert!(!effective_auto_paste(false, true));
    }
}

#[tauri::command]
pub async fn set_show_overlay(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.show_overlay = enabled;
    })?;
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().show_overlay = persisted.show_overlay;
    info!("Show overlay: {enabled}");
    Ok(())
}

#[tauri::command]
pub async fn set_initial_prompt(
    controller: State<'_, SharedController>,
    prompt: String,
    expected_source: Option<String>,
) -> Result<(), String> {
    let mut conflict_source = None;
    let persisted = sagascript_core::settings::store::try_update(|settings| {
        let current_source = expected_source
            .as_ref()
            .map(|_| settings.initial_prompt.clone());
        match set_initial_prompt_in(settings, &prompt, expected_source.as_deref()) {
            Ok(()) => Ok(()),
            Err(error) => {
                if error.starts_with(DICTIONARY_CHANGED_ELSEWHERE_PREFIX) {
                    conflict_source = current_source;
                }
                Err(error)
            }
        }
    });
    match persisted {
        Ok(persisted) => {
            let mut ctrl = controller.lock().unwrap();
            ctrl.settings_mut().initial_prompt = persisted.initial_prompt;
            info!("Initial prompt set ({} chars)", prompt.len());
            Ok(())
        }
        Err(error) => {
            if let (Some(expected), Some(persisted_source)) =
                (expected_source.as_deref(), conflict_source.as_deref())
            {
                let mut ctrl = controller.lock().unwrap();
                reconcile_global_dictionary_after_conflict(
                    ctrl.settings_mut(),
                    expected,
                    persisted_source,
                );
            }
            Err(error)
        }
    }
}

const DICTIONARY_CHANGED_ELSEWHERE_PREFIX: &str = "Dictionary changed elsewhere:";

fn ensure_expected_dictionary_source(
    current_source: &str,
    expected_source: Option<&str>,
) -> Result<(), String> {
    if expected_source.is_some_and(|expected| {
        !sagascript_core::settings::store::glossary_sources_match(expected, current_source)
    }) {
        return Err(format!(
            "{DICTIONARY_CHANGED_ELSEWHERE_PREFIX} the persisted source no longer matches the editor baseline"
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DictionarySourceSnapshot {
    present: bool,
    source: String,
}

fn profile_dictionary_source_snapshot(
    settings: &Settings,
    profile_id: &str,
) -> DictionarySourceSnapshot {
    settings
        .profile_glossaries
        .get(profile_id)
        .map(|source| DictionarySourceSnapshot {
            present: true,
            source: source.clone(),
        })
        .unwrap_or_else(|| DictionarySourceSnapshot {
            present: false,
            source: String::new(),
        })
}

fn reconcile_global_dictionary_after_conflict(
    settings: &mut Settings,
    expected_source: &str,
    persisted_source: &str,
) {
    if sagascript_core::settings::store::glossary_sources_match(
        &settings.initial_prompt,
        expected_source,
    ) {
        settings.initial_prompt = persisted_source.to_string();
    }
}

fn reconcile_profile_dictionary_after_conflict(
    settings: &mut Settings,
    profile_id: &str,
    expected_source: &str,
    persisted_source: &DictionarySourceSnapshot,
) {
    let current_source = settings
        .profile_glossaries
        .get(profile_id)
        .map(String::as_str)
        .unwrap_or_default();
    if !sagascript_core::settings::store::glossary_sources_match(current_source, expected_source) {
        return;
    }

    if persisted_source.present {
        settings
            .profile_glossaries
            .insert(profile_id.to_string(), persisted_source.source.clone());
    } else {
        settings.profile_glossaries.remove(profile_id);
    }
}

fn set_initial_prompt_in(
    settings: &mut Settings,
    prompt: &str,
    expected_source: Option<&str>,
) -> Result<(), String> {
    ensure_expected_dictionary_source(&settings.initial_prompt, expected_source)?;
    settings.initial_prompt = prompt.to_string();
    Ok(())
}

fn ensure_known_profile(settings: &Settings, profile_id: &str) -> Result<(), String> {
    let profile = settings
        .resolved_hotkey_profiles()
        .into_iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| format!("Unknown dictation profile '{profile_id}'"))?;
    if profile.language == Language::Auto {
        return Err(
            "Personal dictionary aliases require a profile with an explicit language".to_string(),
        );
    }
    Ok(())
}

fn set_profile_glossary_in(
    settings: &mut Settings,
    profile_id: &str,
    source: String,
    expected_source: Option<&str>,
) -> Result<(), String> {
    ensure_known_profile(settings, profile_id)?;
    let current_source = settings
        .profile_glossaries
        .get(profile_id)
        .map(String::as_str)
        .unwrap_or_default();
    ensure_expected_dictionary_source(current_source, expected_source)?;
    settings
        .profile_glossaries
        .insert(profile_id.to_owned(), source);
    Ok(())
}

#[tauri::command]
pub async fn set_profile_glossary(
    controller: State<'_, SharedController>,
    profile_id: String,
    source: String,
    expected_source: Option<String>,
) -> Result<(), String> {
    let source_len = source.chars().count();
    let mut conflict_source = None;
    let persisted = sagascript_core::settings::store::try_update(|settings| {
        let current_source = expected_source
            .as_ref()
            .map(|_| profile_dictionary_source_snapshot(settings, &profile_id));
        match set_profile_glossary_in(settings, &profile_id, source, expected_source.as_deref()) {
            Ok(()) => Ok(()),
            Err(error) => {
                if error.starts_with(DICTIONARY_CHANGED_ELSEWHERE_PREFIX) {
                    conflict_source = current_source;
                }
                Err(error)
            }
        }
    });
    match persisted {
        Ok(persisted) => {
            controller.lock().unwrap().update_settings(persisted);
            info!(source_len, "Profile personal dictionary updated");
            Ok(())
        }
        Err(error) => {
            if let (Some(expected), Some(persisted_source)) =
                (expected_source.as_deref(), conflict_source.as_ref())
            {
                let mut ctrl = controller.lock().unwrap();
                reconcile_profile_dictionary_after_conflict(
                    ctrl.settings_mut(),
                    &profile_id,
                    expected,
                    persisted_source,
                );
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod dictionary_compare_and_set_tests {
    use super::*;

    #[test]
    fn dictionary_cas_matches_the_stores_trailing_newline_normalization() {
        assert!(ensure_expected_dictionary_source("OpenRouter", Some("OpenRouter\n\n")).is_ok());
        assert!(
            ensure_expected_dictionary_source("merge = merch", Some("merge = merch\r\n")).is_ok()
        );
        assert!(ensure_expected_dictionary_source("", Some("\r\n")).is_ok());
        assert!(ensure_expected_dictionary_source("OpenRouter ", Some("OpenRouter")).is_err());
        assert!(
            ensure_expected_dictionary_source("OpenRouter\nmerge", Some("OpenRouter merge"))
                .is_err()
        );
    }

    fn profile_settings() -> Settings {
        Settings {
            hotkey_profiles: vec![HotkeyProfile {
                id: "english".to_string(),
                name: "English".to_string(),
                shortcut: "Option+Space".to_string(),
                language: Language::English,
            }],
            ..Settings::default()
        }
    }

    #[test]
    fn stale_global_source_is_rejected_without_mutation() {
        let mut settings = Settings {
            initial_prompt: "remote".to_string(),
            ..Settings::default()
        };
        let before = settings.clone();

        let error = set_initial_prompt_in(&mut settings, "local", Some("old")).unwrap_err();

        assert!(error.starts_with("Dictionary changed elsewhere:"));
        assert_eq!(settings.initial_prompt, before.initial_prompt);
        assert_eq!(settings.profile_glossaries, before.profile_glossaries);
    }

    #[test]
    fn stale_profile_source_is_rejected_without_mutation() {
        let mut settings = profile_settings();
        settings
            .profile_glossaries
            .insert("english".to_string(), "remote".to_string());
        let before = settings.clone();

        let error =
            set_profile_glossary_in(&mut settings, "english", "local".to_string(), Some("old"))
                .unwrap_err();

        assert!(error.starts_with("Dictionary changed elsewhere:"));
        assert_eq!(settings.initial_prompt, before.initial_prompt);
        assert_eq!(settings.profile_glossaries, before.profile_glossaries);
    }

    #[test]
    fn matching_expected_sources_write_both_dictionary_scopes() {
        let mut settings = profile_settings();
        settings.initial_prompt = "global old".to_string();
        settings
            .profile_glossaries
            .insert("english".to_string(), "profile old".to_string());

        set_initial_prompt_in(&mut settings, "global new", Some("global old")).unwrap();
        set_profile_glossary_in(
            &mut settings,
            "english",
            "profile new".to_string(),
            Some("profile old"),
        )
        .unwrap();

        assert_eq!(settings.initial_prompt, "global new");
        assert_eq!(settings.profile_glossaries["english"], "profile new");
    }

    #[test]
    fn omitted_expected_source_preserves_legacy_writes() {
        let mut settings = profile_settings();
        settings.initial_prompt = "global old".to_string();
        settings
            .profile_glossaries
            .insert("english".to_string(), "profile old".to_string());

        set_initial_prompt_in(&mut settings, "global new", None).unwrap();
        set_profile_glossary_in(&mut settings, "english", "profile new".to_string(), None).unwrap();

        assert_eq!(settings.initial_prompt, "global new");
        assert_eq!(settings.profile_glossaries["english"], "profile new");
    }

    #[test]
    fn unrelated_scope_change_does_not_block_compare_and_set() {
        let mut settings = profile_settings();
        settings.initial_prompt = "global changed elsewhere".to_string();
        settings
            .profile_glossaries
            .insert("english".to_string(), "profile old".to_string());

        set_profile_glossary_in(
            &mut settings,
            "english",
            "profile new".to_string(),
            Some("profile old"),
        )
        .unwrap();

        assert_eq!(settings.initial_prompt, "global changed elsewhere");
        assert_eq!(settings.profile_glossaries["english"], "profile new");
    }

    #[test]
    fn global_conflict_refreshes_stale_controller_field_without_touching_profiles() {
        let mut controller_settings = profile_settings();
        controller_settings.initial_prompt = "editor baseline".to_string();
        controller_settings
            .profile_glossaries
            .insert("english".to_string(), "unchanged profile".to_string());

        reconcile_global_dictionary_after_conflict(
            &mut controller_settings,
            "editor baseline",
            "fresh persisted source",
        );

        assert_eq!(controller_settings.initial_prompt, "fresh persisted source");
        assert_eq!(
            controller_settings.profile_glossaries["english"],
            "unchanged profile"
        );
    }

    #[test]
    fn global_conflict_does_not_clobber_a_newer_controller_field() {
        let mut controller_settings = profile_settings();
        controller_settings.initial_prompt = "newer controller value".to_string();

        reconcile_global_dictionary_after_conflict(
            &mut controller_settings,
            "editor baseline",
            "fresh persisted source",
        );

        assert_eq!(controller_settings.initial_prompt, "newer controller value");
    }

    #[test]
    fn profile_conflict_preserves_persisted_presence_and_rejects_newer_controller_value() {
        let mut stale_controller = profile_settings();
        stale_controller
            .profile_glossaries
            .insert("english".to_string(), "editor baseline".to_string());
        let missing = DictionarySourceSnapshot {
            present: false,
            source: String::new(),
        };
        reconcile_profile_dictionary_after_conflict(
            &mut stale_controller,
            "english",
            "editor baseline",
            &missing,
        );
        assert!(!stale_controller.profile_glossaries.contains_key("english"));

        stale_controller
            .profile_glossaries
            .insert("english".to_string(), "editor baseline".to_string());
        let explicitly_empty = DictionarySourceSnapshot {
            present: true,
            source: String::new(),
        };
        reconcile_profile_dictionary_after_conflict(
            &mut stale_controller,
            "english",
            "editor baseline",
            &explicitly_empty,
        );
        assert_eq!(
            stale_controller.profile_glossaries.get("english"),
            Some(&String::new())
        );

        stale_controller
            .profile_glossaries
            .insert("english".to_string(), "newer controller value".to_string());
        reconcile_profile_dictionary_after_conflict(
            &mut stale_controller,
            "english",
            "editor baseline",
            &DictionarySourceSnapshot {
                present: true,
                source: "fresh persisted source".to_string(),
            },
        );
        assert_eq!(
            stale_controller.profile_glossaries["english"],
            "newer controller value"
        );
    }
}

fn apply_reviewed_training_candidates(
    settings: &mut Settings,
    heard: &str,
    corrected: &str,
    profile_id: &str,
    accepted: &[GlossarySuggestion],
) -> Result<(), String> {
    ensure_known_profile(settings, profile_id)?;
    let effective = Glossary::parse(&settings.effective_glossary_source(Some(profile_id)));
    let allowed = suggest_glossary_candidates(heard, corrected, &effective);
    let mut matched = vec![false; allowed.len()];
    for candidate in accepted {
        validate_edited_training_candidate(candidate)?;
        let canonical_key = training_term_key(&candidate.canonical);
        let conflicts_with_existing_alias = effective.entries().iter().any(|entry| {
            entry
                .aliases
                .iter()
                .any(|alias| training_term_key(alias) == canonical_key)
        });
        let conflicts_with_reviewed_alias = accepted.iter().any(|other| {
            other.kind == GlossarySuggestionKind::Alias
                && training_term_key(&other.observed) == canonical_key
        });
        if conflicts_with_existing_alias || conflicts_with_reviewed_alias {
            return Err(
                "Edited preferred spelling conflicts with a personal dictionary alias".to_string(),
            );
        }
        let Some((index, _)) = allowed.iter().enumerate().find(|(index, original)| {
            !matched[*index]
                && original.observed == candidate.observed
                && original.context == candidate.context
                && match (original.kind, candidate.kind) {
                    (GlossarySuggestionKind::Alias, GlossarySuggestionKind::Alias)
                    | (GlossarySuggestionKind::Alias, GlossarySuggestionKind::HintOnly)
                    | (GlossarySuggestionKind::HintOnly, GlossarySuggestionKind::HintOnly) => true,
                    (GlossarySuggestionKind::HintOnly, GlossarySuggestionKind::Alias) => false,
                }
        }) else {
            return Err(
                "Dictionary suggestions changed; review the corrected transcript again".to_string(),
            );
        };
        matched[index] = true;
    }

    let source = settings
        .profile_glossaries
        .entry(profile_id.to_string())
        .or_default();
    let mut scoped = Glossary::parse(source);
    for candidate in accepted {
        match candidate.kind {
            GlossarySuggestionKind::Alias => {
                scoped.upsert(
                    candidate.canonical.clone(),
                    vec![candidate.observed.clone()],
                );
            }
            GlossarySuggestionKind::HintOnly => {
                scoped.upsert(candidate.canonical.clone(), Vec::new());
            }
        }
    }
    *source = scoped.render();
    Ok(())
}

fn training_term_key(value: &str) -> String {
    value.chars().flat_map(char::to_lowercase).collect()
}

fn validate_edited_training_candidate(candidate: &GlossarySuggestion) -> Result<(), String> {
    let canonical = candidate.canonical.trim();
    if canonical.is_empty()
        || canonical != candidate.canonical
        || canonical.len() > 96
        || canonical.split_whitespace().count() > 4
        || canonical
            .chars()
            .any(|character| matches!(character, '\n' | '\r' | ',' | '=' | '|'))
    {
        return Err(
            "Edited dictionary terms must be 1-4 words without glossary delimiters".to_string(),
        );
    }
    Ok(())
}

#[tauri::command]
pub async fn suggest_training_glossary(
    heard: String,
    corrected: String,
    profile_id: String,
) -> Result<Vec<GlossarySuggestion>, String> {
    let settings = sagascript_core::settings::store::load();
    ensure_known_profile(&settings, &profile_id)?;
    let glossary = Glossary::parse(&settings.effective_glossary_source(Some(&profile_id)));
    Ok(suggest_glossary_candidates(&heard, &corrected, &glossary))
}

#[tauri::command]
pub async fn apply_training_glossary(
    controller: State<'_, SharedController>,
    heard: String,
    corrected: String,
    profile_id: String,
    accepted: Vec<GlossarySuggestion>,
) -> Result<(), String> {
    if accepted.is_empty() {
        return Err("Select at least one dictionary suggestion".to_string());
    }

    let accepted_count = accepted.len();
    let persisted = sagascript_core::settings::store::try_update(|settings| {
        apply_reviewed_training_candidates(settings, &heard, &corrected, &profile_id, &accepted)
    })?;

    controller.lock().unwrap().settings_mut().profile_glossaries = persisted.profile_glossaries;
    info!(accepted_count, profile_id = %profile_id, "Applied reviewed training glossary suggestions");
    Ok(())
}

#[cfg(test)]
mod training_glossary_tests {
    use super::*;

    #[test]
    fn reviewed_candidates_are_saved_only_to_the_selected_profile() {
        let mut settings = Settings::default();
        let heard = "Jag heter Magnus Jille.";
        let corrected = "Jag heter Magnus Gille.";
        let candidates = suggest_glossary_candidates(heard, corrected, &Glossary::default());

        apply_reviewed_training_candidates(&mut settings, heard, corrected, "default", &candidates)
            .unwrap();

        assert_eq!(settings.initial_prompt, "");
        assert_eq!(
            settings
                .profile_glossaries
                .get("default")
                .map(String::as_str),
            Some("Gille = Jille")
        );
    }

    #[test]
    fn fabricated_candidate_fails_without_mutating_settings() {
        let mut settings = Settings::default();
        let fabricated = GlossarySuggestion {
            observed: "anything".to_string(),
            canonical: "dangerous".to_string(),
            kind: GlossarySuggestionKind::Alias,
            context: "Magnus Jille".to_string(),
        };

        let error = apply_reviewed_training_candidates(
            &mut settings,
            "Magnus Jille",
            "Magnus Gille",
            "default",
            &[fabricated],
        )
        .unwrap_err();

        assert!(error.contains("review"));
        assert!(settings.profile_glossaries.is_empty());
    }

    #[test]
    fn reviewed_alias_can_be_edited_or_converted_to_a_hint() {
        let heard = "Jag använder Love a ball idag.";
        let corrected = "Jag använder Lovable idag.";
        let original = suggest_glossary_candidates(heard, corrected, &Glossary::default())
            .into_iter()
            .next()
            .unwrap();

        let mut edited = original.clone();
        edited.canonical = "Lovable.dev".to_string();
        let mut settings = Settings::default();
        apply_reviewed_training_candidates(&mut settings, heard, corrected, "default", &[edited])
            .unwrap();
        assert_eq!(
            settings
                .profile_glossaries
                .get("default")
                .map(String::as_str),
            Some("Lovable.dev = Love a ball")
        );

        let mut as_hint = original;
        as_hint.kind = GlossarySuggestionKind::HintOnly;
        let mut settings = Settings::default();
        apply_reviewed_training_candidates(&mut settings, heard, corrected, "default", &[as_hint])
            .unwrap();
        assert_eq!(
            settings
                .profile_glossaries
                .get("default")
                .map(String::as_str),
            Some("Lovable")
        );
    }

    #[test]
    fn auto_language_profile_fails_closed_without_mutation() {
        let mut settings = Settings {
            language: Language::Auto,
            ..Default::default()
        };
        let heard = "Magnus Jille";
        let corrected = "Magnus Gille";
        let candidates = suggest_glossary_candidates(heard, corrected, &Glossary::default());

        let error = apply_reviewed_training_candidates(
            &mut settings,
            heard,
            corrected,
            "default",
            &candidates,
        )
        .unwrap_err();

        assert!(error.contains("explicit language"));
        assert!(settings.profile_glossaries.is_empty());
    }

    #[test]
    fn edited_canonical_cannot_shadow_an_existing_profile_alias() {
        let mut settings = Settings::default();
        settings
            .profile_glossaries
            .insert("default".into(), "Existing = dangerous".into());
        let original = settings.profile_glossaries.clone();
        let heard = "Magnus Jille";
        let corrected = "Magnus Gille";
        let mut candidate = suggest_glossary_candidates(
            heard,
            corrected,
            &Glossary::parse(&settings.effective_glossary_source(Some("default"))),
        )
        .into_iter()
        .next()
        .unwrap();
        candidate.canonical = "Dangerous".to_string();

        let error = apply_reviewed_training_candidates(
            &mut settings,
            heard,
            corrected,
            "default",
            &[candidate],
        )
        .unwrap_err();

        assert!(error.contains("conflicts"));
        assert_eq!(settings.profile_glossaries, original);
    }

    #[test]
    fn inactive_global_alias_does_not_block_reviewed_profile_term() {
        let mut settings = Settings {
            initial_prompt: "Existing = dangerous".to_string(),
            ..Default::default()
        };
        let heard = "Magnus Jille";
        let corrected = "Magnus Gille";
        let mut candidate = suggest_glossary_candidates(
            heard,
            corrected,
            &Glossary::parse(&settings.effective_glossary_source(Some("default"))),
        )
        .into_iter()
        .next()
        .unwrap();
        candidate.canonical = "Dangerous".to_string();
        apply_reviewed_training_candidates(
            &mut settings,
            heard,
            corrected,
            "default",
            &[candidate],
        )
        .unwrap();
        assert_eq!(settings.initial_prompt, "Existing = dangerous");
        assert_eq!(settings.profile_glossaries["default"], "Dangerous = Jille");
    }
}

#[tauri::command]
pub async fn set_beam_size(
    controller: State<'_, SharedController>,
    beam_size: u32,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.beam_size = beam_size;
    })?;
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().beam_size = persisted.beam_size;
    info!("Beam size: {beam_size}");
    Ok(())
}

#[tauri::command]
pub async fn set_temperature_fallback(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.temperature_fallback = enabled;
    })?;
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().temperature_fallback = persisted.temperature_fallback;
    info!("Temperature fallback: {enabled}");
    Ok(())
}

#[tauri::command]
pub async fn set_vad_enabled(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.vad_enabled = enabled;
    })?;
    {
        let mut ctrl = controller.lock().unwrap();
        ctrl.settings_mut().vad_enabled = persisted.vad_enabled;
    }
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

// -- File transcription --

#[tauri::command]
pub async fn transcribe_file(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    whisper: State<'_, SharedWhisper>,
    file_path: String,
    prompt: Option<String>,
    diarize: Option<bool>,
    profile_id: Option<String>,
) -> Result<String, String> {
    use tauri::Emitter;

    let FileTranscriptionContext {
        language,
        model: effective_model,
        glossary,
        options: mut opts,
    } = {
        let ctrl = controller.lock().unwrap();
        file_transcription_context(ctrl.settings(), profile_id.as_deref(), prompt.as_deref())?
    };

    let path = std::path::PathBuf::from(&file_path);

    // Decode audio file
    let audio = tokio::task::spawn_blocking(move || decoder::decode_audio_file(&path))
        .await
        .map_err(|e| format!("Decode task failed: {e}"))?
        .map_err(|e| e.to_string())?;

    if audio.is_empty() {
        return Err("No audio decoded from file".to_string());
    }

    // File transcription (beam search / diarization) is far slower than live
    // dictation, so scale the timeout by the decoded duration rather than using
    // the short live-dictation timeout (which beam search could otherwise hit).
    let file_timeout =
        Duration::from_secs(((audio.len() / 16_000) as u64 * 6).max(TRANSCRIPTION_TIMEOUT_SECS));

    // Suppress unused-variable warning on `diarize` when the diarization feature is off
    #[cfg(not(feature = "diarization"))]
    let _ = &diarize;

    #[cfg(feature = "diarization")]
    let context_profile = ContextProfile::for_diarization(diarize.unwrap_or(false));
    #[cfg(not(feature = "diarization"))]
    let context_profile = ContextProfile::FlashAttention;

    // Show model loading status if the exact model/profile runtime is not warm.
    if whisper.needs_reload_with_profile(effective_model, context_profile) {
        let _ = app.emit(crate::events::event::STATE_CHANGED, "loading_model");
    }

    // Diarization path — runs both diarization and timestamped transcription in parallel,
    // then merges and consolidates speaker-attributed segments.
    #[cfg(feature = "diarization")]
    if diarize.unwrap_or(false) {
        use sagascript_core::diarization::{
            diarize as run_diarize,
            merge::{consolidate, merge_with_transcript},
            model::{download_model as download_diarization_model, DiarizationModel},
            DiarizeConfig, TimestampedSegment,
        };

        // Checking diarization in the file-transcription UI is an explicit
        // action: verify existing app-managed artifacts and repair only exact
        // integrity mismatches before native ONNX parsing. This never runs as
        // a silent startup download.
        for diarization_model in DiarizationModel::ALL {
            if let Err(error) = download_diarization_model(*diarization_model, |_, _| {}).await {
                let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");
                return Err(error.to_string());
            }
        }

        let _ = app.emit(crate::events::event::STATE_CHANGED, "transcribing");

        let whisper_ref = whisper.inner().clone();
        // Fall back to the saved initial_prompt when the file-dialog prompt is
        // empty (matches the standard file path).
        let prompt_ref = glossary.decoder_prompt();
        let audio_for_diarize = audio.clone();
        let audio_for_transcribe = audio.clone();

        // Run diarization
        let mut diarize_fut = tokio::task::spawn_blocking(move || {
            run_diarize(&audio_for_diarize, &DiarizeConfig::default())
        });

        // Run word-level timestamped transcription when DTW is available.
        // Segment-level timestamps can span multiple speaker turns and would
        // cause maximum-overlap merging to collapse the GUI output to one label.
        let mut transcribe_fut = tokio::task::spawn_blocking(move || {
            whisper_ref.with_model(effective_model, ContextProfile::TokenAlignment, |backend| {
                backend.transcribe_sync_for_diarization(
                    &audio_for_transcribe,
                    language,
                    prompt_ref.as_deref(),
                )
            })
        });

        let timeout = file_timeout;
        // Join over BORROWED handles so the transcription handle stays available
        // for the post-abort grace await on the timeout path below.
        let (speaker_segments, raw_segments) = match tokio::time::timeout(timeout, async {
            tokio::join!(&mut diarize_fut, &mut transcribe_fut)
        })
        .await
        {
            Ok((Ok(Ok(spk)), Ok(Ok(trx)))) => (spk, trx),
            Ok((Ok(Err(e)), _)) | Ok((_, Ok(Err(e)))) => {
                let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");
                return Err(e.to_string());
            }
            Ok((Err(e), _)) | Ok((_, Err(e))) => {
                let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");
                return Err(format!("Task join error: {e}"));
            }
            Err(_) => {
                // Real abort: releases the whisper warm-state lock so the next
                // transcription isn't wedged. (The diarization half runs its
                // own compute and simply detaches when its handle is dropped.)
                warn!(
                    "Diarized transcription timed out after {}s — requesting abort",
                    timeout.as_secs()
                );
                whisper.request_abort();
                // Brief grace for the aborted inference to unwind; log which
                // outcome occurred so a genuine hang is visible.
                match tokio::time::timeout(
                    Duration::from_secs(ABORT_GRACE_SECS),
                    &mut transcribe_fut,
                )
                .await
                {
                    Ok(_) => info!("Aborted transcription task exited — warm-state lock released"),
                    Err(_) => error!(
                        "Transcription task still running {ABORT_GRACE_SECS}s after abort — \
                         warm state may stay locked until it unwinds; further transcriptions \
                         will report ModelBusy rather than block forever"
                    ),
                }
                let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");
                return Err(format!(
                    "Transcription timed out after {}s (inference aborted)",
                    timeout.as_secs()
                ));
            }
        };

        let transcript: Vec<TimestampedSegment> = raw_segments
            .into_iter()
            .map(|(start, end, text)| TimestampedSegment { start, end, text })
            .collect();

        let diarized = merge_with_transcript(&speaker_segments, &transcript);
        let mut consolidated = consolidate(&diarized);
        for segment in &mut consolidated {
            segment.text = sagascript_core::transcription::normalize_nonspeech_markers(
                &segment.text,
                language,
            );
        }

        let text = consolidated
            .iter()
            .map(|s| format!("[{}] {}", s.speaker, s.text.trim()))
            .collect::<Vec<_>>()
            .join("\n");
        let text = apply_glossary(text, &glossary);

        info!("Diarized file transcription complete: {} chars", text.len());

        let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");

        // Auto-paste if enabled
        let should_paste = {
            let c = controller.lock().unwrap();
            c.settings().auto_paste
        };
        if should_paste {
            let text_for_paste = text.clone();
            if let Err(e) = app.run_on_main_thread(move || {
                let paste_svc = crate::paste::PasteService::new();
                if let Err(e) = paste_svc.paste(&text_for_paste) {
                    error!("Auto-paste failed: {e}");
                }
            }) {
                error!("Failed to dispatch paste to main thread: {e}");
            }
        }

        return Ok(text);
    }

    // Standard (non-diarize) transcription path. File transcription defaults to
    // beam search (quality over latency).
    opts.parallel_chunks =
        recommended_parallel_chunks(audio.len(), effective_model, opts.beam_size);
    let _ = app.emit(crate::events::event::STATE_CHANGED, "transcribing");
    let whisper_ref = whisper.inner().clone();
    let app_progress = app.clone();
    // Borrowed handle (`&mut fut`) so the timeout path can await the task's
    // actual exit after requesting an abort — mirrors the live dictation path.
    let mut fut = tokio::task::spawn_blocking(move || {
        whisper_ref.with_model(effective_model, context_profile, |backend| {
            backend.transcribe_sync_with_options(&audio, language, &opts, move |pct| {
                let _ = app_progress.emit(crate::events::event::TRANSCRIPTION_PROGRESS, pct);
            })
        })
    });

    let timeout = file_timeout;
    let result = match tokio::time::timeout(timeout, &mut fut).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");
            return Err(format!("Transcription task failed: {e}"));
        }
        Err(_) => {
            warn!(
                "File transcription timed out after {}s — requesting abort",
                timeout.as_secs()
            );
            whisper.request_abort();
            // Brief grace for the aborted inference to unwind; log which outcome
            // occurred so a genuine hang is visible.
            match tokio::time::timeout(Duration::from_secs(ABORT_GRACE_SECS), &mut fut).await {
                Ok(_) => info!("Aborted transcription task exited — warm-state lock released"),
                Err(_) => error!(
                    "Transcription task still running {ABORT_GRACE_SECS}s after abort — \
                     warm state may stay locked until it unwinds; further transcriptions \
                     will report ModelBusy rather than block forever"
                ),
            }
            let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");
            return Err(format!(
                "Transcription timed out after {}s (inference aborted)",
                timeout.as_secs()
            ));
        }
    };

    let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");

    match result {
        Ok(text) => {
            let text = apply_glossary(text, &glossary);
            info!("File transcription complete: {} chars", text.len());

            // Auto-paste if enabled
            let should_paste = {
                let c = controller.lock().unwrap();
                c.settings().auto_paste
            };

            if should_paste {
                let text_for_paste = text.clone();
                if let Err(e) = app.run_on_main_thread(move || {
                    let paste_svc = crate::paste::PasteService::new();
                    if let Err(e) = paste_svc.paste(&text_for_paste) {
                        error!("Auto-paste failed: {e}");
                    }
                }) {
                    error!("Failed to dispatch paste to main thread: {e}");
                }
            }

            Ok(text)
        }
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
pub async fn get_supported_formats() -> Result<Vec<String>, String> {
    Ok(decoder::SUPPORTED_EXTENSIONS
        .iter()
        .map(|s| s.to_string())
        .collect())
}

// -- Build info --

#[tauri::command]
pub async fn get_build_info() -> Result<BuildInfo, String> {
    Ok(BuildInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        git_hash: env!("GIT_HASH").to_string(),
        build_date: env!("BUILD_DATE").to_string(),
    })
}

#[derive(serde::Serialize)]
pub struct BuildInfo {
    version: String,
    git_hash: String,
    build_date: String,
}

#[derive(serde::Serialize)]
pub struct ModelInfo {
    id: String,
    display_name: String,
    description: String,
    size_mb: u32,
    downloaded: bool,
    active: bool,
}

// -- Permission / platform queries (for onboarding) --

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
        crate::platform::macos::request_accessibility_permission()?;
    }
    Ok(())
}

#[tauri::command]
pub async fn open_accessibility_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
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

    fn authorization_status_label(status: isize, denied_recheck_granted: bool) -> &'static str {
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
    enum AccessRequestDecision {
        QuerySystem,
        Complete(bool),
    }

    fn access_request_decision(status: isize) -> AccessRequestDecision {
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
            if let Some(cb) = callback.lock().unwrap().take() {
                cb(granted);
            }
        });
        // The block must be copied to the heap for async use by the framework
        let completion = completion.copy();

        unsafe {
            let _: () = msg_send![cls, requestAccessForMediaType: media_type completionHandler: &*completion];
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{access_request_decision, authorization_status_label, AccessRequestDecision};

        #[test]
        fn maps_avfoundation_authorization_constants_correctly() {
            assert_eq!(authorization_status_label(0, false), "not_determined");
            assert_eq!(authorization_status_label(1, false), "restricted");
            assert_eq!(authorization_status_label(2, false), "denied");
            assert_eq!(authorization_status_label(3, false), "authorized");
        }

        #[test]
        fn denied_cache_can_be_refreshed_without_remapping_authorized() {
            assert_eq!(authorization_status_label(2, true), "authorized");
            assert_eq!(authorization_status_label(3, false), "authorized");
        }

        #[test]
        fn request_decision_live_queries_undetermined_and_cached_denied() {
            assert_eq!(
                access_request_decision(0),
                AccessRequestDecision::QuerySystem
            );
            assert_eq!(
                access_request_decision(2),
                AccessRequestDecision::QuerySystem
            );
        }

        #[test]
        fn request_decision_completes_known_restricted_and_authorized_states() {
            assert_eq!(
                access_request_decision(1),
                AccessRequestDecision::Complete(false)
            );
            assert_eq!(
                access_request_decision(3),
                AccessRequestDecision::Complete(true)
            );
        }
    }
}

#[tauri::command]
pub async fn get_platform() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        Ok("macos".to_string())
    }
    #[cfg(target_os = "windows")]
    {
        Ok("windows".to_string())
    }
    #[cfg(target_os = "linux")]
    {
        Ok("linux".to_string())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Ok("unknown".to_string())
    }
}

#[derive(serde::Serialize)]
pub struct LoadedModelInfo {
    effective_model: String,
    effective_model_id: String,
    loaded_model: Option<String>,
    is_loaded: bool,
    is_downloaded: bool,
}
