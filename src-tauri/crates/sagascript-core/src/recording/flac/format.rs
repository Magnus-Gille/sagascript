//! FLAC bytes: the CRCs, the file header this store writes and reads, and the
//! frame header (RFC 9639).
//!
//! # The file header
//!
//! Every track file starts with the same fixed-size header, so STREAMINFO and
//! the seek table can be rewritten in place at the end of a recording and by
//! recovery without moving a byte of audio:
//!
//! ```text
//! "fLaC"
//! STREAMINFO block   (4 + 34 bytes)
//! reserved region    RESERVED_LEN bytes, one of
//!   while recording  PADDING (all of it)
//!   when finished    SEEKTABLE (n points), PADDING (the rest, possibly empty)
//! frames...
//! ```
//!
//! `flacenc` 0.5.1 writes STREAMINFO and carries arbitrary metadata blocks
//! (`MetadataBlockData::new_unknown`), so PADDING (type 1) and SEEKTABLE
//! (type 3) are written through it; only their contents are built here.

use flacenc::bitsink::ByteSink;
use flacenc::component::{BitRepr, MetadataBlockData, Stream};

use crate::recording::error::RecordingError;

/// Samples per frame, for every rate (design §4.4).
pub(crate) const BLOCK_SIZE: usize = 4096;
/// STREAMINFO body length.
pub(crate) const STREAMINFO_LEN: usize = 34;
const BLOCK_HEADER_LEN: usize = 4;
/// Bytes of one SEEKTABLE point.
pub(crate) const SEEK_POINT_LEN: usize = 18;
/// Seek points the reserved region can hold. One point per ten seconds fills
/// it after 5.7 hours; longer recordings thin the table (see `SeekIndex`).
pub(crate) const SEEK_RESERVE_POINTS: usize = 2048;
/// The reserved region after STREAMINFO: a SEEKTABLE and a PADDING block
/// header and the points.
pub(crate) const RESERVED_LEN: usize = 2 * BLOCK_HEADER_LEN + SEEK_POINT_LEN * SEEK_RESERVE_POINTS;
/// Where the reserved region starts: "fLaC", the STREAMINFO header and body.
pub(crate) const RESERVED_START: usize = 4 + BLOCK_HEADER_LEN + STREAMINFO_LEN;
/// The whole header of a file this store creates.
pub(crate) const HEADER_LEN: usize = RESERVED_START + RESERVED_LEN;
/// The largest total sample count STREAMINFO can hold (36 bits).
const MAX_TOTAL_SAMPLES: u64 = (1 << 36) - 1;

// ---- CRCs ----------------------------------------------------------------

const fn crc8_table() -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

const fn crc16_table() -> [u16; 256] {
    let mut table = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u16) << 8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

static CRC8_TABLE: [u8; 256] = crc8_table();
static CRC16_TABLE: [u16; 256] = crc16_table();

/// The frame header CRC: polynomial 0x07, initial value 0.
pub(crate) fn crc8(data: &[u8]) -> u8 {
    data.iter()
        .fold(0u8, |crc, byte| CRC8_TABLE[usize::from(crc ^ byte)])
}

/// The frame CRC: polynomial 0x8005, initial value 0, no reflection, no final xor.
pub(crate) fn crc16(data: &[u8]) -> u16 {
    data.iter().fold(0u16, |crc, byte| {
        (crc << 8) ^ CRC16_TABLE[usize::from((crc >> 8) as u8 ^ byte)]
    })
}

// ---- stream parameters and the file header --------------------------------

/// What a FLAC stream is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StreamParams {
    pub sample_rate: u32,
    pub channels: u8,
    pub bits: u8,
}

