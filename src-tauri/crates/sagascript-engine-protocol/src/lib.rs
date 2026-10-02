//! Wire types for the `sagascript-engine-host` protocol, version 1.
//!
//! The contract is `docs/engine-host-protocol.md`. Messages are single JSON
//! objects on one `\n`-terminated line. Unknown fields are ignored everywhere
//! and unknown error codes decode to [`ErrorCode::Unknown`].

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// Protocol version implemented by this crate.
pub const PROTOCOL_VERSION: u32 = 1;
/// Maximum size of one line, in bytes.
pub const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

// ---------------------------------------------------------------- requests

/// Identity the client announces in `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
    pub git_sha: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Interactive,
    Batch,
}

pub const MAX_BOOST_TERMS: usize = 500;
pub const MAX_BOOST_TERM_CHARS: usize = 64;
pub const MAX_BOOST_WEIGHT: f32 = 20.0;

fn is_zero_weight(weight: &f32) -> bool {
    *weight == 0.0
}

/// A request body; the `op` tag and its parameters, without `v` and `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RequestOp {
    Hello {
        client: ClientInfo,
    },
    Load {
        model_dir: String,
        model_id: String,
        compute_units: String,
    },
    TranscribeWindow {
        pcm_path: String,
        offset_samples: u64,
        num_samples: u64,
        sample_rate: u32,
        format: String,
        priority: Priority,
        /// Optional context-biasing dictionary (at most `MAX_BOOST_TERMS` terms of at most
        /// `MAX_BOOST_TERM_CHARS` characters). Absent or empty: decoding is unchanged.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        boost_terms: Vec<String>,
        /// Logit bonus for dictionary-continuing tokens (`0..=MAX_BOOST_WEIGHT`).
        #[serde(default, skip_serializing_if = "is_zero_weight")]
        boost_weight: f32,
    },
    Cancel {
        target: u64,
    },
    Status,
    Ping,
    Unload,
    Shutdown,
}

impl RequestOp {
    pub fn name(&self) -> &'static str {
        match self {
            RequestOp::Hello { .. } => "hello",
            RequestOp::Load { .. } => "load",
            RequestOp::TranscribeWindow { .. } => "transcribe_window",
            RequestOp::Cancel { .. } => "cancel",
            RequestOp::Status => "status",
            RequestOp::Ping => "ping",
            RequestOp::Unload => "unload",
            RequestOp::Shutdown => "shutdown",
        }
    }
}

#[derive(Serialize)]
struct RequestEnvelope<'a> {
    v: u32,
    id: u64,
    #[serde(flatten)]
    op: &'a RequestOp,
}

/// Encode a request as one line including the trailing `\n`.
pub fn encode_request(id: u64, op: &RequestOp) -> String {
    let mut line = serde_json::to_string(&RequestEnvelope {
        v: PROTOCOL_VERSION,
        id,
        op,
    })
    .expect("request serialization is infallible");
    line.push('\n');
    line
}

