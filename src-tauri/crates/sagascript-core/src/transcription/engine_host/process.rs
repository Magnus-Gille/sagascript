//! One spawned host process: JSON-Lines transport and response routing.

use super::{EngineHostError, Result};
use crate::sync::{lock_or_recover, recover_poisoned};
use sagascript_engine_protocol::{
    decode_incoming, encode_request, ErrorBody, Incoming, RequestOp, MAX_LINE_BYTES,
};
use serde_json::{Map, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Size of the stderr ring buffer used for error messages.
pub const STDERR_TAIL_BYTES: usize = 8 * 1024;

/// How long one stdin write may take before the host counts as no longer
/// reading (see [`HostProcess::with_write_timeout`]). A host reads stdin on its
/// own thread, so a healthy write completes in microseconds.
pub const DEFAULT_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

// Poisoned transport locks are recovered as-is (ADR 0005): every critical
// section is a single map insert/remove, `Option` take/clone or channel send,
// byte-ring update or child `try_wait`/`kill`, so the data stays valid whatever a panic
// interrupted. Panicking instead would kill the stdout reader thread (leaving
// waiters to time out against a host that never reports closing) or turn the
// `Drop` reap into an abort while unwinding.
const ROUTING_LOCK: &str = "engine_host.routing";
const STDERR_TAIL_LOCK: &str = "engine_host.stderr_tail";
const STDIN_LOCK: &str = "engine_host.stdin";
const CHILD_LOCK: &str = "engine_host.child";
const READER_LOCK: &str = "engine_host.reader";

/// Why and how the process ended, with the stderr tail.
#[derive(Debug, Clone)]
pub struct CrashInfo {
    pub status: String,
    pub stderr_tail: String,
    /// True when the client asked for the exit (shutdown/kill/drop).
    pub expected: bool,
    /// Set when the client ended the host for breaking the protocol
    /// (for example an over-long line); in-flight requests then fail with a
    /// protocol error instead of a crash.
    pub violation: Option<String>,
    /// Set when reading the host's stdout failed with an I/O error (not EOF);
    /// in-flight requests then fail with that error.
    pub io_error: Option<String>,
}

/// What a waiter receives for a request id.
#[derive(Debug)]
pub enum Delivery {
    Event {
        name: String,
        fields: Map<String, Value>,
    },
    Response(std::result::Result<Value, ErrorBody>),
    Closed(CrashInfo),
}

#[derive(Default)]
struct Routing {
    waiters: HashMap<u64, Sender<Delivery>>,
    closed: Option<CrashInfo>,
}

struct Shared {
    routing: Mutex<Routing>,
    closed_cv: Condvar,
    stderr_tail: Mutex<VecDeque<u8>>,
    expected_exit: AtomicBool,
}

/// Outcome of [`read_bounded_line`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LineRead {
    Eof,
    /// A complete line (including its `\n`) is in the buffer.
    Line,
    /// More than `max` bytes arrived without a newline; the buffer holds at most `max` bytes.
    TooLong,
}

/// `read_until(b'\n')` that never buffers more than `max` bytes per line.
pub(crate) fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    max: usize,
) -> std::io::Result<LineRead> {
    buf.clear();
    loop {
        let available = match reader.fill_buf() {
            Ok(a) => a,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            return Ok(if buf.is_empty() {
                LineRead::Eof
            } else {
                LineRead::Line // final line without a trailing newline
            });
        }
        let (take, found) = match available.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (available.len(), false),
        };
        if buf.len() + take > max {
            let room = max - buf.len();
            buf.extend_from_slice(&available[..room]);
            return Ok(LineRead::TooLong);
        }
        buf.extend_from_slice(&available[..take]);
        reader.consume(take);
        if found {
            return Ok(LineRead::Line);
        }
    }
}

impl Shared {
    fn tail_string(&self) -> String {
        let t = lock_or_recover(&self.stderr_tail, STDERR_TAIL_LOCK);
        let bytes: Vec<u8> = t.iter().copied().collect();
        String::from_utf8_lossy(&bytes).trim().to_string()
    }
}

/// One request line for the stdin writer thread; `done` receives the outcome.
struct WriteJob {
    line: String,
    done: Sender<std::io::Result<()>>,
}

/// A pending request handle.
pub struct Pending {
    pub id: u64,
    rx: Receiver<Delivery>,
}

impl Pending {
    /// Wait up to `timeout` for the next delivery (events included).
    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> std::result::Result<Delivery, RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }
}

