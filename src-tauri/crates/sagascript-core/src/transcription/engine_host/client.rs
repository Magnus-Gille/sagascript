//! Handshake, lifecycle (lazy start, idle unload/shutdown, restart budget)
//! and window scheduling on top of [`HostProcess`].

use super::pipeline::CancelToken;
use super::process::{crashed, Delivery, HostProcess, Pending};
use super::{EngineHostError, Result};
use crate::sync::{lock_or_recover, lock_or_recover_with, recover_poisoned};
use sagascript_engine_protocol::{
    parse_result, Capabilities, ClientInfo, ErrorCode, HelloResult, HostInfo, LoadResult, Priority,
    RequestOp, StatusResult, TranscribeWindowResult, WindowTimings, WireToken, PROTOCOL_VERSION,
};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, TryLockError, Weak};
use std::time::{Duration, Instant};

const STATE_LOCK: &str = "engine_host_client.state";
const LIFECYCLE_LOCK: &str = "engine_host_client.lifecycle";
/// Recovered as-is: every critical section is one `Option` assignment or clone.
const BOOST_LOCK: &str = "engine_host_client.boost";

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

/// Default [`Timeouts::wedge_grace`]: how long a host may take, after a
/// timed-out window was cancelled, to send that window's terminal response
/// before it counts as wedged and is killed.
///
/// A host honours `cancel` only between inference stages, and a stage can be
/// uninterruptible: a Core ML prediction always runs to its end, and when the
/// Neural Engine has evicted the compiled encoder the first prediction
/// recompiles it, which took 11.5-16.7 s in #397. The grace must exceed the
/// longest such legitimate stage with a wide margin; otherwise a healthy host
/// is killed mid-compile, the compile is lost, the next dictation pays for it
/// again and the kills use up the restart budget. Nobody waits on it: the
/// caller gets its timeout at once, and a detached watcher thread keeps the
/// window's slot until the host answers or is killed.
pub const DEFAULT_WEDGE_GRACE: Duration = Duration::from_secs(30);

/// Default [`EngineHostConfig::failed_cooldown`]. While failed, requests fail
/// at once without spawning anything; the first request after the cooldown
/// gets one fresh start (half-open). If that start fails the cooldown begins
/// again; if it succeeds the crash history is cleared.
pub const DEFAULT_FAILED_COOLDOWN: Duration = Duration::from_secs(60);

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
    /// Upper bound for writing one request to the host's stdin. A host that
    /// stops reading it is killed and the request fails with a timeout.
    pub write: Duration,
    /// After a window times out and is cancelled, how long the host gets to
    /// send that request's terminal response before it counts as wedged and
    /// is killed (see [`DEFAULT_WEDGE_GRACE`]). The caller does not wait for
    /// it; the window keeps its slot meanwhile.
    pub wedge_grace: Duration,
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
            write: super::process::DEFAULT_WRITE_TIMEOUT,
            wedge_grace: DEFAULT_WEDGE_GRACE,
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

/// Test-only synchronization hook, called with a named point of the request
/// path (`"after_ready"`). Lets tests interleave lifecycle operations
/// deterministically; production code never sets it.
#[derive(Clone)]
pub struct TestHook(pub Arc<dyn Fn(&str) + Send + Sync>);

impl std::fmt::Debug for TestHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TestHook")
    }
}

/// Model-load lifecycle reported to a [`LoadObserver`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadEvent {
    /// The host is being started and/or the model is being loaded.
    Started,
    /// The model is loaded and ready.
    Ready { load_ms: u64 },
    /// Start or load failed.
    Failed(String),
    /// The wait for a started load was cancelled. Terminal for the wait only:
    /// the host keeps loading, but observers must stop showing a pending load.
    Cancelled,
}

/// Callback told when a model load starts, finishes or fails, from whichever
/// thread drives the load (pre-warm or the first transcription).
#[derive(Clone)]
pub struct LoadObserver(pub Arc<dyn Fn(LoadEvent) + Send + Sync>);

impl std::fmt::Debug for LoadObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LoadObserver")
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
    /// How long the client stays failed after the restart budget is spent
    /// (see [`DEFAULT_FAILED_COOLDOWN`]).
    pub failed_cooldown: Duration,
    /// How often a window that was in flight during a crash is resubmitted.
    pub max_crash_retries: u32,
    pub busy_retries: u32,
    pub busy_retry_delay: Duration,
    /// Parent for the private PCM temp dir (`None`: the system temp dir).
    pub temp_dir: Option<PathBuf>,
    /// See [`TestHook`].
    #[doc(hidden)]
    pub test_hook: Option<TestHook>,
    /// Told when a model load starts, finishes or fails (see [`LoadObserver`]).
    pub load_observer: Option<LoadObserver>,
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
            failed_cooldown: DEFAULT_FAILED_COOLDOWN,
            max_crash_retries: 1,
            busy_retries: 3,
            busy_retry_delay: Duration::from_millis(50),
            temp_dir: None,
            test_hook: None,
            load_observer: None,
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
    /// Host processes spawned by this client so far (a healthy session stays at 1).
    pub spawns: u64,
    /// `load` requests sent by this client so far.
    pub loads: u64,
}

/// One transcribed window as returned by the host.
#[derive(Debug, Clone)]
pub struct WindowOutput {
    pub tokens: Vec<WireToken>,
    pub audio_s: f64,
    pub timings: WindowTimings,
    pub round_trip_ms: u64,
}

/// A `load` whose waiter was cancelled; the host keeps loading, so the next
/// caller waits on this instead of sending a second `load`.
struct LoadPending {
    pending: Pending,
    deadline: Instant,
}

/// What [`Inner::plan_start_locked`] found.
enum StartPlan {
    Running(Arc<HostProcess>, Capabilities),
    /// Spawn after `backoff`; `probing` is the one start after a failed cooldown.
    Spawn {
        backoff: Duration,
        probing: bool,
    },
}

