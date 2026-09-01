use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use sagascript_core::diarization::DiarizationAnalysis;
use sagascript_core::error::DictationError;
use sagascript_core::transcription::diagnostics::{
    CoverageProfile, LanguageDetection, LanguageRegionDiagnostics,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const CACHE_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CacheIdentity {
    schema_version: u32,
    input_sha256: String,
    language: String,
    model: String,
    prompt_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DiarizationCache {
    identity: CacheIdentity,
    pub(crate) analysis: DiarizationAnalysis,
    pub(crate) transcript: Vec<(f64, f64, String)>,
    #[serde(default)]
    pub(crate) coverage_profile: CoverageProfile,
    #[serde(default)]
    pub(crate) detected_language: Option<LanguageDetection>,
    #[serde(default)]
    pub(crate) language_regions: Option<LanguageRegionDiagnostics>,
}

pub(crate) enum CacheLookup {
    Hit(Box<DiarizationCache>),
    Miss(&'static str),
}

impl CacheIdentity {
    pub(crate) fn for_input(
        input: &Path,
        language: &str,
        model: &str,
        prompt: Option<&str>,
    ) -> Result<Self, DictationError> {
        Ok(Self {
            schema_version: CACHE_SCHEMA_VERSION,
            input_sha256: sha256_file(input)?,
            language: language.to_string(),
            model: model.to_string(),
            prompt_sha256: sha256_bytes(prompt.unwrap_or_default().as_bytes()),
        })
    }
}

impl DiarizationCache {
    pub(crate) fn new(
        identity: CacheIdentity,
        analysis: DiarizationAnalysis,
        transcript: Vec<(f64, f64, String)>,
        coverage_profile: CoverageProfile,
        detected_language: Option<LanguageDetection>,
        language_regions: Option<LanguageRegionDiagnostics>,
    ) -> Self {
        Self {
            identity,
            analysis,
            transcript,
            coverage_profile,
            detected_language,
            language_regions,
        }
    }
}

pub(crate) fn load(path: &Path, expected: &CacheIdentity) -> Result<CacheLookup, DictationError> {
    if !path.exists() {
        return Ok(CacheLookup::Miss("not found"));
    }
    let file = File::open(path).map_err(|error| cache_error(path, "open", error))?;
    let cached: DiarizationCache =
        serde_json::from_reader(BufReader::new(file)).map_err(|error| {
            DictationError::FileDecodeError(format!(
                "Diarization cache {} is invalid JSON: {error}",
                path.display()
            ))
        })?;
    if cached.identity != *expected {
        return Ok(CacheLookup::Miss(
            "input, model, language, prompt, or schema changed",
        ));
    }
    cached.analysis.validate()?;
    if !cached.coverage_profile.validate() {
        return Err(DictationError::DiarizationError(
            "Cached diarization contains an invalid coverage profile".to_string(),
        ));
    }
    let duration = cached.coverage_profile.duration_seconds();
    if cached.transcript.iter().any(|(start, end, _)| {
        !start.is_finite()
            || !end.is_finite()
            || *start < 0.0
            || end < start
            || *end > duration + 0.001
    }) {
        return Err(DictationError::DiarizationError(
            "Cached diarization contains invalid transcript timestamps".to_string(),
        ));
    }
    Ok(CacheLookup::Hit(Box::new(cached)))
}

pub(crate) fn save(path: &Path, cache: &DiarizationCache) -> Result<(), DictationError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| cache_error(path, "create parent", error))?;
    }

    let temp_path = cache_temp_path(path);
    let write_result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temp_path)
            .map_err(|error| cache_error(path, "create", error))?;
        serde_json::to_writer(&mut file, cache).map_err(|error| {
            DictationError::FileDecodeError(format!(
                "Failed to serialize diarization cache {}: {error}",
                path.display()
            ))
        })?;
        file.write_all(b"\n")
            .map_err(|error| cache_error(path, "write", error))?;
        file.sync_all()
            .map_err(|error| cache_error(path, "sync", error))?;
        #[cfg(windows)]
        if path.exists() {
            std::fs::remove_file(path).map_err(|error| cache_error(path, "replace", error))?;
        }
        std::fs::rename(&temp_path, path).map_err(|error| cache_error(path, "replace", error))?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    write_result
}

