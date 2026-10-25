//! Desktop lifetime/permission boundary for the shared live meeting session.
use crate::{
    command_error::CommandError,
    commands::{lock_controller, SharedController},
    meeting_jobs::ExportFormat,
};
use sagascript_core::{
    meeting::live::{session::Session, state::Snapshot as LiveSnapshot},
    sync::lock_or_recover,
};
use std::sync::{Arc, Mutex};
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

#[derive(Default)]
pub struct LiveMeetings(Mutex<Option<Arc<Session>>>);
pub type SharedLiveMeetings = Arc<LiveMeetings>;

impl LiveMeetings {
    /// A temporary session needs an explicit export/discard decision before app exit.
    pub fn has_pending_session(&self) -> bool {
        lock_or_recover(&self.0, "live.jobs").is_some()
    }
    pub fn shutdown(&self) {
        if let Some(session) = lock_or_recover(&self.0, "live.jobs").as_ref() {
            session.shutdown();
        }
    }
}

fn get(jobs: &LiveMeetings, id: &str) -> Result<Arc<Session>, CommandError> {
    lock_or_recover(&jobs.0, "live.jobs")
        .as_ref()
        .filter(|session| session.id() == id)
        .cloned()
        .ok_or_else(|| CommandError::NotFound("This live meeting is no longer available.".into()))
}

#[tauri::command]
pub fn live_meeting_supported() -> Result<bool, CommandError> {
    Ok(Session::supported())
}

