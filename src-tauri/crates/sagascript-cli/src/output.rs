//! The CLI's output layer (#364).
//!
//! Every byte a subcommand writes goes through an [`Out`]: the result on
//! stdout, everything else (confirmations, warnings, `--progress-json` events)
//! on stderr. One writer per stream gives the CLI a single place that
//!
//! - survives a reader that stops early (`sagascript … | head -1`): a broken
//!   stdout pipe makes every later stdout write fail fast with
//!   [`WriteError::Closed`], the command unwinds through `?`, and
//!   [`crate::run_with`] ends the run quietly with exit 0 instead of the
//!   `failed printing to stdout` panic (exit 101) of `println!`. A command
//!   that failed for another reason keeps its failure and status, and a check
//!   writes its report through [`ignore_closed`] so a reader that left cannot
//!   hide its verdict. A stdout that is not open at all (`>&-`) is neither: the
//!   write fails with `EBADF`, which std swallows, so the command exits 0. Only
//!   a real I/O error (a full disk) prints `Error: could not write to stdout`
//!   and exits 1;
//! - remembers whether stdout was used, so a failed `--json`/`--jsonl` run can
//!   still report a structured error object ([`crate::exit`]);
//! - lets tests run a command in-process and read both streams
//!   ([`Out::capture`]) instead of spawning the binary.
//!
//! Commands receive `&Out` from `crate::run_with` and write with [`outln!`],
//! [`errln!`], [`Out::json`] and [`Out::jsonl`]; `println!`/`eprintln!` are
//! denied in this crate (clippy `print_stdout`/`print_stderr`). Two writers
//! deliberately stay outside: `tracing` logs and the `indicatif` progress bars,
//! which draw on stderr only when it is a terminal.
//!
//! Bytes are unchanged from the `println!` era: a line is its text plus `\n`,
//! `--json` documents are `serde_json::to_string_pretty` plus `\n`, `--jsonl`
//! items are `serde_json::to_string` plus `\n`, flushed per item.

use std::fmt;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use indicatif::{ProgressBar, ProgressStyle};
use sagascript_core::error::DictationError;
use sagascript_core::meeting::pipeline::narrate::{DecodeBar, Narrator};
use sagascript_core::sync::lock_or_recover;
use serde::Serialize;

// Poisoned stream locks are recovered (ADR 0005): every critical section is
// one write or flush of a complete line or document, or one assignment.
const STDOUT_LOCK: &str = "cli.out.stdout";
const STDERR_LOCK: &str = "cli.out.stderr";
const STDOUT_ERROR_LOCK: &str = "cli.out.stdout_error";

type Stream = Mutex<Box<dyn Write + Send>>;

/// The two output streams of one CLI run. Cloning is cheap and shares the
/// streams, so worker threads and progress callbacks can write too.
#[derive(Clone)]
pub struct Out {
    shared: Arc<Shared>,
}

struct Shared {
    stdout: Stream,
    stderr: Stream,
    /// A stdout write was attempted.
    stdout_used: AtomicBool,
    /// The reader closed stdout; nothing more is written there.
    stdout_closed: AtomicBool,
    /// The first stdout failure other than a closed reader (a full disk, say).
    stdout_error: Mutex<Option<String>>,
}

/// A stdout write that did not happen.
#[derive(Debug)]
pub enum WriteError {
    /// The reader closed stdout (`… | head -1`). Not a failure of the
    /// command: [`crate::run_with`] ends the run quietly with exit 0, unless
    /// the command failed for another reason ([`crate::exit::conclude`]).
    Closed,
    /// Any other I/O failure; the run fails with exit 1.
    Io(io::Error),
    /// The value could not be encoded as JSON.
    Encode(serde_json::Error),
}

/// What [`WriteError::Closed`] says, and what it reads as once `?` has turned
/// it into a [`DictationError`].
const CLOSED_MESSAGE: &str = "stdout was closed by its reader";

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => f.write_str(CLOSED_MESSAGE),
            Self::Io(error) => write!(f, "could not write to stdout: {error}"),
            Self::Encode(error) => write!(f, "could not encode JSON output: {error}"),
        }
    }
}

impl std::error::Error for WriteError {}

