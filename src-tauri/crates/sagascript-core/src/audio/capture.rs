use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use tracing::{error, info};

use crate::error::DictationError;
use super::resample::{resample_to_16khz, TARGET_SAMPLE_RATE};

/// Maximum recording length: 15 minutes. Capped in device-rate samples while
/// recording (the buffer holds raw mono at the device rate), then resampled to
/// 16 kHz on stop.
const MAX_BUFFER_SECONDS: usize = 60 * 15;
const STREAM_NOT_READY: u64 = u64::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioCaptureMetrics {
    /// Time from the capture request until `stream.play()` returns. A slow
    /// Bluetooth profile switch is visible here without recording a device
    /// name or any audio content.
    pub stream_play_return_ms: Option<u64>,
    /// Time from the capture request until the first input callback runs.
    /// This can be later than `stream_play_return_ms` when the audio backend
    /// takes time to deliver the first buffer.
    pub first_callback_ms: Option<u64>,
    pub device_sample_rate_hz: Option<u32>,
}

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
    /// Time published by the capture thread after `stream.play()` returns.
    stream_play_return_ms: Arc<AtomicU64>,
    /// Time published by the first input callback.
    first_callback_ms: Arc<AtomicU64>,
    /// First initialization or stream-processing error from the capture
    /// thread for the current capture, if any.
    worker_error: Arc<Mutex<Option<DictationError>>>,
    capture_thread: Option<thread::JoinHandle<()>>,
    /// Retained audio from last capture for retry
    last_captured: Option<Vec<f32>>,
}

// AudioCaptureService is Send+Sync because it doesn't hold cpal::Stream directly
unsafe impl Send for AudioCaptureService {}
unsafe impl Sync for AudioCaptureService {}

impl Default for AudioCaptureService {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioCaptureService {
    pub fn new() -> Self {
        Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            stop_signal: Arc::new(Mutex::new(false)),
            device_sample_rate: Arc::new(AtomicU32::new(0)),
            stream_play_return_ms: Arc::new(AtomicU64::new(STREAM_NOT_READY)),
            first_callback_ms: Arc::new(AtomicU64::new(STREAM_NOT_READY)),
            worker_error: Arc::new(Mutex::new(None)),
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
        let stream_play_return_ms = Arc::clone(&self.stream_play_return_ms);
        let first_callback_ms = Arc::clone(&self.first_callback_ms);
        clear_worker_error(&self.worker_error);
        let worker_error = Arc::clone(&self.worker_error);
        device_sample_rate.store(0, Ordering::SeqCst);
        stream_play_return_ms.store(STREAM_NOT_READY, Ordering::SeqCst);
        first_callback_ms.store(STREAM_NOT_READY, Ordering::SeqCst);
        let capture_requested_at = Instant::now();

        // Spawn a thread that owns the cpal::Stream
        let handle = thread::spawn(move || {
            if let Err(e) = run_capture(
                buffer,
                stop_signal,
                device_sample_rate,
                stream_play_return_ms,
                first_callback_ms,
                worker_error.clone(),
                capture_requested_at,
            ) {
                record_first_worker_error(&worker_error, e.clone());
                error!("Audio capture thread error: {e}");
            }
        });

        self.capture_thread = Some(handle);

        // Give the capture thread a moment to initialize
        thread::sleep(std::time::Duration::from_millis(50));

        // Initialization failures are often known by now. Return them to the
        // caller while retaining the error for stop_capture as well.
        let startup_error = { self.worker_error.lock().unwrap().clone() };
        if let Some(error) = startup_error {
            if let Some(handle) = self.capture_thread.take() {
                let _ = handle.join();
            }
            return Err(error);
        }

        info!("Audio capture started");
        Ok(())
    }

