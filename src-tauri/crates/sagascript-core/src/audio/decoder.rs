use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::formats::{FormatOptions, Packet};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use tracing::info;

use super::resample::resample_to_16khz_with_control;
use crate::error::DictationError;

/// Supported audio/video file extensions.
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "wav", "mp3", "m4a", "aac", "mp4", "mov", "ogg", "webm", "flac", "qta",
];

/// Hard ceiling on decoded audio length, expressed in samples of a 16kHz mono
/// clip of equivalent duration (~4 hours: `4 * 3600 * 16_000`). Complements the
/// byte budget ([`DecodeBudget`]): the decoder accumulates the whole mono
/// stream before resampling, and an adversarial/corrupt file can decode to far
/// more "audio" than its file size implies.
///
/// The check is done against a *duration-normalized* sample count (raw
/// accumulated samples, divided by channel count and source sample rate, then
/// scaled to 16kHz) rather than the literal raw interleaved sample count, so
/// the cap applies consistently regardless of the source format — a 4-hour
/// clip is rejected the same whether it's mono 16kHz or stereo 48kHz. This is
/// deliberately generous: no legitimate multi-hour podcast batch-
/// transcription job should ever hit it.
pub const MAX_DECODE_SAMPLES: usize = 4 * 3600 * 16_000; // ~230M, ~4h @ 16kHz mono equivalent

/// Check whether the accumulated decode so far has exceeded
/// [`MAX_DECODE_SAMPLES`], normalized to a 16kHz-mono-equivalent duration.
/// Factored out of the decode loop so it can be unit tested without
/// allocating gigabytes of PCM data.
fn check_decode_size_cap(
    raw_sample_count: usize,
    channels: usize,
    sample_rate: u32,
) -> Result<(), DictationError> {
    let channels = (channels.max(1)) as f64;
    let sample_rate = if sample_rate == 0 { 44_100.0 } else { sample_rate as f64 };

    let duration_secs = raw_sample_count as f64 / channels / sample_rate;
    let equivalent_16k_mono_samples = duration_secs * 16_000.0;

    if equivalent_16k_mono_samples > MAX_DECODE_SAMPLES as f64 {
        return Err(DictationError::FileDecodeError(format!(
            "Audio file is too long to decode (exceeds the ~4 hour / {MAX_DECODE_SAMPLES}-sample \
             16kHz-mono-equivalent limit); aborting to avoid unbounded memory use"
        )));
    }

    Ok(())
}

/// Memory budget for one decode, in bytes of `f32` PCM.
///
/// Decoding streams: each packet is downmixed to mono as it arrives, so the
/// source-rate multichannel PCM is never retained (only one packet's worth of
/// interleaved samples exists at a time). What stays resident is the
/// source-rate mono signal (4 bytes per source frame) plus, at the end, the
/// 16 kHz resampled copy. `max_total_bytes` bounds that peak
/// (`mono_frames * 4 + ceil(mono_frames * 16_000 / rate) * 4`) and
/// `max_packet_bytes` bounds the transient per-packet conversion buffers
/// (interleaved sample buffer plus its mono downmix).
///
/// 4 GiB admits the ~4 hour duration cap at the usual 44.1/48 kHz rates
/// (~3.5 GiB peak at 48 kHz) so long recordings keep working, while rejecting
/// 4 hours of 96 kHz+ audio (~6 GiB) that previously needed far more.
#[derive(Debug, Clone, Copy)]
struct DecodeBudget {
    max_total_bytes: usize,
    max_packet_bytes: usize,
}

const DEFAULT_DECODE_BUDGET: DecodeBudget = DecodeBudget {
    max_total_bytes: 4 * 1024 * 1024 * 1024,
    max_packet_bytes: 256 * 1024 * 1024,
};

const SAMPLE_BYTES: usize = std::mem::size_of::<f32>();

fn budget_error(what: &str) -> DictationError {
    DictationError::FileDecodeError(format!(
        "Audio file decodes to too much PCM data ({what}); aborting to avoid unbounded memory use"
    ))
}

/// `frames * channels * 4` with checked arithmetic.
fn pcm_bytes(frames: usize, channels: usize) -> Result<usize, DictationError> {
    frames
        .checked_mul(channels.max(1))
        .and_then(|samples| samples.checked_mul(SAMPLE_BYTES))
        .ok_or_else(|| budget_error("size overflow"))
}

/// Peak bytes while `mono_frames` source-rate frames are held and converted
/// to 16 kHz: the mono buffer plus the resampled output buffer.
fn conversion_peak_bytes(mono_frames: usize, sample_rate: u32) -> Result<usize, DictationError> {
    let rate = if sample_rate == 0 { 44_100 } else { sample_rate } as u128;
    let out_frames = (mono_frames as u128 * 16_000).div_ceil(rate);
    let out_frames = usize::try_from(out_frames).map_err(|_| budget_error("size overflow"))?;
    pcm_bytes(mono_frames, 1)?
        .checked_add(pcm_bytes(out_frames, 1)?)
        .ok_or_else(|| budget_error("size overflow"))
}

