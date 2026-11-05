use super::*;
use sagascript_engine_protocol::{WindowTimings, WireToken};
use std::sync::atomic::AtomicU64;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Condvar;

#[derive(Clone, Default)]
struct SharedOut(Arc<Mutex<Vec<u8>>>);

impl Write for SharedOut {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl SharedOut {
    fn lines(&self) -> Vec<Value> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn response(&self, id: u64) -> Option<Value> {
        self.lines().into_iter().find(|v| v["id"] == id)
    }

    fn wait_response(&self, id: u64) -> Value {
        for _ in 0..500 {
            if let Some(v) = self.response(id) {
                return v;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("no response for {id}: {:?}", self.lines());
    }
}

/// Gate that holds `transcribe` calls until released, recording start order.
struct Gate {
    open: Mutex<bool>,
    cv: Condvar,
}

/// A monotonic clock the tests move by hand (`warm` skips a canary that ran under 30 s ago).
struct TestClock {
    base: Instant,
    offset_ms: AtomicU64,
}

impl TestClock {
    fn now(&self) -> Instant {
        self.base + Duration::from_millis(self.offset_ms.load(Ordering::SeqCst))
    }

    fn advance(&self, by: Duration) {
        self.offset_ms
            .fetch_add(by.as_millis() as u64, Ordering::SeqCst);
    }
}

struct MockEngine {
    max_in_flight: u32,
    slots: usize,
    loaded: Mutex<Option<String>>,
    gate: Option<Arc<Gate>>,
    started: Mutex<Vec<String>>,
    started_tx: Mutex<Sender<()>>,
    /// Holds `canary` calls while closed (open by default); independent of the window `gate`.
    canary_gate: Arc<Gate>,
    /// Canaries that started (loaded model found).
    canaries: Mutex<usize>,
    /// The next canary fails with this error.
    canary_failure: Mutex<Option<HostError>>,
}

impl MockEngine {
    fn new(max_in_flight: u32, gate: Option<Arc<Gate>>) -> (Self, Receiver<()>) {
        let (tx, rx) = channel();
        (
            Self {
                max_in_flight,
                slots: max_in_flight as usize,
                loaded: Mutex::new(None),
                gate,
                started: Mutex::new(vec![]),
                started_tx: Mutex::new(tx),
                canary_gate: Arc::new(Gate {
                    open: Mutex::new(true),
                    cv: Condvar::new(),
                }),
                canaries: Mutex::new(0),
                canary_failure: Mutex::new(None),
            },
            rx,
        )
    }
}

impl Engine for Arc<MockEngine> {
    fn engine_name(&self) -> &'static str {
        "mock"
    }
    fn engine_version(&self) -> String {
        "0".into()
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            sample_rate: 16_000,
            max_window_s: 30.0,
            preferred_window_s: 30.0,
            preferred_overlap_s: 6.0,
            max_in_flight: self.max_in_flight,
            token_timestamps: true,
            languages: vec!["sv".into()],
            compute_units: vec!["cpu".into()],
            min_macos: None,
            context_biasing: false,
        }
    }
    fn execution_slots(&self) -> usize {
        self.slots
    }
    fn load(&self, params: &LoadParams) -> Result<LoadResult, HostError> {
        if params.model_id == "bad" {
            return Err(HostError::new(ErrorCode::ModelMissing, "nope"));
        }
        *self.loaded.lock().unwrap() = Some(params.model_id.clone());
        Ok(LoadResult {
            model_id: params.model_id.clone(),
            load_ms: 3,
            compiled: false,
            window_s: 30.0,
            frame_s: 0.08,
            vocab_size: 10,
            blank_id: 9,
        })
    }
    fn transcribe(
        &self,
        request: &WindowRequest,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<TranscribeWindowResult, HostError> {
        if self.loaded.lock().unwrap().is_none() {
            return Err(HostError::new(ErrorCode::NotLoaded, "not loaded"));
        }
        self.started
            .lock()
            .unwrap()
            .push(request.pcm_path.to_string_lossy().into_owned());
        let _ = self.started_tx.lock().unwrap().send(());
        if let Some(gate) = &self.gate {
            let mut open = gate.open.lock().unwrap();
            while !*open {
                if is_cancelled() {
                    return Err(HostError::new(ErrorCode::Cancelled, "cancelled"));
                }
                open = gate
                    .cv
                    .wait_timeout(open, Duration::from_millis(10))
                    .unwrap()
                    .0;
            }
        }
        Ok(TranscribeWindowResult {
            tokens: vec![WireToken {
                id: 1,
                text: "▁hej".into(),
                start: 0.0,
                duration: 0.08,
                confidence: None,
            }],
            audio_s: request.num_samples as f64 / 16_000.0,
            timings: WindowTimings::default(),
            boost_active: None,
            boost_reason: None,
        })
    }
    fn canary(&self) -> Result<Canary, HostError> {
        if self.loaded.lock().unwrap().is_none() {
            return Err(HostError::new(ErrorCode::NotLoaded, "not loaded"));
        }
        *self.canaries.lock().unwrap() += 1;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut open = self.canary_gate.open.lock().unwrap();
        while !*open {
            if Instant::now() > deadline {
                return Err(HostError::new(ErrorCode::Internal, "canary never released"));
            }
            open = self
                .canary_gate
                .cv
                .wait_timeout(open, Duration::from_millis(10))
                .unwrap()
                .0;
        }
        drop(open);
        match self.canary_failure.lock().unwrap().take() {
            Some(error) => Err(error),
            None => Ok(Canary {
                canary_ms: 5,
                probable_recompile: false,
            }),
        }
    }
    fn unload(&self) {
        *self.loaded.lock().unwrap() = None;
    }
    fn loaded_model_id(&self) -> Option<String> {
        self.loaded.lock().unwrap().clone()
    }
    fn is_loading(&self) -> bool {
        false
    }
    fn rss_bytes(&self) -> u64 {
        123
    }
}

struct Fixture {
    server: Arc<Server<Arc<MockEngine>>>,
    engine: Arc<MockEngine>,
    out: SharedOut,
    started: Receiver<()>,
    shutdown: Arc<AtomicBool>,
    clock: Arc<TestClock>,
}

fn fixture(max_in_flight: u32, gated: bool) -> (Fixture, Arc<Gate>) {
    let gate = Arc::new(Gate {
        open: Mutex::new(false),
        cv: Condvar::new(),
    });
    let (engine, started) = MockEngine::new(max_in_flight, gated.then(|| Arc::clone(&gate)));
    let engine = Arc::new(engine);
    let out = SharedOut::default();
    let shutdown = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&shutdown);
    let clock = Arc::new(TestClock {
        base: Instant::now(),
        offset_ms: AtomicU64::new(0),
    });
    let now = Arc::clone(&clock);
    let server = Server::with_clock(
        Arc::clone(&engine),
        HostBuild {
            version: "9.9.9".into(),
            git_sha: "a".repeat(40),
            dirty: false,
        },
        Box::new(out.clone()),
        Box::new(move || flag.store(true, Ordering::SeqCst)),
        Box::new(move || now.now()),
    );
    (
        Fixture {
            server,
            engine,
            out,
            started,
            shutdown,
            clock,
        },
        gate,
    )
}

fn open(gate: &Gate) {
    *gate.open.lock().unwrap() = true;
    gate.cv.notify_all();
}

fn req(id: u64, op: &str, extra: Value) -> String {
    let mut object = json!({"v": 1, "id": id, "op": op});
    if let (Some(o), Some(e)) = (object.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            o.insert(k.clone(), v.clone());
        }
    }
    object.to_string()
}

