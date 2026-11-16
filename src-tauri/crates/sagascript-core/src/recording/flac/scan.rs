//! Finding where FLAC frames end without decoding their samples.
//!
//! A FLAC frame has no length field. Recovery needs the exact frames a killed
//! process left, so a frame is *walked*: the header is parsed (CRC-8), each
//! subframe's structure is followed bit by bit to the end of the residual
//! (the Rice codes are counted, not decoded), the frame is padded to a byte
//! and its CRC-16 is compared with the two bytes that follow. A frame is valid
//! only when all of that agrees; the first position where it does not is where
//! the torn tail starts. Nothing is searched for: a frame starts where the
//! previous one ended, and its number must be the next one.

use std::io::{self, Read};

use super::format::{
    crc16, parse_frame_header, ChannelAssignment, Parse, StreamParams, BLOCK_SIZE,
};

/// The largest frame accepted. A raw 2-channel 32-bit block of 65535 samples
/// is 512 KiB; ours are far smaller.
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
/// How much is read from the file at a time.
const READ_CHUNK: usize = 256 * 1024;

/// A reader of bits, most significant first, over a byte slice.
struct Bits<'a> {
    buf: &'a [u8],
    /// Position in bits.
    pos: usize,
}

/// Why a walk stopped before the end of a frame.
#[derive(Debug, PartialEq, Eq)]
enum Stop {
    /// The buffer ended inside the frame.
    NeedMore,
    /// The bytes are not a frame.
    Invalid,
}

impl<'a> Bits<'a> {
    fn left(&self) -> usize {
        self.buf.len() * 8 - self.pos
    }

    fn read(&mut self, n: u32) -> Result<u64, Stop> {
        debug_assert!(n <= 32);
        if self.left() < n as usize {
            return Err(Stop::NeedMore);
        }
        if n == 0 {
            return Ok(0);
        }
        let byte = self.pos >> 3;
        let bit = self.pos & 7;
        let need = (bit + n as usize).div_ceil(8);
        let mut acc = 0u64;
        for i in 0..need {
            acc = (acc << 8) | u64::from(self.buf[byte + i]);
        }
        acc >>= need * 8 - bit - n as usize;
        self.pos += n as usize;
        Ok(acc & ((1u64 << n) - 1))
    }

    fn skip(&mut self, n: usize) -> Result<(), Stop> {
        if self.left() < n {
            return Err(Stop::NeedMore);
        }
        self.pos += n;
        Ok(())
    }

    /// Count zero bits up to and including the next one bit; returns the zeros.
    fn unary(&mut self) -> Result<u32, Stop> {
        let mut zeros = 0u32;
        loop {
            let byte_index = self.pos >> 3;
            let Some(&byte) = self.buf.get(byte_index) else {
                return Err(Stop::NeedMore);
            };
            let bit = (self.pos & 7) as u32;
            let remaining = byte << bit; // the unread bits, left aligned
            if remaining != 0 {
                let lead = remaining.leading_zeros();
                zeros += lead;
                self.pos += lead as usize + 1;
                return Ok(zeros);
            }
            let consumed = 8 - bit;
            zeros += consumed;
            self.pos += consumed as usize;
            if zeros > (MAX_FRAME_BYTES as u32) * 8 {
                return Err(Stop::Invalid);
            }
        }
    }

    fn align(&mut self) -> Result<(), Stop> {
        let pad = (8 - (self.pos & 7)) & 7;
        // The padding bits must be zero.
        if pad > 0 && self.read(pad as u32)? != 0 {
            return Err(Stop::Invalid);
        }
        Ok(())
    }
}

/// Walk one subframe of `bits_per_sample` bits and `block` samples.
fn walk_subframe(bits: &mut Bits<'_>, bits_per_sample: u32, block: usize) -> Result<(), Stop> {
    if bits.read(1)? != 0 {
        return Err(Stop::Invalid);
    }
    let kind = bits.read(6)? as u32;
    let mut sample_bits = bits_per_sample;
    if bits.read(1)? == 1 {
        // Wasted bits: k - 1 zeros, then a one.
        let wasted = bits.unary()? + 1;
        if wasted >= sample_bits {
            return Err(Stop::Invalid);
        }
        sample_bits -= wasted;
    }
    let order = match kind {
        0 => {
            bits.skip(sample_bits as usize)?;
            return Ok(());
        }
        1 => {
            bits.skip(
                block
                    .checked_mul(sample_bits as usize)
                    .ok_or(Stop::Invalid)?,
            )?;
            return Ok(());
        }
        8..=15 => {
            let order = (kind & 7) as usize;
            if order > 4 {
                return Err(Stop::Invalid);
            }
            bits.skip(order * sample_bits as usize)?;
            order
        }
        32..=63 => {
            let order = (kind & 31) as usize + 1;
            bits.skip(order * sample_bits as usize)?;
            let precision = bits.read(4)? as usize;
            if precision == 15 {
                return Err(Stop::Invalid);
            }
            bits.skip(5)?; // the shift
            bits.skip(order * (precision + 1))?;
            order
        }
        _ => return Err(Stop::Invalid),
    };
    walk_residual(bits, block, order)
}