/// Check, before any allocation, that one decoded packet of `capacity_frames`
/// frames x `channels` fits the transient conversion budget (interleaved
/// buffer plus mono downmix).
fn check_packet_budget(
    capacity_frames: usize,
    channels: usize,
    budget: &DecodeBudget,
) -> Result<(), DictationError> {
    let bytes = pcm_bytes(capacity_frames, channels.max(1).saturating_add(1))?;
    if bytes > budget.max_packet_bytes {
        return Err(budget_error("single packet"));
    }
    Ok(())
}

/// Append `mono_add` frames (produced from `samples`) to `mono`, enforcing the
/// byte budget and the duration cap BEFORE the buffer grows. Growth is
/// geometric but never beyond the budget, and uses fallible reservation so an
/// allocation failure is an error rather than an abort.
fn append_mono_within_budget(
    mono: &mut Vec<f32>,
    add_frames: usize,
    sample_rate: u32,
    budget: &DecodeBudget,
    fill: impl FnOnce(&mut Vec<f32>),
) -> Result<(), DictationError> {
    let new_len = mono
        .len()
        .checked_add(add_frames)
        .ok_or_else(|| budget_error("size overflow"))?;
    if conversion_peak_bytes(new_len, sample_rate)? > budget.max_total_bytes {
        return Err(budget_error("total decoded audio"));
    }
    check_decode_size_cap(new_len, 1, sample_rate)?;
    if new_len > mono.capacity() {
        let ceiling = (budget.max_total_bytes / SAMPLE_BYTES).max(new_len);
        let target = mono.capacity().saturating_mul(2).clamp(new_len, ceiling);
        mono.try_reserve_exact(target - mono.len())
            .map_err(|_| budget_error("allocation refused"))?;
    }
    fill(mono);
    Ok(())
}

/// Decode an audio or video file to `Vec<f32>` at 16 kHz mono (Whisper input format).
///
/// Uses symphonia to probe the file format, find the first audio track,
/// decode all packets, then resample and mix to mono.
pub fn decode_audio_file(path: &Path) -> Result<Vec<f32>, DictationError> {
    decode_audio_file_inner(path, None, None, None)
}

/// Decode an audio or video file while checking a caller-owned cancellation or
/// progress callback at bounded decoder boundaries. Codec and resampler FFI
/// calls themselves are not interruptible.
pub fn decode_audio_file_with_control(
    path: &Path,
    checkpoint: &dyn Fn() -> Result<(), DictationError>,
) -> Result<Vec<f32>, DictationError> {
    decode_audio_file_inner(path, Some(checkpoint), None, None)
}

/// Measured decode fraction as a byte ratio (0–100): consumed audio-packet
/// bytes over source file size. The denominator is file size because
/// containers give no trustworthy total upfront (VBR headers lie, m4a often
/// omits frame counts) — so this is an I/O fraction, not a time fraction,
/// and container overhead means packet bytes asymptote below 100 (the caller
/// snaps to 100 on clean EOF). `total == 0` (unknown size) yields 0 and the
/// caller must not display any percent at all.
pub fn decode_percent(consumed_bytes: u64, total_bytes: u64) -> u8 {
    if total_bytes == 0 {
        return 0;
    }
    ((consumed_bytes as f64 / total_bytes as f64) * 100.0).clamp(0.0, 100.0) as u8
}

/// Decode with byte-fraction progress (`on_decode`, 0–100, called only on
/// strict increase, clamped to 99 until the post-resample 100) plus the
/// cancellable `checkpoint`. Either callback may be `None`.
pub fn decode_audio_file_with_progress(
    path: &Path,
    checkpoint: Option<&dyn Fn() -> Result<(), DictationError>>,
    on_decode: Option<&dyn Fn(u8)>,
) -> Result<Vec<f32>, DictationError> {
    decode_audio_file_inner(path, checkpoint, on_decode, None)
}

/// Independent progress for packet decoding and PCM conversion. The conversion
/// callback starts at zero before mixing and measures resampled input chunks.
/// These are stage percentages, never percentages of the entire transcription.
pub fn decode_audio_file_with_stage_progress(
    path: &Path,
    checkpoint: Option<&dyn Fn() -> Result<(), DictationError>>,
    on_decode: &dyn Fn(u8),
    on_resample: &dyn Fn(u8),
) -> Result<Vec<f32>, DictationError> {
    decode_audio_file_inner(path, checkpoint, Some(on_decode), Some(on_resample))
}

