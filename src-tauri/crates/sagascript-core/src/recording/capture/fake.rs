//! [`FakeSource`]: a capture source that needs no hardware (ADR 0004).
//!
//! The test (or a child process of a test) drives it through a
//! [`FakeHandle`]: *it* decides when audio arrives, on a [`ManualClock`] that
//! moves only when told to. Every event the design names can be injected:
//! an overrun (frames the callback had to drop), a stall (time passes, no
//! blocks), a device that is lost for a while or replaced, a format change, a
//! system sleep. The audio is a pure function of the frame index, so a test
//! can compute exactly what a track should hold.
//!
//! It is real code in the library, not `cfg(test)`, because the integration
//! tests (a process killed in the middle of a session) and the phase 1c tests
//! of the CLI and the app drive sessions with it too. Nothing in a production
//! path constructs one.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::block::NativeFormat;
use super::clock::{ManualClock, SessionClock};
use super::sink::{BlockSink, SignalHandle, SourceSignal};
use super::source::CaptureSource;
use crate::recording::RecordingError;
use crate::sync::lock_or_recover;

const FAKE_LOCK: &str = "recording.fake_source";

/// The audio a [`FakeSource`] produces: a function of the frame index only.
#[derive(Clone, Debug)]
pub enum FakeSignal {
    Silence,
    /// A sine of `hz` and `amplitude` (0.0 to 1.0), the same on every channel.
    Sine {
        hz: f64,
        amplitude: f32,
    },
    /// Deterministic noise of `amplitude`, different on every channel.
    Noise {
        amplitude: f32,
        seed: u64,
    },
    /// A fixture: interleaved samples in the source's format, played from the
    /// start and then looped.
    Fixture(Arc<Vec<f32>>),
}

impl FakeSignal {
    /// The sample of `channel` at absolute frame `index`.
    pub fn sample(&self, index: u64, channel: usize, format: NativeFormat) -> f32 {
        match self {
            Self::Silence => 0.0,
            Self::Sine { hz, amplitude } => {
                let t = index as f64 / f64::from(format.sample_rate);
                (f64::from(*amplitude) * (std::f64::consts::TAU * hz * t).sin()) as f32
            }
            Self::Noise { amplitude, seed } => {
                let mut x = index
                    .wrapping_mul(2)
                    .wrapping_add(channel as u64)
                    .wrapping_add(*seed)
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15);
                x ^= x >> 29;
                x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
                x ^= x >> 32;
                let unit = (x >> 40) as f32 / (1u64 << 24) as f32; // [0, 1)
                (unit * 2.0 - 1.0) * amplitude
            }
            Self::Fixture(samples) => {
                let channels = usize::from(format.channels);
                if samples.is_empty() {
                    return 0.0;
                }
                let frames = samples.len() / channels;
                samples[(index as usize % frames) * channels + channel]
            }
        }
    }
}

struct FakeState {
    format: NativeFormat,
    signal: FakeSignal,
    sink: Option<BlockSink>,
    handle: Option<SignalHandle>,
    /// Frames of audio time that have passed (delivered or dropped).
    position: u64,
    delivered_frames: u64,
    dropped_frames: u64,
    deliveries: u64,
}

struct FakeShared {
    clock: Arc<ManualClock>,
    state: Mutex<FakeState>,
}

/// The source. Give it to the session; keep the [`FakeHandle`].
pub struct FakeSource {
    shared: Arc<FakeShared>,
    device: Option<String>,
    prime: Duration,
}

/// The test's control of a [`FakeSource`]; cheap to clone.
#[derive(Clone)]
pub struct FakeHandle {
    shared: Arc<FakeShared>,
}

impl FakeSource {
    /// A source with a clock of its own (reachable as [`FakeHandle::clock`]).
    pub fn new(format: NativeFormat) -> (Self, FakeHandle) {
        Self::with_clock(format, Arc::new(ManualClock::new()))
    }

