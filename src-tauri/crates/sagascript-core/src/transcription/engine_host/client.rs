//! Handshake, lifecycle (lazy start, idle unload/shutdown, restart budget)
//! and window scheduling on top of [`HostProcess`].

use super::pipeline::CancelToken;
use super::process::{crashed, Delivery, HostProcess, Pending};
use super::{EngineHostError, Result};
use sagascript_engine_protocol::{
    parse_result, Capabilities, ClientInfo, ErrorCode, HelloResult, HostInfo, LoadResult, Priority,
    RequestOp, StatusResult, TranscribeWindowResult, WindowTimings, WireToken, PROTOCOL_VERSION,
};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

/// Identity announced in `hello`; compared with the host's build for a warning.
#[derive(Debug, Clone)]
pub struct ClientIdentity {
    pub name: String,
    pub version: String,
    pub git_sha: String,
}

impl Default for ClientIdentity {
    fn default() -> Self {
        Self {
            name: "sagascript".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            git_sha: option_env!("SAGASCRIPT_GIT_SHA")
                .unwrap_or("unknown")
                .into(),
        }
    }
}

/// What to load after the handshake.
#[derive(Debug, Clone)]
pub struct LoadSpec {
    pub model_dir: PathBuf,
    pub model_id: String,
    pub compute_units: String,
}

/// Request timeouts; injectable so tests do not wait for real ones.
#[derive(Debug, Clone)]
pub struct Timeouts {
    pub hello: Duration,
    pub load: Duration,
    pub ping: Duration,
    pub status: Duration,
    pub unload: Duration,
    /// `transcribe_window` timeout is `max(transcribe_min, transcribe_factor * window_s)`.
    pub transcribe_min: Duration,
    pub transcribe_factor: f64,
    pub shutdown_grace: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            hello: Duration::from_secs(10),
            load: Duration::from_secs(180),
            ping: Duration::from_secs(2),
            status: Duration::from_secs(5),
            unload: Duration::from_secs(10),
            transcribe_min: Duration::from_secs(15),
            transcribe_factor: 3.0,
            shutdown_grace: Duration::from_secs(2),
        }
    }
}

impl Timeouts {
    pub fn transcribe(&self, window_s: f64) -> Duration {
        self.transcribe_min.max(Duration::from_secs_f64(
            (self.transcribe_factor * window_s).max(0.0),
        ))
    }
}

#[derive(Debug, Clone)]
pub struct EngineHostConfig {
    pub host_path: PathBuf,
    /// Extra arguments after the mandatory `--protocol 1`.
    pub extra_args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub load: LoadSpec,
    pub identity: ClientIdentity,
    pub timeouts: Timeouts,
    /// Unload the model after this much inactivity (`None`: never).
    pub idle_unload: Option<Duration>,
    /// Shut the process down this long after the unload (`None`: never).
    pub idle_shutdown: Option<Duration>,
    /// How often the supervisor thread checks idleness.
    pub supervisor_tick: Duration,
    /// Delay before restart number 1, 2, 3 ... (last value repeats).
    pub restart_backoff: Vec<Duration>,
    pub max_restarts: usize,
    pub restart_window: Duration,
    /// How often a window that was in flight during a crash is resubmitted.
    pub max_crash_retries: u32,
    pub busy_retries: u32,
    pub busy_retry_delay: Duration,
    /// Parent for the private PCM temp dir (`None`: the system temp dir).
    pub temp_dir: Option<PathBuf>,
}

impl EngineHostConfig {
    pub fn new(host_path: impl Into<PathBuf>, load: LoadSpec) -> Self {
        Self {
            host_path: host_path.into(),
            extra_args: Vec::new(),
            env: Vec::new(),
            load,
            identity: ClientIdentity::default(),
            timeouts: Timeouts::default(),
            idle_unload: Some(Duration::from_secs(120)),
            idle_shutdown: Some(Duration::from_secs(600)),
            supervisor_tick: Duration::from_secs(5),
            restart_backoff: vec![
                Duration::ZERO,
                Duration::from_secs(1),
                Duration::from_secs(5),
            ],
            max_restarts: 3,
            restart_window: Duration::from_secs(300),
            max_crash_retries: 1,
            busy_retries: 3,
            busy_retry_delay: Duration::from_millis(50),
            temp_dir: None,
        }
    }
}