const MAX_CONSECUTIVE_DECODE_ERRORS: usize = 64;

fn note_decode_progress(consecutive_errors: &mut usize) {
    *consecutive_errors = 0;
}

fn note_decode_error(
    consecutive_errors: &mut usize,
    kind: &str,
) -> Result<(), DictationError> {
    *consecutive_errors = consecutive_errors.saturating_add(1);
    if *consecutive_errors >= MAX_CONSECUTIVE_DECODE_ERRORS {
        return Err(DictationError::FileDecodeError(format!(
            "Audio decoding aborted after too many consecutive {kind} errors"
        )));
    }
    Ok(())
}

/// Packet loop, separated from container probing so tests can drive it with a
/// scripted packet source and decoder.
#[allow(clippy::too_many_arguments)]
fn decode_packets(
    next_packet: &mut dyn FnMut() -> symphonia::core::errors::Result<Packet>,
    decoder: &mut dyn Decoder,
    track_id: u32,
    sample_rate: u32,
    codec_channels: usize,
    total_bytes: u64,
    checkpoint: Option<&dyn Fn() -> Result<(), DictationError>>,
    on_decode: Option<&dyn Fn(u8)>,
    budget: &DecodeBudget,
) -> Result<(Vec<f32>, usize), DictationError> {
    let mut consumed_bytes = 0u64;
    let mut last_decode_pct = 0u8;
    let mut mono: Vec<f32> = Vec::new();
    let mut actual_channels: usize = codec_channels.max(1);
    let mut consecutive_errors = 0usize;

    // Decode all packets
    loop {
        if let Some(checkpoint) = checkpoint {
            checkpoint()?;
        }
        let packet = match next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break; // End of stream
            }
            Err(e) => {
                // Non-fatal decode errors are skipped, but only within the
                // unconditional consecutive-error budget (at most
                // MAX_CONSECUTIVE_DECODE_ERRORS log lines per failing run).
                info!("Decode warning (skipping packet): {e}");
                note_decode_error(&mut consecutive_errors, "format")?;
                continue;
            }
        };

        // Skip packets from other tracks
        if packet.track_id() != track_id {
            continue;
        }

        // Measured progress: consumed packet bytes over file size. Counted
        // here (not after decode) so undecodable packets still count as
        // consumed input. Clamped to 99 — container overhead means packet
        // bytes asymptote below 100; clean EOF snaps to 100 after resample.
        if total_bytes > 0 {
            consumed_bytes = consumed_bytes.saturating_add(packet.data.len() as u64);
            let pct = decode_percent(consumed_bytes, total_bytes).min(99);
            if pct > last_decode_pct {
                last_decode_pct = pct;
                if let Some(on_decode) = on_decode {
                    on_decode(pct);
                }
            }
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => {
                note_decode_progress(&mut consecutive_errors);
                d
            }
            Err(e) => {
                info!("Decode warning (skipping packet): {e}");
                note_decode_error(&mut consecutive_errors, "codec")?;
                continue;
            }
        };

        if let Some(checkpoint) = checkpoint {
            checkpoint()?;
        }
        let spec = *decoded.spec();
        // Use actual channel count from decoded frame spec (more reliable than codec_params)
        actual_channels = spec.channels.count().max(1);
        let capacity = decoded.capacity();
        let frames = decoded.frames();

        // Budget checks happen before the sample buffer or `mono` can grow.
        check_packet_budget(capacity, actual_channels, budget)?;
        if let Some(checkpoint) = checkpoint {
            checkpoint()?;
        }

        let mut sample_buf = SampleBuffer::<f32>::new(capacity as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);
        let samples = sample_buf.samples();
        let channels = actual_channels;

        // Streaming downmix: average each frame's channels as it arrives so the
        // multichannel source-rate PCM is never accumulated.
        append_mono_within_budget(&mut mono, frames, sample_rate, budget, |mono| {
            if channels <= 1 {
                mono.extend_from_slice(samples);
            } else {
                mono.extend(
                    samples
                        .chunks(channels)
                        .map(|frame| frame.iter().sum::<f32>() / channels as f32),
                );
            }
        })?;
    }

    Ok((mono, actual_channels))
}

