use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CodecParameters, Decoder, DecoderOptions, CODEC_TYPE_ALAC};
use symphonia::core::formats::{FormatOptions, FormatReader, Packet};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use tracing::info;

use super::resample::{
    resample_to_16khz_bounded, StreamResampleError, StreamingResampler, STREAM_CHUNK,
    TARGET_SAMPLE_RATE,
};
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
/// transcription job should ever hit it. The streaming decode
/// ([`decode_audio_file_streaming`]) keeps nothing, but counts its 16 kHz
/// output against the same limit.
pub(crate) const MAX_DECODE_SAMPLES: usize = 4 * 3600 * 16_000; // ~230M, ~4h @ 16kHz mono equivalent

/// Check whether the accumulated decode so far has exceeded
/// [`MAX_DECODE_SAMPLES`], normalized to a 16kHz-mono-equivalent duration.
/// Factored out of the decode loop so it can be unit tested without
/// allocating gigabytes of PCM data.
#[cfg(test)]
fn check_decode_size_cap(
    raw_sample_count: usize,
    channels: usize,
    sample_rate: u32,
) -> Result<(), DictationError> {
    check_decode_size_cap_within(MAX_DECODE_SAMPLES, raw_sample_count, channels, sample_rate)
}

/// [`check_decode_size_cap`] against an explicit limit (in 16 kHz mono samples).
fn check_decode_size_cap_within(
    limit: usize,
    raw_sample_count: usize,
    channels: usize,
    sample_rate: u32,
) -> Result<(), DictationError> {
    let channels = (channels.max(1)) as f64;
    let sample_rate = if sample_rate == 0 {
        44_100.0
    } else {
        sample_rate as f64
    };

    let duration_secs = raw_sample_count as f64 / channels / sample_rate;
    let equivalent_16k_mono_samples = duration_secs * 16_000.0;

    if equivalent_16k_mono_samples > limit as f64 {
        return Err(DictationError::FileDecodeError(format!(
            "Audio file is too long to decode (exceeds the ~4 hour / {limit}-sample \
             16kHz-mono-equivalent limit); aborting to avoid unbounded memory use"
        )));
    }

    Ok(())
}

/// Memory budget for one decode, in bytes. All arithmetic is `u64` with checked
/// conversion to `usize`, so it is independent of the target's pointer width.
///
/// Scope: this budget covers Sagascript's OWN decode, downmix and resample
/// buffers, plus the checked codec constructors (ALAC, PCM, ADPCM; FLAC, AAC
/// and MP3 are bounded by the codecs themselves). Symphonia's container
/// parsing (ISOMP4, MKV, core io) and Vorbis setup allocate outside this
/// budget; see follow-up issue #319.
///
/// Decoding streams: each packet is downmixed to mono as it arrives, so the
/// source-rate multichannel PCM is never retained. What stays resident is the
/// source-rate mono signal (4 bytes per source frame, counted by allocated
/// *capacity*) plus, at the end, the 16 kHz resampled copy.
///
/// * `max_total_bytes` is a ceiling on the real peak of those allocations:
///   `capacity * 4 + resampled_output + conversion_scratch_bytes`, and also on
///   the transient old+new overlap while the mono buffer reallocates
///   (`old + new capacity`). Mono capacity only takes values on a halving
///   grid below [`max_mono_frames`] (`ceiling >> j`), so a growth step never
///   has `old > new / 2`; each reservation is additionally checked against
///   the actual `old + new` capacity plus the other live buffers.
/// * `conversion_scratch_bytes` covers resampler state, padding and flush.
/// * `max_packet_bytes` bounds per-packet decoder/conversion buffers, both the
///   codec's own buffers (checked before the decoder is constructed) and ours.
///
/// 4 GiB admits the ~4 hour duration cap at 8/16/44.1/48 kHz (real conversion
/// peak ~3.4 GiB at 48 kHz) while rejecting 4 hours at 96 kHz or higher.
#[derive(Debug, Clone, Copy)]
struct DecodeBudget {
    max_total_bytes: u64,
    max_packet_bytes: u64,
    conversion_scratch_bytes: u64,
    /// The duration cap, as 16 kHz mono samples ([`MAX_DECODE_SAMPLES`] in
    /// production; a field so that tests can use a small one).
    max_duration_samples: usize,
}

const DEFAULT_DECODE_BUDGET: DecodeBudget = DecodeBudget {
    max_total_bytes: 4 * 1024 * 1024 * 1024,
    max_packet_bytes: 64 * 1024 * 1024,
    conversion_scratch_bytes: 64 * 1024 * 1024,
    max_duration_samples: MAX_DECODE_SAMPLES,
};

const SAMPLE_BYTES: u64 = std::mem::size_of::<f32>() as u64;

fn budget_error(what: &str) -> DictationError {
    DictationError::FileDecodeError(format!(
        "Audio file decodes to too much PCM data ({what}); aborting to avoid unbounded memory use"
    ))
}

/// `frames * channels * 4` with checked arithmetic.
fn pcm_bytes(frames: u64, channels: u64) -> Result<u64, DictationError> {
    frames
        .checked_mul(channels.max(1))
        .and_then(|samples| samples.checked_mul(SAMPLE_BYTES))
        .ok_or_else(|| budget_error("size overflow"))
}

fn effective_rate(sample_rate: u32) -> u128 {
    if sample_rate == 0 {
        44_100
    } else {
        sample_rate as u128
    }
}

/// Real peak bytes when `mono_capacity` source-rate frames are allocated and
/// converted to 16 kHz: mono buffer + output buffer + scratch.
fn conversion_peak_bytes(
    mono_capacity: u64,
    sample_rate: u32,
    budget: &DecodeBudget,
) -> Result<u64, DictationError> {
    let out_frames = (mono_capacity as u128 * 16_000).div_ceil(effective_rate(sample_rate));
    let out_frames = u64::try_from(out_frames).map_err(|_| budget_error("size overflow"))?;
    pcm_bytes(mono_capacity, 1)?
        .checked_add(pcm_bytes(out_frames, 1)?)
        .and_then(|b| b.checked_add(budget.conversion_scratch_bytes))
        .ok_or_else(|| budget_error("size overflow"))
}

/// Largest mono capacity (frames) whose conversion peak AND reallocation
/// overlap (`1.5x` the mono buffer on the halving growth grid) both stay
/// within `max_total_bytes`, leaving `max_packet_bytes` for packet buffers.
fn max_mono_frames(sample_rate: u32, budget: &DecodeBudget) -> u64 {
    let rate = effective_rate(sample_rate);
    // Keep room for scratch and for the live per-packet buffers.
    let avail = budget
        .max_total_bytes
        .saturating_sub(budget.conversion_scratch_bytes)
        .saturating_sub(budget.max_packet_bytes.min(budget.max_total_bytes / 16))
        as u128;
    // Per-frame cost in units of 1/rate bytes: max(6, 4 + 4*16000/rate) bytes.
    let per_frame_x_rate = (6 * rate).max(4 * rate + 64_000);
    let frames = avail * rate / per_frame_x_rate;
    let by_address_space = (isize::MAX as u128) / SAMPLE_BYTES as u128;
    u64::try_from(frames.min(by_address_space)).unwrap_or(u64::MAX)
}

/// Check, before any allocation, that one decoded packet of `capacity_frames`
/// frames x `channels` fits the transient conversion budget (interleaved
/// buffer plus mono downmix).
fn check_packet_budget(
    capacity_frames: u64,
    channels: u64,
    budget: &DecodeBudget,
) -> Result<(), DictationError> {
    let bytes = pcm_bytes(capacity_frames, channels.max(1).saturating_add(1))?;
    if bytes > budget.max_packet_bytes {
        return Err(budget_error("single packet"));
    }
    Ok(())
}

/// Reject codec parameters whose decoder would allocate an oversized buffer in
/// its constructor. Must run BEFORE the decoder is built: symphonia's
/// constructors allocate infallibly from file-supplied shapes.
///
/// * ALAC reads `frame_length` (u32) and the channel count from the magic
///   cookie and allocates `frame_length x channels` plus a tail buffer.
/// * PCM/ADPCM allocate `max_frames_per_packet x channels` in the widest
///   sample type (8 bytes).
/// * FLAC (u16 block size, <= 8 ch), AAC (1024 frames, <= 2 ch), MP3 (1152
///   frames) and the Vorbis output buffer (block size <= 2^15, <= 8 ch) have
///   shapes the codec itself bounds well below the packet budget.
///
/// Not covered (follow-up issue #319): Symphonia's own container parsing
/// (ISOMP4 stsz/packet storage, MKV EBML lengths, core io) and Vorbis setup
/// codebooks allocate before or outside this check.
fn check_codec_allocation(
    params: &CodecParameters,
    budget: &DecodeBudget,
) -> Result<(), DictationError> {
    let mut worst = 0u64;
    if params.codec == CODEC_TYPE_ALAC {
        if let Some(cookie) = params.extra_data.as_deref() {
            // 24- or 48-byte cookie: frame_length at 0..4, num_channels at 9.
            if (cookie.len() == 24 || cookie.len() == 48) && cookie.len() > 9 {
                let frame_length =
                    u32::from_be_bytes([cookie[0], cookie[1], cookie[2], cookie[3]]) as u64;
                let channels = cookie[9] as u64;
                // Output buffer (i32) + tail buffer (<= 2 ch x i32) per frame.
                worst = worst.max(pcm_bytes(frame_length, channels.saturating_add(2))?);
            }
        }
    }
    if let Some(frames) = params.max_frames_per_packet {
        let channels = params
            .channels
            .map(|c| c.count() as u64)
            .or_else(|| {
                params
                    .channel_layout
                    .map(|l| l.into_channels().count() as u64)
            })
            .unwrap_or(1);
        // Widest sample type is f64: 8 bytes per sample.
        worst = worst.max(
            pcm_bytes(frames, channels.max(1))?
                .checked_mul(2)
                .ok_or_else(|| budget_error("size overflow"))?,
        );
    }
    if worst > budget.max_packet_bytes {
        return Err(budget_error("codec buffers"));
    }
    Ok(())
}

