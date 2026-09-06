use std::io::Cursor;

use sagascript_cli::latency::summarize_reader;
use serde_json::{json, Value};

fn envelope(app_session: &str, dictation_session: &str, event: &str, data: Value) -> Value {
    json!({
        "ts": "2026-09-05T12:00:00Z",
        "level": "info",
        "appSession": app_session,
        "dictationSession": dictation_session,
        "category": "Performance",
        "event": event,
        "data": data,
    })
}

fn capture(app_session: &str, dictation_session: &str, audio_duration_ms: u64) -> Value {
    envelope(
        app_session,
        dictation_session,
        "capture_stopped",
        json!({
            "recordingDurationMs": audio_duration_ms + 100,
            "audioDurationMs": audio_duration_ms,
            "audioSamples": audio_duration_ms * 16,
            "captureRequestToStreamPlayReturnMs": 5,
            "captureRequestToFirstAudioCallbackMs": 7,
            "deviceSampleRateHz": 16000,
        }),
    )
}

fn phase(app_session: &str, dictation_session: &str, model_was_warm: bool, total_ms: u64) -> Value {
    envelope(
        app_session,
        dictation_session,
        "dictation_phase_timings",
        json!({
            "outcome": "success",
            "model": "Whisper Base",
            "language": "English",
            "modelWasWarm": model_was_warm,
            "beamSize": 5,
            "temperatureFallback": false,
            "vadEnabled": true,
            "keyUpToCaptureStoppedMs": total_ms,
            "modelLoadMs": total_ms,
            "keyUpToModelReadyMs": total_ms,
            "whisperMs": total_ms,
            "keyUpToWhisperCompleteMs": total_ms,
            "keyUpToPasteCompletedMs": total_ms,
            "totalMs": total_ms,
            "pasteOutcome": "succeeded",
        }),
    )
}

fn summarize_rows(rows: &[Value]) -> Result<Value, String> {
    let mut input = String::new();
    for row in rows {
        input.push_str(&serde_json::to_string(row).map_err(|error| error.to_string())?);
        input.push('\n');
    }
    summarize_text(&input)
}

fn summarize_text(input: &str) -> Result<Value, String> {
    let report = summarize_reader(Cursor::new(input.as_bytes()))?;
    serde_json::to_value(report).map_err(|error| error.to_string())
}

fn group<'a>(report: &'a Value, warm: bool, length_bucket: &str) -> &'a Value {
    report["groups"]
        .as_array()
        .expect("groups should be an array")
        .iter()
        .find(|candidate| {
            candidate["model"] == "base"
                && candidate["language"] == "en"
                && candidate["modelWasWarm"] == warm
                && candidate["lengthBucket"] == length_bucket
        })
        .unwrap_or_else(|| panic!("missing group {warm}/{length_bucket}: {report}"))
}

fn phase_metric<'a>(group: &'a Value, field: &str) -> &'a Value {
    &group["phases"][field]
}

#[test]
fn reports_exact_schema_and_nearest_rank_statistics_per_warm_cohort() {
    let mut rows = Vec::new();
    for index in 1..=20u64 {
        let session = format!("cold-{index}");
        rows.push(capture("app-a", &session, 5_000));
        rows.push(phase("app-a", &session, false, index));
    }
    rows.push(capture("app-a", "warm-1", 5_000));
    rows.push(phase("app-a", "warm-1", true, 999));

    let report = summarize_rows(&rows).expect("synthetic report should parse");
    assert_eq!(report["schemaVersion"], 1);
    assert_eq!(
        report["metricBoundary"],
        "paste_call_completion_not_visible_text"
    );
    assert_eq!(report["inputRecords"], 42);
    assert_eq!(report["phaseRecords"], 21);

    let cold = group(&report, false, "short");
    assert_eq!(cold["samples"], 20);
    assert_eq!(phase_metric(cold, "totalMs")["count"], 20);
    assert_eq!(phase_metric(cold, "totalMs")["numericCount"], 20);
    assert_eq!(phase_metric(cold, "totalMs")["p50Ms"], 10.0);
    assert_eq!(phase_metric(cold, "totalMs")["p95Ms"], 19.0);

    let warm = group(&report, true, "short");
    assert_eq!(warm["samples"], 1);
    assert_eq!(phase_metric(warm, "totalMs")["p50Ms"], 999.0);
    assert_eq!(phase_metric(warm, "totalMs")["p95Ms"], 999.0);
}

#[test]
fn keeps_audio_duration_boundaries_in_separate_length_buckets() {
    let rows = [("short", 5_000), ("medium", 15_000), ("long", 15_001)]
        .into_iter()
        .enumerate()
        .flat_map(|(index, (expected_bucket, duration))| {
            let session = format!("boundary-{index}");
            let rows = [
                capture("app-boundary", &session, duration),
                phase("app-boundary", &session, false, duration),
            ];
            assert!(!expected_bucket.is_empty());
            rows
        })
        .collect::<Vec<_>>();

    let report = summarize_rows(&rows).expect("boundary fixture should parse");
    for bucket in ["short", "medium", "long"] {
        assert_eq!(group(&report, false, bucket)["samples"], 1);
    }
}

#[test]
fn joins_by_both_session_keys_and_is_input_order_invariant() {
    let rows = vec![
        capture("app-a", "dict-1", 5_000),
        phase("app-b", "dict-1", false, 10),
        capture("app-b", "dict-2", 15_000),
        phase("app-b", "dict-2", false, 20),
    ];
    let forward = summarize_rows(&rows).expect("forward fixture should parse");
    let reverse_rows = rows.into_iter().rev().collect::<Vec<_>>();
    let reverse = summarize_rows(&reverse_rows).expect("reverse fixture should parse");

    assert_eq!(forward, reverse);
    assert_eq!(forward["captureWithoutPhases"], 1);
    assert_eq!(group(&forward, false, "medium")["samples"], 1);
    assert_eq!(group(&forward, false, "unknown")["samples"], 1);
}

