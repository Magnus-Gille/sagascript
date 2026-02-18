use std::sync::{Arc, Mutex};

use tauri::State;
use tracing::{error, info};

use crate::app_controller::{AppController, AppState};
use crate::settings::{
    HotkeyMode, Language, Settings, TranscriptionBackendType, WhisperModel,
};
use crate::transcription::{model, OpenAIBackend, WhisperBackend};

/// Shared app state type — uses std::sync::Mutex (not tokio) because
/// cpal::Stream is !Send and we need sync access from Tauri commands
pub type SharedController = Mutex<AppController>;

/// Shared whisper backend — separate from AppController to avoid holding
/// the controller lock during blocking transcription
pub type SharedWhisper = Arc<WhisperBackend>;

// -- State queries --

#[tauri::command]
pub async fn get_state(controller: State<'_, SharedController>) -> Result<AppState, String> {
    let ctrl = controller.lock().unwrap();
    Ok(ctrl.state())
}

#[tauri::command]
pub async fn get_settings(controller: State<'_, SharedController>) -> Result<Settings, String> {
    let ctrl = controller.lock().unwrap();
    Ok(ctrl.settings().clone())
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

// -- Settings mutations --

#[tauri::command]
pub async fn update_settings(
    controller: State<'_, SharedController>,
    settings: Settings,
) -> Result<(), String> {
    let mut ctrl = controller.lock().unwrap();
    ctrl.update_settings(settings);
    info!("Settings updated");
    Ok(())
}

#[tauri::command]
pub async fn set_language(
    controller: State<'_, SharedController>,
    language: Language,
) -> Result<(), String> {
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().language = language;
    info!("Language set to {:?}", language);
    Ok(())
}

#[tauri::command]
pub async fn set_backend(
    controller: State<'_, SharedController>,
    backend: TranscriptionBackendType,
) -> Result<(), String> {
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().backend = backend;
    info!("Backend set to {:?}", backend);
    Ok(())
}

#[tauri::command]
pub async fn set_whisper_model(
    controller: State<'_, SharedController>,
    model: WhisperModel,
) -> Result<(), String> {
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().whisper_model = model;
    ctrl.settings_mut().auto_select_model = false;
    info!("Model set to {:?}", model);
    Ok(())
}

#[tauri::command]
pub async fn set_auto_select_model(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().auto_select_model = enabled;
    info!("Auto-select model: {enabled}");
    Ok(())
}

#[tauri::command]
pub async fn set_hotkey_mode(
    controller: State<'_, SharedController>,
    mode: HotkeyMode,
) -> Result<(), String> {
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().hotkey_mode = mode;
    info!("Hotkey mode set to {:?}", mode);
    Ok(())
}

// -- API Key --

#[tauri::command]
pub async fn save_api_key(
    controller: State<'_, SharedController>,
    key: String,
) -> Result<bool, String> {
    let ctrl = controller.lock().unwrap();
    Ok(ctrl.keyring().save_api_key(&key))
}

#[tauri::command]
pub async fn has_api_key(controller: State<'_, SharedController>) -> Result<bool, String> {
    let ctrl = controller.lock().unwrap();
    Ok(ctrl.keyring().has_api_key())
}

#[tauri::command]
pub async fn delete_api_key(controller: State<'_, SharedController>) -> Result<bool, String> {
    let ctrl = controller.lock().unwrap();
    Ok(ctrl.keyring().delete_api_key())
}

// -- Recording --

#[tauri::command]
pub async fn start_recording(controller: State<'_, SharedController>) -> Result<(), String> {
    let mut ctrl = controller.lock().unwrap();
    ctrl.start_recording().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn stop_and_transcribe(
    controller: State<'_, SharedController>,
    whisper: State<'_, SharedWhisper>,
) -> Result<String, String> {
    // Extract what we need with the lock held briefly
    let (audio, backend, language, keyring, effective_model) = {
        let mut ctrl = controller.lock().unwrap();
        let audio = ctrl.stop_recording();
        let backend = ctrl.backend();
        let language = ctrl.language();
        let keyring = ctrl.keyring().clone();
        let effective_model = ctrl.settings().effective_model();
        (audio, backend, language, keyring, effective_model)
    };
    // Lock is dropped here

    if audio.is_empty() {
        let mut ctrl = controller.lock().unwrap();
        ctrl.on_transcription_error("No audio captured");
        return Err("No audio captured".to_string());
    }

    // Transcribe without holding the controller lock
    let result = match backend {
        TranscriptionBackendType::Remote => {
            use crate::transcription::backend::TranscriptionBackend;
            let openai = OpenAIBackend::new(keyring);
            openai.transcribe(&audio, language).await
        }
        TranscriptionBackendType::Local => {
            // Ensure model is loaded
            whisper
                .ensure_model(effective_model)
                .map_err(|e| e.to_string())?;

            // Run blocking transcription on a separate thread
            let whisper = whisper.inner().clone();
            let audio = audio.clone();
            tokio::task::spawn_blocking(move || {
                whisper.transcribe_sync(&audio, language)
            })
            .await
            .map_err(|e| format!("Transcription task failed: {e}"))?
        }
    };

    match result {
        Ok(text) => {
            let mut ctrl = controller.lock().unwrap();
            ctrl.on_transcription_success(&text);
            if let Err(e) = ctrl.auto_paste(&text) {
                error!("Auto-paste failed: {e}");
            }
            Ok(text)
        }
        Err(e) => {
            let mut ctrl = controller.lock().unwrap();
            ctrl.on_transcription_error(&e.to_string());
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn cancel_recording(controller: State<'_, SharedController>) -> Result<(), String> {
    let mut ctrl = controller.lock().unwrap();
    ctrl.cancel_recording();
    Ok(())
}

// -- Model management --

#[tauri::command]
pub async fn is_model_downloaded(whisper_model: WhisperModel) -> Result<bool, String> {
    Ok(model::is_model_downloaded(whisper_model))
}

#[tauri::command]
pub async fn get_model_info() -> Result<Vec<ModelInfo>, String> {
    let all_models = [
        WhisperModel::TinyEn,
        WhisperModel::Tiny,
        WhisperModel::BaseEn,
        WhisperModel::Base,
        WhisperModel::KbWhisperTiny,
        WhisperModel::KbWhisperBase,
        WhisperModel::KbWhisperSmall,
    ];

    Ok(all_models
        .iter()
        .map(|m| ModelInfo {
            id: format!("{:?}", m),
            display_name: m.display_name().to_string(),
            description: m.description().to_string(),
            size_mb: m.size_mb(),
            downloaded: model::is_model_downloaded(*m),
            english_only: m.is_english_only(),
            swedish_optimized: m.is_swedish_optimized(),
        })
        .collect())
}

#[derive(serde::Serialize)]
pub struct ModelInfo {
    id: String,
    display_name: String,
    description: String,
    size_mb: u32,
    downloaded: bool,
    english_only: bool,
    swedish_optimized: bool,
}
