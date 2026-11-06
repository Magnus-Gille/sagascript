//! The recorder's commands (SAGA-564, docs/recorder-design.md §4.5-4.7): the
//! session, the library of recordings and the recorder's settings.
//!
//! The decisions are plain functions in `recording_session.rs` (and `trash.rs`),
//! tested there; the commands name them and add what needs the app. Every
//! failure is a [`CommandError`]; a recorder failure keeps its `recording_*`
//! code ([`sagascript_core::recording::RecordingError`]).

use std::path::Path;
use std::sync::Arc;

use sagascript_core::recording::{list_recordings as list_recordings_in, RecordingSummary};
use sagascript_core::settings::{expand_home, RecordingQuality, Settings};
use tauri::State;
use tracing::info;

use super::{lock_controller, persist_and_sync, try_persist_and_mirror, SharedController};
use crate::command_error::CommandError;
use crate::events::{event, EmitEvent};
use crate::recording_session::{
    check_permission, delete_recording_in, folder_info_on_disk, recover_pending, start_session,
    stop_session, Emit, RecordingFinished, RecordingFolderInfo, RecordingSessionInfo, RecoveryNote,
    SharedRecorder,
};
use crate::trash::recording_folder;

/// The settings the recorder reads, from the controller's copy.
fn recorder_settings(controller: &SharedController) -> Settings {
    lock_controller(controller).settings().clone()
}

/// Sagascript does not record a session while it dictates or transcribes.
fn ensure_not_dictating(controller: &SharedController) -> Result<(), CommandError> {
    if lock_controller(controller).state().is_busy() {
        return Err(CommandError::Busy(
            "Cannot start a recording while Sagascript is dictating or transcribing. Wait for it to finish."
                .into(),
        ));
    }
    Ok(())
}

/// The microphone permission, asked for if the user has not answered yet: the
/// prompt must not happen inside the session's wait for the first audio.
async fn ensure_microphone_permission() -> Result<(), CommandError> {
    let permission = crate::platform::mic_permission();
    let mut status = permission.status();
    if status == "not_determined" {
        status = permission.request().await?;
    }
    check_permission(&status)?;
    Ok(())
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, CommandError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| CommandError::task_failed(error.to_string()))
}

#[tauri::command]
pub async fn start_recording_session(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    recorder: State<'_, SharedRecorder>,
    title: Option<String>,
) -> Result<RecordingSessionInfo, CommandError> {
    ensure_not_dictating(&controller)?;
    ensure_microphone_permission().await?;
    let settings = recorder_settings(&controller);
    let emit: Emit = Arc::new(move |info| {
        let _ = app.emit_event(event::RECORDING_SESSION_CHANGED, info);
    });
    let started = start_session(&recorder, settings, title, emit).await?;
    info!("Recording session started");
    Ok(started)
}

#[tauri::command]
pub async fn stop_recording_session(
    recorder: State<'_, SharedRecorder>,
) -> Result<RecordingFinished, CommandError> {
    let finished = stop_session(&recorder).await?;
    info!("Recording session stopped");
    Ok(finished)
}

#[tauri::command]
pub async fn get_recording_session(
    recorder: State<'_, SharedRecorder>,
) -> Result<RecordingSessionInfo, CommandError> {
    Ok(recorder.info())
}

/// The recordings in the configured folder, newest first.
#[tauri::command]
pub async fn list_recordings(
    controller: State<'_, SharedController>,
) -> Result<Vec<RecordingSummary>, CommandError> {
    let root = recorder_settings(&controller).recordings_dir();
    blocking(move || list_recordings_in(&root)).await
}

/// Move a recording to the Trash or Recycle Bin. Never deletes for good.
#[tauri::command]
pub async fn delete_recording(
    controller: State<'_, SharedController>,
    recorder: State<'_, SharedRecorder>,
    folder_name: String,
) -> Result<(), CommandError> {
    let root = recorder_settings(&controller).recordings_dir();
    let active = recorder.active_folder_name();
    let trash = recorder.trash();
    blocking(move || delete_recording_in(&root, &folder_name, active.as_deref(), trash.as_ref()))
        .await??;
    info!("A recording was moved to the trash");
    Ok(())
}

/// Show a recording's folder in Finder or Explorer.
#[tauri::command]
pub async fn reveal_recording(
    controller: State<'_, SharedController>,
    folder_name: String,
) -> Result<(), CommandError> {
    let root = recorder_settings(&controller).recordings_dir();
    let folder = recording_folder(&root, &folder_name)?;
    reveal_in_file_manager(&folder)
}

#[cfg(target_os = "macos")]
fn reveal_in_file_manager(folder: &Path) -> Result<(), CommandError> {
    std::process::Command::new("open")
        .arg("-R")
        .arg(folder)
        .spawn()
        .map(drop)
        .map_err(|error| {
            CommandError::LaunchFailed("Could not show the recording in Finder.".into())
                .because(error)
        })
}

#[cfg(windows)]
fn reveal_in_file_manager(folder: &Path) -> Result<(), CommandError> {
    use std::os::windows::process::CommandExt;
    // Explorer parses its own command line: `/select,"<path>"`, quoted as one
    // piece. (Its exit code is 1 even on success, so it is not waited for.)
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", folder.display()))
        .spawn()
        .map(drop)
        .map_err(|error| {
            CommandError::LaunchFailed("Could not show the recording in Explorer.".into())
                .because(error)
        })
}