/// Sleep `duration` in short slices; `Cancelled` as soon as `cancel` fires.
fn sleep_unless_cancelled(duration: Duration, cancel: Option<&CancelToken>) -> Result<()> {
    let end = Instant::now() + duration;
    loop {
        if cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(EngineHostError::Cancelled);
        }
        let now = Instant::now();
        if now >= end {
            return Ok(());
        }
        std::thread::sleep((end - now).min(Duration::from_millis(10)));
    }
}

/// The restart budget was spent at `since`.
struct Failure {
    reason: String,
    since: Instant,
}

struct State {
    proc: Option<Arc<HostProcess>>,
    /// Effective capabilities: `hello` capabilities bounded by the loaded
    /// model's `LoadResult.window_s` (see [`effective_capabilities`]).
    caps: Option<Capabilities>,
    /// Capabilities exactly as announced in `hello` (before any load).
    base_caps: Option<Capabilities>,
    load_pending: Option<LoadPending>,
    host_info: Option<HostInfo>,
    last_load: Option<LoadResult>,
    loaded: bool,
    crashes: VecDeque<Instant>,
    failed: Option<Failure>,
    last_activity: Instant,
    /// Slots held (batch + interactive), so idle logic never fires mid-request.
    active: usize,
    batch_in_flight: usize,
    interactive_active: usize,
}

impl State {
    /// Called when `state` was poisoned (ADR 0005). Every critical section
    /// writes related fields without a panic point in between, so the data
    /// is valid, but `loaded` may describe a load or unload the panicking
    /// thread did not finish. Clearing it sends the next request through the
    /// lifecycle-serialized readiness path, which re-checks that the host is
    /// alive (restarting it within the crash budget) and re-sends `load` (a
    /// no-op for an already loaded model). The slot counters are left alone:
    /// live `Slot`/`Reservation` guards still decrement them.
    fn revalidate_after_poison(&mut self) {
        self.loaded = false;
    }
}

pub(crate) struct Inner {
    pub(crate) cfg: EngineHostConfig,
    state: Mutex<State>,
    cv: Condvar,
    /// Serializes start / load / unload / shutdown.
    lifecycle: Mutex<()>,
    /// Dictionary applied to every window job that does not carry its own.
    boost: Mutex<Option<Arc<BoostSpec>>>,
    /// Context-biasing outcome counters (see [`BoostReport`]).
    boost_windows: AtomicU64,
    boost_inactive_windows: AtomicU64,
    boost_unusable_windows: AtomicU64,
    boost_us: AtomicU64,
    /// Pid of the host we last warned about having no interactive reservation.
    warned_no_reserve_pid: AtomicU32,
    /// Host spawns / `load` requests, for diagnostics (see [`HostSnapshot`]).
    spawns: AtomicU64,
    loads: AtomicU64,
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
                base_caps: None,
                load_pending: None,
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
            boost: Mutex::new(None),
            boost_windows: AtomicU64::new(0),
            boost_inactive_windows: AtomicU64::new(0),
            boost_unusable_windows: AtomicU64::new(0),
            boost_us: AtomicU64::new(0),
            warned_no_reserve_pid: AtomicU32::new(0),
            spawns: AtomicU64::new(0),
            loads: AtomicU64::new(0),
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

    /// Set (or clear) the context-biasing dictionary for later jobs.
    pub fn set_boost(&self, boost: Option<BoostSpec>) {
        *lock_or_recover(&self.inner.boost, BOOST_LOCK) = boost.map(Arc::new);
    }

    /// What happened to the dictionary on the windows sent so far.
    pub fn boost_report(&self) -> BoostReport {
        BoostReport {
            windows: self.inner.boost_windows.load(Ordering::Relaxed),
            inactive_windows: self.inner.boost_inactive_windows.load(Ordering::Relaxed),
            unusable_windows: self.inner.boost_unusable_windows.load(Ordering::Relaxed),
            trie_build_us: self.inner.boost_us.load(Ordering::Relaxed),
        }
    }

    pub fn boost(&self) -> Option<Arc<BoostSpec>> {
        lock_or_recover(&self.inner.boost, BOOST_LOCK).clone()
    }

    /// Start the host and load the model now (pre-warm). Returns its capabilities.
    pub fn warm(&self) -> Result<Capabilities> {
        self.inner.ensure_ready(None).map(|(_, caps)| caps)
    }

    /// Like [`Self::warm`], but returns `Cancelled` promptly when `cancel` fires.
    ///
    /// Cancelling stops the wait only: the host is not killed and keeps
    /// loading, so a first-use compile still finishes for the next caller.
    pub fn warm_with_cancel(&self, cancel: &CancelToken) -> Result<Capabilities> {
        self.inner.ensure_ready(Some(cancel)).map(|(_, caps)| caps)
    }

    /// Run one idle-supervision pass now (tests; the supervisor thread does this on a timer).
    #[doc(hidden)]
    pub fn run_idle_check_now(&self) {
        self.inner.idle_check();
    }

    pub fn snapshot(&self) -> HostSnapshot {
        let mut st = self.inner.lock_state();
        let cutoff = self.inner.cfg.restart_window;
        st.crashes.retain(|t| t.elapsed() <= cutoff);
        HostSnapshot {
            running: st.proc.as_ref().is_some_and(|p| p.is_alive()),
            loaded: st.loaded && st.proc.as_ref().is_some_and(|p| p.is_alive()),
            failed: st.failed.as_ref().map(|f| f.reason.clone()),
            pid: st.proc.as_ref().filter(|p| p.is_alive()).map(|p| p.pid()),
            crashes_in_window: st.crashes.len(),
            capabilities: st.caps.clone(),
            host: st.host_info.clone(),
            last_load: st.last_load.clone(),
            spawns: self.inner.spawns.load(Ordering::Relaxed),
            loads: self.inner.loads.load(Ordering::Relaxed),
        }
    }

    /// Start the host and complete the `hello` handshake without loading the
    /// model (diagnostics). The returned snapshot carries the host identity.
    pub fn connect(&self) -> Result<HostSnapshot> {
        {
            let _g = self.inner.lock_lifecycle_blocking();
            self.inner.ensure_started_locked(None)?;
        }
        Ok(self.snapshot())
    }

