//! Capture: how audio gets from a device to the recording store (SAGA-563,
//! design §4.1-§4.3).
//!
//! ```text
//!  audio callback (realtime)                    writer thread
//!  ┌───────────────────────┐   Block (pooled)   ┌───────────────────┐
//!  │ CaptureSource         │ ─── spsc queue ──► │ TrackReceiver     │
//!  │   └ BlockSink         │ ◄── spsc pool ──── │   (see session/)  │
//!  │ peaks, drops, signals │ ─── atomics ─────► │                   │
//!  └───────────────────────┘                    └───────────────────┘
//! ```
//!
//! | Piece | What it is |
//! |---|---|
//! | [`block`] | [`Block`] (a pooled buffer) and [`NativeFormat`] |
//! | `spsc` | the bounded lock-free queue (the one `unsafe` of the recorder) |
//! | [`sink`] | [`BlockSink`] (callback end), [`TrackReceiver`] (writer end), signals |
//! | [`source`] | the [`CaptureSource`] trait |
//! | [`clock`] | the host clock and its domain, the session clock, sleep detection |
//! | [`fake`] | [`FakeSource`]: every session test runs on it |
//! | [`synthetic`] | [`SyntheticSource`]: a real-time sine on its own thread, for the CLI's tests |
//! | `mic` | [`MicSource`] over `cpal` (feature `record`) |
//!
//! The module is portable: only `mic` needs the `record` feature.

pub mod block;
pub mod clock;
pub mod fake;
pub mod sink;
pub mod source;
pub(crate) mod spsc;
pub mod synthetic;
#[cfg(windows)]
mod win;

#[cfg(feature = "record")]
pub mod mic;

pub use block::{Block, NativeFormat};
pub use clock::{
    host_now_ns, sleep_offset_ns, system_sleep_probe, ManualClock, SessionClock, SleepDetector,
    SleepProbe, SystemClock, HOST_CLOCK_DOMAIN, MANUAL_HOST_BASE_NS,
};
pub use fake::{FakeHandle, FakeSignal, FakeSource};
#[cfg(feature = "record")]
pub use mic::{MicConfig, MicSource};
pub use sink::{
    track_channel, BlockSink, ChannelConfig, SignalHandle, Signals, SourceSignal, TrackReceiver,
};
pub use source::CaptureSource;
pub use synthetic::{
    test_source_requested, SyntheticConfig, SyntheticSource, SYNTHETIC_DEVICE_NAME, TEST_SOURCE_ENV,
};
