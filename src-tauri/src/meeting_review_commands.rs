//! Explicit, local-only review transforms and document dialogs.
use std::io::Read;

use sagascript_core::meeting::MeetingTranscript;
use sagascript_core::meeting_review::{CorrectionFile, MeetingReview, MAX_SERIALIZED_REVIEW_BYTES};
use serde::Serialize;
use tauri_plugin_dialog::DialogExt;

use crate::meeting_jobs::{write_new_export, ExportFormat};

#[derive(Serialize)]
pub struct ReviewState {
    review: MeetingReview,
    transcript: MeetingTranscript,
}

impl ReviewState {
    pub(crate) fn from_review(review: MeetingReview) -> Result<Self, String> {
        let transcript = review.materialize().map_err(|e| e.to_string())?;
        Ok(Self { review, transcript })
    }
}

pub(crate) fn require_settings(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "settings" {
        return Err("Meeting documents are available only in Settings.".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn create_meeting_review(transcript: MeetingTranscript) -> Result<ReviewState, String> {
    review_worker(move || MeetingReview::new(transcript).map_err(|e| e.to_string())).await
}

async fn review_worker(
    operation: impl FnOnce() -> Result<MeetingReview, String> + Send + 'static,
) -> Result<ReviewState, String> {
    tauri::async_runtime::spawn_blocking(move || ReviewState::from_review(operation()?))
        .await
        .map_err(|e| format!("Review operation worker failed: {e}"))?
}

#[tauri::command]
pub async fn apply_meeting_corrections(
    review: MeetingReview,
    corrections: CorrectionFile,
) -> Result<ReviewState, String> {
    review_worker(move || {
        review
            .apply_corrections(&corrections)
            .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
pub async fn undo_meeting_review(
    review: MeetingReview,
    expected_revision: String,
) -> Result<ReviewState, String> {
    review_worker(move || review.undo(&expected_revision).map_err(|e| e.to_string())).await
}

#[tauri::command]
pub async fn reset_meeting_review(
    review: MeetingReview,
    expected_revision: String,
) -> Result<ReviewState, String> {
    review_worker(move || review.reset(&expected_revision).map_err(|e| e.to_string())).await
}

fn read_review(path: &std::path::Path) -> Result<MeetingReview, String> {
    let limit = MAX_SERIALIZED_REVIEW_BYTES as u64;
    let metadata = std::fs::metadata(path).map_err(|e| format!("Cannot read review: {e}"))?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("Choose a regular review file no larger than 24 MiB.".into());
    }
    let file = std::fs::File::open(path).map_err(|e| format!("Cannot open review: {e}"))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("Cannot inspect review: {e}"))?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("Choose a regular review file no larger than 24 MiB.".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Cannot read review: {e}"))?;
    if bytes.len() > MAX_SERIALIZED_REVIEW_BYTES {
        return Err("Review file exceeds 24 MiB.".into());
    }
    let review: MeetingReview =
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid review document: {e}"))?;
    Ok(review)
}

#[tauri::command]
pub async fn open_meeting_review(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<Option<ReviewState>, String> {
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
            .map_err(|e| format!("Choose a local review file: {e}"))?;
        ReviewState::from_review(read_review(&path)?).map(Some)
    })
    .await
    .map_err(|e| format!("Review open worker failed: {e}"))?
}

#[tauri::command]
pub async fn save_meeting_review(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    review: MeetingReview,
    format: ExportFormat,
) -> Result<bool, String> {
    require_settings(&window)?;
    let (format, extension) = format.details();
    tauri::async_runtime::spawn_blocking(move || {
        let output = review.export(format).map_err(|e| e.to_string())?;
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
            .map_err(|e| format!("Choose a local export file: {e}"))?;
        write_new_export(&path, output.as_bytes())?;
        Ok(true)
    })
    .await
    .map_err(|e| format!("Review save worker failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use sagascript_core::meeting::{MeetingExportFormat, MeetingSegmentInput, MeetingSpeaker};

    #[test]
    fn review_file_roundtrip_and_failed_overwrite_preserve_bytes() {
        let directory =
            std::env::temp_dir().join(format!("sagascript-review-io-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let original = MeetingTranscript::new(
            "a".repeat(64),
            "sv",
            "base",
            2.0,
            vec![MeetingSegmentInput {
                start: 0.0,
                end: 1.0,
                text: "Hej åäö!".into(),
                speaker: "s1".into(),
            }],
            vec![MeetingSpeaker {
                id: "s1".into(),
                label: "Talare".into(),
            }],
        )
        .unwrap();
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
        assert_eq!(std::fs::read(&invalid).unwrap(), b"{}");
        // Only this test's exact, uniquely created synthetic files are removed.
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(invalid).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
