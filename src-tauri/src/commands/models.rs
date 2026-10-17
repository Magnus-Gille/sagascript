//! Speech models: what each picker offers, what is downloaded or loaded, and
//! the downloads themselves.

use sagascript_core::settings::{FileModel, FileModelPreference, Language, Settings, WhisperModel};
use sagascript_core::transcription::model;
use sagascript_core::transcription::pianissimo_model;
use tauri::State;

use super::{lock_controller, SharedController, SharedWhisper};
use crate::events::EmitEvent;

/// Returns the display name of the currently loaded (or effective) model
#[tauri::command]
pub async fn get_loaded_model(
    controller: State<'_, SharedController>,
    whisper: State<'_, SharedWhisper>,
) -> Result<LoadedModelInfo, String> {
    let ctrl = lock_controller(&controller);
    let chosen = ctrl
        .settings()
        .dictation_model_for_profile(&ctrl.settings().default_profile().id)?;
    if chosen == FileModel::PianissimoOriginal {
        return Ok(LoadedModelInfo {
            effective_model: "Pianissimo".into(),
            effective_model_id: "pianissimo-sv".into(),
            loaded_model: None,
            is_loaded: false,
            is_downloaded: pianissimo_model::is_downloaded(),
        });
    }
    let FileModel::Whisper(effective) = chosen else {
        unreachable!()
    };
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

#[tauri::command]
pub async fn is_model_downloaded(whisper_model: WhisperModel) -> Result<bool, String> {
    Ok(model::is_model_downloaded(whisper_model))
}

#[tauri::command]
pub async fn get_model_info(
    controller: State<'_, SharedController>,
) -> Result<Vec<ModelInfo>, String> {
    let ctrl = lock_controller(&controller);
    let profile = ctrl.settings().default_profile();
    let effective = ctrl.settings().dictation_model_for_profile(&profile.id)?;
    Ok(lineup_model_infos(
        profile.language,
        pianissimo_supported(),
        Some(effective),
        Some(effective),
    ))
}

fn pianissimo_supported() -> bool {
    sagascript_core::transcription::pianissimo_backend::runtime_supported_on_this_os()
}

/// Wire description of one engine choice (Whisper model or Pianissimo).
fn model_info_for(choice: FileModel, active: bool) -> ModelInfo {
    match choice {
        FileModel::Whisper(model) => ModelInfo {
            id: serde_json::to_value(model)
                .and_then(serde_json::from_value::<String>)
                .expect("Whisper model has a stable ID"),
            display_name: model.display_name().to_string(),
            description: model.description().to_string(),
            size_mb: model.size_mb(),
            downloaded: model::is_model_downloaded(model),
            active,
        },
        FileModel::PianissimoOriginal => ModelInfo {
            id: "pianissimo-sv".into(),
            display_name: "Pianissimo".into(),
            description: if cfg!(windows) {
                "Swedish file transcription · ONNX Runtime on the CPU · Windows on ARM (Snapdragon)"
                    .into()
            } else {
                "Swedish file transcription · Core ML on the Neural Engine · macOS 14+ on Apple Silicon".into()
            },
            size_mb: pianissimo_model_size_mb(),
            downloaded: pianissimo_model::is_downloaded(),
            active,
        },
    }
}

/// The app's model choices for `language` (after the "Recommended" entry),
/// with the platform gate injected. `include` keeps a currently selected
/// model (for example a retired KB-Whisper Base) visible in the list.
fn lineup_model_infos(
    language: Language,
    pianissimo_supported: bool,
    include: Option<FileModel>,
    active: Option<FileModel>,
) -> Vec<ModelInfo> {
    FileModel::app_lineup(language, pianissimo_supported, include)
        .into_iter()
        .map(|choice| model_info_for(choice, active == Some(choice)))
        .collect()
}

/// Model choices for the pickers. `include` is the currently selected model
/// ID (a stable persisted ID); it stays listed even when the app hides it.
#[tauri::command]
pub async fn get_file_model_options(
    language: Language,
    include: Option<String>,
) -> Result<Vec<ModelInfo>, String> {
    let include = include
        .as_deref()
        .and_then(|id| match FileModelPreference::parse_id(id) {
            Ok(FileModelPreference::Whisper(model)) => Some(FileModel::Whisper(model)),
            Ok(FileModelPreference::PianissimoOriginal) => Some(FileModel::PianissimoOriginal),
            _ => None,
        });
    Ok(lineup_model_infos(
        language,
        pianissimo_supported(),
        include,
        None,
    ))
}

/// Download size of the Pianissimo model, in MB, for model listings.
fn pianissimo_model_size_mb() -> u32 {
    (pianissimo_model::download_size_bytes() / 1_048_576) as u32
}

#[tauri::command]
pub async fn get_profile_model_info(
    controller: State<'_, SharedController>,
    profile_id: String,
) -> Result<ModelInfo, String> {
    let ctrl = lock_controller(&controller);
    let choice = ctrl.settings().dictation_model_for_profile(&profile_id)?;
    Ok(model_info_for(choice, true))
}

#[tauri::command]
pub async fn download_pianissimo_model(app: tauri::AppHandle) -> Result<(), String> {
    if !sagascript_core::transcription::pianissimo_backend::runtime_supported_on_this_os() {
        return Err(sagascript_core::transcription::pianissimo_backend::UNSUPPORTED_MESSAGE.into());
    }
    let progress_app = app.clone();
    pianissimo_model::download(move |downloaded, total| {
        let _ = progress_app.emit_event(
            crate::events::event::MODEL_DOWNLOAD_PROGRESS,
            crate::events::ModelDownloadProgress::new("pianissimo-sv".into(), downloaded, total),
        );
    })
    .await
    .map_err(|error| error.to_string())?;
    let _ = app.emit_event(crate::events::event::MODEL_READY, ());
    Ok(())
}

/// Return the effective speech engine for one profile language. This keeps the
/// normal Dictate surface model-name-free while still giving it enough state to
/// offer a direct download when an upgraded or newly added profile is not ready.
#[tauri::command]
pub async fn get_effective_model_info(
    _controller: State<'_, SharedController>,
    language: Language,
) -> Result<ModelInfo, String> {
    Ok(model_info_for(
        FileModel::recommended_for(language, pianissimo_supported()),
        true,
    ))
}

/// The model an `Auto` file-transcription preference will actually use for
/// `language`: the recommended model when downloaded, else the best already
/// downloaded compatible model (same resolution the transcribe command uses).
#[tauri::command]
pub async fn get_auto_file_model_info(language: Language) -> Result<ModelInfo, String> {
    let auto = Settings {
        file_transcription_model: FileModelPreference::Auto,
        ..Default::default()
    };
    Ok(model_info_for(
        auto.effective_file_model_for(language)?,
        true,
    ))
}

#[tauri::command]
pub async fn get_dictation_model_info(
    controller: State<'_, SharedController>,
    language: Language,
) -> Result<ModelInfo, String> {
    let profile = lock_controller(&controller).settings().default_profile();
    if profile.language == language {
        return get_profile_model_info(controller, profile.id).await;
    }
    get_effective_model_info(controller, language).await
}

// -- Model download --

#[tauri::command]
pub async fn download_model(
    app: tauri::AppHandle,
    whisper_model: WhisperModel,
) -> Result<(), String> {
    let app_handle = app.clone();
    model::download_model(whisper_model, move |downloaded, total| {
        let _ = app_handle.emit_event(
            crate::events::event::MODEL_DOWNLOAD_PROGRESS,
            // The Debug name is a legacy quirk the frontend ignores (#347).
            crate::events::ModelDownloadProgress::new(
                format!("{whisper_model:?}"),
                downloaded,
                total,
            ),
        );
    })
    .await
    .map_err(|e| e.to_string())?;

    let _ = app.emit_event(crate::events::event::MODEL_READY, ());
    Ok(())
}

#[derive(serde::Serialize)]
pub struct EngineStatus {
    installed: bool,
    supported: bool,
    host_path: Option<String>,
    host_version: Option<String>,
    host_git_sha: Option<String>,
    warm: bool,
}

/// Read-only Pianissimo engine status for Settings. Only starts the host (for
/// `hello`, no model load) when the model is installed, Pianissimo is selected
/// and no identity is cached yet.
#[tauri::command]
pub async fn engine_status(
    controller: State<'_, SharedController>,
) -> Result<EngineStatus, String> {
    use sagascript_core::settings::FileModelPreference;
    use sagascript_core::transcription::pianissimo_backend as backend;
    let supported = backend::runtime_supported_on_this_os();
    let installed = pianissimo_model::is_downloaded();
    let host = backend::resolve_host();
    let selected = {
        let ctrl = lock_controller(&controller);
        let settings = ctrl.settings();
        settings.file_transcription_model == FileModelPreference::PianissimoOriginal
            || settings.profile_models.values().any(|m| *m == FileModelPreference::PianissimoOriginal)
            // "Recommended for Swedish" resolves to Pianissimo where supported.
            || settings.resolved_hotkey_profiles().iter().any(|profile| {
                settings
                    .dictation_model_for_profile_gated(&profile.id, supported)
                    .is_ok_and(|choice| choice == FileModel::PianissimoOriginal)
            })
    };
    let mut identity = backend::cached_host_identity();
    if identity.is_none()
        && supported
        && installed
        && selected
        && host.as_ref().is_some_and(|p| p.is_file())
    {
        identity = tokio::task::spawn_blocking(backend::refresh_host_identity)
            .await
            .map_err(|e| format!("engine status task failed: {e}"))?;
    }
    let (host_version, host_git_sha) = match identity {
        Some((version, sha)) => (Some(version), sha),
        None => (None, None),
    };
    Ok(EngineStatus {
        installed,
        supported,
        host_path: host.map(|p| p.display().to_string()),
        host_version,
        host_git_sha,
        warm: backend::engine_is_warm(),
    })
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

#[derive(serde::Serialize)]
pub struct LoadedModelInfo {
    effective_model: String,
    effective_model_id: String,
    loaded_model: Option<String>,
    is_loaded: bool,
    is_downloaded: bool,
}

#[cfg(test)]
#[path = "models_tests.rs"]
mod tests;