pub struct HostProcess {
    shared: Arc<Shared>,
    child: Arc<Mutex<Child>>,
    /// Queue to the thread that owns the host's stdin; `None` once closed.
    /// The lock only guards taking or using the sender, never a write, so a
    /// host that stops reading cannot block `send` or `terminate` on it.
    stdin: Mutex<Option<Sender<WriteJob>>>,
    write_timeout: Duration,
    next_id: AtomicU64,
    pid: u32,
    started: Instant,
    reader: Mutex<Option<JoinHandle<()>>>,
    crash_counted: AtomicBool,
}

impl HostProcess {
    pub fn spawn(path: &Path, args: &[String], env: &[(String, String)]) -> Result<Self> {
        Self::spawn_with(path, args, env, |s| Box::new(s))
    }

    /// Like `spawn`, but lets the caller wrap the host's stdout (fault injection in tests).
    pub(crate) fn spawn_with(
        path: &Path,
        args: &[String],
        env: &[(String, String)],
        wrap_stdout: impl FnOnce(ChildStdout) -> Box<dyn Read + Send + 'static>,
    ) -> Result<Self> {
        let mut cmd = Command::new(path);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // The ONNX Runtime host is a console-subsystem program: without this
        // flag Windows opens a console window for it next to the GUI app.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(|source| EngineHostError::Spawn {
            path: path.display().to_string(),
            source,
        })?;
        let pid = child.id();
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            // Unreachable with `Stdio::piped()`, but never leave the child running.
            let _ = child.kill();
            let _ = child.wait();
            return Err(EngineHostError::Spawn {
                path: path.display().to_string(),
                source: std::io::Error::other("engine host started without piped stdio"),
            });
        };
        let stdout = wrap_stdout(stdout);
        let shared = Arc::new(Shared {
            routing: Mutex::new(Routing::default()),
            closed_cv: Condvar::new(),
            stderr_tail: Mutex::new(VecDeque::with_capacity(STDERR_TAIL_BYTES)),
            expected_exit: AtomicBool::new(false),
        });

        let (stdin_tx, stdin_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("engine-host-stdin".into())
            .spawn(move || write_stdin(stdin, stdin_rx))?;

        let s = shared.clone();
        let stderr_thread = std::thread::Builder::new()
            .name("engine-host-stderr".into())
            .spawn(move || drain_stderr(stderr, &s))?;

        let child = Arc::new(Mutex::new(child));
        let s = shared.clone();
        let c = child.clone();
        let reader = std::thread::Builder::new()
            .name("engine-host-stdout".into())
            .spawn(move || read_stdout(stdout, &s, stderr_thread, &c, pid))?;

        Ok(Self {
            shared,
            child,
            stdin: Mutex::new(Some(stdin_tx)),
            write_timeout: DEFAULT_WRITE_TIMEOUT,
            next_id: AtomicU64::new(1),
            pid,
            started: Instant::now(),
            reader: Mutex::new(Some(reader)),
            crash_counted: AtomicBool::new(false),
        })
    }

    /// Bound each stdin write by `timeout` instead of [`DEFAULT_WRITE_TIMEOUT`].
    pub fn with_write_timeout(mut self, timeout: Duration) -> Self {
        self.write_timeout = timeout;
        self
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn stderr_tail(&self) -> String {
        self.shared.tail_string()
    }

    /// `Some` once the stdout stream has closed (process gone or closing).
    pub fn closed(&self) -> Option<CrashInfo> {
        lock_or_recover(&self.shared.routing, ROUTING_LOCK)
            .closed
            .clone()
    }

    /// True exactly once per process: lets the client count a crash a single time.
    pub fn claim_crash(&self) -> bool {
        !self.crash_counted.swap(true, Ordering::SeqCst)
    }

    pub fn is_alive(&self) -> bool {
        self.closed().is_none()
    }

    /// Send a request; the returned handle receives events and the response.
    ///
    /// The write is bounded by the write timeout: a host that stops reading
    /// its stdin is killed (it counts as a crash) and the request fails with
    /// [`EngineHostError::Timeout`] instead of blocking forever.
    pub fn send(&self, op: &RequestOp) -> Result<Pending> {
        self.send_within(op, self.write_timeout)
    }

    fn send_within(&self, op: &RequestOp, write_timeout: Duration) -> Result<Pending> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        {
            let mut r = lock_or_recover(&self.shared.routing, ROUTING_LOCK);
            if let Some(info) = &r.closed {
                return Err(crashed(info));
            }
            r.waiters.insert(id, tx);
        }
        let (done, written) = mpsc::channel();
        let job = WriteJob {
            line: encode_request(id, op),
            done,
        };
        let queued = lock_or_recover(&self.stdin, STDIN_LOCK)
            .as_ref()
            .is_some_and(|writer| writer.send(job).is_ok());
        let write = if queued {
            match written.recv_timeout(write_timeout) {
                Ok(result) => result,
                Err(RecvTimeoutError::Timeout) => {
                    self.forget(id);
                    tracing::error!(
                        pid = self.pid,
                        op = op.name(),
                        ?write_timeout,
                        "engine host stopped reading its stdin; killing it"
                    );
                    // Ending the process fails the blocked write, which frees
                    // the writer thread, and fails every in-flight request.
                    let _ = lock_or_recover(&self.child, CHILD_LOCK).kill();
                    return Err(EngineHostError::Timeout {
                        op: op.name(),
                        after: write_timeout,
                    });
                }
                Err(RecvTimeoutError::Disconnected) => {
                    Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
                }
            }
        } else {
            Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
        };
        if let Err(e) = write {
            self.forget(id);
            // Broken pipe means the host is going away; report the crash with its stderr.
            if let Some(info) = self.wait_closed(Duration::from_millis(1500)) {
                return Err(crashed(&info));
            }
            return Err(EngineHostError::Io(e));
        }
        Ok(Pending { id, rx })
    }

    /// Drop the waiter for `id` (late responses are discarded).
    pub fn forget(&self, id: u64) {
        lock_or_recover(&self.shared.routing, ROUTING_LOCK)
            .waiters
            .remove(&id);
    }

    fn wait_closed(&self, timeout: Duration) -> Option<CrashInfo> {
        let deadline = Instant::now() + timeout;
        let mut r = lock_or_recover(&self.shared.routing, ROUTING_LOCK);
        while r.closed.is_none() {
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            r = self
                .shared
                .closed_cv
                .wait_timeout(r, deadline - now)
                .unwrap_or_else(|poisoned| {
                    recover_poisoned(&self.shared.routing, ROUTING_LOCK, poisoned)
                })
                .0;
        }
        r.closed.clone()
    }

    /// Send a request and wait for its terminal response; `timeout` covers
    /// the write as well.
    pub fn request(&self, op: &RequestOp, timeout: Duration) -> Result<Value> {
        let name = op.name();
        let deadline = Instant::now() + timeout;
        let pending = self.send_within(op, timeout.min(self.write_timeout))?;
        loop {
            let now = Instant::now();
            if now >= deadline {
                self.forget(pending.id);
                return Err(EngineHostError::Timeout {
                    op: name,
                    after: timeout,
                });
            }
            match pending.recv_timeout(deadline - now) {
                Ok(Delivery::Event { .. }) => continue,
                Ok(Delivery::Response(Ok(v))) => return Ok(v),
                Ok(Delivery::Response(Err(e))) => return Err(remote(e)),
                Ok(Delivery::Closed(info)) => return Err(crashed(&info)),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(EngineHostError::Protocol("response channel dropped".into()))
                }
            }
        }
    }

    /// Ask the host to exit: `shutdown` request, then EOF, then kill after `grace`.
    /// Returns once the process has been reaped. Never waits on a stdin write
    /// for longer than `grace`.
    pub fn terminate(&self, grace: Duration, graceful_request: bool) {
        self.shared.expected_exit.store(true, Ordering::SeqCst);
        if graceful_request && self.is_alive() {
            let _ = self.request(&RequestOp::Shutdown, grace);
        }
        // EOF on stdin makes a well-behaved host exit on its own: the writer
        // thread closes stdin once its queue is drained. A write that is still
        // blocked is ended by the kill below.
        lock_or_recover(&self.stdin, STDIN_LOCK).take();
        let deadline = Instant::now() + grace;
        loop {
            if let Ok(Some(_)) = lock_or_recover(&self.child, CHILD_LOCK).try_wait() {
                break;
            }
            if Instant::now() >= deadline {
                tracing::warn!(pid = self.pid, "engine host ignored shutdown; killing");
                let mut c = lock_or_recover(&self.child, CHILD_LOCK);
                let _ = c.kill();
                let _ = c.wait();
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if let Some(h) = lock_or_recover(&self.reader, READER_LOCK).take() {
            let _ = h.join();
        }
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        // No orphans: EOF, short grace, then kill.
        self.terminate(Duration::from_millis(1000), false);
    }
}

pub(crate) fn crashed(info: &CrashInfo) -> EngineHostError {
    if let Some(v) = &info.violation {
        return EngineHostError::Protocol(v.clone());
    }
    if let Some(e) = &info.io_error {
        return EngineHostError::Io(std::io::Error::other(format!(
            "reading engine host stdout failed: {e}; host killed (status {}); stderr tail:\n{}",
            info.status, info.stderr_tail
        )));
    }
    EngineHostError::Crashed {
        status: info.status.clone(),
        stderr_tail: info.stderr_tail.clone(),
    }
}

pub(crate) fn remote(e: ErrorBody) -> EngineHostError {
    EngineHostError::Remote {
        code: e.code,
        message: e.message,
        retryable: e.retryable,
    }
}

/// Owns the host's stdin: writes queued lines in order and reports each
/// outcome. After the first failure every later line fails the same way.
/// Returns (closing stdin, which is EOF for the host) once every sender is gone.
fn write_stdin(mut stdin: ChildStdin, jobs: Receiver<WriteJob>) {
    let mut broken: Option<std::io::ErrorKind> = None;
    for job in jobs {
        let result = match broken {
            Some(kind) => Err(std::io::Error::from(kind)),
            None => stdin
                .write_all(job.line.as_bytes())
                .and_then(|()| stdin.flush()),
        };
        if let Err(e) = &result {
            broken.get_or_insert(e.kind());
        }
        let _ = job.done.send(result);
    }
}

fn drain_stderr(stderr: impl Read, shared: &Shared) {
    let mut reader = BufReader::new(stderr);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                {
                    let mut tail = lock_or_recover(&shared.stderr_tail, STDERR_TAIL_LOCK);
                    tail.extend(buf.iter().copied());
                    while tail.len() > STDERR_TAIL_BYTES {
                        tail.pop_front();
                    }
                }
                let line = String::from_utf8_lossy(&buf);
                tracing::debug!(target: "engine_host::stderr", "{}", line.trim_end());
            }
        }
    }
}

