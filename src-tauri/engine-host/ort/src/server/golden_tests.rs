//! Replays the golden protocol transcripts (`src-tauri/engine-host/test-vectors/protocol/`)
//! through this host's [`Server`], wired to the scripted engine in `fake-engine.json`. The Core ML
//! host replays the same files in `ProtocolServerTests.swift`, and the protocol crate decodes them
//! strictly, so the two hosts and the client cannot drift apart silently.

use super::*;
use sagascript_engine_protocol::golden::{
    load_transcripts, matches, FakeBoost, FakeEngine, FakeOutcome, Step, Transcript,
};
use sagascript_engine_protocol::ErrorBody;
use std::time::Duration;

/// How long a response or the shutdown may take before the replay fails.
const WAIT: Duration = Duration::from_secs(5);

struct VectorEngine {
    spec: FakeEngine,
    loaded: Mutex<Option<String>>,
}

fn host_error(error: ErrorBody) -> HostError {
    HostError {
        code: error.code,
        message: error.message,
        retryable: error.retryable,
    }
}

impl Engine for Arc<VectorEngine> {
    fn engine_name(&self) -> &'static str {
        "vectors"
    }
    fn engine_version(&self) -> String {
        "golden transcripts".into()
    }
    fn capabilities(&self) -> Capabilities {
        self.spec.capabilities.clone()
    }
    fn load(&self, params: &LoadParams) -> Result<LoadResult, HostError> {
        let result = self
            .spec
            .load_model(&params.model_dir.to_string_lossy(), &params.model_id)
            .map_err(host_error)?;
        *lock(&self.loaded) = Some(params.model_id.clone());
        Ok(result)
    }
    fn transcribe(
        &self,
        request: &WindowRequest,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<TranscribeWindowResult, HostError> {
        if lock(&self.loaded).is_none() {
            return Err(host_error(self.spec.not_loaded.clone()));
        }
        // The dictionary the real engine would apply: terms with a positive weight.
        let boost =
            (!request.boost_terms.is_empty() && request.boost_weight > 0.0).then(|| FakeBoost {
                terms: request.boost_terms.clone(),
                weight: request.boost_weight,
            });
        let path = request.pcm_path.to_string_lossy();
        match self.spec.window(&path, boost.as_ref()) {
            FakeOutcome::Result(result) => Ok(result),
            FakeOutcome::Error(error) => Err(host_error(error)),
            FakeOutcome::Hold => {
                let started = Instant::now();
                while !is_cancelled() {
                    if started.elapsed() > WAIT * 2 {
                        return Err(HostError::new(
                            ErrorCode::Internal,
                            "held window never cancelled",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(HostError::new(
                    ErrorCode::Cancelled,
                    "Transcription was cancelled",
                ))
            }
        }
    }
    fn unload(&self) {
        *lock(&self.loaded) = None;
    }
    fn loaded_model_id(&self) -> Option<String> {
        lock(&self.loaded).clone()
    }
    fn is_loading(&self) -> bool {
        false
    }
    fn rss_bytes(&self) -> u64 {
        4096
    }
}

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);

impl Write for Output {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        lock(&self.0).extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Output {
    /// Every line written so far, each of which must be one JSON object.
    fn lines(&self) -> Vec<Value> {
        let bytes = lock(&self.0).clone();
        String::from_utf8(bytes)
            .expect("stdout is UTF-8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("stdout carries only JSON lines"))
            .collect()
    }

    /// The first response with `id` not matched yet (events are skipped).
    fn take(&self, id: u64, consumed: &mut Vec<usize>) -> Option<Value> {
        let deadline = Instant::now() + WAIT;
        loop {
            let found = self.lines().into_iter().enumerate().find(|(index, line)| {
                !consumed.contains(index) && line.get("event").is_none() && line["id"] == id
            });
            if let Some((index, line)) = found {
                consumed.push(index);
                return Some(line);
            }
            if Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

fn replay(transcript: &Transcript, spec: &FakeEngine) {
    let engine = Arc::new(VectorEngine {
        spec: spec.clone(),
        loaded: Mutex::new(None),
    });
    let out = Output::default();
    let exited = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&exited);
    let server = Server::new(
        engine,
        HostBuild {
            version: "0.0.0".into(),
            git_sha: "c".repeat(40),
            dirty: false,
        },
        Box::new(out.clone()),
        Box::new(move || flag.store(true, Ordering::SeqCst)),
    );
    let mut consumed = Vec::new();
    for (index, step) in transcript.steps.iter().enumerate() {
        let at = format!("{} step {index}", transcript.name);
        match step {
            Step::Send { request, .. } => server.handle_line(&request.to_string()),
            Step::SendRaw(line) => server.handle_line(line),
            Step::Expect(expected) => {
                let id = expected["id"].as_u64().expect("expect has an id");
                let actual = out.take(id, &mut consumed).unwrap_or_else(|| {
                    panic!(
                        "{at}: no response with id {id}; host wrote {:?}",
                        out.lines()
                    )
                });
                if let Err(difference) = matches(expected, &actual) {
                    panic!("{at}: {difference}\n  expected {expected}\n  actual   {actual}");
                }
            }
            Step::Eof => server.receive_eof(),
            Step::Exit(status) => {
                assert_eq!(*status, 0, "{at}: hosts exit 0");
                let deadline = Instant::now() + WAIT;
                while !exited.load(Ordering::SeqCst) {
                    assert!(
                        Instant::now() < deadline,
                        "{at}: the host did not shut down"
                    );
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }
    for (index, line) in out.lines().iter().enumerate() {
        assert!(
            consumed.contains(&index) || line.get("event").is_some(),
            "{}: response the transcript does not expect: {line}",
            transcript.name
        );
    }
}

#[test]
fn golden_transcripts_replay() {
    let spec = FakeEngine::load().expect("fake-engine.json");
    let transcripts = load_transcripts().expect("golden transcripts");
    for name in [
        "handshake.jsonl",
        "transcribe.jsonl",
        "cancel.jsonl",
        "eof.jsonl",
    ] {
        assert!(transcripts.iter().any(|t| t.name == name), "missing {name}");
    }
    for transcript in &transcripts {
        replay(transcript, &spec);
    }
}
