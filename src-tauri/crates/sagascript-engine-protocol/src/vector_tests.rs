//! The golden transcripts (`src-tauri/engine-host/test-vectors/protocol/`) decoded strictly with
//! this crate's types. The hosts replay the same transcripts (Swift `ProtocolServerTests`, the ONNX
//! host's `server/tests.rs`), so a field renamed on either side fails a test.

use super::golden::{concrete, load_transcripts, matches, FakeEngine, Step, Transcript};
use super::*;
use serde_json::json;
use std::collections::BTreeSet;

/// Result fields the spec marks optional, as `op:path` (`[]` = every array item). Every other
/// field of a result must be required by the types.
const OPTIONAL: [&str; 9] = [
    "hello:host.git_sha",
    "hello:host.build_dirty",
    "hello:capabilities.min_macos",
    "hello:capabilities.context_biasing",
    "transcribe_window:tokens[].confidence",
    "transcribe_window:timings.boost_us",
    "transcribe_window:boost_active",
    "transcribe_window:boost_reason",
    "status:model_id",
];

const OPS: [&str; 8] = [
    "hello",
    "load",
    "transcribe_window",
    "cancel",
    "status",
    "ping",
    "unload",
    "shutdown",
];

fn transcripts() -> Vec<Transcript> {
    load_transcripts().unwrap_or_else(|e| panic!("{e}"))
}

/// Each `expect` step with the op of the request it answers (`None` for id 0).
fn answers(t: &Transcript) -> Vec<(usize, Value, Option<String>)> {
    t.steps
        .iter()
        .enumerate()
        .filter_map(|(index, step)| match step {
            Step::Expect(expected) => {
                let id = expected["id"].as_u64().unwrap();
                let request = t.request_for(id, index);
                assert!(
                    id == 0 || request.is_some(),
                    "{}: step {index} expects id {id}, which no earlier request used",
                    t.name
                );
                let op = request
                    .as_ref()
                    .and_then(|r| r["op"].as_str())
                    .map(str::to_string);
                Some((index, expected.clone(), op))
            }
            _ => None,
        })
        .collect()
}

/// Decode a successful result as the client does and encode it again.
fn reencode(op: &str, result: Value) -> Result<Value, String> {
    fn round<T: serde::de::DeserializeOwned + Serialize>(value: Value) -> Result<Value, String> {
        let typed: T = parse_result(value).map_err(|e| e.0)?;
        serde_json::to_value(typed).map_err(|e| e.to_string())
    }
    match op {
        "hello" => round::<HelloResult>(result),
        "load" => round::<LoadResult>(result),
        "transcribe_window" => round::<TranscribeWindowResult>(result),
        "cancel" => round::<CancelResult>(result),
        "status" => round::<StatusResult>(result),
        "ping" | "unload" | "shutdown" if result == json!({}) => Ok(result),
        "ping" | "unload" | "shutdown" => Err(format!("{op} must answer {{}}, got {result}")),
        other => Err(format!("no result type for {other}")),
    }
}

/// The flattened result of a success line, or the error object of a failure line, which is
/// decoded strictly: `decode_incoming` would turn a malformed error object into a `protocol`
/// failure instead of an error, so it is not the place to check one.
fn decode_line(line: &Value) -> Result<Result<Value, ErrorBody>, String> {
    if line["ok"] == false {
        let error = line
            .get("error")
            .ok_or("a failure line needs an error object")?;
        return Ok(Err(strict_error(error)?));
    }
    match decode_incoming(&line.to_string()).map_err(|e| e.0)? {
        Incoming::Response { result, .. } => Ok(result),
        Incoming::Event { .. } => Err("transcripts contain no events".into()),
    }
}

/// An `error` object decoded with the client's type.
fn strict_error(error: &Value) -> Result<ErrorBody, String> {
    serde_json::from_value(error.clone()).map_err(|e| e.to_string())
}

