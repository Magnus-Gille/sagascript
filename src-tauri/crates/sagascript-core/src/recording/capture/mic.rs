//! [`MicSource`]: the default input device through `cpal` (design §4.2, D7).
//!
//! It delivers the device's **native** format: the rate the device runs at
//! and 1 or 2 channels as it delivers them. A device with more than two
//! channels (a multi-input interface) is reduced to a pair chosen by
//! [`MicConfig::channel_pair`], default channels 1 and 2; the rest are not
//! stored. Nothing is mixed, scaled or resampled here; the only conversion is
//! of the device's sample type to `f32` (an exact scaling for 16- and 24-bit
//! integers).
//!
//! The existing dictation capture in `audio/capture.rs` is untouched and
//! stays what `record -o` and the hotkey use.
//!
//! # Threads
//!
//! `cpal::Stream` is `!Send`, so a thread of its own builds, plays and drops
//! it (like `capture.rs`). The data callback runs on `cpal`'s audio thread
//! and only calls [`BlockSink::push_with`]: no allocation, lock or wait. The
//! stream's error callback runs elsewhere and only sets a bit through the
//! [`SignalHandle`](super::SignalHandle).
//!
//! # Time
//!
//! A block is stamped with `host_now_ns()` at the start of the callback minus
//! the latency `cpal` reports between the capture and the callback. `cpal`'s
//! own stream timestamps are not used as absolute times: on macOS they come
//! from `mach_absolute_time`, which stops while the Mac sleeps, and the
//! manifest names `mach_continuous` (design §4.3, checked in this phase).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use super::block::NativeFormat;
use super::clock::host_now_ns;
use super::sink::{BlockSink, SourceSignal};
use super::source::CaptureSource;
use crate::recording::RecordingError;

/// Which input to record.
#[derive(Clone, Debug)]
pub struct MicConfig {
    /// For a device with more than two channels: the pair to store, counted
    /// from 1 (default `(1, 2)`). `(n, n)` stores channel `n` as mono. A mono
    /// device ignores it.
    pub channel_pair: (u16, u16),
}

impl Default for MicConfig {
    fn default() -> Self {
        Self {
            channel_pair: (1, 2),
        }
    }
}

/// How a device delivers its samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

/// What [`negotiate`] decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Negotiated {
    /// The format of the blocks.
    pub format: NativeFormat,
    /// Zero-based device channels to copy (the second equals the first for mono).
    pub pair: (usize, usize),
    pub device_channels: usize,
}

/// Decide what to store for a device (D7), without touching the device.
pub fn negotiate(device: DeviceFormat, pair: (u16, u16)) -> Result<Negotiated, RecordingError> {
    if device.sample_rate == 0 || device.channels == 0 {
        return Err(RecordingError::SourceUnavailable {
            reason: "the input device reports no usable format",
        });
    }
    let device_channels = usize::from(device.channels);
    if device.channels == 1 {
        return Ok(Negotiated {
            format: NativeFormat {
                sample_rate: device.sample_rate,
                channels: 1,
            },
            pair: (0, 0),
            device_channels,
        });
    }
    let (a, b) = pair;
    if a == 0 || b == 0 || a > device.channels || b > device.channels {
        return Err(RecordingError::InvalidInput {
            reason: "the channel pair is outside the device's channels",
        });
    }
    Ok(Negotiated {
        format: NativeFormat {
            sample_rate: device.sample_rate,
            channels: if a == b { 1 } else { 2 },
        },
        pair: (usize::from(a - 1), usize::from(b - 1)),
        device_channels,
    })
}

/// The default input device as a capture source.
pub struct MicSource {
    config: MicConfig,
    negotiated: Negotiated,
    device_name: Option<String>,
    sample_format: SampleFormat,
    running: Option<Running>,
}

struct Running {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl MicSource {
    /// Look at the default input device and decide the format. Opens no
    /// stream, so it does not prompt for the microphone permission (that
    /// happens in [`start`](CaptureSource::start)).
    pub fn open(config: MicConfig) -> Result<Self, RecordingError> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or(RecordingError::SourceUnavailable {
                reason: "no input device",
            })?;
        let supported =
            device
                .default_input_config()
                .map_err(|_| RecordingError::SourceUnavailable {
                    reason: "the input device reports no format",
                })?;
        let negotiated = negotiate(
            DeviceFormat {
                sample_rate: supported.sample_rate().0,
                channels: supported.channels(),
            },
            config.channel_pair,
        )?;
        Ok(Self {
            config,
            negotiated,
            device_name: device.name().ok(),
            sample_format: supported.sample_format(),
            running: None,
        })
    }

    /// Which channels are stored.
    pub fn negotiated(&self) -> Negotiated {
        self.negotiated
    }
}

impl CaptureSource for MicSource {
    fn format(&self) -> NativeFormat {
        self.negotiated.format
    }

    fn device_name(&self) -> Option<String> {
        self.device_name.clone()
    }

