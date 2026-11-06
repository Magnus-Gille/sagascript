//! [`SyntheticSource`]: a sine delivered in **real time**, with no hardware
//! (SAGA-564).
//!
//! Where [`FakeSource`](super::FakeSource) is driven by a test that owns the
//! clock, this source runs by itself on a thread of its own: every 10 ms it
//! hands the session 10 ms of a sine, so a recording over it takes as long as
//! the audio it holds. That is what the CLI's integration tests need (spawn
//! the real binary, let it record for a second, stop it) and what a developer
//! needs to exercise the recorder on a machine without a microphone.
//!
//! It is real code in the library, free of `cpal` (it compiles without the
//! `record` feature), but nothing in a production path constructs one except
//! through [`test_source_requested`], which reads `SAGASCRIPT_TEST_SOURCE`
//! (`synthetic` or `synthetic:<hz>`) and answers `None` in a release build
//! unless the `dev-overrides` feature is on, the same rule as
//! `SAGASCRIPT_ENGINE_HOST`. The app never reads the variable.
//!
//! Blocks are stamped `start + frames_sent / rate` on the host clock (the way
//! a device's own timestamps advance), not with the time the thread happened
//! to wake, so a busy machine makes a burst, not a false gap.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::block::NativeFormat;
use super::clock::host_now_ns;
use super::sink::BlockSink;
use super::source::CaptureSource;
use crate::recording::RecordingError;

/// The environment variable that asks for a synthetic source.
pub const TEST_SOURCE_ENV: &str = "SAGASCRIPT_TEST_SOURCE";

/// The name of the device a synthetic source reports.
pub const SYNTHETIC_DEVICE_NAME: &str = "Synthetic microphone";

/// How long one block of a synthetic source is.
const BLOCK: Duration = Duration::from_millis(10);

/// What a synthetic source plays.
#[derive(Clone, Debug, PartialEq)]
pub struct SyntheticConfig {
    pub hz: f64,
    /// 0.0 to 1.0.
    pub amplitude: f32,
    pub format: NativeFormat,
}

impl Default for SyntheticConfig {
    fn default() -> Self {
        Self {
            hz: 440.0,
            amplitude: 0.5,
            format: NativeFormat {
                sample_rate: 48_000,
                channels: 1,
            },
        }
    }
}

/// A real-time sine on its own thread; see the module documentation.
pub struct SyntheticSource {
    config: SyntheticConfig,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SyntheticSource {
    pub fn new(config: SyntheticConfig) -> Self {
        Self {
            config,
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }
}

impl Default for SyntheticSource {
    fn default() -> Self {
        Self::new(SyntheticConfig::default())
    }
}

impl CaptureSource for SyntheticSource {
    fn format(&self) -> NativeFormat {
        self.config.format
    }

    fn device_name(&self) -> Option<String> {
        Some(SYNTHETIC_DEVICE_NAME.to_string())
    }

    fn start(&mut self, mut sink: BlockSink) -> Result<(), RecordingError> {
        if self.thread.is_some() {
            return Err(RecordingError::InvalidInput {
                reason: "the source was already started",
            });
        }
        self.stop.store(false, Ordering::Release);
        let stop = Arc::clone(&self.stop);
        let config = self.config.clone();
        let thread = std::thread::Builder::new()
            .name("recording-synthetic".into())
            .spawn(move || {
                let rate = u64::from(config.format.sample_rate);
                let channels = usize::from(config.format.channels);
                let block_frames = (rate * BLOCK.as_millis() as u64 / 1000).max(1);
                let origin = Instant::now();
                let start_ns = host_now_ns();
                let mut sent: u64 = 0;
                let mut blocks: u32 = 0;
                while !stop.load(Ordering::Acquire) {
                    let host_ns = start_ns + sent * 1_000_000_000 / rate;
                    let first = sent;
                    sink.push_with(block_frames as usize, host_ns, |i, out| {
                        let t = (first + i as u64) as f64 / rate as f64;
                        let value = (f64::from(config.amplitude)
                            * (std::f64::consts::TAU * config.hz * t).sin())
                            as f32;
                        out.iter_mut().take(channels).for_each(|slot| *slot = value);
                    });
                    sent += block_frames;
                    blocks += 1;
                    // Wait for the moment the next block would exist.
                    let due = origin + BLOCK * blocks;
                    if let Some(wait) = due.checked_duration_since(Instant::now()) {
                        std::thread::sleep(wait);
                    }
                }
            })
            .map_err(|_| RecordingError::SourceUnavailable {
                reason: "could not start the synthetic source",
            })?;
        self.thread = Some(thread);
        Ok(())
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for SyntheticSource {
    fn drop(&mut self) {
        CaptureSource::stop(self);
    }
}

/// `SAGASCRIPT_TEST_SOURCE` as a development build reads it: `synthetic` or
/// `synthetic:<hz>`. `None` when the variable is unset or not understood, and
/// always in a release build without the `dev-overrides` feature.
pub fn test_source_requested() -> Option<SyntheticConfig> {
    let enabled = cfg!(debug_assertions) || cfg!(feature = "dev-overrides");
    parse_test_source(enabled, std::env::var(TEST_SOURCE_ENV).ok().as_deref())
}

fn parse_test_source(enabled: bool, value: Option<&str>) -> Option<SyntheticConfig> {
    let value = value.map(str::trim).filter(|v| !v.is_empty())?;
    if !enabled {
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            tracing::warn!(
                variable = TEST_SOURCE_ENV,
                "ignoring development override in a release build (rebuild with the `dev-overrides` feature to use it)"
            );
        });
        return None;
    }
    let mut config = SyntheticConfig::default();
    match value.strip_prefix("synthetic")? {
        "" => {}
        rest => {
            let hz: f64 = rest.strip_prefix(':')?.parse().ok()?;
            let nyquist = f64::from(config.format.sample_rate) / 2.0;
            if !(hz.is_finite() && hz > 0.0 && hz < nyquist) {
                return None;
            }
            config.hz = hz;
        }
    }
    Some(config)
}

#[cfg(test)]
#[path = "synthetic_tests.rs"]
mod tests;
