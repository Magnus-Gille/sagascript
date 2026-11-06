//! Exporting a recording as ordinary audio files (SAGA-564, design §4.6).
//!
//! | Layout | Output | What it is |
//! |---|---|---|
//! | `tracks` | one file per track in a directory | the stored audio: FLAC is a **byte copy** of the track file; WAV is a streamed decode to 24-bit PCM at the native rate and channel count |
//! | `stereo` | one file | track 1 left, track 2 right (one track: the same signal on both sides, or its own two channels if it has two) |
//! | `mixdown` | one file, mono | the mean of the tracks |
//!
//! `stereo` and `mixdown` are **derived**: the tracks are aligned by their
//! clock offsets, mixed to mono per track, and, when the tracks were recorded
//! at different rates, resampled to the highest one. The [`ExportReport`] says
//! so, with a note a UI can show as it is.
//!
//! Nothing is ever replaced: every output is created exclusively and a name
//! that exists is an error (the tracks layout checks all names before it
//! writes any). An output that fails half way is removed. The output may not
//! be inside the recording folder. Memory is bounded: audio streams through
//! in blocks of [`BLOCK_FRAMES`] frames, never a whole track.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use rubato::{Resampler, SincFixedIn};

use super::decode::{downmix_into, samples_in, start_offsets_ns, NativeTrackReader};
use super::encoder::{quantise, TrackEncoder};
use super::error::RecordingError;
use super::flac::FlacEncoder;
use super::manifest::{Manifest, RecordingState, Track};
use super::store::load_manifest;
use crate::audio::resample::sinc_params;

/// Frames processed at a time.
pub const BLOCK_FRAMES: usize = 8192;
/// The resampler's input chunk.
const RESAMPLE_CHUNK: usize = 1024;
/// The lowest rate resampled (the buffers grow with the ratio).
const MIN_RATE: u32 = 1_000;
/// The most data a WAV file holds (the RIFF sizes are 32 bits).
const WAV_MAX_DATA_BYTES: u64 = u32::MAX as u64 - 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Flac,
    Wav,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Flac => "flac",
            Self::Wav => "wav",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportLayout {
    Tracks,
    Stereo,
    Mixdown,
}

#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub format: ExportFormat,
    pub layout: ExportLayout,
    /// A directory for [`ExportLayout::Tracks`] (created if missing), else
    /// the file to create.
    pub out: PathBuf,
}

/// What an export made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportReport {
    pub files: Vec<PathBuf>,
    /// The audio is not the stored audio (mixed down or resampled).
    pub derived: bool,
    /// The sample rate the tracks were resampled to, if any were.
    pub resampled_to: Option<u32>,
    /// A sentence to show the user when `derived`.
    pub note: Option<String>,
}

fn invalid(reason: &'static str) -> RecordingError {
    RecordingError::InvalidInput { reason }
}

fn already_exists() -> RecordingError {
    RecordingError::Io {
        action: "create an export file (it already exists)",
        kind: io::ErrorKind::AlreadyExists,
        os_code: None,
    }
}

fn create_error(error: &io::Error) -> RecordingError {
    if error.kind() == io::ErrorKind::AlreadyExists {
        already_exists()
    } else {
        RecordingError::io("create an export file", error)
    }
}

/// Export the finished recording in `folder`; see the module documentation.
pub fn export_recording(
    folder: &Path,
    options: &ExportOptions,
) -> Result<ExportReport, RecordingError> {
    let manifest = load_manifest(folder)?;
    if manifest.state != RecordingState::Complete {
        return Err(RecordingError::RecoveryNeeded);
    }
    if manifest.tracks.is_empty() {
        return Err(invalid("the recording has no tracks"));
    }
    ensure_outside(folder, &options.out)?;
    match options.layout {
        ExportLayout::Tracks => export_tracks(folder, &manifest, options),
        ExportLayout::Stereo | ExportLayout::Mixdown => export_derived(folder, &manifest, options),
    }
}

/// Refuse an output inside the recording folder (it would become part of the
/// recording), looking through symbolic links and `..`.
fn ensure_outside(folder: &Path, out: &Path) -> Result<(), RecordingError> {
    let folder = fs::canonicalize(folder)
        .map_err(|error| RecordingError::io("open the recording folder", &error))?;
    let mut probe = if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out.to_path_buf()
    };
    let mut tail = PathBuf::new();
    let resolved = loop {
        if let Ok(real) = fs::canonicalize(&probe) {
            break real.join(&tail);
        }
        let Some(name) = probe.file_name().map(|name| name.to_owned()) else {
            return Err(invalid("the export path cannot be resolved"));
        };
        tail = Path::new(&name).join(&tail);
        probe = match probe.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        };
    };
    if resolved.components().any(|c| c == Component::ParentDir) {
        return Err(invalid("the export path cannot be resolved"));
    }
    if resolved.starts_with(&folder) {
        return Err(invalid(
            "the export must not be written inside the recording folder",
        ));
    }
    Ok(())
}

