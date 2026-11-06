//! The FLAC encoder of a recording track (ADR 0010, design §4.4).
//!
//! `flacenc`'s frame API encodes one block at a time, so memory stays flat
//! for hours and a killed process leaves every frame that reached the file.
//! The rules found by the spike and the independent review are all here:
//!
//! * **Block size 4096** at every rate; a placeholder STREAMINFO (total
//!   samples and MD5 unknown) is written first, followed by a reserved
//!   metadata region, and rewritten in place when the track finishes.
//! * **Final STREAMINFO `min_blocksize = max_blocksize = 4096`**, even though
//!   the last block is shorter: otherwise libFLAC warns "file might not be
//!   seekable".
//! * **`use_fixed = false`**: with the default configuration `flacenc` 0.5.1
//!   wrote a 100 MB frame for one block of a full-scale square wave. LPC only
//!   (with constant and verbatim subframes) is clean on every edge signal.
//! * **Every frame is compared with the size of the same block stored
//!   verbatim**; if the encoded frame is larger (or its length is not what
//!   `flacenc` claims), the verbatim frame is written instead.
//! * **Samples** are `clamp(round(x * 2^(bits-1)))`, NaN as 0, and the
//!   number of samples that were clamped is counted ([`quantise`]).
//! * **PADDING and SEEKTABLE** blocks: `flacenc` carries them (see
//!   [`format`]); the table has a point every ten seconds and is filled at
//!   finish and by recovery, so a multi-hour file seeks without a rewrite.
//!
//! The file is written through a `BufWriter`: a killed process loses at most
//! the bytes still in it, and [`FlacEncoder::sync`] (every ten seconds and at
//! each gap) bounds what a power loss can take.

pub(crate) mod decode;
pub(crate) mod format;
pub(crate) mod scan;

use std::fs::File;
use std::io::{self, BufWriter, Cursor, SeekFrom, Write};
use std::path::Path;

use flacenc::bitsink::ByteSink;
use flacenc::component::{BitRepr, Stream};
use flacenc::config;
use flacenc::encode_fixed_size_frame;
use flacenc::error::{Verified, Verify};
use flacenc::source::{Context, Fill, FrameBuf};

pub(crate) use self::format::HEADER_LEN;
use self::format::{
    build_header, crc16, crc8, SeekIndex, StreamParams, StreamTotals, BLOCK_SIZE,
    MAX_TOTAL_SAMPLES, RESERVED_LEN,
};
use super::encoder::{quantise, TrackEncoder, TrackSummary};
use super::error::RecordingError;
use super::io::{create_new_private, sync_file};
use crate::hash::StreamingSha256;

/// The most the `BufWriter` of a track holds: about 0.3 s of 24-bit speech.
const WRITE_BUFFER_BYTES: usize = 32 * 1024;

/// Where a track's bytes go: a file, or memory in tests.
pub trait TrackSink: io::Write + io::Seek + Send {
    /// Flush written data to stable storage.
    fn sync_data(&mut self) -> io::Result<()>;
}

impl TrackSink for File {
    fn sync_data(&mut self) -> io::Result<()> {
        sync_file(self)
    }
}

