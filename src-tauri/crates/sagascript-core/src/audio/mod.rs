// Live capture is optional (`record` feature) so a pure batch-transcribe
// build carries no cpal — and on Linux, no ALSA.
#[cfg(feature = "record")]
pub mod capture;
pub mod decoder;
pub mod resample;
pub mod speech;
pub mod wav;

pub use speech::has_audio_signal;

#[cfg(feature = "record")]
pub use capture::AudioCaptureService;
