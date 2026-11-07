//! The encoder seam of a recording track (design §4.4) and the one conversion
//! from the capture API's `f32` samples to the integers that are stored.
//!
//! [`TrackEncoder`] is what the writer thread drives. The only implementation
//! in v1 is [`FlacEncoder`](super::FlacEncoder); a lossy encoder could sit
//! behind the same trait later (it would need its own crash-safe container).

use super::error::RecordingError;

/// What a finished track is: the numbers the manifest records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackSummary {
    /// Sample frames (per channel) stored, silence of gaps included.
    pub frames: u64,
    /// SHA-256 over the FLAC frame bytes, as 64 lowercase hex digits.
    pub audio_sha256: String,
    /// The MD5 of the PCM that STREAMINFO carries.
    pub md5: [u8; 16],
    /// Samples that were clamped (or NaN) when converted to integers.
    pub clipped_samples: u64,
    /// Bytes of FLAC frames (the audio, without the metadata blocks).
    pub frame_bytes: u64,
    /// Size of the file.
    pub file_bytes: u64,
}

/// A track's encoder: interleaved `f32` samples in, a file out.
pub trait TrackEncoder: Send {
    /// Append interleaved samples (whole frames: a multiple of the channel
    /// count). Samples outside -1.0..1.0 are clamped and counted.
    fn push(&mut self, interleaved: &[f32]) -> Result<(), RecordingError>;

    /// Append `frames` frames of digital silence (a gap); no memory is
    /// allocated for the length. A length the track cannot hold is refused
    /// before anything changes ([`ensure_room`](Self::ensure_room)).
    fn push_silence(&mut self, frames: u64) -> Result<(), RecordingError>;

    /// Whether `frames` more frames fit in the track (the container counts
    /// samples in a fixed number of bits). A writer asks before it journals a
    /// gap, so a refusal leaves no trace.
    fn ensure_room(&self, frames: u64) -> Result<(), RecordingError>;

    /// Sample frames accepted so far, including those still waiting for a
    /// full block. A gap starts at this index.
    fn frames_pushed(&self) -> u64;

    /// Samples clamped (or NaN) so far.
    fn clipped_samples(&self) -> u64;

    /// Bytes the track file holds or is about to hold: the header and every
    /// frame encoded so far (not the partial block that waits, not the
    /// `BufWriter` contents lost to an error). The session reports it and
    /// stops a FAT32 track before the 4 GiB limit.
    fn bytes_written(&self) -> u64;

    /// Flush buffered bytes and fsync the file. A full block is encoded as
    /// soon as it is complete; the partial block that waits is not written.
    fn sync(&mut self) -> Result<(), RecordingError>;

    /// Write the last block, rewrite the metadata and fsync.
    fn finish(self: Box<Self>) -> Result<TrackSummary, RecordingError>;

    /// The track ends without a finish: something went wrong after
    /// `keep_frames` frames were accepted (a gap that failed part-way), so
    /// whatever came after them is in doubt. Write out the audio up to
    /// `keep_frames`, **including the partial block still waiting for the rest
    /// of its block**, and fsync it. The metadata is left as it is for
    /// recovery to complete, and nothing past `keep_frames` is added (blocks
    /// of silence that already reached the file stay). The encoder takes
    /// nothing more afterwards, and `keep_frames` more than it accepted is
    /// refused.
    fn salvage(&mut self, keep_frames: u64) -> Result<(), RecordingError>;
}

/// Convert one `f32` sample to a signed integer of `bits` bits:
/// `clamp(round(x * 2^(bits-1)), -2^(bits-1), 2^(bits-1) - 1)`, with NaN
/// stored as 0. The flag says the sample was clamped or NaN, so the manifest's
/// `clipped_samples` counts every sample that is not exactly what the device
/// delivered. A float sample of exactly +1.0 is out of range by one step and
/// counts as clipped; -1.0 is exact.
pub(crate) fn quantise(sample: f32, bits: u8) -> (i32, bool) {
    if sample.is_nan() {
        return (0, true);
    }
    let scale = (1u32 << (bits - 1)) as f32;
    let max = scale - 1.0;
    let min = -scale;
    // Multiplying by a power of two is exact in f32 (barring overflow to
    // infinity, which clamps below).
    let scaled = (sample * scale).round();
    if scaled > max {
        (max as i32, true)
    } else if scaled < min {
        (min as i32, true)
    } else {
        (scaled as i32, false)
    }
}

#[cfg(test)]
#[path = "encoder_tests.rs"]
mod tests;
