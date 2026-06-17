use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use tracing::{error, info};

use crate::error::DictationError;
use super::resample::{resample_to_16khz, TARGET_SAMPLE_RATE};

/// Maximum recording length: 15 minutes. Capped in device-rate samples while
/// recording (the buffer holds raw mono at the device rate), then resampled to
/// 16 kHz on stop.
const MAX_BUFFER_SECONDS: usize = 60 * 15;

/// Audio capture service using cpal
/// The cpal::Stream is !Send, so we spawn a dedicated thread to own it.
/// Communication happens through shared buffers and a stop signal.
pub struct AudioCaptureService {
    /// Raw mono samples at the device sample rate (resampled to 16 kHz on stop).
    buffer: Arc<Mutex<Vec<f32>>>,
    stop_signal: Arc<Mutex<bool>>,
    /// Device sample rate published by the capture thread once the input opens
    /// (0 until known). Read by `stop_capture` to resample the whole buffer.
    device_sample_rate: Arc<AtomicU32>,
    capture_thread: Option<thread::JoinHandle<()>>,
    /// Retained audio from last capture for retry
    last_captured: Option<Vec<f32>>,
}

// AudioCaptureService is Send+Sync because it doesn't hold cpal::Stream directly
unsafe impl Send for AudioCaptureService {}
unsafe impl Sync for AudioCaptureService {}

impl AudioCaptureService {
    pub fn new() -> Self {
        Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            stop_signal: Arc::new(Mutex::new(false)),
            device_sample_rate: Arc::new(AtomicU32::new(0)),
            capture_thread: None,
            last_captured: None,
        }
    }

    /// Start capturing audio from the default input device
    pub fn start_capture(&mut self) -> Result<(), DictationError> {
        // Clear previous buffer and stop signal
        {
            let mut buf = self.buffer.lock().unwrap();
            buf.clear();
        }
        {
            let mut stop = self.stop_signal.lock().unwrap();
            *stop = false;
        }

        let buffer = Arc::clone(&self.buffer);
        let stop_signal = Arc::clone(&self.stop_signal);
        let device_sample_rate = Arc::clone(&self.device_sample_rate);
        device_sample_rate.store(0, Ordering::SeqCst);

        // Spawn a thread that owns the cpal::Stream
        let handle = thread::spawn(move || {
            if let Err(e) = run_capture(buffer, stop_signal, device_sample_rate) {
                error!("Audio capture thread error: {e}");
            }
        });

        self.capture_thread = Some(handle);

        // Give the capture thread a moment to initialize
        thread::sleep(std::time::Duration::from_millis(50));

        info!("Audio capture started");
        Ok(())
    }

    /// Stop capturing and return the audio samples
    pub fn stop_capture(&mut self) -> Vec<f32> {
        // Signal the capture thread to stop
        {
            let mut stop = self.stop_signal.lock().unwrap();
            *stop = true;
        }

        // Wait for the capture thread to finish
        if let Some(handle) = self.capture_thread.take() {
            let _ = handle.join();
        }

        let raw = {
            let mut buf = self.buffer.lock().unwrap();
            std::mem::take(&mut *buf)
        };

        // Resample the entire recording to 16 kHz in a single pass. Doing it
        // here (rather than per-callback) keeps the realtime audio thread cheap
        // and avoids the filter-restart transient that a per-callback resampler
        // injects at every chunk boundary.
        let device_rate = self.device_sample_rate.load(Ordering::SeqCst);
        let samples = if raw.is_empty() || device_rate == 0 {
            raw
        } else {
            match resample_to_16khz(raw, device_rate) {
                Ok(s) => s,
                Err(e) => {
                    error!("Resample failed, dropping recording: {e}");
                    Vec::new()
                }
            }
        };

        let duration = samples.len() as f64 / TARGET_SAMPLE_RATE as f64;
        info!(
            "Audio capture stopped: {} samples ({:.2}s) [device {} Hz]",
            samples.len(),
            duration,
            device_rate
        );

        // Retain for retry
        self.last_captured = Some(samples.clone());

        samples
    }

    /// Get the last captured audio for retry
    #[allow(dead_code)]
    pub fn last_captured_audio(&self) -> Option<&Vec<f32>> {
        self.last_captured.as_ref()
    }

    /// Clear retained audio after successful transcription
    pub fn clear_last_captured(&mut self) {
        self.last_captured = None;
    }
}

