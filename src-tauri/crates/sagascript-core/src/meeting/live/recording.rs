//! Private two-track WAV spooling for live meetings.
//!
//! The spool keeps microphone samples on the left channel and system audio on
//! the right channel. The microphone input is recorded before any AEC or other
//! processing so an exported recording remains an honest capture of both
//! caller-supplied tracks. No destination file exists until [`Recording::export`].

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::fsutil::{self, PublishFs};
use crate::hash::StreamingSha256;

const SAMPLE_RATE: u32 = 16_000;
const CHANNELS: u16 = 2;
const BITS_PER_SAMPLE: u16 = 16;
const BYTES_PER_FRAME: u64 = 4;
const MAX_DURATION_SAMPLES: u64 = 4 * 60 * 60 * SAMPLE_RATE as u64;
const MAX_READ_FRAMES: usize = 30 * SAMPLE_RATE as usize;
const COPY_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub enum RecordingError {
    Io(io::Error),
    MismatchedChannels,
    NonFiniteSample,
    DurationExceeded,
    Finalized,
    NotFinalized,
    ReadStartOutOfBounds,
    ReadLimitExceeded,
    ReadTruncated,
    Publish(fsutil::PublishError),
}

impl RecordingError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "live_recording.io",
            Self::MismatchedChannels => "live_recording.mismatched_channels",
            Self::NonFiniteSample => "live_recording.non_finite_sample",
            Self::DurationExceeded => "live_recording.duration_exceeded",
            Self::Finalized => "live_recording.finalized",
            Self::NotFinalized => "live_recording.not_finalized",
            Self::ReadStartOutOfBounds => "live_recording.read_start_out_of_bounds",
            Self::ReadLimitExceeded => "live_recording.read_limit_exceeded",
            Self::ReadTruncated => "live_recording.read_truncated",
            Self::Publish(_) => "live_recording.publish",
        }
    }
}

impl std::fmt::Display for RecordingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for RecordingError {}

impl From<io::Error> for RecordingError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub struct Recording {
    file: File,
    duration_samples: u64,
    max_samples: u64,
    finalized: bool,
}

impl Recording {
    pub fn new() -> io::Result<Self> {
        Self::with_max_samples(MAX_DURATION_SAMPLES)
    }

    fn with_max_samples(max_samples: u64) -> io::Result<Self> {
        let mut file = tempfile::tempfile()?;
        write_wav_header(&mut file, 0)?;
        Ok(Self {
            file,
            duration_samples: 0,
            max_samples,
            finalized: false,
        })
    }

    pub fn append(&mut self, microphone: &[f32], system: &[f32]) -> Result<(), RecordingError> {
        if self.finalized {
            return Err(RecordingError::Finalized);
        }
        if microphone.len() != system.len() {
            return Err(RecordingError::MismatchedChannels);
        }
        if microphone
            .iter()
            .chain(system.iter())
            .any(|sample| !sample.is_finite())
        {
            return Err(RecordingError::NonFiniteSample);
        }
        let frames = microphone.len() as u64;
        let next_duration = self
            .duration_samples
            .checked_add(frames)
            .ok_or(RecordingError::DurationExceeded)?;
        if next_duration > self.max_samples {
            return Err(RecordingError::DurationExceeded);
        }

        let original_length = self.file.metadata()?.len();
        let write_result = write_frames(&mut self.file, microphone, system);
        if let Err(error) = write_result {
            let _ = self.file.set_len(original_length);
            let _ = self.file.seek(SeekFrom::End(0));
            return Err(error.into());
        }
        self.duration_samples = next_duration;
        Ok(())
    }

    pub fn duration_samples(&self) -> u64 {
        self.duration_samples
    }

    pub fn finalize(&mut self) -> Result<(), RecordingError> {
        if self.finalized {
            return Ok(());
        }
        let data_bytes = self
            .duration_samples
            .checked_mul(BYTES_PER_FRAME)
            .ok_or(RecordingError::DurationExceeded)?;
        let data_bytes = u32::try_from(data_bytes).map_err(|_| RecordingError::DurationExceeded)?;
        write_wav_header(&mut self.file, data_bytes)?;
        self.file.flush()?;
        self.file.sync_all()?;
        self.finalized = true;
        Ok(())
    }

