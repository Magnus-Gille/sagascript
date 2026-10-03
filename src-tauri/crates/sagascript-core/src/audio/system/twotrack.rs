//! Two-track recording file: a 16 kHz, 16-bit PCM stereo WAV with the
//! microphone ("me") on the left channel and system audio ("the others") on the
//! right. Any stereo-capable decoder downmixes it for plain transcription; the
//! diarization path can split the channels (see `read_two_track_wav`).

use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::audio::resample::TARGET_SAMPLE_RATE;

fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

/// Interleave two tracks into stereo, zero-padding the shorter one.
pub fn interleave_padded(left: &[f32], right: &[f32]) -> Vec<f32> {
    let n = left.len().max(right.len());
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        out.push(left.get(i).copied().unwrap_or(0.0));
        out.push(right.get(i).copied().unwrap_or(0.0));
    }
    out
}

/// Leading padding (ms) for each track so both start on the same wall-clock
/// timeline, given when each track's sample 0 occurred (ms since a shared
/// origin). Returns `(mic_pad_ms, system_pad_ms)`; the earlier track gets none.
pub fn track_start_padding(mic_start_ms: u64, system_start_ms: u64) -> (u64, u64) {
    let origin = mic_start_ms.min(system_start_ms);
    (mic_start_ms - origin, system_start_ms - origin)
}

/// Prepend `pad_ms` of silence to a 16 kHz track.
pub fn pad_front(track: &[f32], pad_ms: u64) -> Vec<f32> {
    let pad = (pad_ms * TARGET_SAMPLE_RATE as u64 / 1000) as usize;
    let mut out = vec![0.0; pad];
    out.extend_from_slice(track);
    out
}

/// Mono mix of two tracks for transcription (average, so a lone track keeps
/// half level only where the other is silent; callers wanting full level on a
/// single source should not mix).
pub fn mix_tracks(a: &[f32], b: &[f32]) -> Vec<f32> {
    let n = a.len().max(b.len());
    (0..n)
        .map(|i| (a.get(i).copied().unwrap_or(0.0) + b.get(i).copied().unwrap_or(0.0)) * 0.5)
        .collect()
}

/// Largest data payload a RIFF/WAV header can describe.
const MAX_WAV_DATA_BYTES: u64 = u32::MAX as u64 - 36;

/// Fail (before anything is written) if `frames` of `channels` 16-bit audio do
/// not fit in a WAV file.
pub fn check_wav_capacity(frames: u64, channels: u16) -> io::Result<()> {
    let too_big = || io::Error::other("recording exceeds the 4 GiB WAV limit");
    let bytes = frames.checked_mul(channels as u64).and_then(|v| v.checked_mul(2)).ok_or_else(too_big)?;
    if bytes > MAX_WAV_DATA_BYTES {
        return Err(too_big());
    }
    Ok(())
}

/// Incremental WAV writer; the header is patched in `finish`.
pub struct TwoTrackWriter {
    out: BufWriter<File>,
    channels: u16,
    frames: u64,
    path: PathBuf,
}

impl TwoTrackWriter {
    pub fn create(path: &Path, channels: u16) -> io::Result<Self> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600); // never world-readable, not even briefly
        }
        let file = opts.open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?; // pre-existing file
        }
        let mut out = BufWriter::new(file);
        write_header(&mut out, channels, 0)?;
        Ok(Self { out, channels, frames: 0, path: path.to_path_buf() })
    }

    /// Append interleaved samples (`channels` per frame).
    pub fn write_interleaved(&mut self, samples: &[f32]) -> io::Result<()> {
        debug_assert_eq!(samples.len() % self.channels as usize, 0);
        // Reject before writing so an oversized recording never hits the disk.
        check_wav_capacity(self.frames + (samples.len() / self.channels as usize) as u64, self.channels)?;
        for s in samples {
            self.out.write_all(&to_i16(*s).to_le_bytes())?;
        }
        self.frames += (samples.len() / self.channels as usize) as u64;
        Ok(())
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn finish(mut self) -> io::Result<PathBuf> {
        self.out.flush()?;
        check_wav_capacity(self.frames, self.channels)?;
        let data_bytes = self.frames * self.channels as u64 * 2;
        let mut file = self.out.into_inner().map_err(|e| e.into_error())?;
        file.seek(SeekFrom::Start(0))?;
        write_header(&mut file, self.channels, data_bytes as u32)?;
        file.flush()?;
        Ok(self.path)
    }
}

