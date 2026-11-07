//! Reading a recording back as audio (SAGA-564, design §4.6).
//!
//! [`decode_tracks`] turns a finished recording folder into 16 kHz mono
//! chunks for transcription, one track after another, in bounded memory: the
//! iterator holds one FLAC packet, the resampler's state and at most one chunk
//! ([`CHUNK_SAMPLES`] samples) of output, however long the recording is.
//!
//! * **Timeline.** Chunks carry `start_sample`, an index on the recording's
//!   16 kHz timeline. The earliest track starts at 0; a track whose
//!   `clock.start_ns` is later begins with that much leading silence, emitted
//!   as zero chunks (nothing is allocated for the whole offset). Offsets are
//!   only compared between tracks of one clock domain.
//! * **Gaps** are already silence inside the stored FLAC, so decoding them
//!   needs no special case; what the iterator checks is the length: a track
//!   file that decodes to fewer frames than the manifest says is a typed
//!   [`RecordingError::CorruptTrack`], never a short transcript.
//! * Only a finished recording (`complete`, recovered or not) is read; one
//!   whose manifest still says `recording` is refused with
//!   [`RecordingError::RecoveryNeeded`] (recover it first).
//!
//! [`NativeTrackReader`] is the pull-based FLAC reader both this module and
//! the WAV export use.

use std::fs::File;
use std::path::{Path, PathBuf};

use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use super::error::RecordingError;
use super::manifest::{Manifest, RecordingState, Track};
use super::store::load_manifest;
use crate::audio::resample::{StreamResampleError, StreamingResampler, TARGET_SAMPLE_RATE};

/// The most samples (16 kHz, two seconds) one [`TrackChunk`] holds.
pub const CHUNK_SAMPLES: usize = 32_000;

/// Lowest stored sample rate decoded: the resampler's buffers grow with the
/// ratio, and nothing real is recorded below it.
const MIN_RATE: u32 = 1_000;

/// A piece of one track, 16 kHz mono.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackChunk {
    pub track_id: String,
    pub role: String,
    /// Index of the first sample on the recording's 16 kHz timeline.
    pub start_sample: u64,
    /// At most [`CHUNK_SAMPLES`] samples.
    pub samples: Vec<f32>,
}

/// A FLAC track file read packet by packet as interleaved `f32` in `[-1, 1)`.
pub(crate) struct NativeTrackReader {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    frames_read: u64,
}

impl NativeTrackReader {
    pub(crate) fn open(path: &Path) -> Result<Self, RecordingError> {
        let file =
            File::open(path).map_err(|error| RecordingError::io("open a track file", &error))?;
        let stream = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("flac");
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                stream,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(|_| RecordingError::CorruptTrack {
                reason: "the track file is not a FLAC stream",
            })?;
        let reader = probed.format;
        let track = reader
            .default_track()
            .ok_or(RecordingError::CorruptTrack {
                reason: "the track file has no audio",
            })?
            .clone();
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|_| RecordingError::CorruptTrack {
                reason: "the track file cannot be decoded",
            })?;
        Ok(Self {
            reader,
            decoder,
            track_id: track.id,
            frames_read: 0,
        })
    }

    /// Frames decoded so far.
    pub(crate) fn frames_read(&self) -> u64 {
        self.frames_read
    }

    /// Replace `out` with the next packet's interleaved samples; returns the
    /// channel count, or `None` at the end of the stream.
    pub(crate) fn next_block(
        &mut self,
        out: &mut Vec<f32>,
    ) -> Result<Option<usize>, RecordingError> {
        loop {
            let packet = match self.reader.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::IoError(error))
                    if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return Ok(None);
                }
                Err(SymphoniaError::ResetRequired) => return Ok(None),
                Err(_) => {
                    return Err(RecordingError::CorruptTrack {
                        reason: "a track file cannot be read to its end",
                    })
                }
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded =
                self.decoder
                    .decode(&packet)
                    .map_err(|_| RecordingError::CorruptTrack {
                        reason: "a frame of the track file does not decode",
                    })?;
            let AudioBufferRef::S32(buffer) = decoded else {
                return Err(RecordingError::CorruptTrack {
                    reason: "the track file does not decode to integers",
                });
            };
            let channels = buffer.spec().channels.count();
            let frames = buffer.frames();
            out.clear();
            out.reserve(frames * channels);
            // Samples are left-aligned in 32 bits.
            for frame in 0..frames {
                for channel in 0..channels {
                    out.push(buffer.chan(channel)[frame] as f32 / 2_147_483_648.0);
                }
            }
            self.frames_read += frames as u64;
            if frames == 0 {
                continue;
            }
            return Ok(Some(channels));
        }
    }
}