impl TrackSink for Cursor<Vec<u8>> {
    fn sync_data(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// What [`FlacEncoder::finish_into_parts`] returns.
#[doc(hidden)]
pub struct FinishedParts<W> {
    pub summary: TrackSummary,
    pub sink: W,
    pub frame_lengths: Option<Vec<u32>>,
}

/// The streaming FLAC encoder of one track.
pub struct FlacEncoder<W: TrackSink> {
    out: BufWriter<W>,
    params: StreamParams,
    config: Verified<config::Encoder>,
    stream: Stream,
    framebuf: FrameBuf,
    context: Context,
    /// Interleaved samples waiting for a full block (always shorter than one).
    pending: Vec<i32>,
    verbatim: Vec<u8>,
    next_frame: u64,
    frames_pushed: u64,
    clipped: u64,
    sha256: StreamingSha256,
    frame_bytes: u64,
    min_frame_size: u32,
    max_frame_size: u32,
    seek: SeekIndex,
    frame_log: Option<Vec<u32>>,
    /// Set after a failed write: the file no longer matches the encoder.
    broken: bool,
    /// Tests: store every block verbatim, as the size guard does for a frame
    /// that came out larger than its raw form.
    #[cfg(test)]
    force_verbatim: bool,
}

impl FlacEncoder<File> {
    /// Create the track file (it must not exist yet; mode `0600` on Unix) and
    /// write the placeholder header.
    pub fn create(
        path: &Path,
        sample_rate: u32,
        channels: u8,
        bits: u8,
    ) -> Result<Self, RecordingError> {
        let file = create_new_private(path)
            .map_err(|error| RecordingError::io("create a track file", &error))?;
        Self::new(file, sample_rate, channels, bits)
    }
}

impl<W: TrackSink> FlacEncoder<W> {
    /// Start a stream on `sink`, which must be empty and positioned at 0.
    pub fn new(sink: W, sample_rate: u32, channels: u8, bits: u8) -> Result<Self, RecordingError> {
        let params = StreamParams {
            sample_rate,
            channels,
            bits,
        };
        params.validate_for_encoding()?;

        let mut raw = config::Encoder::default();
        raw.block_size = BLOCK_SIZE;
        raw.multithread = false;
        // The fixed predictors produced a 100 MB frame for a full-scale
        // square wave (study §4); LPC, constant and verbatim are used.
        raw.subframe_coding.use_fixed = false;
        let config = raw
            .into_verified()
            .map_err(|(_, error)| RecordingError::encoder(error))?;

        let stream = Stream::new(
            sample_rate as usize,
            usize::from(channels),
            usize::from(bits),
        )
        .map_err(RecordingError::encoder)?;
        let framebuf = FrameBuf::with_size(usize::from(channels), BLOCK_SIZE)
            .map_err(RecordingError::encoder)?;

        let mut out = BufWriter::with_capacity(WRITE_BUFFER_BYTES, sink);
        let header = build_header(params, None, &[], RESERVED_LEN)?;
        debug_assert_eq!(header.len(), HEADER_LEN);
        out.write_all(&header)
            .map_err(|error| RecordingError::io("write the track header", &error))?;

        Ok(Self {
            out,
            params,
            config,
            stream,
            framebuf,
            context: Context::new(usize::from(bits), usize::from(channels)),
            pending: Vec::with_capacity(BLOCK_SIZE * usize::from(channels)),
            verbatim: Vec::new(),
            next_frame: 0,
            frames_pushed: 0,
            clipped: 0,
            sha256: StreamingSha256::new(),
            frame_bytes: 0,
            min_frame_size: u32::MAX,
            max_frame_size: 0,
            seek: SeekIndex::new(sample_rate),
            frame_log: None,
            broken: false,
            #[cfg(test)]
            force_verbatim: false,
        })
    }

    /// Record the length of every frame written (tests that compare against
    /// an independent oracle; it grows with the recording, so not for
    /// production use).
    #[doc(hidden)]
    pub fn with_frame_log(mut self) -> Self {
        self.frame_log = Some(Vec::new());
        self
    }

    /// The lengths of the frames written so far, when [`with_frame_log`] is on.
    #[doc(hidden)]
    pub fn frame_log(&self) -> Option<&[u32]> {
        self.frame_log.as_deref()
    }

    /// Bytes the encoder itself holds, to show that they do not grow with the
    /// length of the recording.
    #[doc(hidden)]
    pub fn working_set_bytes(&self) -> usize {
        self.pending.capacity() * 4
            + self.verbatim.capacity()
            + self.seek.capacity_bytes()
            + self.out.capacity()
    }

    /// Append already-quantised interleaved samples (whole frames).
    pub fn push_pcm(&mut self, interleaved: &[i32]) -> Result<(), RecordingError> {
        self.guard()?;
        let channels = usize::from(self.params.channels);
        if !interleaved.len().is_multiple_of(channels) {
            return Err(RecordingError::InvalidInput {
                reason: "samples are not a whole number of frames",
            });
        }
        self.frames_pushed = self.total_after((interleaved.len() / channels) as u64)?;
        let block_len = BLOCK_SIZE * channels;
        let mut rest = interleaved;
        while !rest.is_empty() {
            let take = (block_len - self.pending.len()).min(rest.len());
            self.pending.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.pending.len() == block_len {
                self.encode_pending()?;
            }
        }
        Ok(())
    }

    fn guard(&self) -> Result<(), RecordingError> {
        if self.broken {
            return Err(RecordingError::TrackFinished);
        }
        Ok(())
    }

    /// The number of frames the track holds after `frames` more, or the error
    /// when STREAMINFO could not count them (36 bits per channel). Checked
    /// arithmetic: no length, however large, wraps or panics.
    fn total_after(&self, frames: u64) -> Result<u64, RecordingError> {
        self.frames_pushed
            .checked_add(frames)
            .filter(|total| *total <= MAX_TOTAL_SAMPLES)
            .ok_or(RecordingError::UnsupportedFormat {
                reason: "the track is longer than FLAC can describe",
            })
    }

    fn encode_pending(&mut self) -> Result<(), RecordingError> {
        let block = std::mem::take(&mut self.pending);
        let result = self.encode_block(&block);
        self.pending = block;
        self.pending.clear();
        if result.is_err() {
            self.broken = true;
        }
        result
    }

    fn encode_block(&mut self, block: &[i32]) -> Result<(), RecordingError> {
        let channels = usize::from(self.params.channels);
        let samples = block.len() / channels;
        self.framebuf
            .fill_interleaved(block)
            .map_err(RecordingError::encoder)?;
        self.context
            .fill_interleaved(block)
            .map_err(RecordingError::encoder)?;

        let frame = encode_fixed_size_frame(
            &self.config,
            &self.framebuf,
            self.next_frame as usize,
            self.stream.stream_info(),
        )
        .map_err(RecordingError::encoder)?;

        let raw_len = verbatim_frame_len(self.next_frame, samples, self.params);
        // `count_bits` is checked before anything is serialised (the 100 MB
        // frame must never be built), the real length after.
        let mut use_verbatim = frame.count_bits().div_ceil(8) > raw_len;
        let mut encoded = ByteSink::new();
        if !use_verbatim {
            frame.write(&mut encoded).map_err(RecordingError::encoder)?;
            use_verbatim = encoded.as_slice().len() > raw_len;
        }
        drop(frame);
        #[cfg(test)]
        {
            use_verbatim |= self.force_verbatim;
        }

        let bytes: &[u8] = if use_verbatim {
            write_verbatim_frame(&mut self.verbatim, self.next_frame, block, self.params);
            &self.verbatim
        } else {
            encoded.as_slice()
        };

        self.sha256.update(bytes);
        self.out
            .write_all(bytes)
            .map_err(|error| RecordingError::io("write a track frame", &error))?;
        let len = bytes.len() as u32;
        self.min_frame_size = self.min_frame_size.min(len);
        self.max_frame_size = self.max_frame_size.max(len);
        self.seek.observe(
            self.next_frame,
            self.next_frame * BLOCK_SIZE as u64,
            self.frame_bytes,
            samples,
        );
        if let Some(log) = &mut self.frame_log {
            log.push(len);
        }
        self.frame_bytes += u64::from(len);
        self.next_frame += 1;
        Ok(())
    }

    /// Finish the stream and hand back the sink (an in-memory track in
    /// tests), the summary and, when [`with_frame_log`](Self::with_frame_log)
    /// is on, the length of every frame.
    #[doc(hidden)]
    pub fn finish_into_parts(mut self) -> Result<FinishedParts<W>, RecordingError> {
        let summary = self.finish_inner()?;
        let frame_lengths = self.frame_log.take();
        let sink = self
            .out
            .into_inner()
            .map_err(|error| RecordingError::io("finish a track file", error.error()))?;
        Ok(FinishedParts {
            summary,
            sink,
            frame_lengths,
        })
    }

    fn finish_inner(&mut self) -> Result<TrackSummary, RecordingError> {
        self.guard()?;
        if !self.pending.is_empty() {
            self.encode_pending()?;
        }
        let io_error = |error: io::Error| RecordingError::io("finish a track file", &error);
        // The last frame is made durable **before** the metadata is rewritten
        // (design §4.5): a power loss during the rewrite must find the frames
        // on disk, and the rewrite is the only thing that can then be torn.
        // Recovery rebuilds a torn header from the frames.
        self.out.flush().map_err(io_error)?;
        self.out.get_mut().sync_data().map_err(io_error)?;
        let md5 = self.context.md5_digest();
        let totals = StreamTotals {
            total_samples: self.frames_pushed,
            md5,
            min_frame_size: if self.next_frame == 0 {
                0
            } else {
                self.min_frame_size
            },
            max_frame_size: self.max_frame_size,
        };
        let header = build_header(self.params, Some(&totals), self.seek.points(), RESERVED_LEN)?;
        debug_assert_eq!(header.len(), HEADER_LEN);
        {
            let sink = self.out.get_mut();
            sink.seek(SeekFrom::Start(0)).map_err(io_error)?;
            sink.write_all(&header).map_err(io_error)?;
            sink.flush().map_err(io_error)?;
            sink.sync_data().map_err(io_error)?;
        }
        Ok(TrackSummary {
            frames: self.frames_pushed,
            audio_sha256: std::mem::take(&mut self.sha256).finish_hex(),
            md5,
            clipped_samples: self.clipped,
            frame_bytes: self.frame_bytes,
            file_bytes: HEADER_LEN as u64 + self.frame_bytes,
        })
    }
}

impl<W: TrackSink> TrackEncoder for FlacEncoder<W> {
    fn push(&mut self, interleaved: &[f32]) -> Result<(), RecordingError> {
        self.guard()?;
        let channels = usize::from(self.params.channels);
        if !interleaved.len().is_multiple_of(channels) {
            return Err(RecordingError::InvalidInput {
                reason: "samples are not a whole number of frames",
            });
        }
        self.frames_pushed = self.total_after((interleaved.len() / channels) as u64)?;
        let block_len = BLOCK_SIZE * channels;
        let bits = self.params.bits;
        for &sample in interleaved {
            let (value, clipped) = quantise(sample, bits);
            self.clipped += u64::from(clipped);
            self.pending.push(value);
            if self.pending.len() == block_len {
                self.encode_pending()?;
            }
        }
        Ok(())
    }

    fn push_silence(&mut self, frames: u64) -> Result<(), RecordingError> {
        self.guard()?;
        let channels = usize::from(self.params.channels);
        let block_len = BLOCK_SIZE * channels;
        // Everything is checked before the encoder changes: once the total
        // fits in 36 bits the sample count (at most two channels) cannot
        // overflow either, but the product is checked anyway.
        let total = self.total_after(frames)?;
        let mut remaining =
            frames
                .checked_mul(channels as u64)
                .ok_or(RecordingError::UnsupportedFormat {
                    reason: "the track is longer than FLAC can describe",
                })?;
        self.frames_pushed = total;
        while remaining > 0 {
            let take = ((block_len - self.pending.len()) as u64).min(remaining) as usize;
            self.pending.resize(self.pending.len() + take, 0);
            remaining -= take as u64;
            if self.pending.len() == block_len {
                self.encode_pending()?;
            }
        }
        Ok(())
    }

    fn ensure_room(&self, frames: u64) -> Result<(), RecordingError> {
        self.guard()?;
        self.total_after(frames).map(|_| ())
    }

    fn frames_pushed(&self) -> u64 {
        self.frames_pushed
    }

    fn clipped_samples(&self) -> u64 {
        self.clipped
    }

    fn bytes_written(&self) -> u64 {
        HEADER_LEN as u64 + self.frame_bytes
    }

    fn sync(&mut self) -> Result<(), RecordingError> {
        self.guard()?;
        let io_error = |error: io::Error| RecordingError::io("sync a track file", &error);
        self.out.flush().map_err(io_error)?;
        self.out.get_mut().sync_data().map_err(io_error)
    }

    fn finish(mut self: Box<Self>) -> Result<TrackSummary, RecordingError> {
        self.finish_inner()
    }
}

// ---- the verbatim fallback ---------------------------------------------------

fn frame_number_len(frame_number: u64) -> usize {
    match frame_number {
        0..=0x7F => 1,
        0x80..=0x7FF => 2,
        0x800..=0xFFFF => 3,
        0x1_0000..=0x1F_FFFF => 4,
        0x20_0000..=0x3FF_FFFF => 5,
        _ => 6,
    }
}

fn block_size_ext_len(samples: usize) -> usize {
    match samples {
        BLOCK_SIZE => 0,
        0..=256 => 1,
        _ => 2,
    }
}

/// The length of the frame that stores `samples` samples of every channel
/// verbatim: the size an encoded frame must not exceed.
fn verbatim_frame_len(frame_number: u64, samples: usize, params: StreamParams) -> usize {
    let header = 4 + frame_number_len(frame_number) + block_size_ext_len(samples) + 1;
    let bits = usize::from(params.channels) * (8 + samples * usize::from(params.bits));
    header + bits.div_ceil(8) + 2
}

/// A bit writer for the verbatim frame.
struct BitPacker<'a> {
    out: &'a mut Vec<u8>,
    acc: u64,
    nbits: u32,
}

impl BitPacker<'_> {
    fn put(&mut self, value: u64, bits: u32) {
        debug_assert!(bits <= 32);
        self.acc = (self.acc << bits) | (value & ((1u64 << bits) - 1));
        self.nbits += bits;
        while self.nbits >= 8 {
            self.nbits -= 8;
            self.out.push((self.acc >> self.nbits) as u8);
        }
        self.acc &= (1u64 << self.nbits) - 1;
    }