/// Marker text stored in the WAV `LIST/INFO/ICMT` comment of a two-track file.
const MARKER_PREFIX: &str = "sagascript-two-track/1";

/// What a recorder says about the layout of a stereo WAV it wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoTrackMarker {
    /// Recorder release that wrote the file (e.g. `1.4.3`).
    pub recorder_version: String,
}

impl TwoTrackMarker {
    /// Parse the comment text; `None` for anything that is not a version-1
    /// marker with the fixed layout left = microphone, right = system.
    fn parse(comment: &str) -> Option<Self> {
        let mut words = comment.trim_end_matches('\0').split_whitespace();
        if words.next()? != MARKER_PREFIX {
            return None;
        }
        let (mut left, mut right, mut recorder) = (None, None, None);
        for word in words {
            match word.split_once('=')? {
                ("left", v) => left = Some(v),
                ("right", v) => right = Some(v),
                ("recorder", v) => recorder = Some(v),
                _ => {} // unknown keys are ignored: forward compatible
            }
        }
        (left? == "microphone" && right? == "system").then(|| Self {
            recorder_version: recorder.unwrap_or("unknown").to_string(),
        })
    }
}

/// `LIST` chunk (type `INFO`) carrying the software name and the marker. Ordinary
/// players and decoders skip `LIST`; ffmpeg and most taggers write the same chunk.
fn marker_chunk() -> Vec<u8> {
    fn sub(out: &mut Vec<u8>, id: &[u8; 4], text: &str) {
        let mut body = text.as_bytes().to_vec();
        body.push(0); // INFO strings are NUL-terminated
        out.extend_from_slice(id);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        if body.len() % 2 == 1 {
            out.push(0); // RIFF chunks are word aligned
        }
    }
    let version = env!("CARGO_PKG_VERSION");
    let mut info = b"INFO".to_vec();
    sub(&mut info, b"ISFT", &format!("Sagascript {version}"));
    sub(
        &mut info,
        b"ICMT",
        &format!("{MARKER_PREFIX} left=microphone right=system recorder={version}"),
    );
    let mut chunk = b"LIST".to_vec();
    chunk.extend_from_slice(&(info.len() as u32).to_le_bytes());
    chunk.extend_from_slice(&info);
    chunk
}