impl StreamParams {
    /// What the encoder writes: 16 or 24 bit, one or two channels, any rate
    /// FLAC can name.
    pub(crate) fn validate_for_encoding(self) -> Result<(), RecordingError> {
        if self.sample_rate == 0 || self.sample_rate > 655_350 {
            return Err(RecordingError::UnsupportedFormat {
                reason: "sample rate must be 1 to 655350 Hz",
            });
        }
        if !(1..=2).contains(&self.channels) {
            return Err(RecordingError::UnsupportedFormat {
                reason: "only 1 or 2 channels are stored",
            });
        }
        if self.bits != 16 && self.bits != 24 {
            return Err(RecordingError::UnsupportedFormat {
                reason: "only 16 and 24 bit are stored",
            });
        }
        Ok(())
    }
}

/// The STREAMINFO fields known only at the end of a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StreamTotals {
    pub total_samples: u64,
    pub md5: [u8; 16],
    pub min_frame_size: u32,
    pub max_frame_size: u32,
}

/// One SEEKTABLE entry: the first sample of a frame, the frame's byte offset
/// from the first frame header, and the frame's block size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SeekPoint {
    pub sample: u64,
    pub offset: u64,
    pub samples: u16,
}

impl SeekPoint {
    fn to_bytes(self) -> [u8; SEEK_POINT_LEN] {
        let mut bytes = [0u8; SEEK_POINT_LEN];
        bytes[0..8].copy_from_slice(&self.sample.to_be_bytes());
        bytes[8..16].copy_from_slice(&self.offset.to_be_bytes());
        bytes[16..18].copy_from_slice(&self.samples.to_be_bytes());
        bytes
    }
}

/// Build the header of `region_len` reserved bytes. `totals` is `None` for the
/// placeholder written first (total samples and MD5 unknown, frame sizes
/// unknown). With points the region is `SEEKTABLE + PADDING`; as many points
/// as fit are written (at most `region_len`'s share).
pub(crate) fn build_header(
    params: StreamParams,
    totals: Option<&StreamTotals>,
    seek: &[SeekPoint],
    region_len: usize,
) -> Result<Vec<u8>, RecordingError> {
    let mut stream = Stream::new(
        params.sample_rate as usize,
        usize::from(params.channels),
        usize::from(params.bits),
    )
    .map_err(RecordingError::encoder)?;
    {
        let info = stream.stream_info_mut();
        // Fixed-size blocks: the short last block does not lower the minimum
        // (otherwise libFLAC warns "file might not be seekable").
        info.set_block_sizes(BLOCK_SIZE, BLOCK_SIZE)
            .map_err(RecordingError::encoder)?;
        match totals {
            Some(totals) => {
                if totals.total_samples > MAX_TOTAL_SAMPLES {
                    return Err(RecordingError::UnsupportedFormat {
                        reason: "the track is longer than FLAC can describe",
                    });
                }
                info.set_total_samples(totals.total_samples as usize);
                info.set_md5_digest(&totals.md5);
                info.set_frame_sizes(
                    totals.min_frame_size as usize,
                    totals.max_frame_size as usize,
                )
                .map_err(RecordingError::encoder)?;
            }
            None => info
                .set_frame_sizes(0, 0)
                .map_err(RecordingError::encoder)?,
        }
    }
    if region_len >= 2 * BLOCK_HEADER_LEN {
        let fit = (region_len - 2 * BLOCK_HEADER_LEN) / SEEK_POINT_LEN;
        let points = &seek[..seek.len().min(fit)];
        let padding_len = if points.is_empty() {
            region_len - BLOCK_HEADER_LEN
        } else {
            region_len - 2 * BLOCK_HEADER_LEN - points.len() * SEEK_POINT_LEN
        };
        if !points.is_empty() {
            let mut table = Vec::with_capacity(points.len() * SEEK_POINT_LEN);
            for point in points {
                table.extend_from_slice(&point.to_bytes());
            }
            stream.add_metadata_block(
                MetadataBlockData::new_unknown(3, &table).map_err(RecordingError::encoder)?,
            );
        }
        stream.add_metadata_block(
            MetadataBlockData::new_unknown(1, &vec![0u8; padding_len])
                .map_err(RecordingError::encoder)?,
        );
    } else if region_len != 0 {
        // Too small for a block: cannot be re-laid out.
        return Err(RecordingError::CorruptTrack {
            reason: "the metadata region is too small to rewrite",
        });
    }
    let mut sink = ByteSink::new();
    stream.write(&mut sink).map_err(RecordingError::encoder)?;
    let bytes = sink.as_slice().to_vec();
    debug_assert_eq!(bytes.len(), RESERVED_START + region_len);
    Ok(bytes)
}