    fn finish(&mut self) {
        if self.nbits > 0 {
            let pad = 8 - self.nbits;
            self.put(0, pad);
        }
    }
}

/// Build the verbatim (uncompressed, independent channels) frame for `block`.
fn write_verbatim_frame(out: &mut Vec<u8>, frame_number: u64, block: &[i32], params: StreamParams) {
    out.clear();
    let channels = usize::from(params.channels);
    let samples = block.len() / channels;
    // Header: sync, block size code, sample rate from STREAMINFO, channels,
    // sample size.
    let block_code: u8 = match samples {
        BLOCK_SIZE => 12,
        0..=256 => 6,
        _ => 7,
    };
    let size_code: u8 = if params.bits == 16 { 4 } else { 6 };
    out.extend_from_slice(&[
        0xFF,
        0xF8,
        block_code << 4,
        ((params.channels - 1) << 4) | (size_code << 1),
    ]);
    // The frame number, UTF-8 style.
    match frame_number_len(frame_number) {
        1 => out.push(frame_number as u8),
        n => {
            let n = n as u32;
            let lead = (0xFFu16 << (8 - n)) as u8;
            out.push(lead | (frame_number >> (6 * (n - 1))) as u8);
            for i in (0..n - 1).rev() {
                out.push(0x80 | ((frame_number >> (6 * i)) & 0x3F) as u8);
            }
        }
    }
    match block_code {
        6 => out.push((samples - 1) as u8),
        7 => out.extend_from_slice(&((samples - 1) as u16).to_be_bytes()),
        _ => {}
    }
    let header_crc = crc8(out);
    out.push(header_crc);
    // Subframes: verbatim, no wasted bits.
    let mut packer = BitPacker {
        out,
        acc: 0,
        nbits: 0,
    };
    let bits = u32::from(params.bits);
    for channel in 0..channels {
        packer.put(0b0000_0010, 8);
        for sample in 0..samples {
            packer.put(block[sample * channels + channel] as u32 as u64, bits);
        }
    }
    packer.finish();
    let frame_crc = crc16(out);
    out.extend_from_slice(&frame_crc.to_be_bytes());
}

#[cfg(test)]
#[path = "flac_tests.rs"]
mod tests;
