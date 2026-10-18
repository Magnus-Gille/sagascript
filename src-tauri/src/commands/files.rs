//! What file transcription needs before decoding starts (language, model,
//! dictionary and decoder options frozen together) and the exports of a
//! finished transcript: clipboard, save dialog, supported formats.

use sagascript_core::audio::decoder;
use sagascript_core::error::DictationError;
use sagascript_core::run_context::{file_beam_size, RunContext, RunOverrides, RunSelection};
use sagascript_core::settings::{FileModel, Language, Settings, SettingsError, WhisperModel};
use sagascript_core::transcription::{Glossary, TranscribeOptions};
use tauri_plugin_dialog::DialogExt;

use super::build_transcribe_options;
use crate::command_error::CommandError;

/// Like [`build_transcribe_options`] but for file transcription: defaults to
/// beam search for quality (unless the user explicitly set a beam width), and
/// uses the file dialog's prompt when provided (otherwise the saved prompt).
pub(crate) fn build_file_transcribe_options(
    settings: &Settings,
    prompt: Option<String>,
) -> TranscribeOptions {
    let mut opts = build_transcribe_options(settings);
    opts.beam_size = file_beam_size(opts.beam_size);
    opts.prompt = effective_file_glossary(settings, prompt.as_deref()).decoder_prompt();
    opts
}

pub(crate) fn effective_file_glossary(settings: &Settings, prompt: Option<&str>) -> Glossary {
    Glossary::parse(&settings.effective_glossary_source_with_prompt(None, prompt))
}

pub(crate) struct FileTranscriptionContext {
    pub(crate) language: Language,
    pub(crate) model: WhisperModel,
    pub(crate) glossary: Glossary,
    pub(crate) options: TranscribeOptions,
}

/// Freeze language, model and dictionary together before file decoding begins.
/// A missing profile resolves the default profile; unknown IDs fail closed.
///
/// Profile, language and dictionary come from the shared run resolver (the CLI
/// resolves them the same way, #346); the model is this path's own choice. The
/// app offers no language override, so the language is always the profile's.
pub(crate) fn file_transcription_context(
    settings: &Settings,
    profile_id: Option<&str>,
    prompt: Option<&str>,
) -> Result<FileTranscriptionContext, CommandError> {
    let selection = file_transcription_selection(settings, profile_id)?;
    // Meetings and diarization need Whisper: an Auto preference uses the
    // recommended Whisper model rather than Pianissimo.
    let model = match settings.effective_whisper_file_model_for(selection.language)? {
        FileModel::Whisper(model) => model,
        FileModel::PianissimoOriginal => return Err(SettingsError::PianissimoPlainFilesOnly.into()),
    };
    let RunContext {
        language, glossary, ..
    } = selection.into_context(settings, prompt);
    let mut options = build_file_transcribe_options(settings, prompt.map(str::to_owned));
    options.prompt = glossary.decoder_prompt();
    Ok(FileTranscriptionContext {
        language,
        model,
        glossary,
        options,
    })
}

pub(crate) fn file_transcription_language(
    settings: &Settings,
    profile_id: Option<&str>,
) -> Result<Language, CommandError> {
    file_transcription_selection(settings, profile_id).map(|selection| selection.language)
}

/// The profile a file run uses and its language. The resolver's settings error
/// keeps its code and its message, without the CLI's "Settings error: " prefix.
fn file_transcription_selection(
    settings: &Settings,
    profile_id: Option<&str>,
) -> Result<RunSelection, CommandError> {
    RunSelection::resolve(
        settings,
        RunOverrides {
            profile: profile_id,
            language: None,
        },
    )
    .map_err(|error| match error {
        DictationError::Settings(error) => CommandError::Settings(error),
        other => CommandError::Dictation(other),
    })
}

/// Copy a finished plain transcription to the clipboard (#238). Plain
/// `set_text` only — no paste, no restore dance (see `paste::PasteService`
/// for the guarded live-dictation path).
#[tauri::command]
pub async fn copy_transcription_text(text: String) -> Result<(), CommandError> {
    if text.trim().is_empty() {
        return Err(CommandError::InvalidInput(
            "There is no transcription to copy.".to_string(),
        ));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let mut clipboard = arboard::Clipboard::new()
            .map_err(|error| CommandError::Clipboard(format!("Clipboard error: {error}")))?;
        clipboard
            .set_text(text)
            .map_err(|error| CommandError::Clipboard(format!("Could not copy: {error}")))
    })
    .await
    .map_err(|error| CommandError::task_failed(format!("Copy worker failed: {error}")))?
}

/// Explicit manual save of a finished plain transcription (#238). Always
/// shows the native save dialog prefilled with `file_name` in `directory`;
/// cancelling writes nothing. Reuses the atomic, never-overwrite
/// `write_new_export` publisher, so this path can never truncate an
/// existing file. There is deliberately no automatic-save counterpart
/// (privacy-first: transcripts reach disk only on explicit request).
#[tauri::command]
pub async fn save_transcription_text(
    app: tauri::AppHandle,
    text: String,
    file_name: String,
    directory: Option<String>,
) -> Result<bool, CommandError> {
    if text.trim().is_empty() {
        return Err(CommandError::InvalidInput(
            "There is no transcription to save.".to_string(),
        ));
    }
    let file_name = sanitize_save_file_name(&file_name);
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = app
            .dialog()
            .file()
            .set_title("Save transcription — choose a file")
            .set_file_name(&file_name)
            .add_filter("Text", &["txt"]);
        if let Some(dir) = directory.filter(|dir| !dir.trim().is_empty()) {
            dialog = dialog.set_directory(dir);
        }
        let Some(selected) = dialog.blocking_save_file() else {
            return Ok(false);
        };
        let path = selected
            .into_path()
            .map_err(|error| CommandError::InvalidInput(format!("Choose a local file: {error}")))?;
        crate::meeting_jobs::write_new_export(&path, text.as_bytes()).map_err(|error| {
            if path.exists() {
                CommandError::ExportExists(
                    "Choose a new file name — existing files are never overwritten.".to_string(),
                )
            } else {
                error
            }
        })?;
        Ok(true)
    })
    .await
    .map_err(|error| CommandError::task_failed(format!("Save worker failed: {error}")))?
}

/// Keep the save dialog default to a bare file name: strip any directories
/// (the dialog's directory picker owns the destination) and fall back to a
/// neutral name. The extension is preserved when present, else `.txt`.
fn sanitize_save_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if base.is_empty() {
        return "transcription.txt".to_string();
    }
    if base.contains('.') {
        base.to_string()
    } else {
        format!("{base}.txt")
    }
}

#[tauri::command]
pub async fn get_supported_formats() -> Result<Vec<String>, CommandError> {
    Ok(decoder::SUPPORTED_EXTENSIONS
        .iter()
        .map(|s| s.to_string())
        .collect())
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
