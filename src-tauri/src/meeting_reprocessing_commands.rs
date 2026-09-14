//! Explicit proposal workflow. No command implicitly replaces the active review.
use sagascript_core::meeting_reprocess::MigrationPreview;
use sagascript_core::meeting_reprocess_plan::{ReprocessingMode, ReprocessingPlan};
use sagascript_core::meeting_reprocess_proposal::MeetingReprocessingProposal;
use sagascript_core::meeting_review::{CorrectionOperation, MeetingReview};
use serde::Serialize;
use tauri::State;
use tauri_plugin_dialog::DialogExt;

use crate::commands::SharedController;
use crate::meeting_review_commands::{require_settings, ReviewState};

#[derive(Serialize)]
pub struct SelectedReprocessingPlan {
    plan: ReprocessingPlan,
    file_path: String,
    cache_path: Option<String>,
    cache_output: Option<String>,
}

#[derive(Serialize)]
pub struct ProposalState {
    proposal: MeetingReprocessingProposal,
    preview: MigrationPreview,
    candidate: sagascript_core::meeting::MeetingTranscript,
}

impl ProposalState {
    fn new(proposal: MeetingReprocessingProposal) -> Result<Self, String> {
        let preview = proposal.preview().map_err(|e| e.to_string())?;
        let candidate = preview.candidate.materialize().map_err(|e| e.to_string())?;
        Ok(Self {
            proposal,
            preview,
            candidate,
        })
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri injects application/window/controller arguments.
pub async fn plan_meeting_reprocessing(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    controller: State<'_, SharedController>,
    previous: MeetingReview,
    mode: ReprocessingMode,
    threshold: f32,
    save_cache: bool,
    prompt: Option<String>,
    profile_id: Option<String>,
) -> Result<Option<SelectedReprocessingPlan>, String> {
    require_settings(&window)?;
    #[cfg(not(feature = "diarization"))]
    {
        let _ = (
            app, controller, previous, mode, threshold, save_cache, prompt, profile_id,
        );
        Err("This build has no speaker diarization support.".into())
    }
    #[cfg(feature = "diarization")]
    {
        let settings = controller
            .lock()
            .map_err(|_| "Settings are unavailable")?
            .settings()
            .clone();
        let context = crate::commands::file_transcription_context(
            &settings,
            profile_id.as_deref(),
            prompt.as_deref(),
        )?;
        tauri::async_runtime::spawn_blocking(move || {
            previous.validate().map_err(|e| e.to_string())?;
            if save_cache && mode != ReprocessingMode::Full {
                return Err(
                    "Saving a new reusable cache requires explicit full recomputation.".into(),
                );
            }
            let selected = app
                .dialog()
                .file()
                .set_title("Choose the original meeting recording")
                .blocking_pick_file();
            let Some(selected) = selected else {
                return Ok(None);
            };
            let audio = selected
                .into_path()
                .map_err(|_| "Choose a local recording")?;
            let cache = if mode != ReprocessingMode::Full {
                let selected = app
                    .dialog()
                    .file()
                    .set_title("Choose the reusable meeting cache")
                    .add_filter("Meeting cache", &["json"])
                    .blocking_pick_file();
                let Some(selected) = selected else {
                    return Ok(None);
                };
                Some(selected.into_path().map_err(|_| "Choose a local cache")?)
            } else {
                None
            };
            let cache_output = if save_cache {
                let selected = app
                    .dialog()
                    .file()
                    .set_title("Save reusable voice data — choose a NEW file")
                    .set_file_name("meeting-cache.json")
                    .blocking_save_file();
                let Some(selected) = selected else {
                    return Ok(None);
                };
                Some(
                    selected
                        .into_path()
                        .map_err(|_| "Choose a local cache destination")?,
                )
            } else {
                None
            };
            let input = sagascript_cli::meeting_reprocessing::ReprocessingInput {
                audio: &audio,
                cache: cache.as_deref(),
                previous: &previous,
                language: context.language,
                model: context.model,
                glossary: &context.glossary,
            };
            let plan = sagascript_cli::meeting_reprocessing::plan_reprocessing(
                &input, mode, threshold, None,
            )
            .map_err(|e| e.to_string())?;
            let path_string = |path: std::path::PathBuf| {
                path.into_os_string()
                    .into_string()
                    .map_err(|_| "Choose a path that can be represented as Unicode".to_owned())
            };
            Ok(Some(SelectedReprocessingPlan {
                plan,
                file_path: path_string(audio)?,
                cache_path: cache.map(path_string).transpose()?,
                cache_output: cache_output.map(path_string).transpose()?,
            }))
        })
        .await
        .map_err(|_| "Reprocessing planning worker stopped unexpectedly".to_owned())?
    }
}

#[tauri::command]
pub async fn preview_meeting_proposal(
    proposal: MeetingReprocessingProposal,
) -> Result<ProposalState, String> {
    tauri::async_runtime::spawn_blocking(move || ProposalState::new(proposal))
        .await
        .map_err(|_| "Proposal preview worker stopped unexpectedly".to_owned())?
}

#[tauri::command]
pub async fn resolve_meeting_proposal(
    proposal: MeetingReprocessingProposal,
    expected_revision: String,
    index: usize,
    operations: Vec<CorrectionOperation>,
) -> Result<ProposalState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let next = proposal
            .resolve(&expected_revision, index, operations)
            .map_err(|e| e.to_string())?;
        ProposalState::new(next)
    })
    .await
    .map_err(|_| "Proposal resolution worker stopped unexpectedly".to_owned())?
}