#[test]
fn transcripts_cover_every_op_error_code_and_ending() {
    let all = transcripts();
    let mut ops = BTreeSet::new();
    let mut codes = BTreeSet::new();
    let mut eof = false;
    let mut exit = false;
    for t in &all {
        for (_, expected, op) in answers(t) {
            if expected["ok"] == true {
                ops.insert(op.unwrap());
            } else {
                codes.insert(expected["error"]["code"].as_str().unwrap().to_string());
            }
        }
        eof |= t.steps.contains(&Step::Eof);
        exit |= t.steps.contains(&Step::Exit(0));
    }
    assert_eq!(ops, OPS.iter().map(|s| s.to_string()).collect());
    let every_code: BTreeSet<String> = [
        ErrorCode::Protocol,
        ErrorCode::BadRequest,
        ErrorCode::Unsupported,
        ErrorCode::NotLoaded,
        ErrorCode::ModelMissing,
        ErrorCode::ModelLoadFailed,
        ErrorCode::Busy,
        ErrorCode::Cancelled,
        ErrorCode::Engine,
        ErrorCode::Internal,
    ]
    .iter()
    .map(|c| c.as_str().to_string())
    .collect();
    assert_eq!(codes, every_code);
    assert!(
        eof && exit,
        "the transcripts must cover stdin EOF and shutdown"
    );
}

#[test]
fn golden_responses_decode_strictly() {
    for t in transcripts() {
        for (index, expected, op) in answers(&t) {
            let at = format!("{} step {index}", t.name);
            let line = concrete(&expected).unwrap();
            match decode_line(&line).unwrap_or_else(|e| panic!("{at}: {e}")) {
                Ok(result) => {
                    let op = op.unwrap();
                    let again =
                        reencode(&op, result.clone()).unwrap_or_else(|e| panic!("{at}: {e}"));
                    // Unknown fields are dropped and defaulted fields reappear: either differs.
                    matches(&result, &again).unwrap_or_else(|e| panic!("{at}: {e}"));
                }
                Err(error) => {
                    let keys: BTreeSet<&str> = line
                        .as_object()
                        .unwrap()
                        .keys()
                        .map(String::as_str)
                        .collect();
                    assert_eq!(keys, BTreeSet::from(["error", "id", "ok"]), "{at}");
                    assert!(
                        !matches!(error.code, ErrorCode::Unknown(_)),
                        "{at}: unknown error code"
                    );
                    let again = serde_json::to_value(&error).unwrap();
                    matches(&line["error"], &again).unwrap_or_else(|e| panic!("{at}: {e}"));
                    assert_eq!(error.retryable, error.code == ErrorCode::Busy, "{at}");
                }
            }
        }
    }
}

/// Every field of every golden result is required by the types unless [`OPTIONAL`] lists it,
/// so a host that drops or renames a required field gets a decode error, never a silent default.
#[test]
fn required_result_fields_are_required() {
    let mut optional_seen = BTreeSet::new();
    for t in transcripts() {
        for (index, expected, op) in answers(&t) {
            if expected["ok"] != true {
                continue;
            }
            let op = op.unwrap();
            let line = concrete(&expected).unwrap();
            let Ok(result) = decode_line(&line).unwrap() else {
                unreachable!()
            };
            for (path, without) in removals(&result) {
                let key = format!("{op}:{path}");
                let decoded = reencode(&op, without);
                if OPTIONAL.contains(&key.as_str()) {
                    optional_seen.insert(key.clone());
                    assert!(
                        decoded.is_ok(),
                        "{} step {index}: {key} is optional",
                        t.name
                    );
                } else {
                    assert!(
                        decoded.is_err(),
                        "{} step {index}: a result without {key} must not decode",
                        t.name
                    );
                }
            }
        }
    }
    let listed: BTreeSet<String> = OPTIONAL.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        optional_seen, listed,
        "every optional field appears in a transcript"
    );
}