/// Why a header could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeaderError {
    /// The bytes end inside the header.
    Truncated,
    /// Not a FLAC stream this store could have written.
    NotFlac,
}

/// A parsed file header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParsedHeader {
    pub params: StreamParams,
    /// The STREAMINFO body, as the decoder wants it.
    pub streaminfo: [u8; STREAMINFO_LEN],
    pub total_samples: u64,
    pub md5: [u8; 16],
    pub min_block_size: u16,
    pub max_block_size: u16,
    pub min_frame_size: u32,
    pub max_frame_size: u32,
    /// Offset of the first frame header.
    pub first_frame: usize,
}

impl ParsedHeader {
    /// The bytes between STREAMINFO and the first frame.
    pub(crate) fn region_len(&self) -> usize {
        self.first_frame - RESERVED_START
    }
}

/// Read the header at the start of `bytes`.
pub(crate) fn parse_header(bytes: &[u8]) -> Result<ParsedHeader, HeaderError> {
    if bytes.len() < 4 {
        return Err(if b"fLaC".starts_with(bytes) {
            HeaderError::Truncated
        } else {
            HeaderError::NotFlac
        });
    }
    if &bytes[0..4] != b"fLaC" {
        return Err(HeaderError::NotFlac);
    }
    let mut position = 4;
    let mut parsed: Option<ParsedHeader> = None;
    loop {
        if bytes.len() < position + BLOCK_HEADER_LEN {
            return Err(HeaderError::Truncated);
        }
        let flags = bytes[position];
        let last = flags & 0x80 != 0;
        let kind = flags & 0x7F;
        let length = (usize::from(bytes[position + 1]) << 16)
            | (usize::from(bytes[position + 2]) << 8)
            | usize::from(bytes[position + 3]);
        position += BLOCK_HEADER_LEN;
        if kind == 127 {
            return Err(HeaderError::NotFlac);
        }
        if parsed.is_none() {
            // The first block must be STREAMINFO.
            if kind != 0 || length != STREAMINFO_LEN {
                return Err(HeaderError::NotFlac);
            }
            if bytes.len() < position + STREAMINFO_LEN {
                return Err(HeaderError::Truncated);
            }
            let mut body = [0u8; STREAMINFO_LEN];
            body.copy_from_slice(&bytes[position..position + STREAMINFO_LEN]);
            parsed = Some(parse_streaminfo(&body)?);
        } else if bytes.len() < position + length {
            return Err(HeaderError::Truncated);
        }
        position += length;
        if last {
            break;
        }
    }
    let mut parsed = parsed.ok_or(HeaderError::NotFlac)?;
    parsed.first_frame = position;
    Ok(parsed)
}

fn parse_streaminfo(body: &[u8; STREAMINFO_LEN]) -> Result<ParsedHeader, HeaderError> {
    let min_block_size = u16::from_be_bytes([body[0], body[1]]);
    let max_block_size = u16::from_be_bytes([body[2], body[3]]);
    let min_frame_size = u32::from_be_bytes([0, body[4], body[5], body[6]]);
    let max_frame_size = u32::from_be_bytes([0, body[7], body[8], body[9]]);
    // 20 bits sample rate, 3 bits channels - 1, 5 bits bits - 1, 36 bits total.
    let packed = u64::from_be_bytes([
        body[10], body[11], body[12], body[13], body[14], body[15], body[16], body[17],
    ]);
    let sample_rate = (packed >> 44) as u32;
    let channels = ((packed >> 41) & 0x7) as u8 + 1;
    let bits = ((packed >> 36) & 0x1F) as u8 + 1;
    let total_samples = packed & ((1 << 36) - 1);
    let mut md5 = [0u8; 16];
    md5.copy_from_slice(&body[18..34]);
    if sample_rate == 0 {
        return Err(HeaderError::NotFlac);
    }
    Ok(ParsedHeader {
        params: StreamParams {
            sample_rate,
            channels,
            bits,
        },
        streaminfo: *body,
        total_samples,
        md5,
        min_block_size,
        max_block_size,
        min_frame_size,
        max_frame_size,
        first_frame: 0,
    })
}