    fn start(&mut self, sink: BlockSink) -> Result<(), RecordingError> {
        if self.running.is_some() {
            return Err(RecordingError::InvalidInput {
                reason: "the microphone is already running",
            });
        }
        // The same gate as the dictation capture: never enter cpal after an
        // explicit denial, and let the first use prompt.
        let permission = crate::audio::permission::microphone_permission();
        let effective = crate::audio::permission::capture_permission_status(permission.status)
            .map_err(|_| RecordingError::PermissionDenied {
                action: "open the microphone",
            })?;
        if effective.capture_denied() {
            return Err(RecordingError::PermissionDenied {
                action: "open the microphone",
            });
        }

        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel();
        let job = StreamJob {
            expected: self.negotiated,
            expected_device: DeviceFormat {
                sample_rate: self.negotiated.format.sample_rate,
                channels: self.negotiated.device_channels as u16,
            },
            sample_format: self.sample_format,
            channel_pair: self.config.channel_pair,
            stop: Arc::clone(&stop),
        };
        let thread = std::thread::Builder::new()
            .name("recording-mic".into())
            .spawn(move || run_stream(job, sink, ready_tx))
            .map_err(|_| RecordingError::SourceUnavailable {
                reason: "could not start the capture thread",
            })?;
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => {
                self.running = Some(Running { stop, thread });
                Ok(())
            }
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                stop.store(true, Ordering::Release);
                thread.thread().unpark();
                let _ = thread.join();
                Err(RecordingError::SourceUnavailable {
                    reason: "the input device did not start",
                })
            }
        }
    }

    fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            running.stop.store(true, Ordering::Release);
            running.thread.thread().unpark();
            let _ = running.thread.join();
        }
    }
}

impl Drop for MicSource {
    fn drop(&mut self) {
        self.stop();
    }
}

struct StreamJob {
    expected: Negotiated,
    expected_device: DeviceFormat,
    sample_format: SampleFormat,
    channel_pair: (u16, u16),
    stop: Arc<AtomicBool>,
}

/// The capture thread: owns the `!Send` stream from build to drop.
fn run_stream(job: StreamJob, sink: BlockSink, ready: mpsc::Sender<Result<(), RecordingError>>) {
    let stop = Arc::clone(&job.stop);
    match build_stream(job, sink) {
        Ok(stream) => {
            // Make the clock's one-time setup happen here, not in a callback.
            let _ = host_now_ns();
            if stream.play().is_err() {
                let _ = ready.send(Err(RecordingError::SourceUnavailable {
                    reason: "the input stream would not start",
                }));
                return;
            }
            let _ = ready.send(Ok(()));
            while !stop.load(Ordering::Acquire) {
                std::thread::park_timeout(Duration::from_millis(20));
            }
            // The stream, its callbacks and the sink go with it.
            drop(stream);
        }
        Err(error) => {
            let _ = ready.send(Err(error));
        }
    }
}

fn build_stream(job: StreamJob, sink: BlockSink) -> Result<cpal::Stream, RecordingError> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or(RecordingError::SourceUnavailable {
            reason: "no input device",
        })?;
    let supported =
        device
            .default_input_config()
            .map_err(|_| RecordingError::SourceUnavailable {
                reason: "the input device reports no format",
            })?;
    // The default device may have changed between `open` and now.
    let now = DeviceFormat {
        sample_rate: supported.sample_rate().0,
        channels: supported.channels(),
    };
    if now != job.expected_device || supported.sample_format() != job.sample_format {
        return Err(RecordingError::SourceUnavailable {
            reason: "the input device changed while the recording was starting",
        });
    }
    let config: cpal::StreamConfig = supported.config();
    match job.sample_format {
        SampleFormat::F32 => typed::<f32>(&device, &config, job, sink),
        SampleFormat::I16 => typed::<i16>(&device, &config, job, sink),
        SampleFormat::U16 => typed::<u16>(&device, &config, job, sink),
        SampleFormat::I32 => typed::<i32>(&device, &config, job, sink),
        SampleFormat::U32 => typed::<u32>(&device, &config, job, sink),
        SampleFormat::I8 => typed::<i8>(&device, &config, job, sink),
        SampleFormat::U8 => typed::<u8>(&device, &config, job, sink),
        SampleFormat::F64 => typed::<f64>(&device, &config, job, sink),
        _ => Err(RecordingError::UnsupportedFormat {
            reason: "the input device delivers a sample type this build cannot convert",
        }),
    }
}

fn typed<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    job: StreamJob,
    mut sink: BlockSink,
) -> Result<cpal::Stream, RecordingError>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let channels = job.expected.device_channels;
    let (a, b) = job.expected.pair;
    let stereo = job.expected.format.channels == 2;
    let _ = job.channel_pair;
    let signals = sink.handle();
    let on_error = move |error: cpal::StreamError| {
        // Only an atomic is set: this runs on a thread cpal owns.
        signals.signal(match error {
            cpal::StreamError::DeviceNotAvailable => SourceSignal::DeviceChanged,
            _ => SourceSignal::DeviceLost,
        });
    };
    let on_data = move |data: &[T], info: &cpal::InputCallbackInfo| {
        let timestamp = info.timestamp();
        let latency = timestamp
            .callback
            .duration_since(&timestamp.capture)
            .map_or(0, |d| d.as_nanos() as u64);
        let host_ns = host_now_ns().saturating_sub(latency);
        sink.push_with(data.len() / channels, host_ns, |i, out| {
            let frame = &data[i * channels..(i + 1) * channels];
            out[0] = <f32 as FromSample<T>>::from_sample_(frame[a]);
            if stereo {
                out[1] = <f32 as FromSample<T>>::from_sample_(frame[b]);
            }
        });
    };
    device
        .build_input_stream(config, on_data, on_error, None)
        .map_err(|_| RecordingError::SourceUnavailable {
            reason: "the input stream could not be built",
        })
}

#[cfg(test)]
#[path = "mic_tests.rs"]
mod tests;
