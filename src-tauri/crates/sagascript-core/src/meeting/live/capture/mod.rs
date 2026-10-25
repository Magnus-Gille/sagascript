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
}

impl Sink {
    pub fn new(source: Source, sender: SyncSender<Block>, health: Arc<Health>) -> Self {
        Self {
            source,
            sender,
            health,
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
        if rate == 0 || frames > 192_000 || host_ns == 0 {
            self.health.failed.store(true, Ordering::Release);
            return;
        }
        for offset in (0..frames).step_by(BLOCK_SAMPLES) {
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
        let sink = Sink::new(Source::Microphone, mic, mic_health.clone());
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
        let tap = tap::Tap::start(Sink::new(Source::System, system, system_health))?;
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
    #[test]
    fn callback_queue_is_bounded_and_reports_loss() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let health = Arc::new(Health::default());
        let sink = Sink::new(Source::System, tx, health.clone());
        sink.push(1_000_000, 48_000, BLOCK_SAMPLES + 3, |i| i as f32 / 4096.);
        let block = rx.try_recv().unwrap();
        assert_eq!(block.len, BLOCK_SAMPLES);
        assert_eq!(block.host_ns, 1_000_000);
        assert_eq!(health.dropped_samples.load(Ordering::Relaxed), 3);
        assert!(rx.try_recv().is_err());
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
