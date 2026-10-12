//! Golden protocol transcripts: the machine-checked half of the engine-host contract (#367).
//!
//! `src-tauri/engine-host/test-vectors/protocol/*.jsonl` are request/response transcripts that
//! every host replays in its unit tests against the scripted engine in `fake-engine.json`
//! (Swift: `ProtocolServerTests.swift`; ONNX host: `server/tests.rs`), and that this crate decodes
//! strictly (`vector_tests.rs`). The format is specified in docs/engine-host-protocol.md
//! ("Golden transcripts"). This module is test tooling: it is compiled for this crate's tests
//! and, through the `golden` feature, for the tests of other crates.

use crate::{Capabilities, ErrorBody, ErrorCode, LoadResult, TranscribeWindowResult};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The directory holding the transcripts and `fake-engine.json`.
pub fn vector_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../engine-host/test-vectors/protocol")
}

/// One line of a transcript.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// The client writes this request (serialized as one compact JSON line).
    Send {
        request: Value,
        /// Request fields the vector deliberately adds although no host knows them.
        unknown_fields: Vec<String>,
    },
    /// The client writes this exact line (malformed input).
    SendRaw(String),
    /// The host writes a response with this `id` that matches (see [`matches`]).
    Expect(Value),
    /// The client closes stdin.
    Eof,
    /// The host has finished shutting down (a process exits with this status).
    Exit(i64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transcript {
    /// File name, for messages.
    pub name: String,
    pub steps: Vec<Step>,
}

impl Transcript {
    /// The request sent with this id before `step`, if any.
    pub fn request_for(&self, id: u64, step: usize) -> Option<&Value> {
        self.steps[..step].iter().rev().find_map(|s| match s {
            Step::Send { request, .. } if request.get("id").and_then(Value::as_u64) == Some(id) => {
                Some(request)
            }
            _ => None,
        })
    }
}

/// Every `*.jsonl` transcript in [`vector_dir`], sorted by file name.
pub fn load_transcripts() -> Result<Vec<Transcript>, String> {
    let dir = vector_dir();
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    paths.sort();
    if paths.is_empty() {
        return Err(format!("no transcripts in {}", dir.display()));
    }
    paths
        .iter()
        .map(|path| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let text = std::fs::read_to_string(path).map_err(|e| format!("{name}: {e}"))?;
            parse_transcript(&name, &text)
        })
        .collect()
}

/// Parse one transcript. Each non-empty line is an object with exactly one of `send`,
/// `send_raw`, `expect`, `eof`, `exit` or `note` (a comment); `send` may carry `unknown_fields`.
pub fn parse_transcript(name: &str, text: &str) -> Result<Transcript, String> {
    let mut steps = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let at = format!("{name}:{}", index + 1);
        let value: Value = serde_json::from_str(line).map_err(|e| format!("{at}: {e}"))?;
        let Value::Object(object) = value else {
            return Err(format!("{at}: a step must be a JSON object"));
        };
        if let Some(step) = parse_step(&object).map_err(|e| format!("{at}: {e}"))? {
            steps.push(step);
        }
    }
    Ok(Transcript {
        name: name.to_string(),
        steps,
    })
}

fn parse_step(object: &Map<String, Value>) -> Result<Option<Step>, String> {
    const KINDS: [&str; 6] = ["send", "send_raw", "expect", "eof", "exit", "note"];
    let kinds: Vec<&str> = KINDS
        .into_iter()
        .filter(|kind| object.contains_key(*kind))
        .collect();
    let allowed_extra = usize::from(object.contains_key("unknown_fields"));
    if kinds.len() != 1 || object.len() != 1 + allowed_extra {
        return Err(format!("expected exactly one step kind of {KINDS:?}"));
    }
    let value = &object[kinds[0]];
    let step = match kinds[0] {
        "send" => Step::Send {
            request: value.clone(),
            unknown_fields: unknown_fields(object)?,
        },
        "send_raw" => Step::SendRaw(
            value
                .as_str()
                .ok_or("send_raw must be a string")?
                .to_string(),
        ),
        "expect" => {
            if value.get("id").and_then(Value::as_u64).is_none() {
                return Err("expect needs an integer id".into());
            }
            Step::Expect(value.clone())
        }
        "eof" => Step::Eof,
        "exit" => Step::Exit(value.as_i64().ok_or("exit must be an integer status")?),
        _ => return Ok(None),
    };
    if allowed_extra == 1 && !matches!(step, Step::Send { .. }) {
        return Err("unknown_fields belongs to a send step".into());
    }
    Ok(Some(step))
}

