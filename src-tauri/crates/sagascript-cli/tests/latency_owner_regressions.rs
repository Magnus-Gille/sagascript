use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use sagascript_cli::latency::summarize_reader;
use serde_json::{json, Value};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

struct InputFile(PathBuf);

impl InputFile {
    fn new(rows: &[Value]) -> Self {
        let id = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "sagascript-latency-owner-{}-{stamp}-{id}.jsonl",
            std::process::id()
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("create exclusive fixture");
        let owned = Self(path);
        for row in rows {
            writeln!(file, "{row}").expect("write fixture");
        }
        owned
    }
}

impl Drop for InputFile {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.0) {
            eprintln!("fixture cleanup failed: {:?}", error.kind());
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
        .arg(&file.0)
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
        .arg(&file.0)
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
