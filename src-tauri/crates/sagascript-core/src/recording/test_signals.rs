//! Signals and an independent decoder for the storage layer's tests (ADR 0004:
//! fixtures are synthetic). The decoder is `symphonia`'s FLAC reader, not the
//! code under test.

use std::io::Cursor;

use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

pub(crate) const MAX_24: i32 = (1 << 23) - 1;
pub(crate) const MIN_24: i32 = -(1 << 23);

/// A small deterministic generator (xorshift64*).
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `low..=high`.
    pub(crate) fn range(&mut self, low: i64, high: i64) -> i64 {
        low + (self.next_u64() % (high - low + 1) as u64) as i64
    }
}

/// The edge signals of the study (`gen-edge-signals.py`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Edge {
    Silence,
    DcMax,
    DcMin,
    SquareFullScale,
    WhiteFullScale,
    AlternatingExtremes,
    SingleImpulse,
}

pub(crate) const EDGES: [Edge; 7] = [
    Edge::Silence,
    Edge::DcMax,
    Edge::DcMin,
    Edge::SquareFullScale,
    Edge::WhiteFullScale,
    Edge::AlternatingExtremes,
    Edge::SingleImpulse,
];

/// 24-bit samples of an edge signal.
pub(crate) fn edge_signal(kind: Edge, n: usize) -> Vec<i32> {
    let mut rng = Rng::new(547);
    (0..n)
        .map(|i| match kind {
            Edge::Silence => 0,
            Edge::DcMax => MAX_24,
            Edge::DcMin => MIN_24,
            Edge::SquareFullScale => {
                if (i / 24) % 2 == 0 {
                    MAX_24
                } else {
                    MIN_24
                }
            }
            Edge::WhiteFullScale => rng.range(i64::from(MIN_24), i64::from(MAX_24)) as i32,
            Edge::AlternatingExtremes => {
                if i % 2 == 0 {
                    MAX_24
                } else {
                    MIN_24
                }
            }
            Edge::SingleImpulse => {
                if i == n / 2 {
                    MAX_24
                } else {
                    0
                }
            }
        })
        .collect()
}

/// A speech-like 24-bit signal: two tones and a low noise floor.
pub(crate) fn speechlike(n: usize, seed: u64) -> Vec<i32> {
    let mut rng = Rng::new(seed);
    (0..n)
        .map(|i| {
            let t = i as f64 / 48_000.0;
            let tone = 0.30 * (2.0 * std::f64::consts::PI * 220.0 * t).sin()
                + 0.18 * (2.0 * std::f64::consts::PI * 1371.0 * t).sin()
                + 0.05 * (2.0 * std::f64::consts::PI * 3150.0 * t).sin();
            let noise = rng.range(-2000, 2000) as f64;
            ((tone * f64::from(MAX_24)) + noise) as i32
        })
        .collect()
}

/// Reduce 24-bit values to the range of `bits` (values a 16-bit device would
/// deliver); 24 leaves them as they are.
pub(crate) fn at_bits(samples: &[i32], bits: u8) -> Vec<i32> {
    let shift = 24 - u32::from(bits);
    samples.iter().map(|s| s >> shift).collect()
}

/// A second channel derived from the first: delayed and halved.
pub(crate) fn second_channel(first: &[i32]) -> Vec<i32> {
    (0..first.len())
        .map(|i| if i >= 24 { first[i - 24] >> 1 } else { 0 })
        .collect()
}

pub(crate) fn interleave(channels: &[&[i32]]) -> Vec<i32> {
    let n = channels[0].len();
    let mut out = Vec::with_capacity(n * channels.len());
    for i in 0..n {
        for channel in channels {
            out.push(channel[i]);
        }
    }
    out
}

/// `n / 2^(bits-1)`: exact in `f32` for every value of `bits` bits.
pub(crate) fn to_f32(samples: &[i32], bits: u8) -> Vec<f32> {
    let scale = (1u32 << (bits - 1)) as f32;
    samples.iter().map(|s| *s as f32 / scale).collect()
}

/// The MD5 FLAC stores: of the PCM, little-endian, rounded up to whole bytes.
pub(crate) fn pcm_md5(interleaved: &[i32], bits: u8, channels: u8) -> [u8; 16] {
    use flacenc::source::{Context, Fill};
    let mut context = Context::new(usize::from(bits), usize::from(channels));
    for chunk in interleaved.chunks(4096 * usize::from(channels)) {
        context.fill_interleaved(chunk).unwrap();
    }
    context.md5_digest()
}