    /// Clear the `Failed` state and the crash history now, without waiting
    /// for [`EngineHostConfig::failed_cooldown`].
    pub fn reset(&self) {
        let mut st = self.inner.lock_state();
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
        let _g = self.inner.lock_lifecycle_blocking();
        self.inner.unload_locked()
    }

    /// Free model memory now if the model is loaded and nothing is using it
    /// (memory-pressure response). Returns whether an unload was performed;
    /// a busy or mid-lifecycle client is left alone. The next request reloads.
    pub fn unload_if_idle(&self) -> bool {
        let inner = &self.inner;
        {
            let st = inner.lock_state();
            if st.active > 0 || !st.loaded {
                return false;
            }
        }
        let Some(_g) = inner.try_lock_lifecycle() else {
            return false;
        };
        {
            let mut st = inner.lock_state();
            if st.active > 0 || !st.loaded {
                return false;
            }
            st.loaded = false;
        }
        if let Err(e) = inner.unload_locked() {
            tracing::warn!(error = %e, "memory-pressure unload failed");
        }
        true
    }

    /// Graceful shutdown (`shutdown` op, then kill after the grace period).
    pub fn shutdown(&self) {
        let _g = self.inner.lock_lifecycle_blocking();
        self.inner.shutdown_locked();
    }

    /// Capabilities negotiated with the running host, if any.
    pub fn capabilities(&self) -> Option<Capabilities> {
        self.inner.lock_state().caps.clone()
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
        //
        // A batch request holds an activity reservation from before the
        // readiness check until its slot is acquired, so the idle supervisor
        // can never unload the model in between.
        let (mut slot, reservation) = if req.priority == Priority::Interactive {
            (Some(inner.acquire_interactive()), None)
        } else {
            (None, Some(inner.reserve_activity()))
        };
        let (proc, caps) = inner.ensure_ready(Some(cancel))?;
        inner.hook("after_ready");
        if slot.is_none() {
            if caps.max_in_flight == 1
                && inner
                    .warned_no_reserve_pid
                    .swap(proc.pid(), Ordering::SeqCst)
                    != proc.pid()
            {
                tracing::warn!(
                    pid = proc.pid(),
                    "engine host advertises max_in_flight = 1: no slot can be reserved for \
                     interactive requests, so dictation may wait for one running batch window"
                );
            }
            slot = inner.acquire_batch(batch_lane(caps.max_in_flight), cancel, wait_slot)?;
        }
        drop(reservation);
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
            reloaded: false,
            slot: Some(slot),
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
    /// Context-biasing dictionary; `None` leaves decoding unchanged.
    pub boost: Option<Arc<BoostSpec>>,
}

/// Outcome of context biasing over the windows sent so far. A window is inactive when the host
/// did not report `boost_active: true` (an older host that ignores the fields, or a model
/// without top-K joint outputs), so the dictionary had no effect on it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BoostReport {
    /// Windows that carried a dictionary and completed.
    pub windows: u64,
    /// Windows the host could not apply the dictionary to (older host, or a model without top-K outputs).
    pub inactive_windows: u64,
    /// Windows where the host reported that no dictionary term could be tokenized.
    pub unusable_windows: u64,
    /// Total trie build time reported by the host.
    pub trie_build_us: u64,
}

impl BoostReport {
    /// Windows where biasing was in effect (it may still have changed nothing).
    pub fn active_windows(&self) -> u64 {
        self.windows
            .saturating_sub(self.inactive_windows + self.unusable_windows)
    }

    /// A user-facing warning when biasing did not (fully) take effect, else `None`.
    pub fn warning(&self) -> Option<String> {
        if self.inactive_windows > 0 {
            Some(format!(
                "Context biasing was NOT applied to {} of {} windows: the engine host does not support it \
                 (older host or a model without top-K joint outputs). Transcription ran without the dictionary.",
                self.inactive_windows, self.windows
            ))
        } else if self.unusable_windows > 0 {
            Some(format!(
                "Context biasing had no effect on {} of {} windows: none of the dictionary terms could be \
                 tokenized with the model's vocabulary.",
                self.unusable_windows, self.windows
            ))
        } else {
            None
        }
    }
}

/// Dictionary terms and logit bonus sent with each window.
#[derive(Debug, Clone, PartialEq)]
pub struct BoostSpec {
    pub terms: Vec<String>,
    pub weight: f32,
    /// Distinct terms dropped because more than `MAX_BOOST_TERMS` were given.
    pub truncated: usize,
    /// Terms dropped for exceeding `MAX_BOOST_TERM_CHARS`.
    pub too_long: usize,
}

impl BoostSpec {
    /// Bounded (<= 500 terms, <= 64 chars each) and deduplicated; `None` when it would be a no-op.
    pub fn new(terms: impl IntoIterator<Item = String>, weight: f32) -> Option<Self> {
        if !weight.is_finite() || weight <= 0.0 {
            return None;
        }
        let mut seen = std::collections::HashSet::new();
        let mut too_long = 0;
        let mut distinct: Vec<String> = Vec::new();
        for term in terms
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
        {
            if term.chars().count() > sagascript_engine_protocol::MAX_BOOST_TERM_CHARS {
                too_long += 1;
            } else if seen.insert(term.to_lowercase()) {
                distinct.push(term);
            }
        }
        let truncated = distinct
            .len()
            .saturating_sub(sagascript_engine_protocol::MAX_BOOST_TERMS);
        distinct.truncate(sagascript_engine_protocol::MAX_BOOST_TERMS);
        if distinct.is_empty() {
            return None;
        }
        Some(Self {
            terms: distinct,
            weight: weight.min(sagascript_engine_protocol::MAX_BOOST_WEIGHT),
            truncated,
            too_long,
        })
    }
}

