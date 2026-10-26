//! Native capture adapters. Call start/drop on one dedicated capture thread.
//! Callbacks only copy into bounded blocks and try-send; processing runs elsewhere.
mod tap;

use super::document::Source;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{
    mpsc::{SyncSender, TrySendError},
    Arc,
};

pub const BLOCK_SAMPLES: usize = 2048;
/// At most 128 blocks are retained by the consumer (under 1.1 MiB per source).
pub const QUEUE_BLOCKS: usize = 128;

pub struct Block {
    pub source: Source,
    /// Native monotonic capture time, nanoseconds since boot. Never wall time.
    pub host_ns: u64,
    pub sample_rate: u32,
    pub len: usize,
    pub samples: [f32; BLOCK_SAMPLES],
}

#[derive(Default)]
pub struct Health {
    pub dropped_samples: AtomicU64,
    pub failed: AtomicBool,
    pub last_host_ns: AtomicU64,
}

pub(crate) struct Sink {
    source: Source,
    sender: SyncSender<Block>,
    pub health: Arc<Health>,
    activation_epoch: Arc<AtomicU64>,
}

impl Sink {
    #[cfg(test)]
    pub fn new(source: Source, sender: SyncSender<Block>, health: Arc<Health>) -> Self {
        Self::with_activation(source, sender, health, Arc::new(AtomicU64::new(1)))
    }

    fn with_activation(
        source: Source,
        sender: SyncSender<Block>,
        health: Arc<Health>,
        activation_epoch: Arc<AtomicU64>,
    ) -> Self {
        Self {
            source,
            sender,
            health,
            activation_epoch,
        }
    }

    /// The native callback supplies a bounded channel view; no heap buffer here.
    pub fn push(
        &self,
        host_ns: u64,
        rate: u32,
        frames: usize,
        mut sample: impl FnMut(usize) -> f32,
    ) {
        let activation_epoch = self.activation_epoch.load(Ordering::Acquire);
        if activation_epoch == 0 {
            return;
        }
        if rate == 0 || frames > 192_000 || host_ns == 0 {
            self.health.failed.store(true, Ordering::Release);
            return;
        }
        let first_frame = if host_ns < activation_epoch {
            let delta_ns = u128::from(activation_epoch - host_ns);
            let frame_offset = delta_ns
                .saturating_mul(u128::from(rate))
                .saturating_add(999_999_999)
                / 1_000_000_000;
            if frame_offset >= frames as u128 {
                return;
            }
            frame_offset as usize
        } else {
            0
        };
        for offset in (first_frame..frames).step_by(BLOCK_SAMPLES) {
            let len = (frames - offset).min(BLOCK_SAMPLES);
            let mut block = Block {
                source: self.source,
                host_ns: host_ns.saturating_add(offset as u64 * 1_000_000_000 / u64::from(rate)),
                sample_rate: rate,
                len,
                samples: [0.; BLOCK_SAMPLES],
            };
            for (i, slot) in block.samples[..len].iter_mut().enumerate() {
                *slot = sample(offset + i);
                if !slot.is_finite() {
                    self.health.failed.store(true, Ordering::Release);
                    return;
                }
            }
            self.health
                .last_host_ns
                .store(block.host_ns, Ordering::Release);
            match self.sender.try_send(block) {
                Ok(()) => {}
                Err(TrySendError::Full(block)) => {
                    self.health
                        .dropped_samples
                        .fetch_add(block.len as u64, Ordering::Relaxed);
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.health.failed.store(true, Ordering::Release);
                    return;
                }
            }
        }
    }
}

pub struct Capture {
    // CPAL stream must stay on this thread and be dropped before the tap.
    _microphone: cpal::Stream,
    _tap: tap::Tap,
    initial_devices: (u32, u32),
}