/// Point-in-time view for diagnostics and tests.
#[derive(Debug, Clone)]
pub struct HostSnapshot {
    pub running: bool,
    pub loaded: bool,
    pub failed: Option<String>,
    pub pid: Option<u32>,
    pub crashes_in_window: usize,
    pub capabilities: Option<Capabilities>,
    /// Host build identity from the last successful `hello`.
    pub host: Option<HostInfo>,
    /// Result of the last successful `load` on the current process.
    pub last_load: Option<LoadResult>,
}

/// One transcribed window as returned by the host.
#[derive(Debug, Clone)]
pub struct WindowOutput {
    pub tokens: Vec<WireToken>,
    pub audio_s: f64,
    pub timings: WindowTimings,
    pub round_trip_ms: u64,
}

struct State {
    proc: Option<Arc<HostProcess>>,
    caps: Option<Capabilities>,
    host_info: Option<HostInfo>,
    last_load: Option<LoadResult>,
    loaded: bool,
    crashes: VecDeque<Instant>,
    failed: Option<String>,
    last_activity: Instant,
    /// Slots held (batch + interactive), so idle logic never fires mid-request.
    active: usize,
    batch_in_flight: usize,
    interactive_active: usize,
}

pub(crate) struct Inner {
    pub(crate) cfg: EngineHostConfig,
    state: Mutex<State>,
    cv: Condvar,
    /// Serializes start / load / unload / shutdown.
    lifecycle: Mutex<()>,
}

/// Handle to one (lazily started) engine host. Cheap to clone.
#[derive(Clone)]
pub struct EngineHostClient {
    pub(crate) inner: Arc<Inner>,
}

impl EngineHostClient {
    pub fn new(cfg: EngineHostConfig) -> Self {
        let supervise = cfg.idle_unload.is_some() || cfg.idle_shutdown.is_some();
        let inner = Arc::new(Inner {
            cfg,
            state: Mutex::new(State {
                proc: None,
                caps: None,
                host_info: None,
                last_load: None,
                loaded: false,
                crashes: VecDeque::new(),
                failed: None,
                last_activity: Instant::now(),
                active: 0,
                batch_in_flight: 0,
                interactive_active: 0,
            }),
            cv: Condvar::new(),
            lifecycle: Mutex::new(()),
        });
        if supervise {
            let weak = Arc::downgrade(&inner);
            let tick = inner.cfg.supervisor_tick;
            let spawned = std::thread::Builder::new()
                .name("engine-host-supervisor".into())
                .spawn(move || supervise_loop(weak, tick));
            if let Err(e) = spawned {
                tracing::warn!(error = %e, "could not start engine host supervisor; idle handling disabled");
            }
        }
        Self { inner }
    }

    pub fn config(&self) -> &EngineHostConfig {
        &self.inner.cfg
    }

    /// Start the host and load the model now (pre-warm). Returns its capabilities.
    pub fn warm(&self) -> Result<Capabilities> {
        self.inner.ensure_ready().map(|(_, caps)| caps)
    }

    pub fn snapshot(&self) -> HostSnapshot {
        let mut st = self.inner.state.lock().unwrap();
        let cutoff = self.inner.cfg.restart_window;
        st.crashes.retain(|t| t.elapsed() <= cutoff);
        HostSnapshot {
            running: st.proc.as_ref().is_some_and(|p| p.is_alive()),
            loaded: st.loaded && st.proc.as_ref().is_some_and(|p| p.is_alive()),
            failed: st.failed.clone(),
            pid: st.proc.as_ref().filter(|p| p.is_alive()).map(|p| p.pid()),
            crashes_in_window: st.crashes.len(),
            capabilities: st.caps.clone(),
            host: st.host_info.clone(),
            last_load: st.last_load.clone(),
        }
    }

    /// Clear the `Failed` state and the crash history.
    pub fn reset(&self) {
        let mut st = self.inner.state.lock().unwrap();
        st.failed = None;
        st.crashes.clear();
    }

    /// `status` of the running host (never starts one).
    pub fn status(&self) -> Result<StatusResult> {
        let proc = self.inner.live_proc()?;
        let v = proc.request(&RequestOp::Status, self.inner.cfg.timeouts.status)?;
        parse_result(v).map_err(|e| EngineHostError::Protocol(e.0))
    }

