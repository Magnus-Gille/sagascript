use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    input: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root) {
            eprintln!("latency fixture cleanup failed: {:?}", error.kind());
        }
    }
}

fn fixture(entries: Vec<Value>) -> Fixture {
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let root = std::env::temp_dir().join(format!(
        "sagascript-latency-budget-{}-{timestamp}-{id}",
        std::process::id(),
    ));
    fs::create_dir(&root)
        .unwrap_or_else(|error| panic!("create fixture directory: {:?}", error.kind()));
    let input = root.join("events.jsonl");
    let fixture = Fixture { root, input };
    let contents = entries
        .into_iter()
        .map(|entry| serde_json::to_string(&entry).expect("serialize fixture event"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&fixture.input, format!("{contents}\n"))
        .unwrap_or_else(|error| panic!("write fixture JSONL: {:?}", error.kind()));
    fixture
}

fn sample_events(
    entries: &mut Vec<Value>,
    index: usize,
    audio_duration_ms: u64,
    latency_ms: u64,
    warm: bool,
    paste_outcome: &str,
) {
    let app_session = format!("app-budget-{index}");
    let dictation_session = format!("dict-budget-{index}");
    entries.push(json!({
        "ts": format!("2026-01-01T00:00:{index:02}Z"),
        "level": "info",
        "appSession": app_session,
        "dictationSession": dictation_session,
        "category": "Performance",
        "event": "capture_stopped",
        "data": {
            "recordingDurationMs": audio_duration_ms,
            "audioDurationMs": audio_duration_ms,
            "audioSamples": audio_duration_ms * 16,
            "captureRequestToStreamPlayReturnMs": 1,
            "captureRequestToFirstAudioCallbackMs": 2,
            "deviceSampleRateHz": 16_000,
        },
    }));
    entries.push(json!({
        "ts": format!("2026-01-01T00:01:{index:02}Z"),
        "level": "info",
        "appSession": app_session,
        "dictationSession": dictation_session,
        "category": "Performance",
        "event": "dictation_phase_timings",
        "data": {
            "outcome": "success",
            "model": "Whisper Base",
            "language": "English",
            "modelWasWarm": warm,
            "beamSize": 1,
            "temperatureFallback": false,
            "vadEnabled": false,
            "keyUpToCaptureStoppedMs": 10,
            "modelLoadMs": 0,
            "keyUpToModelReadyMs": 11,
            "whisperMs": latency_ms.saturating_sub(20),
            "keyUpToWhisperCompleteMs": latency_ms.saturating_sub(5),
            "pasteOutcome": paste_outcome,
            "keyUpToPasteCompletedMs": if paste_outcome == "succeeded" {
                json!(latency_ms)
            } else {
                Value::Null
            },
            "totalMs": latency_ms,
        },
    }));
}

fn run_report(input: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sagascript"))
        .arg("latency-report")
        .arg("--input")
        .arg(input)
        .args(args)
        .output()
        .expect("run sagascript latency-report")
}

fn parse_report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "latency-report did not emit JSON: {error}; stderr={}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn assert_budget(output: &Output, expected: bool) -> Value {
    let report = parse_report(output);
    assert_eq!(report["budget"]["passed"], expected, "report={report}");
    report
}

#[test]
fn budget_passes_twenty_warm_successful_short_samples_at_exact_threshold() {
    let mut entries = Vec::new();
    for index in 0..20 {
        sample_events(&mut entries, index, 1_000, 100, true, "succeeded");
    }
    let fixture = fixture(entries);
    let output = run_report(
        &fixture.input,
        &["--budget-length", "short", "--max-warm-p95-ms", "100"],
    );

    assert!(output.status.success(), "stderr={:?}", output.stderr);
    assert_budget(&output, true);
}

#[test]
fn budget_failure_still_emits_json_when_p95_is_over_threshold() {
    let mut entries = Vec::new();
    for index in 0..18 {
        sample_events(&mut entries, index, 1_000, 100, true, "succeeded");
    }
    for index in 18..20 {
        sample_events(&mut entries, index, 1_000, 200, true, "succeeded");
    }
    let fixture = fixture(entries);
    let output = run_report(
        &fixture.input,
        &[
            "--budget-length",
            "short",
            "--max-warm-p95-ms",
            "199",
            "--min-samples",
            "20",
        ],
    );

    assert!(!output.status.success(), "budget failure must be nonzero");
    assert_budget(&output, false);
}

#[test]
fn budget_fails_when_samples_are_below_the_requested_minimum() {
    let mut entries = Vec::new();
    for index in 0..19 {
        sample_events(&mut entries, index, 1_000, 100, true, "succeeded");
    }
    let fixture = fixture(entries);
    let output = run_report(
        &fixture.input,
        &["--budget-length", "short", "--max-warm-p95-ms", "100"],
    );

    assert!(
        !output.status.success(),
        "insufficient samples must be nonzero"
    );
    assert_budget(&output, false);
}