fn make_decoder(
    params: &CodecParameters,
    budget: &DecodeBudget,
) -> Result<Box<dyn Decoder>, DictationError> {
    check_codec_allocation(params, budget)?;
    symphonia::default::get_codecs()
        .make(params, &DecoderOptions::default())
        .map_err(|e| DictationError::FileDecodeError(format!("Failed to create decoder: {e}")))
}

/// Smallest capacity on the halving grid `ceiling >> j` that holds `new_len`.
fn grid_capacity(new_len: usize, ceiling: usize) -> usize {
    let mut c = ceiling.max(new_len);
    while c / 2 >= new_len && c / 2 >= 4096 {
        c /= 2;
    }
    c
}

/// Append `add_frames` frames to `mono`, enforcing the byte budget and the
/// duration cap BEFORE the buffer grows. `other_live_bytes` is what else is
/// allocated right now (current packet buffers). Before reserving, the real
/// reallocation footprint `(old + new capacity) * 4 + other + scratch` is
/// checked against the budget; the target is reduced to the exact need, or the
/// call is rejected, when it does not fit. Fallible reservation turns an
/// allocation failure into an error rather than an abort.
fn append_mono_within_budget(
    mono: &mut Vec<f32>,
    add_frames: usize,
    sample_rate: u32,
    budget: &DecodeBudget,
    other_live_bytes: u64,
    fill: impl FnOnce(&mut Vec<f32>),
) -> Result<(), DictationError> {
    let new_len = mono
        .len()
        .checked_add(add_frames)
        .ok_or_else(|| budget_error("size overflow"))?;
    let ceiling = max_mono_frames(sample_rate, budget);
    if new_len as u64 > ceiling {
        return Err(budget_error("total decoded audio"));
    }
    check_decode_size_cap_within(budget.max_duration_samples, new_len, 1, sample_rate)?;
    if new_len > mono.capacity() {
        let old = mono.capacity() as u64;
        let fits = |target: usize| -> bool {
            old.checked_add(target as u64)
                .and_then(|frames| pcm_bytes(frames, 1).ok())
                .and_then(|b| b.checked_add(other_live_bytes))
                .and_then(|b| b.checked_add(budget.conversion_scratch_bytes))
                .is_some_and(|b| b <= budget.max_total_bytes)
        };
        let ceiling = usize::try_from(ceiling).unwrap_or(usize::MAX);
        let mut target = grid_capacity(new_len, ceiling);
        if !fits(target) {
            target = new_len;
            if !fits(target) {
                return Err(budget_error("reallocation overlap"));
            }
        }
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
pub(crate) fn decode_percent(consumed_bytes: u64, total_bytes: u64) -> u8 {
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

fn note_decode_error(consecutive_errors: &mut usize, kind: &str) -> Result<(), DictationError> {
    *consecutive_errors = consecutive_errors.saturating_add(1);
    if *consecutive_errors >= MAX_CONSECUTIVE_DECODE_ERRORS {
        return Err(DictationError::FileDecodeError(format!(
            "Audio decoding aborted after too many consecutive {kind} errors"
        )));
    }
    Ok(())
}

/// One decoded packet as the packet loop hands it to its consumer.
struct PacketAudio<'a> {
    /// Interleaved samples, `frames * channels` of them.
    interleaved: &'a [f32],
    frames: usize,
    channels: usize,
    /// Capacity of the decoded buffer, in frames (for the live-buffer budget).
    capacity: usize,
}

/// Packet loop, separated from container probing so tests can drive it with a
/// scripted packet source and decoder. Every decoded packet goes to `on_audio`,
/// after the per-packet budget check and before the next packet is pulled.
/// Returns the channel count of the last decoded packet (`codec_channels` if
/// none decoded).
///
/// `consumed` (when given, and the file size is known) is kept at the
/// audio-packet bytes consumed so far, for progress estimates.
#[allow(clippy::too_many_arguments)]
fn run_packet_loop(
    next_packet: &mut dyn FnMut() -> symphonia::core::errors::Result<Packet>,
    decoder: &mut dyn Decoder,
    track_id: u32,
    codec_channels: usize,
    total_bytes: u64,
    checkpoint: Option<&dyn Fn() -> Result<(), DictationError>>,
    on_decode: Option<&dyn Fn(u8)>,
    consumed: Option<&AtomicU64>,
    budget: &DecodeBudget,
    on_audio: &mut dyn FnMut(PacketAudio<'_>) -> Result<(), DictationError>,
) -> Result<usize, DictationError> {
    let mut consumed_bytes = 0u64;
    let mut last_decode_pct = 0u8;
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
            if let Some(consumed) = consumed {
                consumed.store(consumed_bytes, Ordering::Relaxed);
            }
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

        // Budget checks happen before the sample buffer or the consumer's
        // buffers can grow.
        check_packet_budget(capacity as u64, actual_channels as u64, budget)?;
        if let Some(checkpoint) = checkpoint {
            checkpoint()?;
        }

        let mut sample_buf = SampleBuffer::<f32>::new(capacity as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);
        on_audio(PacketAudio {
            interleaved: sample_buf.samples(),
            frames,
            channels: actual_channels,
            capacity,
        })?;
    }

    Ok(actual_channels)
}

/// Average each frame's channels and append the mono frames to `mono`: the
/// streaming downmix both decode paths use, so their values are identical.
fn downmix_extend(audio: &PacketAudio<'_>, mono: &mut Vec<f32>) {
    if audio.channels <= 1 {
        mono.extend_from_slice(audio.interleaved);
    } else {
        let channels = audio.channels;
        mono.extend(
            audio
                .interleaved
                .chunks(channels)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32),
        );
    }
}

/// Accumulating packet loop of the batch decode: each packet is downmixed to
/// mono as it arrives (the multichannel source-rate PCM is never retained) and
/// appended to one source-rate mono buffer, within the byte budget.
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
    let mut mono: Vec<f32> = Vec::new();
    let actual_channels = run_packet_loop(
        next_packet,
        decoder,
        track_id,
        codec_channels,
        total_bytes,
        checkpoint,
        on_decode,
        None,
        budget,
        &mut |audio| {
            // Live besides `mono`: the interleaved sample buffer just allocated.
            let other_live = pcm_bytes(audio.capacity as u64, audio.channels as u64)?;
            append_mono_within_budget(
                &mut mono,
                audio.frames,
                sample_rate,
                budget,
                other_live,
                |mono| downmix_extend(&audio, mono),
            )
        },
    )?;
    Ok((mono, actual_channels))
}

/// A probed file with its decoder ready: what both decode paths start from.
struct OpenedAudio {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    sample_rate: u32,
    /// Channel count from the codec parameters; can be wrong (the packet loop
    /// reports the real one).
    codec_channels: usize,
    /// File size, the denominator of the decode progress fraction (0: unknown).
    total_bytes: u64,
    /// Frames the container announces, when it does (not trustworthy).
    n_frames: Option<u64>,
}

/// Check the extension, open and probe the file, pick the first audio track and
/// build its decoder (after the codec-allocation check). The `checkpoint` runs
/// first, before any file access.
fn open_audio(
    path: &Path,
    checkpoint: Option<&dyn Fn() -> Result<(), DictationError>>,
) -> Result<OpenedAudio, DictationError> {
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

    let file = std::fs::File::open(path).map_err(|e| match e.kind() {
        // Typed so callers (the GUI forgets a moved file) never match the text.
        std::io::ErrorKind::NotFound => DictationError::FileNotFound(e.to_string()),
        _ => DictationError::FileDecodeError(format!("Failed to open file: {e}")),
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

    let format = probed.format;

    // Find the first audio track
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| {
            DictationError::FileDecodeError("No audio track found in file".to_string())
        })?;

    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44_100);
    // channels from codec_params can be wrong (e.g. AAC stereo reporting 1).
    // We'll detect the real channel count from the first decoded frame.
    let codec_channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(0);
    let n_frames = track.codec_params.n_frames;

    info!(
        "Decoding audio: {} Hz, codec_channels={}, codec {:?}",
        sample_rate, codec_channels, track.codec_params.codec
    );

    let decoder = make_decoder(&track.codec_params, &DEFAULT_DECODE_BUDGET)?;

    Ok(OpenedAudio {
        format,
        decoder,
        track_id,
        sample_rate,
        codec_channels,
        total_bytes,
        n_frames,
    })
}

fn decode_audio_file_inner(
    path: &Path,
    checkpoint: Option<&dyn Fn() -> Result<(), DictationError>>,
    on_decode: Option<&dyn Fn(u8)>,
    on_resample: Option<&dyn Fn(u8)>,
) -> Result<Vec<f32>, DictationError> {
    let OpenedAudio {
        mut format,
        mut decoder,
        track_id,
        sample_rate,
        codec_channels,
        total_bytes,
        ..
    } = open_audio(path, checkpoint)?;

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
            if let Some(decode) = on_decode {
                decode(100);
            }
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
    // Defence in depth: the allocated mono capacity plus its converted output
    // must still fit the real-peak ceiling before conversion starts.
    if conversion_peak_bytes(mono.capacity() as u64, sample_rate, &DEFAULT_DECODE_BUDGET)?
        > DEFAULT_DECODE_BUDGET.max_total_bytes
    {
        return Err(budget_error("conversion peak"));
    }
    // Remaining budget for the converted output: the whole budget minus the
    // allocated mono buffer and the conversion scratch allowance.
    let remaining_bytes = DEFAULT_DECODE_BUDGET
        .max_total_bytes
        .saturating_sub(mono.capacity() as u64 * SAMPLE_BYTES)
        .saturating_sub(DEFAULT_DECODE_BUDGET.conversion_scratch_bytes);
    let max_output_samples = usize::try_from(remaining_bytes / SAMPLE_BYTES).unwrap_or(usize::MAX);
    let resampled = resample_to_16khz_bounded(
        mono,
        sample_rate,
        resample_cb.as_ref().map(|f| f as &dyn Fn(f64)),
        &|| checkpoint.map_or(Ok(()), |check| check().map_err(|error| error.to_string())),
        max_output_samples,
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

/// What a streaming decode knows as soon as the file is open.
#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub source_rate: u32,
    /// Channel count from the codec parameters (can be wrong; the real count
    /// is in [`StreamedDecode::channels`]).
    pub codec_channels: usize,
    /// Output length (16 kHz samples) implied by the container's frame count,
    /// if it announces one. An estimate for progress: containers can lie, and
    /// the resampler adds a short tail.
    pub expected_samples: Option<usize>,
    /// File size in bytes (0: unknown).
    pub total_bytes: u64,
    /// Audio-packet bytes consumed so far, kept up to date while decoding (only
    /// when `total_bytes` is known). With `total_bytes` it is a fraction of the
    /// file to estimate progress from when `expected_samples` is `None`.
    pub consumed_bytes: Arc<AtomicU64>,
}

/// The outcome of a streaming decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamedDecode {
    /// Total 16 kHz mono samples handed to the sink.
    pub samples: usize,
    pub source_rate: u32,
    /// Channel count of the decoded audio (the real one, not the codec's claim).
    pub channels: usize,
    /// As in [`StreamInfo::expected_samples`].
    pub expected_samples: Option<usize>,
}

