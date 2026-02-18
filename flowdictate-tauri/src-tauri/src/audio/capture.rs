use std::sync::{Arc, Mutex};
use std::thread;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use tracing::{error, info};

use crate::error::DictationError;

/// Required audio format for Whisper
const TARGET_SAMPLE_RATE: u32 = 16_000;
/// Maximum buffer: 15 minutes at 16kHz
const MAX_BUFFER_SAMPLES: usize = 16_000 * 60 * 15;

/// Audio capture service using cpal
/// The cpal::Stream is !Send, so we spawn a dedicated thread to own it.
/// Communication happens through shared buffers and a stop signal.
pub struct AudioCaptureService {
    buffer: Arc<Mutex<Vec<f32>>>,
    stop_signal: Arc<Mutex<bool>>,
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

        // Spawn a thread that owns the cpal::Stream
        let handle = thread::spawn(move || {
            if let Err(e) = run_capture(buffer, stop_signal) {
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

        let samples = {
            let mut buf = self.buffer.lock().unwrap();
            std::mem::take(&mut *buf)
        };

        let duration = samples.len() as f64 / TARGET_SAMPLE_RATE as f64;
        info!(
            "Audio capture stopped: {} samples ({:.2}s)",
            samples.len(),
            duration
        );

        // Retain for retry
        self.last_captured = Some(samples.clone());

        samples
    }

    /// Get the last captured audio for retry
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
                        let float_data: Vec<f32> =
                            data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                        process_samples(
                            &float_data,
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
    // Mix to mono if multi-channel
    let mono: Vec<f32> = if channels > 1 {
        data.chunks(channels as usize)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    } else {
        data.to_vec()
    };

    // Simple nearest-neighbor resampling if needed
    let samples = if device_rate != TARGET_SAMPLE_RATE {
        let ratio = TARGET_SAMPLE_RATE as f64 / device_rate as f64;
        let out_len = (mono.len() as f64 * ratio) as usize;
        (0..out_len)
            .map(|i| {
                let src_idx = ((i as f64 / ratio) as usize).min(mono.len().saturating_sub(1));
                mono[src_idx]
            })
            .collect()
    } else {
        mono
    };

    // Append to buffer with size limit
    let mut buf = buffer.lock().unwrap();
    if buf.len() + samples.len() <= MAX_BUFFER_SAMPLES {
        buf.extend_from_slice(&samples);
    }
}