    /// Stop capturing and return the captured 16 kHz samples.
    ///
    /// On resample failure this returns `Err` (finding 4) rather than an empty
    /// `Vec` — an empty buffer means genuine silence, so masking a device/format
    /// error as empty made a real failure indistinguishable from silence (and
    /// surfaced the misleading "No audio captured" to the user).
    pub fn stop_capture(&mut self) -> Result<Vec<f32>, DictationError> {
        // Signal the capture thread to stop
        {
            let mut stop = self.stop_signal.lock().unwrap();
            *stop = true;
        }

        // Wait for the capture thread to finish
        if let Some(handle) = self.capture_thread.take() {
            let _ = handle.join();
        }

        if let Some(error) = self.worker_error.lock().unwrap().take() {
            error!("Audio capture failed: {error}");
            return Err(error);
        }

        let raw = {
            let mut buf = self.buffer.lock().unwrap();
            std::mem::take(&mut *buf)
        };

        // Resample the entire recording to 16 kHz in a single pass. Doing it
        // here (rather than per-callback) keeps the realtime audio thread cheap
        // and avoids the filter-restart transient that a per-callback resampler
        // injects at every chunk boundary. An empty buffer or unknown device
        // rate is genuine silence — Ok(empty); a resample failure is a real
        // error and is propagated.
        let device_rate = self.device_sample_rate.load(Ordering::SeqCst);
        let samples = if raw.is_empty() || device_rate == 0 {
            raw
        } else {
            resample_to_16khz(raw, device_rate).map_err(|e| {
                error!("Resample failed: {e}");
                DictationError::AudioCaptureError(format!("Resample failed: {e}"))
            })?
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

        Ok(samples)
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

    pub fn metrics(&self) -> AudioCaptureMetrics {
        let stream_play_return_ms = self.stream_play_return_ms.load(Ordering::SeqCst);
        let first_callback_ms = self.first_callback_ms.load(Ordering::SeqCst);
        let device_sample_rate_hz = self.device_sample_rate.load(Ordering::SeqCst);
        AudioCaptureMetrics {
            stream_play_return_ms: (stream_play_return_ms != STREAM_NOT_READY)
                .then_some(stream_play_return_ms),
            first_callback_ms: (first_callback_ms != STREAM_NOT_READY).then_some(first_callback_ms),
            device_sample_rate_hz: (device_sample_rate_hz != 0).then_some(device_sample_rate_hz),
        }
    }
}

/// Run audio capture on a dedicated thread (owns the !Send cpal::Stream)
fn run_capture(
    buffer: Arc<Mutex<Vec<f32>>>,
    stop_signal: Arc<Mutex<bool>>,
    device_sample_rate_out: Arc<AtomicU32>,
    stream_play_return_ms_out: Arc<AtomicU64>,
    first_callback_ms_out: Arc<AtomicU64>,
    worker_error: Arc<Mutex<Option<DictationError>>>,
    capture_requested_at: Instant,
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

    let worker_error_for_callback = Arc::clone(&worker_error);
    let stop_signal_for_callback = Arc::clone(&stop_signal);
    let err_fn = move |err: cpal::StreamError| {
        record_first_worker_error(
            &worker_error_for_callback,
            DictationError::AudioCaptureError(format!("Audio stream error: {err}")),
        );
        error!("Audio stream error: {err}");
        if let Ok(mut stop) = stop_signal_for_callback.lock() {
            *stop = true;
        }
    };

    let buf_clone = Arc::clone(&buffer);
    let first_callback_ms_clone = Arc::clone(&first_callback_ms_out);

    let stream = match config.sample_format() {
        SampleFormat::F32 => {
            let config = config.into();
            device
                .build_input_stream(
                    &config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        publish_first_callback_ms(
                            &first_callback_ms_clone,
                            elapsed_ms(capture_requested_at),
                        );
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
            let first_callback_ms_clone = Arc::clone(&first_callback_ms_out);
            device
                .build_input_stream(
                    &config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        publish_first_callback_ms(
                            &first_callback_ms_clone,
                            elapsed_ms(capture_requested_at),
                        );
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
    let stream_play_return_ms = elapsed_ms(capture_requested_at);
    stream_play_return_ms_out.store(stream_play_return_ms, Ordering::SeqCst);
    info!("Audio input stream play returned after {stream_play_return_ms}ms");

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

fn clear_worker_error(error: &Arc<Mutex<Option<DictationError>>>) {
    *error.lock().unwrap() = None;
}

fn record_first_worker_error(
    error_slot: &Arc<Mutex<Option<DictationError>>>,
    error: DictationError,
) {
    let mut first_error = error_slot.lock().unwrap();
    if first_error.is_none() {
        *first_error = Some(error);
    }
}

fn elapsed_ms(since: Instant) -> u64 {
    since
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn publish_first_callback_ms(output: &AtomicU64, elapsed_ms: u64) {
    let _ = output.compare_exchange(
        STREAM_NOT_READY,
        elapsed_ms,
        Ordering::SeqCst,
        Ordering::SeqCst,
    );
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

    // Finding 4: stop_capture returns a Result so a real device/resample failure
    // is distinguishable from silence. With no capture ever started the buffer is
    // empty and the device rate unknown (0), so genuine silence must be
    // Ok(empty) — NOT an error and NOT indistinguishable from a failure.
    #[test]
    fn stop_capture_silence_is_ok_empty() {
        let mut svc = AudioCaptureService::new();
        let out = svc
            .stop_capture()
            .expect("silence should be Ok(empty), not Err");
        assert!(out.is_empty());
    }

    #[test]
    fn capture_metrics_are_unknown_before_a_stream_opens() {
        let svc = AudioCaptureService::new();
        assert_eq!(
            svc.metrics(),
            AudioCaptureMetrics {
                stream_play_return_ms: None,
                first_callback_ms: None,
                device_sample_rate_hz: None,
            }
        );
    }

    #[test]
    fn capture_metrics_preserve_zero_latency_and_known_sample_rate() {
        let svc = AudioCaptureService::new();
        svc.stream_play_return_ms.store(0, Ordering::SeqCst);
        svc.first_callback_ms.store(0, Ordering::SeqCst);
        svc.device_sample_rate.store(48_000, Ordering::SeqCst);

        assert_eq!(
            svc.metrics(),
            AudioCaptureMetrics {
                stream_play_return_ms: Some(0),
                first_callback_ms: Some(0),
                device_sample_rate_hz: Some(48_000),
            }
        );
    }

    #[test]
    fn capture_metrics_keep_zero_sample_rate_unknown() {
        let svc = AudioCaptureService::new();
        svc.stream_play_return_ms.store(0, Ordering::SeqCst);
        svc.first_callback_ms.store(3, Ordering::SeqCst);

        assert_eq!(
            svc.metrics(),
            AudioCaptureMetrics {
                stream_play_return_ms: Some(0),
                first_callback_ms: Some(3),
                device_sample_rate_hz: None,
            }
        );
    }

    #[test]
    fn first_callback_metric_records_only_the_first_callback() {
        let output = AtomicU64::new(STREAM_NOT_READY);
        publish_first_callback_ms(&output, 0);
        publish_first_callback_ms(&output, 12);

        assert_eq!(output.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn first_worker_error_is_retained_and_taken_once() {
        let error_slot = Arc::new(Mutex::new(None));
        record_first_worker_error(
            &error_slot,
            DictationError::MicrophonePermissionDenied,
        );
        record_first_worker_error(
            &error_slot,
            DictationError::AudioCaptureError("later error".into()),
        );

        assert!(matches!(
            error_slot.lock().unwrap().take(),
            Some(DictationError::MicrophonePermissionDenied)
        ));
        assert!(error_slot.lock().unwrap().is_none());
    }

    #[test]
    fn stop_capture_returns_worker_error_and_consumes_it() {
        let mut svc = AudioCaptureService::new();
        *svc.worker_error.lock().unwrap() = Some(DictationError::AudioCaptureError(
            "stream stopped".into(),
        ));

        let err = svc.stop_capture().expect_err("worker error should not become silence");
        assert_eq!(err.to_string(), "Audio capture error: stream stopped");
        assert!(svc.stop_capture().expect("error should be consumed").is_empty());
    }

    #[test]
    fn clearing_worker_error_removes_stale_capture_failure() {
        let svc = AudioCaptureService::new();
        *svc.worker_error.lock().unwrap() = Some(DictationError::MicrophonePermissionDenied);

        clear_worker_error(&svc.worker_error);

        assert!(svc.worker_error.lock().unwrap().is_none());
    }
}