fn unknown_fields(object: &Map<String, Value>) -> Result<Vec<String>, String> {
    let Some(fields) = object.get("unknown_fields") else {
        return Ok(Vec::new());
    };
    fields
        .as_array()
        .and_then(|a| a.iter().map(|f| f.as_str().map(str::to_string)).collect())
        .ok_or_else(|| "unknown_fields must be an array of strings".to_string())
}

// ------------------------------------------------------------- matching

/// Placeholders allowed in `expect` lines for values that differ between hosts or runs, with the
/// sample value [`concrete`] substitutes for each.
pub const PLACEHOLDERS: [&str; 5] = ["<string>", "<sha40>", "<bool>", "<uint>", "<number>"];

fn placeholder(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|s| s.starts_with('<') && s.ends_with('>'))
}

/// Compare an expected value from a transcript with what a host wrote. Objects must have the
/// same keys (a renamed, missing or extra field fails), arrays the same length. A number written
/// as an integer in the transcript must be an integer; a fractional one matches any number within
/// a relative 1e-6 (f32 round trips). Errors name the JSON path of the first difference.
pub fn matches(expected: &Value, actual: &Value) -> Result<(), String> {
    match_at("$", expected, actual)
}

fn match_at(path: &str, expected: &Value, actual: &Value) -> Result<(), String> {
    if let Some(name) = placeholder(expected) {
        return match_placeholder(path, name, actual);
    }
    let mismatch = || format!("{path}: expected {expected}, got {actual}");
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => match_objects(path, e, a),
        (Value::Array(e), Value::Array(a)) => {
            if e.len() != a.len() {
                return Err(format!(
                    "{path}: expected {} items, got {}",
                    e.len(),
                    a.len()
                ));
            }
            e.iter()
                .zip(a)
                .enumerate()
                .try_for_each(|(i, (e, a))| match_at(&format!("{path}[{i}]"), e, a))
        }
        (Value::Number(e), Value::Number(a)) => {
            if numbers_match(e, a) {
                Ok(())
            } else {
                Err(mismatch())
            }
        }
        _ if expected == actual => Ok(()),
        _ => Err(mismatch()),
    }
}

fn match_objects(
    path: &str,
    expected: &Map<String, Value>,
    actual: &Map<String, Value>,
) -> Result<(), String> {
    let missing: Vec<&String> = expected
        .keys()
        .filter(|k| !actual.contains_key(*k))
        .collect();
    let extra: Vec<&String> = actual
        .keys()
        .filter(|k| !expected.contains_key(*k))
        .collect();
    if !missing.is_empty() || !extra.is_empty() {
        return Err(format!(
            "{path}: missing fields {missing:?}, unexpected fields {extra:?}"
        ));
    }
    expected
        .iter()
        .try_for_each(|(key, e)| match_at(&format!("{path}.{key}"), e, &actual[key]))
}

fn numbers_match(expected: &serde_json::Number, actual: &serde_json::Number) -> bool {
    if expected.is_f64() {
        let (Some(e), Some(a)) = (expected.as_f64(), actual.as_f64()) else {
            return false;
        };
        return (e - a).abs() <= 1e-6 * e.abs().max(1.0);
    }
    !actual.is_f64() && expected == actual
}

fn match_placeholder(path: &str, name: &str, actual: &Value) -> Result<(), String> {
    let ok = match name {
        "<string>" => actual.is_string(),
        "<sha40>" => actual.as_str().is_some_and(|s| {
            s.len() == 40
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }),
        "<bool>" => actual.is_boolean(),
        "<uint>" => actual.is_u64(),
        "<number>" => actual.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0),
        _ => return Err(format!("{path}: unknown placeholder {name}")),
    };
    if ok {
        Ok(())
    } else {
        Err(format!("{path}: expected {name}, got {actual}"))
    }
}

