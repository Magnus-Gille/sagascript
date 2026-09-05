use sagascript_cli::latency::summarize_reader;
use serde_json::{json, Value};
use std::io::{self, BufReader, Cursor, ErrorKind, Read};

const PRIVATE_SENTINEL: &str = "PRIVATE_LATENCY_LIMIT_SENTINEL_4c2e";

fn event(app_session: &str, dictation_session: &str, name: &str, data: Value) -> Value {
    json!({
        "ts": "2026-01-01T00:00:00.000Z",
        "level": "info",
        "appSession": app_session,
        "dictationSession": dictation_session,
        "category": "Performance",
        "event": name,
        "data": data,
    })
}

fn capture_event(app_session: &str, dictation_session: &str, audio_duration_ms: u64) -> Value {
    event(
        app_session,
        dictation_session,
        "capture_stopped",
        json!({
            "recordingDurationMs": audio_duration_ms,
            "audioDurationMs": audio_duration_ms,
            "audioSamples": audio_duration_ms * 16,
            "captureRequestToStreamPlayReturnMs": 1,
            "captureRequestToFirstAudioCallbackMs": 2,
            "deviceSampleRateHz": 16_000,
        }),
    )
}

fn success_phase_event(app_session: &str, dictation_session: &str) -> Value {
    event(
        app_session,
        dictation_session,
        "dictation_phase_timings",
        json!({
            "outcome": "success",
            "model": "Whisper Base",
            "language": "English",
            "modelWasWarm": true,
            "beamSize": 1,
            "temperatureFallback": false,
            "vadEnabled": false,
            "keyUpToCaptureStoppedMs": 10,
            "modelLoadMs": 0,
            "keyUpToModelReadyMs": 11,
            "whisperMs": 20,
            "keyUpToWhisperCompleteMs": 31,
            "pasteOutcome": "succeeded",
            "keyUpToPasteCompletedMs": 42,
            "totalMs": 42,
        }),
    )
}

fn jsonl(events: &[Value]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for event in events {
        bytes.extend(serde_json::to_vec(event).expect("serialize test event"));
        bytes.push(b'\n');
    }
    bytes
}

fn assert_error_without_payload<T>(result: Result<T, String>, payload: &str) {
    let error = result_error(result, "input should be rejected");
    assert!(!error.contains(payload), "error echoed rejected payload");
    assert!(
        !error.contains(PRIVATE_SENTINEL),
        "error echoed private sentinel"
    );
}

fn result_error<T>(result: Result<T, String>, message: &str) -> String {
    match result {
        Ok(_) => panic!("{message}"),
        Err(error) => error,
    }
}

#[test]
fn oversized_single_line_is_rejected_without_echoing_payload() {
    let mut padding = "x".repeat(1_048_600);
    padding.push_str(PRIVATE_SENTINEL);
    let line = json!({
        "event": "unrelated",
        "data": {"padding": padding},
    });
    let input = jsonl(&[line]);

    let result = summarize_reader(Cursor::new(input));
    let error = result_error(result, "single line over 1 MiB must be rejected");
    assert!(error.to_ascii_lowercase().contains("line 1"));
    assert!(!error.contains(PRIVATE_SENTINEL));
}

struct RepeatingLines {
    line: Vec<u8>,
    remaining: usize,
    offset: usize,
}

impl RepeatingLines {
    fn new(line: Vec<u8>, remaining: usize) -> Self {
        Self {
            line,
            remaining,
            offset: 0,
        }
    }
}

impl Read for RepeatingLines {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        let amount = (self.line.len() - self.offset).min(buffer.len());
        buffer[..amount].copy_from_slice(&self.line[self.offset..self.offset + amount]);
        self.offset += amount;
        if self.offset == self.line.len() {
            self.offset = 0;
            self.remaining -= 1;
        }
        Ok(amount)
    }
}

#[test]
fn oversized_total_input_is_rejected_while_streaming_valid_unrelated_jsonl() {
    let mut line = serde_json::to_vec(&json!({
        "event": "unrelated",
        "data": {"private": PRIVATE_SENTINEL},
    }))
    .expect("serialize repeated event");
    line.push(b'\n');
    let count = (32 * 1024 * 1024 / line.len()) + 1;
    let reader = BufReader::new(RepeatingLines::new(line, count));

    let result = summarize_reader(reader);
    let error = result_error(result, "total input over 32 MiB must be rejected");
    assert!(!error.contains(PRIVATE_SENTINEL));
}

struct FailingReader;