/// Lets code that returns `DictationError` write with `?`. The message is
/// never what the user sees for a closed or failing stdout: `run_with` reads
/// those from the [`Out`] itself. [`DictationError`] is shared with the app and
/// has no variant for a CLI's stdout, so a closed stdout stays recognizable by
/// [`is_closed_stdout`] instead; `CliError::from` turns it back into
/// `CliError::StdoutClosed`.
impl From<WriteError> for DictationError {
    fn from(error: WriteError) -> Self {
        DictationError::TranscriptionFailed(error.to_string())
    }
}

/// Whether `error` is a [`WriteError::Closed`] that `?` turned into a
/// [`DictationError`]: the command stopped because nobody reads its stdout,
/// not because it failed.
pub(crate) fn is_closed_stdout(error: &DictationError) -> bool {
    matches!(error, DictationError::TranscriptionFailed(message) if message == CLOSED_MESSAGE)
}

/// For the report of a check: a reader that left does not change the verdict,
/// so a closed stdout is not an error here and the command still returns its
/// own result (`CliError::CheckFailed`). Any other write failure stays one.
pub(crate) fn ignore_closed(result: Result<(), WriteError>) -> Result<(), WriteError> {
    match result {
        Err(WriteError::Closed) => Ok(()),
        other => other,
    }
}

impl Out {
    /// The process's stdout and stderr.
    pub fn stdio() -> Self {
        Self::new(io::stdout(), io::stderr())
    }

    /// Any pair of writers, for example a pipe or a test double.
    pub fn new(stdout: impl Write + Send + 'static, stderr: impl Write + Send + 'static) -> Self {
        Self {
            shared: Arc::new(Shared {
                stdout: Mutex::new(Box::new(stdout)),
                stderr: Mutex::new(Box::new(stderr)),
                stdout_used: AtomicBool::new(false),
                stdout_closed: AtomicBool::new(false),
                stdout_error: Mutex::new(None),
            }),
        }
    }

    /// In-memory streams; the returned [`Captured`] reads what was written.
    pub fn capture() -> (Self, Captured) {
        let captured = Captured::default();
        let out = Self::new(captured.stdout.clone(), captured.stderr.clone());
        (out, captured)
    }

    /// `stdout` as given and an in-memory stderr, to test how a command
    /// reacts to a stdout that fails.
    pub fn capture_stderr(stdout: impl Write + Send + 'static) -> (Self, Captured) {
        let captured = Captured::default();
        let out = Self::new(stdout, captured.stderr.clone());
        (out, captured)
    }

    /// Writes `args` and a newline to stdout, like `println!`.
    pub fn line(&self, args: fmt::Arguments<'_>) -> Result<(), WriteError> {
        let mut text = args.to_string();
        text.push('\n');
        self.bytes(text.as_bytes())
    }

    /// Writes `bytes` to stdout unchanged.
    pub fn bytes(&self, bytes: &[u8]) -> Result<(), WriteError> {
        self.write_stdout(|stdout| stdout.write_all(bytes))
    }

    /// Writes one pretty-printed JSON document and a newline (`--json`).
    pub fn json(&self, value: &impl Serialize) -> Result<(), WriteError> {
        let mut text = serde_json::to_string_pretty(value).map_err(WriteError::Encode)?;
        text.push('\n');
        self.bytes(text.as_bytes())
    }

    /// Writes one compact JSON line and flushes it, so a consumer sees each
    /// item as soon as it is ready (`--jsonl`).
    pub fn jsonl(&self, value: &impl Serialize) -> Result<(), WriteError> {
        let mut text = serde_json::to_string(value).map_err(WriteError::Encode)?;
        text.push('\n');
        self.write_stdout(|stdout| {
            stdout.write_all(text.as_bytes())?;
            stdout.flush()
        })
    }

    /// Flushes stdout. A no-op once stdout is closed or failed.
    pub fn flush(&self) -> Result<(), WriteError> {
        if self.stdout_closed() {
            return Err(WriteError::Closed);
        }
        let result = lock_or_recover(&self.shared.stdout, STDOUT_LOCK).flush();
        result.map_err(|error| self.stdout_failed(error))
    }

    /// Writes `args` and a newline to stderr, like `eprintln!`. Diagnostics
    /// are best effort: a stderr that cannot be written never fails a command.
    pub fn err_line(&self, args: fmt::Arguments<'_>) {
        let mut text = args.to_string();
        text.push('\n');
        self.err_text(&text);
    }