// ---- the seek table -----------------------------------------------------------

/// Chooses the frames that get a SEEKTABLE point while a stream is written or
/// scanned: one every ten seconds, thinned (every second point dropped, the
/// interval doubled) when the table would outgrow the reserved region, so its
/// memory and the file header stay bounded for a recording of any length.
#[derive(Clone, Debug)]
pub(crate) struct SeekIndex {
    interval_frames: u64,
    points: Vec<SeekPoint>,
}

impl SeekIndex {
    pub(crate) fn new(sample_rate: u32) -> Self {
        let interval_frames = (u64::from(sample_rate) * 10)
            .div_ceil(BLOCK_SIZE as u64)
            .max(1);
        Self {
            interval_frames,
            points: Vec::with_capacity(SEEK_RESERVE_POINTS + 1),
        }
    }

    /// Frame `frame_number` starts at `sample` and `offset` bytes after the
    /// first frame header and holds `block_size` samples.
    pub(crate) fn observe(
        &mut self,
        frame_number: u64,
        sample: u64,
        offset: u64,
        block_size: usize,
    ) {
        if !frame_number.is_multiple_of(self.interval_frames) {
            return;
        }
        self.points.push(SeekPoint {
            sample,
            offset,
            samples: block_size.min(usize::from(u16::MAX)) as u16,
        });
        if self.points.len() > SEEK_RESERVE_POINTS {
            let thinned: Vec<SeekPoint> = self.points.iter().copied().step_by(2).collect();
            self.points.clear();
            self.points.extend(thinned);
            self.interval_frames *= 2;
        }
    }

    pub(crate) fn points(&self) -> &[SeekPoint] {
        &self.points
    }

    pub(crate) fn capacity_bytes(&self) -> usize {
        self.points.capacity() * std::mem::size_of::<SeekPoint>()
    }
}

// ---- frame headers ---------------------------------------------------------

/// A parse that can need more bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Parse<T> {
    Ok(T),
    /// The buffer ends before the answer is known.
    NeedMore,
    /// These bytes are not a frame of this stream.
    Invalid,
}

/// A frame header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FrameHeader {
    /// Bytes the header takes, CRC-8 included.
    pub len: usize,
    pub block_size: usize,
    pub frame_number: u64,
    pub assignment: ChannelAssignment,
}

/// How a frame's channels are coded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChannelAssignment {
    /// Each of `n` channels coded alone.
    Independent(u8),
    LeftSide,
    SideRight,
    MidSide,
}

impl ChannelAssignment {
    pub(crate) fn channels(self) -> u8 {
        match self {
            Self::Independent(n) => n,
            _ => 2,
        }
    }

    /// The sample size of channel `index` given the stream's: the side
    /// channel needs one bit more.
    pub(crate) fn subframe_bits(self, index: usize, bits: u8) -> u32 {
        let wide = match self {
            Self::Independent(_) => false,
            Self::LeftSide | Self::MidSide => index == 1,
            Self::SideRight => index == 0,
        };
        u32::from(bits) + u32::from(wide)
    }
}

const SAMPLE_RATES: [u32; 12] = [
    0, 88_200, 176_400, 192_000, 8_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000, 96_000,
];

