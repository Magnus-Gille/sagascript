//! Where a file run says what it is doing (#374): one line at a time on the run's diagnostic
//! stream, which is stderr, and, for a long decode, a progress bar.
//!
//! The pipeline narrates through a [`Narrator`] instead of writing to a stream of its own, so the
//! CLI hands it its output layer (which keeps its stderr, its capture in tests and its decode
//! bar), and a meeting job in the app hands it [`StderrNarrator`], which is the process's stderr
//! without a bar. Narration is best effort: a stderr that cannot be written never fails a run.

use std::fmt;
use std::io::Write;
use std::sync::Arc;

/// A progress bar for the decoding of a large file, drawn by whoever owns the terminal.
pub trait DecodeBar: Send + Sync {
    /// The decoder is at `percent` of the file.
    fn decoding(&self, percent: u8);
    /// The resampler is at `percent` of the decoded audio.
    fn resampling(&self, percent: u8);
    /// The decode is over; take the bar down.
    fn finish(&self);
}

/// The diagnostic stream of a run. `Sync` because progress callbacks of other threads write too.
pub trait Narrator: Send + Sync {
    /// Writes `args` and a newline to the diagnostic stream, like `eprintln!`. Best effort.
    fn err_line(&self, args: fmt::Arguments<'_>);

    /// An owned handle to the same stream, for a callback that outlives a borrow.
    fn handle(&self) -> Arc<dyn Narrator>;

    /// A progress bar for decoding a file of `input_bytes`, or none (the default). The pipeline
    /// asks once per decode.
    fn decode_bar(&self, input_bytes: u64) -> Option<Box<dyn DecodeBar>> {
        let _ = input_bytes;
        None
    }
}

/// The process's stderr, line by line, and no progress bar. What a meeting job in the app narrates
/// to: the lines are the ones the CLI writes for the same run.
#[derive(Clone, Copy, Debug, Default)]
pub struct StderrNarrator;

impl Narrator for StderrNarrator {
    fn err_line(&self, args: fmt::Arguments<'_>) {
        let mut text = args.to_string();
        text.push('\n');
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(text.as_bytes());
        let _ = stderr.flush();
    }

    fn handle(&self) -> Arc<dyn Narrator> {
        Arc::new(*self)
    }
}

/// `eprintln!` through a [`Narrator`]: `errln!(out, "{x}")` writes one line to the diagnostic
/// stream, best effort.
#[allow(unused_macros)]
macro_rules! errln {
    ($out:expr) => {
        $out.err_line(format_args!(""))
    };
    ($out:expr, $($arg:tt)+) => {
        $out.err_line(format_args!($($arg)+))
    };
}

#[allow(unused_imports)]
pub(crate) use errln;
