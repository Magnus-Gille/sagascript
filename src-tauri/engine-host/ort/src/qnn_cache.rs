//! Content- and runtime-bound cache for compiled QNN encoder contexts.
//!
//! The caller supplies the complete compatibility identity. This module only
//! manages immutable generations and their integrity; it does not decide
//! whether a runtime or device can load a generation.

use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const CACHE_VERSION: &str = "qnn-v1";
const MODEL_FILE: &str = "context.onnx";
const WEIGHTS_FILE: &str = "context_qnn.bin";
const COMPLETE_FILE: &str = "complete.json";
static GENERATION_NONCE: AtomicU64 = AtomicU64::new(0);
static LAST_GENERATION_NANOS: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct Cache {
    identity_hash: String,
    cache_dir: PathBuf,
    _lock: File,
}

#[derive(Debug)]
struct ArtifactDigest {
    size: u64,
    sha256: String,
}

#[derive(Debug)]
struct Completion {
    format: u32,
    identity_sha256: String,
    model: ArtifactDigest,
    weights: ArtifactDigest,
}

impl Cache {
    /// Open the cache for an absolute caller-owned root and compatibility identity.
    /// The returned value holds an OS-exclusive lock for its lifetime.
    pub fn open(root: &Path, identity: &str) -> Result<Self, String> {
        if !root.is_absolute() {
            return Err("QNN cache root must be absolute".into());
        }
        if identity.is_empty() {
            return Err("QNN cache identity must not be empty".into());
        }

        let identity_hash = hex(&Sha256::digest(identity.as_bytes()));
        let cache_dir = root.join(CACHE_VERSION).join(&identity_hash);
        fs::create_dir_all(&cache_dir)
            .map_err(|error| format!("cannot create QNN cache directory: {error}"))?;
        let cache_dir = cache_dir
            .canonicalize()
            .map_err(|error| format!("cannot resolve QNN cache directory: {error}"))?;
        let lock_path = cache_dir.join(".lock");
        let lock = open_exclusive_lock(&lock_path)?;

        Ok(Self {
            identity_hash,
            cache_dir,
            _lock: lock,
        })
    }