impl Capture {
    /// Explicit capture only: may request microphone/system-audio permission.
    pub fn start(
        mic: SyncSender<Block>,
        system: SyncSender<Block>,
        mic_health: Arc<Health>,
        system_health: Arc<Health>,
        activation_epoch: Arc<AtomicU64>,
    ) -> Result<Self, String> {
        let permission = crate::audio::permission::microphone_permission();
        let effective = crate::audio::permission::capture_permission_status(permission.status)
            .map_err(|e| e.to_string())?;
        if effective.capture_denied() {
            return Err("Microphone permission denied".into());
        }
        let initial_devices = tap::default_devices()?;
        let device = cpal::default_host()
            .default_input_device()
            .ok_or("No microphone available")?;
        let supported = device.default_input_config().map_err(|e| e.to_string())?;
        let format = supported.sample_format();
        let config = supported.config();
        let rate = config.sample_rate.0;
        let channels = usize::from(config.channels);
        if !(8_000..=192_000).contains(&rate) || channels == 0 || channels > 64 {
            return Err("Unsupported microphone format".into());
        }
        let sink = Sink::with_activation(
            Source::Microphone,
            mic,
            mic_health.clone(),
            activation_epoch.clone(),
        );
        let mut first: Option<(cpal::StreamInstant, u64)> = None;
        let stream = device
            .build_input_stream_raw(
                &config,
                format,
                move |data, info| {
                    let capture = info.timestamp().capture;
                    let (origin, host) = *first.get_or_insert_with(|| {
                        let delay = info
                            .timestamp()
                            .callback
                            .duration_since(&capture)
                            .unwrap_or_default();
                        (
                            capture,
                            tap::host_ns().saturating_sub(delay.as_nanos() as u64),
                        )
                    });
                    let Some(delta) = capture.duration_since(&origin) else {
                        sink.health.failed.store(true, Ordering::Release);
                        return;
                    };
                    let time = host.saturating_add(delta.as_nanos() as u64);
                    macro_rules! send {
                        ($ty:ty,$convert:expr) => {
                            if let Some(samples) = data.as_slice::<$ty>() {
                                if samples.len() % channels != 0 {
                                    sink.health.failed.store(true, Ordering::Release);
                                    return;
                                }
                                sink.push(time, rate, samples.len() / channels, |frame| {
                                    samples[frame * channels..(frame + 1) * channels]
                                        .iter()
                                        .map($convert)
                                        .sum::<f32>()
                                        / channels as f32
                                });
                            }
                        };
                    }
                    match format {
                        cpal::SampleFormat::F32 => send!(f32, |v: &f32| *v),
                        cpal::SampleFormat::I16 => send!(i16, |v: &i16| f32::from(*v) / 32768.),
                        cpal::SampleFormat::U16 => {
                            send!(u16, |v: &u16| (f32::from(*v) - 32768.) / 32768.)
                        }
                        _ => sink.health.failed.store(true, Ordering::Release),
                    }
                },
                move |_| {
                    mic_health.failed.store(true, Ordering::Release);
                },
                None,
            )
            .map_err(|e| e.to_string())?;
        if !matches!(
            format,
            cpal::SampleFormat::F32 | cpal::SampleFormat::I16 | cpal::SampleFormat::U16
        ) {
            return Err("Unsupported microphone sample format".into());
        }
        let tap = tap::Tap::start(Sink::with_activation(
            Source::System,
            system,
            system_health,
            activation_epoch,
        ))?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self {
            _microphone: stream,
            _tap: tap,
            initial_devices,
        })
    }

    /// Stop on a route change instead of silently mislabelling a changed format.
    pub fn check_devices(&self) -> Result<(), String> {
        if tap::default_devices()? != self.initial_devices {
            return Err(
                "Audio device changed. Stop and save this meeting, then start a new session."
                    .into(),
            );
        }
        Ok(())
    }
}

pub fn supported() -> bool {
    tap::supported()
}

pub fn host_ns() -> u64 {
    tap::host_ns()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    #[test]
    fn callback_queue_is_bounded_and_reports_loss() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let health = Arc::new(Health::default());
        let sink = Sink::with_activation(
            Source::System,
            tx,
            health.clone(),
            Arc::new(AtomicU64::new(1)),
        );
        sink.push(1_000_000, 48_000, BLOCK_SAMPLES + 3, |i| i as f32 / 4096.);
        let block = rx.try_recv().unwrap();
        assert_eq!(block.len, BLOCK_SAMPLES);
        assert_eq!(block.host_ns, 1_000_000);
        assert_eq!(health.dropped_samples.load(Ordering::Relaxed), 3);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn preactivation_callbacks_are_ignored_without_loss_or_health_updates() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let health = Arc::new(Health::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let sink = Sink::with_activation(
            Source::Microphone,
            tx,
            health.clone(),
            Arc::new(AtomicU64::new(0)),
        );
        for index in 0..=QUEUE_BLOCKS {
            let calls = calls.clone();
            sink.push(1_000_000 + index as u64 * 1_000_000, 48_000, 512, |_| {
                calls.fetch_add(1, Ordering::Relaxed);
                0.25
            });
        }
        assert!(rx.try_recv().is_err());
        assert_eq!(calls.load(Ordering::Acquire), 0);
        assert_eq!(health.dropped_samples.load(Ordering::Acquire), 0);
        assert_eq!(health.last_host_ns.load(Ordering::Acquire), 0);
        assert!(!health.failed.load(Ordering::Acquire));
    }

    #[test]
    fn crossing_activation_epoch_trims_old_frames_and_reanchors_timestamp() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let health = Arc::new(Health::default());
        let epoch = 1_000_000_000;
        let sink = Sink::with_activation(
            Source::Microphone,
            tx,
            health.clone(),
            Arc::new(AtomicU64::new(epoch)),
        );
        sink.push(epoch - 31_250, 48_000, 8, |index| index as f32);
        let block = rx.try_recv().expect("post-activation samples");
        assert_eq!(block.host_ns, epoch + 10_416);
        assert_eq!(block.len, 6);
        assert_eq!(&block.samples[..block.len], &[2., 3., 4., 5., 6., 7.]);
        assert_eq!(health.last_host_ns.load(Ordering::Acquire), epoch + 10_416);
    }

    #[test]
    fn wholly_pre_activation_block_is_ignored() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let health = Arc::new(Health::default());
        let epoch = 1_000_000_000;
        let sink = Sink::with_activation(
            Source::System,
            tx,
            health.clone(),
            Arc::new(AtomicU64::new(epoch)),
        );
        sink.push(epoch - 10_000_000, 1_000, 5, |_| 0.25);
        assert!(rx.try_recv().is_err());
        assert_eq!(health.dropped_samples.load(Ordering::Acquire), 0);
        assert_eq!(health.last_host_ns.load(Ordering::Acquire), 0);
        assert!(!health.failed.load(Ordering::Acquire));
    }

    #[test]
    fn invalid_pcm_and_clock_fail_visibly() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let health = Arc::new(Health::default());
        let sink = Sink::new(Source::Microphone, tx, health.clone());
        sink.push(1, 16_000, 3, |_| f32::NAN);
        assert!(health.failed.load(Ordering::Acquire));
        assert!(rx.try_recv().is_err());
    }
}