#[tauri::command]
pub async fn accept_meeting_proposal(
    proposal: MeetingReprocessingProposal,
    current_review: MeetingReview,
) -> Result<ReviewState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        current_review.validate().map_err(|e| e.to_string())?;
        ReviewState::from_review(
            proposal
                .accept(&current_review.revision)
                .map_err(|e| e.to_string())?,
        )
    })
    .await
    .map_err(|_| "Proposal acceptance worker stopped unexpectedly".to_owned())?
}

#[tauri::command]
pub async fn save_meeting_proposal(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    proposal: MeetingReprocessingProposal,
) -> Result<bool, String> {
    require_settings(&window)?;
    tauri::async_runtime::spawn_blocking(move || {
        proposal.validate().map_err(|e| e.to_string())?;
        let output = serde_json::to_vec(&proposal).map_err(|_| "Could not serialize proposal")?;
        let selected = app
            .dialog()
            .file()
            .set_title("Save meeting proposal — choose a NEW file")
            .set_file_name("meeting-proposal.json")
            .blocking_save_file();
        let Some(selected) = selected else {
            return Ok(false);
        };
        let path = selected
            .into_path()
            .map_err(|_| "Choose a local proposal destination")?;
        crate::meeting_jobs::write_new_export(&path, &output)?;
        Ok(true)
    })
    .await
    .map_err(|_| "Proposal save worker stopped unexpectedly".to_owned())?
}

#[tauri::command]
pub async fn open_meeting_proposal(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<Option<ProposalState>, String> {
    require_settings(&window)?;
    tauri::async_runtime::spawn_blocking(move || {
        let selected = app
            .dialog()
            .file()
            .set_title("Open meeting proposal")
            .add_filter("Meeting proposal", &["json"])
            .blocking_pick_file();
        let Some(selected) = selected else {
            return Ok(None);
        };
        let path = selected
            .into_path()
            .map_err(|_| "Choose a local proposal")?;
        let proposal = sagascript_cli::meeting_proposal::read_proposal(&path)
            .map_err(|_| "Could not read a valid, bounded meeting proposal".to_owned())?;
        ProposalState::new(proposal).map(Some)
    })
    .await
    .map_err(|_| "Proposal open worker stopped unexpectedly".to_owned())?
}