#[tauri::command]
pub fn begin_live_meeting(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    jobs: State<'_, SharedLiveMeetings>,
    controller: State<'_, SharedController>,
) -> Result<(), CommandError> {
    crate::meeting_review_commands::require_settings(&window)?;
    let mut slot = lock_or_recover(&jobs.0, "live.jobs");
    if slot.is_some() {
        return Err(CommandError::Busy(
            "Save or discard the previous live meeting before starting another.".into(),
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let mut ctrl = lock_controller(&controller);
    if !ctrl.begin_meeting_job(&id) {
        return Err(CommandError::Busy(
            "Finish the current dictation or meeting first.".into(),
        ));
    }
    let session = match Session::start_with_id(ctrl.settings().clone(), id.clone()) {
        Ok(session) => session,
        Err(error) => {
            ctrl.finish_meeting_job(&id);
            return Err(CommandError::Unavailable(error));
        }
    };
    *slot = Some(session.clone());
    drop(ctrl);
    drop(slot);
    tauri::async_runtime::spawn(async move {
        while !session.is_done() {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let jobs = app.state::<SharedLiveMeetings>();
        let slot = lock_or_recover(&jobs.0, "live.jobs");
        if !slot.as_ref().is_some_and(|current| current.id() == id) {
            return;
        }
        let controller = app.state::<SharedController>();
        let mut ctrl = lock_controller(&controller);
        if let Err(error) = ctrl.set_update_result_pending(&format!("live:{id}"), true) {
            tracing::warn!(%error,"Could not track pending live meeting");
        }
        ctrl.finish_meeting_job(&id);
    });
    Ok(())
}

#[tauri::command]
pub fn get_live_meeting(
    window: tauri::WebviewWindow,
    jobs: State<'_, SharedLiveMeetings>,
) -> Result<Option<LiveSnapshot>, CommandError> {
    crate::meeting_review_commands::require_settings(&window)?;
    Ok(lock_or_recover(&jobs.0, "live.jobs")
        .as_ref()
        .map(|session| session.snapshot()))
}
#[tauri::command]
pub fn stop_live_meeting(
    window: tauri::WebviewWindow,
    jobs: State<'_, SharedLiveMeetings>,
    session_id: String,
) -> Result<(), CommandError> {
    crate::meeting_review_commands::require_settings(&window)?;
    get(&jobs, &session_id)?.stop();
    Ok(())
}
#[tauri::command]
pub fn discard_live_meeting(
    window: tauri::WebviewWindow,
    jobs: State<'_, SharedLiveMeetings>,
    controller: State<'_, SharedController>,
    session_id: String,
) -> Result<(), CommandError> {
    crate::meeting_review_commands::require_settings(&window)?;
    let mut slot = lock_or_recover(&jobs.0, "live.jobs");
    let session = slot
        .as_ref()
        .filter(|session| session.id() == session_id)
        .ok_or_else(|| {
            CommandError::NotFound("This live meeting is no longer available.".into())
        })?;
    session.discard().map_err(CommandError::Busy)?;
    // Lock order is jobs -> controller, identical to begin. Do not release another session's lease.
    let mut ctrl = lock_controller(&controller);
    ctrl.set_update_result_pending(&format!("live:{session_id}"), false)?;
    ctrl.finish_meeting_job(&session_id);
    *slot = None;
    Ok(())
}
#[tauri::command]
pub async fn save_live_meeting_text(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    jobs: State<'_, SharedLiveMeetings>,
    session_id: String,
    format: ExportFormat,
) -> Result<bool, CommandError> {
    crate::meeting_review_commands::require_settings(&window)?;
    let session = get(&jobs, &session_id)?;
    let (format, extension) = format.details();
    tauri::async_runtime::spawn_blocking(move || {
        let selection = app
            .dialog()
            .file()
            .set_title("Save live transcript — choose a new file")
            .set_file_name(format!("live-meeting.{extension}"))
            .add_filter("Transcript", &[extension])
            .blocking_save_file();
        let Some(selection) = selection else {
            return Ok(false);
        };
        let path = selection
            .into_path()
            .map_err(|e| CommandError::InvalidInput(e.to_string()))?;
        let published = session
            .export_text(&path, format)
            .map_err(CommandError::Io)?;
        if published.leftover.is_some() {
            tracing::warn!("Live transcript saved with an extra staging file in the chosen folder");
        }
        Ok(true)
    })
    .await
    .map_err(|e| CommandError::task_failed(format!("Live export worker failed: {e}")))?
}
#[tauri::command]
pub async fn save_live_meeting_audio(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    jobs: State<'_, SharedLiveMeetings>,
    session_id: String,
) -> Result<bool, CommandError> {
    crate::meeting_review_commands::require_settings(&window)?;
    let session = get(&jobs, &session_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let selection = app
            .dialog()
            .file()
            .set_title("Save two-track audio — choose a new file")
            .set_file_name("live-meeting.wav")
            .add_filter("WAV audio", &["wav"])
            .blocking_save_file();
        let Some(selection) = selection else {
            return Ok(false);
        };
        let path = selection
            .into_path()
            .map_err(|e| CommandError::InvalidInput(e.to_string()))?;
        let published = session.export_audio(&path).map_err(CommandError::Io)?;
        if published.leftover.is_some() {
            tracing::warn!("Live audio saved with an extra staging file in the chosen folder");
        }
        Ok(true)
    })
    .await
    .map_err(|e| CommandError::task_failed(format!("Live audio export worker failed: {e}")))?
}

#[cfg(test)]
mod tests {
    use sagascript_core::meeting::live::{
        document::{Gap, LiveSegment, SegmentStatus, Source},
        state::State,
    };
    #[test]
    fn transport_contract_live_meeting() {
        let mut state = State::new("contract-live".into()).unwrap();
        let starting = state.snapshot();
        state.started().unwrap();
        state.advance_by(1500).unwrap();
        state
            .set_source_connected(Source::Microphone, true)
            .unwrap();
        state
            .set_source_signal_level(Source::Microphone, 0.2)
            .unwrap();
        state
            .set_source_last_audio_ms(Source::Microphone, Some(1400))
            .unwrap();
        state
            .set_source_last_text_ms(Source::Microphone, Some(900))
            .unwrap();
        state
            .set_source_text_delay_ms(Source::Microphone, 600)
            .unwrap();
        state
            .upsert(LiveSegment {
                id: "0-1".into(),
                source: Source::Microphone,
                start_ms: 100,
                end_ms: 900,
                text: "Hej mötet".into(),
                status: SegmentStatus::Preview,
                revision: 1,
            })
            .unwrap();
        let running = state.snapshot();
        state
            .append_gap(Gap {
                source: Source::System,
                start_ms: 500,
                end_ms: 800,
                reason: "Capture interrupted".into(),
            })
            .unwrap();
        state
            .finish_failed("System audio interrupted", true)
            .unwrap();
        crate::contract::assert_fixture(
            "get_live_meeting.json",
            &serde_json::json!({"starting":starting,"running":running,"failed":state.snapshot()}),
        );
    }
}
