//! Explicit, source-bound, read-only audio attachment. Never copies recordings.
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::SystemTime,
};

pub const MAX_AUDIO_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_RANGE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("The selected audio does not match this review.")]
    SourceMismatch,
    #[error("Select a nonempty regular audio file no larger than 2 GiB.")]
    InvalidFile,
    #[error("The selected audio container is unsupported.")]
    Unsupported,
    #[error("The recording changed; attach it again before playback.")]
    Changed,
    #[error("The requested audio range is invalid or too large.")]
    InvalidRange,
    #[error("Audio file I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

pub struct MeetingAudio {
    file: File,
    length: u64,
    modified: SystemTime,
    mime: &'static str,
    invalidated: bool,
}

fn read_digest_and_header<R: Read>(reader: &mut R) -> Result<(Vec<u8>, String, u64), MediaError> {
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut header = Vec::with_capacity(64);
    let mut total = 0u64;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_AUDIO_BYTES {
            return Err(MediaError::Changed);
        }
        let header_remaining = 64usize.saturating_sub(header.len());
        header.extend_from_slice(&buffer[..count.min(header_remaining)]);
        digest.update(&buffer[..count]);
    }
    Ok((header, format!("{:x}", digest.finalize()), total))
}

impl MeetingAudio {
    pub fn open(path: &Path, source_sha256: &str) -> Result<Self, MediaError> {
        if source_sha256.len() != 64
            || !source_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(MediaError::SourceMismatch);
        }
        let before_open = std::fs::metadata(path)?;
        if !before_open.is_file() || before_open.len() == 0 || before_open.len() > MAX_AUDIO_BYTES {
            return Err(MediaError::InvalidFile);
        }
        let mut file = File::open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_AUDIO_BYTES {
            return Err(MediaError::InvalidFile);
        }
        let modified = metadata.modified()?;
        let length = metadata.len();
        let (header, digest, total) = read_digest_and_header(&mut file)?;
        let after = file.metadata()?;
        if total != length || after.len() != length || after.modified()? != modified {
            return Err(MediaError::Changed);
        }
        if digest != source_sha256 {
            return Err(MediaError::SourceMismatch);
        }
        let mime = if header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WAVE") {
            "audio/wav"
        } else if header.starts_with(b"fLaC") {
            "audio/flac"
        } else if header.starts_with(b"OggS") {
            "audio/ogg"
        } else if header.get(4..8) == Some(b"ftyp") {
            "audio/mp4"
        } else if header.starts_with(b"ID3")
            || (header.len() >= 2 && header[0] == 0xff && header[1] & 0xe0 == 0xe0)
        {
            "audio/mpeg"
        } else {
            return Err(MediaError::Unsupported);
        };
        Ok(Self {
            file,
            length,
            modified,
            mime,
            invalidated: false,
        })
    }
    pub fn len(&self) -> u64 {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
    pub fn mime(&self) -> &'static str {
        self.mime
    }
    pub fn check_unchanged(&mut self) -> Result<(), MediaError> {
        if self.invalidated {
            return Err(MediaError::Changed);
        }
        // The descriptor prevents path replacement from redirecting playback.
        // Metadata detects ordinary in-place edits, not adversarial edits that
        // preserve size and restore mtime. No claim of an immutable snapshot.
        let result = self
            .file
            .metadata()
            .and_then(|meta| Ok(meta.len() == self.length && meta.modified()? == self.modified));
        match result {
            Ok(true) => Ok(()),
            Ok(false) => {
                self.invalidated = true;
                Err(MediaError::Changed)
            }
            Err(error) => {
                self.invalidated = true;
                Err(MediaError::Io(error))
            }
        }
    }
    pub fn read_range(&mut self, start: u64, length: u64) -> Result<Vec<u8>, MediaError> {
        if length == 0
            || length > MAX_RANGE_BYTES
            || start
                .checked_add(length)
                .is_none_or(|end| end > self.length)
        {
            return Err(MediaError::InvalidRange);
        }
        self.check_unchanged()?;
        let mut bytes = vec![0; length as usize];
        if let Err(error) = self
            .file
            .seek(SeekFrom::Start(start))
            .and_then(|_| self.file.read_exact(&mut bytes))
        {
            self.invalidated = true;
            return Err(MediaError::Io(error));
        }
        self.check_unchanged()?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct ShortReader {
        bytes: Vec<u8>,
        offset: usize,
    }

    impl Read for ShortReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.offset == self.bytes.len() || buffer.is_empty() {
                return Ok(0);
            }
            buffer[0] = self.bytes[self.offset];
            self.offset += 1;
            Ok(1)
        }
    }

    struct TestFile {
        path: std::path::PathBuf,
        file: File,
    }
    impl TestFile {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("sagascript-media-test-{}", uuid::Uuid::new_v4()));
            let file = File::options()
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            Self { path, file }
        }
        fn path(&self) -> &Path {
            &self.path
        }
    }
    impl std::ops::Deref for TestFile {
        type Target = File;
        fn deref(&self) -> &File {
            &self.file
        }
    }
    impl std::ops::DerefMut for TestFile {
        fn deref_mut(&mut self) -> &mut File {
            &mut self.file
        }
    }
    impl Drop for TestFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn fixture() -> (TestFile, Vec<u8>, String) {
        let mut file = TestFile::new();
        let bytes = b"RIFF\x24\0\0\0WAVEfmt \x10\0\0\0test audio contents".to_vec();
        file.write_all(&bytes).unwrap();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        (file, bytes, hash)
    }
    #[test]
    fn short_reads_accumulate_header_before_format_detection() {
        let bytes = b"RIFF\x24\0\0\0WAVEfmt \x10\0\0\0test audio contents".to_vec();
        let mut reader = ShortReader {
            bytes: bytes.clone(),
            offset: 0,
        };
        let (header, digest, total) = read_digest_and_header(&mut reader).unwrap();
        assert_eq!(header, bytes);
        assert_eq!(total, bytes.len() as u64);
        assert_eq!(digest, format!("{:x}", Sha256::digest(&bytes)));
        assert!(header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WAVE"));
    }
    #[test]
    fn source_bound_ranges_are_exact_and_bounded() {
        let (file, bytes, hash) = fixture();
        let mut audio = MeetingAudio::open(file.path(), &hash).unwrap();
        assert_eq!(audio.len(), bytes.len() as u64);
        assert_eq!(audio.mime(), "audio/wav");
        assert_eq!(audio.read_range(2, 9).unwrap(), bytes[2..11]);
        for (start, length) in [
            (0, 0),
            (0, MAX_RANGE_BYTES + 1),
            (audio.len(), 1),
            (u64::MAX, 2),
        ] {
            assert!(audio.read_range(start, length).is_err());
        }
    }
    #[test]
    fn rejects_wrong_source_empty_and_non_audio() {
        let (file, _, _) = fixture();
        assert!(MeetingAudio::open(file.path(), &"0".repeat(64)).is_err());
        let empty = TestFile::new();
        assert!(MeetingAudio::open(empty.path(), &format!("{:x}", Sha256::digest([]))).is_err());
        assert!(MeetingAudio::open(file.path().parent().unwrap(), &"0".repeat(64)).is_err());
        let mut text = TestFile::new();
        text.write_all(b"not audio").unwrap();
        assert!(
            MeetingAudio::open(text.path(), &format!("{:x}", Sha256::digest(b"not audio")))
                .is_err()
        );
    }
    #[test]
    fn changes_invalidate_attachment_permanently() {
        let (mut file, _, hash) = fixture();
        let mut audio = MeetingAudio::open(file.path(), &hash).unwrap();
        file.write_all(b"changed").unwrap();
        assert!(audio.check_unchanged().is_err());
        assert!(audio.read_range(0, 1).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn replaced_path_does_not_redirect_open_descriptor() {
        let (file, bytes, hash) = fixture();
        let mut audio = MeetingAudio::open(file.path(), &hash).unwrap();
        let replacement = TestFile::new();
        std::fs::rename(replacement.path(), file.path()).unwrap();
        assert_eq!(audio.read_range(0, bytes.len() as u64).unwrap(), bytes);
    }
}