/// Mix interleaved `channels`-channel audio down to mono in `out` (cleared).
pub(crate) fn downmix_into(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    if channels <= 1 {
        out.extend_from_slice(interleaved);
    } else {
        out.extend(
            interleaved
                .chunks_exact(channels)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32),
        );
    }
}

/// The reading state of the audio of one track.
struct Audio {
    source: NativeTrackReader,
    resampler: StreamingResampler,
    interleaved: Vec<f32>,
    mono: Vec<f32>,
    eof: bool,
}

/// The track being yielded.
struct Cursor {
    track: Track,
    path: PathBuf,
    /// Leading silence not yet emitted, in 16 kHz samples.
    lead_remaining: u64,
    /// The next `start_sample`.
    next_sample: u64,
    audio: Option<Audio>,
    /// 16 kHz samples produced and not yet yielded; under a chunk plus one
    /// resampler output between calls.
    ready: Vec<f32>,
}

/// The chunks of a recording; see the module documentation.
pub struct TrackDecoder {
    manifest: Manifest,
    folder: PathBuf,
    /// Per track: leading silence in 16 kHz samples.
    leads: Vec<u64>,
    index: usize,
    current: Option<Cursor>,
    failed: bool,
}

/// Decode the tracks of the finished recording in `folder` to 16 kHz mono.
pub fn decode_tracks(folder: &Path) -> Result<TrackDecoder, RecordingError> {
    let manifest = load_manifest(folder)?;
    if manifest.state != RecordingState::Complete {
        return Err(RecordingError::RecoveryNeeded);
    }
    for track in &manifest.tracks {
        if track.sample_rate < MIN_RATE {
            return Err(RecordingError::UnsupportedFormat {
                reason: "a track's sample rate is too low to decode",
            });
        }
    }
    let leads = leading_silences(&manifest);
    Ok(TrackDecoder {
        manifest,
        folder: folder.to_path_buf(),
        leads,
        index: 0,
        current: None,
        failed: false,
    })
}

/// Each track's start relative to the earliest track of its clock domain, in
/// nanoseconds.
pub(crate) fn start_offsets_ns(manifest: &Manifest) -> Vec<u64> {
    manifest
        .tracks
        .iter()
        .map(|track| {
            let earliest = manifest
                .tracks
                .iter()
                .filter(|other| other.clock.domain == track.clock.domain)
                .map(|other| other.clock.start_ns)
                .min()
                .unwrap_or(track.clock.start_ns);
            track.clock.start_ns.saturating_sub(earliest)
        })
        .collect()
}

/// `offset_ns` as whole samples at `rate`.
pub(crate) fn samples_in(offset_ns: u64, rate: u32) -> u64 {
    (u128::from(offset_ns) * u128::from(rate) / 1_000_000_000) as u64
}

fn leading_silences(manifest: &Manifest) -> Vec<u64> {
    start_offsets_ns(manifest)
        .into_iter()
        .map(|offset| samples_in(offset, TARGET_SAMPLE_RATE))
        .collect()
}

