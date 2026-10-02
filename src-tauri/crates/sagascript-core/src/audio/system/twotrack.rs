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

/// Incremental WAV writer; the header is patched in `finish`.
pub struct TwoTrackWriter {
    out: BufWriter<File>,
    channels: u16,
    frames: u64,
    path: PathBuf,
}

impl TwoTrackWriter {
    pub fn create(path: &Path, channels: u16) -> io::Result<Self> {
        let file = File::create(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        let mut out = BufWriter::new(file);
        write_header(&mut out, channels, 0)?;
        Ok(Self { out, channels, frames: 0, path: path.to_path_buf() })
    }

    /// Append interleaved samples (`channels` per frame).
    pub fn write_interleaved(&mut self, samples: &[f32]) -> io::Result<()> {
        debug_assert_eq!(samples.len() % self.channels as usize, 0);
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
        let data_bytes = self.frames * self.channels as u64 * 2;
        if data_bytes > u32::MAX as u64 - 36 {
            return Err(io::Error::other("recording exceeds the 4 GiB WAV limit"));
        }
        let mut file = self.out.into_inner().map_err(|e| e.into_error())?;
        file.seek(SeekFrom::Start(0))?;
        write_header(&mut file, self.channels, data_bytes as u32)?;
        file.flush()?;
        Ok(self.path)
    }
}

fn write_header(w: &mut impl Write, channels: u16, data_bytes: u32) -> io::Result<()> {
    let rate = TARGET_SAMPLE_RATE;
    w.write_all(b"RIFF")?;
    w.write_all(&(data_bytes.saturating_add(36)).to_le_bytes())?;
    w.write_all(b"WAVEfmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?;
    w.write_all(&channels.to_le_bytes())?;
    w.write_all(&rate.to_le_bytes())?;
    w.write_all(&(rate * channels as u32 * 2).to_le_bytes())?;
    w.write_all(&(channels * 2).to_le_bytes())?;
    w.write_all(&16u16.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_bytes.to_le_bytes())
}

/// Write a complete two-track file from in-memory tracks.
pub fn write_two_track_wav(path: &Path, mic: &[f32], system: &[f32]) -> io::Result<()> {
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
        for (c, s) in frame.chunks_exact(2).enumerate() {
            tracks[c].push(i16::from_le_bytes([s[0], s[1]]) as f32 / i16::MAX as f32);
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
        assert_eq!(u32::from_le_bytes(b[40..44].try_into().unwrap()), 1280);
        assert_eq!(b.len(), 44 + 1280);
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
}
