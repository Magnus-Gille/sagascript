//! Cooperative Ctrl-C / SIGTERM cancellation for file transcription (#234).
//!
//! The first signal sets a process-wide flag and fires every registered
//! aborter (Whisper's abort callback flag, the Pianissimo engine-host
//! `cancel`), so inference stops promptly and `run` can unwind normally, which
//! also shuts the engine host down (no orphan). A second signal exits
//! immediately with the same status as an interrupt.
use sagascript_core::error::DictationError;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Conventional exit status for termination by SIGINT (128 + 2).
pub const EXIT_CANCELLED: i32 = 130;

type Aborter = Box<dyn Fn() + Send>;

/// A cancellation flag plus the aborters to fire when it is set. The process
/// uses one global instance; tests build their own.
pub struct Canceller {
    flag: AtomicBool,
    aborters: Mutex<Vec<(u64, Aborter)>>,
    next_id: Mutex<u64>,
}

impl Canceller {
    pub const fn new() -> Self {
        Self { flag: AtomicBool::new(false), aborters: Mutex::new(Vec::new()), next_id: Mutex::new(0) }
    }

    pub fn flag(&self) -> &AtomicBool {
        &self.flag
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Err when cancellation was requested.
    pub fn check(&self) -> Result<(), DictationError> {
        if self.is_cancelled() {
            Err(DictationError::TranscriptionFailed("Cancelled".into()))
        } else {
            Ok(())
        }
    }

    /// Set the flag and fire every registered aborter. Returns whether this
    /// was the first request.
    pub fn request(&self) -> bool {
        let first = !self.flag.swap(true, Ordering::SeqCst);
        self.fire();
        first
    }

    fn fire(&self) {
        if let Ok(aborters) = self.aborters.lock() {
            for (_, abort) in aborters.iter() {
                abort();
            }
        }
    }

    /// Register `abort` to run on cancellation. If cancellation already
    /// happened it fires immediately, so there is no registration race.
    pub fn register<'a>(&'a self, abort: impl Fn() + Send + 'static) -> AborterGuard<'a> {
        let id = {
            let mut next = self.next_id.lock().unwrap();
            *next += 1;
            *next
        };
        self.aborters.lock().unwrap().push((id, Box::new(abort)));
        if self.is_cancelled() {
            self.fire();
        }
        AborterGuard { owner: self, id }
    }
}

impl Default for Canceller {
    fn default() -> Self {
        Self::new()
    }
}

/// Keeps an aborter registered until dropped.
pub struct AborterGuard<'a> {
    owner: &'a Canceller,
    id: u64,
}

impl Drop for AborterGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut aborters) = self.owner.aborters.lock() {
            aborters.retain(|(id, _)| *id != self.id);
        }
    }
}

pub(crate) static GLOBAL: Canceller = Canceller::new();
static INSTALLED: OnceLock<bool> = OnceLock::new();
static EXIT_CODE: AtomicI32 = AtomicI32::new(EXIT_CANCELLED);

/// Exit status for the signal that cancelled the run: 130 for SIGINT (and on
/// Windows), 143 for SIGTERM.
pub fn exit_code() -> i32 {
    EXIT_CODE.load(Ordering::SeqCst)
}

/// First signal: cancel cooperatively. Second: shut the engine host down within
/// a bounded wait (no orphan), then exit.
fn on_signal(exit_code: i32) {
    if GLOBAL.is_cancelled() {
        let _ = writeln!(std::io::stderr(), "\nCancelled (forced exit).");
        shutdown_engine_host_bounded(Duration::from_millis(1500));
        std::process::exit(EXIT_CODE.load(Ordering::SeqCst));
    }
    EXIT_CODE.store(exit_code, Ordering::SeqCst);
    let _ = writeln!(std::io::stderr(), "\nCancelling... (press Ctrl-C again to force quit)");
    GLOBAL.request();
}

fn shutdown_engine_host_bounded(limit: Duration) {
    let (done, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        sagascript_core::transcription::pianissimo_backend::shutdown_shared_client();
        let _ = done.send(());
    });
    let _ = wait.recv_timeout(limit);
}

/// Install the signal handler once. Failure to install is non-fatal: the
/// default signal disposition still terminates the process.
pub fn install() {
    INSTALLED.get_or_init(install_handler);
}

#[cfg(unix)]
fn install_handler() -> bool {
    use signal_hook::consts::{SIGINT, SIGTERM};
    let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGINT, SIGTERM]) else {
        return false;
    };
    std::thread::spawn(move || {
        for signal in signals.forever() {
            on_signal(if signal == SIGTERM { 143 } else { EXIT_CANCELLED });
        }
    });
    true
}

#[cfg(not(unix))]
fn install_handler() -> bool {
    ctrlc::set_handler(|| on_signal(EXIT_CANCELLED)).is_ok()
}

pub fn flag() -> &'static AtomicBool {
    GLOBAL.flag()
}

pub fn is_cancelled() -> bool {
    GLOBAL.is_cancelled()
}

pub fn check() -> Result<(), DictationError> {
    GLOBAL.check()
}

pub fn register(abort: impl Fn() + Send + 'static) -> AborterGuard<'static> {
    GLOBAL.register(abort)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn request_fires_registered_aborters_and_sets_the_flag() {
        let canceller = Canceller::new();
        let hits = Arc::new(AtomicBool::new(false));
        let seen = hits.clone();
        let _guard = canceller.register(move || seen.store(true, Ordering::SeqCst));
        assert!(canceller.check().is_ok());
        assert!(canceller.request());
        assert!(!canceller.request(), "second request is reported as repeat");
        assert!(hits.load(Ordering::SeqCst));
        assert_eq!(canceller.check().unwrap_err().to_string(), "Transcription failed: Cancelled");
    }

    #[test]
    fn dropped_guard_is_not_fired_and_late_registration_fires_at_once() {
        let canceller = Canceller::new();
        let hits = Arc::new(AtomicBool::new(false));
        let seen = hits.clone();
        drop(canceller.register(move || seen.store(true, Ordering::SeqCst)));
        canceller.request();
        assert!(!hits.load(Ordering::SeqCst));
        let late = Arc::new(AtomicBool::new(false));
        let seen = late.clone();
        let _guard = canceller.register(move || seen.store(true, Ordering::SeqCst));
        assert!(late.load(Ordering::SeqCst), "cancel before registration must still abort");
    }
}