impl TrackDecoder {
    /// The manifest the chunks are decoded under.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn open_audio(&self, cursor: &Cursor) -> Result<Audio, RecordingError> {
        Ok(Audio {
            source: NativeTrackReader::open(&cursor.path)?,
            resampler: StreamingResampler::new(cursor.track.sample_rate),
            interleaved: Vec::new(),
            mono: Vec::new(),
            eof: false,
        })
    }

    fn start_track(&mut self) -> Result<bool, RecordingError> {
        let Some(track) = self.manifest.tracks.get(self.index) else {
            return Ok(false);
        };
        self.current = Some(Cursor {
            path: self.folder.join(&track.file),
            track: track.clone(),
            lead_remaining: self.leads[self.index],
            next_sample: 0,
            audio: None,
            ready: Vec::new(),
        });
        Ok(true)
    }

    fn advance(&mut self) -> Result<Option<TrackChunk>, RecordingError> {
        loop {
            if self.current.is_none() && !self.start_track()? {
                return Ok(None);
            }
            let mut cursor = self.current.take().expect("a track is current");

            if cursor.lead_remaining > 0 {
                let take = cursor.lead_remaining.min(CHUNK_SAMPLES as u64);
                cursor.lead_remaining -= take;
                let chunk = cursor.chunk(vec![0.0; take as usize]);
                self.current = Some(cursor);
                return Ok(Some(chunk));
            }
            if cursor.audio.is_none() {
                cursor.audio = Some(self.open_audio(&cursor)?);
            }
            while cursor.ready.len() < CHUNK_SAMPLES
                && !cursor.audio.as_ref().is_some_and(|audio| audio.eof)
            {
                cursor.fill()?;
            }
            if cursor.ready.is_empty() {
                // This track is done; the next one starts from its own lead.
                self.index += 1;
                continue;
            }
            let take = cursor.ready.len().min(CHUNK_SAMPLES);
            let samples: Vec<f32> = cursor.ready.drain(..take).collect();
            let chunk = cursor.chunk(samples);
            self.current = Some(cursor);
            return Ok(Some(chunk));
        }
    }
}

impl Cursor {
    fn chunk(&mut self, samples: Vec<f32>) -> TrackChunk {
        let chunk = TrackChunk {
            track_id: self.track.id.clone(),
            role: self.track.role.clone(),
            start_sample: self.next_sample,
            samples,
        };
        self.next_sample += chunk.samples.len() as u64;
        chunk
    }

    /// Decode one packet into `ready`; at the end of the file check the length
    /// and flush the resampler.
    fn fill(&mut self) -> Result<(), RecordingError> {
        let audio = self.audio.as_mut().expect("audio is open");
        let ready = &mut self.ready;
        let mut emit = |chunk: &[f32]| -> Result<(), std::convert::Infallible> {
            ready.extend_from_slice(chunk);
            Ok(())
        };
        let resample_failed = |error: StreamResampleError<std::convert::Infallible>| match error {
            StreamResampleError::Resample(_) => RecordingError::CorruptTrack {
                reason: "a track could not be resampled",
            },
            StreamResampleError::Output(never) => match never {},
        };
        match audio.source.next_block(&mut audio.interleaved)? {
            Some(channels) => {
                downmix_into(&audio.interleaved, channels, &mut audio.mono);
                audio
                    .resampler
                    .push_with(&audio.mono, &mut emit)
                    .map_err(resample_failed)
            }
            None => {
                audio.eof = true;
                if audio.source.frames_read() < self.track.frames {
                    return Err(RecordingError::CorruptTrack {
                        reason: "the track file holds fewer frames than the manifest says",
                    });
                }
                audio
                    .resampler
                    .finish_with(&mut emit)
                    .map_err(resample_failed)
            }
        }
    }
}

impl Iterator for TrackDecoder {
    type Item = Result<TrackChunk, RecordingError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        match self.advance() {
            Ok(chunk) => chunk.map(Ok),
            Err(error) => {
                self.failed = true;
                Some(Err(error))
            }
        }
    }
}

#[cfg(test)]
#[path = "decode_tests.rs"]
mod tests;