    /// `ping` of the running host (never starts one); returns the round trip.
    pub fn ping(&self) -> Result<Duration> {
        let proc = self.inner.live_proc()?;
        let t = Instant::now();
        proc.request(&RequestOp::Ping, self.inner.cfg.timeouts.ping)?;
        Ok(t.elapsed())
    }

    /// Free model memory but keep the process.
    pub fn unload(&self) -> Result<()> {
        let _g = self.inner.lifecycle.lock().unwrap();
        self.inner.unload_locked()
    }

    /// Graceful shutdown (`shutdown` op, then kill after the grace period).
    pub fn shutdown(&self) {
        let _g = self.inner.lifecycle.lock().unwrap();
        self.inner.shutdown_locked();
    }

    /// Capabilities negotiated with the running host, if any.
    pub fn capabilities(&self) -> Option<Capabilities> {
        self.inner.state.lock().unwrap().caps.clone()
    }

    /// Ensure the host is loaded, then submit one window.
    ///
    /// Batch windows wait (up to `wait_slot`) for a free slot and yield to any
    /// interactive request; interactive windows are sent immediately.
    /// Returns `Ok(None)` when no slot became free within `wait_slot`.
    pub fn submit_window(
        &self,
        req: &WindowRequest,
        cancel: &CancelToken,
        wait_slot: Duration,
    ) -> Result<Option<WindowTicket>> {
        let inner = &self.inner;
        // Interactive registers first so queued batch windows step aside while it
        // is starting the host or loading the model.
        let mut slot = if req.priority == Priority::Interactive {
            Some(inner.acquire_interactive())
        } else {
            None
        };
        let (proc, caps) = inner.ensure_ready()?;
        if slot.is_none() {
            slot = inner.acquire_batch(caps.max_in_flight.max(1) as usize, cancel, wait_slot)?;
        }
        let Some(slot) = slot else { return Ok(None) };
        let timeout = inner
            .cfg
            .timeouts
            .transcribe(req.num_samples as f64 / f64::from(caps.sample_rate));
        let pending = proc.send(&window_op(req))?;
        Ok(Some(WindowTicket {
            inner: inner.clone(),
            proc,
            pending: Some(pending),
            req: req.clone(),
            timeout,
            deadline: Instant::now() + timeout,
            started: Instant::now(),
            busy_attempts: 0,
            _slot: slot,
        }))
    }
}

/// Parameters of one window request.
#[derive(Debug, Clone)]
pub struct WindowRequest {
    pub pcm_path: PathBuf,
    pub offset_samples: u64,
    pub num_samples: u64,
    pub sample_rate: u32,
    pub priority: Priority,
}

fn window_op(req: &WindowRequest) -> RequestOp {
    RequestOp::TranscribeWindow {
        pcm_path: req.pcm_path.display().to_string(),
        offset_samples: req.offset_samples,
        num_samples: req.num_samples,
        sample_rate: req.sample_rate,
        format: "f32le".into(),
        priority: req.priority,
    }
}

/// An in-flight window. Dropping it cancels the request best-effort.
pub struct WindowTicket {
    inner: Arc<Inner>,
    proc: Arc<HostProcess>,
    pending: Option<Pending>,
    req: WindowRequest,
    timeout: Duration,
    deadline: Instant,
    started: Instant,
    busy_attempts: u32,
    _slot: Slot,
}