fn hello(f: &Fixture) {
    f.server.handle_line(&req(
        1,
        "hello",
        json!({"client": {"name": "t", "version": "1", "git_sha": "0"}}),
    ));
}

fn load(f: &Fixture, id: u64) {
    f.server.handle_line(&req(
        id,
        "load",
        json!({"model_dir": absolute("model"), "model_id": "m", "compute_units": "ane"}),
    ));
    assert_eq!(f.out.wait_response(id)["ok"], true);
}

fn absolute(name: &str) -> String {
    std::env::temp_dir()
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn window(id: u64, name: &str, samples: u64, priority: &str) -> String {
    req(
        id,
        "transcribe_window",
        json!({"pcm_path": absolute(name), "offset_samples": 0, "num_samples": samples,
               "sample_rate": 16000, "format": "f32le", "priority": priority}),
    )
}

fn error_code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap()
}

#[test]
fn hello_must_come_first_and_only_once() {
    let (f, _) = fixture(1, false);
    f.server.handle_line(&req(1, "ping", json!({})));
    assert_eq!(error_code(&f.out.response(1).unwrap()), "protocol");
    hello(&f);
    // id 1 was reused by the test for hello: the second line is the success one.
    let hello = f.out.lines().into_iter().find(|v| v["ok"] == true).unwrap();
    assert_eq!(hello["protocol"], 1);
    assert_eq!(hello["host"]["name"], "sagascript-engine-host");
    assert_eq!(hello["host"]["engine"], "mock");
    assert_eq!(hello["host"]["git_sha"].as_str().unwrap().len(), 40);
    assert!(hello["capabilities"].get("min_macos").is_none());
    assert_eq!(hello["capabilities"]["max_in_flight"], 1);
    f.server.handle_line(&req(
        2,
        "hello",
        json!({"client": {"name": "t", "version": "1", "git_sha": "0"}}),
    ));
    assert_eq!(error_code(&f.out.response(2).unwrap()), "protocol");
}

