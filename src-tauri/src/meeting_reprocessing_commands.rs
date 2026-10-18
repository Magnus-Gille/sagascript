//! Explicit proposal workflow. No command implicitly replaces the active review.
use sagascript_core::meeting::reprocess::plan::{ReprocessingMode, ReprocessingPlan};
use sagascript_core::meeting::reprocess::proposal::MeetingReprocessingProposal;
use sagascript_core::meeting::reprocess::MigrationPreview;
use sagascript_core::meeting::review::{CorrectionOperation, MeetingReview};
use serde::Serialize;
use tauri::State;
use tauri_plugin_dialog::DialogExt;

use crate::command_error::CommandError;
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
    fn new(proposal: MeetingReprocessingProposal) -> Result<Self, CommandError> {
        let preview = proposal.preview()?;
        let candidate = preview.candidate.materialize()?;
        Ok(Self {
            proposal,
            preview,
            candidate,
        })
    }
}

/// The proposal at `path`. The text of a failure is fixed on purpose (a
/// proposal holds a transcript); the cause, scrubbed, goes to the log and stays
/// on the error ([`CommandError::because`]).
fn read_proposal_file(
    path: &std::path::Path,
) -> Result<sagascript_core::meeting::reprocess::proposal::MeetingReprocessingProposal, CommandError>
{
    sagascript_cli::meeting_proposal::read_proposal(path).map_err(|error| {
        CommandError::InvalidInput("Could not read a valid, bounded meeting proposal".into())
            .because(error)
    })
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
) -> Result<Option<SelectedReprocessingPlan>, CommandError> {
    require_settings(&window)?;
    #[cfg(not(feature = "diarization"))]
    {
        let _ = (
            app, controller, previous, mode, threshold, save_cache, prompt, profile_id,
        );
        Err(CommandError::Unavailable(
            "This build has no speaker diarization support.".into(),
        ))
    }
    #[cfg(feature = "diarization")]
    {
        let settings = crate::commands::lock_controller(&controller)
            .settings()
            .clone();
        let context = crate::commands::file_transcription_context(
            &settings,
            profile_id.as_deref(),
            prompt.as_deref(),
        )?;
        tauri::async_runtime::spawn_blocking(move || {
            previous.validate()?;
            if save_cache && mode != ReprocessingMode::Full {
                return Err(CommandError::InvalidInput(
                    "Saving a new reusable cache requires explicit full recomputation.".into(),
                ));
            }
            let selected = app
                .dialog()
                .file()
                .set_title("Choose the original meeting recording")
                .blocking_pick_file();
            let Some(selected) = selected else {
                return Ok(None);
            };
            let audio = selected.into_path().map_err(|error| {
                CommandError::InvalidInput("Choose a local recording".into()).because(error)
            })?;
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
                Some(selected.into_path().map_err(|error| {
                    CommandError::InvalidInput("Choose a local cache".into()).because(error)
                })?)
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
                Some(selected.into_path().map_err(|error| {
                    CommandError::InvalidInput("Choose a local cache destination".into())
                        .because(error)
                })?)
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
            )?;
            let path_string = |path: std::path::PathBuf| {
                path.into_os_string().into_string().map_err(|_| {
                    CommandError::InvalidInput(
                        "Choose a path that can be represented as Unicode".into(),
                    )
                })
            };
            Ok(Some(SelectedReprocessingPlan {
                plan,
                file_path: path_string(audio)?,
                cache_path: cache.map(path_string).transpose()?,
                cache_output: cache_output.map(path_string).transpose()?,
            }))
        })
        .await
        .map_err(|error| {
            CommandError::task_failed("Reprocessing planning worker stopped unexpectedly")
                .because(error)
        })?
    }
}

#[tauri::command]
pub async fn preview_meeting_proposal(
    proposal: MeetingReprocessingProposal,
) -> Result<ProposalState, CommandError> {
    tauri::async_runtime::spawn_blocking(move || ProposalState::new(proposal))
        .await
        .map_err(|error| {
            CommandError::task_failed("Proposal preview worker stopped unexpectedly").because(error)
        })?
}

#[tauri::command]
pub async fn resolve_meeting_proposal(
    proposal: MeetingReprocessingProposal,
    expected_revision: String,
    index: usize,
    operations: Vec<CorrectionOperation>,
) -> Result<ProposalState, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let next = proposal.resolve(&expected_revision, index, operations)?;
        ProposalState::new(next)
    })
    .await
    .map_err(|error| {
        CommandError::task_failed("Proposal resolution worker stopped unexpectedly").because(error)
    })?
}

#[tauri::command]
pub async fn accept_meeting_proposal(
    proposal: MeetingReprocessingProposal,
    current_review: MeetingReview,
) -> Result<ReviewState, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        current_review.validate()?;
        ReviewState::from_review(proposal.accept(&current_review.revision)?)
    })
    .await
    .map_err(|error| {
        CommandError::task_failed("Proposal acceptance worker stopped unexpectedly").because(error)
    })?
}

#[tauri::command]
pub async fn save_meeting_proposal(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    proposal: MeetingReprocessingProposal,
) -> Result<bool, CommandError> {
    require_settings(&window)?;
    tauri::async_runtime::spawn_blocking(move || {
        proposal.validate()?;
        let output = serde_json::to_vec(&proposal).map_err(|error| {
            CommandError::InvalidInput("Could not serialize proposal".into()).because(error)
        })?;
        let selected = app
            .dialog()
            .file()
            .set_title("Save meeting proposal — choose a NEW file")
            .set_file_name("meeting-proposal.json")
            .blocking_save_file();
        let Some(selected) = selected else {
            return Ok(false);
        };
        let path = selected.into_path().map_err(|error| {
            CommandError::InvalidInput("Choose a local proposal destination".into()).because(error)
        })?;
        crate::meeting_jobs::write_new_export(&path, &output)?;
        Ok(true)
    })
    .await
    .map_err(|error| {
        CommandError::task_failed("Proposal save worker stopped unexpectedly").because(error)
    })?
}

#[tauri::command]
pub async fn open_meeting_proposal(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<Option<ProposalState>, CommandError> {
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
        let path = selected.into_path().map_err(|error| {
            CommandError::InvalidInput("Choose a local proposal".into()).because(error)
        })?;
        ProposalState::new(read_proposal_file(&path)?).map(Some)
    })
    .await
    .map_err(|error| {
        CommandError::task_failed("Proposal open worker stopped unexpectedly").because(error)
    })?
}

#[cfg(test)]
#[path = "meeting_reprocessing_commands_tests.rs"]
mod tests;
