//! Reading a window of little-endian f32 mono PCM from the client's temp file.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Read `num_samples` samples starting at `offset_samples`.
pub fn read_window(path: &Path, offset_samples: u64, num_samples: u64) -> Result<Vec<f32>, String> {
    let mut file = File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let length = file
        .metadata()
        .map_err(|e| format!("cannot stat {}: {e}", path.display()))?
        .len();
    let start = offset_samples
        .checked_mul(4)
        .ok_or_else(|| "offset_samples overflows".to_string())?;
    let bytes = num_samples
        .checked_mul(4)
        .ok_or_else(|| "num_samples overflows".to_string())?;
    if start.checked_add(bytes).is_none_or(|end| end > length) {
        return Err(format!(
            "window {offset_samples}+{num_samples} samples exceeds the {} samples in {}",
            length / 4,
            path.display()
        ));
    }
    file.seek(SeekFrom::Start(start))
        .map_err(|e| format!("cannot seek {}: {e}", path.display()))?;
    let mut raw = vec![0u8; bytes as usize];
    file.read_exact(&mut raw)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Ok(raw
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn file_with(samples: &[f32]) -> tempfile_path::Temp {
        let temp = tempfile_path::Temp::new();
        let mut f = File::create(&temp.0).unwrap();
        for s in samples {
            f.write_all(&s.to_le_bytes()).unwrap();
        }
        temp
    }

    /// Minimal self-deleting temp file (avoids a dev-dependency).
    mod tempfile_path {
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicU32, Ordering};
        pub struct Temp(pub PathBuf);
        impl Temp {
            pub fn new() -> Self {
                static N: AtomicU32 = AtomicU32::new(0);
                Temp(std::env::temp_dir().join(format!(
                    "sagascript-ort-pcm-test-{}-{}.f32le",
                    std::process::id(),
                    N.fetch_add(1, Ordering::SeqCst)
                )))
            }
        }
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
    }

    #[test]
    fn reads_the_requested_range() {
        let temp = file_with(&[0.0, 1.5, -2.0, 3.25]);
        assert_eq!(read_window(&temp.0, 1, 2).unwrap(), vec![1.5, -2.0]);
        assert_eq!(read_window(&temp.0, 0, 4).unwrap().len(), 4);
    }

    #[test]
    fn rejects_out_of_range_and_missing_files() {
        let temp = file_with(&[0.0, 1.0]);
        assert!(read_window(&temp.0, 1, 2).is_err());
        assert!(read_window(&temp.0, u64::MAX, 1).is_err());
        assert!(read_window(Path::new("/nonexistent/x.f32le"), 0, 1).is_err());
    }
}
