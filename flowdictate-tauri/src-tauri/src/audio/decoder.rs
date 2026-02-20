use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use tracing::info;

use super::resample::{mix_to_mono, resample_to_16khz};
use crate::error::DictationError;

/// Supported audio/video file extensions.
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "wav", "mp3", "m4a", "aac", "mp4", "mov", "ogg", "webm", "flac",
];

/// Decode an audio or video file to `Vec<f32>` at 16 kHz mono (Whisper input format).
///
/// Uses symphonia to probe the file format, find the first audio track,
/// decode all packets, then resample and mix to mono.
pub fn decode_audio_file(path: &Path) -> Result<Vec<f32>, DictationError> {
    // Validate extension
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if !SUPPORTED_EXTENSIONS.contains(&ext.as_str()) {
        return Err(DictationError::UnsupportedFormat(format!(
            "'.{ext}' is not supported. Supported formats: {}",
            SUPPORTED_EXTENSIONS.join(", ")
        )));
    }

    let file = std::fs::File::open(path).map_err(|e| {
        DictationError::FileDecodeError(format!("Failed to open file: {e}"))
    })?;

    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    hint.with_extension(&ext);

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| {
            DictationError::FileDecodeError(format!("Failed to probe file format: {e}"))
        })?;

    let mut format = probed.format;

    // Find the first audio track
    let track = format
        .tracks()
        .iter()
        .find(|t| {
            t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL
        })
        .ok_or_else(|| {
            DictationError::FileDecodeError("No audio track found in file".to_string())
        })?;

    let track_id = track.id;
    let channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(1);
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44_100);

    info!(
        "Decoding audio: {} Hz, {} ch, codec {:?}",
        sample_rate, channels, track.codec_params.codec
    );

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| {
            DictationError::FileDecodeError(format!("Failed to create decoder: {e}"))
        })?;

    let mut all_samples: Vec<f32> = Vec::new();

    // Decode all packets
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break; // End of stream
            }
            Err(e) => {
                // Log non-fatal decode errors and continue
                info!("Decode warning (skipping packet): {e}");
                continue;
            }
        };

        // Skip packets from other tracks
        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(e) => {
                info!("Decode warning (skipping packet): {e}");
                continue;
            }
        };

        let spec = *decoded.spec();
        let num_frames = decoded.capacity();

        let mut sample_buf = SampleBuffer::<f32>::new(num_frames as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);

        all_samples.extend_from_slice(sample_buf.samples());
    }

    if all_samples.is_empty() {
        return Err(DictationError::FileDecodeError(
            "No audio samples decoded from file".to_string(),
        ));
    }

    let duration_secs = all_samples.len() as f64 / (sample_rate as f64 * channels as f64);
    info!(
        "Decoded {} raw samples ({:.1}s), resampling to 16kHz mono",
        all_samples.len(),
        duration_secs
    );

    // Mix to mono and resample
    let mono = mix_to_mono(&all_samples, channels);
    let resampled = resample_to_16khz(mono, sample_rate);

    info!(
        "Resampled to {} samples ({:.1}s at 16kHz)",
        resampled.len(),
        resampled.len() as f64 / 16_000.0
    );

    Ok(resampled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn supported_extensions_list() {
        assert!(SUPPORTED_EXTENSIONS.contains(&"wav"));
        assert!(SUPPORTED_EXTENSIONS.contains(&"mp3"));
        assert!(SUPPORTED_EXTENSIONS.contains(&"m4a"));
        assert!(SUPPORTED_EXTENSIONS.contains(&"flac"));
        assert!(SUPPORTED_EXTENSIONS.contains(&"ogg"));
        assert!(SUPPORTED_EXTENSIONS.contains(&"mp4"));
        assert!(SUPPORTED_EXTENSIONS.contains(&"mov"));
        assert!(SUPPORTED_EXTENSIONS.contains(&"webm"));
        assert!(SUPPORTED_EXTENSIONS.contains(&"aac"));
    }

    #[test]
    fn unsupported_extension_returns_error() {
        let path = PathBuf::from("/tmp/test.xyz");
        let result = decode_audio_file(&path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            DictationError::UnsupportedFormat(msg) => {
                assert!(msg.contains(".xyz"), "error should mention extension: {msg}");
                assert!(msg.contains("wav"), "error should list supported formats: {msg}");
            }
            other => panic!("expected UnsupportedFormat, got: {:?}", other),
        }
    }

    #[test]
    fn no_extension_returns_error() {
        let path = PathBuf::from("/tmp/testfile");
        let result = decode_audio_file(&path);
        assert!(result.is_err());
    }

    #[test]
    fn nonexistent_file_returns_error() {
        let path = PathBuf::from("/tmp/definitely_does_not_exist_flowdictate_test.wav");
        let result = decode_audio_file(&path);
        assert!(result.is_err());
        match result.unwrap_err() {
            DictationError::FileDecodeError(msg) => {
                assert!(msg.contains("open"), "error should mention opening: {msg}");
            }
            other => panic!("expected FileDecodeError, got: {:?}", other),
        }
    }

    #[test]
    fn decode_wav_from_encode() {
        // Create a valid WAV file from our own encoder, then decode it
        let original_samples: Vec<f32> = (0..16000)
            .map(|i| (i as f32 / 16000.0 * std::f32::consts::TAU * 440.0).sin())
            .collect();

        let wav_bytes = crate::audio::wav::encode_wav(&original_samples);

        // Write to temp file
        let tmp = std::env::temp_dir().join("flowdictate_test_decode.wav");
        std::fs::write(&tmp, &wav_bytes).unwrap();

        let result = decode_audio_file(&tmp);
        // Clean up
        let _ = std::fs::remove_file(&tmp);

        let decoded = result.unwrap();
        // Should be approximately 16000 samples (at 16kHz, same rate → no resampling)
        assert!(
            (decoded.len() as i64 - 16000).abs() < 100,
            "expected ~16000 samples, got {}",
            decoded.len()
        );
    }

    #[test]
    fn case_insensitive_extension() {
        // The code lowercases the extension, so .WAV should work (file-not-found, not unsupported)
        let path = PathBuf::from("/tmp/definitely_does_not_exist.WAV");
        let result = decode_audio_file(&path);
        assert!(result.is_err());
        // Should be a FileDecodeError (file not found), not UnsupportedFormat
        match result.unwrap_err() {
            DictationError::FileDecodeError(_) => {} // expected
            DictationError::UnsupportedFormat(msg) => {
                panic!("uppercase .WAV should be treated as supported, got UnsupportedFormat: {msg}")
            }
            other => panic!("unexpected error: {:?}", other),
        }
    }
}