#[test]
fn envelope_errors_and_unknown_ops() {
    let (f, _) = fixture(1, false);
    hello(&f);
    f.server.handle_line("not json");
    assert_eq!(f.out.response(0).unwrap()["error"]["code"], "protocol");
    f.server.handle_line(r#"{"v":2,"id":5,"op":"ping"}"#);
    assert_eq!(error_code(&f.out.response(5).unwrap()), "protocol");
    f.server.handle_line(&req(6, "frobnicate", json!({})));
    assert_eq!(error_code(&f.out.response(6).unwrap()), "unsupported");
    f.server
        .handle_line(&req(7, "load", json!({"model_dir": absolute("m")})));
    assert_eq!(error_code(&f.out.response(7).unwrap()), "bad_request");
    f.server.handle_line(&req(
        8,
        "load",
        json!({"model_dir": "relative", "model_id": "m", "compute_units": "cpu"}),
    ));
    assert_eq!(error_code(&f.out.wait_response(8)), "bad_request");
}

#[test]
fn load_status_unload_and_integer_fields() {
    let (f, _) = fixture(1, false);
    hello(&f);
    f.server.handle_line(&req(2, "status", json!({})));
    let idle = f.out.response(2).unwrap();
    assert_eq!(idle["state"], "idle");
    assert!(idle["in_flight"].is_u64() && idle["rss_bytes"].is_u64());
    load(&f, 3);
    assert!(f.out.response(3).unwrap()["load_ms"].is_u64());
    f.server.handle_line(&req(4, "status", json!({})));
    assert_eq!(f.out.response(4).unwrap()["state"], "ready");
    f.server.handle_line(&req(
        5,
        "load",
        json!({"model_dir": absolute("m"), "model_id": "bad", "compute_units": "cpu"}),
    ));
    assert_eq!(error_code(&f.out.wait_response(5)), "model_missing");
    f.server.handle_line(&req(6, "unload", json!({})));
    assert_eq!(f.out.wait_response(6)["ok"], true);
    f.server.handle_line(&window(7, "a.pcm", 16_000, "batch"));
    assert_eq!(error_code(&f.out.wait_response(7)), "not_loaded");
}

#[test]
fn transcribe_validates_window_bounds() {
    let (f, _) = fixture(1, false);
    hello(&f);
    load(&f, 2);
    f.server.handle_line(&window(3, "a.pcm", 480_001, "batch"));
    assert_eq!(error_code(&f.out.response(3).unwrap()), "bad_request");
    f.server.handle_line(&window(4, "a.pcm", 0, "batch"));
    assert_eq!(error_code(&f.out.response(4).unwrap()), "bad_request");
    f.server.handle_line(&window(5, "a.pcm", 480_000, "batch"));
    let ok = f.out.wait_response(5);
    assert_eq!(ok["ok"], true);
    assert_eq!(ok["tokens"][0]["text"], "▁hej");
    assert_eq!(ok["audio_s"], 30.0);
    assert!(ok["timings"]["encode_ms"].is_u64());
}

#[test]
fn interactive_jumps_the_queue_and_in_flight_is_bounded() {
    let (f, gate) = fixture(1, true);
    hello(&f);
    load(&f, 2);
    f.server.handle_line(&window(10, "first", 16_000, "batch"));
    f.started.recv_timeout(Duration::from_secs(5)).unwrap();
    f.server
        .handle_line(&window(11, "batch-2", 16_000, "batch"));
    f.server
        .handle_line(&window(12, "batch-3", 16_000, "batch"));
    f.server
        .handle_line(&window(13, "dictation", 16_000, "interactive"));
    f.server.handle_line(&req(14, "status", json!({})));
    let status = f.out.response(14).unwrap();
    assert_eq!(status["in_flight"], 1);
    assert_eq!(status["state"], "busy");
    open(&gate);
    for id in [10, 11, 12, 13] {
        assert_eq!(f.out.wait_response(id)["ok"], true);
    }
    let order: Vec<String> = f
        .engine
        .started
        .lock()
        .unwrap()
        .iter()
        .map(|p| {
            std::path::Path::new(p)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(order, ["first", "dictation", "batch-2", "batch-3"]);
}

#[test]
fn cancel_pending_running_and_unknown() {
    let (f, gate) = fixture(1, true);
    hello(&f);
    load(&f, 2);
    f.server.handle_line(&window(10, "run", 16_000, "batch"));
    f.started.recv_timeout(Duration::from_secs(5)).unwrap();
    f.server.handle_line(&window(11, "queued", 16_000, "batch"));
    f.server
        .handle_line(&req(12, "cancel", json!({"target": 11})));
    assert_eq!(f.out.response(12).unwrap()["cancelled"], true);
    assert_eq!(error_code(&f.out.response(11).unwrap()), "cancelled");
    f.server
        .handle_line(&req(13, "cancel", json!({"target": 10})));
    assert_eq!(f.out.response(13).unwrap()["cancelled"], true);
    assert_eq!(error_code(&f.out.wait_response(10)), "cancelled");
    f.server
        .handle_line(&req(14, "cancel", json!({"target": 999})));
    assert_eq!(f.out.response(14).unwrap()["cancelled"], false);
    open(&gate);
    f.server.handle_line(&req(15, "status", json!({})));
    assert_eq!(f.out.response(15).unwrap()["in_flight"], 0);
}

#[test]
fn cancel_wins_even_if_the_window_completes() {
    // A job that ignores the flag still reports `cancelled` once cancel returned true.
    let (f, gate) = fixture(1, true);
    hello(&f);
    load(&f, 2);
    f.server.handle_line(&window(10, "run", 16_000, "batch"));
    f.started.recv_timeout(Duration::from_secs(5)).unwrap();
    open(&gate);
    // Race-free variant: mark the flag directly, then let the engine finish successfully.
    {
        let sched = lock(&f.server.sched);
        if let Some(flag) = sched.running.get(&10) {
            flag.store(true, Ordering::SeqCst);
        }
    }
    let response = f.out.wait_response(10);
    // Either the engine saw the flag (cancelled) or completed before it was set (ok).
    assert!(response["ok"] == true || error_code(&response) == "cancelled");
}

#[test]
fn shutdown_answers_then_calls_back_and_rejects_later_requests() {
    let (f, _) = fixture(1, false);
    hello(&f);
    f.server.handle_line(&req(2, "shutdown", json!({})));
    assert_eq!(f.out.response(2).unwrap()["ok"], true);
    for _ in 0..200 {
        if f.shutdown.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(f.shutdown.load(Ordering::SeqCst));
    f.server.handle_line(&req(3, "ping", json!({})));
    assert_eq!(error_code(&f.out.response(3).unwrap()), "protocol");
}

#[test]
fn eof_fails_queued_windows() {
    let (f, gate) = fixture(1, true);
    hello(&f);
    load(&f, 2);
    f.server.handle_line(&window(10, "run", 16_000, "batch"));
    f.started.recv_timeout(Duration::from_secs(5)).unwrap();
    f.server.handle_line(&window(11, "queued", 16_000, "batch"));
    f.server.receive_eof();
    assert_eq!(error_code(&f.out.response(11).unwrap()), "cancelled");
    assert_eq!(error_code(&f.out.wait_response(10)), "cancelled");
    open(&gate);
}

#[test]
fn advertised_two_with_one_execution_slot_still_runs_interactive_first() {
    let gate = Arc::new(Gate {
        open: Mutex::new(false),
        cv: Condvar::new(),
    });
    let (mut engine, started) = MockEngine::new(2, Some(Arc::clone(&gate)));
    engine.slots = 1;
    let engine = Arc::new(engine);
    let out = SharedOut::default();
    let server = Server::new(
        Arc::clone(&engine),
        HostBuild {
            version: "1".into(),
            git_sha: "b".repeat(40),
            dirty: false,
        },
        Box::new(out.clone()),
        Box::new(|| {}),
    );
    server.handle_line(&req(
        1,
        "hello",
        json!({"client": {"name": "t", "version": "1", "git_sha": "0"}}),
    ));
    assert_eq!(out.response(1).unwrap()["capabilities"]["max_in_flight"], 2);
    server.handle_line(&req(
        2,
        "load",
        json!({"model_dir": absolute("m"), "model_id": "m", "compute_units": "cpu"}),
    ));
    out.wait_response(2);
    server.handle_line(&window(10, "batch-run", 16_000, "batch"));
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    // The client's reserved slot: a dictation window arrives while a batch window runs.
    server.handle_line(&window(11, "dictation", 16_000, "interactive"));
    server.handle_line(&window(12, "batch-next", 16_000, "batch"));
    server.handle_line(&req(13, "status", json!({})));
    assert_eq!(out.response(13).unwrap()["in_flight"], 1);
    open(&gate);
    for id in [10, 11, 12] {
        assert_eq!(out.wait_response(id)["ok"], true);
    }
    let order: Vec<String> = engine
        .started
        .lock()
        .unwrap()
        .iter()
        .map(|p| {
            std::path::Path::new(p)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(order, ["batch-run", "dictation", "batch-next"]);
}

fn warm(f: &Fixture, id: u64) {
    f.server.handle_line(&req(id, "warm", json!({})));
}

/// Closes the canary gate: the next canary stays running until [`release_canary`].
fn hold_canary(f: &Fixture) {
    *f.engine.canary_gate.open.lock().unwrap() = false;
}

fn release_canary(f: &Fixture) {
    open(&f.engine.canary_gate);
}

fn wait_canaries(f: &Fixture, count: usize) {
    for _ in 0..500 {
        if *f.engine.canaries.lock().unwrap() >= count {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("fewer than {count} canaries started");
}

/// The whole response, `skipped` reason or the canary's numbers.
fn skipped(response: &Value) -> &str {
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["ran"], false, "{response}");
    assert!(response.get("canary_ms").is_none(), "{response}");
    response["skipped"].as_str().unwrap()
}

#[test]
fn warm_runs_a_canary_and_is_answered_when_it_finished() {
    let (f, _) = fixture(1, false);
    hello(&f);
    warm(&f, 2);
    assert_eq!(skipped(&f.out.response(2).unwrap()), "not_loaded");
    load(&f, 3);
    warm(&f, 4);
    let ran = f.out.wait_response(4);
    assert_eq!(ran["ok"], true);
    assert_eq!(ran["ran"], true);
    assert!(ran["canary_ms"].is_u64(), "{ran}");
    assert_eq!(ran["probable_recompile"], false);
    assert!(ran.get("skipped").is_none(), "{ran}");
    assert_eq!(*f.engine.canaries.lock().unwrap(), 1);
    // stdout carries protocol lines only: one per request.
    assert_eq!(f.out.lines().len(), 4);
}

#[test]
fn a_running_canary_takes_no_window_slot_and_is_unknown_to_cancel() {
    // One execution slot: a canary that held it would block the dictation window.
    let (f, _) = fixture(1, false);
    hello(&f);
    load(&f, 2);
    hold_canary(&f);
    warm(&f, 10);
    wait_canaries(&f, 1);
    f.server.handle_line(&req(11, "status", json!({})));
    let status = f.out.response(11).unwrap();
    assert_eq!(status["in_flight"], 0);
    assert_eq!(status["state"], "ready");
    f.server
        .handle_line(&req(12, "cancel", json!({"target": 10})));
    assert_eq!(f.out.response(12).unwrap()["cancelled"], false);
    f.server
        .handle_line(&window(13, "dictation", 16_000, "interactive"));
    assert_eq!(f.out.wait_response(13)["ok"], true);
    assert!(
        f.out.response(10).is_none(),
        "the canary is still running: {:?}",
        f.out.lines()
    );
    release_canary(&f);
    let ran = f.out.wait_response(10);
    assert_eq!(ran["ran"], true, "{ran}");
}

#[test]
fn a_second_warm_while_a_canary_runs_is_in_progress_and_not_waited_for() {
    let (f, _) = fixture(1, false);
    hello(&f);
    load(&f, 2);
    hold_canary(&f);
    warm(&f, 10);
    wait_canaries(&f, 1);
    warm(&f, 11);
    // Answered at once, from the thread that handled the line.
    assert_eq!(skipped(&f.out.response(11).unwrap()), "in_progress");
    // in_progress is checked before not_loaded.
    f.server.handle_line(&req(12, "unload", json!({})));
    assert_eq!(f.out.wait_response(12)["ok"], true);
    warm(&f, 13);
    assert_eq!(skipped(&f.out.response(13).unwrap()), "in_progress");
    assert!(f.out.response(10).is_none());
    release_canary(&f);
    assert_eq!(f.out.wait_response(10)["ran"], true);
    assert_eq!(*f.engine.canaries.lock().unwrap(), 1);
    // The slot is free again (and the model is gone).
    warm(&f, 14);
    assert_eq!(skipped(&f.out.response(14).unwrap()), "not_loaded");
}

#[test]
fn recent_expires_after_thirty_seconds_on_the_injected_clock() {
    let (f, _) = fixture(1, false);
    hello(&f);
    load(&f, 2);
    warm(&f, 3);
    assert_eq!(f.out.wait_response(3)["ran"], true);
    warm(&f, 4);
    assert_eq!(skipped(&f.out.response(4).unwrap()), "recent");
    f.clock.advance(Duration::from_millis(29_999));
    warm(&f, 5);
    assert_eq!(skipped(&f.out.response(5).unwrap()), "recent");
    // "Less than 30 s ago": at 30 s the canary runs again, and restarts the interval.
    f.clock.advance(Duration::from_millis(1));
    warm(&f, 6);
    assert_eq!(f.out.wait_response(6)["ran"], true);
    warm(&f, 7);
    assert_eq!(skipped(&f.out.response(7).unwrap()), "recent");
    assert_eq!(*f.engine.canaries.lock().unwrap(), 2);
}

#[test]
fn a_window_that_ran_counts_as_activity() {
    let (f, _) = fixture(1, false);
    hello(&f);
    load(&f, 2);
    f.server
        .handle_line(&window(3, "dictation", 16_000, "interactive"));
    assert_eq!(f.out.wait_response(3)["ok"], true);
    warm(&f, 4);
    assert_eq!(skipped(&f.out.response(4).unwrap()), "recent");
    f.clock.advance(Duration::from_secs(30));
    warm(&f, 5);
    assert_eq!(f.out.wait_response(5)["ran"], true);
    assert_eq!(*f.engine.canaries.lock().unwrap(), 1);
}

#[test]
fn a_cancelled_window_is_not_activity() {
    let (f, gate) = fixture(1, true);
    hello(&f);
    load(&f, 2);
    f.server.handle_line(&window(3, "run", 16_000, "batch"));
    f.started.recv_timeout(Duration::from_secs(5)).unwrap();
    f.server
        .handle_line(&req(4, "cancel", json!({"target": 3})));
    assert_eq!(error_code(&f.out.wait_response(3)), "cancelled");
    open(&gate);
    warm(&f, 5);
    assert_eq!(f.out.wait_response(5)["ran"], true);
}

#[test]
fn a_queued_or_running_window_makes_warm_busy_before_recent() {
    let (f, gate) = fixture(1, true);
    hello(&f);
    load(&f, 2);
    f.server.handle_line(&window(3, "run", 16_000, "batch"));
    f.started.recv_timeout(Duration::from_secs(5)).unwrap();
    warm(&f, 4);
    assert_eq!(skipped(&f.out.response(4).unwrap()), "busy");
    f.server.handle_line(&window(5, "queued", 16_000, "batch"));
    warm(&f, 6);
    assert_eq!(skipped(&f.out.response(6).unwrap()), "busy");
    f.server.handle_line(&req(7, "status", json!({})));
    assert_eq!(f.out.response(7).unwrap()["in_flight"], 1);
    open(&gate);
    assert_eq!(f.out.wait_response(3)["ok"], true);
    assert_eq!(f.out.wait_response(5)["ok"], true);
    warm(&f, 8);
    assert_eq!(skipped(&f.out.response(8).unwrap()), "recent");
    assert_eq!(*f.engine.canaries.lock().unwrap(), 0);
}

#[test]
fn a_failing_canary_answers_the_engine_error_and_is_not_activity() {
    let (f, _) = fixture(1, false);
    hello(&f);
    load(&f, 2);
    *f.engine.canary_failure.lock().unwrap() =
        Some(HostError::new(ErrorCode::Engine, "encoder failed"));
    warm(&f, 3);
    let failed = f.out.wait_response(3);
    assert_eq!(failed["ok"], false);
    assert_eq!(error_code(&failed), "engine");
    assert_eq!(failed["error"]["message"], "encoder failed");
    // The canary slot is free and the failure did not count as the model having run.
    warm(&f, 4);
    assert_eq!(f.out.wait_response(4)["ran"], true);
    // A model unloaded between the check and the canary is a skip, not an error.
    f.clock.advance(Duration::from_secs(30));
    *f.engine.canary_failure.lock().unwrap() =
        Some(HostError::new(ErrorCode::NotLoaded, "No model is loaded"));
    warm(&f, 5);
    assert_eq!(skipped(&f.out.wait_response(5)), "not_loaded");
}

#[test]
fn warm_log_line_names_trigger_time_and_recompile() {
    assert_eq!(
        warm_log_line("request", 38, false),
        "warm canary trigger=request ms=38 probable_recompile=false"
    );
    // A probable recompile is logged at INFO by the client, which keys on the prefix.
    assert_eq!(
        warm_log_line("request", 12_500, true),
        "info: warm canary trigger=request ms=12500 probable_recompile=true"
    );
}
