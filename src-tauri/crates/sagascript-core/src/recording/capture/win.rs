//! Windows only: how long the machine has slept, from two clocks.
//!
//! `QueryInterruptTime` counts 100 ns units since boot *including* time spent
//! asleep; `QueryUnbiasedInterruptTime` does not. Their difference grows by the
//! length of every sleep and by nothing else, which is the same trick the macOS
//! build plays with `mach_continuous_time` and `mach_absolute_time`
//! (`capture/clock.rs`). `QueryInterruptTime` needs Windows 10; the beta
//! targets Windows 11.
//!
//! This file depends on `windows-sys` and `std` only, so it can be compiled
//! on its own for a Windows target.

use windows_sys::Win32::System::WindowsProgramming::{
    QueryInterruptTime, QueryUnbiasedInterruptTime,
};

/// Nanoseconds the machine has slept since boot, or `None` if the clocks
/// cannot be read.
pub(super) fn sleep_offset_ns() -> Option<u64> {
    let mut biased: u64 = 0;
    let mut unbiased: u64 = 0;
    // SAFETY: both are valid out-pointers for the duration of the calls.
    let ok = unsafe {
        QueryInterruptTime(&mut biased);
        QueryUnbiasedInterruptTime(&mut unbiased)
    };
    (ok != 0).then(|| biased.saturating_sub(unbiased).saturating_mul(100))
}