/// Read a frame header at the start of `buf` against the stream it must
/// belong to (fixed-size blocks only).
pub(crate) fn parse_frame_header(buf: &[u8], stream: StreamParams) -> Parse<FrameHeader> {
    if buf.len() < 4 {
        // The first bytes can already be wrong.
        if !buf.is_empty() && buf[0] != 0xFF {
            return Parse::Invalid;
        }
        if buf.len() >= 2 && buf[1] != 0xF8 {
            return Parse::Invalid;
        }
        return Parse::NeedMore;
    }
    // Sync code, then reserved 0 and blocking strategy 0 (fixed): 0xFFF8.
    if buf[0] != 0xFF || buf[1] != 0xF8 {
        return Parse::Invalid;
    }
    let block_code = buf[2] >> 4;
    let rate_code = buf[2] & 0x0F;
    let channel_code = buf[3] >> 4;
    let size_code = (buf[3] >> 1) & 0x07;
    if buf[3] & 1 != 0 {
        return Parse::Invalid;
    }
    let assignment = match channel_code {
        0..=7 => ChannelAssignment::Independent(channel_code + 1),
        8 => ChannelAssignment::LeftSide,
        9 => ChannelAssignment::SideRight,
        10 => ChannelAssignment::MidSide,
        _ => return Parse::Invalid,
    };
    if assignment.channels() != stream.channels {
        return Parse::Invalid;
    }
    let size_bits = match size_code {
        0 => stream.bits,
        1 => 8,
        2 => 12,
        4 => 16,
        5 => 20,
        6 => 24,
        7 => 32,
        _ => return Parse::Invalid,
    };
    if size_bits != stream.bits {
        return Parse::Invalid;
    }

    // The coded frame number: UTF-8 style, 1 to 7 bytes.
    if buf.len() < 5 {
        return Parse::NeedMore;
    }
    let first = buf[4];
    let (extra, mut value) = match first {
        0x00..=0x7F => (0usize, u64::from(first)),
        0xC0..=0xDF => (1, u64::from(first & 0x1F)),
        0xE0..=0xEF => (2, u64::from(first & 0x0F)),
        0xF0..=0xF7 => (3, u64::from(first & 0x07)),
        0xF8..=0xFB => (4, u64::from(first & 0x03)),
        0xFC..=0xFD => (5, u64::from(first & 0x01)),
        0xFE => (6, 0),
        _ => return Parse::Invalid,
    };
    let mut position = 5;
    for _ in 0..extra {
        let Some(&byte) = buf.get(position) else {
            return Parse::NeedMore;
        };
        if byte & 0xC0 != 0x80 {
            return Parse::Invalid;
        }
        value = (value << 6) | u64::from(byte & 0x3F);
        position += 1;
    }
    if value >= 1 << 31 {
        return Parse::Invalid;
    }

    let block_size = match block_code {
        0 => return Parse::Invalid,
        1 => 192,
        2..=5 => 576usize << (block_code - 2),
        6 => {
            let Some(&byte) = buf.get(position) else {
                return Parse::NeedMore;
            };
            position += 1;
            usize::from(byte) + 1
        }
        7 => {
            let Some(pair) = buf.get(position..position + 2) else {
                return Parse::NeedMore;
            };
            position += 2;
            usize::from(u16::from_be_bytes([pair[0], pair[1]])) + 1
        }
        _ => 256usize << (block_code - 8),
    };

    let rate = match rate_code {
        0 => stream.sample_rate,
        1..=11 => SAMPLE_RATES[usize::from(rate_code)],
        12 => {
            let Some(&byte) = buf.get(position) else {
                return Parse::NeedMore;
            };
            position += 1;
            u32::from(byte) * 1000
        }
        13 | 14 => {
            let Some(pair) = buf.get(position..position + 2) else {
                return Parse::NeedMore;
            };
            position += 2;
            let raw = u32::from(u16::from_be_bytes([pair[0], pair[1]]));
            if rate_code == 13 {
                raw
            } else {
                raw * 10
            }
        }
        _ => return Parse::Invalid,
    };
    if rate != stream.sample_rate {
        return Parse::Invalid;
    }

    let Some(&stored_crc) = buf.get(position) else {
        return Parse::NeedMore;
    };
    if crc8(&buf[..position]) != stored_crc {
        return Parse::Invalid;
    }
    Parse::Ok(FrameHeader {
        len: position + 1,
        block_size,
        frame_number: value,
        assignment,
    })
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