fn read_stdout(
    stdout: impl Read,
    shared: &Shared,
    stderr_thread: JoinHandle<()>,
    child: &Mutex<Child>,
    pid: u32,
) {
    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::new();
    let mut violation: Option<String> = None;
    let mut io_error: Option<String> = None;
    loop {
        match read_bounded_line(&mut reader, &mut buf, MAX_LINE_BYTES) {
            Ok(LineRead::Eof) => break,
            Err(e) => {
                tracing::error!(pid, error = %e, "reading engine host stdout failed; killing the host");
                io_error = Some(e.to_string());
                break;
            }
            Ok(LineRead::Line) => {}
            Ok(LineRead::TooLong) => {
                tracing::error!(pid, "engine host line exceeds 16 MiB; killing the host");
                violation = Some("engine host line exceeds 16 MiB without a newline".into());
                // Ending the process closes stdout, which then completes the
                // shutdown path below and fails every in-flight request.
                let _ = lock_or_recover(child, CHILD_LOCK).kill();
                break;
            }
        }
        let line = String::from_utf8_lossy(&buf);
        if line.trim().is_empty() {
            continue;
        }
        match decode_incoming(&line) {
            Ok(Incoming::Event { id, name, fields }) => {
                let r = lock_or_recover(&shared.routing, ROUTING_LOCK);
                if let Some(w) = r.waiters.get(&id) {
                    let _ = w.send(Delivery::Event { name, fields });
                }
            }
            Ok(Incoming::Response { id, result }) => {
                let w = lock_or_recover(&shared.routing, ROUTING_LOCK)
                    .waiters
                    .remove(&id);
                match w {
                    Some(w) => {
                        let _ = w.send(Delivery::Response(result));
                    }
                    None => {
                        tracing::debug!(pid, id, "engine host response for unknown/forgotten id")
                    }
                }
            }
            Err(e) => {
                let preview: String = line.trim().chars().take(200).collect();
                tracing::warn!(pid, error = %e, line = %preview, "ignoring malformed engine host line");
            }
        }
    }
    if io_error.is_some() {
        // The stream is unusable: end the host now and reap it so nothing is orphaned.
        let mut c = lock_or_recover(child, CHILD_LOCK);
        let _ = c.kill();
        let _ = c.wait();
    }
    // stdout closed: the host exited (or closed its stdout). Collect stderr, then status.
    let _ = stderr_thread.join();
    let deadline = Instant::now() + Duration::from_millis(500);
    let status = loop {
        match lock_or_recover(child, CHILD_LOCK).try_wait() {
            Ok(Some(st)) => break st.to_string(),
            Ok(None) if Instant::now() < deadline => {}
            Ok(None) => break "stdout closed, process still running".to_string(),
            Err(e) => break format!("status unavailable: {e}"),
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let info = CrashInfo {
        status,
        stderr_tail: shared.tail_string(),
        expected: shared.expected_exit.load(Ordering::SeqCst),
        violation,
        io_error,
    };
    let mut r = lock_or_recover(&shared.routing, ROUTING_LOCK);
    let waiters: Vec<_> = r.waiters.drain().collect();
    r.closed = Some(info.clone());
    drop(r);
    shared.closed_cv.notify_all();
    for (_, w) in waiters {
        let _ = w.send(Delivery::Closed(info.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_line_reader_splits_lines_and_bounds_memory() {
        let mut r = std::io::Cursor::new(b"ab\ncd\nlast".to_vec());
        let mut buf = Vec::new();
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 8).unwrap(),
            LineRead::Line
        );
        assert_eq!(buf, b"ab\n");
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 8).unwrap(),
            LineRead::Line
        );
        assert_eq!(buf, b"cd\n");
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 8).unwrap(),
            LineRead::Line
        );
        assert_eq!(buf, b"last");
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 8).unwrap(),
            LineRead::Eof
        );

        // Exactly max bytes including the newline is fine; one more is not.
        let mut r = std::io::Cursor::new(b"1234567\n12345678\n".to_vec());
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 8).unwrap(),
            LineRead::Line
        );
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 8).unwrap(),
            LineRead::TooLong
        );
        assert!(buf.len() <= 8);

        // Tiny BufReader capacity: the bound holds across refills.
        let mut r = BufReader::with_capacity(3, std::io::Cursor::new(vec![b'x'; 100]));
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 10).unwrap(),
            LineRead::TooLong
        );
        assert_eq!(buf.len(), 10);
    }

    /// Reader that blocks until released, then fails with a non-EOF I/O error.
    #[cfg(unix)]
    struct FailingReader(Mutex<Receiver<()>>);

    #[cfg(unix)]
    impl Read for FailingReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            let _ = self.0.lock().expect("release channel").recv();
            Err(std::io::Error::other("injected stdout failure"))
        }
    }

    /// A stdout read error is not EOF: waiters get the I/O error text and the
    /// child is killed and reaped.
    #[cfg(unix)]
    #[test]
    fn stdout_read_error_fails_waiters_and_reaps_child() {
        let (release, rx) = mpsc::channel::<()>();
        let proc = HostProcess::spawn_with(Path::new("/bin/cat"), &[], &[], |_real| {
            Box::new(FailingReader(Mutex::new(rx)))
        })
        .unwrap();
        let pending = proc.send(&RequestOp::Ping).unwrap();
        release.send(()).unwrap();

        match pending.recv_timeout(Duration::from_secs(5)) {
            Ok(Delivery::Closed(info)) => {
                assert!(info
                    .io_error
                    .as_deref()
                    .unwrap()
                    .contains("injected stdout failure"));
                match crashed(&info) {
                    EngineHostError::Io(e) => {
                        assert!(e.to_string().contains("injected stdout failure"), "{e}")
                    }
                    other => panic!("expected Io error, got {other:?}"),
                }
            }
            _ => panic!("waiter did not receive the close"),
        }
        assert!(!proc.is_alive());
        // Reaped: the exit status is already collected and it was killed.
        let status = proc.child.lock().expect("child").try_wait().unwrap();
        assert!(status.is_some_and(|s| !s.success()), "{status:?}");
        // New requests fail the same way.
        match proc.send(&RequestOp::Ping) {
            Err(EngineHostError::Io(e)) => assert!(e.to_string().contains("injected")),
            other => panic!("{:?}", other.err()),
        }
    }

    // Only the unix-gated test below poisons locks (it spawns /bin/cat).
    #[cfg(unix)]
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

    /// Every transport lock is recovered after a panic, and the Drop path
    /// (`terminate`, which also runs while unwinding) must not double-panic.
    #[cfg(unix)]
    #[test]
    fn host_process_recovers_poisoned_locks_and_drops_cleanly() {
        let proc = HostProcess::spawn(Path::new("/bin/cat"), &[], &[]).unwrap();
        poison(&proc.shared.routing);
        poison(&proc.shared.stderr_tail);
        poison(&proc.stdin);
        poison(&*proc.child);
        poison(&proc.reader);

        assert!(proc.is_alive());
        assert_eq!(proc.stderr_tail(), "");
        let dropped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(proc)));
        assert!(dropped.is_ok(), "HostProcess::drop must not panic");
    }
}
