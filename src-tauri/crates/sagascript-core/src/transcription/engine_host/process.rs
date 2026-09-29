//! One spawned host process: JSON-Lines transport and response routing.

use super::{EngineHostError, Result};
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
        let t = self.stderr_tail.lock().unwrap();
        let bytes: Vec<u8> = t.iter().copied().collect();
        String::from_utf8_lossy(&bytes).trim().to_string()
    }
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
    stdin: Mutex<Option<ChildStdin>>,
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
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(|source| EngineHostError::Spawn {
            path: path.display().to_string(),
            source,
        })?;
        let pid = child.id();
        let stdin = child.stdin.take();
        let stdout = wrap_stdout(child.stdout.take().expect("piped stdout"));
        let stderr = child.stderr.take().expect("piped stderr");
        let shared = Arc::new(Shared {
            routing: Mutex::new(Routing::default()),
            closed_cv: Condvar::new(),
            stderr_tail: Mutex::new(VecDeque::with_capacity(STDERR_TAIL_BYTES)),
            expected_exit: AtomicBool::new(false),
        });

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
            stdin: Mutex::new(stdin),
            next_id: AtomicU64::new(1),
            pid,
            started: Instant::now(),
            reader: Mutex::new(Some(reader)),
            crash_counted: AtomicBool::new(false),
        })
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
        self.shared.routing.lock().unwrap().closed.clone()
    }

    /// True exactly once per process: lets the client count a crash a single time.
    pub fn claim_crash(&self) -> bool {
        !self.crash_counted.swap(true, Ordering::SeqCst)
    }

    pub fn is_alive(&self) -> bool {
        self.closed().is_none()
    }

    /// Send a request; the returned handle receives events and the response.
    pub fn send(&self, op: &RequestOp) -> Result<Pending> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        {
            let mut r = self.shared.routing.lock().unwrap();
            if let Some(info) = &r.closed {
                return Err(crashed(info));
            }
            r.waiters.insert(id, tx);
        }
        let line = encode_request(id, op);
        let write = {
            let mut guard = self.stdin.lock().unwrap();
            match guard.as_mut() {
                Some(w) => w.write_all(line.as_bytes()).and_then(|()| w.flush()),
                None => Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
            }
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
        self.shared.routing.lock().unwrap().waiters.remove(&id);
    }

    fn wait_closed(&self, timeout: Duration) -> Option<CrashInfo> {
        let deadline = Instant::now() + timeout;
        let mut r = self.shared.routing.lock().unwrap();
        while r.closed.is_none() {
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            r = self
                .shared
                .closed_cv
                .wait_timeout(r, deadline - now)
                .unwrap()
                .0;
        }
        r.closed.clone()
    }

    /// Send a request and wait for its terminal response.
    pub fn request(&self, op: &RequestOp, timeout: Duration) -> Result<Value> {
        let name = op.name();
        let pending = self.send(op)?;
        let deadline = Instant::now() + timeout;
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
    /// Returns once the process has been reaped.
    pub fn terminate(&self, grace: Duration, graceful_request: bool) {
        self.shared.expected_exit.store(true, Ordering::SeqCst);
        if graceful_request && self.is_alive() {
            let _ = self.request(&RequestOp::Shutdown, grace);
        }
        // EOF on stdin makes a well-behaved host exit on its own.
        self.stdin.lock().unwrap().take();
        let deadline = Instant::now() + grace;
        loop {
            if let Ok(Some(_)) = self.child.lock().unwrap().try_wait() {
                break;
            }
            if Instant::now() >= deadline {
                tracing::warn!(pid = self.pid, "engine host ignored shutdown; killing");
                let mut c = self.child.lock().unwrap();
                let _ = c.kill();
                let _ = c.wait();
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if let Some(h) = self.reader.lock().unwrap().take() {
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

fn drain_stderr(stderr: impl Read, shared: &Shared) {
    let mut reader = BufReader::new(stderr);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                {
                    let mut tail = shared.stderr_tail.lock().unwrap();
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
                let _ = child.lock().unwrap().kill();
                break;
            }
        }
        let line = String::from_utf8_lossy(&buf);
        if line.trim().is_empty() {
            continue;
        }
        match decode_incoming(&line) {
            Ok(Incoming::Event { id, name, fields }) => {
                let r = shared.routing.lock().unwrap();
                if let Some(w) = r.waiters.get(&id) {
                    let _ = w.send(Delivery::Event { name, fields });
                }
            }
            Ok(Incoming::Response { id, result }) => {
                let w = shared.routing.lock().unwrap().waiters.remove(&id);
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
        let mut c = child.lock().unwrap();
        let _ = c.kill();
        let _ = c.wait();
    }
    // stdout closed: the host exited (or closed its stdout). Collect stderr, then status.
    let _ = stderr_thread.join();
    let deadline = Instant::now() + Duration::from_millis(500);
    let status = loop {
        match child.lock().unwrap().try_wait() {
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
    let mut r = shared.routing.lock().unwrap();
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
        assert_eq!(read_bounded_line(&mut r, &mut buf, 8).unwrap(), LineRead::Line);
        assert_eq!(buf, b"ab\n");
        assert_eq!(read_bounded_line(&mut r, &mut buf, 8).unwrap(), LineRead::Line);
        assert_eq!(buf, b"cd\n");
        assert_eq!(read_bounded_line(&mut r, &mut buf, 8).unwrap(), LineRead::Line);
        assert_eq!(buf, b"last");
        assert_eq!(read_bounded_line(&mut r, &mut buf, 8).unwrap(), LineRead::Eof);

        // Exactly max bytes including the newline is fine; one more is not.
        let mut r = std::io::Cursor::new(b"1234567\n12345678\n".to_vec());
        assert_eq!(read_bounded_line(&mut r, &mut buf, 8).unwrap(), LineRead::Line);
        assert_eq!(read_bounded_line(&mut r, &mut buf, 8).unwrap(), LineRead::TooLong);
        assert!(buf.len() <= 8);

        // Tiny BufReader capacity: the bound holds across refills.
        let mut r = BufReader::with_capacity(3, std::io::Cursor::new(vec![b'x'; 100]));
        assert_eq!(read_bounded_line(&mut r, &mut buf, 10).unwrap(), LineRead::TooLong);
        assert_eq!(buf.len(), 10);
    }

    /// Reader that blocks until released, then fails with a non-EOF I/O error.
    struct FailingReader(Mutex<Receiver<()>>);

    impl Read for FailingReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            let _ = self.0.lock().unwrap().recv();
            Err(std::io::Error::other("injected stdout failure"))
        }
    }

    /// A stdout read error is not EOF: waiters get the I/O error text and the
    /// child is killed and reaped.
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
                assert!(info.io_error.as_deref().unwrap().contains("injected stdout failure"));
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
        let status = proc.child.lock().unwrap().try_wait().unwrap();
        assert!(status.is_some_and(|s| !s.success()), "{status:?}");
        // New requests fail the same way.
        match proc.send(&RequestOp::Ping) {
            Err(EngineHostError::Io(e)) => assert!(e.to_string().contains("injected")),
            other => panic!("{:?}", other.err()),
        }
    }
}