/// `code`, `message` and `retryable` are all required in an error object: a host that drops one
/// gets its failure turned into a `protocol` error of that request, never a silent default.
#[test]
fn required_error_fields_are_required() {
    for t in transcripts() {
        for (index, expected, _) in answers(&t) {
            if expected["ok"] != false {
                continue;
            }
            let line = concrete(&expected).unwrap();
            let error = line["error"].as_object().unwrap();
            assert_eq!(
                error.keys().map(String::as_str).collect::<BTreeSet<_>>(),
                BTreeSet::from(["code", "message", "retryable"]),
                "{} step {index}",
                t.name
            );
            for key in error.keys() {
                let mut without = error.clone();
                without.remove(key);
                assert!(
                    strict_error(&Value::Object(without.clone())).is_err(),
                    "{} step {index}: an error object without {key} must not decode",
                    t.name
                );
                let mut broken = line.clone();
                broken["error"] = Value::Object(without);
                match decode_incoming(&broken.to_string()) {
                    Ok(Incoming::Response {
                        result: Err(failure),
                        ..
                    }) => assert_eq!(failure.code, ErrorCode::Protocol, "{} step {index}", t.name),
                    other => panic!("{} step {index}: {other:?}", t.name),
                }
            }
        }
    }
}

/// `(path, value without that field)` for every field of an object, its nested objects and the
/// objects in its arrays (the field removed from every item).
fn removals(value: &Value) -> Vec<(String, Value)> {
    let Value::Object(object) = value else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, child) in object {
        let mut without = object.clone();
        without.remove(key);
        out.push((key.clone(), Value::Object(without)));
        let nested: Vec<(String, Value)> = match child {
            Value::Object(_) => removals(child)
                .into_iter()
                .map(|(path, v)| (format!("{key}.{path}"), v))
                .collect(),
            Value::Array(items) if !items.is_empty() => removals(&items[0])
                .into_iter()
                .map(|(path, _)| {
                    let field = path.clone();
                    let stripped: Vec<Value> = items
                        .iter()
                        .map(|item| {
                            let mut item = item.clone();
                            item.as_object_mut().unwrap().remove(&field);
                            item
                        })
                        .collect();
                    (format!("{key}[].{path}"), Value::Array(stripped))
                })
                .collect(),
            _ => Vec::new(),
        };
        for (path, replacement) in nested {
            let mut copy = object.clone();
            copy.insert(key.clone(), replacement);
            out.push((path, Value::Object(copy)));
        }
    }
    out
}

/// One instance of each request with every optional field set, to learn the request field names.
fn maximal_request(op: &str) -> RequestOp {
    match op {
        "hello" => RequestOp::Hello {
            client: ClientInfo {
                name: "n".into(),
                version: "v".into(),
                git_sha: "s".into(),
            },
        },
        "load" => RequestOp::Load {
            model_dir: "/m".into(),
            model_id: "m".into(),
            compute_units: "ane".into(),
        },
        "transcribe_window" => RequestOp::TranscribeWindow {
            pcm_path: "/p".into(),
            offset_samples: 1,
            num_samples: 1,
            sample_rate: 16_000,
            format: "f32le".into(),
            priority: Priority::Batch,
            boost_terms: vec!["t".into()],
            boost_weight: 1.0,
        },
        "cancel" => RequestOp::Cancel { target: 1 },
        "status" => RequestOp::Status,
        "ping" => RequestOp::Ping,
        "unload" => RequestOp::Unload,
        "shutdown" => RequestOp::Shutdown,
        other => panic!("unknown op {other}"),
    }
}

fn request_keys(op: &str) -> BTreeSet<String> {
    let line = encode_request(1, &maximal_request(op));
    let value: Value = serde_json::from_str(&line).unwrap();
    value.as_object().unwrap().keys().cloned().collect()
}

/// The first `expect` for `id` after `step`.
fn answer_to(t: &Transcript, id: u64, step: usize) -> Option<&Value> {
    t.steps[step..].iter().find_map(|s| match s {
        Step::Expect(e) if e["id"].as_u64() == Some(id) => Some(e),
        _ => None,
    })
}