fn walk_residual(bits: &mut Bits<'_>, block: usize, order: usize) -> Result<(), Stop> {
    let (parameter_bits, escape) = match bits.read(2)? {
        0 => (4u32, 15u64),
        1 => (5, 31),
        _ => return Err(Stop::Invalid),
    };
    let partition_order = bits.read(4)? as u32;
    let partitions = 1usize << partition_order;
    if !block.is_multiple_of(partitions) || block / partitions < order {
        return Err(Stop::Invalid);
    }
    let per_partition = block / partitions;
    for index in 0..partitions {
        let samples = if index == 0 {
            per_partition - order
        } else {
            per_partition
        };
        let parameter = bits.read(parameter_bits)?;
        if parameter == escape {
            let raw = bits.read(5)? as usize;
            bits.skip(samples * raw)?;
        } else {
            let k = parameter as usize;
            for _ in 0..samples {
                bits.unary()?;
                bits.skip(k)?;
            }
        }
    }
    Ok(())
}

/// A complete, valid frame found in a buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FrameEnd {
    /// Bytes the frame takes, CRC-16 included.
    pub len: usize,
    pub block_size: usize,
}

/// The result of looking for a frame at the start of a buffer.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Walk {
    Frame(FrameEnd),
    NeedMore,
    Invalid,
}

/// Is there a valid frame numbered `expected_number` at the start of `buf`?
pub(crate) fn walk_frame(buf: &[u8], stream: StreamParams, expected_number: u64) -> Walk {
    let header = match parse_frame_header(buf, stream) {
        Parse::Ok(header) => header,
        Parse::NeedMore => return Walk::NeedMore,
        Parse::Invalid => return Walk::Invalid,
    };
    if header.frame_number != expected_number
        || header.block_size == 0
        || header.block_size > BLOCK_SIZE
    {
        return Walk::Invalid;
    }
    let mut bits = Bits {
        buf,
        pos: header.len * 8,
    };
    let channels = header.assignment.channels();
    let assignment: ChannelAssignment = header.assignment;
    let result = (|| {
        for index in 0..usize::from(channels) {
            walk_subframe(
                &mut bits,
                assignment.subframe_bits(index, stream.bits),
                header.block_size,
            )?;
        }
        bits.align()?;
        let end = bits.pos / 8;
        let Some(stored) = buf.get(end..end + 2) else {
            return Err(Stop::NeedMore);
        };
        if crc16(&buf[..end]) != u16::from_be_bytes([stored[0], stored[1]]) {
            return Err(Stop::Invalid);
        }
        Ok(FrameEnd {
            len: end + 2,
            block_size: header.block_size,
        })
    })();
    match result {
        Ok(end) => Walk::Frame(end),
        Err(Stop::NeedMore) => Walk::NeedMore,
        Err(Stop::Invalid) => Walk::Invalid,
    }
}

/// A frame handed to the scan callback.
pub(crate) struct ScannedFrame<'a> {
    pub bytes: &'a [u8],
    pub block_size: usize,
    pub frame_number: u64,
    /// The frame's offset in the file.
    pub offset: u64,
}

/// What a scan found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScanSummary {
    /// Frames accepted.
    pub frames: u64,
    pub samples: u64,
    /// The file offset just after the last accepted frame: where the tail,
    /// if any, starts.
    pub end_offset: u64,
}

/// Walk the frames that follow `start_offset` in `reader` (which is positioned
/// there), in order, handing each valid one to `accept`. The scan ends at the
/// first position that is not a complete valid next frame, at a frame `accept`
/// refuses, or at the end of the stream. A frame shorter than a block must be
/// the last.
pub(crate) fn scan_frames<R: Read>(
    reader: &mut R,
    start_offset: u64,
    stream: StreamParams,
    accept: &mut dyn FnMut(&ScannedFrame<'_>) -> bool,
) -> io::Result<ScanSummary> {
    let mut summary = ScanSummary {
        frames: 0,
        samples: 0,
        end_offset: start_offset,
    };
    let mut window: Vec<u8> = Vec::with_capacity(READ_CHUNK + 4096);
    let mut head = 0usize; // start of unconsumed bytes in `window`
    let mut at_eof = false;
    let mut short_seen = false;
    loop {
        if short_seen {
            break;
        }
        let walk = walk_frame(&window[head..], stream, summary.frames);
        match walk {
            Walk::Frame(end) => {
                let bytes = &window[head..head + end.len];
                let frame = ScannedFrame {
                    bytes,
                    block_size: end.block_size,
                    frame_number: summary.frames,
                    offset: summary.end_offset,
                };
                if !accept(&frame) {
                    break;
                }
                summary.frames += 1;
                summary.samples += end.block_size as u64;
                summary.end_offset += end.len as u64;
                head += end.len;
                short_seen = end.block_size != BLOCK_SIZE;
            }
            Walk::Invalid => break,
            Walk::NeedMore => {
                if at_eof {
                    break;
                }
                if window.len() - head >= MAX_FRAME_BYTES {
                    break;
                }
                // Drop what was consumed, then read more.
                if head > 0 {
                    window.drain(..head);
                    head = 0;
                }
                let before = window.len();
                let target = (before + READ_CHUNK).min(MAX_FRAME_BYTES);
                window.resize(target, 0);
                let mut filled = before;
                while filled < target {
                    match reader.read(&mut window[filled..target]) {
                        Ok(0) => {
                            at_eof = true;
                            break;
                        }
                        Ok(n) => filled += n,
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(error) => return Err(error),
                    }
                }
                window.truncate(filled);
                if filled == before && at_eof {
                    // Nothing more: the walk above decides on what is left.
                    let last = walk_frame(&window[head..], stream, summary.frames);
                    if matches!(last, Walk::Frame(_)) {
                        continue;
                    }
                    break;
                }
            }
        }
    }
    Ok(summary)
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