/// Header = `RIFF`, `fmt `, optional marker `LIST` (two-channel files only),
/// `data` chunk header. Its length does not depend on `data_bytes`, so
/// `finish` can rewrite it in place.
fn write_header(w: &mut impl Write, channels: u16, data_bytes: u32) -> io::Result<()> {
    let rate = TARGET_SAMPLE_RATE;
    let marker = if channels == 2 { marker_chunk() } else { Vec::new() };
    w.write_all(b"RIFF")?;
    let riff_size = 36u32.saturating_add(marker.len() as u32).saturating_add(data_bytes);
    w.write_all(&riff_size.to_le_bytes())?;
    w.write_all(b"WAVEfmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?;
    w.write_all(&channels.to_le_bytes())?;
    w.write_all(&rate.to_le_bytes())?;
    w.write_all(&(rate * channels as u32 * 2).to_le_bytes())?;
    w.write_all(&(channels * 2).to_le_bytes())?;
    w.write_all(&16u16.to_le_bytes())?;
    w.write_all(&marker)?;
    w.write_all(b"data")?;
    w.write_all(&data_bytes.to_le_bytes())
}

/// Read the two-track marker from the chunks in front of the `data` chunk.
/// `Ok(None)` for any WAV (or non-WAV) without a valid marker; only the first
/// few KiB are read, never the audio.
pub fn read_two_track_marker(path: &Path) -> io::Result<Option<TwoTrackMarker>> {
    let mut head = Vec::new();
    File::open(path)?.take(16 * 1024).read_to_end(&mut head)?;
    if head.len() < 12 || &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return Ok(None);
    }
    let mut pos = 12usize;
    while pos + 8 <= head.len() {
        let id = &head[pos..pos + 4];
        let size = u32::from_le_bytes(head[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        if id == b"data" {
            return Ok(None); // the marker must precede the audio
        }
        if id == b"LIST" && body + 4 <= head.len() && &head[body..body + 4] == b"INFO" {
            let end = (body + size).min(head.len());
            let mut sub = body + 4;
            while sub + 8 <= end {
                let sub_size = u32::from_le_bytes(head[sub + 4..sub + 8].try_into().unwrap()) as usize;
                let text_end = (sub + 8 + sub_size).min(end);
                if &head[sub..sub + 4] == b"ICMT" {
                    let text = String::from_utf8_lossy(&head[sub + 8..text_end]);
                    if let Some(marker) = TwoTrackMarker::parse(&text) {
                        return Ok(Some(marker));
                    }
                }
                sub += 8 + sub_size + (sub_size & 1);
            }
        }
        pos = body + size + (size & 1);
    }
    Ok(None)
}

/// Write a complete two-track file from in-memory tracks.
pub fn write_two_track_wav(path: &Path, mic: &[f32], system: &[f32]) -> io::Result<()> {
    check_wav_capacity(mic.len().max(system.len()) as u64, 2)?;
    let mut w = TwoTrackWriter::create(path, 2)?;
    w.write_interleaved(&interleave_padded(mic, system))?;
    w.finish().map(|_| ())
}

/// Read a 16-bit PCM WAV as separate channels (used by the diarization path to
/// separate "me" from "others"). Only the format Sagascript itself writes is
/// accepted.
pub fn read_two_track_wav(path: &Path) -> io::Result<Vec<Vec<f32>>> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad("not a WAV file"));
    }
    let (mut pos, mut channels, mut data) = (12usize, 0u16, None);
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        if id == b"fmt " && body + 16 <= bytes.len() {
            let fmt = u16::from_le_bytes(bytes[body..body + 2].try_into().unwrap());
            channels = u16::from_le_bytes(bytes[body + 2..body + 4].try_into().unwrap());
            let rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().unwrap());
            let bits = u16::from_le_bytes(bytes[body + 14..body + 16].try_into().unwrap());
            if fmt != 1 || bits != 16 || rate != TARGET_SAMPLE_RATE {
                return Err(bad("expected 16 kHz 16-bit PCM"));
            }
        } else if id == b"data" {
            let end = (body + size).min(bytes.len());
            data = Some(&bytes[body..end]);
            break;
        }
        pos = body + size + (size & 1);
    }
    let data = data.ok_or_else(|| bad("missing data chunk"))?;
    if channels == 0 {
        return Err(bad("missing fmt chunk"));
    }
    let mut tracks = vec![Vec::new(); channels as usize];
    for frame in data.chunks_exact(2 * channels as usize) {
        for (c, s) in frame.as_chunks::<2>().0.iter().enumerate() {
            tracks[c].push(i16::from_le_bytes(*s) as f32 / i16::MAX as f32);
        }
    }
    Ok(tracks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleave_pads_shorter_track() {
        assert_eq!(interleave_padded(&[1.0, 2.0], &[3.0]), vec![1.0, 3.0, 2.0, 0.0]);
        assert!(interleave_padded(&[], &[]).is_empty());
    }

    #[test]
    fn start_padding_delays_the_later_track() {
        assert_eq!(track_start_padding(120, 20), (100, 0));
        assert_eq!(track_start_padding(0, 50), (0, 50));
        assert_eq!(track_start_padding(7, 7), (0, 0));
        assert_eq!(pad_front(&[1.0], 1).len(), 17);
    }

    #[test]
    fn mix_averages_and_pads() {
        assert_eq!(mix_tracks(&[1.0, 1.0], &[0.0]), vec![0.5, 0.5]);
    }

    #[test]
    fn two_track_roundtrip_mic_left_system_right() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.wav");
        let mic: Vec<f32> = (0..1600).map(|i| if i % 2 == 0 { 0.5 } else { -0.5 }).collect();
        let system = vec![0.25f32; 800];
        write_two_track_wav(&path, &mic, &system).unwrap();
        let tracks = read_two_track_wav(&path).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].len(), 1600);
        assert_eq!(tracks[1].len(), 1600);
        assert!((tracks[0][0] - 0.5).abs() < 1e-3);
        assert!((tracks[1][10] - 0.25).abs() < 1e-3);
        assert_eq!(tracks[1][900], 0.0);
    }

    #[test]
    fn writer_header_is_patched_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w.wav");
        let mut w = TwoTrackWriter::create(&path, 2).unwrap();
        w.write_interleaved(&[0.0; 640]).unwrap();
        assert_eq!(w.frames(), 320);
        w.finish().unwrap();
        let b = std::fs::read(&path).unwrap();
        assert_eq!(&b[0..4], b"RIFF");
        let header = b.len() - 1280; // fmt + marker + data header
        assert_eq!(u32::from_le_bytes(b[header - 4..header].try_into().unwrap()), 1280);
        assert_eq!(&b[header - 8..header - 4], b"data");
        assert_eq!(u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize, b.len() - 8);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn rejects_non_wav() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.wav");
        std::fs::write(&path, b"nope").unwrap();
        assert!(read_two_track_wav(&path).is_err());
    }

    #[test]
    fn existing_decoder_downmixes_two_track_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.wav");
        write_two_track_wav(&path, &vec![0.5; 16_000], &vec![0.5; 16_000]).unwrap();
        let mono = crate::audio::decoder::decode_audio_file(&path).unwrap();
        assert!((mono.len() as i64 - 16_000).abs() < 200, "len {}", mono.len());
        assert!((mono[8000] - 0.5).abs() < 0.02);
    }

    #[test]
    fn wav_capacity_is_checked_before_writing() {
        assert!(check_wav_capacity(1_000, 2).is_ok());
        // 2 channels * 2 bytes per frame: just over / under the limit.
        let max_frames = MAX_WAV_DATA_BYTES / 4;
        assert!(check_wav_capacity(max_frames, 2).is_ok());
        assert!(check_wav_capacity(max_frames + 1, 2).is_err());
        assert!(check_wav_capacity(u64::MAX, 2).is_err()); // overflow, not wraparound
    }

    #[test]
    fn writer_rejects_an_oversized_append_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.wav");
        let mut w = TwoTrackWriter::create(&path, 2).unwrap();
        w.frames = MAX_WAV_DATA_BYTES / 4 - 1; // one frame of room left
        w.write_interleaved(&[0.1, 0.1]).unwrap(); // the last legal append succeeds
        assert_eq!(w.frames(), MAX_WAV_DATA_BYTES / 4);
        assert!(w.write_interleaved(&[0.1, 0.1]).is_err());
        assert_eq!(w.frames(), MAX_WAV_DATA_BYTES / 4, "rejected append must not count");
        drop(w); // BufWriter flushes only the header
        let len = std::fs::metadata(&path).unwrap().len();
        assert!(len > 44 + 4, "marker chunk makes the header longer than a bare one");
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[bytes.len() - 4 - 8..bytes.len() - 8 + 4 - 4], b"data", "only the one legal frame follows the header");
    }

    fn chunk_ids(bytes: &[u8]) -> Vec<[u8; 4]> {
        let (mut pos, mut ids) = (12, Vec::new());
        while pos + 8 <= bytes.len() {
            ids.push(bytes[pos..pos + 4].try_into().unwrap());
            let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
            pos += 8 + size + (size & 1);
        }
        ids
    }

    #[test]
    fn marker_round_trips_and_chunks_stay_standard() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.wav");
        write_two_track_wav(&path, &[0.1; 1600], &[0.2; 1600]).unwrap();
        let marker = read_two_track_marker(&path).unwrap().expect("marker present");
        assert_eq!(marker.recorder_version, env!("CARGO_PKG_VERSION"));
        // Ordinary readers: RIFF/WAVE, then fmt, LIST (skippable), data, in
        // order, with sizes that tile the file exactly.
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(chunk_ids(&bytes), vec![*b"fmt ", *b"LIST", *b"data"]);
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize, bytes.len() - 8);
        assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 2);
        // Existing reader still returns the audio.
        let tracks = read_two_track_wav(&path).unwrap();
        assert_eq!((tracks.len(), tracks[0].len()), (2, 1600));
        assert!((tracks[1][5] - 0.2).abs() < 1e-3);
    }

    #[test]
    fn marker_absent_for_plain_stereo_and_mono() {
        let dir = tempfile::tempdir().unwrap();
        let stereo = dir.path().join("s.wav");
        let mut raw = Vec::new();
        for _ in 0..100 {
            raw.extend_from_slice(&1000i16.to_le_bytes());
            raw.extend_from_slice(&(-1000i16).to_le_bytes());
        }
        // A bare 44-byte-header stereo WAV like any other tool writes.
        let mut bare = Vec::new();
        bare.extend_from_slice(b"RIFF");
        bare.extend_from_slice(&(36 + raw.len() as u32).to_le_bytes());
        bare.extend_from_slice(b"WAVEfmt ");
        bare.extend_from_slice(&16u32.to_le_bytes());
        bare.extend_from_slice(&1u16.to_le_bytes());
        bare.extend_from_slice(&2u16.to_le_bytes());
        bare.extend_from_slice(&16_000u32.to_le_bytes());
        bare.extend_from_slice(&64_000u32.to_le_bytes());
        bare.extend_from_slice(&4u16.to_le_bytes());
        bare.extend_from_slice(&16u16.to_le_bytes());
        bare.extend_from_slice(b"data");
        bare.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        bare.extend_from_slice(&raw);
        std::fs::write(&stereo, &bare).unwrap();
        assert_eq!(read_two_track_marker(&stereo).unwrap(), None);
        assert_eq!(read_two_track_wav(&stereo).unwrap().len(), 2);

        let mono = dir.path().join("mono.wav");
        let mut w = TwoTrackWriter::create(&mono, 1).unwrap();
        w.write_interleaved(&[0.0; 10]).unwrap();
        w.finish().unwrap();
        assert_eq!(read_two_track_marker(&mono).unwrap(), None);
        let text = dir.path().join("t.wav");
        std::fs::write(&text, b"not a wav").unwrap();
        assert_eq!(read_two_track_marker(&text).unwrap(), None);
    }

    #[test]
    fn marker_parse_requires_known_layout() {
        assert!(TwoTrackMarker::parse("sagascript-two-track/1 left=microphone right=system recorder=2.0.0").is_some());
        assert!(TwoTrackMarker::parse("sagascript-two-track/1 left=system right=microphone").is_none());
        assert!(TwoTrackMarker::parse("sagascript-two-track/2 left=microphone right=system").is_none());
        assert!(TwoTrackMarker::parse("something else").is_none());
        assert!(TwoTrackMarker::parse("sagascript-two-track/1 left=microphone right=system x=y").is_some());
    }
}