impl Read for FailingReader {
    fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(
            ErrorKind::PermissionDenied,
            "PRIVATE_IO_ERROR_PAYLOAD",
        ))
    }
}

#[test]
fn read_io_error_preserves_line_and_kind_without_raw_error_payload() {
    let reader = BufReader::new(FailingReader);
    let result = summarize_reader(reader);
    let error = result_error(result, "reader error must be returned");
    let lower = error.to_ascii_lowercase();
    assert!(
        lower.contains("line 1"),
        "error omitted input line: {error}"
    );
    assert!(
        lower.contains("permission") || lower.contains("denied"),
        "error omitted I/O kind: {error}"
    );
    assert!(!error.contains("PRIVATE_IO_ERROR_PAYLOAD"));
}

#[test]
fn malformed_shapes_and_unknown_model_or_language_fail_without_echoing_values() {
    let malformed_root = json!([]);
    assert_error_without_payload(
        summarize_reader(Cursor::new(jsonl(&[malformed_root]))),
        "[]",
    );

    let malformed_data = json!({
        "appSession": "app-malformed-data",
        "dictationSession": "dict-malformed-data",
        "event": "dictation_phase_timings",
        "data": [],
    });
    assert_error_without_payload(
        summarize_reader(Cursor::new(jsonl(&[malformed_data]))),
        "[]",
    );

    for (field, value) in [
        ("model", "PRIVATE_UNKNOWN_MODEL_SENTINEL"),
        ("language", "PRIVATE_UNKNOWN_LANGUAGE_SENTINEL"),
    ] {
        let app = "app-unknown-value";
        let dict = "dict-unknown-value";
        let mut phase = success_phase_event(app, dict);
        phase["data"][field] = json!(value);
        let input = jsonl(&[capture_event(app, dict, 1_000), phase]);
        assert_error_without_payload(summarize_reader(Cursor::new(input)), value);
    }
}

#[test]
fn success_with_missing_required_configuration_is_rejected() {
    let app = "app-missing-config";
    let dict = "dict-missing-config";
    let mut phase = success_phase_event(app, dict);
    phase["data"]
        .as_object_mut()
        .expect("phase data object")
        .remove("model");
    let input = jsonl(&[capture_event(app, dict, 1_000), phase]);

    let result = summarize_reader(Cursor::new(input));
    assert!(result.is_err(), "success without model must be rejected");
}

fn contains_object_with_null_model_and_language(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            let has_null_metadata = object.get("model") == Some(&Value::Null)
                && object.get("language") == Some(&Value::Null);
            has_null_metadata
                || object
                    .values()
                    .any(contains_object_with_null_model_and_language)
        }
        Value::Array(values) => values
            .iter()
            .any(contains_object_with_null_model_and_language),
        _ => false,
    }
}

fn contains_numeric_metric_summary(value: &Value, metric: &str) -> bool {
    match value {
        Value::Object(object) => {
            let metric_is_numeric = object
                .get(metric)
                .and_then(Value::as_object)
                .and_then(|metric| metric.get("p50Ms"))
                .is_some_and(Value::is_number);
            metric_is_numeric
                || object
                    .values()
                    .any(|child| contains_numeric_metric_summary(child, metric))
        }
        Value::Array(values) => values
            .iter()
            .any(|child| contains_numeric_metric_summary(child, metric)),
        _ => false,
    }
}

#[test]
fn early_no_speech_without_config_is_accepted_with_null_metadata() {
    let app = "app-legacy-no-speech";
    let dict = "dict-legacy-no-speech";
    let phase = event(
        app,
        dict,
        "dictation_phase_timings",
        json!({
            "outcome": "no_speech",
            "keyUpToCaptureStoppedMs": 12,
            "keyUpToModelReadyMs": null,
            "modelLoadMs": null,
            "whisperMs": null,
            "keyUpToPasteCompletedMs": null,
            "totalMs": 15,
        }),
    );
    let input = jsonl(&[capture_event(app, dict, 1_000), phase]);
    let report = summarize_reader(Cursor::new(input)).expect("legacy no-speech record is valid");
    let value = serde_json::to_value(report).expect("serialize report");

    let reporter_version = value["reporterVersion"]
        .as_str()
        .expect("reporterVersion string");
    assert!(!reporter_version.is_empty());
    assert_eq!(value["sourceBuild"], Value::Null);
    assert!(contains_object_with_null_model_and_language(&value));
    assert!(contains_numeric_metric_summary(&value, "totalMs"));
    assert!(contains_numeric_metric_summary(
        &value,
        "keyUpToCaptureStoppedMs"
    ));
}