/// 16 kHz samples that `n_frames` source frames at `sample_rate` come to.
fn expected_output_samples(n_frames: Option<u64>, sample_rate: u32) -> Option<usize> {
    let frames = n_frames.filter(|&n| n > 0)?;
    if sample_rate == 0 {
        return None;
    }
    let samples =
        (u128::from(frames) * u128::from(TARGET_SAMPLE_RATE)).div_ceil(u128::from(sample_rate));
    usize::try_from(samples).ok()
}

/// What the resampler's own padding and flush may add to the output beyond the
/// nominal length of the audio. The batch decode caps the *source* duration and
/// lets the resampler's output exceed the nominal count within its memory
/// budget (it reserves three maximal output chunks plus 1024 for it). The
/// streaming decode caps the source the same way, and checks its output against
/// the limit plus this much: three maximal output chunks (twice the chunk times
/// the highest ratio it accepts, plus 64, each) and one more chunk. So audio
/// exactly at the limit decodes in both paths, and the output is never trimmed.
const STREAM_OUTPUT_HEADROOM: usize = 3
    * (2 * STREAM_CHUNK * (TARGET_SAMPLE_RATE / MIN_STREAM_SOURCE_RATE) as usize + 64)
    + STREAM_CHUNK;

/// Add `added` streamed 16 kHz samples to the running total, enforcing the cap
/// on the *output* count: `limit` (in 16 kHz samples) plus the resampler's
/// bounded overhead, [`STREAM_OUTPUT_HEADROOM`]. The source-duration check at
/// exactly `limit` is what decides whether a file is too long; this one is the
/// backstop on what the resampler can produce.
fn count_streamed_output(
    total: &mut usize,
    added: usize,
    limit: usize,
) -> Result<(), DictationError> {
    *total = total
        .checked_add(added)
        .ok_or_else(|| budget_error("size overflow"))?;
    check_decode_size_cap_within(
        limit,
        total.saturating_sub(STREAM_OUTPUT_HEADROOM),
        1,
        TARGET_SAMPLE_RATE,
    )
}

/// Decode an audio or video file to 16 kHz mono like [`decode_audio_file`], but
/// hand the samples to `sink` as they are produced instead of returning them,
/// so a consumer can start on the first seconds while the rest decodes.
///
/// The concatenation of what `sink` receives, in order, is bit-identical to
/// `decode_audio_file(path)`. Each packet is downmixed and resampled as it
/// arrives (see [`StreamingResampler`]); neither the source-rate mono nor the
/// full output is accumulated, so memory stays flat however long the file is.
/// `sink` is called with non-empty slices, at the pace of the decoder; if it
/// returns an error, decoding stops with that error.
///
/// All safeguards of the batch decode apply: the extension check, the codec
/// allocation and per-packet budget checks, the consecutive-error limits and
/// the ~4 hour cap (on source frames before each packet is resampled, as the
/// batch decode checks them, and on the streamed output). Source rates under
/// 1 kHz are refused, which the batch decode does not do. Empty audio fails
/// with the same "No audio samples decoded" error. `checkpoint` is called at the same
/// bounded points as in [`decode_audio_file_with_control`]; it is `Sync` so the
/// caller can run the decode on a worker thread while another observes
/// cancellation. `on_open` is called once, after the file is probed and before
/// the first packet is decoded.
pub fn decode_audio_file_streaming(
    path: &Path,
    checkpoint: &(dyn Fn() -> Result<(), DictationError> + Sync),
    on_open: &mut dyn FnMut(&StreamInfo),
    sink: &mut dyn FnMut(&[f32]) -> Result<(), DictationError>,
) -> Result<StreamedDecode, DictationError> {
    let checkpoint: &dyn Fn() -> Result<(), DictationError> = checkpoint;
    let OpenedAudio {
        mut format,
        mut decoder,
        track_id,
        sample_rate,
        codec_channels,
        total_bytes,
        n_frames,
    } = open_audio(path, Some(checkpoint))?;
    let expected_samples = expected_output_samples(n_frames, sample_rate);
    let consumed = Arc::new(AtomicU64::new(0));
    on_open(&StreamInfo {
        source_rate: sample_rate,
        codec_channels,
        expected_samples,
        total_bytes,
        consumed_bytes: consumed.clone(),
    });

    let mut next_packet = || format.next_packet();
    let run = stream_packets(
        &mut next_packet,
        decoder.as_mut(),
        track_id,
        sample_rate,
        codec_channels,
        total_bytes,
        checkpoint,
        &consumed,
        &DEFAULT_DECODE_BUDGET,
        sink,
    )?;
    Ok(StreamedDecode {
        samples: run.streamed,
        source_rate: sample_rate,
        channels: run.channels,
        expected_samples,
    })
}

/// Source rates below this are refused by the streaming decode. Nothing real is
/// recorded at under 1 kHz, and upsampling from such a rate multiplies the
/// audio by more than 16, so a single resampler chunk (1024 source samples)
/// would be tens of thousands of output samples or more: the sinc resampler's
/// buffers scale with the ratio. A defence against crafted headers; the batch
/// decode, which bounds its output up front instead, is unchanged.
const MIN_STREAM_SOURCE_RATE: u32 = 1_000;

/// What [`stream_packets`] saw.
#[derive(Debug, Clone, Copy)]
struct StreamRun {
    /// Channel count of the last decoded packet.
    channels: usize,
    /// 16 kHz samples handed to the sink.
    streamed: usize,
}

fn resample_failure(error: StreamResampleError<DictationError>) -> DictationError {
    match error {
        StreamResampleError::Resample(e) => {
            DictationError::TranscriptionFailed(format!("Resample failed: {e}"))
        }
        StreamResampleError::Output(e) => e,
    }
}