    /// Writes `text` to stderr as it is, like `eprint!` (an in-place
    /// progress redraw starting with `\r`). Best effort, like
    /// [`Out::err_line`].
    pub fn err_text(&self, text: &str) {
        let mut stderr = lock_or_recover(&self.shared.stderr, STDERR_LOCK);
        let _ = stderr.write_all(text.as_bytes());
        let _ = stderr.flush();
    }

    /// Whether anything was written (or attempted) on stdout.
    pub fn stdout_used(&self) -> bool {
        self.shared.stdout_used.load(Ordering::SeqCst)
    }

    /// Whether the reader closed stdout.
    pub fn stdout_closed(&self) -> bool {
        self.shared.stdout_closed.load(Ordering::SeqCst)
    }

    /// The first stdout failure other than a closed reader.
    pub fn stdout_error(&self) -> Option<String> {
        lock_or_recover(&self.shared.stdout_error, STDOUT_ERROR_LOCK).clone()
    }

    fn write_stdout(
        &self,
        write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
    ) -> Result<(), WriteError> {
        if self.stdout_closed() {
            return Err(WriteError::Closed);
        }
        self.shared.stdout_used.store(true, Ordering::SeqCst);
        let result = write(&mut **lock_or_recover(&self.shared.stdout, STDOUT_LOCK));
        result.map_err(|error| self.stdout_failed(error))
    }

    fn stdout_failed(&self, error: io::Error) -> WriteError {
        if error.kind() == io::ErrorKind::BrokenPipe {
            self.shared.stdout_closed.store(true, Ordering::SeqCst);
            return WriteError::Closed;
        }
        lock_or_recover(&self.shared.stdout_error, STDOUT_ERROR_LOCK)
            .get_or_insert_with(|| error.to_string());
        WriteError::Io(error)
    }
}

/// The file pipeline narrates through the CLI's own output layer (#374): its lines go to this
/// stderr, and a large decode gets the byte-fraction bar that stderr draws when it is a terminal.
impl Narrator for Out {
    fn err_line(&self, args: fmt::Arguments<'_>) {
        Out::err_line(self, args);
    }

    fn handle(&self) -> Arc<dyn Narrator> {
        Arc::new(self.clone())
    }

    /// A bar for a decode of more than 8 MB (#237): smaller files decode sub-second, so a bar
    /// would only flicker.
    fn decode_bar(&self, input_bytes: u64) -> Option<Box<dyn DecodeBar>> {
        (input_bytes > 8_000_000).then(|| {
            let bar = ProgressBar::new(100);
            bar.set_style(ProgressStyle::with_template("  Decoding [{bar:40}] {pos}%").unwrap());
            Box::new(IndicatifDecodeBar(bar)) as Box<dyn DecodeBar>
        })
    }
}

/// The decode bar of [`Out`]: decoding and then resampling, each from 0 to 100 %.
struct IndicatifDecodeBar(ProgressBar);

impl DecodeBar for IndicatifDecodeBar {
    fn decoding(&self, percent: u8) {
        self.0.set_position(u64::from(percent));
    }

    fn resampling(&self, percent: u8) {
        if percent == 0 {
            self.0
                .set_style(ProgressStyle::with_template("  Resampling [{bar:40}] {pos}%").unwrap());
        }
        self.0.set_position(u64::from(percent));
    }

    fn finish(&self) {
        self.0.finish_and_clear();
    }
}

/// `println!` through an [`Out`]: `outln!(out, "{x}")?` writes one line to
/// stdout and returns `Result<(), WriteError>`.
macro_rules! outln {
    ($out:expr) => {
        $out.line(format_args!(""))
    };
    ($out:expr, $($arg:tt)+) => {
        $out.line(format_args!($($arg)+))
    };
}

/// `eprintln!` through an [`Out`]: `errln!(out, "{x}")` writes one line to
/// stderr, best effort.
macro_rules! errln {
    ($out:expr) => {
        $out.err_line(format_args!(""))
    };
    ($out:expr, $($arg:tt)+) => {
        $out.err_line(format_args!($($arg)+))
    };
}

pub(crate) use {errln, outln};

/// What a captured [`Out`] wrote.
#[derive(Clone, Default)]
pub struct Captured {
    stdout: SharedBuffer,
    stderr: SharedBuffer,
}

impl Captured {
    /// Everything written to stdout, lossily decoded.
    pub fn stdout(&self) -> String {
        self.stdout.text()
    }

