//! Explicit, local-only review transforms and document dialogs.
use std::io::Read;

use sagascript_core::meeting::review::{
    CorrectionFile, MeetingReview, MAX_SERIALIZED_REVIEW_BYTES,
};
use sagascript_core::meeting::MeetingTranscript;
use serde::Serialize;
use tauri_plugin_dialog::DialogExt;

use crate::command_error::CommandError;
use crate::meeting_jobs::{write_new_export, ExportFormat};

#[derive(Serialize)]
pub struct ReviewState {
    review: MeetingReview,
    transcript: MeetingTranscript,
}

impl ReviewState {
    pub(crate) fn from_review(review: MeetingReview) -> Result<Self, CommandError> {
        let transcript = review.materialize()?;
        Ok(Self { review, transcript })
    }
}

pub(crate) fn require_settings(window: &tauri::WebviewWindow) -> Result<(), CommandError> {
    if window.label() != "settings" {
        return Err(CommandError::Unavailable(
            "Meeting documents are available only in Settings.".into(),
        ));
    }
    Ok(())
}

#[tauri::command]
pub async fn create_meeting_review(
    transcript: MeetingTranscript,
) -> Result<ReviewState, CommandError> {
    review_worker(move || Ok(MeetingReview::new(transcript)?)).await
}

async fn review_worker(
    operation: impl FnOnce() -> Result<MeetingReview, CommandError> + Send + 'static,
) -> Result<ReviewState, CommandError> {
    tauri::async_runtime::spawn_blocking(move || ReviewState::from_review(operation()?))
        .await
        .map_err(|e| CommandError::task_failed(format!("Review operation worker failed: {e}")))?
}

#[tauri::command]
pub async fn apply_meeting_corrections(
    review: MeetingReview,
    corrections: CorrectionFile,
) -> Result<ReviewState, CommandError> {
    review_worker(move || Ok(review.apply_corrections(&corrections)?)).await
}

#[tauri::command]
pub async fn undo_meeting_review(
    review: MeetingReview,
    expected_revision: String,
) -> Result<ReviewState, CommandError> {
    review_worker(move || Ok(review.undo(&expected_revision)?)).await
}

#[tauri::command]
pub async fn reset_meeting_review(
    review: MeetingReview,
    expected_revision: String,
) -> Result<ReviewState, CommandError> {
    review_worker(move || Ok(review.reset(&expected_revision)?)).await
}

fn read_review(path: &std::path::Path) -> Result<MeetingReview, CommandError> {
    let limit = MAX_SERIALIZED_REVIEW_BYTES as u64;
    let metadata = std::fs::metadata(path)
        .map_err(|e| CommandError::Io(format!("Cannot read review: {e}")))?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(CommandError::InvalidInput(
            "Choose a regular review file no larger than 24 MiB.".into(),
        ));
    }
    let file = std::fs::File::open(path)
        .map_err(|e| CommandError::Io(format!("Cannot open review: {e}")))?;
    let metadata = file
        .metadata()
        .map_err(|e| CommandError::Io(format!("Cannot inspect review: {e}")))?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(CommandError::InvalidInput(
            "Choose a regular review file no larger than 24 MiB.".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| CommandError::Io(format!("Cannot read review: {e}")))?;
    if bytes.len() > MAX_SERIALIZED_REVIEW_BYTES {
        return Err(CommandError::InvalidInput(
            "Review file exceeds 24 MiB.".into(),
        ));
    }
    let review: MeetingReview = serde_json::from_slice(&bytes)
        .map_err(|e| CommandError::InvalidInput(format!("Invalid review document: {e}")))?;
    Ok(review)
}

#[tauri::command]
pub async fn open_meeting_review(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<Option<ReviewState>, CommandError> {
    require_settings(&window)?;
    tauri::async_runtime::spawn_blocking(move || {
        let selected = app
            .dialog()
            .file()
            .set_title("Open meeting review")
            .add_filter("Meeting review", &["json"])
            .blocking_pick_file();
        let Some(selected) = selected else {
            return Ok(None);
        };
        let path = selected
            .into_path()
            .map_err(|e| CommandError::InvalidInput(format!("Choose a local review file: {e}")))?;
        ReviewState::from_review(read_review(&path)?).map(Some)
    })
    .await
    .map_err(|e| CommandError::task_failed(format!("Review open worker failed: {e}")))?
}

#[tauri::command]
pub async fn save_meeting_review(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    review: MeetingReview,
    format: ExportFormat,
) -> Result<bool, CommandError> {
    require_settings(&window)?;
    let (format, extension) = format.details();
    tauri::async_runtime::spawn_blocking(move || {
        let output = review.export(format)?;
        let selected = app
            .dialog()
            .file()
            .set_title("Save meeting review — choose a new file")
            .set_file_name(format!("meeting-reviewed.{extension}"))
            .add_filter("Meeting review", &[extension])
            .blocking_save_file();
        let Some(selected) = selected else {
            return Ok(false);
        };
        let path = selected
            .into_path()
            .map_err(|e| CommandError::InvalidInput(format!("Choose a local export file: {e}")))?;
        write_new_export(&path, output.as_bytes())?;
        Ok(true)
    })
    .await
    .map_err(|e| CommandError::task_failed(format!("Review save worker failed: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use sagascript_core::meeting::MeetingExportFormat;
    use sagascript_core::test_support::meeting_transcript;
    use sagascript_testsupport::TempDir;

    #[test]
    fn review_file_roundtrip_and_failed_overwrite_preserve_bytes() {
        let directory = TempDir::new();
        let original = meeting_transcript()
            .language("sv")
            .model("base")
            .duration(2.0)
            .segment(0.0, 1.0, "Hej åäö!", "s1")
            .speaker("s1", "Talare")
            .build();
        let review = MeetingReview::new(original).unwrap();
        let output = review.export(MeetingExportFormat::Json).unwrap();
        let path = directory.join("review.json");
        write_new_export(&path, output.as_bytes()).unwrap();
        assert_eq!(read_review(&path).unwrap(), review);
        assert!(write_new_export(&path, b"replace").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), output.as_bytes());
        assert!(read_review(&directory).is_err());
        let invalid = directory.join("invalid.json");
        write_new_export(&invalid, b"{}").unwrap();
        assert!(read_review(&invalid).is_err());
        // Each refusal says what kind it is: a folder is not a review file, a
        // missing file cannot be read, a file that is no review is invalid.
        assert_eq!(read_review(&directory).unwrap_err().code(), "invalid_input");
        assert_eq!(
            read_review(&directory.join("nothing.json"))
                .unwrap_err()
                .code(),
            "io_error"
        );
        let invalid_document = read_review(&invalid).unwrap_err();
        assert_eq!(invalid_document.code(), "invalid_input");
        assert!(
            invalid_document
                .to_string()
                .starts_with("Invalid review document:"),
            "{invalid_document}"
        );
        assert_eq!(std::fs::read(&invalid).unwrap(), b"{}");
    }
}
