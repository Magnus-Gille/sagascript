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
use std::sync::OnceLock;
use std::time::Duration;

/// Conventional exit status for termination by SIGINT (128 + 2).
pub const EXIT_CANCELLED: i32 = 130;

pub use sagascript_core::meeting::control::{AborterGuard, Canceller};

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
    let _ = writeln!(
        std::io::stderr(),
        "\nCancelling... (press Ctrl-C again to force quit)"
    );
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
            on_signal(if signal == SIGTERM {
                143
            } else {
                EXIT_CANCELLED
            });
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