/// Result of decoding a request line on the host side.
#[derive(Debug, Clone, PartialEq)]
pub enum ParsedRequest {
    Ok {
        id: u64,
        op: RequestOp,
    },
    /// Valid envelope but the `op` is not known (answer `unsupported`).
    UnknownOp {
        id: u64,
        op: String,
    },
    /// Envelope is valid but parameters are wrong (answer `bad_request`).
    BadParams {
        id: u64,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DecodeError(pub String);

/// Decode a request line. Fails only when no usable `id` can be found
/// (answer `protocol`, id 0).
pub fn decode_request(line: &str) -> Result<ParsedRequest, DecodeError> {
    let value: Value =
        serde_json::from_str(line.trim()).map_err(|e| DecodeError(format!("invalid JSON: {e}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| DecodeError("request is not a JSON object".into()))?;
    let id = obj
        .get("id")
        .and_then(Value::as_u64)
        .filter(|id| *id >= 1)
        .ok_or_else(|| DecodeError("missing or invalid id".into()))?;
    let op_name = obj
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| DecodeError("missing op".into()))?
        .to_string();
    const KNOWN: [&str; 8] = [
        "hello",
        "load",
        "transcribe_window",
        "cancel",
        "status",
        "ping",
        "unload",
        "shutdown",
    ];
    if !KNOWN.contains(&op_name.as_str()) {
        return Ok(ParsedRequest::UnknownOp { id, op: op_name });
    }
    match serde_json::from_value::<RequestOp>(value) {
        Ok(op) => Ok(ParsedRequest::Ok { id, op }),
        Err(e) => Ok(ParsedRequest::BadParams {
            id,
            message: e.to_string(),
        }),
    }
}

// --------------------------------------------------------- errors / results

/// Error codes; unknown codes from newer hosts are preserved.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    Protocol,
    BadRequest,
    Unsupported,
    NotLoaded,
    ModelMissing,
    ModelLoadFailed,
    Busy,
    Cancelled,
    Engine,
    Internal,
    Unknown(String),
}

impl ErrorCode {
    pub fn as_str(&self) -> &str {
        match self {
            ErrorCode::Protocol => "protocol",
            ErrorCode::BadRequest => "bad_request",
            ErrorCode::Unsupported => "unsupported",
            ErrorCode::NotLoaded => "not_loaded",
            ErrorCode::ModelMissing => "model_missing",
            ErrorCode::ModelLoadFailed => "model_load_failed",
            ErrorCode::Busy => "busy",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::Engine => "engine",
            ErrorCode::Internal => "internal",
            ErrorCode::Unknown(s) => s,
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "protocol" => ErrorCode::Protocol,
            "bad_request" => ErrorCode::BadRequest,
            "unsupported" => ErrorCode::Unsupported,
            "not_loaded" => ErrorCode::NotLoaded,
            "model_missing" => ErrorCode::ModelMissing,
            "model_load_failed" => ErrorCode::ModelLoadFailed,
            "busy" => ErrorCode::Busy,
            "cancelled" => ErrorCode::Cancelled,
            "engine" => ErrorCode::Engine,
            "internal" => ErrorCode::Internal,
            other => ErrorCode::Unknown(other.to_string()),
        }
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(ErrorCode::parse(&s))
    }
}

/// The `error` object of a failure response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: ErrorCode,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    /// Required by the client; `None` makes the handshake fail.
    #[serde(default)]
    pub git_sha: Option<String>,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub engine_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    pub sample_rate: u32,
    pub max_window_s: f64,
    pub preferred_window_s: f64,
    pub preferred_overlap_s: f64,
    pub max_in_flight: u32,
    #[serde(default)]
    pub token_timestamps: bool,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub compute_units: Vec<String>,
    #[serde(default)]
    pub min_macos: Option<String>,
    /// The host honours `boost_terms` / `boost_weight` on `transcribe_window`. Absent (an older
    /// host) means it ignores them and decodes unboosted.
    #[serde(default)]
    pub context_biasing: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloResult {
    pub protocol: u32,
    pub host: HostInfo,
    pub capabilities: Capabilities,
}

/// Accept a non-negative integer, or a finite non-negative float (rounded).
/// Hosts written in languages whose JSON encoders emit doubles for millisecond
/// counters (e.g. Swift `Double`) must not break the client over `95520.4`.
fn lenient_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Number::deserialize(deserializer)?;
    if let Some(n) = value.as_u64() {
        return Ok(n);
    }
    match value.as_f64() {
        Some(f) if f.is_finite() && f >= 0.0 && f <= u64::MAX as f64 => Ok(f.round() as u64),
        _ => Err(serde::de::Error::custom(format!(
            "expected a non-negative number, got {value}"
        ))),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoadResult {
    #[serde(default)]
    pub model_id: String,
    #[serde(default, deserialize_with = "lenient_u64")]
    pub load_ms: u64,
    #[serde(default)]
    pub compiled: bool,
    #[serde(default)]
    pub window_s: f64,
    #[serde(default)]
    pub frame_s: f64,
    #[serde(default)]
    pub vocab_size: u32,
    #[serde(default)]
    pub blank_id: u32,
}

/// One SentencePiece token; `start`/`duration` are relative to the window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireToken {
    pub id: u32,
    pub text: String,
    pub start: f64,
    pub duration: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowTimings {
    #[serde(default, deserialize_with = "lenient_u64")]
    pub preprocess_ms: u64,
    #[serde(default, deserialize_with = "lenient_u64")]
    pub encode_ms: u64,
    #[serde(default, deserialize_with = "lenient_u64")]
    pub decode_ms: u64,
    /// Microseconds spent building the context-biasing trie (0 on a cache hit or when unused).
    #[serde(default, deserialize_with = "lenient_u64", skip_serializing_if = "is_zero_u64")]
    pub boost_us: u64,
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscribeWindowResult {
    pub tokens: Vec<WireToken>,
    #[serde(default)]
    pub audio_s: f64,
    #[serde(default)]
    pub timings: WindowTimings,
    /// Present only when the request carried a dictionary: whether biasing was in effect. `false`
    /// means the host could not apply it (for example a model without top-K outputs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boost_active: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CancelResult {
    pub cancelled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusResult {
    pub state: String,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub in_flight: u32,
    #[serde(default, deserialize_with = "lenient_u64")]
    pub rss_bytes: u64,
    #[serde(default)]
    pub uptime_s: f64,
}

// ---------------------------------------------------------------- incoming

/// A line from the host, classified.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// Terminal response: the flattened result on success, the error on failure.
    Response {
        id: u64,
        result: Result<Value, ErrorBody>,
    },
    Event {
        id: u64,
        name: String,
        fields: Map<String, Value>,
    },
}

impl Incoming {
    pub fn id(&self) -> u64 {
        match self {
            Incoming::Response { id, .. } | Incoming::Event { id, .. } => *id,
        }
    }
}

/// Decode one host line. Unknown fields are kept in the raw value and ignored
/// by the typed accessors.
pub fn decode_incoming(line: &str) -> Result<Incoming, DecodeError> {
    let value: Value =
        serde_json::from_str(line.trim()).map_err(|e| DecodeError(format!("invalid JSON: {e}")))?;
    let Value::Object(mut obj) = value else {
        return Err(DecodeError("line is not a JSON object".into()));
    };
    let id = obj
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| DecodeError("missing or invalid id".into()))?;
    if let Some(Value::String(name)) = obj.get("event") {
        let name = name.clone();
        obj.remove("id");
        obj.remove("event");
        return Ok(Incoming::Event {
            id,
            name,
            fields: obj,
        });
    }
    match obj.get("ok").and_then(Value::as_bool) {
        Some(true) => {
            obj.remove("id");
            obj.remove("ok");
            Ok(Incoming::Response {
                id,
                result: Ok(Value::Object(obj)),
            })
        }
        Some(false) => {
            let err = obj
                .remove("error")
                .map(serde_json::from_value::<ErrorBody>)
                .transpose()
                .map_err(|e| DecodeError(format!("invalid error object: {e}")))?
                .unwrap_or(ErrorBody {
                    code: ErrorCode::Internal,
                    message: "failure response without error object".into(),
                    retryable: false,
                });
            Ok(Incoming::Response {
                id,
                result: Err(err),
            })
        }
        None => Err(DecodeError("neither ok nor event present".into())),
    }
}

/// Parse the flattened result of a successful response into a typed struct.
pub fn parse_result<T: DeserializeOwned>(value: Value) -> Result<T, DecodeError> {
    serde_json::from_value(value).map_err(|e| DecodeError(format!("unexpected result shape: {e}")))
}

// -------------------------------------------------------- response encoding

/// Encode a success response (host side / fake host). `result` must be an
/// object (or null for an empty result); it is flattened next to `id`/`ok`.
pub fn encode_ok(id: u64, result: Value) -> String {
    let mut obj = match result {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    obj.insert("id".into(), id.into());
    obj.insert("ok".into(), true.into());
    let mut line = Value::Object(obj).to_string();
    line.push('\n');
    line
}

pub fn encode_err(id: u64, code: ErrorCode, message: &str, retryable: bool) -> String {
    let mut line = serde_json::json!({
        "id": id,
        "ok": false,
        "error": {"code": code, "message": message, "retryable": retryable},
    })
    .to_string();
    line.push('\n');
    line
}

pub fn encode_event(id: u64, name: &str, fields: Value) -> String {
    let mut obj = match fields {
        Value::Object(o) => o,
        _ => Map::new(),
    };
    obj.insert("id".into(), id.into());
    obj.insert("event".into(), name.into());
    let mut line = Value::Object(obj).to_string();
    line.push('\n');
    line
}

#[cfg(test)]
mod tests;