    /// Return the newest completed generation whose fixed-name artifacts still
    /// match the recorded sizes and SHA-256 hashes. Invalid generations are skipped.
    pub fn cached_model(&self) -> Result<Option<PathBuf>, String> {
        let entries = fs::read_dir(&self.cache_dir)
            .map_err(|error| format!("cannot read QNN cache directory: {error}"))?;
        let mut generations = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| format!("cannot read QNN cache entry: {error}"))?;
            let name = entry.file_name();
            if name.to_string_lossy().starts_with("gen-")
                && entry.file_type().map(|ty| ty.is_dir()).unwrap_or(false)
            {
                generations.push(entry.path());
            }
        }
        generations.sort();
        for generation in generations.into_iter().rev() {
            if let Some(model) = self.valid_generation(&generation) {
                return Ok(Some(model));
            }
        }
        Ok(None)
    }

    /// Reserve a fresh immutable generation and return its model path for ORT's
    /// `context_file_path`. The caller lets ORT create both fixed-name files.
    pub fn begin_generation(&self) -> Result<PathBuf, String> {
        for _ in 0..32 {
            let wall_nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| format!("system clock is before Unix epoch: {error}"))?
                .as_nanos() as u64;
            let nanos = next_generation_timestamp(wall_nanos);
            let nonce = GENERATION_NONCE.fetch_add(1, Ordering::Relaxed);
            let generation = self.cache_dir.join(format!("gen-{nanos:020}-{nonce:016x}"));
            match fs::create_dir(&generation) {
                Ok(()) => return Ok(generation.join(MODEL_FILE)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("cannot create QNN cache generation: {error}")),
            }
        }
        Err("could not reserve a unique QNN cache generation".into())
    }

    /// Publish a generation only after both ORT output files have been written
    /// and hashed. Existing completion records are never overwritten.
    pub fn complete_generation(&self, model_path: &Path) -> Result<(), String> {
        let generation = self.owned_generation(model_path)?;
        let model = digest_nonempty(&generation.join(MODEL_FILE))?;
        let weights = digest_nonempty(&generation.join(WEIGHTS_FILE))?;
        let completion = Completion {
            format: 1,
            identity_sha256: self.identity_hash.clone(),
            model,
            weights,
        };
        let final_path = generation.join(COMPLETE_FILE);
        if final_path.exists() {
            return Err("QNN cache generation is already completed".into());
        }
        let temp_path = generation.join("complete.tmp");
        let bytes = serde_json::to_vec(&serde_json::json!({
            "format": completion.format,
            "identity_sha256": completion.identity_sha256,
            "model": {"size": completion.model.size, "sha256": completion.model.sha256},
            "weights": {"size": completion.weights.size, "sha256": completion.weights.sha256},
        }))
        .map_err(|error| format!("cannot serialize QNN cache metadata: {error}"))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|error| format!("cannot create QNN cache metadata: {error}"))?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("cannot write QNN cache metadata: {error}"))?;
        drop(file);
        fs::rename(&temp_path, &final_path)
            .map_err(|error| format!("cannot publish QNN cache metadata: {error}"))
    }

    /// Make an owned generation unavailable without deleting its files.
    pub fn invalidate(&self, model_path: &Path) -> Result<(), String> {
        let generation = self.owned_generation(model_path)?;
        let metadata = generation.join(COMPLETE_FILE);
        match fs::remove_file(metadata) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("cannot invalidate QNN cache generation: {error}")),
        }
    }

    fn owned_generation(&self, model_path: &Path) -> Result<PathBuf, String> {
        if model_path.file_name().and_then(|name| name.to_str()) != Some(MODEL_FILE) {
            return Err("QNN cache model path must use the fixed context filename".into());
        }
        let generation = model_path
            .parent()
            .ok_or_else(|| "QNN cache model path has no parent".to_string())?;
        let canonical = generation
            .canonicalize()
            .map_err(|error| format!("cannot resolve QNN cache generation: {error}"))?;
        if canonical.parent() != Some(self.cache_dir.as_path())
            || !canonical
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("gen-"))
        {
            return Err("QNN cache generation is outside this cache identity".into());
        }
        Ok(canonical)
    }

    fn valid_generation(&self, generation: &Path) -> Option<PathBuf> {
        let canonical = generation.canonicalize().ok()?;
        if canonical.parent()? != self.cache_dir {
            return None;
        }
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(canonical.join(COMPLETE_FILE)).ok()?).ok()?;
        let metadata = Completion {
            format: u32::try_from(value.get("format")?.as_u64()?).ok()?,
            identity_sha256: value.get("identity_sha256")?.as_str()?.to_owned(),
            model: parse_digest(value.get("model")?)?,
            weights: parse_digest(value.get("weights")?)?,
        };
        if metadata.format != 1 || metadata.identity_sha256 != self.identity_hash {
            return None;
        }
        if !digest_matches(&canonical.join(MODEL_FILE), &metadata.model)
            || !digest_matches(&canonical.join(WEIGHTS_FILE), &metadata.weights)
        {
            return None;
        }
        Some(canonical.join(MODEL_FILE))
    }
}

fn next_generation_timestamp(wall_nanos: u64) -> u64 {
    let mut previous = LAST_GENERATION_NANOS.load(Ordering::Relaxed);
    loop {
        let next = wall_nanos.max(previous.saturating_add(1));
        match LAST_GENERATION_NANOS.compare_exchange_weak(
            previous,
            next,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return next,
            Err(actual) => previous = actual,
        }
    }
}

fn parse_digest(value: &serde_json::Value) -> Option<ArtifactDigest> {
    Some(ArtifactDigest {
        size: value.get("size")?.as_u64()?,
        sha256: value.get("sha256")?.as_str()?.to_owned(),
    })
}