fn sha256_file(path: &Path) -> Result<String, DictationError> {
    let mut file = File::open(path).map_err(|error| cache_error(path, "hash", error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| cache_error(path, "hash", error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn cache_temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("diarization-cache");
    path.with_file_name(format!(".{name}.{}.tmp", uuid::Uuid::new_v4()))
}

fn cache_error(path: &Path, action: &str, error: std::io::Error) -> DictationError {
    DictationError::FileDecodeError(format!(
        "Failed to {action} diarization cache {}: {error}",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sagascript_core::transcription::diagnostics::{LanguageRegion, TranscriptionWarning};

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "sagascript-diarization-cache-{}",
            uuid::Uuid::new_v4()
        ))
    }

    fn identity(input: &Path) -> CacheIdentity {
        CacheIdentity::for_input(input, "sv", "kb-whisper-large", Some("Grimnir")).unwrap()
    }

    #[test]
    fn round_trip_and_reject_changed_identity() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("audio.m4a");
        std::fs::write(&input, b"audio").unwrap();
        let cache_path = dir.join("analysis.json");
        let expected = identity(&input);
        let analysis: DiarizationAnalysis =
            serde_json::from_str(r#"{"raw_segments":[],"embeddings":[]}"#).unwrap();
        save(
            &cache_path,
            &DiarizationCache::new(
                expected.clone(),
                analysis,
                vec![(0.0, 1.0, " hej".into())],
                CoverageProfile::from_audio(&vec![0.1; 16_000]),
                Some(LanguageDetection {
                    language: "sv".into(),
                    probability: 0.99,
                }),
                Some(LanguageRegionDiagnostics {
                    regions: vec![LanguageRegion {
                        start: 0.0,
                        end: 1.0,
                        language: "sv".into(),
                        probability: 0.98,
                        window_count: 1,
                        stable: true,
                        first_sequence: 0,
                        last_sequence: 0,
                    }],
                    warnings: vec![TranscriptionWarning {
                        code: "test".into(),
                        message: "preserved".into(),
                        start: None,
                        end: None,
                    }],
                }),
            ),
        )
        .unwrap();

        let CacheLookup::Hit(hit) = load(&cache_path, &expected).unwrap() else {
            panic!("expected cache hit");
        };
        assert_eq!(hit.transcript.len(), 1);
        assert_eq!(hit.detected_language.unwrap().language, "sv");
        let regions = hit.language_regions.unwrap();
        assert_eq!(regions.regions[0].language, "sv");
        assert_eq!(regions.warnings[0].message, "preserved");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&cache_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        std::fs::write(&input, b"changed").unwrap();
        let changed = identity(&input);
        assert!(matches!(
            load(&cache_path, &changed).unwrap(),
            CacheLookup::Miss(_)
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn prior_schema_is_a_cache_miss() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("audio.m4a");
        std::fs::write(&input, b"audio").unwrap();
        let cache_path = dir.join("analysis.json");
        let expected = identity(&input);
        let mut old_identity = expected.clone();
        old_identity.schema_version -= 1;
        let analysis: DiarizationAnalysis =
            serde_json::from_str(r#"{"raw_segments":[],"embeddings":[]}"#).unwrap();
        save(
            &cache_path,
            &DiarizationCache::new(
                old_identity,
                analysis,
                Vec::new(),
                CoverageProfile::from_audio(&[]),
                None,
                None,
            ),
        )
        .unwrap();
        let mut old_json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cache_path).unwrap()).unwrap();
        old_json.as_object_mut().unwrap().remove("coverage_profile");
        std::fs::write(&cache_path, serde_json::to_vec(&old_json).unwrap()).unwrap();

        assert!(matches!(
            load(&cache_path, &expected).unwrap(),
            CacheLookup::Miss(_)
        ));
        let _ = std::fs::remove_dir_all(dir);
    }
}