/// Requests decode as the hosts answer them, and the ones a host accepts use only field names
/// the types know: decoding and encoding again reproduces every field the request carries.
/// A `send_raw` line is decoded as written, so it keeps number classes such as `2.0`.
#[test]
fn golden_requests_decode_as_the_hosts_answer_them() {
    for t in transcripts() {
        for (index, step) in t.steps.iter().enumerate() {
            let at = format!("{} step {index}", t.name);
            let (request, unknown, line) = match step {
                Step::Send {
                    request,
                    unknown_fields,
                } => (
                    request.clone(),
                    unknown_fields.as_slice(),
                    request.to_string(),
                ),
                Step::SendRaw(line) => match serde_json::from_str::<Value>(line) {
                    Ok(request) if request.is_object() => (request, &[][..], line.clone()),
                    _ => {
                        assert!(decode_request(line).is_err(), "{at}");
                        answer_to(&t, 0, index).unwrap_or_else(|| panic!("{at}: no answer"));
                        continue;
                    }
                },
                _ => continue,
            };
            let request = &request;
            let parsed = decode_request(&line);
            let id = request["id"].as_u64().filter(|id| *id >= 1);
            let answer =
                answer_to(&t, id.unwrap_or(0), index).unwrap_or_else(|| panic!("{at}: no answer"));
            if id.is_none() {
                assert!(
                    parsed.is_err(),
                    "{at}: a request without a usable id cannot be decoded"
                );
                continue;
            }
            // Engine errors and cancellation come after the host accepted the request.
            const ACCEPTED: [&str; 7] = [
                "not_loaded",
                "model_missing",
                "model_load_failed",
                "busy",
                "cancelled",
                "engine",
                "internal",
            ];
            match answer["error"]["code"].as_str() {
                None => check_accepted(&at, request, unknown, parsed),
                Some(code) if ACCEPTED.contains(&code) => {
                    check_accepted(&at, request, unknown, parsed)
                }
                Some("unsupported") => {
                    assert!(
                        matches!(parsed, Ok(ParsedRequest::UnknownOp { .. })),
                        "{at}"
                    )
                }
                Some(_) => assert!(
                    unknown.is_empty(),
                    "{at}: unknown_fields on a rejected request"
                ),
            }
        }
    }
}

fn check_accepted(
    at: &str,
    request: &Value,
    unknown: &[String],
    parsed: Result<ParsedRequest, DecodeError>,
) {
    let Ok(ParsedRequest::Ok { id, op }) = parsed else {
        panic!("{at}: an accepted request must decode, got {parsed:?}");
    };
    let known = request_keys(op.name());
    for key in request.as_object().unwrap().keys() {
        let declared = unknown.contains(key);
        assert_eq!(
            known.contains(key),
            !declared,
            "{at}: field {key} (declared unknown: {declared})"
        );
    }
    let again: Value = serde_json::from_str(&encode_request(id, &op)).unwrap();
    for (key, value) in again.as_object().unwrap() {
        matches(&request[key], value).unwrap_or_else(|e| panic!("{at}: {key}: {e}"));
    }
}

#[test]
fn fake_engine_definition_decodes() {
    let engine = FakeEngine::load().unwrap();
    assert!(
        engine.capabilities.max_in_flight >= 2,
        "cancel.jsonl queues a third window"
    );
    assert!(engine.windows.values().any(|w| w.hold));
    let load = engine.load_model("/vectors/model", "m").unwrap();
    assert_eq!(load.model_id, "m");
    for (dir, error) in &engine.load_errors {
        assert_eq!(engine.load_model(dir, "m").unwrap_err(), *error);
    }
}

#[test]
fn a_mutated_golden_line_fails_the_strict_decode() {
    // What the tests above catch when a host renames a field the golden lines carry.
    let load = json!({"model_id":"m","load_ms":4,"compiled":false,"window_s":1.0,"frame_s":0.08,
        "vocab_size":3,"blank_id":2});
    assert!(reencode("load", load.clone()).is_ok());
    for field in ["window_s", "vocab_size", "blank_id"] {
        let mut renamed = load.as_object().unwrap().clone();
        let value = renamed.remove(field).unwrap();
        renamed.insert(format!("{field}_renamed"), value);
        assert!(
            reencode("load", Value::Object(renamed)).is_err(),
            "{field} renamed must not decode"
        );
    }
    let error = json!({"code":"busy","message":"full","retryable":true});
    assert!(strict_error(&error).is_ok());
    let mut renamed = error.as_object().unwrap().clone();
    let retryable = renamed.remove("retryable").unwrap();
    renamed.insert("retry".into(), retryable);
    assert!(
        strict_error(&Value::Object(renamed)).is_err(),
        "retryable renamed must not decode"
    );
}