    /// A source on a clock the test shares with the session.
    pub fn with_clock(format: NativeFormat, clock: Arc<ManualClock>) -> (Self, FakeHandle) {
        let shared = Arc::new(FakeShared {
            clock,
            state: Mutex::new(FakeState {
                format,
                signal: FakeSignal::Sine {
                    hz: 440.0,
                    amplitude: 0.5,
                },
                sink: None,
                handle: None,
                position: 0,
                delivered_frames: 0,
                dropped_frames: 0,
                deliveries: 0,
            }),
        });
        (
            Self {
                shared: Arc::clone(&shared),
                device: Some("Fake microphone".into()),
                prime: Duration::from_millis(20),
            },
            FakeHandle { shared },
        )
    }

    /// How much audio [`start`](CaptureSource::start) delivers at once, like a
    /// device whose first callback comes straight away (default 20 ms; zero
    /// delivers nothing, so the start times out unless the test delivers).
    pub fn with_prime(mut self, prime: Duration) -> Self {
        self.prime = prime;
        self
    }

    pub fn with_device_name(mut self, name: Option<String>) -> Self {
        self.device = name;
        self
    }
}

impl CaptureSource for FakeSource {
    fn format(&self) -> NativeFormat {
        lock_or_recover(&self.shared.state, FAKE_LOCK).format
    }

    fn device_name(&self) -> Option<String> {
        self.device.clone()
    }

    fn start(&mut self, sink: BlockSink) -> Result<(), RecordingError> {
        {
            let mut state = lock_or_recover(&self.shared.state, FAKE_LOCK);
            state.handle = Some(sink.handle());
            state.sink = Some(sink);
        }
        if !self.prime.is_zero() {
            FakeHandle {
                shared: Arc::clone(&self.shared),
            }
            .deliver(self.prime);
        }
        Ok(())
    }

    fn stop(&mut self) {
        let mut state = lock_or_recover(&self.shared.state, FAKE_LOCK);
        state.sink = None;
        state.handle = None;
    }
}

fn frames_in(duration: Duration, rate: u32) -> u64 {
    (duration.as_nanos() * u128::from(rate) / 1_000_000_000) as u64
}

fn nanos_of(frames: u64, rate: u32) -> u64 {
    (u128::from(frames) * 1_000_000_000 / u128::from(rate)) as u64
}

impl FakeHandle {
    /// The clock the source stamps blocks with.
    pub fn clock(&self) -> Arc<ManualClock> {
        Arc::clone(&self.shared.clock)
    }

    pub fn format(&self) -> NativeFormat {
        lock_or_recover(&self.shared.state, FAKE_LOCK).format
    }

    pub fn set_signal(&self, signal: FakeSignal) {
        lock_or_recover(&self.shared.state, FAKE_LOCK).signal = signal;
    }

    /// The signal as the track should hold it at `frame..frame + frames`
    /// (interleaved), for assertions.
    pub fn expected(&self, frame: u64, frames: u64) -> Vec<f32> {
        let state = lock_or_recover(&self.shared.state, FAKE_LOCK);
        let channels = usize::from(state.format.channels);
        let mut out = Vec::with_capacity(frames as usize * channels);
        for index in frame..frame + frames {
            for channel in 0..channels {
                out.push(state.signal.sample(index, channel, state.format));
            }
        }
        out
    }

    /// Audio arrives: `duration` of the signal as one block-sized callback at
    /// the clock's host time, then the clock moves past it. Returns false when
    /// the source is not running.
    pub fn deliver(&self, duration: Duration) -> bool {
        let rate = self.format().sample_rate;
        self.deliver_frames(frames_in(duration, rate))
    }

    /// [`deliver`](Self::deliver) by frame count.
    pub fn deliver_frames(&self, frames: u64) -> bool {
        let mut state = lock_or_recover(&self.shared.state, FAKE_LOCK);
        let format = state.format;
        let channels = usize::from(format.channels);
        let host_ns = self.shared.clock.host_ns();
        let position = state.position;
        let signal = state.signal.clone();
        let Some(sink) = state.sink.as_mut() else {
            return false;
        };
        sink.push_with(frames as usize, host_ns, |i, out| {
            for (channel, slot) in out.iter_mut().enumerate().take(channels) {
                *slot = signal.sample(position + i as u64, channel, format);
            }
        });
        state.position += frames;
        state.delivered_frames += frames;
        state.deliveries += 1;
        drop(state);
        self.shared
            .clock
            .advance_ns(nanos_of(frames, format.sample_rate));
        true
    }