/// `expected` with every placeholder replaced by a sample value of its type, so a typed decoder
/// can read it. Unknown placeholders are reported.
pub fn concrete(expected: &Value) -> Result<Value, String> {
    if let Some(name) = placeholder(expected) {
        return match name {
            "<string>" => Ok(Value::from("sample")),
            "<sha40>" => Ok(Value::from("0123456789abcdef0123456789abcdef01234567")),
            "<bool>" => Ok(Value::from(true)),
            "<uint>" => Ok(Value::from(7_u64)),
            "<number>" => Ok(Value::from(1.5_f64)),
            _ => Err(format!("unknown placeholder {name}")),
        };
    }
    Ok(match expected {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(k, v)| concrete(v).map(|v| (k.clone(), v)))
                .collect::<Result<_, _>>()?,
        ),
        Value::Array(items) => Value::Array(items.iter().map(concrete).collect::<Result<_, _>>()?),
        other => other.clone(),
    })
}

// ---------------------------------------------------------- fake engine

/// `fake-engine.json`: the engine a host's protocol server is wired to while it replays the
/// transcripts. Hosts implement it in their test harness from this data.
#[derive(Debug, Clone, Deserialize)]
pub struct FakeEngine {
    /// What `capabilities` returns (and therefore what `hello` announces).
    pub capabilities: Capabilities,
    /// What a successful `load` returns, with the requested `model_id` added.
    pub load_result: FakeLoad,
    /// `load` of these `model_dir`s fails with the error.
    pub load_errors: BTreeMap<String, ErrorBody>,
    /// `transcribe` before a successful `load`, or after `unload`, fails with this error.
    pub not_loaded: ErrorBody,
    /// What `transcribe` does, by `pcm_path`.
    pub windows: BTreeMap<String, FakeWindow>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FakeLoad {
    pub load_ms: u64,
    pub compiled: bool,
    pub window_s: f64,
    pub frame_s: f64,
    pub vocab_size: u32,
    pub blank_id: u32,
}

/// The dictionary the engine must receive for a window (absent: no biasing).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FakeBoost {
    pub terms: Vec<String>,
    pub weight: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FakeWindow {
    #[serde(default)]
    pub expect_boost: Option<FakeBoost>,
    /// Block until the window is cancelled, then fail with `cancelled`.
    #[serde(default)]
    pub hold: bool,
    #[serde(default)]
    pub error: Option<ErrorBody>,
    #[serde(default)]
    pub result: Option<TranscribeWindowResult>,
}

/// What the fake engine does with one window.
#[derive(Debug, Clone, PartialEq)]
pub enum FakeOutcome {
    Result(TranscribeWindowResult),
    Error(ErrorBody),
    Hold,
}

impl FakeEngine {
    pub fn load() -> Result<Self, String> {
        let path = vector_dir().join("fake-engine.json");
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// `load(model_dir, model_id)`.
    pub fn load_model(&self, model_dir: &str, model_id: &str) -> Result<LoadResult, ErrorBody> {
        if let Some(error) = self.load_errors.get(model_dir) {
            return Err(error.clone());
        }
        let r = &self.load_result;
        Ok(LoadResult {
            model_id: model_id.to_string(),
            load_ms: r.load_ms,
            compiled: r.compiled,
            window_s: r.window_s,
            frame_s: r.frame_s,
            vocab_size: r.vocab_size,
            blank_id: r.blank_id,
        })
    }

    /// `transcribe` of `pcm_path` on a loaded model, given the dictionary the engine received.
    pub fn window(&self, pcm_path: &str, boost: Option<&FakeBoost>) -> FakeOutcome {
        let fail = |code, message: String| {
            FakeOutcome::Error(ErrorBody {
                code,
                message,
                retryable: false,
            })
        };
        let Some(window) = self.windows.get(pcm_path) else {
            return fail(
                ErrorCode::Engine,
                format!("fake engine has no window {pcm_path}"),
            );
        };
        if window.expect_boost.as_ref() != boost {
            return fail(
                ErrorCode::Internal,
                format!(
                    "fake engine expected dictionary {:?}, got {boost:?}",
                    window.expect_boost
                ),
            );
        }
        if window.hold {
            return FakeOutcome::Hold;
        }
        match (&window.error, &window.result) {
            (Some(error), _) => FakeOutcome::Error(error.clone()),
            (None, Some(result)) => FakeOutcome::Result(result.clone()),
            (None, None) => fail(
                ErrorCode::Internal,
                format!("fake window {pcm_path} has no outcome"),
            ),
        }
    }
}