fn decode_audio_file_inner(
    path: &Path,
    checkpoint: Option<&dyn Fn() -> Result<(), DictationError>>,
    on_decode: Option<&dyn Fn(u8)>,
    on_resample: Option<&dyn Fn(u8)>,
) -> Result<Vec<f32>, DictationError> {
    if let Some(checkpoint) = checkpoint {
        checkpoint()?;
    }

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

    // Byte-fraction denominator for progress. File size is always available,
    // unlike packet counts or durations the container may omit or misreport.
    // Zero (unstatable file) disables percent emission entirely.
    let total_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    // Map format aliases: Symphonia doesn't know about .qta but decodes it as mov/isomp4
    let hint_ext = match ext.as_str() {
        "qta" => "mov",
        other => other,
    };
    let mut hint = Hint::new();
    hint.with_extension(hint_ext);

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
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44_100);
    // channels from codec_params can be wrong (e.g. AAC stereo reporting 1).
    // We'll detect the real channel count from the first decoded frame.
    let codec_channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(0);

    info!(
        "Decoding audio: {} Hz, codec_channels={}, codec {:?}",
        sample_rate, codec_channels, track.codec_params.codec
    );

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| {
            DictationError::FileDecodeError(format!("Failed to create decoder: {e}"))
        })?;

    let mut next_packet = || format.next_packet();
    let (mono, actual_channels) = decode_packets(
        &mut next_packet,
        decoder.as_mut(),
        track_id,
        sample_rate,
        codec_channels,
        total_bytes,
        checkpoint,
        on_decode,
        &DEFAULT_DECODE_BUDGET,
    )?;

    if mono.is_empty() {
        return Err(DictationError::FileDecodeError(
            "No audio samples decoded from file".to_string(),
        ));
    }

    let duration_secs = mono.len() as f64 / sample_rate as f64;
    info!(
        "Decoded {} mono frames ({:.1}s), {} ch at {} Hz, resampling to 16kHz mono",
        mono.len(),
        duration_secs,
        actual_channels,
        sample_rate,
    );

    // Packet decoding is complete. Reset the percentage for a separate stage:
    // sharing last_decode_pct here suppresses conversion updates until 99%.
    if let Some(checkpoint) = checkpoint {
        checkpoint()?;
    }
    if let Some(cb) = on_resample {
        if total_bytes > 0 {
            if let Some(decode) = on_decode { decode(100); }
        }
        cb(0);
    }
    let last_reported = std::cell::Cell::new(0u8);
    let resample_cb = on_resample.map(|cb| {
        move |frac: f64| {
            let pct = (frac * 100.0).clamp(0.0, 99.0) as u8;
            if pct > last_reported.get() {
                last_reported.set(pct);
                cb(pct);
            }
        }
    });
    let resampled = resample_to_16khz_with_control(
        mono,
        sample_rate,
        resample_cb.as_ref().map(|f| f as &dyn Fn(f64)),
        &|| checkpoint.map_or(Ok(()), |check| check().map_err(|error| error.to_string())),
    )
    .map_err(|e| DictationError::TranscriptionFailed(format!("Resample failed: {e}")))?;
    if let Some(cb) = on_resample {
        cb(100);
    } else if total_bytes > 0 {
        if let Some(on_decode) = on_decode {
            on_decode(100);
        }
    }
    if let Some(checkpoint) = checkpoint {
        checkpoint()?;
    }

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
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn supported_extensions_list() {
        assert!(SUPPORTED_EXTENSIONS.contains(&"wav"));
        assert!(SUPPORTED_EXTENSIONS.contains(&"mp3"));
        assert!(SUPPORTED_EXTENSIONS.contains(&"m4a"));
        assert!(SUPPORTED_EXTENSIONS.contains(&"flac"));
        assert!(SUPPORTED_EXTENSIONS.contains(&"ogg"));
        assert!(SUPPORTED_EXTENSIONS.contains(&"mp4"));
        assert!(SUPPORTED_EXTENSIONS.contains(&"mov"));
        assert!(SUPPORTED_EXTENSIONS.contains(&"qta"));
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
        let path = PathBuf::from("/tmp/definitely_does_not_exist_sagascript_test.wav");
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
    fn controlled_decode_checks_before_opening() {
        let calls = AtomicUsize::new(0);
        let checkpoint = || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(DictationError::FileDecodeError(
                "meeting decode cancelled".to_string(),
            ))
        };
        let path = PathBuf::from("/tmp/no-such-controlled-meeting-input.wav");

        let result = decode_audio_file_with_control(&path, &checkpoint);
        assert!(matches!(
            result,
            Err(DictationError::FileDecodeError(message))
                if message == "meeting decode cancelled"
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn controlled_decode_can_cancel_after_a_decoded_packet() {
        let path = std::env::temp_dir().join(format!(
            "sagascript-controlled-decode-{}.wav",
            uuid::Uuid::new_v4()
        ));
        let samples = vec![0.0_f32; 16_000];
        std::fs::write(&path, crate::audio::wav::encode_wav(&samples))
            .expect("write controlled decode fixture");

        let calls = AtomicUsize::new(0);
        let checkpoint = || {
            let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == 4 {
                Err(DictationError::FileDecodeError(
                    "meeting decode cancelled after packet".to_string(),
                ))
            } else {
                Ok(())
            }
        };
        let result = decode_audio_file_with_control(&path, &checkpoint);
        let _ = std::fs::remove_file(&path);

        assert!(matches!(
            result,
            Err(DictationError::FileDecodeError(message))
                if message == "meeting decode cancelled after packet"
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn consecutive_decode_error_budget_is_bounded_and_resets_on_progress() {
        let mut consecutive_errors = 0;
        for _ in 0..(MAX_CONSECUTIVE_DECODE_ERRORS - 1) {
            assert!(note_decode_error(&mut consecutive_errors, "codec").is_ok());
        }
        let error = note_decode_error(&mut consecutive_errors, "codec")
            .expect_err("the controlled decoder must stop after 64 errors");
        assert!(matches!(error, DictationError::FileDecodeError(message) if message.contains("64") || message.contains("too many")));

        note_decode_progress(&mut consecutive_errors);
        assert_eq!(consecutive_errors, 0);
        assert!(note_decode_error(&mut consecutive_errors, "format").is_ok());
    }

    #[test]
    fn decode_wav_from_encode() {
        // Create a valid WAV file from our own encoder, then decode it
        let original_samples: Vec<f32> = (0..16000)
            .map(|i| (i as f32 / 16000.0 * std::f32::consts::TAU * 440.0).sin())
            .collect();

        let wav_bytes = crate::audio::wav::encode_wav(&original_samples);

        // Write to temp file
        let tmp = std::env::temp_dir().join("sagascript_test_decode.wav");
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
    fn supported_extensions_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for ext in SUPPORTED_EXTENSIONS {
            assert!(!ext.is_empty(), "extension should not be empty");
            assert!(
                seen.insert(ext),
                "duplicate extension in SUPPORTED_EXTENSIONS: {ext}"
            );
        }
    }

    #[test]
    fn supported_extensions_no_leading_dot() {
        for ext in SUPPORTED_EXTENSIONS {
            assert!(
                !ext.starts_with('.'),
                "extension should not have leading dot: {ext}"
            );
        }
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

    // -- decode size cap --
    // These test the extracted check_decode_size_cap function directly
    // (constructing the accumulated-count condition rather than actually
    // decoding tens of GB of audio).

    #[test]
    fn decode_size_cap_rejects_over_limit() {
        // 5 hours of mono 16kHz raw samples — over the ~4h cap.
        let raw_samples = 5 * 3600 * 16_000;
        let result = check_decode_size_cap(raw_samples, 1, 16_000);
        assert!(result.is_err(), "5h mono 16kHz should exceed the cap");
        match result.unwrap_err() {
            DictationError::FileDecodeError(msg) => {
                assert!(
                    msg.contains("4 hour") || msg.contains(&MAX_DECODE_SAMPLES.to_string()),
                    "error should name the limit: {msg}"
                );
            }
            other => panic!("expected FileDecodeError, got: {:?}", other),
        }
    }

    #[test]
    fn decode_size_cap_allows_under_limit() {
        // 3 hours of stereo 48kHz raw samples — well under the cap once
        // normalized to a 16kHz-mono-equivalent duration.
        let raw_samples = 3 * 3600 * 48_000 * 2;
        let result = check_decode_size_cap(raw_samples, 2, 48_000);
        assert!(result.is_ok(), "3h stereo 48kHz should not be rejected: {:?}", result);
    }

    #[test]
    fn decode_size_cap_does_not_reject_legitimate_multi_hour_podcasts() {
        // A ~3h55m clip in several common real-world source formats should
        // all pass — this is the "generous enough" requirement: the cap must
        // not reject realistic multi-hour batch-transcription jobs.
        let almost_four_hours_secs = 3.917 * 3600.0; // ~3h 55m
        for (channels, sample_rate) in [
            (1u32, 16_000u32), // mono 16kHz (Whisper-native)
            (1, 8_000),        // mono 8kHz (phone-quality)
            (2, 44_100),       // stereo CD-quality
            (2, 48_000),       // stereo studio/podcast standard
        ] {
            let raw = (almost_four_hours_secs * channels as f64 * sample_rate as f64) as usize;
            let result = check_decode_size_cap(raw, channels as usize, sample_rate);
            assert!(
                result.is_ok(),
                "channels={channels} sample_rate={sample_rate} (~3h55m) should not be rejected: {:?}",
                result
            );
        }
    }

    #[test]
    fn decode_size_cap_treats_zero_channels_as_one() {
        // channels=0 is a defensive/malformed-metadata case; must not divide
        // by zero (NaN/inf) or panic.
        let raw_samples = 3600 * 16_000; // 1h mono-equivalent
        let result = check_decode_size_cap(raw_samples, 0, 16_000);
        assert!(result.is_ok());
    }

    #[test]
    fn decode_size_cap_treats_zero_sample_rate_as_default() {
        // sample_rate=0 would otherwise divide by zero; must not panic.
        let result = check_decode_size_cap(1_000_000, 1, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn existing_decode_calls_pass_the_cap_check() {
        // Sanity: the ~1s clip used by decode_wav_from_encode is nowhere
        // near the cap.
        let result = check_decode_size_cap(16_000, 1, 16_000);
        assert!(result.is_ok());
    }

    #[test]
    fn conversion_progress_is_not_suppressed_by_decode_reaching_99() {
        use std::cell::RefCell;
        let path = std::env::temp_dir().join(format!("sagascript-stages-{}.wav", uuid::Uuid::new_v4()));
        let mut wav = crate::audio::wav::encode_wav(&vec![0.1; 88_200]);
        wav[24..28].copy_from_slice(&44_100u32.to_le_bytes());
        wav[28..32].copy_from_slice(&88_200u32.to_le_bytes());
        std::fs::write(&path, wav).unwrap();
        let events = RefCell::new(Vec::new());
        let result = decode_audio_file_with_stage_progress(
            &path, None,
            &|pct| events.borrow_mut().push(("decoding", pct)),
            &|pct| events.borrow_mut().push(("resampling", pct)),
        );
        std::fs::remove_file(path).unwrap();
        assert!(!result.unwrap().is_empty());
        let events = events.borrow();
        let boundary = events.iter().position(|e| *e == ("resampling", 0)).unwrap();
        assert_eq!(events[boundary - 1], ("decoding", 100));
        assert!(events[..boundary].contains(&("decoding", 99)));
        let conversion = &events[boundary..];
        assert!(conversion.len() > 20, "must report during conversion, not just completion");
        assert!(conversion.iter().any(|(_, pct)| (20..80).contains(pct)));
        assert!(conversion.windows(2).all(|w| w[0].1 < w[1].1));
        assert_eq!(conversion.last(), Some(&("resampling", 100)));
    }

    #[test]
    fn decode_percent_is_a_clamped_byte_ratio() {
        assert_eq!(decode_percent(0, 1000), 0);
        assert_eq!(decode_percent(500, 1000), 50);
        assert_eq!(decode_percent(999, 1000), 99);
        assert_eq!(decode_percent(1000, 1000), 100);
        assert_eq!(decode_percent(1500, 1000), 100);
        // Unknown size disables display: never divide, never emit.
        assert_eq!(decode_percent(500, 0), 0);
        assert_eq!(decode_percent(0, 0), 0);
    }

    // -- scripted packet source / decoder for termination tests --

    use symphonia::core::audio::{AudioBuffer, AudioBufferRef, Channels, Signal, SignalSpec};
    use symphonia::core::codecs::{CodecDescriptor, CodecParameters, FinalizeResult};
    use symphonia::core::errors::Error as SymError;

    /// Decoder that always fails (`fail == true`) or returns a fixed buffer.
    struct ScriptedDecoder {
        params: CodecParameters,
        fail: bool,
        buf: AudioBuffer<f32>,
    }

    impl ScriptedDecoder {
        fn new(fail: bool, rate: u32, channels: Channels, frames: usize) -> Self {
            let spec = SignalSpec::new(rate, channels);
            let mut buf = AudioBuffer::<f32>::new(frames as u64, spec);
            buf.render_silence(Some(frames));
            Self { params: CodecParameters::new(), fail, buf }
        }
    }

    impl Decoder for ScriptedDecoder {
        fn try_new(_: &CodecParameters, _: &DecoderOptions) -> symphonia::core::errors::Result<Self> {
            unimplemented!()
        }
        fn supported_codecs() -> &'static [CodecDescriptor] {
            &[]
        }
        fn reset(&mut self) {}
        fn codec_params(&self) -> &CodecParameters {
            &self.params
        }
        fn decode(&mut self, _: &Packet) -> symphonia::core::errors::Result<AudioBufferRef<'_>> {
            if self.fail {
                Err(SymError::DecodeError("scripted codec failure"))
            } else {
                Ok(AudioBufferRef::F32(std::borrow::Cow::Borrowed(&self.buf)))
            }
        }
        fn finalize(&mut self) -> FinalizeResult {
            FinalizeResult::default()
        }
        fn last_decoded(&self) -> AudioBufferRef<'_> {
            AudioBufferRef::F32(std::borrow::Cow::Borrowed(&self.buf))
        }
    }

    /// Run the loop with a source that yields `script_len` non-EOF items
    /// (format errors when `format_errors`, else packets for a decoder that
    /// fails) and then a clean EOF, so a missing budget terminates the test
    /// instead of hanging it. Returns (result, items pulled).
    fn run_error_script(
        format_errors: bool,
        script_len: usize,
        checkpoint: Option<&dyn Fn() -> Result<(), DictationError>>,
    ) -> (Result<(Vec<f32>, usize), DictationError>, usize) {
        let pulled = std::cell::Cell::new(0usize);
        let mut next = || {
            let n = pulled.get();
            pulled.set(n + 1);
            if n >= script_len {
                return Err(SymError::IoError(std::io::Error::from(
                    std::io::ErrorKind::UnexpectedEof,
                )));
            }
            if format_errors {
                Err(SymError::DecodeError("scripted format failure"))
            } else {
                Ok(Packet::new_from_slice(0, 0, 0, &[0u8; 4]))
            }
        };
        let mut decoder = ScriptedDecoder::new(true, 16_000, Channels::FRONT_LEFT, 16);
        let result = decode_packets(&mut next, &mut decoder, 0, 16_000, 1, 0, checkpoint, None, &DEFAULT_DECODE_BUDGET);
        (result, pulled.get())
    }

    fn assert_bounded_failure(
        outcome: (Result<(Vec<f32>, usize), DictationError>, usize),
        what: &str,
    ) {
        let (result, pulled) = outcome;
        match result {
            Err(DictationError::FileDecodeError(message)) => {
                assert!(message.contains("too many consecutive"), "{what}: {message}");
            }
            other => panic!("{what}: expected bounded failure, got {other:?}"),
        }
        assert_eq!(pulled, MAX_CONSECUTIVE_DECODE_ERRORS, "{what}: pulls before abort");
    }

    #[test]
    fn callback_free_decode_stops_on_endless_format_errors() {
        assert_bounded_failure(run_error_script(true, 100_000, None), "callback-free format");
    }

    #[test]
    fn callback_free_decode_stops_on_endless_codec_errors() {
        assert_bounded_failure(run_error_script(false, 100_000, None), "callback-free codec");
    }

    #[test]
    fn controlled_decode_stops_on_endless_errors() {
        let checkpoint = || Ok(());
        assert_bounded_failure(run_error_script(true, 100_000, Some(&checkpoint)), "controlled format");
        assert_bounded_failure(run_error_script(false, 100_000, Some(&checkpoint)), "controlled codec");
    }

    #[test]
    fn progress_callbacks_do_not_change_error_budget() {
        let pulled = std::cell::Cell::new(0usize);
        let mut next = || {
            pulled.set(pulled.get() + 1);
            if pulled.get() > 100_000 {
                return Err(SymError::IoError(std::io::Error::from(
                    std::io::ErrorKind::UnexpectedEof,
                )));
            }
            Err(SymError::DecodeError("scripted format failure"))
        };
        let mut decoder = ScriptedDecoder::new(true, 16_000, Channels::FRONT_LEFT, 16);
        let on_decode = |_: u8| {};
        let result = decode_packets(&mut next, &mut decoder, 0, 16_000, 1, 1000, None, Some(&on_decode), &DEFAULT_DECODE_BUDGET);
        assert!(result.is_err());
        assert_eq!(pulled.get(), MAX_CONSECUTIVE_DECODE_ERRORS);
    }

    #[test]
    fn invalid_media_fails_promptly_through_both_entry_points() {
        let path = std::env::temp_dir().join(format!("sagascript-invalid-{}.mp3", uuid::Uuid::new_v4()));
        let garbage: Vec<u8> = [0xFFu8, 0xFB, 0x90, 0x00].into_iter().cycle().take(64 * 1024).collect();
        std::fs::write(&path, garbage).unwrap();
        let start = std::time::Instant::now();
        let plain = decode_audio_file(&path);
        let controlled = decode_audio_file_with_control(&path, &|| Ok(()));
        std::fs::remove_file(&path).unwrap();
        assert!(plain.is_err());
        assert!(controlled.is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(10));
    }

    // -- decoded-byte budget --

    #[test]
    fn pcm_size_arithmetic_is_checked_for_extreme_shapes() {
        // No allocation: pure arithmetic.
        assert_eq!(pcm_bytes(1_000, 2).unwrap(), 8_000);
        // 4 h of 192 kHz x 32 channels is ~354 GB; must compute, not overflow.
        let frames = 4 * 3600 * 192_000;
        assert_eq!(pcm_bytes(frames, 32).unwrap(), frames * 32 * 4);
        assert!(pcm_bytes(usize::MAX, 2).is_err());
        assert!(pcm_bytes(usize::MAX / 4 + 1, 1).is_err());
        assert!(check_packet_budget(usize::MAX, 32, &DEFAULT_DECODE_BUDGET).is_err());
        assert!(conversion_peak_bytes(usize::MAX, 8_000).is_err());
        assert!(conversion_peak_bytes(usize::MAX, 0).is_err());
    }

    #[test]
    fn budget_admits_long_ordinary_recordings_and_rejects_extreme_rates() {
        let hours = |h: usize, rate: usize| h * 3600 * rate;
        let b = DEFAULT_DECODE_BUDGET.max_total_bytes;
        for rate in [8_000u32, 16_000, 44_100, 48_000] {
            let peak = conversion_peak_bytes(hours(4, rate as usize), rate).unwrap();
            assert!(peak <= b, "4h at {rate} Hz must fit, peak={peak}");
        }
        assert!(conversion_peak_bytes(hours(1, 192_000), 192_000).unwrap() <= b);
        assert!(conversion_peak_bytes(hours(4, 96_000), 96_000).unwrap() > b);
        assert!(conversion_peak_bytes(hours(4, 192_000), 192_000).unwrap() > b);
    }

    #[test]
    fn packet_budget_counts_interleaved_and_mono_buffers() {
        let budget = DecodeBudget { max_total_bytes: usize::MAX, max_packet_bytes: 12_000 };
        // 1000 frames x (2 channels + 1 mono) x 4 bytes == 12_000: fits.
        assert!(check_packet_budget(1_000, 2, &budget).is_ok());
        assert!(check_packet_budget(1_001, 2, &budget).is_err());
        // Many channels blow the same packet budget at far fewer frames.
        assert!(check_packet_budget(1_000, 32, &budget).is_err());
    }

    #[test]
    fn budget_rejection_happens_before_buffer_growth() {
        let budget = DecodeBudget { max_total_bytes: 4_000, max_packet_bytes: usize::MAX };
        let mut mono: Vec<f32> = Vec::with_capacity(10);
        mono.extend_from_slice(&[0.5; 10]);
        let capacity = mono.capacity();
        let filled = std::cell::Cell::new(false);
        // 16 kHz: peak = 2 * 4 bytes per frame; 10 + 500 frames = 4_080 > 4_000.
        let result = append_mono_within_budget(&mut mono, 500, 16_000, &budget, |_| filled.set(true));
        assert!(matches!(result, Err(DictationError::FileDecodeError(_))));
        assert!(!filled.get(), "fill must not run after a rejection");
        assert_eq!(mono.len(), 10);
        assert_eq!(mono.capacity(), capacity, "buffer must not grow on rejection");
        // Within budget it grows, never past the budget ceiling.
        append_mono_within_budget(&mut mono, 100, 16_000, &budget, |m| m.extend_from_slice(&[0.0; 100])).unwrap();
        assert_eq!(mono.len(), 110);
        assert!(mono.capacity() <= budget.max_total_bytes / SAMPLE_BYTES);
    }

    #[test]
    fn decode_loop_aborts_on_oversized_packet_without_decoding_further() {
        // 1000-frame stereo packets against a 4 KiB per-packet budget.
        let budget = DecodeBudget { max_total_bytes: usize::MAX, max_packet_bytes: 4_096 };
        let pulled = std::cell::Cell::new(0usize);
        let mut next = || {
            pulled.set(pulled.get() + 1);
            Ok(Packet::new_from_slice(0, 0, 0, &[0u8; 4]))
        };
        let mut decoder = ScriptedDecoder::new(false, 48_000, Channels::FRONT_LEFT | Channels::FRONT_RIGHT, 1_000);
        let result = decode_packets(&mut next, &mut decoder, 0, 48_000, 2, 0, None, None, &budget);
        assert!(matches!(result, Err(DictationError::FileDecodeError(_))));
        assert_eq!(pulled.get(), 1);
    }

    #[test]
    fn decode_loop_downmixes_streaming_and_stops_at_total_budget() {
        // 100-frame stereo packets (L=1, R=0 after silence is 0 -> mono 0).
        let budget = DecodeBudget { max_total_bytes: 3_200, max_packet_bytes: usize::MAX };
        let pulled = std::cell::Cell::new(0usize);
        let mut next = || {
            pulled.set(pulled.get() + 1);
            Ok(Packet::new_from_slice(0, 0, 0, &[0u8; 4]))
        };
        let mut decoder = ScriptedDecoder::new(false, 16_000, Channels::FRONT_LEFT | Channels::FRONT_RIGHT, 100);
        let result = decode_packets(&mut next, &mut decoder, 0, 16_000, 2, 0, None, None, &budget);
        // Each packet adds 100 mono frames = 800 bytes of peak at 16 kHz; the
        // 5th packet (500 frames = 4_000 > 3_200) is rejected: 4 pulls... the
        // 4th (400 frames = 3_200) still fits, so rejection is on pull 5.
        assert!(matches!(result, Err(DictationError::FileDecodeError(_))));
        assert_eq!(pulled.get(), 5);
    }
}
