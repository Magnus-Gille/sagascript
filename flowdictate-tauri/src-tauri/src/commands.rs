use std::sync::{Arc, Mutex};

use tauri::State;
use tracing::{error, info};

use crate::app_controller::{AppController, AppState};
use crate::audio::decoder;
use crate::settings::{HotkeyMode, Language, Settings, WhisperModel};
use crate::transcription::{model, WhisperBackend};

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
        effective_model_id: format!("{:?}", effective),
        loaded_model: loaded.map(|m| m.display_name().to_string()),
        is_loaded: loaded == Some(effective),
        is_downloaded: model::is_model_downloaded(effective),
    })
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
    let (audio, language, effective_model) = {
        let mut ctrl = controller.lock().unwrap();
        let audio = ctrl.stop_recording();
        let language = ctrl.language();
        let effective_model = ctrl.settings().effective_model();
        (audio, language, effective_model)
    };

    if audio.is_empty() {
        let mut ctrl = controller.lock().unwrap();
        ctrl.on_transcription_error("No audio captured");
        return Err("No audio captured".to_string());
    }

    // Ensure model is loaded
    whisper
        .ensure_model(effective_model)
        .map_err(|e| e.to_string())?;

    // Run blocking transcription on a separate thread
    let whisper = whisper.inner().clone();
    let audio = audio.clone();
    let result = tokio::task::spawn_blocking(move || whisper.transcribe_sync(&audio, language))
        .await
        .map_err(|e| format!("Transcription task failed: {e}"))?;

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
            id: format!("{:?}", m),
            display_name: m.display_name().to_string(),
            description: m.description().to_string(),
            size_mb: m.size_mb(),
            downloaded: model::is_model_downloaded(*m),
            active: *m == effective,
        })
        .collect())
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

#[tauri::command]
pub async fn set_auto_paste(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().auto_paste = enabled;
    info!("Auto-paste: {enabled}");
    Ok(())
}

#[tauri::command]
pub async fn set_show_overlay(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().show_overlay = enabled;
    info!("Show overlay: {enabled}");
    Ok(())
}

// -- File transcription --

#[tauri::command]
pub async fn transcribe_file(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    whisper: State<'_, SharedWhisper>,
    file_path: String,
) -> Result<String, String> {
    use tauri::Emitter;

    let path = std::path::PathBuf::from(&file_path);

    // Decode audio file
    let audio = tokio::task::spawn_blocking(move || decoder::decode_audio_file(&path))
        .await
        .map_err(|e| format!("Decode task failed: {e}"))?
        .map_err(|e| e.to_string())?;

    if audio.is_empty() {
        return Err("No audio decoded from file".to_string());
    }

    // Get transcription settings
    let (language, effective_model) = {
        let ctrl = controller.lock().unwrap();
        (ctrl.language(), ctrl.settings().effective_model())
    };

    // Show model loading status if needed
    if whisper.needs_reload(effective_model) {
        let _ = app.emit(crate::events::event::STATE_CHANGED, "loading_model");
    }

    // Ensure model is loaded
    whisper
        .ensure_model(effective_model)
        .map_err(|e| e.to_string())?;

    let _ = app.emit(crate::events::event::STATE_CHANGED, "transcribing");

    // Run blocking transcription
    let whisper = whisper.inner().clone();
    let result = tokio::task::spawn_blocking(move || whisper.transcribe_sync(&audio, language))
        .await
        .map_err(|e| format!("Transcription task failed: {e}"))?;

    let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");

    match result {
        Ok(text) => {
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
        crate::platform::macos::request_accessibility_permission();
    }
    Ok(())
}

#[tauri::command]
pub async fn check_microphone_permission() -> Result<bool, String> {
    use cpal::traits::HostTrait;
    Ok(cpal::default_host().default_input_device().is_some())
}

#[tauri::command]
pub async fn request_microphone_permission() -> Result<bool, String> {
    // Briefly open a cpal input stream to trigger macOS's native permission dialog,
    // then check if the device became available.
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        if let Some(device) = host.default_input_device() {
            let config = device
                .default_input_config()
                .map(|c: cpal::SupportedStreamConfig| c.config())
                .unwrap_or(cpal::StreamConfig {
                    channels: 1,
                    sample_rate: cpal::SampleRate(16000),
                    buffer_size: cpal::BufferSize::Default,
                });
            if let Ok(stream) = device.build_input_stream(
                &config,
                |_data: &[f32], _: &cpal::InputCallbackInfo| {},
                |_err| {},
                None,
            ) {
                let _ = stream.play();
                std::thread::sleep(std::time::Duration::from_millis(200));
                drop(stream);
            }
        }
        let available = host.default_input_device().is_some();
        let _ = tx.send(available);
    });
    let result = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap_or(false);
    Ok(result)
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
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
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