/// Run audio capture on a dedicated thread (owns the !Send cpal::Stream)
fn run_capture(
    buffer: Arc<Mutex<Vec<f32>>>,
    stop_signal: Arc<Mutex<bool>>,
    device_sample_rate_out: Arc<AtomicU32>,
) -> Result<(), DictationError> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or(DictationError::MicrophonePermissionDenied)?;

    let config = device
        .default_input_config()
        .map_err(|e| DictationError::AudioCaptureError(format!("Failed to get input config: {e}")))?;

    let device_sample_rate = config.sample_rate().0;
    let device_channels = config.channels();

    // Publish the rate so stop_capture can resample the buffer.
    device_sample_rate_out.store(device_sample_rate, Ordering::SeqCst);

    info!(
        "Audio input: {} Hz, {} ch, {:?}",
        device_sample_rate,
        device_channels,
        config.sample_format()
    );

    let err_fn = |err: cpal::StreamError| {
        error!("Audio stream error: {err}");
    };

    let buf_clone = Arc::clone(&buffer);

    let stream = match config.sample_format() {
        SampleFormat::F32 => {
            let config = config.into();
            device
                .build_input_stream(
                    &config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        process_samples(data, device_channels, device_sample_rate, &buf_clone);
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| {
                    DictationError::AudioCaptureError(format!("Failed to build stream: {e}"))
                })?
        }
        SampleFormat::I16 => {
            let config = config.into();
            device
                .build_input_stream(
                    &config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        process_samples_i16(
                            data,
                            device_channels,
                            device_sample_rate,
                            &buf_clone,
                        );
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| {
                    DictationError::AudioCaptureError(format!("Failed to build stream: {e}"))
                })?
        }
        format => {
            return Err(DictationError::AudioCaptureError(format!(
                "Unsupported sample format: {format:?}"
            )));
        }
    };

    stream
        .play()
        .map_err(|e| DictationError::AudioCaptureError(format!("Failed to start stream: {e}")))?;

    // Spin until stop signal (the stream callback fills the buffer)
    loop {
        thread::sleep(std::time::Duration::from_millis(10));
        let stop = stop_signal.lock().unwrap();
        if *stop {
            break;
        }
    }

    // Stream is dropped here, stopping capture
    Ok(())
}

fn process_samples(
    data: &[f32],
    channels: u16,
    device_rate: u32,
    buffer: &Arc<Mutex<Vec<f32>>>,
) {
    // Realtime-safe hot path: downmix to mono and append raw device-rate samples
    // with a length cap. No resampling and no per-callback allocation here —
    // resampling to 16 kHz happens once on stop (see stop_capture).
    let max_samples = (device_rate as usize).saturating_mul(MAX_BUFFER_SECONDS);
    let channels = channels.max(1) as usize;

    let mut buf = buffer.lock().unwrap();
    if buf.len() >= max_samples {
        return;
    }

    if channels == 1 {
        let take = (max_samples - buf.len()).min(data.len());
        buf.extend_from_slice(&data[..take]);
    } else {
        // Average channels into mono, pushing directly to avoid a temporary Vec.
        for frame in data.chunks(channels) {
            if buf.len() >= max_samples {
                break;
            }
            buf.push(frame.iter().sum::<f32>() / channels as f32);
        }
    }
}

/// Like `process_samples` but for i16 input — converts to f32 and downmixes
/// directly into the buffer, staying allocation-free on the realtime callback
/// (no intermediate `Vec<f32>`).
fn process_samples_i16(
    data: &[i16],
    channels: u16,
    device_rate: u32,
    buffer: &Arc<Mutex<Vec<f32>>>,
) {
    let max_samples = (device_rate as usize).saturating_mul(MAX_BUFFER_SECONDS);
    let channels = channels.max(1) as usize;

    let mut buf = buffer.lock().unwrap();
    if buf.len() >= max_samples {
        return;
    }

    if channels == 1 {
        for &s in data {
            if buf.len() >= max_samples {
                break;
            }
            buf.push(s as f32 / i16::MAX as f32);
        }
    } else {
        for frame in data.chunks(channels) {
            if buf.len() >= max_samples {
                break;
            }
            let avg =
                frame.iter().map(|&s| s as f32 / i16::MAX as f32).sum::<f32>() / channels as f32;
            buf.push(avg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf() -> Arc<Mutex<Vec<f32>>> {
        Arc::new(Mutex::new(Vec::new()))
    }

    #[test]
    fn f32_mono_appends_raw() {
        let b = buf();
        process_samples(&[0.1, 0.2, 0.3], 1, 16_000, &b);
        assert_eq!(*b.lock().unwrap(), vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn f32_stereo_downmixes_to_mono() {
        let b = buf();
        process_samples(&[1.0, 0.0, 0.0, 1.0], 2, 16_000, &b);
        let out = b.lock().unwrap();
        assert_eq!(out.len(), 2);
        assert!((out[0] - 0.5).abs() < 1e-6);
        assert!((out[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn i16_mono_converts_to_unit_range() {
        let b = buf();
        process_samples_i16(&[i16::MAX, 0, i16::MIN], 1, 16_000, &b);
        let out = b.lock().unwrap();
        assert_eq!(out.len(), 3);
        assert!((out[0] - 1.0).abs() < 1e-4);
        assert!(out[1].abs() < 1e-6);
        assert!((out[2] - (-1.0)).abs() < 1e-3); // MIN/MAX ≈ -1.00003
    }

    #[test]
    fn i16_stereo_downmix_averages_channels() {
        let b = buf();
        process_samples_i16(&[i16::MAX, 0, 0, i16::MAX], 2, 16_000, &b);
        let out = b.lock().unwrap();
        assert_eq!(out.len(), 2);
        assert!((out[0] - 0.5).abs() < 1e-4);
        assert!((out[1] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn cap_enforced_f32() {
        let b = buf();
        let cap = MAX_BUFFER_SECONDS; // rate = 1 → cap = MAX_BUFFER_SECONDS samples
        process_samples(&vec![0.0f32; cap + 100], 1, 1, &b);
        assert_eq!(b.lock().unwrap().len(), cap);
    }

    #[test]
    fn cap_enforced_i16() {
        let b = buf();
        let cap = MAX_BUFFER_SECONDS;
        process_samples_i16(&vec![0i16; cap + 100], 1, 1, &b);
        assert_eq!(b.lock().unwrap().len(), cap);
    }
}
