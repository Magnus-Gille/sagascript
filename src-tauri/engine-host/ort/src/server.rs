//! Protocol v1 server: envelope handling, request scheduling and output framing.
//!
//! Engine-agnostic (see [`Engine`]); the scheduler mirrors the Swift Core ML host:
//! at most `max_in_flight` `transcribe_window` jobs run at once, and queued jobs start
//! interactive-first, then in arrival order. `ping`, `status` and `cancel` are answered
//! immediately, `load`/`unload` run on one serial control thread.

use serde_json::{json, Map, Value};
use sagascript_engine_protocol::{
    decode_request, Capabilities, ErrorCode, LoadResult, ParsedRequest, Priority, RequestOp,
    TranscribeWindowResult, MAX_LINE_BYTES, PROTOCOL_VERSION,
};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl HostError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadParams {
    pub model_dir: PathBuf,
    pub model_id: String,
    pub compute_units: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowRequest {
    pub pcm_path: PathBuf,
    pub offset_samples: u64,
    pub num_samples: u64,
    pub priority: Priority,
    /// Context-biasing dictionary and logit bonus (empty terms or zero weight: off).
    pub boost_terms: Vec<String>,
    pub boost_weight: f32,
}

/// Build identity reported in `hello`.
#[derive(Debug, Clone)]
pub struct HostBuild {
    pub version: String,
    pub git_sha: String,
    pub dirty: bool,
}

pub trait Engine: Send + Sync + 'static {
    /// Engine name for `hello.host.engine` (for example `"onnx"`).
    fn engine_name(&self) -> &'static str;
    /// Free-form version for `hello.host.engine_version` (evaluated at `hello` time).
    fn engine_version(&self) -> String;
    fn capabilities(&self) -> Capabilities;
    /// How many windows actually execute at once. `capabilities().max_in_flight` is how many
    /// the client may keep outstanding; a CPU engine can advertise 2 (so the client reserves
    /// one slot for dictation) yet execute one at a time, queueing the rest by priority.
    fn execution_slots(&self) -> usize {
        self.capabilities().max_in_flight.max(1) as usize
    }
    fn load(&self, params: &LoadParams) -> Result<LoadResult, HostError>;
    /// Must poll `is_cancelled` between stages and return `ErrorCode::Cancelled` when set.
    fn transcribe(
        &self,
        request: &WindowRequest,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<TranscribeWindowResult, HostError>;
    fn unload(&self);
    fn loaded_model_id(&self) -> Option<String>;
    fn is_loading(&self) -> bool;
    fn rss_bytes(&self) -> u64;
}

struct Job {
    id: u64,
    priority: Priority,
    sequence: u64,
    request: WindowRequest,
    flag: Arc<AtomicBool>,
}

#[derive(Default)]
struct Sched {
    next_sequence: u64,
    pending: Vec<Job>,
    running: HashMap<u64, Arc<AtomicBool>>,
}

#[derive(Default)]
struct Flags {
    saw_hello: bool,
    stopping: bool,
}

type ControlTask = Box<dyn FnOnce() + Send>;

pub struct Server<E: Engine> {
    engine: E,
    build: HostBuild,
    max_in_flight: usize,
    output: Mutex<Box<dyn Write + Send>>,
    on_shutdown: Box<dyn Fn() + Send + Sync>,
    flags: Mutex<Flags>,
    sched: Mutex<Sched>,
    control: Mutex<Sender<ControlTask>>,
    started: Instant,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A poisoned lock only means another thread panicked; the data is still consistent
    // for our simple state, and dying here would take the whole host down.
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn priority_rank(priority: Priority) -> u8 {
    match priority {
        Priority::Interactive => 0,
        Priority::Batch => 1,
    }
}

impl<E: Engine> Server<E> {
    pub fn new(
        engine: E,
        build: HostBuild,
        output: Box<dyn Write + Send>,
        on_shutdown: Box<dyn Fn() + Send + Sync>,
    ) -> Arc<Self> {
        let (tx, rx) = channel::<ControlTask>();
        std::thread::Builder::new()
            .name("control".into())
            .spawn(move || {
                while let Ok(task) = rx.recv() {
                    task();
                }
            })
            .expect("spawn control thread");
        let max_in_flight = engine.execution_slots().max(1);
        Arc::new(Self {
            engine,
            build,
            max_in_flight,
            output: Mutex::new(output),
            on_shutdown,
            flags: Mutex::new(Flags::default()),
            sched: Mutex::new(Sched::default()),
            control: Mutex::new(tx),
            started: Instant::now(),
        })
    }

    pub fn handle_line(self: &Arc<Self>, line: &str) {
        if line.len() > MAX_LINE_BYTES {
            self.send_failure(0, ErrorCode::Protocol, "JSON line exceeds 16 MiB", false);
            return;
        }
        let parsed = match decode_request(line) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.send_failure(0, ErrorCode::Protocol, &error.to_string(), false);
                return;
            }
        };
        let (id, op) = match parsed {
            ParsedRequest::Ok { id, op } => (id, Some(op)),
            ParsedRequest::UnknownOp { id, .. } | ParsedRequest::BadParams { id, .. } => (id, None),
        };
        // Envelope version.
        let version_ok = serde_json::from_str::<Value>(line.trim())
            .ok()
            .and_then(|v| v.get("v").and_then(Value::as_u64))
            == Some(u64::from(PROTOCOL_VERSION));
        if !version_ok {
            self.send_failure(id, ErrorCode::Protocol, "Invalid request envelope", false);
            return;
        }
        let (stopping, saw_hello) = {
            let flags = lock(&self.flags);
            (flags.stopping, flags.saw_hello)
        };
        if stopping {
            self.send_failure(id, ErrorCode::Protocol, "Host is shutting down", false);
            return;
        }
        let is_hello = matches!(op, Some(RequestOp::Hello { .. }));
        if !saw_hello {
            match op {
                Some(RequestOp::Hello { .. }) => self.handle_hello(id),
                _ => self.send_failure(id, ErrorCode::Protocol, "hello must be the first operation", false),
            }
            return;
        }
        if is_hello {
            self.send_failure(id, ErrorCode::Protocol, "hello may only be sent once", false);
            return;
        }
        let Some(op) = op else {
            // Unknown op or bad parameters, re-parse to tell them apart.
            match decode_request(line) {
                Ok(ParsedRequest::UnknownOp { op, .. }) => {
                    self.send_failure(id, ErrorCode::Unsupported, &format!("Unknown operation: {op}"), false)
                }
                Ok(ParsedRequest::BadParams { message, .. }) => {
                    self.send_failure(id, ErrorCode::BadRequest, &message, false)
                }
                _ => self.send_failure(id, ErrorCode::Protocol, "Invalid request envelope", false),
            }
            return;
        };
        match op {
            RequestOp::Hello { .. } => unreachable!("handled above"),
            RequestOp::Ping => self.send_success(id, Map::new()),
            RequestOp::Status => self.handle_status(id),
            RequestOp::Cancel { target } => {
                let cancelled = self.cancel(target);
                let mut fields = Map::new();
                fields.insert("cancelled".into(), json!(cancelled));
                self.send_success(id, fields);
            }
            RequestOp::Load { model_dir, model_id, compute_units } => {
                self.handle_load(id, model_dir, model_id, compute_units)
            }
            RequestOp::TranscribeWindow {
                pcm_path,
                offset_samples,
                num_samples,
                sample_rate,
                format,
                priority,
                boost_terms,
                boost_weight,
            } => self.handle_transcribe(id, pcm_path, offset_samples, num_samples, sample_rate, &format, priority, boost_terms, boost_weight),
            RequestOp::Unload => {
                let this = Arc::clone(self);
                self.run_control(Box::new(move || {
                    this.engine.unload();
                    this.send_success(id, Map::new());
                }));
            }
            RequestOp::Shutdown => {
                lock(&self.flags).stopping = true;
                self.send_success(id, Map::new());
                let this = Arc::clone(self);
                self.run_control(Box::new(move || {
                    this.cancel_all();
                    (this.on_shutdown)();
                }));
            }
        }
    }

    /// stdin reached EOF or the parent went away: fail queued work, cancel running work.
    pub fn receive_eof(&self) {
        lock(&self.flags).stopping = true;
        self.cancel_all();
    }

    fn run_control(&self, task: ControlTask) {
        if lock(&self.control).send(task).is_err() {
            eprintln!("control thread is gone");
        }
    }

    fn handle_hello(&self, id: u64) {
        lock(&self.flags).saw_hello = true;
        let mut capabilities = serde_json::to_value(self.engine.capabilities()).unwrap_or(Value::Null);
        if let Some(object) = capabilities.as_object_mut() {
            // `min_macos` is Core ML specific; leave it out rather than sending null.
            if object.get("min_macos").is_some_and(Value::is_null) {
                object.remove("min_macos");
            }
        }
        let mut fields = Map::new();
        fields.insert("protocol".into(), json!(PROTOCOL_VERSION));
        fields.insert(
            "host".into(),
            json!({
                "name": "sagascript-engine-host",
                "version": self.build.version,
                "git_sha": self.build.git_sha,
                "build_dirty": self.build.dirty,
                "engine": self.engine.engine_name(),
                "engine_version": self.engine.engine_version(),
            }),
        );
        fields.insert("capabilities".into(), capabilities);
        self.send_success(id, fields);
    }

    fn handle_status(&self, id: u64) {
        let in_flight = lock(&self.sched).running.len();
        let model_id = self.engine.loaded_model_id();
        let state = if self.engine.is_loading() {
            "loading"
        } else if in_flight > 0 {
            "busy"
        } else if model_id.is_some() {
            "ready"
        } else {
            "idle"
        };
        let mut fields = Map::new();
        fields.insert("state".into(), json!(state));
        fields.insert("model_id".into(), json!(model_id));
        fields.insert("in_flight".into(), json!(in_flight));
        fields.insert("rss_bytes".into(), json!(self.engine.rss_bytes()));
        fields.insert("uptime_s".into(), json!(self.started.elapsed().as_secs_f64()));
        self.send_success(id, fields);
    }

    fn handle_load(self: &Arc<Self>, id: u64, model_dir: String, model_id: String, compute_units: String) {
        let path = PathBuf::from(&model_dir);
        if !path.is_absolute() || model_id.is_empty() {
            self.send_failure(
                id,
                ErrorCode::BadRequest,
                "load requires absolute model_dir, model_id, and compute_units",
                false,
            );
            return;
        }
        let params = LoadParams { model_dir: path, model_id, compute_units };
        let this = Arc::clone(self);
        self.run_control(Box::new(move || match this.engine.load(&params) {
            Ok(result) => match serde_json::to_value(result) {
                Ok(Value::Object(fields)) => this.send_success(id, fields),
                _ => this.send_failure(id, ErrorCode::Internal, "cannot encode load result", false),
            },
            Err(error) => this.send_error(id, &error),
        }));
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_transcribe(
        self: &Arc<Self>,
        id: u64,
        pcm_path: String,
        offset_samples: u64,
        num_samples: u64,
        sample_rate: u32,
        format: &str,
        priority: Priority,
        boost_terms: Vec<String>,
        boost_weight: f32,
    ) {
        let path = PathBuf::from(&pcm_path);
        if !path.is_absolute() || num_samples == 0 || format != "f32le" {
            self.send_failure(id, ErrorCode::BadRequest, "transcribe_window has invalid parameters", false);
            return;
        }
        let capabilities = self.engine.capabilities();
        if sample_rate != capabilities.sample_rate {
            self.send_failure(id, ErrorCode::BadRequest, "sample_rate must match the host sample rate", false);
            return;
        }
        let maximum = (capabilities.max_window_s * f64::from(capabilities.sample_rate)).floor() as u64;
        if num_samples > maximum {
            self.send_failure(id, ErrorCode::BadRequest, "num_samples exceeds max_window_s", false);
            return;
        }
        use sagascript_engine_protocol::{MAX_BOOST_TERMS, MAX_BOOST_TERM_CHARS, MAX_BOOST_WEIGHT};
        if boost_terms.len() > MAX_BOOST_TERMS
            || boost_terms.iter().any(|t| t.chars().count() > MAX_BOOST_TERM_CHARS)
            || !boost_weight.is_finite()
            || !(0.0..=MAX_BOOST_WEIGHT).contains(&boost_weight)
        {
            self.send_failure(id, ErrorCode::BadRequest, "boost_terms must be at most 500 strings of at most 64 characters and boost_weight within 0..=20", false);
            return;
        }
        let request = WindowRequest { pcm_path: path, offset_samples, num_samples, priority, boost_terms, boost_weight };
        {
            let mut sched = lock(&self.sched);
            let sequence = sched.next_sequence;
            sched.next_sequence += 1;
            sched.pending.push(Job {
                id,
                priority,
                sequence,
                request,
                flag: Arc::new(AtomicBool::new(false)),
            });
        }
        self.pump();
    }

    /// Start as many queued jobs as free slots allow (interactive first, then FIFO).
    fn pump(self: &Arc<Self>) {
        let started: Vec<Job> = {
            let mut sched = lock(&self.sched);
            let mut started = Vec::new();
            while sched.running.len() < self.max_in_flight && !sched.pending.is_empty() {
                let index = sched
                    .pending
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, job)| (priority_rank(job.priority), job.sequence))
                    .map(|(index, _)| index)
                    .expect("pending is not empty");
                let job = sched.pending.remove(index);
                sched.running.insert(job.id, Arc::clone(&job.flag));
                started.push(job);
            }
            started
        };
        for job in started {
            let this = Arc::clone(self);
            let spawned = std::thread::Builder::new()
                .name(format!("window-{}", job.id))
                .spawn({
                    let this = Arc::clone(&this);
                    move || this.run_job(job)
                });
            if let Err(error) = spawned {
                eprintln!("cannot spawn worker thread: {error}");
            }
        }
    }

    fn run_job(self: Arc<Self>, job: Job) {
        let flag = Arc::clone(&job.flag);
        let is_cancelled = move || flag.load(Ordering::SeqCst);
        let result = self.engine.transcribe(&job.request, &is_cancelled);
        // Honor a `cancel` that returned true even if the window happened to finish.
        let result = match result {
            Ok(_) if job.flag.load(Ordering::SeqCst) => {
                Err(HostError::new(ErrorCode::Cancelled, "Transcription was cancelled"))
            }
            other => other,
        };
        lock(&self.sched).running.remove(&job.id);
        self.pump();
        match result {
            Ok(result) => match serde_json::to_value(result) {
                Ok(Value::Object(fields)) => self.send_success(job.id, fields),
                _ => self.send_failure(job.id, ErrorCode::Internal, "cannot encode result", false),
            },
            Err(error) => self.send_error(job.id, &error),
        }
    }

    /// True when a pending or running request with this id existed.
    fn cancel(&self, target: u64) -> bool {
        let removed = {
            let mut sched = lock(&self.sched);
            if let Some(index) = sched.pending.iter().position(|job| job.id == target) {
                Some(sched.pending.remove(index))
            } else if let Some(flag) = sched.running.get(&target) {
                flag.store(true, Ordering::SeqCst);
                return true;
            } else {
                None
            }
        };
        match removed {
            Some(job) => {
                self.send_failure(job.id, ErrorCode::Cancelled, "Transcription was cancelled", false);
                true
            }
            None => false,
        }
    }

    fn cancel_all(&self) {
        let pending = {
            let mut sched = lock(&self.sched);
            for flag in sched.running.values() {
                flag.store(true, Ordering::SeqCst);
            }
            std::mem::take(&mut sched.pending)
        };
        for job in pending {
            self.send_failure(job.id, ErrorCode::Cancelled, "Host is exiting", false);
        }
    }

    fn send_success(&self, id: u64, fields: Map<String, Value>) {
        let mut response = Map::new();
        response.insert("id".into(), json!(id));
        response.insert("ok".into(), json!(true));
        for (key, value) in fields {
            response.insert(key, value);
        }
        self.write(&Value::Object(response));
    }

    fn send_error(&self, id: u64, error: &HostError) {
        self.send_failure(id, error.code.clone(), &error.message, error.retryable);
    }

    fn send_failure(&self, id: u64, code: ErrorCode, message: &str, retryable: bool) {
        self.write(&json!({
            "id": id,
            "ok": false,
            "error": {"code": code.as_str(), "message": message, "retryable": retryable},
        }));
    }

    fn write(&self, value: &Value) {
        let mut line = match serde_json::to_string(value) {
            Ok(line) => line,
            Err(error) => {
                eprintln!("protocol serialization error: {error}");
                return;
            }
        };
        line.push('\n');
        let mut output = lock(&self.output);
        // stdout is protocol-only; a broken pipe means the parent is gone (EOF handling exits).
        if let Err(error) = output.write_all(line.as_bytes()).and_then(|()| output.flush()) {
            eprintln!("cannot write response: {error}");
        }
    }
}

#[cfg(test)]
mod tests;