#[test]
fn budget_fails_with_no_eligible_warm_samples() {
    let mut entries = Vec::new();
    for index in 0..20 {
        sample_events(&mut entries, index, 1_000, 100, false, "succeeded");
    }
    let fixture = fixture(entries);
    let output = run_report(
        &fixture.input,
        &["--budget-length", "short", "--max-warm-p95-ms", "100"],
    );

    assert!(!output.status.success(), "cold-only budget must be nonzero");
    assert_budget(&output, false);
}

#[test]
fn selected_length_does_not_combine_other_length_samples() {
    let mut entries = Vec::new();
    for index in 0..10 {
        sample_events(&mut entries, index, 1_000, 100, true, "succeeded");
    }
    for index in 10..30 {
        sample_events(&mut entries, index, 6_000, 100, true, "succeeded");
    }
    let fixture = fixture(entries);
    let output = run_report(
        &fixture.input,
        &[
            "--budget-length",
            "short",
            "--max-warm-p95-ms",
            "100",
            "--min-samples",
            "20",
        ],
    );

    assert!(
        !output.status.success(),
        "short cohort must remain under min samples"
    );
    assert_budget(&output, false);
}

#[test]
fn slow_configuration_group_cannot_be_hidden_by_fast_group() {
    let mut entries = Vec::new();
    for index in 0..20 {
        sample_events(&mut entries, index, 1_000, 100, true, "succeeded");
    }
    sample_events(&mut entries, 20, 1_000, 200, true, "succeeded");
    let slow_phase = entries.last_mut().expect("slow phase event");
    slow_phase["data"]["model"] = json!("KB-Whisper Base");
    slow_phase["data"]["beamSize"] = json!(2);

    let fixture = fixture(entries);
    let output = run_report(
        &fixture.input,
        &[
            "--budget-length",
            "short",
            "--max-warm-p95-ms",
            "150",
            "--min-samples",
            "1",
        ],
    );

    assert!(
        !output.status.success(),
        "slow configuration must fail the budget"
    );
    assert_budget(&output, false);
}

#[test]
fn incomplete_or_unsuccessful_paste_samples_cannot_pass_budget() {
    let mut entries = Vec::new();
    for index in 0..19 {
        sample_events(&mut entries, index, 1_000, 100, true, "succeeded");
    }

    // A failed paste has a numeric-looking latency but must not count as a
    // successful sample toward the requested minimum.
    sample_events(&mut entries, 19, 1_000, 100, true, "failed");
    entries.last_mut().expect("failed paste phase")["data"]["keyUpToPasteCompletedMs"] = json!(100);

    // A successful phase with its endpoint metric missing is incomplete and
    // must fail closed rather than being treated as a fast zero/omitted row.
    sample_events(&mut entries, 20, 1_000, 100, true, "succeeded");
    entries.last_mut().expect("incomplete phase")["data"]
        .as_object_mut()
        .expect("phase data object")
        .remove("keyUpToPasteCompletedMs");

    let fixture = fixture(entries);
    let output = run_report(
        &fixture.input,
        &[
            "--budget-length",
            "short",
            "--max-warm-p95-ms",
            "100",
            "--min-samples",
            "20",
        ],
    );

    assert!(
        !output.status.success(),
        "incomplete eligible metrics must fail"
    );
    assert_budget(&output, false);
}

#[test]
fn invalid_budget_options_fail_closed() {
    let mut entries = Vec::new();
    sample_events(&mut entries, 0, 1_000, 100, true, "succeeded");
    let fixture = fixture(entries);
    let invalid_options = [
        vec!["--budget-length", "short"],
        vec!["--max-warm-p95-ms", "100"],
        vec!["--budget-length", "short", "--max-warm-p95-ms=-1"],
        vec!["--budget-length", "short", "--max-warm-p95-ms", "NaN"],
        vec![
            "--budget-length",
            "short",
            "--max-warm-p95-ms",
            "100",
            "--min-samples",
            "0",
        ],
    ];

    for options in invalid_options {
        let output = run_report(&fixture.input, &options);
        assert!(!output.status.success(), "options should fail: {options:?}");
    }
}

#[test]
fn report_only_mode_works_without_budget_flags() {
    let mut entries = Vec::new();
    sample_events(&mut entries, 0, 1_000, 100, true, "succeeded");
    let fixture = fixture(entries);
    let output = run_report(&fixture.input, &[]);

    assert!(output.status.success(), "stderr={:?}", output.stderr);
    let report = parse_report(&output);
    assert!(report.is_object());
}

#[test]
fn missing_input_fails_without_echoing_private_path() {
    let sentinel = "PRIVATE_LATENCY_SENTINEL_9f0c";
    let missing = std::env::temp_dir()
        .join(sentinel)
        .join("missing-events.jsonl");
    let output = run_report(&missing, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(!stdout.contains(sentinel), "stdout echoed private sentinel");
    assert!(!stderr.contains(sentinel), "stderr echoed private sentinel");
}