impl WindowTicket {
    /// Block until the window finishes, fails, times out or `cancel` fires.
    pub fn wait(mut self, cancel: &CancelToken) -> Result<WindowOutput> {
        loop {
            let pending = self
                .pending
                .as_ref()
                .expect("pending present until finished");
            if cancel.is_cancelled() {
                self.abandon();
                return Err(EngineHostError::Cancelled);
            }
            let now = Instant::now();
            if now >= self.deadline {
                self.abandon();
                self.inner.after_timeout(&self.proc);
                return Err(EngineHostError::Timeout {
                    op: "transcribe_window",
                    after: self.timeout,
                });
            }
            let slice = (self.deadline - now).min(Duration::from_millis(20));
            match pending.recv_timeout(slice) {
                Ok(Delivery::Event { .. }) | Err(RecvTimeoutError::Timeout) => {}
                Ok(Delivery::Response(Ok(v))) => {
                    self.pending = None;
                    let r: TranscribeWindowResult =
                        parse_result(v).map_err(|e| EngineHostError::Protocol(e.0))?;
                    return Ok(WindowOutput {
                        tokens: r.tokens,
                        audio_s: r.audio_s,
                        timings: r.timings,
                        round_trip_ms: self.started.elapsed().as_millis() as u64,
                    });
                }
                Ok(Delivery::Response(Err(e))) => {
                    self.pending = None;
                    let busy = e.code == ErrorCode::Busy && e.retryable;
                    if busy && self.busy_attempts < self.inner.cfg.busy_retries {
                        self.busy_attempts += 1;
                        std::thread::sleep(self.inner.cfg.busy_retry_delay);
                        self.pending = Some(self.proc.send(&window_op(&self.req))?);
                        continue;
                    }
                    return Err(super::process::remote(e));
                }
                Ok(Delivery::Closed(info)) => {
                    self.pending = None;
                    if !info.expected && self.proc.claim_crash() {
                        Inner::record_crash(&mut self.inner.state.lock().unwrap());
                    }
                    return Err(crashed(&info));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.pending = None;
                    return Err(EngineHostError::Protocol("response channel dropped".into()));
                }
            }
        }
    }

    /// Best-effort `cancel` for the request and stop listening for its answer.
    fn abandon(&mut self) {
        if let Some(p) = self.pending.take() {
            if self.proc.is_alive() {
                // Fire and forget: the answer is discarded.
                let _ = self.proc.send(&RequestOp::Cancel { target: p.id });
            }
            self.proc.forget(p.id);
        }
    }
}

impl Drop for WindowTicket {
    fn drop(&mut self) {
        self.abandon();
    }
}

/// Scheduling slot; releasing it wakes queued batch windows.
struct Slot {
    inner: Arc<Inner>,
    priority: Priority,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut st = self.inner.state.lock().unwrap();
        st.active -= 1;
        match self.priority {
            Priority::Batch => st.batch_in_flight -= 1,
            Priority::Interactive => st.interactive_active -= 1,
        }
        st.last_activity = Instant::now();
        drop(st);
        self.inner.cv.notify_all();
    }
}

impl Inner {
    fn acquire_interactive(self: &Arc<Self>) -> Slot {
        let mut st = self.state.lock().unwrap();
        st.active += 1;
        st.interactive_active += 1;
        Slot {
            inner: self.clone(),
            priority: Priority::Interactive,
        }
    }