/// The streaming counterpart of [`decode_packets`], separated from container
/// probing so tests can drive it with a scripted packet source and decoder.
///
/// Memory stays bounded whatever a packet claims: before a packet is
/// resampled, its frames are added to the running source count and checked
/// against the ~4 hour cap (the check the batch decode makes as it accumulates
/// mono, on the same numbers), and the resampler hands its output to the sink
/// one chunk at a time, each counted against the cap as it comes.
#[allow(clippy::too_many_arguments)]
fn stream_packets(
    next_packet: &mut dyn FnMut() -> symphonia::core::errors::Result<Packet>,
    decoder: &mut dyn Decoder,
    track_id: u32,
    sample_rate: u32,
    codec_channels: usize,
    total_bytes: u64,
    checkpoint: &dyn Fn() -> Result<(), DictationError>,
    consumed: &AtomicU64,
    budget: &DecodeBudget,
    sink: &mut dyn FnMut(&[f32]) -> Result<(), DictationError>,
) -> Result<StreamRun, DictationError> {
    let mut resampler = StreamingResampler::new(sample_rate);
    let mut mono: Vec<f32> = Vec::new();
    let mut source_frames = 0usize;
    let mut streamed = 0usize;
    let mut emit = |chunk: &[f32]| -> Result<(), DictationError> {
        count_streamed_output(&mut streamed, chunk.len(), budget.max_duration_samples)?;
        sink(chunk)
    };

    let channels = run_packet_loop(
        next_packet,
        decoder,
        track_id,
        codec_channels,
        total_bytes,
        Some(checkpoint),
        None,
        Some(consumed),
        budget,
        &mut |audio| {
            // Same downmix as the batch decode, one packet at a time.
            mono.clear();
            downmix_extend(&audio, &mut mono);
            if mono.is_empty() {
                return Ok(());
            }
            // The duration cap, on source frames and before any resampling: a
            // packet can claim millions of frames, and at a low rate each is
            // many output samples.
            source_frames = source_frames
                .checked_add(mono.len())
                .ok_or_else(|| budget_error("size overflow"))?;
            check_decode_size_cap_within(
                budget.max_duration_samples,
                source_frames,
                1,
                sample_rate,
            )?;
            if sample_rate < MIN_STREAM_SOURCE_RATE {
                return Err(DictationError::FileDecodeError(format!(
                    "Audio sample rate {sample_rate} Hz is too low to decode \
                     (the minimum is {MIN_STREAM_SOURCE_RATE} Hz)"
                )));
            }
            resampler
                .push_with(&mono, &mut emit)
                .map_err(resample_failure)
        },
    )?;

    if source_frames == 0 {
        return Err(DictationError::FileDecodeError(
            "No audio samples decoded from file".to_string(),
        ));
    }
    checkpoint()?;
    resampler.finish_with(&mut emit).map_err(resample_failure)?;
    checkpoint()?;

    info!(
        "Streamed {} source frames ({:.1}s), {} ch at {} Hz, as {} samples at 16kHz",
        source_frames,
        source_frames as f64 / f64::from(sample_rate.max(1)),
        channels,
        sample_rate,
        streamed
    );
    Ok(StreamRun { channels, streamed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sagascript_testsupport::TempDir;
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
                assert!(
                    msg.contains(".xyz"),
                    "error should mention extension: {msg}"
                );
                assert!(
                    msg.contains("wav"),
                    "error should list supported formats: {msg}"
                );
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
            DictationError::FileNotFound(msg) => {
                assert!(
                    !msg.contains("definitely_does_not_exist"),
                    "the OS text must not name the path: {msg}"
                );
            }
            other => panic!("expected FileNotFound, got: {:?}", other),
        }
    }

    /// Only a missing file is `FileNotFound`; any other reason it cannot be opened
    /// stays a decode error (a path below a regular file is NotADirectory on Unix).
    #[cfg(unix)]
    #[test]
    fn a_file_that_cannot_be_opened_for_another_reason_is_a_decode_error() {
        let dir = TempDir::new();
        let file = dir.join("plain.wav");
        std::fs::write(&file, b"x").expect("write fixture");
        let below_a_file = file.join("inner.wav");
        match decode_audio_file(&below_a_file).unwrap_err() {
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
        let dir = TempDir::new();
        let path = dir.join("controlled-decode.wav");
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
        assert!(
            matches!(error, DictationError::FileDecodeError(message) if message.contains("64") || message.contains("too many"))
        );

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
        let dir = TempDir::new();
        let tmp = dir.join("decode.wav");
        std::fs::write(&tmp, &wav_bytes).unwrap();

        let result = decode_audio_file(&tmp);

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
        // Should be FileNotFound (the file is missing), not UnsupportedFormat
        match result.unwrap_err() {
            DictationError::FileNotFound(_) => {} // expected
            DictationError::UnsupportedFormat(msg) => {
                panic!(
                    "uppercase .WAV should be treated as supported, got UnsupportedFormat: {msg}"
                )
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
        assert!(
            result.is_ok(),
            "3h stereo 48kHz should not be rejected: {:?}",
            result
        );
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
        let dir = TempDir::new();
        let path = dir.join("stages.wav");
        let mut wav = crate::audio::wav::encode_wav(&vec![0.1; 88_200]);
        wav[24..28].copy_from_slice(&44_100u32.to_le_bytes());
        wav[28..32].copy_from_slice(&88_200u32.to_le_bytes());
        std::fs::write(&path, wav).unwrap();
        let events = RefCell::new(Vec::new());
        let result = decode_audio_file_with_stage_progress(
            &path,
            None,
            &|pct| events.borrow_mut().push(("decoding", pct)),
            &|pct| events.borrow_mut().push(("resampling", pct)),
        );
        assert!(!result.unwrap().is_empty());
        let events = events.borrow();
        let boundary = events.iter().position(|e| *e == ("resampling", 0)).unwrap();
        assert_eq!(events[boundary - 1], ("decoding", 100));
        assert!(events[..boundary].contains(&("decoding", 99)));
        let conversion = &events[boundary..];
        assert!(
            conversion.len() > 20,
            "must report during conversion, not just completion"
        );
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
            Self {
                params: CodecParameters::new(),
                fail,
                buf,
            }
        }
    }

    impl Decoder for ScriptedDecoder {
        fn try_new(
            _: &CodecParameters,
            _: &DecoderOptions,
        ) -> symphonia::core::errors::Result<Self> {
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
        let result = decode_packets(
            &mut next,
            &mut decoder,
            0,
            16_000,
            1,
            0,
            checkpoint,
            None,
            &DEFAULT_DECODE_BUDGET,
        );
        (result, pulled.get())
    }

    fn assert_bounded_failure(
        outcome: (Result<(Vec<f32>, usize), DictationError>, usize),
        what: &str,
    ) {
        let (result, pulled) = outcome;
        match result {
            Err(DictationError::FileDecodeError(message)) => {
                assert!(
                    message.contains("too many consecutive"),
                    "{what}: {message}"
                );
            }
            other => panic!("{what}: expected bounded failure, got {other:?}"),
        }
        assert_eq!(
            pulled, MAX_CONSECUTIVE_DECODE_ERRORS,
            "{what}: pulls before abort"
        );
    }

    #[test]
    fn callback_free_decode_stops_on_endless_format_errors() {
        assert_bounded_failure(
            run_error_script(true, 100_000, None),
            "callback-free format",
        );
    }

    #[test]
    fn callback_free_decode_stops_on_endless_codec_errors() {
        assert_bounded_failure(
            run_error_script(false, 100_000, None),
            "callback-free codec",
        );
    }

    #[test]
    fn controlled_decode_stops_on_endless_errors() {
        let checkpoint = || Ok(());
        assert_bounded_failure(
            run_error_script(true, 100_000, Some(&checkpoint)),
            "controlled format",
        );
        assert_bounded_failure(
            run_error_script(false, 100_000, Some(&checkpoint)),
            "controlled codec",
        );
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
        let result = decode_packets(
            &mut next,
            &mut decoder,
            0,
            16_000,
            1,
            1000,
            None,
            Some(&on_decode),
            &DEFAULT_DECODE_BUDGET,
        );
        assert!(result.is_err());
        assert_eq!(pulled.get(), MAX_CONSECUTIVE_DECODE_ERRORS);
    }

    #[test]
    fn invalid_media_fails_promptly_through_both_entry_points() {
        let dir = TempDir::new();
        let path = dir.join("invalid.mp3");
        let garbage: Vec<u8> = [0xFFu8, 0xFB, 0x90, 0x00]
            .into_iter()
            .cycle()
            .take(64 * 1024)
            .collect();
        std::fs::write(&path, garbage).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_path = path.clone();
        std::thread::spawn(move || {
            let plain = decode_audio_file(&worker_path);
            let controlled = decode_audio_file_with_control(&worker_path, &|| Ok(()));
            let _ = tx.send((plain, controlled));
        });
        let outcome = rx.recv_timeout(std::time::Duration::from_secs(10));
        let (plain, controlled) = outcome.expect("decode of invalid media must not hang");
        for result in [plain, controlled] {
            match result {
                Err(DictationError::FileDecodeError(message)) => assert!(
                    message.contains("No audio samples decoded")
                        || message.contains("too many consecutive"),
                    "unexpected failure: {message}"
                ),
                other => panic!("expected bounded decode failure, got {other:?}"),
            }
        }
    }

    #[test]
    fn retry_budget_resets_on_successful_decode_through_the_loop() {
        // 63 errors, a good packet, 63 errors, a good packet, EOF: never 64
        // consecutive, so the loop must finish with the two decoded packets.
        let n = std::cell::Cell::new(0usize);
        let mut next = || {
            let i = n.get();
            n.set(i + 1);
            match i {
                0..=62 | 64..=126 => Err(SymError::DecodeError("scripted format failure")),
                63 | 127 => Ok(Packet::new_from_slice(0, 0, 0, &[0u8; 4])),
                _ => Err(SymError::IoError(std::io::Error::from(
                    std::io::ErrorKind::UnexpectedEof,
                ))),
            }
        };
        let mut decoder = ScriptedDecoder::new(false, 16_000, Channels::FRONT_LEFT, 16);
        let (mono, _) = decode_packets(
            &mut next,
            &mut decoder,
            0,
            16_000,
            1,
            0,
            None,
            None,
            &DEFAULT_DECODE_BUDGET,
        )
        .expect("fewer than 64 consecutive errors must not abort");
        assert_eq!(mono.len(), 32);
        // 64 consecutive errors across a good packet boundary do abort.
        let n = std::cell::Cell::new(0usize);
        let mut next = || {
            let i = n.get();
            n.set(i + 1);
            if i < 64 {
                Err(SymError::DecodeError("scripted format failure"))
            } else {
                Err(SymError::IoError(std::io::Error::from(
                    std::io::ErrorKind::UnexpectedEof,
                )))
            }
        };
        assert!(decode_packets(
            &mut next,
            &mut decoder,
            0,
            16_000,
            1,
            0,
            None,
            None,
            &DEFAULT_DECODE_BUDGET
        )
        .is_err());
    }

    // -- per-thread largest-allocation tracking (no large allocation happens) --

    struct TrackingAlloc;
    thread_local! {
        static LARGEST_ALLOC: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }
    // SAFETY: every method forwards to `System` with the arguments it was given,
    // so the `GlobalAlloc` contract holds exactly as for `System`. The only
    // addition is a thread-local counter update (const-initialised `Cell`,
    // read through `try_with`), which neither allocates nor panics.
    unsafe impl std::alloc::GlobalAlloc for TrackingAlloc {
        unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
            let _ = LARGEST_ALLOC.try_with(|c| c.set(c.get().max(l.size())));
            unsafe { std::alloc::System.alloc(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
            unsafe { std::alloc::System.dealloc(p, l) }
        }
        unsafe fn alloc_zeroed(&self, l: std::alloc::Layout) -> *mut u8 {
            let _ = LARGEST_ALLOC.try_with(|c| c.set(c.get().max(l.size())));
            unsafe { std::alloc::System.alloc_zeroed(l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: std::alloc::Layout, n: usize) -> *mut u8 {
            let _ = LARGEST_ALLOC.try_with(|c| c.set(c.get().max(n)));
            unsafe { std::alloc::System.realloc(p, l, n) }
        }
    }
    #[global_allocator]
    static ALLOC: TrackingAlloc = TrackingAlloc;

    fn largest_alloc_during<T>(f: impl FnOnce() -> T) -> (T, usize) {
        LARGEST_ALLOC.with(|c| c.set(0));
        let out = f();
        (out, LARGEST_ALLOC.with(|c| c.get()))
    }

    /// 24-byte ALAC magic cookie declaring `frame_length` frames.
    fn alac_params(frame_length: u32, channels: u8) -> CodecParameters {
        let mut cookie = Vec::new();
        cookie.extend_from_slice(&frame_length.to_be_bytes());
        cookie.extend_from_slice(&[0, 16, 40, 10, 14, channels]);
        cookie.extend_from_slice(&255u16.to_be_bytes());
        cookie.extend_from_slice(&0u32.to_be_bytes());
        cookie.extend_from_slice(&0u32.to_be_bytes());
        cookie.extend_from_slice(&44_100u32.to_be_bytes());
        assert_eq!(cookie.len(), 24);
        let mut params = CodecParameters::new();
        params
            .for_codec(CODEC_TYPE_ALAC)
            .with_extra_data(cookie.into_boxed_slice());
        params
    }

    #[test]
    fn oversized_alac_cookie_is_rejected_before_decoder_construction() {
        // 2^30 stereo frames would make symphonia request >= 8 GiB.
        let (result, largest) = largest_alloc_during(|| {
            make_decoder(&alac_params(1 << 30, 2), &DEFAULT_DECODE_BUDGET).map(|_| ())
        });
        assert!(
            matches!(result, Err(DictationError::FileDecodeError(m)) if m.contains("too much PCM"))
        );
        assert!(
            largest < 1 << 20,
            "no large allocation may be requested, saw {largest}"
        );
        // A normal cookie still constructs.
        assert!(make_decoder(&alac_params(4096, 2), &DEFAULT_DECODE_BUDGET).is_ok());
    }

    #[test]
    fn oversized_pcm_shape_is_rejected_before_decoder_construction() {
        let mut params = CodecParameters::new();
        params
            .for_codec(symphonia::core::codecs::CODEC_TYPE_PCM_S16LE)
            .with_sample_rate(48_000)
            .with_channels(Channels::FRONT_LEFT | Channels::FRONT_RIGHT)
            .with_max_frames_per_packet(1 << 31);
        let (result, largest) =
            largest_alloc_during(|| make_decoder(&params, &DEFAULT_DECODE_BUDGET).map(|_| ()));
        assert!(result.is_err());
        assert!(largest < 1 << 20, "saw {largest}");
    }

    // -- decoded-byte budget (u64 arithmetic, no large allocations) --

    #[test]
    fn pcm_size_arithmetic_is_checked_for_extreme_shapes() {
        assert_eq!(pcm_bytes(1_000, 2).unwrap(), 8_000);
        let frames = 4u64 * 3600 * 192_000;
        assert_eq!(pcm_bytes(frames, 32).unwrap(), frames * 32 * 4);
        assert!(pcm_bytes(u64::MAX, 2).is_err());
        assert!(pcm_bytes(u64::MAX / 4 + 1, 1).is_err());
        assert!(check_packet_budget(u64::MAX, 32, &DEFAULT_DECODE_BUDGET).is_err());
        assert!(conversion_peak_bytes(u64::MAX, 8_000, &DEFAULT_DECODE_BUDGET).is_err());
        assert!(conversion_peak_bytes(u64::MAX, 0, &DEFAULT_DECODE_BUDGET).is_err());
    }

    #[test]
    fn real_peak_never_exceeds_the_budget_at_the_frame_ceiling() {
        let b = DEFAULT_DECODE_BUDGET;
        for rate in [
            1u32, 8_000, 16_000, 22_050, 44_100, 48_000, 96_000, 192_000, 1_000_000,
        ] {
            let n = max_mono_frames(rate, &b);
            assert!(
                conversion_peak_bytes(n, rate, &b).unwrap() <= b.max_total_bytes,
                "conversion {rate}"
            );
            // reallocation overlap: old (<= n/2... conservatively 1.5x) + scratch
            assert!(n * 4 * 3 / 2 <= b.max_total_bytes, "realloc overlap {rate}");
        }
    }

    #[test]
    fn budget_admits_long_ordinary_recordings_and_rejects_extreme_rates() {
        let b = DEFAULT_DECODE_BUDGET;
        for rate in [8_000u32, 16_000, 44_100, 48_000] {
            let four_hours = 4 * 3600 * rate as u64;
            assert!(
                four_hours <= max_mono_frames(rate, &b),
                "4h at {rate} Hz must fit"
            );
        }
        assert!(3600 * 192_000u64 <= max_mono_frames(192_000, &b));
        assert!(4 * 3600 * 96_000u64 > max_mono_frames(96_000, &b));
        assert!(4 * 3600 * 192_000u64 > max_mono_frames(192_000, &b));
    }

    #[test]
    fn packet_budget_counts_interleaved_and_mono_buffers() {
        let budget = DecodeBudget {
            max_packet_bytes: 12_000,
            ..DEFAULT_DECODE_BUDGET
        };
        assert!(check_packet_budget(1_000, 2, &budget).is_ok());
        assert!(check_packet_budget(1_001, 2, &budget).is_err());
        assert!(check_packet_budget(1_000, 32, &budget).is_err());
    }

    #[test]
    fn budget_rejection_happens_before_buffer_growth() {
        let budget = DecodeBudget {
            max_total_bytes: 4_000,
            max_packet_bytes: 0,
            conversion_scratch_bytes: 0,
            max_duration_samples: MAX_DECODE_SAMPLES,
        };
        // 16 kHz: 8 bytes per frame (mono + output) -> ceiling 500 frames.
        assert_eq!(max_mono_frames(16_000, &budget), 500);
        let mut mono: Vec<f32> = Vec::with_capacity(10);
        mono.extend_from_slice(&[0.5; 10]);
        let capacity = mono.capacity();
        let filled = std::cell::Cell::new(false);
        let (result, largest) = largest_alloc_during(|| {
            append_mono_within_budget(&mut mono, 600, 16_000, &budget, 0, |_| filled.set(true))
        });
        assert!(matches!(result, Err(DictationError::FileDecodeError(_))));
        assert!(!filled.get());
        assert_eq!((mono.len(), mono.capacity()), (10, capacity));
        assert!(
            largest < 1024,
            "only the error message may allocate, saw {largest}"
        );
        // Fill to the ceiling: capacity never exceeds it, so peak stays bounded.
        for _ in 0..1000 {
            if append_mono_within_budget(&mut mono, 50, 16_000, &budget, 0, |m| {
                m.extend_from_slice(&[0.0; 50])
            })
            .is_err()
            {
                break;
            }
            assert!(mono.capacity() as u64 <= max_mono_frames(16_000, &budget));
            assert!(
                conversion_peak_bytes(mono.capacity() as u64, 16_000, &budget).unwrap()
                    <= budget.max_total_bytes
            );
        }
        assert!(mono.len() <= 500);
    }

    #[test]
    fn reallocation_overlap_counts_old_plus_new_capacity() {
        // 48 kHz: 6 bytes/frame -> ceiling 1000 frames for a 6_000 byte budget.
        let budget = DecodeBudget {
            max_total_bytes: 6_000,
            max_packet_bytes: 0,
            conversion_scratch_bytes: 0,
            max_duration_samples: MAX_DECODE_SAMPLES,
        };
        assert_eq!(max_mono_frames(48_000, &budget), 1_000);
        // Previous capacity (900) is greater than half the clamped target.
        let mut mono: Vec<f32> = Vec::with_capacity(900);
        mono.resize(900, 0.0);
        let old_cap = mono.capacity() as u64;
        let r = append_mono_within_budget(&mut mono, 50, 48_000, &budget, 0, |m| {
            m.extend_from_slice(&[0.0; 50])
        });
        let new_cap = mono.capacity() as u64;
        if new_cap != old_cap {
            assert!(r.is_ok());
            assert!(
                (old_cap + new_cap) * 4 <= budget.max_total_bytes,
                "old {old_cap} + new {new_cap} frames overlap the budget"
            );
        } else {
            assert!(r.is_err(), "must reject when growth cannot fit");
            assert_eq!(mono.len(), 900);
        }
    }

    #[test]
    fn growth_steps_never_overlap_past_the_budget_and_reach_the_ceiling() {
        let budget = DecodeBudget {
            max_total_bytes: 60_000,
            max_packet_bytes: 0,
            conversion_scratch_bytes: 0,
            max_duration_samples: MAX_DECODE_SAMPLES,
        };
        let ceiling = max_mono_frames(48_000, &budget);
        let mut mono: Vec<f32> = Vec::new();
        let mut last = 0u64;
        loop {
            let old = mono.capacity() as u64;
            let r = append_mono_within_budget(&mut mono, 7, 48_000, &budget, 0, |m| {
                m.extend_from_slice(&[0.0; 7])
            });
            let new = mono.capacity() as u64;
            if new != old && old > 0 {
                assert!(
                    (old + new) * 4 <= budget.max_total_bytes,
                    "old {old} + new {new}"
                );
            }
            if r.is_err() {
                break;
            }
            last = mono.len() as u64;
            assert!(new <= ceiling);
        }
        assert!(
            last + 7 > ceiling,
            "must be able to fill up to the ceiling, got {last} of {ceiling}"
        );
    }

    #[test]
    fn decode_loop_aborts_on_oversized_packet_before_any_sample_buffer() {
        let budget = DecodeBudget {
            max_packet_bytes: 4_096,
            ..DEFAULT_DECODE_BUDGET
        };
        let pulled = std::cell::Cell::new(0usize);
        let mut next = || {
            pulled.set(pulled.get() + 1);
            Ok(Packet::new_from_slice(0, 0, 0, &[0u8; 4]))
        };
        let mut decoder = ScriptedDecoder::new(
            false,
            48_000,
            Channels::FRONT_LEFT | Channels::FRONT_RIGHT,
            1_000,
        );
        let (result, largest) = largest_alloc_during(|| {
            decode_packets(
                &mut next,
                &mut decoder,
                0,
                48_000,
                2,
                0,
                None,
                None,
                &budget,
            )
        });
        assert!(matches!(result, Err(DictationError::FileDecodeError(_))));
        assert_eq!(pulled.get(), 1);
        // SampleBuffer for 1000 stereo frames would be 8_000 bytes.
        assert!(
            largest < 8_000,
            "SampleBuffer must not be allocated, saw {largest}"
        );
    }

    #[test]
    fn decode_loop_downmixes_streaming_values_and_stops_at_total_budget() {
        // Stereo packets with L=0.5, R=-0.25 -> mono 0.125.
        let mut decoder = ScriptedDecoder::new(
            false,
            16_000,
            Channels::FRONT_LEFT | Channels::FRONT_RIGHT,
            100,
        );
        for i in 0..100 {
            decoder.buf.chan_mut(0)[i] = 0.5;
            decoder.buf.chan_mut(1)[i] = -0.25;
        }
        let budget = DecodeBudget {
            max_total_bytes: 5_600,
            max_packet_bytes: 1_200,
            conversion_scratch_bytes: 0,
            max_duration_samples: MAX_DECODE_SAMPLES,
        };
        // ceiling = (5_600 - 350 packet margin) / 8 = 656 frames at 16 kHz.
        let pulled = std::cell::Cell::new(0usize);
        let mut next = || {
            pulled.set(pulled.get() + 1);
            Ok(Packet::new_from_slice(0, 0, 0, &[0u8; 4]))
        };
        let result = decode_packets(
            &mut next,
            &mut decoder,
            0,
            16_000,
            2,
            0,
            None,
            None,
            &budget,
        );
        assert!(matches!(result, Err(DictationError::FileDecodeError(_))));
        assert_eq!(pulled.get(), 7); // 6 packets fit, the 7th is rejected
        let mut next = {
            let n = std::cell::Cell::new(0usize);
            move || {
                n.set(n.get() + 1);
                if n.get() > 2 {
                    Err(SymError::IoError(std::io::Error::from(
                        std::io::ErrorKind::UnexpectedEof,
                    )))
                } else {
                    Ok(Packet::new_from_slice(0, 0, 0, &[0u8; 4]))
                }
            }
        };
        let (mono, ch) = decode_packets(
            &mut next,
            &mut decoder,
            0,
            16_000,
            2,
            0,
            None,
            None,
            &budget,
        )
        .unwrap();
        assert_eq!((mono.len(), ch), (200, 2));
        assert!(mono.iter().all(|&v| v == 0.125));
    }

    // -- bitwise compatibility with the previous accumulate-then-convert pipeline --

    fn pcm_wav(rate: u32, ch: u16, bits: u16, float: bool, frames: usize) -> Vec<u8> {
        let bps = (bits / 8) as usize;
        let mut data = Vec::new();
        let mut x: u32 = 99;
        for i in 0..frames {
            for c in 0..ch as usize {
                x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                let v = ((i as f32 * 0.03 * (c as f32 + 1.0)).sin() * 0.6)
                    + ((x >> 24) as f32 - 128.0) / 1024.0;
                match (bits, float) {
                    (8, _) => data.push(((v * 127.0) as i32 + 128) as u8),
                    (16, _) => data.extend_from_slice(&((v * 32767.0) as i16).to_le_bytes()),
                    (24, _) => {
                        data.extend_from_slice(&((v * 8_388_607.0) as i32).to_le_bytes()[..3])
                    }
                    (32, true) => data.extend_from_slice(&v.to_le_bytes()),
                    _ => data.extend_from_slice(&((v * 2_147_483_647.0) as i32).to_le_bytes()),
                }
            }
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&(if float { 3u16 } else { 1 }).to_le_bytes());
        out.extend_from_slice(&ch.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * ch as u32 * bps as u32).to_le_bytes());
        out.extend_from_slice(&(ch * bps as u16).to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        out
    }

    /// The previous implementation, verbatim in structure: accumulate every
    /// interleaved sample, then `mix_to_mono`, then resample.
    fn reference_decode(path: &Path) -> Vec<f32> {
        let file = std::fs::File::open(path).unwrap();
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("wav");
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .unwrap();
        let mut format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
            .unwrap();
        let track_id = track.id;
        let rate = track.codec_params.sample_rate.unwrap_or(44_100);
        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .unwrap();
        let mut all: Vec<f32> = Vec::new();
        let mut channels = 1;
        while let Ok(packet) = format.next_packet() {
            if packet.track_id() != track_id {
                continue;
            }
            let decoded = decoder.decode(&packet).unwrap();
            let spec = *decoded.spec();
            channels = spec.channels.count().max(1);
            let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            buf.copy_interleaved_ref(decoded);
            all.extend_from_slice(buf.samples());
        }
        let mono = crate::audio::resample::mix_to_mono(&all, channels);
        crate::audio::resample::resample_to_16khz_with_control(mono, rate, None, &|| Ok(()))
            .unwrap()
    }

    #[test]
    fn streaming_decode_is_bitwise_identical_to_the_previous_pipeline() {
        let dir = TempDir::new();
        let mut checked = 0;
        for (rate, ch, bits, float, frames) in [
            (16_000u32, 1u16, 16u16, false, 5_037usize),
            (44_100, 2, 16, false, 20_011),
            (48_000, 2, 24, false, 30_001),
            (48_000, 1, 32, true, 9_999),
            (44_100, 2, 32, false, 12_345),
            (22_050, 1, 8, false, 4_099),
            (48_000, 3, 16, false, 7_777),
            (8_000, 2, 16, false, 1_025),
            (48_000, 2, 16, false, 700),
        ] {
            let path = dir.join(format!("f_{rate}_{ch}_{bits}_{float}_{frames}.wav"));
            std::fs::write(&path, pcm_wav(rate, ch, bits, float, frames)).unwrap();
            let want = reference_decode(&path);
            let got = decode_audio_file(&path).unwrap();
            let got_controlled = decode_audio_file_with_control(&path, &|| Ok(())).unwrap();
            let bits_of = |v: &[f32]| v.iter().map(|f| f.to_bits()).collect::<Vec<_>>();
            assert_eq!(
                bits_of(&got),
                bits_of(&want),
                "{rate} {ch} {bits} {float} {frames}"
            );
            assert_eq!(bits_of(&got_controlled), bits_of(&want));
            assert!(want.iter().any(|v| *v != 0.0));
            checked += 1;
        }
        assert_eq!(checked, 9);
    }

    /// Hashes of the output of main's (08d0a0c) unmodified decode+resample
    /// pipeline (macOS aarch64), frozen so resampler compatibility with main
    /// stays pinned independently of the current resampler.
    #[test]
    fn decode_output_matches_frozen_hashes_from_main() {
        fn fnv(v: &[f32]) -> u64 {
            let mut h = 0xcbf29ce484222325u64;
            for s in v {
                for b in s.to_bits().to_le_bytes() {
                    h ^= b as u64;
                    h = h.wrapping_mul(0x100000001b3);
                }
            }
            h
        }
        let dir = TempDir::new();
        for (rate, ch, bits, float, frames, len, hash) in [
            (
                44_100u32,
                2u16,
                16u16,
                false,
                20_011usize,
                7_383usize,
                0x6578a6d283bd38fa_u64,
            ),
            (48_000, 1, 32, true, 9_999, 3_370, 0x1e226ca4855289c0),
            (8_000, 2, 16, false, 1_025, 3_836, 0xb42a5ee8279acd85),
        ] {
            let path = dir.join(format!("frozen_{rate}.wav"));
            std::fs::write(&path, pcm_wav(rate, ch, bits, float, frames)).unwrap();
            let got = decode_audio_file(&path).unwrap();
            assert_eq!(got.len(), len, "{rate} Hz {ch} ch {bits}-bit");
            // Float results (libm sin in the fixture, sinc resampler) differ
            // bit-for-bit across platforms, so the hashes were frozen on, and
            // are only asserted for, macOS aarch64.
            if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
                assert_eq!(fnv(&got), hash, "{rate} Hz {ch} ch {bits}-bit");
            }
        }
    }

    // -- streaming decode --

    /// What a streaming decode produced: every sink call, plus the result.
    struct Streamed {
        calls: Vec<usize>,
        samples: Vec<f32>,
        info: Option<StreamInfo>,
        result: Result<StreamedDecode, DictationError>,
    }

    fn stream(
        path: &Path,
        checkpoint: &(dyn Fn() -> Result<(), DictationError> + Sync),
    ) -> Streamed {
        let mut calls = Vec::new();
        let mut samples = Vec::new();
        let mut info = None;
        let result = decode_audio_file_streaming(
            path,
            checkpoint,
            &mut |i| info = Some(i.clone()),
            &mut |chunk| {
                assert!(!chunk.is_empty(), "the sink is never called with nothing");
                calls.push(chunk.len());
                samples.extend_from_slice(chunk);
                Ok(())
            },
        );
        Streamed {
            calls,
            samples,
            info,
            result,
        }
    }

    fn bits_of(v: &[f32]) -> Vec<u32> {
        v.iter().map(|f| f.to_bits()).collect()
    }

    fn repo_fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../test-audio")
            .join(name)
    }

    #[test]
    fn streaming_decode_equals_batch_decode_on_the_repo_fixtures() {
        for name in [
            "english-jfk.wav",
            "swedish-fleurs-hongkong.wav",
            "norwegian-short-3s.mp3",
            "norwegian-medium-8s.mp3",
        ] {
            let path = repo_fixture(name);
            let want = decode_audio_file(&path).unwrap();
            let got = stream(&path, &|| Ok(()));
            let done = got.result.unwrap();
            assert_eq!(bits_of(&got.samples), bits_of(&want), "{name}");
            assert_eq!(done.samples, want.len(), "{name}");
            assert!(got.calls.len() > 1, "{name}: delivered in pieces");
            assert!(got.info.is_some(), "{name}");
        }
    }

    #[test]
    fn streaming_decode_equals_batch_decode_for_resampled_and_multichannel_sources() {
        let dir = TempDir::new();
        for (rate, ch, bits, float, frames) in [
            (16_000u32, 1u16, 16u16, false, 5_037usize),
            (44_100, 2, 16, false, 20_011),
            (48_000, 2, 24, false, 30_001),
            (48_000, 1, 32, true, 9_999),
            (22_050, 1, 8, false, 4_099),
            (48_000, 3, 16, false, 7_777),
            (8_000, 2, 16, false, 1_025),
            (8_000, 1, 16, false, 1_024),
            // Fewer than one resampler chunk of source audio.
            (48_000, 2, 16, false, 700),
            (44_100, 1, 16, false, 1),
        ] {
            let path = dir.join(format!("s_{rate}_{ch}_{bits}_{float}_{frames}.wav"));
            std::fs::write(&path, pcm_wav(rate, ch, bits, float, frames)).unwrap();
            let want = decode_audio_file(&path).unwrap();
            let got = stream(&path, &|| Ok(()));
            let done = got.result.unwrap();
            let label = format!("{rate} Hz {ch} ch {bits}-bit {frames} frames");
            assert_eq!(bits_of(&got.samples), bits_of(&want), "{label}");
            assert_eq!(done.samples, want.len(), "{label}");
            assert_eq!((done.source_rate, done.channels), (rate, usize::from(ch)));
            let info = got.info.unwrap();
            assert_eq!(info.source_rate, rate);
            assert_eq!(
                info.expected_samples,
                expected_output_samples(Some(frames as u64), rate)
            );
        }
    }

    #[test]
    fn streaming_decode_announces_the_expected_length_and_tracks_consumed_bytes() {
        let dir = TempDir::new();
        let path = dir.join("expected.wav");
        std::fs::write(&path, pcm_wav(44_100, 2, 16, false, 44_100)).unwrap();
        let total = std::fs::metadata(&path).unwrap().len();
        let mut seen = Vec::new();
        let info = std::cell::RefCell::new(None::<StreamInfo>);
        let done = decode_audio_file_streaming(
            &path,
            &|| Ok(()),
            &mut |i| *info.borrow_mut() = Some(i.clone()),
            &mut |_| {
                let info = info.borrow();
                seen.push(
                    info.as_ref()
                        .unwrap()
                        .consumed_bytes
                        .load(Ordering::Relaxed),
                );
                Ok(())
            },
        )
        .unwrap();
        let info = info.into_inner().unwrap();
        assert_eq!(info.total_bytes, total);
        assert_eq!(info.expected_samples, Some(16_000));
        assert!(
            (done.samples as i64 - 16_000).abs() < 400,
            "close to the announced length: {}",
            done.samples
        );
        assert!(seen.windows(2).all(|w| w[0] <= w[1]), "never goes back");
        assert!(*seen.last().unwrap() > 0 && *seen.last().unwrap() <= total);
    }

    #[test]
    fn streaming_decode_keeps_the_batch_error_behaviour() {
        let dir = TempDir::new();
        // Extension and missing file: the batch errors, before any callback.
        let called = AtomicUsize::new(0);
        let counting = || {
            called.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        let unsupported = stream(&dir.join("x.xyz"), &counting);
        assert!(matches!(
            unsupported.result,
            Err(DictationError::UnsupportedFormat(_))
        ));
        assert!(unsupported.info.is_none());
        let missing = stream(&dir.join("missing.wav"), &counting);
        assert!(matches!(
            missing.result,
            Err(DictationError::FileNotFound(_))
        ));
        assert_eq!(
            called.load(Ordering::SeqCst),
            2,
            "the checkpoint runs first"
        );

        // A WAV without a single frame: the batch error text.
        let empty = dir.join("empty.wav");
        std::fs::write(&empty, pcm_wav(16_000, 1, 16, false, 0)).unwrap();
        let want = decode_audio_file(&empty).unwrap_err().to_string();
        let got = stream(&empty, &|| Ok(())).result.unwrap_err().to_string();
        assert_eq!(got, want);
        assert!(got.contains("No audio samples decoded"), "{got}");
    }

    #[test]
    fn streaming_decode_stops_on_cancel_and_on_a_failing_sink() {
        let dir = TempDir::new();
        let path = dir.join("long.wav");
        std::fs::write(&path, pcm_wav(44_100, 2, 16, false, 200_000)).unwrap();

        // Cancel after a few packets: stops with the checkpoint's error, having
        // delivered only part of the audio.
        let calls = AtomicUsize::new(0);
        let cancel_late = || {
            if calls.fetch_add(1, Ordering::SeqCst) >= 40 {
                Err(DictationError::Cancelled)
            } else {
                Ok(())
            }
        };
        let cancelled = stream(&path, &cancel_late);
        assert!(matches!(cancelled.result, Err(DictationError::Cancelled)));
        assert!(!cancelled.samples.is_empty());
        let full = decode_audio_file(&path).unwrap();
        assert!(cancelled.samples.len() < full.len() / 2);

        // A sink error ends the decode with that error and no further calls.
        let mut delivered = 0;
        let error = decode_audio_file_streaming(&path, &|| Ok(()), &mut |_| {}, &mut |_| {
            delivered += 1;
            Err(DictationError::FileDecodeError("sink full".into()))
        })
        .unwrap_err();
        assert!(matches!(&error, DictationError::FileDecodeError(m) if m == "sink full"));
        assert_eq!(delivered, 1);
    }

    #[test]
    fn streaming_decode_can_run_on_a_worker_thread_while_the_caller_cancels() {
        let dir = TempDir::new();
        let path = dir.join("worker.wav");
        std::fs::write(&path, pcm_wav(48_000, 2, 16, false, 300_000)).unwrap();
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let checkpoint = || {
            if cancelled.load(Ordering::SeqCst) {
                Err(DictationError::Cancelled)
            } else {
                Ok(())
            }
        };
        let outcome = std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                let mut total = 0usize;
                let result =
                    decode_audio_file_streaming(&path, &checkpoint, &mut |_| {}, &mut |chunk| {
                        total += chunk.len();
                        cancelled.store(true, Ordering::SeqCst);
                        Ok(())
                    });
                (result, total)
            });
            worker.join().unwrap()
        });
        assert!(matches!(outcome.0, Err(DictationError::Cancelled)));
        assert!(outcome.1 > 0);
    }

    #[test]
    fn the_duration_cap_applies_to_the_streamed_output_count() {
        // The output may exceed the nominal limit by the resampler's bounded
        // overhead, and no more.
        let limit = MAX_DECODE_SAMPLES;
        let most = limit + STREAM_OUTPUT_HEADROOM;
        let mut total = 0usize;
        count_streamed_output(&mut total, most - 10, limit).unwrap();
        count_streamed_output(&mut total, 10, limit).unwrap();
        assert_eq!(total, most);
        let error = count_streamed_output(&mut total, 1, limit).unwrap_err();
        assert!(
            matches!(&error, DictationError::FileDecodeError(m) if m.contains("4 hour")),
            "{error:?}"
        );
        let mut near_overflow = usize::MAX - 1;
        assert!(count_streamed_output(&mut near_overflow, 5, limit).is_err());
    }

    #[test]
    fn expected_length_comes_from_the_announced_frame_count() {
        assert_eq!(expected_output_samples(Some(48_000), 48_000), Some(16_000));
        assert_eq!(expected_output_samples(Some(44_100), 44_100), Some(16_000));
        assert_eq!(expected_output_samples(Some(8_000), 8_000), Some(16_000));
        assert_eq!(expected_output_samples(Some(1), 48_000), Some(1));
        assert_eq!(expected_output_samples(None, 48_000), None);
        assert_eq!(expected_output_samples(Some(0), 48_000), None);
        assert_eq!(expected_output_samples(Some(10), 0), None);
    }

    // -- streaming decode: bounded before it allocates --

    /// Run the streaming core over `packets` scripted packets of the decoder's
    /// fixed buffer, at source rate `rate`, with a mono source.
    fn run_scripted_stream(
        decoder: &mut ScriptedDecoder,
        rate: u32,
        packets: usize,
        sink: &mut dyn FnMut(&[f32]) -> Result<(), DictationError>,
    ) -> Result<StreamRun, DictationError> {
        run_scripted_stream_within(decoder, rate, packets, &DEFAULT_DECODE_BUDGET, sink)
    }

    fn run_scripted_stream_within(
        decoder: &mut ScriptedDecoder,
        rate: u32,
        packets: usize,
        budget: &DecodeBudget,
        sink: &mut dyn FnMut(&[f32]) -> Result<(), DictationError>,
    ) -> Result<StreamRun, DictationError> {
        let pulled = std::cell::Cell::new(0usize);
        let mut next = || {
            let n = pulled.get();
            pulled.set(n + 1);
            if n >= packets {
                Err(SymError::IoError(std::io::Error::from(
                    std::io::ErrorKind::UnexpectedEof,
                )))
            } else {
                Ok(Packet::new_from_slice(0, 0, 0, &[0u8; 4]))
            }
        };
        stream_packets(
            &mut next,
            decoder,
            0,
            rate,
            1,
            0,
            &|| Ok(()),
            &AtomicU64::new(0),
            budget,
            sink,
        )
    }

    /// The batch path over the same scripted packets: `packets` of the decoder's buffer.
    fn run_scripted_batch(
        decoder: &mut ScriptedDecoder,
        rate: u32,
        packets: usize,
        budget: &DecodeBudget,
    ) -> Result<(Vec<f32>, usize), DictationError> {
        let pulled = std::cell::Cell::new(0usize);
        let mut next = || {
            let n = pulled.get();
            pulled.set(n + 1);
            if n >= packets {
                Err(SymError::IoError(std::io::Error::from(
                    std::io::ErrorKind::UnexpectedEof,
                )))
            } else {
                Ok(Packet::new_from_slice(0, 0, 0, &[0u8; 4]))
            }
        };
        decode_packets(&mut next, decoder, 0, rate, 1, 0, None, None, budget)
    }

    #[test]
    fn audio_at_exactly_the_duration_limit_decodes_in_both_paths_and_one_frame_more_fails_in_both()
    {
        // A small limit stands in for the 4 hours: 2 s of 16 kHz, which is 16_000
        // frames at 8 kHz (a rate the streaming path accepts).
        let limit = 32_000;
        let rate = 8_000;
        let budget = DecodeBudget {
            max_duration_samples: limit,
            ..DEFAULT_DECODE_BUDGET
        };

        // Exactly at the limit: both succeed. The resampler's flush takes the
        // output past the nominal count, which the streaming path must allow.
        let mut batch_decoder = ScriptedDecoder::new(false, rate, Channels::FRONT_LEFT, 16_000);
        let (mono, _) = run_scripted_batch(&mut batch_decoder, rate, 1, &budget)
            .expect("the batch path allows exactly the limit");
        let batch_output = crate::audio::resample::resample_to_16khz(mono, rate).unwrap();
        let mut stream_decoder = ScriptedDecoder::new(false, rate, Channels::FRONT_LEFT, 16_000);
        let mut streamed = Vec::new();
        run_scripted_stream_within(&mut stream_decoder, rate, 1, &budget, &mut |chunk| {
            streamed.extend_from_slice(chunk);
            Ok(())
        })
        .expect("the streaming path allows exactly the limit");
        assert!(
            batch_output.len() > limit,
            "the premise: the resampled audio is longer than the nominal limit ({} vs {limit})",
            batch_output.len()
        );
        assert_eq!(
            bits_of(&streamed),
            bits_of(&batch_output),
            "the output is never trimmed"
        );

        // One frame more: both refuse, with the same error.
        let mut batch_decoder = ScriptedDecoder::new(false, rate, Channels::FRONT_LEFT, 16_001);
        let batch_error = run_scripted_batch(&mut batch_decoder, rate, 1, &budget).unwrap_err();
        let mut stream_decoder = ScriptedDecoder::new(false, rate, Channels::FRONT_LEFT, 16_001);
        let stream_error =
            run_scripted_stream_within(&mut stream_decoder, rate, 1, &budget, &mut |_| Ok(()))
                .unwrap_err();
        for error in [&batch_error, &stream_error] {
            assert!(
                matches!(error, DictationError::FileDecodeError(m) if m.contains("4 hour")),
                "{error:?}"
            );
        }
        assert_eq!(batch_error.to_string(), stream_error.to_string());
    }

    #[test]
    fn the_output_headroom_covers_the_batch_paths_reservation_at_the_lowest_accepted_rate() {
        use rubato::{Resampler, SincFixedIn};
        let ratio = f64::from(TARGET_SAMPLE_RATE) / f64::from(MIN_STREAM_SOURCE_RATE);
        let resampler = SincFixedIn::<f32>::new(
            ratio,
            2.0,
            crate::audio::resample::sinc_params(),
            STREAM_CHUNK,
            1,
        )
        .unwrap();
        // What the batch code sets aside for padding and flush.
        let batch_headroom = resampler.output_frames_max() * 3 + 1024;
        assert!(
            STREAM_OUTPUT_HEADROOM >= batch_headroom,
            "{STREAM_OUTPUT_HEADROOM} < {batch_headroom}"
        );
    }

    #[test]
    fn a_huge_packet_at_a_tiny_rate_is_refused_before_it_is_resampled() {
        // The largest packet the per-packet budget admits (8M mono frames) at
        // 1 Hz is 92 days of audio; resampled it would be 16000 times as many
        // samples (hundreds of GiB). The source-frame cap must refuse it first.
        let frames = 8_000_000;
        assert!(check_packet_budget(frames as u64, 1, &DEFAULT_DECODE_BUDGET).is_ok());
        let mut decoder = ScriptedDecoder::new(false, 1, Channels::FRONT_LEFT, frames);
        let delivered = std::cell::Cell::new(0usize);
        let (result, largest) = largest_alloc_during(|| {
            run_scripted_stream(&mut decoder, 1, 1, &mut |chunk| {
                delivered.set(delivered.get() + chunk.len());
                Ok(())
            })
        });
        match result {
            Err(DictationError::FileDecodeError(message)) => {
                assert!(message.contains("4 hour"), "{message}")
            }
            other => panic!("expected the duration cap, got {other:?}"),
        }
        assert_eq!(delivered.get(), 0, "nothing was resampled or delivered");
        assert!(
            largest < 64 << 20,
            "only the packet's own buffers (about 32 MB) may be allocated, saw {largest}"
        );
    }

    #[test]
    fn implausibly_low_source_rates_are_refused_in_the_streaming_path() {
        for rate in [1u32, 8, 500, MIN_STREAM_SOURCE_RATE - 1] {
            let mut decoder = ScriptedDecoder::new(false, rate, Channels::FRONT_LEFT, 1_000);
            let (result, largest) = largest_alloc_during(|| {
                run_scripted_stream(&mut decoder, rate, 3, &mut |_| Ok(()))
            });
            match result {
                Err(DictationError::FileDecodeError(message)) => {
                    assert!(message.contains("too low"), "{rate} Hz: {message}")
                }
                other => panic!("{rate} Hz: expected a refusal, got {other:?}"),
            }
            assert!(largest < 1 << 20, "{rate} Hz: saw {largest}");
        }
        // The lowest rate accepted decodes.
        let mut decoder =
            ScriptedDecoder::new(false, MIN_STREAM_SOURCE_RATE, Channels::FRONT_LEFT, 2_000);
        let streamed = std::cell::Cell::new(0usize);
        let run = run_scripted_stream(&mut decoder, MIN_STREAM_SOURCE_RATE, 2, &mut |chunk| {
            streamed.set(streamed.get() + chunk.len());
            Ok(())
        })
        .unwrap();
        assert_eq!(run.streamed, streamed.get());
        assert!(run.streamed >= 4_000 * 16 - 2_048, "{}", run.streamed);
    }

    #[test]
    fn the_source_frame_cap_trips_at_the_same_duration_as_the_batch_decode() {
        // At 1 Hz the cap is 14_400 frames (4 hours), small enough to test at the
        // boundary with real buffers.
        let rate = 1;
        let at_cap = 14_400;
        // Batch: one frame more than the cap is refused, the cap itself is not.
        let mut mono: Vec<f32> = Vec::new();
        let budget = DEFAULT_DECODE_BUDGET;
        append_mono_within_budget(&mut mono, at_cap, rate, &budget, 0, |m| {
            m.extend(std::iter::repeat_n(0.0, at_cap))
        })
        .expect("exactly the cap is allowed");
        let over = append_mono_within_budget(&mut mono, 1, rate, &budget, 0, |m| m.push(0.0));
        assert!(matches!(&over, Err(DictationError::FileDecodeError(m)) if m.contains("4 hour")));

        // Streaming: the same boundary. At the cap the source-frame check passes
        // (the low-rate refusal comes next); one frame more is the duration cap.
        let mut at = ScriptedDecoder::new(false, rate, Channels::FRONT_LEFT, at_cap);
        match run_scripted_stream(&mut at, rate, 1, &mut |_| Ok(())) {
            Err(DictationError::FileDecodeError(m)) => assert!(m.contains("too low"), "{m}"),
            other => panic!("expected the rate refusal at the cap, got {other:?}"),
        }
        let mut past = ScriptedDecoder::new(false, rate, Channels::FRONT_LEFT, at_cap + 1);
        match run_scripted_stream(&mut past, rate, 1, &mut |_| Ok(())) {
            Err(DictationError::FileDecodeError(m)) => assert!(m.contains("4 hour"), "{m}"),
            other => panic!("expected the duration cap past it, got {other:?}"),
        }
    }

    /// Output reaches the sink a chunk at a time and counts against the cap as
    /// it comes: a sink-side limit stops the resampling mid-packet.
    #[test]
    fn output_is_delivered_per_resampler_chunk_and_stops_when_the_sink_refuses() {
        let mut decoder = ScriptedDecoder::new(false, 8_000, Channels::FRONT_LEFT, 200_000);
        let sizes = std::cell::RefCell::new(Vec::new());
        let run = run_scripted_stream(&mut decoder, 8_000, 1, &mut |chunk| {
            sizes.borrow_mut().push(chunk.len());
            Ok(())
        })
        .unwrap();
        let sizes = sizes.into_inner();
        assert_eq!(run.streamed, sizes.iter().sum::<usize>());
        assert!(
            sizes.len() >= 190,
            "one delivery per 1024-sample chunk, not one per packet: {}",
            sizes.len()
        );
        assert!(
            sizes.iter().all(|&n| n <= 2_100),
            "a chunk of 1024 source samples at 8 kHz is about 2048 outputs: {sizes:?}"
        );

        let mut decoder = ScriptedDecoder::new(false, 8_000, Channels::FRONT_LEFT, 200_000);
        let calls = std::cell::Cell::new(0usize);
        let error = run_scripted_stream(&mut decoder, 8_000, 1, &mut |_| {
            calls.set(calls.get() + 1);
            if calls.get() == 3 {
                Err(DictationError::FileDecodeError("sink is full".into()))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(matches!(&error, DictationError::FileDecodeError(m) if m == "sink is full"));
        assert_eq!(calls.get(), 3, "no chunk is resampled after the refusal");
    }

    /// Opt-in parity check on a real file of any supported format:
    /// `SAGASCRIPT_STREAM_PARITY_FILE=/path/to/file cargo test --release -p
    /// sagascript-core --lib streaming_decode_parity_on_env_file -- --ignored`.
    #[test]
    #[ignore = "needs SAGASCRIPT_STREAM_PARITY_FILE"]
    fn streaming_decode_parity_on_env_file() {
        let path = PathBuf::from(
            std::env::var_os("SAGASCRIPT_STREAM_PARITY_FILE")
                .expect("set SAGASCRIPT_STREAM_PARITY_FILE"),
        );
        let want = decode_audio_file(&path).unwrap();
        let got = stream(&path, &|| Ok(()));
        let done = got.result.unwrap();
        assert_eq!(done.samples, want.len());
        assert_eq!(got.samples.len(), want.len());
        assert!(bits_of(&got.samples) == bits_of(&want), "samples differ");
        eprintln!(
            "parity ok: {} samples, {} Hz, {} ch, expected {:?}, {} sink calls",
            done.samples,
            done.source_rate,
            done.channels,
            done.expected_samples,
            got.calls.len()
        );
    }
}