    /// Read up to `max_frames` of the finalized stereo PCM payload.
    ///
    /// The returned vectors contain the microphone (left) and system (right)
    /// channels, normalized back to the `-1.0..=1.0` range used by `append`.
    /// Reads are bounded so a caller cannot request an unbounded allocation.
    pub fn read_frames(
        &mut self,
        start_sample: u64,
        max_frames: usize,
    ) -> Result<(Vec<f32>, Vec<f32>), RecordingError> {
        if !self.finalized {
            return Err(RecordingError::NotFinalized);
        }
        if max_frames > MAX_READ_FRAMES {
            return Err(RecordingError::ReadLimitExceeded);
        }
        if start_sample > self.duration_samples {
            return Err(RecordingError::ReadStartOutOfBounds);
        }

        let data_bytes = self
            .duration_samples
            .checked_mul(BYTES_PER_FRAME)
            .ok_or(RecordingError::ReadTruncated)?;
        let expected_len = 44u64
            .checked_add(data_bytes)
            .ok_or(RecordingError::ReadTruncated)?;
        if self.file.metadata()?.len() < expected_len {
            return Err(RecordingError::ReadTruncated);
        }

        let frames = (self.duration_samples - start_sample).min(max_frames as u64) as usize;
        let byte_offset = 44u64
            .checked_add(
                start_sample
                    .checked_mul(BYTES_PER_FRAME)
                    .ok_or(RecordingError::ReadTruncated)?,
            )
            .ok_or(RecordingError::ReadTruncated)?;
        let byte_len = frames
            .checked_mul(BYTES_PER_FRAME as usize)
            .ok_or(RecordingError::ReadTruncated)?;

        let result = (|| {
            self.file.seek(SeekFrom::Start(byte_offset))?;
            let mut bytes = vec![0u8; byte_len];
            match self.file.read_exact(&mut bytes) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    return Err(RecordingError::ReadTruncated);
                }
                Err(error) => return Err(RecordingError::Io(error)),
            }

            let mut microphone = Vec::with_capacity(frames);
            let mut system = Vec::with_capacity(frames);
            for frame in bytes.as_chunks::<4>().0 {
                let microphone_sample = i16::from_le_bytes([frame[0], frame[1]]);
                let system_sample = i16::from_le_bytes([frame[2], frame[3]]);
                microphone.push(microphone_sample as f32 / i16::MAX as f32);
                system.push(system_sample as f32 / i16::MAX as f32);
            }
            Ok((microphone, system))
        })();
        let restore_result = self.file.seek(SeekFrom::End(0));
        match (result, restore_result) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(RecordingError::Io(error)),
            (Ok(frames), Ok(_)) => Ok(frames),
        }
    }

    pub fn sha256(&mut self) -> Result<String, RecordingError> {
        if !self.finalized {
            return Err(RecordingError::NotFinalized);
        }
        self.file.seek(SeekFrom::Start(0))?;
        let mut hasher = StreamingSha256::new();
        let mut buffer = [0u8; COPY_BUFFER_BYTES];
        let digest = loop {
            let read = self.file.read(&mut buffer)?;
            if read == 0 {
                break hasher.finish_hex();
            }
            hasher.update(&buffer[..read]);
        };
        self.file.seek(SeekFrom::End(0))?;
        Ok(digest)
    }

    pub fn export(&mut self, destination: &Path) -> Result<fsutil::Published, RecordingError> {
        self.finalize()?;
        let staged = fsutil::StagedFile::reserve(destination).map_err(RecordingError::Publish)?;
        let write_result = self.write_staged(staged.staged_path());
        if let Err(error) = write_result {
            drop(staged);
            return Err(error);
        }
        staged.publish().map_err(RecordingError::Publish)
    }

    fn write_staged(&mut self, staged_path: &Path) -> Result<(), RecordingError> {
        let ops = fsutil::RealFs;
        let mut output = ops.create_new(staged_path)?;
        self.file.seek(SeekFrom::Start(0))?;
        let mut buffer = [0u8; COPY_BUFFER_BYTES];
        loop {
            let read = self.file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            output.write_all(&buffer[..read])?;
        }
        output.flush()?;
        output.sync_all()?;
        self.file.seek(SeekFrom::End(0))?;
        Ok(())
    }
}

fn write_frames(file: &mut File, microphone: &[f32], system: &[f32]) -> io::Result<()> {
    let mut buffer = [0u8; COPY_BUFFER_BYTES];
    let mut used = 0;
    for (&microphone, &system) in microphone.iter().zip(system) {
        buffer[used..used + 2].copy_from_slice(&pcm16(microphone).to_le_bytes());
        buffer[used + 2..used + 4].copy_from_slice(&pcm16(system).to_le_bytes());
        used += 4;
        if used == buffer.len() {
            file.write_all(&buffer)?;
            used = 0;
        }
    }
    if used > 0 {
        file.write_all(&buffer[..used])?;
    }
    Ok(())
}

fn pcm16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

fn write_wav_header(file: &mut File, data_bytes: u32) -> io::Result<()> {
    let riff_size = 36u32
        .checked_add(data_bytes)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "WAV too large"))?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(b"RIFF")?;
    file.write_all(&riff_size.to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&CHANNELS.to_le_bytes())?;
    file.write_all(&SAMPLE_RATE.to_le_bytes())?;
    let byte_rate = SAMPLE_RATE * u32::from(CHANNELS) * u32::from(BITS_PER_SAMPLE) / 8;
    file.write_all(&byte_rate.to_le_bytes())?;
    let block_align = CHANNELS * BITS_PER_SAMPLE / 8;
    file.write_all(&block_align.to_le_bytes())?;
    file.write_all(&BITS_PER_SAMPLE.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_bytes.to_le_bytes())?;
    file.seek(SeekFrom::End(0))?;
    Ok(())
}

#[cfg(test)]
#[path = "recording_tests.rs"]
mod recording_tests;
