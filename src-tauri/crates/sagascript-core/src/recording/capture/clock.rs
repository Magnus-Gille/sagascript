//! Clocks (design §4.3).
//!
//! * The **host clock** stamps the first frame of every block and the anchors
//!   `(host_ns, frame_index)` in the journal. Its domain is named in the
//!   manifest (`clock.domain`): `mach_continuous` on macOS, which counts
//!   through sleep; `qpc` on Windows; `monotonic` elsewhere. A block is stamped
//!   with the time the callback ran minus the stream's own capture latency
//!   (see `mic.rs`), not with the stream's timestamp: `cpal` derives that from
//!   `mach_absolute_time`, which *stops* during sleep, so it would not be in
//!   the domain the manifest names.
//! * The **session clock** ([`SessionClock`]) is what the writer measures its
//!   own timeouts with (the 500 ms watchdog, the 10 s fsync and anchor
//!   cadence, disk polling). In production it is the host clock; a test uses
//!   [`ManualClock`], which moves only when told to, so a two-hour soak needs
//!   no real time.
//!
//! On Windows the domain is `std::time::Instant`, which is
//! `QueryPerformanceCounter` there; its origin is the first use in the
//! process, which is enough to align tracks captured by one process (phase 2)
//! but not to compare with another process's QPC. That is the documented
//! limit of this seam.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The name of the host clock's domain, as written to the manifest.
#[cfg(target_vendor = "apple")]
pub const HOST_CLOCK_DOMAIN: &str = "mach_continuous";
#[cfg(windows)]
pub const HOST_CLOCK_DOMAIN: &str = "qpc";
#[cfg(not(any(target_vendor = "apple", windows)))]
pub const HOST_CLOCK_DOMAIN: &str = "monotonic";

#[cfg(target_vendor = "apple")]
mod host {
    use std::sync::OnceLock;

    /// `struct mach_timebase_info` of `<mach/mach_time.h>`. (`libc` declares
    /// these too but marks them deprecated in favour of the `mach2` crate,
    /// which this crate does not depend on.)
    #[repr(C)]
    struct TimebaseInfo {
        numer: u32,
        denom: u32,
    }

    extern "C" {
        /// Ticks since boot, including time asleep. Readable from any thread
        /// without a lock; a commpage read.
        fn mach_continuous_time() -> u64;
        /// Ticks since boot, not counting time asleep.
        fn mach_absolute_time() -> u64;
        fn mach_timebase_info(info: *mut TimebaseInfo) -> i32;
    }

    fn timebase() -> (u64, u64) {
        static TIMEBASE: OnceLock<(u64, u64)> = OnceLock::new();
        *TIMEBASE.get_or_init(|| {
            let mut info = TimebaseInfo { numer: 0, denom: 0 };
            // SAFETY: `info` is a valid out-pointer for the duration of the call.
            let status = unsafe { mach_timebase_info(&mut info) };
            if status != 0 || info.denom == 0 {
                (1, 1)
            } else {
                (u64::from(info.numer), u64::from(info.denom))
            }
        })
    }

    fn to_ns(ticks: u64) -> u64 {
        let (numer, denom) = timebase();
        (u128::from(ticks) * u128::from(numer) / u128::from(denom)) as u64
    }

    pub(super) fn now_ns() -> u64 {
        // SAFETY: a plain syscall-free libSystem function with no arguments.
        to_ns(unsafe { mach_continuous_time() })
    }

    /// `mach_absolute_time`, which does not advance while the Mac sleeps.
    pub(super) fn awake_ns() -> u64 {
        // SAFETY: as above.
        to_ns(unsafe { mach_absolute_time() })
    }
}

#[cfg(not(target_vendor = "apple"))]
mod host {
    use std::sync::OnceLock;
    use std::time::Instant;

    pub(super) fn now_ns() -> u64 {
        static ORIGIN: OnceLock<Instant> = OnceLock::new();
        // Never zero: a start stamp of 0 would read as "no stamp".
        ORIGIN.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1
    }
}

/// The host clock now, in nanoseconds of [`HOST_CLOCK_DOMAIN`].
///
/// Realtime-safe once it has been called once (the timebase is cached): no
/// allocation, no lock.
pub fn host_now_ns() -> u64 {
    host::now_ns()
}

/// How far the clock that counts through sleep is ahead of the one that does
/// not. It grows by the length of every sleep and by nothing else, which is
/// how a sleep is noticed after the fact without a notification API. `None`
/// where there is no such pair of clocks (the seam for a Windows or Linux
/// implementation; a notification registration would also fit here).
pub fn sleep_offset_ns() -> Option<u64> {
    #[cfg(target_vendor = "apple")]
    {
        Some(host::now_ns().saturating_sub(host::awake_ns()))
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        None
    }
}

/// Notices a system sleep from the growth of [`sleep_offset_ns`].
#[derive(Debug)]
pub struct SleepDetector {
    last_offset: Option<u64>,
    threshold_ns: u64,
}

impl SleepDetector {
    /// A sleep shorter than `threshold` is not noticed.
    pub fn new(threshold: Duration) -> Self {
        Self {
            last_offset: None,
            threshold_ns: threshold.as_nanos() as u64,
        }
    }

    /// Feed an offset; true when it has grown by more than the threshold since
    /// the last one. The first observation only sets the baseline.
    pub fn observe(&mut self, offset_ns: u64) -> bool {
        let slept = match self.last_offset {
            Some(last) => offset_ns.saturating_sub(last) > self.threshold_ns,
            None => false,
        };
        self.last_offset = Some(offset_ns);
        slept
    }

    /// [`observe`](Self::observe) with the real clocks; false where they do not exist.
    pub fn poll(&mut self) -> bool {
        sleep_offset_ns().is_some_and(|offset| self.observe(offset))
    }
}

/// What the writer measures its timeouts with, and the host time it stamps
/// stop-time gaps with.
pub trait SessionClock: Send + Sync {
    /// Monotonic nanoseconds since an arbitrary origin.
    fn now_ns(&self) -> u64;
    /// The host clock (the domain of block stamps) now.
    fn host_ns(&self) -> u64;
}

/// The real clocks.
#[derive(Debug)]
pub struct SystemClock {
    origin: std::time::Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionClock for SystemClock {
    fn now_ns(&self) -> u64 {
        self.origin.elapsed().as_nanos() as u64
    }

    fn host_ns(&self) -> u64 {
        host_now_ns()
    }
}

/// A clock that moves only when told to; the host clock is the same clock
/// plus a fixed base, so block stamps and timeouts agree.
#[derive(Debug)]
pub struct ManualClock {
    now: AtomicU64,
}

/// The host time a [`ManualClock`] reads at time zero.
pub const MANUAL_HOST_BASE_NS: u64 = 1_000_000_000_000;

impl ManualClock {
    pub fn new() -> Self {
        Self {
            now: AtomicU64::new(0),
        }
    }

    /// Move time forward.
    pub fn advance(&self, by: Duration) {
        self.now.fetch_add(by.as_nanos() as u64, Ordering::SeqCst);
    }

    /// Move time forward by `ns` nanoseconds.
    pub fn advance_ns(&self, ns: u64) {
        self.now.fetch_add(ns, Ordering::SeqCst);
    }
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionClock for ManualClock {
    fn now_ns(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }

    fn host_ns(&self) -> u64 {
        MANUAL_HOST_BASE_NS + self.now.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
#[path = "clock_tests.rs"]
mod tests;