fn digest_nonempty(path: &Path) -> Result<ArtifactDigest, String> {
    let (size, sha256) = digest_file(path)?;
    if size == 0 {
        return Err(format!(
            "QNN cache artifact is empty: {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    Ok(ArtifactDigest { size, sha256 })
}

fn digest_matches(path: &Path, expected: &ArtifactDigest) -> bool {
    digest_file(path)
        .is_ok_and(|(size, sha256)| size > 0 && size == expected.size && sha256 == expected.sha256)
}

fn digest_file(path: &Path) -> Result<(u64, String), String> {
    let mut file =
        File::open(path).map_err(|error| format!("cannot open QNN cache artifact: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot read QNN cache artifact: {error}"))?;
        if read == 0 {
            break;
        }
        size = size.saturating_add(read as u64);
        hasher.update(&buffer[..read]);
    }
    Ok((size, hex(&hasher.finalize())))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(windows)]
fn open_exclusive_lock(path: &Path) -> Result<File, String> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(path)
        .map_err(|error| format!("QNN cache is already in use or cannot be locked: {error}"))
}

// QNN is currently a Windows-only runtime. Keep other targets buildable for
// host tests; they use a persistent lock file but do not run QNN concurrently.
#[cfg(not(windows))]
fn open_exclusive_lock(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|error| format!("cannot open QNN cache lock file: {error}"))
}

#[cfg(test)]
mod tests {
    use super::Cache;
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "sagascript-qnn-cache-test-{}-{nonce}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn cache(root: &Path, identity: &str) -> Cache {
        Cache::open(root, identity).unwrap()
    }

    fn finish(cache: &Cache, model: &Path) {
        fs::write(model, b"context graph").unwrap();
        fs::write(
            model.parent().unwrap().join("context_qnn.bin"),
            b"compiled weights",
        )
        .unwrap();
        cache.complete_generation(model).unwrap();
    }

    #[test]
    fn requires_absolute_root_and_identity_isolates_directories() {
        assert!(Cache::open(Path::new("relative-cache"), "model").is_err());
        let temp = TempDir::new().unwrap();
        let first = cache(temp.path(), "graph-a/runtime-a/device-a");
        let model = first.begin_generation().unwrap();
        finish(&first, &model);
        assert!(first.cached_model().unwrap().is_some());
        let second = cache(temp.path(), "graph-b/runtime-a/device-a");
        assert!(second.cached_model().unwrap().is_none());
    }

    #[test]
    fn incomplete_and_corrupt_generations_are_never_reused() {
        let temp = TempDir::new().unwrap();
        let cache = cache(temp.path(), "identity");
        let incomplete = cache.begin_generation().unwrap();
        fs::write(&incomplete, b"partial graph").unwrap();
        assert!(cache.cached_model().unwrap().is_none());

        let complete = cache.begin_generation().unwrap();
        finish(&cache, &complete);
        assert_eq!(cache.cached_model().unwrap(), Some(complete.clone()));
        fs::write(&complete, b"tampered graph").unwrap();
        assert!(cache.cached_model().unwrap().is_none());
    }

    #[test]
    fn empty_artifacts_cannot_be_completed() {
        let temp = TempDir::new().unwrap();
        let cache = cache(temp.path(), "identity");
        let model = cache.begin_generation().unwrap();
        fs::write(&model, []).unwrap();
        fs::write(model.parent().unwrap().join("context_qnn.bin"), b"weights").unwrap();
        assert!(cache.complete_generation(&model).is_err());
        assert!(cache.cached_model().unwrap().is_none());
    }

    #[test]
    fn latest_valid_generation_wins_and_invalidate_preserves_files() {
        let temp = TempDir::new().unwrap();
        let cache = cache(temp.path(), "identity");
        let older = cache.begin_generation().unwrap();
        finish(&cache, &older);
        let newer = cache.begin_generation().unwrap();
        finish(&cache, &newer);
        assert_eq!(cache.cached_model().unwrap(), Some(newer.clone()));
        cache.invalidate(&newer).unwrap();
        assert!(newer.exists());
        assert_eq!(cache.cached_model().unwrap(), Some(older));
    }

    #[test]
    fn completion_and_invalidation_reject_paths_outside_owned_generation() {
        let temp = TempDir::new().unwrap();
        let cache = cache(temp.path(), "identity");
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let fake = outside.join("context.onnx");
        fs::write(&fake, b"model").unwrap();
        fs::write(outside.join("context_qnn.bin"), b"weights").unwrap();
        assert!(cache.complete_generation(&fake).is_err());
        assert!(cache.invalidate(&fake).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn exclusive_lock_releases_when_cache_is_dropped() {
        let temp = TempDir::new().unwrap();
        let first = cache(temp.path(), "identity");
        assert!(Cache::open(temp.path(), "identity").is_err());
        drop(first);
        assert!(Cache::open(temp.path(), "identity").is_ok());
    }
}