    fn acquire_batch(
        self: &Arc<Self>,
        max_in_flight: usize,
        cancel: &CancelToken,
        wait: Duration,
    ) -> Result<Option<Slot>> {
        let deadline = Instant::now() + wait;
        let mut st = self.state.lock().unwrap();
        loop {
            if cancel.is_cancelled() {
                return Err(EngineHostError::Cancelled);
            }
            if st.batch_in_flight < max_in_flight && st.interactive_active == 0 {
                st.active += 1;
                st.batch_in_flight += 1;
                return Ok(Some(Slot {
                    inner: self.clone(),
                    priority: Priority::Batch,
                }));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let slice = (deadline - now).min(Duration::from_millis(20));
            st = self.cv.wait_timeout(st, slice).unwrap().0;
        }
    }

    fn live_proc(&self) -> Result<Arc<HostProcess>> {
        let st = self.state.lock().unwrap();
        match &st.proc {
            Some(p) if p.is_alive() => Ok(p.clone()),
            _ => Err(EngineHostError::NotRunning),
        }
    }

    /// Lazy start + handshake + load. Fast path when everything is ready.
    fn ensure_ready(&self) -> Result<(Arc<HostProcess>, Capabilities)> {
        {
            let mut st = self.state.lock().unwrap();
            if let (Some(p), Some(c)) = (&st.proc, &st.caps) {
                if st.loaded && p.is_alive() {
                    let out = (p.clone(), c.clone());
                    st.last_activity = Instant::now();
                    return Ok(out);
                }
            }
        }
        let _g = self.lifecycle.lock().unwrap();
        let (proc, caps) = self.ensure_started_locked()?;
        let loaded = self.state.lock().unwrap().loaded;
        if !loaded {
            let load = &self.cfg.load;
            let v = proc.request(
                &RequestOp::Load {
                    model_dir: load.model_dir.display().to_string(),
                    model_id: load.model_id.clone(),
                    compute_units: load.compute_units.clone(),
                },
                self.cfg.timeouts.load,
            )?;
            let r: LoadResult = parse_result(v).map_err(|e| EngineHostError::Protocol(e.0))?;
            tracing::info!(model_id = %r.model_id, load_ms = r.load_ms, compiled = r.compiled, "engine host model loaded");
            let mut st = self.state.lock().unwrap();
            st.loaded = true;
            st.last_load = Some(r);
        }
        self.state.lock().unwrap().last_activity = Instant::now();
        Ok((proc, caps))
    }

    fn record_crash(st: &mut State) {
        st.crashes.push_back(Instant::now());
    }

    fn ensure_started_locked(&self) -> Result<(Arc<HostProcess>, Capabilities)> {
        // Inspect the current process; account for an unexpected exit.
        let (backoff, dead) = {
            let mut st = self.state.lock().unwrap();
            if let Some(p) = &st.proc {
                if p.is_alive() {
                    return Ok((p.clone(), st.caps.clone().expect("caps set with proc")));
                }
            }
            let dead = st.proc.take();
            st.caps = None;
            st.loaded = false;
            if let Some((info, p)) = dead.as_ref().and_then(|p| p.closed().map(|i| (i, p))) {
                if !info.expected && p.claim_crash() {
                    tracing::warn!(status = %info.status, "engine host exited unexpectedly");
                    Self::record_crash(&mut st);
                }
            }
            if let Some(reason) = &st.failed {
                return Err(EngineHostError::Failed(reason.clone()));
            }
            let window = self.cfg.restart_window;
            st.crashes.retain(|t| t.elapsed() <= window);
            let n = st.crashes.len();
            if n > self.cfg.max_restarts {
                let reason = format!(
                    "{n} crashes within {:?}, limit is {} restarts",
                    window, self.cfg.max_restarts
                );
                st.failed = Some(reason.clone());
                return Err(EngineHostError::Failed(reason));
            }
            let backoff = if n == 0 {
                Duration::ZERO
            } else {
                let i = (n - 1).min(self.cfg.restart_backoff.len().saturating_sub(1));
                let want = self.cfg.restart_backoff.get(i).copied().unwrap_or_default();
                let since = st.crashes.back().map_or(Duration::MAX, |t| t.elapsed());
                want.saturating_sub(since)
            };
            (backoff, dead)
        };
        drop(dead); // reap the old process outside the state lock
        if !backoff.is_zero() {
            tracing::info!(?backoff, "delaying engine host restart");
            std::thread::sleep(backoff);
        }
        match self.start_and_handshake() {
            Ok((proc, caps, host_info)) => {
                let mut st = self.state.lock().unwrap();
                st.proc = Some(proc.clone());
                st.caps = Some(caps.clone());
                st.host_info = Some(host_info);
                st.last_load = None;
                st.loaded = false;
                Ok((proc, caps))
            }
            Err(e) => {
                Self::record_crash(&mut self.state.lock().unwrap());
                Err(e)
            }
        }
    }

    fn start_and_handshake(&self) -> Result<(Arc<HostProcess>, Capabilities, HostInfo)> {
        let mut args = vec!["--protocol".to_string(), PROTOCOL_VERSION.to_string()];
        args.extend(self.cfg.extra_args.iter().cloned());
        let proc = HostProcess::spawn(&self.cfg.host_path, &args, &self.cfg.env)?;
        let id = &self.cfg.identity;
        let v = proc.request(
            &RequestOp::Hello {
                client: ClientInfo {
                    name: id.name.clone(),
                    version: id.version.clone(),
                    git_sha: id.git_sha.clone(),
                },
            },
            self.cfg.timeouts.hello,
        )?;
        let hello: HelloResult = parse_result(v)
            .map_err(|e| EngineHostError::Handshake(format!("malformed hello: {}", e.0)))?;
        let caps = validate_hello(&hello, &self.cfg.identity)?;
        tracing::info!(
            pid = proc.pid(),
            host_version = %hello.host.version,
            engine = %hello.host.engine,
            "engine host started"
        );
        Ok((Arc::new(proc), caps, hello.host))
    }

    fn unload_locked(&self) -> Result<()> {
        let proc = {
            let st = self.state.lock().unwrap();
            match &st.proc {
                Some(p) if p.is_alive() => p.clone(),
                _ => return Ok(()),
            }
        };
        proc.request(&RequestOp::Unload, self.cfg.timeouts.unload)?;
        let mut st = self.state.lock().unwrap();
        st.loaded = false;
        st.last_activity = Instant::now();
        Ok(())
    }

    fn shutdown_locked(&self) {
        let proc = {
            let mut st = self.state.lock().unwrap();
            st.loaded = false;
            st.caps = None;
            st.proc.take()
        };
        if let Some(p) = proc {
            p.terminate(self.cfg.timeouts.shutdown_grace, true);
        }
    }

    /// A window timed out: if the host no longer answers `ping`, it is hung, so
    /// count a crash and kill it (the next request restarts it).
    fn after_timeout(&self, proc: &Arc<HostProcess>) {
        if !proc.is_alive() {
            return;
        }
        if proc
            .request(&RequestOp::Ping, self.cfg.timeouts.ping)
            .is_ok()
        {
            return;
        }
        tracing::warn!(
            pid = proc.pid(),
            "engine host unresponsive after timeout; killing"
        );
        if proc.claim_crash() {
            Self::record_crash(&mut self.state.lock().unwrap());
        }
        proc.terminate(Duration::from_millis(200), false);
    }

    fn idle_check(&self) {
        let (unload, shutdown) = {
            let st = self.state.lock().unwrap();
            let Some(p) = &st.proc else { return };
            if st.active > 0 || !p.is_alive() {
                return;
            }
            let idle = st.last_activity.elapsed();
            let unload = st.loaded && self.cfg.idle_unload.is_some_and(|d| idle >= d);
            let shutdown = !st.loaded && self.cfg.idle_shutdown.is_some_and(|d| idle >= d);
            (unload, shutdown)
        };
        if !unload && !shutdown {
            return;
        }
        let Ok(_g) = self.lifecycle.try_lock() else {
            return;
        };
        // Re-check under the lifecycle lock: a request may have slipped in.
        {
            let st = self.state.lock().unwrap();
            if st.active > 0 {
                return;
            }
        }
        if unload {
            tracing::info!("engine host idle; unloading model");
            if let Err(e) = self.unload_locked() {
                tracing::warn!(error = %e, "idle unload failed");
            }
        } else {
            tracing::info!("engine host idle; shutting down");
            self.shutdown_locked();
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let proc = self.state.get_mut().unwrap().proc.take();
        if let Some(p) = proc {
            p.terminate(self.cfg.timeouts.shutdown_grace, true);
        }
    }
}

fn supervise_loop(weak: Weak<Inner>, tick: Duration) {
    loop {
        std::thread::sleep(tick);
        let Some(inner) = weak.upgrade() else { return };
        inner.idle_check();
    }
}

/// Validate a `hello` result and return the usable capabilities.
pub fn validate_hello(hello: &HelloResult, client: &ClientIdentity) -> Result<Capabilities> {
    if hello.protocol != PROTOCOL_VERSION {
        return Err(EngineHostError::Handshake(format!(
            "host speaks protocol {}, client requires {PROTOCOL_VERSION}",
            hello.protocol
        )));
    }
    let Some(host_sha) = hello.host.git_sha.as_deref().filter(|s| !s.is_empty()) else {
        return Err(EngineHostError::Handshake(
            "host did not report a git_sha (build identity is mandatory)".into(),
        ));
    };
    let c = &hello.capabilities;
    if c.sample_rate == 0 || c.max_in_flight == 0 {
        return Err(EngineHostError::Handshake(
            "host capabilities have zero sample_rate or max_in_flight".into(),
        ));
    }
    let ok_window = c.max_window_s > 0.0
        && c.preferred_window_s > 0.0
        && c.preferred_window_s <= c.max_window_s
        && c.preferred_overlap_s >= 0.0
        && c.preferred_overlap_s < c.preferred_window_s;
    if !ok_window {
        return Err(EngineHostError::Handshake(format!(
            "inconsistent window capabilities (max {} s, preferred {} s, overlap {} s)",
            c.max_window_s, c.preferred_window_s, c.preferred_overlap_s
        )));
    }
    let ours = client.git_sha.as_str();
    let comparable = ours != "unknown" && !ours.is_empty();
    if comparable && !(host_sha.starts_with(ours) || ours.starts_with(host_sha)) {
        tracing::warn!(
            client = ours,
            host = host_sha,
            "engine host build differs from client build"
        );
    }
    Ok(c.clone())
}