    /// Everything written to stderr, lossily decoded.
    pub fn stderr(&self) -> String {
        self.stderr.text()
    }
}

#[derive(Clone, Default)]
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

const CAPTURE_LOCK: &str = "cli.out.capture";

impl SharedBuffer {
    fn text(&self) -> String {
        String::from_utf8_lossy(&lock_or_recover(&self.0, CAPTURE_LOCK)).into_owned()
    }
}

impl Write for SharedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        lock_or_recover(&self.0, CAPTURE_LOCK).extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Stdout doubles for tests of the failure paths.
#[cfg(test)]
pub(crate) mod test_writers {
    use std::io::{self, Write};

    /// A stdout whose reader has gone away (`… | head -1` after `head` exited).
    pub(crate) struct ClosedPipe;

    impl Write for ClosedPipe {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A stdout redirected to a full disk.
    pub(crate) struct FullDisk;

    impl Write for FullDisk {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("No space left on device"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_writers::{ClosedPipe, FullDisk};
    use super::*;

    #[test]
    fn lines_and_json_keep_the_println_bytes() {
        let (out, captured) = Out::capture();
        outln!(out, "{:<6}|", "key").unwrap();
        outln!(out).unwrap();
        out.json(&serde_json::json!({"text": "hi", "n": 1}))
            .unwrap();
        out.jsonl(&serde_json::json!({"status": "ok"})).unwrap();
        errln!(out, "Warning: {}", 1);

        assert_eq!(
            captured.stdout(),
            "key   |\n\n{\n  \"n\": 1,\n  \"text\": \"hi\"\n}\n{\"status\":\"ok\"}\n"
        );
        assert_eq!(captured.stderr(), "Warning: 1\n");
        assert!(out.stdout_used());
        assert!(!out.stdout_closed());
    }

    #[test]
    fn stderr_only_output_leaves_stdout_unused() {
        let (out, captured) = Out::capture();
        errln!(out, "Saved");
        assert!(!out.stdout_used());
        assert_eq!(captured.stdout(), "");
    }

    #[test]
    fn a_closed_reader_is_remembered_and_later_writes_fail_fast() {
        let out = Out::new(ClosedPipe, io::sink());
        assert!(matches!(outln!(out, "first"), Err(WriteError::Closed)));
        assert!(out.stdout_closed());
        assert!(matches!(out.jsonl(&1), Err(WriteError::Closed)));
        assert!(matches!(out.flush(), Err(WriteError::Closed)));
        assert_eq!(out.stdout_error(), None, "a closed reader is not a failure");
    }

    #[test]
    fn other_stdout_failures_are_recorded_once() {
        let out = Out::new(FullDisk, io::sink());
        assert!(matches!(outln!(out, "first"), Err(WriteError::Io(_))));
        assert!(matches!(outln!(out, "second"), Err(WriteError::Io(_))));
        assert!(!out.stdout_closed());
        assert_eq!(
            out.stdout_error().as_deref(),
            Some("No space left on device")
        );
    }

    #[test]
    fn a_closed_stdout_is_recognized_after_it_became_a_dictation_error() {
        let error = DictationError::from(WriteError::Closed);
        assert!(is_closed_stdout(&error));
        assert!(!is_closed_stdout(&DictationError::TranscriptionFailed(
            "engine doctor failed".into()
        )));
        assert!(!is_closed_stdout(&DictationError::from(WriteError::Io(
            io::Error::other("No space left on device")
        ))));
    }

    #[test]
    fn ignore_closed_drops_only_a_closed_stdout() {
        assert!(ignore_closed(Err(WriteError::Closed)).is_ok());
        assert!(ignore_closed(Ok(())).is_ok());
        assert!(matches!(
            ignore_closed(Err(WriteError::Io(io::Error::other("disk full")))),
            Err(WriteError::Io(_))
        ));
    }

    #[test]
    fn stderr_failures_never_fail_a_command() {
        let out = Out::new(io::sink(), ClosedPipe);
        errln!(out, "Warning");
        assert!(outln!(out, "result").is_ok());
    }

    #[test]
    fn clones_share_the_streams() {
        let (out, captured) = Out::capture();
        let worker = out.clone();
        std::thread::spawn(move || outln!(worker, "from a worker").unwrap())
            .join()
            .unwrap();
        assert_eq!(captured.stdout(), "from a worker\n");
        assert!(out.stdout_used());
    }
}