    /// `total` of audio in callbacks of `chunk` each.
    pub fn deliver_in_chunks(&self, total: Duration, chunk: Duration) {
        let rate = self.format().sample_rate;
        let mut remaining = frames_in(total, rate);
        let chunk = frames_in(chunk, rate).max(1);
        while remaining > 0 {
            let take = remaining.min(chunk);
            self.deliver_frames(take);
            remaining -= take;
        }
    }

    /// Time passes and the device delivers nothing (a stall). The audio that
    /// would have arrived is lost: the generator moves on with the clock.
    pub fn stall(&self, duration: Duration) {
        let mut state = lock_or_recover(&self.shared.state, FAKE_LOCK);
        let rate = state.format.sample_rate;
        state.position += frames_in(duration, rate);
        drop(state);
        self.shared.clock.advance(duration);
    }

    /// Only the clock moves (the audio does not exist yet): the idle time
    /// between two deliveries, enough for a watchdog to look.
    pub fn idle(&self, duration: Duration) {
        self.shared.clock.advance(duration);
    }

    /// The callback dropped `frames` frames because the writer was behind.
    pub fn overrun(&self, frames: u64) {
        let mut state = lock_or_recover(&self.shared.state, FAKE_LOCK);
        let rate = state.format.sample_rate;
        if let Some(sink) = state.sink.as_mut() {
            sink.drop_frames(frames);
        }
        state.position += frames;
        state.dropped_frames += frames;
        drop(state);
        self.shared.clock.advance_ns(nanos_of(frames, rate));
    }

    fn signal(&self, signal: SourceSignal) {
        let handle = lock_or_recover(&self.shared.state, FAKE_LOCK)
            .handle
            .clone();
        if let Some(handle) = handle {
            handle.signal(signal);
        }
    }

    /// The device stopped delivering and may come back.
    pub fn lose_device(&self) {
        self.signal(SourceSignal::DeviceLost);
    }

    /// The device was unplugged or replaced.
    pub fn change_device(&self) {
        self.signal(SourceSignal::DeviceChanged);
    }

    /// The device now delivers another format.
    pub fn change_format(&self, format: NativeFormat) {
        self.signal(SourceSignal::FormatChanged(format));
    }

    /// The system goes to sleep for `duration` (the clock keeps counting, as
    /// `mach_continuous_time` does) and wakes up.
    pub fn sleep(&self, duration: Duration) {
        self.signal(SourceSignal::SystemSleep);
        let mut state = lock_or_recover(&self.shared.state, FAKE_LOCK);
        let rate = state.format.sample_rate;
        state.position += frames_in(duration, rate);
        drop(state);
        self.shared.clock.advance(duration);
    }

    /// Frames delivered to the sink (not counting dropped ones).
    pub fn frames_delivered(&self) -> u64 {
        lock_or_recover(&self.shared.state, FAKE_LOCK).delivered_frames
    }

    /// Frames lost through [`overrun`](Self::overrun).
    pub fn frames_dropped(&self) -> u64 {
        lock_or_recover(&self.shared.state, FAKE_LOCK).dropped_frames
    }

    /// Frames of audio time that have passed, delivered or not.
    pub fn position(&self) -> u64 {
        lock_or_recover(&self.shared.state, FAKE_LOCK).position
    }

    /// Stop the source the way `CaptureSource::stop` does (for a test that
    /// stands in for the session's own stopper).
    pub fn stop_source(&self) {
        let mut state = lock_or_recover(&self.shared.state, FAKE_LOCK);
        state.sink = None;
        state.handle = None;
    }

    /// Whether the source is started and not stopped.
    pub fn running(&self) -> bool {
        lock_or_recover(&self.shared.state, FAKE_LOCK)
            .sink
            .is_some()
    }
}

#[cfg(test)]
#[path = "fake_tests.rs"]
mod tests;