#[test]
fn counts_unmatched_capture_and_unknown_phase_without_inventing_zeroes() {
    let rows = vec![
        capture("app-a", "capture-only", 1_000),
        phase("app-b", "phase-only", false, 25),
    ];

    let report = summarize_rows(&rows).expect("unmatched fixture should parse");
    assert_eq!(report["captureWithoutPhases"], 1);
    let unknown = group(&report, false, "unknown");
    assert_eq!(unknown["samples"], 1);
    assert_eq!(phase_metric(unknown, "totalMs")["numericCount"], 1);
    assert_eq!(phase_metric(unknown, "totalMs")["nullCount"], 0);
    assert_eq!(phase_metric(unknown, "totalMs")["missingCount"], 0);
}

#[test]
fn keeps_null_and_missing_metrics_distinct() {
    let mut row = phase("app-a", "null-missing", false, 25);
    let data = row["data"].as_object_mut().expect("phase data object");
    data.insert("modelLoadMs".to_owned(), Value::Null);
    data.remove("whisperMs");

    let report = summarize_rows(&[capture("app-a", "null-missing", 1_000), row])
        .expect("null/missing fixture should parse");
    let metrics = group(&report, false, "short");

    let null_metric = phase_metric(metrics, "modelLoadMs");
    assert_eq!(null_metric["count"], 1);
    assert_eq!(null_metric["numericCount"], 0);
    assert_eq!(null_metric["nullCount"], 1);
    assert_eq!(null_metric["missingCount"], 0);
    assert!(null_metric["p50Ms"].is_null());
    assert!(null_metric["p95Ms"].is_null());

    let missing_metric = phase_metric(metrics, "whisperMs");
    assert_eq!(missing_metric["count"], 1);
    assert_eq!(missing_metric["numericCount"], 0);
    assert_eq!(missing_metric["nullCount"], 0);
    assert_eq!(missing_metric["missingCount"], 1);
    assert!(missing_metric["p50Ms"].is_null());
    assert!(missing_metric["p95Ms"].is_null());
}

#[test]
fn rejects_duplicate_relevant_events_without_echoing_session_ids() {
    let first = phase("SECRET_APP_MARKER", "SECRET_DICT_MARKER", false, 25);
    let duplicate = first.clone();
    let error = summarize_rows(&[
        capture("SECRET_APP_MARKER", "SECRET_DICT_MARKER", 1_000),
        first,
        duplicate,
    ])
    .expect_err("duplicate phase events must fail");

    assert!(!error.contains("SECRET_APP_MARKER"));
    assert!(!error.contains("SECRET_DICT_MARKER"));
}

#[test]
fn rejects_malformed_negative_and_wrong_type_relevant_data_without_echoing_values() {
    let malformed =
        "{\"event\":\"dictation_phase_timings\",\"data\":{\"totalMs\":\"SECRET_MARKER\"}";
    let malformed_error = summarize_text(malformed).expect_err("malformed JSON must fail");
    assert!(!malformed_error.contains("SECRET_MARKER"));

    let mut negative = phase("SECRET_APP_MARKER", "SECRET_DICT_MARKER", false, 25);
    negative["data"]["totalMs"] = json!(-1);
    let negative_error = summarize_rows(&[
        capture("SECRET_APP_MARKER", "SECRET_DICT_MARKER", 1_000),
        negative,
    ])
    .expect_err("negative timing must fail");
    assert!(!negative_error.contains("SECRET_APP_MARKER"));
    assert!(!negative_error.contains("SECRET_DICT_MARKER"));

    let mut wrong_type = phase("app-a", "wrong-type", false, 25);
    wrong_type["data"]["totalMs"] = json!("SECRET_MARKER");
    let wrong_type_error = summarize_rows(&[capture("app-a", "wrong-type", 1_000), wrong_type])
        .expect_err("wrong timing type must fail");
    assert!(!wrong_type_error.contains("SECRET_MARKER"));
}

#[test]
fn ignores_unknown_events_and_fields_without_leaking_sensitive_values() {
    let mut known = phase("app-a", "known", false, 25);
    known["data"]["transcript"] = json!("TRANSCRIPT_SECRET_MARKER");
    known["data"]["path"] = json!("/private/SECRET_PATH_MARKER");
    known["data"]["unknownField"] = json!("UNKNOWN_SECRET_MARKER");
    let unknown = json!({
        "ts": "2026-09-05T12:00:00Z",
        "level": "info",
        "appSession": "UNKNOWN_SESSION_MARKER",
        "dictationSession": "UNKNOWN_DICTATION_MARKER",
        "event": "unrelated_event",
        "data": {
            "transcript": "UNKNOWN_TRANSCRIPT_MARKER",
            "path": "/private/UNKNOWN_PATH_MARKER",
            "secret": "UNKNOWN_SECRET_MARKER"
        }
    });

    let report = summarize_rows(&[capture("app-a", "known", 1_000), known, unknown])
        .expect("unknown event/field fixture should parse");
    let serialized = report.to_string();
    for marker in [
        "TRANSCRIPT_SECRET_MARKER",
        "SECRET_PATH_MARKER",
        "UNKNOWN_SECRET_MARKER",
        "UNKNOWN_SESSION_MARKER",
        "UNKNOWN_DICTATION_MARKER",
        "UNKNOWN_TRANSCRIPT_MARKER",
        "UNKNOWN_PATH_MARKER",
    ] {
        assert!(!serialized.contains(marker), "report leaked {marker}");
    }
}