/// Created files, removed again unless [`Created::keep`] is called.
struct Created {
    paths: Vec<PathBuf>,
    keep: bool,
}

impl Created {
    fn new() -> Self {
        Self {
            paths: Vec::new(),
            keep: false,
        }
    }

    fn create(&mut self, path: &Path) -> Result<File, RecordingError> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| create_error(&error))?;
        self.paths.push(path.to_path_buf());
        Ok(file)
    }

    fn keep(mut self) -> Vec<PathBuf> {
        self.keep = true;
        std::mem::take(&mut self.paths)
    }
}

impl Drop for Created {
    fn drop(&mut self) {
        if !self.keep {
            for path in &self.paths {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn export_tracks(
    folder: &Path,
    manifest: &Manifest,
    options: &ExportOptions,
) -> Result<ExportReport, RecordingError> {
    if options.out.exists() && !options.out.is_dir() {
        return Err(invalid("the export folder is a file"));
    }
    let destinations: Vec<PathBuf> = manifest
        .tracks
        .iter()
        .map(|track| {
            options
                .out
                .join(format!("{}.{}", track.id, options.format.extension()))
        })
        .collect();
    // All names before any write: an export is whole or it is not made.
    if destinations.iter().any(|path| path.exists()) {
        return Err(already_exists());
    }
    fs::create_dir_all(&options.out)
        .map_err(|error| RecordingError::io("create the export folder", &error))?;
    let mut created = Created::new();
    for (track, destination) in manifest.tracks.iter().zip(&destinations) {
        let source = folder.join(&track.file);
        let file = created.create(destination)?;
        match options.format {
            ExportFormat::Flac => copy_track(&source, file)?,
            ExportFormat::Wav => wav_track(&source, file, track)?,
        }
    }
    Ok(ExportReport {
        files: created.keep(),
        derived: false,
        resampled_to: None,
        note: None,
    })
}

/// A byte copy of a stored FLAC track.
fn copy_track(source: &Path, mut file: File) -> Result<(), RecordingError> {
    let mut input =
        File::open(source).map_err(|error| RecordingError::io("open a track file", &error))?;
    io::copy(&mut input, &mut file)
        .map_err(|error| RecordingError::io("write an export file", &error))?;
    file.sync_all()
        .map_err(|error| RecordingError::io("write an export file", &error))
}

/// A streamed decode of a stored track to a 24-bit WAV at its own rate.
fn wav_track(source: &Path, file: File, track: &Track) -> Result<(), RecordingError> {
    let mut reader = NativeTrackReader::open(source)?;
    let mut wav = WavWriter::new(file, track.sample_rate, u16::from(track.channels))?;
    let mut block = Vec::new();
    while let Some(channels) = reader.next_block(&mut block)? {
        if channels != usize::from(track.channels) {
            return Err(RecordingError::CorruptTrack {
                reason: "the track file does not match the manifest",
            });
        }
        wav.write(&block)?;
    }
    if reader.frames_read() < track.frames {
        return Err(RecordingError::CorruptTrack {
            reason: "the track file holds fewer frames than the manifest says",
        });
    }
    wav.finish()
}

// ---- WAV ---------------------------------------------------------------------

/// A 24-bit PCM WAV written as it is produced: the header goes out with
/// placeholder sizes and is patched at the end.
struct WavWriter {
    out: BufWriter<File>,
    rate: u32,
    channels: u16,
    data_bytes: u64,
}

impl WavWriter {
    fn new(file: File, rate: u32, channels: u16) -> Result<Self, RecordingError> {
        let mut writer = Self {
            out: BufWriter::new(file),
            rate,
            channels,
            data_bytes: 0,
        };
        let header = writer.header();
        writer.put(&header)?;
        Ok(writer)
    }

    fn put(&mut self, bytes: &[u8]) -> Result<(), RecordingError> {
        self.out
            .write_all(bytes)
            .map_err(|error| RecordingError::io("write an export file", &error))
    }

    fn header(&self) -> [u8; 44] {
        let bytes_per_frame = u32::from(self.channels) * 3;
        let pad = self.data_bytes % 2;
        let mut header = [0u8; 44];
        header[0..4].copy_from_slice(b"RIFF");
        header[4..8].copy_from_slice(&((36 + self.data_bytes + pad) as u32).to_le_bytes());
        header[8..16].copy_from_slice(b"WAVEfmt ");
        header[16..20].copy_from_slice(&16u32.to_le_bytes());
        header[20..22].copy_from_slice(&1u16.to_le_bytes());
        header[22..24].copy_from_slice(&self.channels.to_le_bytes());
        header[24..28].copy_from_slice(&self.rate.to_le_bytes());
        header[28..32].copy_from_slice(&(self.rate * bytes_per_frame).to_le_bytes());
        header[32..34].copy_from_slice(&(bytes_per_frame as u16).to_le_bytes());
        header[34..36].copy_from_slice(&24u16.to_le_bytes());
        header[36..40].copy_from_slice(b"data");
        header[40..44].copy_from_slice(&(self.data_bytes as u32).to_le_bytes());
        header
    }

    /// Append interleaved samples.
    fn write(&mut self, interleaved: &[f32]) -> Result<(), RecordingError> {
        if self.data_bytes + interleaved.len() as u64 * 3 > WAV_MAX_DATA_BYTES {
            return Err(RecordingError::UnsupportedFormat {
                reason: "the recording is too long for one WAV file",
            });
        }
        let mut bytes = Vec::with_capacity(interleaved.len() * 3);
        for sample in interleaved {
            let (value, _) = quantise(*sample, 24);
            bytes.extend_from_slice(&value.to_le_bytes()[..3]);
        }
        self.put(&bytes)?;
        self.data_bytes += bytes.len() as u64;
        Ok(())
    }

    fn finish(mut self) -> Result<(), RecordingError> {
        if self.data_bytes % 2 == 1 {
            self.put(&[0])?;
        }
        let header = self.header();
        let io_error = |error: io::Error| RecordingError::io("write an export file", &error);
        self.out.flush().map_err(io_error)?;
        let mut file = self
            .out
            .into_inner()
            .map_err(|e| io_error(e.into_error()))?;
        file.seek(SeekFrom::Start(0)).map_err(io_error)?;
        file.write_all(&header).map_err(io_error)?;
        file.sync_all().map_err(io_error)
    }
}

// ---- derived layouts ------------------------------------------------------------

/// Where derived audio goes.
enum Sink {
    Wav(WavWriter),
    Flac(Box<FlacEncoder<File>>),
}

impl Sink {
    fn write(&mut self, interleaved: &[f32]) -> Result<(), RecordingError> {
        match self {
            Self::Wav(wav) => wav.write(interleaved),
            Self::Flac(flac) => flac.push(interleaved),
        }
    }

    fn finish(self) -> Result<(), RecordingError> {
        match self {
            Self::Wav(wav) => wav.finish(),
            Self::Flac(flac) => {
                let parts = flac.finish_into_parts()?;
                parts
                    .sink
                    .sync_all()
                    .map_err(|error| RecordingError::io("write an export file", &error))
            }
        }
    }
}

/// A track as a stream of interleaved frames at the export rate: leading
/// silence for its clock offset, then the decoded (and resampled) audio.
struct TrackStream {
    reader: NativeTrackReader,
    channels: usize,
    expected_frames: u64,
    converter: Option<RateConverter>,
    lead: u64,
    ready: Vec<f32>,
    position: usize,
    block: Vec<f32>,
    eof: bool,
}

impl TrackStream {
    fn open(
        folder: &Path,
        track: &Track,
        target_rate: u32,
        offset_ns: u64,
    ) -> Result<Self, RecordingError> {
        let channels = usize::from(track.channels);
        let converter = (track.sample_rate != target_rate)
            .then(|| RateConverter::new(track.sample_rate, target_rate, channels))
            .transpose()?;
        Ok(Self {
            reader: NativeTrackReader::open(&folder.join(&track.file))?,
            channels,
            expected_frames: track.frames,
            converter,
            lead: samples_in(offset_ns, target_rate),
            ready: Vec::new(),
            position: 0,
            block: Vec::new(),
            eof: false,
        })
    }

    fn buffered_frames(&self) -> usize {
        (self.ready.len() - self.position) / self.channels
    }

    /// Replace `out` with up to `frames` frames; fewer only at the end.
    fn pull(&mut self, frames: usize, out: &mut Vec<f32>) -> Result<(), RecordingError> {
        out.clear();
        let silence = self.lead.min(frames as u64) as usize;
        self.lead -= silence as u64;
        out.resize(silence * self.channels, 0.0);
        let wanted = frames - silence;
        while self.buffered_frames() < wanted && !self.eof {
            self.fill()?;
        }
        let take = self.buffered_frames().min(wanted);
        out.extend_from_slice(&self.ready[self.position..self.position + take * self.channels]);
        self.position += take * self.channels;
        if self.position == self.ready.len() {
            self.ready.clear();
            self.position = 0;
        }
        Ok(())
    }

    fn fill(&mut self) -> Result<(), RecordingError> {
        if self.position > 0 {
            self.ready.drain(..self.position);
            self.position = 0;
        }
        match self.reader.next_block(&mut self.block)? {
            Some(channels) => {
                if channels != self.channels {
                    return Err(RecordingError::CorruptTrack {
                        reason: "the track file does not match the manifest",
                    });
                }
                match self.converter.as_mut() {
                    Some(converter) => converter.push(&self.block, &mut self.ready),
                    None => {
                        self.ready.extend_from_slice(&self.block);
                        Ok(())
                    }
                }
            }
            None => {
                self.eof = true;
                if self.reader.frames_read() < self.expected_frames {
                    return Err(RecordingError::CorruptTrack {
                        reason: "the track file holds fewer frames than the manifest says",
                    });
                }
                match self.converter.as_mut() {
                    Some(converter) => converter.finish(&mut self.ready),
                    None => Ok(()),
                }
            }
        }
    }
}

fn export_derived(
    folder: &Path,
    manifest: &Manifest,
    options: &ExportOptions,
) -> Result<ExportReport, RecordingError> {
    let mixdown = options.layout == ExportLayout::Mixdown;
    let used: Vec<usize> = if mixdown {
        (0..manifest.tracks.len()).collect()
    } else {
        (0..manifest.tracks.len().min(2)).collect()
    };
    if used
        .iter()
        .any(|&i| manifest.tracks[i].sample_rate < MIN_RATE)
    {
        return Err(RecordingError::UnsupportedFormat {
            reason: "a track's sample rate is too low to export",
        });
    }
    let target_rate = used
        .iter()
        .map(|&i| manifest.tracks[i].sample_rate)
        .max()
        .expect("at least one track");
    let mut from_rates: Vec<u32> = used
        .iter()
        .map(|&i| manifest.tracks[i].sample_rate)
        .filter(|rate| *rate != target_rate)
        .collect();
    from_rates.sort_unstable();
    from_rates.dedup();

    let offsets = start_offsets_ns(manifest);
    let mut streams = Vec::with_capacity(used.len());
    for &i in &used {
        streams.push(TrackStream::open(
            folder,
            &manifest.tracks[i],
            target_rate,
            offsets[i],
        )?);
    }
    let out_channels: u8 = if mixdown { 1 } else { 2 };

    if let Some(parent) = options.out.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)
            .map_err(|error| RecordingError::io("create the export folder", &error))?;
    }
    let mut created = Created::new();
    let file = created.create(&options.out)?;
    let mut sink = match options.format {
        ExportFormat::Wav => Sink::Wav(WavWriter::new(file, target_rate, u16::from(out_channels))?),
        ExportFormat::Flac => Sink::Flac(Box::new(FlacEncoder::new(
            file,
            target_rate,
            out_channels,
            24,
        )?)),
    };

    let mut pulled: Vec<Vec<f32>> = vec![Vec::new(); streams.len()];
    let mut mono: Vec<Vec<f32>> = vec![Vec::new(); streams.len()];
    let mut mixed: Vec<f32> = Vec::new();
    loop {
        for (stream, buffer) in streams.iter_mut().zip(&mut pulled) {
            stream.pull(BLOCK_FRAMES, buffer)?;
        }
        let frames = pulled
            .iter()
            .zip(&streams)
            .map(|(buffer, stream)| buffer.len() / stream.channels)
            .max()
            .unwrap_or(0);
        if frames == 0 {
            break;
        }
        for ((buffer, stream), mono) in pulled.iter().zip(&streams).zip(&mut mono) {
            downmix_into(buffer, stream.channels, mono);
            mono.resize(frames, 0.0);
        }
        mixed.clear();
        if mixdown {
            let count = mono.len() as f32;
            mixed.extend((0..frames).map(|f| mono.iter().map(|m| m[f]).sum::<f32>() / count));
        } else if streams.len() >= 2 {
            for (left, right) in mono[0].iter().zip(&mono[1]) {
                mixed.push(*left);
                mixed.push(*right);
            }
        } else if streams[0].channels == 2 {
            mixed.extend_from_slice(&pulled[0]);
            mixed.resize(frames * 2, 0.0);
        } else {
            for sample in &mono[0] {
                mixed.push(*sample);
                mixed.push(*sample);
            }
        }
        sink.write(&mixed)?;
    }
    sink.finish()?;

    let resampled_to = (!from_rates.is_empty()).then_some(target_rate);
    let mut note = match (&resampled_to, from_rates.as_slice()) {
        (Some(to), from) => {
            let from: Vec<String> = from.iter().map(|rate| format!("{rate} Hz")).collect();
            format!(
                "derived and resampled from {} to {to} Hz",
                from.join(" and ")
            )
        }
        (None, _) => "derived from the stored tracks".to_string(),
    };
    if mixdown && used.len() > 1 {
        note.push_str("; tracks were summed");
    }
    if !mixdown && manifest.tracks.len() > 2 {
        note.push_str("; only the first two tracks are included");
    }
    Ok(ExportReport {
        files: created.keep(),
        derived: true,
        resampled_to,
        note: Some(note),
    })
}

// ---- resampling to a rate of choice ---------------------------------------------

/// A streaming sinc resampler between two arbitrary rates, the length trimmed
/// to `ceil(input * ratio)`. rubato's windows start centred on the first input
/// sample, so the output is in phase with the input with no delay to remove
/// (the test checks it against an ideal sine).
struct RateConverter {
    resampler: SincFixedIn<f32>,
    channels: usize,
    ratio: f64,
    /// Planar input waiting for a full chunk.
    pending: Vec<Vec<f32>>,
    input_frames: u64,
    output_frames: u64,
}

impl RateConverter {
    fn new(from: u32, to: u32, channels: usize) -> Result<Self, RecordingError> {
        let ratio = f64::from(to) / f64::from(from);
        let resampler =
            SincFixedIn::<f32>::new(ratio, 2.0, sinc_params(), RESAMPLE_CHUNK, channels).map_err(
                |_| RecordingError::CorruptTrack {
                    reason: "a track could not be resampled",
                },
            )?;
        Ok(Self {
            resampler,
            channels,
            ratio,
            pending: vec![Vec::new(); channels],
            input_frames: 0,
            output_frames: 0,
        })
    }

    fn failed<E>(_: E) -> RecordingError {
        RecordingError::CorruptTrack {
            reason: "a track could not be resampled",
        }
    }

    /// Append the interleaved output that `interleaved` completes to `out`.
    fn push(&mut self, interleaved: &[f32], out: &mut Vec<f32>) -> Result<(), RecordingError> {
        for frame in interleaved.chunks_exact(self.channels) {
            for (channel, sample) in self.pending.iter_mut().zip(frame) {
                channel.push(*sample);
            }
        }
        self.input_frames += (interleaved.len() / self.channels) as u64;
        while self.pending[0].len() >= RESAMPLE_CHUNK {
            let produced = {
                let chunk: Vec<&[f32]> =
                    self.pending.iter().map(|p| &p[..RESAMPLE_CHUNK]).collect();
                self.resampler.process(&chunk, None).map_err(Self::failed)?
            };
            for channel in &mut self.pending {
                channel.drain(..RESAMPLE_CHUNK);
            }
            self.append(&produced, out);
        }
        Ok(())
    }

    /// Flush the tail of the signal.
    fn finish(&mut self, out: &mut Vec<f32>) -> Result<(), RecordingError> {
        let remainder = std::mem::replace(&mut self.pending, vec![Vec::new(); self.channels]);
        let mut produced = if remainder[0].is_empty() {
            vec![Vec::new(); self.channels]
        } else {
            self.resampler
                .process_partial(Some(&remainder), None)
                .map_err(Self::failed)?
        };
        // One more chunk of silence pushes out the filter's tail.
        let tail = self
            .resampler
            .process_partial(None::<&[Vec<f32>]>, None)
            .map_err(Self::failed)?;
        for (channel, extra) in produced.iter_mut().zip(tail) {
            channel.extend(extra);
        }
        let before = self.output_frames;
        let mut tail_out = Vec::new();
        self.append(&produced, &mut tail_out);
        let expected = (self.input_frames as f64 * self.ratio).ceil() as u64;
        let allowed = expected.saturating_sub(before) as usize;
        tail_out.truncate(allowed * self.channels);
        out.extend_from_slice(&tail_out);
        Ok(())
    }

    fn append(&mut self, planar: &[Vec<f32>], out: &mut Vec<f32>) {
        let frames = planar.first().map_or(0, Vec::len);
        for f in 0..frames {
            for channel in planar {
                out.push(channel[f]);
            }
        }
        self.output_frames += frames as u64;
    }
}

#[cfg(test)]
#[path = "export_tests.rs"]
mod tests;
