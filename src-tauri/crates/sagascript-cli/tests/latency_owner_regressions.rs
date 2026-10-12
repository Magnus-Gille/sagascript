use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::process::Command;

use sagascript_cli::latency::summarize_reader;
use sagascript_testsupport::TempDir;
use serde_json::{json, Value};

/// A JSONL input file in its own temporary directory, removed on drop.
struct InputFile {
    path: PathBuf,
    _directory: TempDir,
}

impl InputFile {
    fn new(rows: &[Value]) -> Self {
        let directory = TempDir::new();
        let path = directory.join("events.jsonl");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("create exclusive fixture");
        for row in rows {
            writeln!(file, "{row}").expect("write fixture");
        }
        Self {
            path,
            _directory: directory,
        }
    }
}

fn phase(index: usize) -> Value {
    json!({"appSession":"test-app", "dictationSession":format!("dict-{index}"),
    "event":"dictation_phase_timings", "data":{
        "outcome":"success", "model":"Whisper Base", "language":"English",
        "modelWasWarm":true, "beamSize":1, "temperatureFallback":false,
        "vadEnabled":false, "pasteOutcome":"succeeded",
        "keyUpToCaptureStoppedMs":1, "modelLoadMs":0, "keyUpToModelReadyMs":2,
        "whisperMs":90, "keyUpToWhisperCompleteMs":92,
        "keyUpToPasteCompletedMs":100, "totalMs":100
    }})
}

fn capture(index: usize) -> Value {
    json!({"appSession":"test-app", "dictationSession":format!("dict-{index}"),
        "event":"capture_stopped", "data":{"audioDurationMs":1000}})
}

#[test]
fn budget_rejects_incomplete_group_even_with_enough_numeric_samples() {
    let mut rows = Vec::new();
    for index in 0..21 {
        rows.push(capture(index));
        let mut row = phase(index);
        if index == 20 {
            row["data"]["keyUpToPasteCompletedMs"] = Value::Null;
        }
        rows.push(row);
    }
    let file = InputFile::new(&rows);
    let output = Command::new(env!("CARGO_BIN_EXE_sagascript"))
        .args(["latency-report", "--input"])
        .arg(&file.path)
        .args([
            "--budget-length",
            "short",
            "--max-warm-p95-ms",
            "100",
            "--min-samples",
            "20",
        ])
        .output()
        .expect("run report");
    let report: Value = serde_json::from_slice(&output.stdout).expect("budget result JSON");
    assert!(!output.status.success(), "incomplete group must not pass");
    assert_eq!(report["budget"]["passed"], false);
}

#[test]
fn unknown_duration_is_not_a_selectable_successful_budget() {
    let file = InputFile::new(&(0..20).map(phase).collect::<Vec<_>>());
    let output = Command::new(env!("CARGO_BIN_EXE_sagascript"))
        .args(["latency-report", "--input"])
        .arg(&file.path)
        .args(["--budget-length", "unknown", "--max-warm-p95-ms", "100"])
        .output()
        .expect("run report");
    assert!(
        !output.status.success(),
        "unknown length must not be budget-eligible"
    );
}

#[test]
fn non_object_json_is_not_a_log_record() {
    assert!(summarize_reader(Cursor::new(b"[1,2,3]\n")).is_err());
}

#[test]
fn early_no_speech_cannot_claim_a_paste_outcome() {
    let row = json!({"appSession":"test-app", "dictationSession":"dict-empty",
        "event":"dictation_phase_timings", "data":{
            "outcome":"no_speech", "pasteOutcome":"succeeded",
            "keyUpToCaptureStoppedMs":1, "totalMs":1}});
    assert!(summarize_reader(Cursor::new(format!("{row}\n"))).is_err());
}
