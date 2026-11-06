//! Decoding frames for the MD5 that STREAMINFO carries.
//!
//! FLAC's MD5 is taken over the decoded PCM, so recovery has to decode every
//! frame it keeps. `symphonia` (already a dependency, `flac` feature) decodes
//! one frame per packet; the MD5 itself is `flacenc`'s `Context`, the same
//! code that computed it while recording.

use flacenc::source::{Context, Fill};
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{CodecParameters, Decoder, DecoderOptions, CODEC_TYPE_FLAC};
use symphonia::core::formats::Packet;

use super::format::{StreamParams, STREAMINFO_LEN};
use crate::recording::error::RecordingError;

/// Decodes valid frames in order and keeps the MD5 of the PCM.
pub(crate) struct Md5Calculator {
    decoder: Box<dyn Decoder>,
    context: Context,
    channels: usize,
    /// Symphonia returns samples left-aligned in 32 bits.
    shift: u32,
    pcm: Vec<i32>,
}

impl Md5Calculator {
    pub(crate) fn new(
        streaminfo: &[u8; STREAMINFO_LEN],
        params: StreamParams,
    ) -> Result<Self, RecordingError> {
        let mut codec = CodecParameters::new();
        codec
            .for_codec(CODEC_TYPE_FLAC)
            .with_extra_data(Box::from(&streaminfo[..]));
        let decoder = symphonia::default::get_codecs()
            .make(&codec, &DecoderOptions::default())
            .map_err(RecordingError::encoder)?;
        Ok(Self {
            decoder,
            context: Context::new(usize::from(params.bits), usize::from(params.channels)),
            channels: usize::from(params.channels),
            shift: 32 - u32::from(params.bits),
            pcm: Vec::new(),
        })
    }

    /// Decode one whole frame (header to CRC-16) and add its samples to the
    /// MD5. `false` means the frame does not decode; nothing was added.
    pub(crate) fn add_frame(&mut self, frame: &[u8]) -> bool {
        let packet = Packet::new_from_slice(0, 0, 0, frame);
        let Ok(decoded) = self.decoder.decode(&packet) else {
            return false;
        };
        let AudioBufferRef::S32(buffer) = decoded else {
            return false;
        };
        if buffer.spec().channels.count() != self.channels {
            return false;
        }
        let frames = buffer.frames();
        self.pcm.clear();
        self.pcm.reserve(frames * self.channels);
        for index in 0..frames {
            for channel in 0..self.channels {
                self.pcm.push(buffer.chan(channel)[index] >> self.shift);
            }
        }
        self.context.fill_interleaved(&self.pcm).is_ok()
    }

    /// The MD5 of everything added and the sample frames it covers.
    pub(crate) fn finish(self) -> ([u8; 16], u64) {
        (
            self.context.md5_digest(),
            self.context.total_samples() as u64,
        )
    }
}
