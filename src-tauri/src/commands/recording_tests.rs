use sagascript_core::recording::{Quality, RecordingState};
use serde_json::json;

use super::*;
use crate::contract;
use crate::recording_session::{RecordingFinished, RecordingFolderInfo};

#[test]
fn the_default_folder_is_stored_as_empty() {
    assert_eq!(recording_directory_to_store("").expect("empty"), "");
    assert_eq!(recording_directory_to_store("  \t ").expect("blank"), "");
}

#[test]
fn an_absolute_folder_is_stored_as_given_without_the_padding() {
    let dir = std::env::temp_dir();
    let given = format!("  {}  ", dir.display());
    assert_eq!(
        recording_directory_to_store(&given).expect("absolute"),
        dir.to_string_lossy()
    );
}

#[test]
fn a_leading_tilde_is_the_home_folder() {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    assert_eq!(
        recording_directory_to_store("~/Takes").expect("home"),
        home.join("Takes").to_string_lossy()
    );
}

#[test]
fn a_relative_folder_is_refused_with_a_message_without_the_path() {
    let error = recording_directory_to_store("Takes/today").expect_err("relative");
    assert_eq!(error.code(), "invalid_input");
    assert!(!error.message().contains("Takes"), "{}", error.message());
}

#[test]
fn the_unfinished_transcription_command_says_it_is_unavailable() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let error = runtime
        .block_on(transcribe_recording("x".into()))
        .expect_err("not yet");
    assert_eq!(error.code(), "unavailable");
}

// -- The shared fixtures the frontend parses (contracts/transport/) ------------

fn summary(
    state: RecordingState,
    quality: Quality,
    ended_reason: Option<&str>,
    needs_recovery: bool,
) -> RecordingSummary {
    let complete = state == RecordingState::Complete;
    RecordingSummary {
        folder_name: "2026-11-06 10.00 Interview [0d2c1f]".into(),
        recording_id: "0d2c1f3a-1111-4222-8333-444455556666".into(),
        title: "Interview".into(),
        state,
        recovered: false,
        started_at: "2026-11-06T10:00:00.000+01:00".into(),
        quality,
        duration_seconds: if complete { 3725.5 } else { 0.0 },
        bytes: 1_104_000_000,
        track_count: 1,
        gap_count: 1,
        gap_seconds: 0.25,
        ended_reason: ended_reason.map(str::to_string),
        codec: "flac".into(),
        sample_rate: if quality == Quality::Compact {
            16_000
        } else {
            48_000
        },
        channels: 1,
        needs_recovery,
        playable: complete,
    }
}

/// What `stop_recording_session` resolves with (and the rows of
/// `list_recordings`, which are its `summary`).
#[test]
fn transport_contract_stop_recording_session() {
    let finished = |summary, ended_early: Option<&str>| {
        contract::to_value(RecordingFinished {
            summary,
            ended_early: ended_early.map(str::to_string),
        })
    };
    let actual = json!({
        "stopped": finished(
            summary(RecordingState::Complete, Quality::Lossless, Some("stopped"), false),
            None,
        ),
        "device_changed": finished(
            summary(RecordingState::Complete, Quality::Compact, Some("device_changed"), false),
            Some("device_changed"),
        ),
        "interrupted": finished(
            summary(RecordingState::Recording, Quality::Lossless, None, true),
            None,
        ),
    });
    contract::assert_fixture("stop_recording_session.json", &actual);
}

/// `get_recording_folder_info`: an existing folder with warnings, one that is
/// not there yet on an unknown volume, and the default folder.
#[test]
fn transport_contract_get_recording_folder_info() {
    let info = |path: &str, exists, free_bytes, warnings: &[&str], is_default| {
        contract::to_value(RecordingFolderInfo {
            path: path.into(),
            exists,
            free_bytes,
            warnings: warnings.iter().map(|w| w.to_string()).collect(),
            is_default,
        })
    };
    let actual = json!({
        "existing": info(
            "/Volumes/Archive/Sagascript",
            true,
            Some(412_000_000_000_u64),
            &["exfat", "cloud_synced"],
            false,
        ),
        "not_created_yet": info("/Users/x/Music/Sagascript", false, None, &[], true),
    });
    contract::assert_fixture("get_recording_folder_info.json", &actual);
}