#[cfg(not(any(target_os = "macos", windows)))]
fn reveal_in_file_manager(_folder: &Path) -> Result<(), CommandError> {
    Err(CommandError::Unavailable(
        "Showing a recording in the file manager is not available on this system.".into(),
    ))
}

/// Recover interrupted recordings and describe what was kept of each, with the
/// notes the startup recovery left for the window. Recordings that were
/// complete already, and the one being made now, get no note.
#[tauri::command]
pub async fn recover_recordings(
    controller: State<'_, SharedController>,
    recorder: State<'_, SharedRecorder>,
) -> Result<Vec<RecoveryNote>, CommandError> {
    let root = recorder_settings(&controller).recordings_dir();
    let skip = recorder.active_folder_name();
    let scanned = blocking(move || recover_pending(&root, skip.as_deref())).await??;
    let mut notes = recorder.take_notes();
    notes.extend(scanned);
    Ok(notes)
}

// TODO(agent2): transcribe_recording hands a finished recording to the file
// transcription path (`decode_tracks` of the mic track). Until then the window
// gets `unavailable`.
#[tauri::command]
pub async fn transcribe_recording(folder_name: String) -> Result<Option<String>, CommandError> {
    let _ = folder_name;
    Err(CommandError::Unavailable(
        "Transcribing a recording is not available yet.".into(),
    ))
}

/// The recordings folder (`path` null: the configured one) and its volume.
#[tauri::command]
pub async fn get_recording_folder_info(
    controller: State<'_, SharedController>,
    path: Option<String>,
) -> Result<RecordingFolderInfo, CommandError> {
    let settings = recorder_settings(&controller);
    blocking(move || folder_info_on_disk(&settings, path.as_deref())).await
}

// -- Settings ----------------------------------------------------------------------

#[tauri::command]
pub async fn set_recording_quality(
    controller: State<'_, SharedController>,
    quality: RecordingQuality,
) -> Result<(), CommandError> {
    persist_and_sync(&controller, |settings| {
        settings.recording_quality = quality;
    })?;
    info!("Recording quality: {}", quality.id());
    Ok(())
}

/// The folder as it is stored: `~/` expanded, empty for the default. The app
/// has no working directory worth resolving against (unlike the CLI), so a
/// relative path is refused.
fn recording_directory_to_store(path: &str) -> Result<String, CommandError> {
    let path = path.trim();
    if path.is_empty() {
        return Ok(String::new());
    }
    let expanded = expand_home(path);
    if !Path::new(&expanded).is_absolute() {
        return Err(CommandError::InvalidInput(
            "Choose the recordings folder by its full path.".into(),
        ));
    }
    Ok(expanded)
}

/// The persistence half of `set_recording_directory`.
pub(super) fn persist_recording_directory(
    controller: &SharedController,
    path: &str,
) -> Result<Settings, CommandError> {
    let directory = recording_directory_to_store(path)?;
    let is_file = Path::new(&directory).is_file();
    let persisted = try_persist_and_mirror(
        controller,
        |settings| settings.set_recording_directory(&directory, is_file),
        |memory, written| memory.recording_directory = written.recording_directory.clone(),
    )?;
    Ok(persisted.current)
}

/// An empty path selects the default folder.
#[tauri::command]
pub async fn set_recording_directory(
    controller: State<'_, SharedController>,
    path: String,
) -> Result<(), CommandError> {
    persist_recording_directory(&controller, &path)?;
    info!("Recording folder changed");
    Ok(())
}

/// The persistence half of `set_recording_channel_pair`.
pub(super) fn persist_recording_channel_pair(
    controller: &SharedController,
    first: u16,
    second: u16,
) -> Result<Settings, CommandError> {
    let persisted = try_persist_and_mirror(
        controller,
        |settings| settings.set_recording_channel_pair(first, second),
        |memory, written| memory.recording_channel_pair = written.recording_channel_pair,
    )?;
    Ok(persisted.current)
}

/// Input channels `first` and `second`, counted from 1; `[n, n]` records
/// channel n as mono.
#[tauri::command]
pub async fn set_recording_channel_pair(
    controller: State<'_, SharedController>,
    first: u16,
    second: u16,
) -> Result<(), CommandError> {
    persist_recording_channel_pair(&controller, first, second)?;
    info!("Recording channels: {first}-{second}");
    Ok(())
}

/// The user has confirmed the recordings folder (first use).
#[tauri::command]
pub async fn confirm_recording_folder(
    controller: State<'_, SharedController>,
) -> Result<(), CommandError> {
    persist_and_sync(&controller, |settings| {
        settings.recording_folder_confirmed = true;
    })?;
    Ok(())
}

/// The user has read the reminder to ask before recording other people.
#[tauri::command]
pub async fn acknowledge_recording_consent(
    controller: State<'_, SharedController>,
) -> Result<(), CommandError> {
    persist_and_sync(&controller, |settings| {
        settings.recording_consent_acknowledged = true;
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "recording_tests.rs"]
mod tests;