/// What an independent decoder made of a FLAC file.
#[derive(Debug)]
pub(crate) struct Decoded {
    pub sample_rate: u32,
    pub channels: usize,
    /// Interleaved, in the file's own bit depth.
    pub samples: Vec<i32>,
    /// `Some(true)` when STREAMINFO's MD5 was set and matched the decode.
    pub md5_verified: Option<bool>,
    /// Decode errors the reader skipped over (a clean file has none).
    pub errors: usize,
}

fn open(bytes: &[u8]) -> Box<dyn symphonia::core::formats::FormatReader> {
    let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("flac");
    symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .expect("symphonia probes the file as FLAC")
        .format
}

/// Decode a whole FLAC file with `symphonia`.
pub(crate) fn decode_flac(bytes: &[u8]) -> Decoded {
    let mut reader = open(bytes);
    let track = reader.default_track().expect("a track").clone();
    let bits = track.codec_params.bits_per_sample.expect("bit depth");
    let shift = 32 - bits;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions { verify: true })
        .expect("a FLAC decoder");
    let sample_rate = track.codec_params.sample_rate.expect("rate");
    let mut samples = Vec::new();
    let mut channels = 0;
    let mut errors = 0;
    loop {
        let packet = match reader.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(_)) => break,
            Err(SymphoniaError::DecodeError(_)) => {
                errors += 1;
                continue;
            }
            Err(error) => panic!("unexpected symphonia error: {error}"),
        };
        match decoder.decode(&packet) {
            Ok(AudioBufferRef::S32(buffer)) => {
                channels = buffer.spec().channels.count();
                for i in 0..buffer.frames() {
                    for c in 0..channels {
                        samples.push(buffer.chan(c)[i] >> shift);
                    }
                }
            }
            Ok(_) => panic!("FLAC decodes to S32"),
            Err(_) => errors += 1,
        }
    }
    Decoded {
        sample_rate,
        channels,
        samples,
        md5_verified: decoder.finalize().verify_ok,
        errors,
    }
}

/// Seek a FLAC file (through its SEEKTABLE) to `seconds` and decode from
/// there: the interleaved samples of the first packet read, and the sample
/// index the reader says it landed on.
pub(crate) fn seek_and_read(bytes: &[u8], seconds: f64) -> (u64, Vec<i32>) {
    let mut reader = open(bytes);
    let track = reader.default_track().expect("a track").clone();
    let bits = track.codec_params.bits_per_sample.expect("bit depth");
    let shift = 32 - bits;
    let landed = reader
        .seek(
            SeekMode::Coarse,
            SeekTo::Time {
                time: Time::from(seconds),
                track_id: Some(track.id),
            },
        )
        .expect("the reader seeks");
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .expect("a FLAC decoder");
    let packet = reader.next_packet().expect("a packet after the seek");
    let mut samples = Vec::new();
    if let AudioBufferRef::S32(buffer) = decoder.decode(&packet).expect("decodes") {
        for i in 0..buffer.frames() {
            for c in 0..buffer.spec().channels.count() {
                samples.push(buffer.chan(c)[i] >> shift);
            }
        }
    }
    (landed.actual_ts, samples)
}

// ---- recordings on disk -------------------------------------------------------

use std::path::{Path, PathBuf};

use super::manifest::{Client, Quality};
use super::store::{NewRecording, NewTrack};

pub(crate) const STARTED_AT: &str = "2026-11-05T14:02:11.482+01:00";

pub(crate) fn track(id: &str, sample_rate: u32, channels: u8, bits: u8) -> NewTrack {
    NewTrack {
        id: id.into(),
        role: id.into(),
        sample_rate,
        channels,
        bits,
        clock_domain: "mach_continuous".into(),
        start_ns: 1_000_000,
        resampled_from: None,
        device: Some("Test Microphone".into()),
    }
}

pub(crate) fn recording(title: &str, tracks: Vec<NewTrack>) -> NewRecording {
    NewRecording {
        title: title.into(),
        started_at: STARTED_AT.into(),
        client: Client {
            kind: "desktop".into(),
            app_version: "1.4.8".into(),
            build: "test".into(),
            extra: Default::default(),
        },
        quality: Quality::Lossless,
        tracks,
    }
}

/// Copy a folder of plain files.
pub(crate) fn copy_dir(from: &Path, to: &Path) -> PathBuf {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
    to.to_path_buf()
}

/// Every file of a folder and its bytes, sorted by name.
pub(crate) fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect();
    files.sort();
    files
}

pub(crate) fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}