fn window_op(req: &WindowRequest) -> RequestOp {
    RequestOp::TranscribeWindow {
        pcm_path: req.pcm_path.display().to_string(),
        offset_samples: req.offset_samples,
        num_samples: req.num_samples,
        sample_rate: req.sample_rate,
        format: "f32le".into(),
        priority: req.priority,
        boost_terms: req
            .boost
            .as_ref()
            .map(|b| b.terms.clone())
            .unwrap_or_default(),
        boost_weight: req.boost.as_ref().map_or(0.0, |b| b.weight),
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
    /// The window was already resent once after a `not_loaded` answer.
    reloaded: bool,
    /// `None` only after a timeout handed it to a [`WedgeWatch`].
    slot: Option<Slot>,
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
                if let Some(pending) = self.pending.take() {
                    // Returns at once; the host's late answer (or its kill) is
                    // awaited on a detached thread.
                    self.inner
                        .watch_timed_out_window(self.proc.clone(), pending, self.slot.take());
                }
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
                    if self.req.boost.is_some() {
                        self.inner.boost_windows.fetch_add(1, Ordering::Relaxed);
                        self.inner
                            .boost_us
                            .fetch_add(r.timings.boost_us, Ordering::Relaxed);
                        if r.boost_active != Some(true) {
                            let counter = if r.boost_reason.as_deref() == Some("no_usable_terms") {
                                &self.inner.boost_unusable_windows
                            } else {
                                &self.inner.boost_inactive_windows
                            };
                            counter.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    return Ok(WindowOutput {
                        tokens: r.tokens,
                        audio_s: r.audio_s,
                        timings: r.timings,
                        round_trip_ms: self.started.elapsed().as_millis() as u64,
                    });
                }
                Ok(Delivery::Response(Err(e))) => {
                    self.pending = None;
                    if e.code == ErrorCode::NotLoaded && !self.reloaded {
                        // The model went away between readiness and dispatch
                        // (idle unload, host restart): reload and resend once.
                        self.reloaded = true;
                        tracing::warn!("engine host reported not_loaded; reloading and resending");
                        self.inner.mark_unloaded(&self.proc);
                        let (proc, _) = self.inner.ensure_ready(Some(cancel))?;
                        self.proc = proc;
                        self.deadline = Instant::now() + self.timeout;
                        self.pending = Some(self.proc.send(&window_op(&self.req))?);
                        continue;
                    }
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
                        Inner::record_crash(&mut self.inner.lock_state());
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
///
/// Holds the client weakly: a slot kept by a [`WedgeWatch`] must not keep a
/// dropped client, and so its host, alive. Once the client is gone there is
/// no count left to update.
struct Slot {
    inner: Weak<Inner>,
    priority: Priority,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let mut st = inner.lock_state();
        st.active -= 1;
        match self.priority {
            Priority::Batch => st.batch_in_flight -= 1,
            Priority::Interactive => st.interactive_active -= 1,
        }
        st.last_activity = Instant::now();
        drop(st);
        inner.cv.notify_all();
    }
}

/// A timed-out window whose terminal response is still outstanding. It keeps
/// the window's slot until the host answers or is killed, because the host
/// really is still busy with it.
///
/// It holds the client weakly, so dropping the last client handle shuts the
/// host down at once instead of after the grace.
struct WedgeWatch {
    inner: Weak<Inner>,
    proc: Arc<HostProcess>,
    pending: Pending,
    grace: Duration,
    _slot: Option<Slot>,
}

impl WedgeWatch {
    /// Cancel the window and give the host `grace` to send its terminal
    /// response (`cancelled` or a late result). Without one the host is
    /// wedged on that request, which keeps its slot forever although `ping`
    /// is still answered inline, so count a crash and kill it; the next
    /// request restarts it. The slot is released when this returns.
    fn run(self) {
        if self.proc.is_alive() {
            // The answer to the cancel itself is not needed.
            let _ = self.proc.send(&RequestOp::Cancel {
                target: self.pending.id,
            });
        }
        let deadline = Instant::now() + self.grace;
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            match self.pending.recv_timeout(deadline - now) {
                Ok(Delivery::Event { .. }) | Err(RecvTimeoutError::Timeout) => {}
                // Answered: the host is responsive and has freed the slot.
                Ok(Delivery::Response(_)) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(Delivery::Closed(info)) => {
                    if !info.expected {
                        self.record_crash();
                    }
                    return;
                }
            }
        }
        self.proc.forget(self.pending.id);
        tracing::warn!(
            pid = self.proc.pid(),
            grace_ms = self.grace.as_millis() as u64,
            "engine host did not finish a timed-out window after cancel; killing it"
        );
        self.record_crash();
        self.proc.terminate(Duration::from_millis(200), false);
    }

    fn record_crash(&self) {
        if !self.proc.claim_crash() {
            return;
        }
        if let Some(inner) = self.inner.upgrade() {
            Inner::record_crash(&mut inner.lock_state());
        }
    }
}

/// Counts as activity (blocks idle unload) without occupying a scheduling slot.
struct Reservation {
    inner: Arc<Inner>,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut st = self.inner.lock_state();
        st.active -= 1;
        st.last_activity = Instant::now();
    }
}

/// Batch windows in flight at most: one host slot stays free for interactive work.
/// A host advertising 1 slot cannot reserve one; batch then uses that slot and an
/// interactive request is sent next, ahead of queued batch windows.
pub(crate) fn batch_lane(max_in_flight: u32) -> usize {
    match max_in_flight {
        0 | 1 => 1,
        n => n as usize - 1,
    }
}

/// `hello` capabilities bounded by what the loaded model accepts. The window
/// in `LoadResult.window_s` is authoritative; the pre-load values are defaults.
pub fn effective_capabilities(base: &Capabilities, load_window_s: f64) -> Result<Capabilities> {
    if !load_window_s.is_finite() || load_window_s <= 0.0 {
        return Err(EngineHostError::Protocol(format!(
            "load reported an invalid window_s ({load_window_s})"
        )));
    }
    let mut c = base.clone();
    c.max_window_s = c.max_window_s.min(load_window_s);
    c.preferred_window_s = c.preferred_window_s.min(c.max_window_s);
    if c.preferred_overlap_s >= c.preferred_window_s {
        c.preferred_overlap_s = c.preferred_window_s / 2.0;
    }
    Ok(c)
}

impl Inner {
    fn reserve_activity(self: &Arc<Self>) -> Reservation {
        self.lock_state().active += 1;
        Reservation {
            inner: self.clone(),
        }
    }

    fn mark_unloaded(&self, proc: &Arc<HostProcess>) {
        let mut st = self.lock_state();
        if st.proc.as_ref().is_some_and(|p| Arc::ptr_eq(p, proc)) {
            st.loaded = false;
        }
    }

    /// `state.lock()`, recovering from poison (ADR 0005). Each critical
    /// section updates `State` without a panic point between related fields,
    /// so the data is valid; [`State::revalidate_after_poison`] only stops the
    /// client from trusting a `loaded` flag the panic may have left stale.
    fn lock_state(&self) -> MutexGuard<'_, State> {
        lock_or_recover_with(&self.state, STATE_LOCK, State::revalidate_after_poison)
    }

    /// `lifecycle.lock()`, recovering from poison.
    fn lock_lifecycle_blocking(&self) -> MutexGuard<'_, ()> {
        self.lifecycle
            .lock()
            .unwrap_or_else(|poisoned| self.recover_lifecycle(poisoned))
    }

    /// A start/load/unload/shutdown was cut short by a panic. Its effects
    /// live in `state`, which every lifecycle step re-reads; additionally force
    /// the next request to re-validate the host before dispatching to it.
    fn recover_lifecycle<'a>(
        &'a self,
        poisoned: PoisonError<MutexGuard<'a, ()>>,
    ) -> MutexGuard<'a, ()> {
        let guard = recover_poisoned(&self.lifecycle, LIFECYCLE_LOCK, poisoned);
        self.lock_state().revalidate_after_poison();
        guard
    }

    /// `lifecycle.try_lock()` for opportunistic passes (idle supervision,
    /// memory pressure): `None` only while another lifecycle step runs. A
    /// poisoned lock is recovered (and the host re-validated), not mistaken
    /// for "busy", or these passes would never run again.
    fn try_lock_lifecycle(&self) -> Option<MutexGuard<'_, ()>> {
        match self.lifecycle.try_lock() {
            Ok(guard) => Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => Some(self.recover_lifecycle(poisoned)),
            Err(TryLockError::WouldBlock) => None,
        }
    }

    /// `lifecycle.lock()`, but a cancelled caller stops waiting for it.
    fn lock_lifecycle(&self, cancel: Option<&CancelToken>) -> Result<MutexGuard<'_, ()>> {
        let Some(cancel) = cancel else {
            return Ok(self.lock_lifecycle_blocking());
        };
        loop {
            match self.lifecycle.try_lock() {
                Ok(g) => return Ok(g),
                Err(TryLockError::WouldBlock) => {
                    if cancel.is_cancelled() {
                        return Err(EngineHostError::Cancelled);
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(TryLockError::Poisoned(poisoned)) => {
                    return Ok(self.recover_lifecycle(poisoned))
                }
            }
        }
    }

    /// Send `load` (or resume waiting for one a cancelled caller left running).
    fn load_locked(
        &self,
        proc: &Arc<HostProcess>,
        cancel: Option<&CancelToken>,
    ) -> Result<LoadResult> {
        let resumed = self.lock_state().load_pending.take();
        let (pending, deadline) = match resumed {
            Some(p) => (p.pending, p.deadline),
            None => {
                let load = &self.cfg.load;
                let n = self.loads.fetch_add(1, Ordering::Relaxed) + 1;
                tracing::info!(
                    pid = proc.pid(),
                    load_count = n,
                    model_id = %load.model_id,
                    "engine host: sending load"
                );
                let pending = proc.send(&RequestOp::Load {
                    model_dir: load.model_dir.display().to_string(),
                    model_id: load.model_id.clone(),
                    compute_units: load.compute_units.clone(),
                })?;
                (pending, Instant::now() + self.cfg.timeouts.load)
            }
        };
        let value = loop {
            if cancel.is_some_and(CancelToken::is_cancelled) {
                self.lock_state().load_pending = Some(LoadPending { pending, deadline });
                return Err(EngineHostError::Cancelled);
            }
            let now = Instant::now();
            if now >= deadline {
                proc.forget(pending.id);
                return Err(EngineHostError::Timeout {
                    op: "load",
                    after: self.cfg.timeouts.load,
                });
            }
            match pending.recv_timeout((deadline - now).min(Duration::from_millis(20))) {
                Ok(Delivery::Event { .. }) | Err(RecvTimeoutError::Timeout) => {}
                Ok(Delivery::Response(Ok(v))) => break v,
                Ok(Delivery::Response(Err(e))) => return Err(super::process::remote(e)),
                Ok(Delivery::Closed(info)) => return Err(crashed(&info)),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(EngineHostError::Protocol("response channel dropped".into()))
                }
            }
        };
        parse_result(value).map_err(|e| EngineHostError::Protocol(e.0))
    }

    fn acquire_interactive(self: &Arc<Self>) -> Slot {
        let mut st = self.lock_state();
        st.active += 1;
        st.interactive_active += 1;
        Slot {
            inner: Arc::downgrade(self),
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
        let mut st = self.lock_state();
        loop {
            if cancel.is_cancelled() {
                return Err(EngineHostError::Cancelled);
            }
            if st.batch_in_flight < max_in_flight && st.interactive_active == 0 {
                st.active += 1;
                st.batch_in_flight += 1;
                return Ok(Some(Slot {
                    inner: Arc::downgrade(self),
                    priority: Priority::Batch,
                }));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let slice = (deadline - now).min(Duration::from_millis(20));
            st = match self.cv.wait_timeout(st, slice) {
                Ok((st, _)) => st,
                Err(poisoned) => {
                    let (mut st, _) = recover_poisoned(&self.state, STATE_LOCK, poisoned);
                    st.revalidate_after_poison();
                    st
                }
            };
        }
    }

    fn hook(&self, point: &str) {
        if let Some(h) = &self.cfg.test_hook {
            (h.0)(point);
        }
    }

    fn live_proc(&self) -> Result<Arc<HostProcess>> {
        let st = self.lock_state();
        match &st.proc {
            Some(p) if p.is_alive() => Ok(p.clone()),
            _ => Err(EngineHostError::NotRunning),
        }
    }

    /// Lazy start + handshake + load. Fast path when everything is ready.
    /// Returns the effective capabilities (bounded by the loaded model).
    fn ensure_ready(
        &self,
        cancel: Option<&CancelToken>,
    ) -> Result<(Arc<HostProcess>, Capabilities)> {
        {
            let mut st = self.lock_state();
            if let (Some(p), Some(c)) = (&st.proc, &st.caps) {
                if st.loaded && p.is_alive() {
                    let out = (p.clone(), c.clone());
                    st.last_activity = Instant::now();
                    return Ok(out);
                }
            }
        }
        let guard = self.lock_lifecycle(cancel)?;
        if cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(EngineHostError::Cancelled);
        }
        self.notify_load(LoadEvent::Started);
        let (result, _guard) = match self.wait_restart_backoff(guard, cancel) {
            Ok(guard) => (self.start_and_load_locked(cancel), Some(guard)),
            Err(e) => (Err(e), None),
        };
        match &result {
            Ok((_, load_ms)) => self.notify_load(LoadEvent::Ready { load_ms: *load_ms }),
            Err(EngineHostError::Cancelled) => self.notify_load(LoadEvent::Cancelled),
            Err(error) => self.notify_load(LoadEvent::Failed(error.to_string())),
        }
        let (proc, _) = result?;
        let mut st = self.lock_state();
        st.last_activity = Instant::now();
        let caps = st.caps.clone().expect("caps set with proc");
        Ok((proc, caps))
    }

    /// Sleep a pending restart backoff (up to 5 s by default) with the
    /// lifecycle lock released, so shutdown, unload and the idle supervisor
    /// are not held up behind it; `Cancelled` as soon as `cancel` fires.
    /// Returns with the lock held again.
    fn wait_restart_backoff<'a>(
        &'a self,
        mut guard: MutexGuard<'a, ()>,
        cancel: Option<&CancelToken>,
    ) -> Result<MutexGuard<'a, ()>> {
        loop {
            let backoff = match self.plan_start_locked() {
                Ok(StartPlan::Spawn { backoff, .. }) if !backoff.is_zero() => backoff,
                // Running, nothing to wait for, or an error the start reports.
                _ => return Ok(guard),
            };
            drop(guard);
            tracing::info!(?backoff, "delaying engine host restart");
            sleep_unless_cancelled(backoff, cancel)?;
            guard = self.lock_lifecycle(cancel)?;
        }
    }

    fn notify_load(&self, event: LoadEvent) {
        if let Some(observer) = &self.cfg.load_observer {
            (observer.0)(event);
        }
    }

    /// Start the host if needed and load the model if not yet loaded. Returns
    /// the load time in ms (0 when nothing had to load). Lifecycle lock held.
    fn start_and_load_locked(
        &self,
        cancel: Option<&CancelToken>,
    ) -> Result<(Arc<HostProcess>, u64)> {
        let started = Instant::now();
        let (proc, _) = self.ensure_started_locked(cancel)?;
        let loaded = self.lock_state().loaded;
        if !loaded {
            let r = self.load_locked(&proc, cancel)?;
            tracing::info!(model_id = %r.model_id, load_ms = r.load_ms, window_s = r.window_s, compiled = r.compiled, "engine host model loaded");
            let mut st = self.lock_state();
            let base = st.base_caps.clone().expect("base caps set with proc");
            st.caps = Some(effective_capabilities(&base, r.window_s)?);
            st.loaded = true;
            st.last_load = Some(r);
        }
        Ok((
            proc,
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        ))
    }

    fn record_crash(st: &mut State) {
        st.crashes.push_back(Instant::now());
    }

    /// Inspect the current process (lifecycle lock held): account for an
    /// unexpected exit, apply the failed state and the crash budget, and work
    /// out the restart backoff. Calling it again changes nothing further.
    fn plan_start_locked(&self) -> Result<StartPlan> {
        let (plan, dead) = {
            let mut st = self.lock_state();
            if let Some(p) = &st.proc {
                if p.is_alive() {
                    let caps = st.caps.clone().expect("caps set with proc");
                    return Ok(StartPlan::Running(p.clone(), caps));
                }
            }
            let dead = st.proc.take();
            st.caps = None;
            st.base_caps = None;
            st.load_pending = None;
            st.loaded = false;
            if let Some((info, p)) = dead.as_ref().and_then(|p| p.closed().map(|i| (i, p))) {
                if !info.expected && p.claim_crash() {
                    tracing::warn!(status = %info.status, "engine host exited unexpectedly");
                    Self::record_crash(&mut st);
                }
            }
            // Failed, but only for the cooldown: afterwards one fresh start
            // (half-open), without a restart backoff (the cooldown was one).
            if let Some(failure) = &st.failed {
                let waited = failure.since.elapsed();
                if waited < self.cfg.failed_cooldown {
                    return Err(failed_error(failure, self.cfg.failed_cooldown - waited));
                }
                let plan = StartPlan::Spawn {
                    backoff: Duration::ZERO,
                    probing: true,
                };
                (plan, dead)
            } else {
                let window = self.cfg.restart_window;
                st.crashes.retain(|t| t.elapsed() <= window);
                let n = st.crashes.len();
                if n > self.cfg.max_restarts {
                    let failure = Failure {
                        reason: format!(
                            "{n} crashes within {:?}, limit is {} restarts",
                            window, self.cfg.max_restarts
                        ),
                        since: Instant::now(),
                    };
                    let error = failed_error(&failure, self.cfg.failed_cooldown);
                    tracing::warn!(%error, "engine host marked failed");
                    st.failed = Some(failure);
                    return Err(error);
                }
                let plan = StartPlan::Spawn {
                    backoff: self.restart_backoff(&st, n),
                    probing: false,
                };
                (plan, dead)
            }
        };
        drop(dead); // reap the old process outside the state lock
        Ok(plan)
    }

    /// Start the host if it is not running (lifecycle lock held). Callers
    /// normally slept the backoff already ([`Self::wait_restart_backoff`]);
    /// what is left is slept here, and a cancelled caller stops waiting.
    fn ensure_started_locked(
        &self,
        cancel: Option<&CancelToken>,
    ) -> Result<(Arc<HostProcess>, Capabilities)> {
        let probing = match self.plan_start_locked()? {
            StartPlan::Running(proc, caps) => return Ok((proc, caps)),
            StartPlan::Spawn { backoff, probing } => {
                if !backoff.is_zero() {
                    tracing::info!(?backoff, "delaying engine host restart");
                    sleep_unless_cancelled(backoff, cancel)?;
                }
                probing
            }
        };
        if probing {
            tracing::info!("engine host failed earlier; trying a fresh start after the cooldown");
        }
        match self.start_and_handshake() {
            Ok((proc, caps, host_info)) => {
                let mut st = self.lock_state();
                if probing {
                    tracing::info!("engine host recovered after the failed cooldown");
                    st.failed = None;
                    st.crashes.clear();
                }
                st.proc = Some(proc.clone());
                st.caps = Some(caps.clone());
                st.base_caps = Some(caps.clone());
                st.host_info = Some(host_info);
                st.last_load = None;
                st.load_pending = None;
                st.loaded = false;
                Ok((proc, caps))
            }
            Err(e) => {
                let mut st = self.lock_state();
                Self::record_crash(&mut st);
                if probing {
                    // The fresh start failed too: fail fast for another cooldown.
                    let first_line = e.to_string().lines().next().unwrap_or_default().to_string();
                    st.failed = Some(Failure {
                        reason: format!("start after the cooldown failed: {first_line}"),
                        since: Instant::now(),
                    });
                }
                Err(e)
            }
        }
    }

    /// Delay before the next start after `n` crashes in the restart window.
    fn restart_backoff(&self, st: &State, n: usize) -> Duration {
        if n == 0 {
            return Duration::ZERO;
        }
        let i = (n - 1).min(self.cfg.restart_backoff.len().saturating_sub(1));
        let want = self.cfg.restart_backoff.get(i).copied().unwrap_or_default();
        let since = st.crashes.back().map_or(Duration::MAX, |t| t.elapsed());
        want.saturating_sub(since)
    }

    fn start_and_handshake(&self) -> Result<(Arc<HostProcess>, Capabilities, HostInfo)> {
        let mut args = vec!["--protocol".to_string(), PROTOCOL_VERSION.to_string()];
        args.extend(self.cfg.extra_args.iter().cloned());
        let n = self.spawns.fetch_add(1, Ordering::Relaxed) + 1;
        // No paths: the persistent log must not carry the user's home directory.
        tracing::info!(spawn_count = n, "engine host: spawning");
        let proc = HostProcess::spawn(&self.cfg.host_path, &args, &self.cfg.env)?
            .with_write_timeout(self.cfg.timeouts.write);
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
            let st = self.lock_state();
            match &st.proc {
                Some(p) if p.is_alive() => p.clone(),
                _ => return Ok(()),
            }
        };
        self.lock_state().load_pending = None;
        proc.request(&RequestOp::Unload, self.cfg.timeouts.unload)?;
        let mut st = self.lock_state();
        st.loaded = false;
        st.last_activity = Instant::now();
        Ok(())
    }

    fn shutdown_locked(&self) {
        let proc = {
            let mut st = self.lock_state();
            st.loaded = false;
            st.caps = None;
            st.base_caps = None;
            st.load_pending = None;
            st.proc.take()
        };
        if let Some(p) = proc {
            p.terminate(self.cfg.timeouts.shutdown_grace, true);
        }
    }

    /// A window timed out: hand it to a detached [`WedgeWatch`] so the caller
    /// gets its timeout at once. If no thread can be started, the watch runs
    /// here (the caller then waits up to the wedge grace, as a fallback).
    fn watch_timed_out_window(
        self: &Arc<Self>,
        proc: Arc<HostProcess>,
        pending: Pending,
        slot: Option<Slot>,
    ) {
        let watch = WedgeWatch {
            inner: Arc::downgrade(self),
            proc,
            pending,
            grace: self.cfg.timeouts.wedge_grace,
            _slot: slot,
        };
        // Passed over a channel so the watch is still ours when the thread
        // cannot be started (or ends before receiving it).
        let (tx, rx) = mpsc::channel::<WedgeWatch>();
        let spawned = std::thread::Builder::new()
            .name("engine-host-wedge-watch".into())
            .spawn(move || {
                if let Ok(watch) = rx.recv() {
                    watch.run();
                }
            });
        let watch = match spawned {
            Ok(_) => match tx.send(watch) {
                Ok(()) => return,
                Err(mpsc::SendError(watch)) => watch,
            },
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "could not start the engine host wedge watcher; waiting on this thread"
                );
                watch
            }
        };
        watch.run();
    }

    fn idle_check(&self) {
        let (unload, shutdown) = {
            let st = self.lock_state();
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
        let Some(_g) = self.try_lock_lifecycle() else {
            return;
        };
        // Re-check under the lifecycle lock: a request may have slipped in.
        // Marking the model unloaded in the same critical section makes any
        // later request take the (lifecycle-serialized) reload path instead of
        // dispatching to a host that is about to unload.
        {
            let mut st = self.lock_state();
            if st.active > 0 {
                return;
            }
            if unload {
                st.loaded = false;
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
        // Exclusive access while dropping: a poisoned flag needs no report.
        let proc = self
            .state
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .proc
            .take();
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

fn failed_error(failure: &Failure, retry_in: Duration) -> EngineHostError {
    EngineHostError::Failed(format!(
        "{}; next attempt in {} s",
        failure.reason,
        retry_in.as_secs_f64().ceil() as u64
    ))
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

#[cfg(test)]
mod poison_tests {
    use super::*;

    fn poison<T: Send>(mutex: &Mutex<T>) {
        std::thread::scope(|scope| {
            let panicked = scope
                .spawn(|| {
                    let _guard = mutex.lock().expect("not poisoned yet");
                    panic!("injected panic while holding the lock (#359)");
                })
                .join();
            assert!(panicked.is_err());
        });
        assert!(mutex.is_poisoned());
    }

    fn client() -> EngineHostClient {
        let mut cfg = EngineHostConfig::new(
            "/nonexistent/sagascript-engine-host",
            LoadSpec {
                model_dir: PathBuf::from("/nonexistent"),
                model_id: "test".into(),
                compute_units: "cpu".into(),
            },
        );
        cfg.idle_unload = None;
        cfg.idle_shutdown = None;
        EngineHostClient::new(cfg)
    }

    /// A panic under the state lock must not panic every later request, and
    /// the client must not trust a `loaded` flag the panic may have
    /// interrupted: the next request re-validates the host first.
    #[test]
    fn poisoned_state_is_recovered_and_forces_host_revalidation() {
        let client = client();
        client.inner.state.lock().expect("not poisoned yet").loaded = true;
        poison(&client.inner.state);

        let snapshot = client.snapshot();
        assert!(!snapshot.running);
        assert!(!client.inner.state.is_poisoned());
        assert!(
            !client.inner.state.lock().expect("recovered").loaded,
            "recovery must force the next request through the readiness check"
        );

        // Slot/reservation guards drop while unwinding: no double panic.
        let reservation = client.inner.reserve_activity();
        poison(&client.inner.state);
        let dropped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(reservation)));
        assert!(dropped.is_ok(), "Reservation::drop must not panic");
        assert_eq!(client.inner.state.lock().expect("recovered").active, 0);
    }

    #[test]
    fn poisoned_lifecycle_lock_is_recovered() {
        let client = client();
        poison(&client.inner.lifecycle);
        assert!(client.unload().is_ok());

        poison(&client.inner.lifecycle);
        let started = client.warm_with_cancel(&CancelToken::new());
        assert!(
            matches!(started, Err(EngineHostError::Spawn { .. })),
            "expected the missing host to fail to spawn, got {:?}",
            started.err()
        );
    }

    /// A client whose idle passes act immediately, with a live stand-in host
    /// (`/bin/cat` never answers, so requests to it time out quickly).
    #[cfg(unix)]
    fn idle_client_with_live_host() -> EngineHostClient {
        let mut cfg = EngineHostConfig::new(
            "/nonexistent/sagascript-engine-host",
            LoadSpec {
                model_dir: PathBuf::from("/nonexistent"),
                model_id: "test".into(),
                compute_units: "cpu".into(),
            },
        );
        cfg.idle_unload = Some(Duration::ZERO);
        cfg.idle_shutdown = Some(Duration::ZERO);
        cfg.supervisor_tick = Duration::from_secs(3600); // passes run only when the test asks
        cfg.timeouts.unload = Duration::from_millis(50);
        cfg.timeouts.shutdown_grace = Duration::from_millis(50);
        let client = EngineHostClient::new(cfg);
        let host = HostProcess::spawn(std::path::Path::new("/bin/cat"), &[], &[]).unwrap();
        client.inner.lock_state().proc = Some(Arc::new(host));
        client
    }

    /// The idle supervisor and the memory-pressure unload take the lifecycle
    /// lock with `try_lock`; a poisoned lock must be recovered there, not
    /// treated as "busy" forever (no idle unload or shutdown ever again).
    #[cfg(unix)]
    #[test]
    fn idle_and_memory_pressure_passes_recover_a_poisoned_lifecycle_lock() {
        let client = idle_client_with_live_host();
        client.inner.lock_state().loaded = true;
        poison(&client.inner.lifecycle);

        // Recovery re-validates the host, so this pass finds nothing loaded…
        assert!(!client.unload_if_idle());
        assert!(!client.inner.lifecycle.is_poisoned());
        // …and the next one (after a reload marked the model loaded) unloads.
        client.inner.lock_state().loaded = true;
        assert!(client.unload_if_idle(), "memory-pressure unload runs again");

        poison(&client.inner.lifecycle);
        client.run_idle_check_now();
        assert!(!client.inner.lifecycle.is_poisoned());
        assert!(
            !client.snapshot().running,
            "the idle pass shut the host down despite the poisoned lock"
        );
    }
}

#[cfg(test)]
mod boost_spec_tests {
    use super::BoostSpec;

    #[test]
    fn off_when_weight_or_terms_are_empty() {
        assert!(BoostSpec::new(vec!["Gille".into()], 0.0).is_none());
        assert!(BoostSpec::new(vec!["Gille".into()], f32::NAN).is_none());
        assert!(BoostSpec::new(vec!["  ".into()], 3.0).is_none());
    }

    #[test]
    fn report_warns_only_when_windows_ran_without_biasing() {
        use super::BoostReport;
        assert!(BoostReport {
            windows: 3,
            inactive_windows: 0,
            unusable_windows: 0,
            trie_build_us: 5
        }
        .warning()
        .is_none());
        let warning = BoostReport {
            windows: 3,
            inactive_windows: 3,
            unusable_windows: 0,
            trie_build_us: 0,
        }
        .warning()
        .unwrap();
        assert!(warning.contains("NOT applied to 3 of 3"));
        let unusable = BoostReport {
            windows: 2,
            inactive_windows: 0,
            unusable_windows: 2,
            trie_build_us: 0,
        };
        assert!(
            unusable.warning().unwrap().contains("could be tokenized")
                && unusable.active_windows() == 0
        );
    }

    #[test]
    fn bounds_and_deduplicates_terms() {
        let long = "x".repeat(65);
        let many: Vec<String> = (0..600).map(|i| format!("Namn{i}")).collect();
        let spec = BoostSpec::new(
            vec!["Gille".into(), "gille".into(), long, "Magnus Gille".into()]
                .into_iter()
                .chain(many),
            99.0,
        )
        .unwrap();
        assert_eq!(spec.terms.len(), 500);
        assert_eq!(spec.too_long, 1);
        assert_eq!(spec.truncated, 102); // 2 distinct short terms + 600 others, minus 500
        assert_eq!(&spec.terms[..2], ["Gille", "Magnus Gille"]);
        assert_eq!(spec.weight, 20.0);
    }
}
