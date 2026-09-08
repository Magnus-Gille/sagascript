use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use sagascript_core::diarization::{model::DiarizationModel, DiarizationAnalysis, DiarizeConfig};
use sagascript_core::error::DictationError;
use sagascript_core::transcription::diagnostics::{
    CoverageProfile, LanguageDetection, LanguageRegionDiagnostics,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const CACHE_SCHEMA_VERSION: u32 = 4;
const MAX_CACHE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CacheIdentity {
    schema_version: u32,
    input_sha256: String,
    language: String,
    model: String,
    prompt_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AnalysisIdentity {
    segmentation_sha256: String,
    embedding_sha256: String,
    min_segment: f64,
    min_gap: f64,
}

impl AnalysisIdentity {
    pub(crate) fn current() -> Self {
        Self {
            segmentation_sha256: DiarizationModel::PyannoteSegmentation3
                .download_integrity()
                .sha256
                .to_string(),
            embedding_sha256: DiarizationModel::WeSpeakerResNet34LM
                .download_integrity()
                .sha256
                .to_string(),
            min_segment: DiarizeConfig::default().min_segment,
            min_gap: DiarizeConfig::default().min_gap,
        }
    }

    fn is_well_formed(&self) -> bool {
        is_lowercase_sha256(&self.segmentation_sha256)
            && is_lowercase_sha256(&self.embedding_sha256)
            && self.min_segment.is_finite()
            && self.min_segment >= 0.0
            && self.min_gap.is_finite()
            && self.min_gap >= 0.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DiarizationCache {
    identity: CacheIdentity,
    #[serde(default)]
    pub(crate) analysis_identity: Option<AnalysisIdentity>,
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
        let input_sha256 = sha256_file(input)?;
        Self::for_source_sha256(&input_sha256, language, model, prompt)
    }

    pub(crate) fn for_source_sha256(
        source_sha256: &str,
        language: &str,
        model: &str,
        prompt: Option<&str>,
    ) -> Result<Self, DictationError> {
        if !is_lowercase_sha256(source_sha256) {
            return Err(DictationError::FileDecodeError(
                "Meeting source hash must be a lowercase SHA-256 digest".to_string(),
            ));
        }
        Ok(Self {
            schema_version: CACHE_SCHEMA_VERSION,
            input_sha256: source_sha256.to_string(),
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
            analysis_identity: Some(AnalysisIdentity::current()),
            analysis,
            transcript,
            coverage_profile,
            detected_language,
            language_regions,
        }
    }
}

pub(crate) fn load(path: &Path, expected: &CacheIdentity) -> Result<CacheLookup, DictationError> {
    load_internal(path, expected, false)
}

pub(crate) fn load_for_rediarization(
    path: &Path,
    expected: &CacheIdentity,
) -> Result<CacheLookup, DictationError> {
    load_internal(path, expected, true)
}

fn load_internal(
    path: &Path,
    expected: &CacheIdentity,
    allow_stale_analysis: bool,
) -> Result<CacheLookup, DictationError> {
    if !path.exists() {
        return Ok(CacheLookup::Miss("not found"));
    }
    let bytes = read_cache_bytes(path)?;
    let cached: DiarizationCache = serde_json::from_slice(&bytes).map_err(|_| {
        DictationError::FileDecodeError(format!(
            "Diarization cache {} is invalid JSON",
            path.display()
        ))
    })?;
    if cached.identity != *expected || cached.identity.schema_version != CACHE_SCHEMA_VERSION {
        return Ok(CacheLookup::Miss(
            "input, model, language, prompt, or schema changed",
        ));
    }
    let Some(analysis_identity) = cached.analysis_identity.as_ref() else {
        return Ok(CacheLookup::Miss("analysis provenance is missing"));
    };
    if !analysis_identity.is_well_formed() {
        return Ok(CacheLookup::Miss("analysis provenance is malformed"));
    }
    if !allow_stale_analysis && *analysis_identity != AnalysisIdentity::current() {
        return Ok(CacheLookup::Miss(
            "analysis model, minimum segment, or minimum gap changed",
        ));
    }
    validate_cache_payload(&cached)?;
    Ok(CacheLookup::Hit(Box::new(cached)))
}

fn read_cache_bytes(path: &Path) -> Result<Vec<u8>, DictationError> {
    let metadata = std::fs::metadata(path).map_err(|error| cache_error(path, "inspect", error))?;
    if !metadata.is_file() {
        return Err(DictationError::FileDecodeError(format!(
            "Diarization cache {} is not a regular file",
            path.display()
        )));
    }
    if metadata.len() > MAX_CACHE_BYTES {
        return Err(DictationError::FileDecodeError(format!(
            "Diarization cache {} exceeds the {} MiB size limit",
            path.display(),
            MAX_CACHE_BYTES / (1024 * 1024)
        )));
    }

    let file = File::open(path).map_err(|error| cache_error(path, "open", error))?;
    let mut reader = BufReader::new(file).take(MAX_CACHE_BYTES + 1);
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| cache_error(path, "read", error))?;
    if bytes.len() as u64 > MAX_CACHE_BYTES {
        return Err(DictationError::FileDecodeError(format!(
            "Diarization cache {} exceeds the {} MiB size limit",
            path.display(),
            MAX_CACHE_BYTES / (1024 * 1024)
        )));
    }
    Ok(bytes)
}

fn validate_cache_payload(cached: &DiarizationCache) -> Result<(), DictationError> {
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
    Ok(())
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

/// Persist a newly recomputed cache without ever replacing an existing path.
/// The temporary file is created beside the destination, synced, then linked
/// into place; hard-link creation is the no-replace commit point.
pub(crate) fn save_new(path: &Path, cache: &DiarizationCache) -> Result<(), DictationError> {
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
            .map_err(|error| cache_error(path, "create new", error))?;
        serde_json::to_writer(&mut file, cache).map_err(|error| {
            DictationError::FileDecodeError(format!(
                "Failed to serialize new diarization cache {}: {error}",
                path.display()
            ))
        })?;
        file.write_all(b"\n")
            .map_err(|error| cache_error(path, "write new", error))?;
        file.sync_all()
            .map_err(|error| cache_error(path, "sync new", error))?;
        match std::fs::hard_link(&temp_path, path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(DictationError::FileDecodeError(format!(
                    "Diarization cache output already exists: {}",
                    path.display()
                )));
            }
            Err(error) => return Err(cache_error(path, "commit new", error)),
        }
        std::fs::remove_file(&temp_path)
            .map_err(|error| cache_error(path, "clean temporary", error))?;
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

fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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
        identity_for(input, "sv", "kb-whisper-large", Some("Grimnir"))
    }

    fn identity_for(
        input: &Path,
        language: &str,
        model: &str,
        prompt: Option<&str>,
    ) -> CacheIdentity {
        CacheIdentity::for_input(input, language, model, prompt).unwrap()
    }

    fn write_minimal_cache(path: &Path, identity: CacheIdentity) {
        write_cache_with_payload(
            path,
            identity,
            r#"{"raw_segments":[],"embeddings":[]}"#,
            Vec::new(),
        );
    }

    fn write_cache_with_payload(
        path: &Path,
        identity: CacheIdentity,
        analysis_json: &str,
        transcript: Vec<(f64, f64, String)>,
    ) {
        let analysis: DiarizationAnalysis = serde_json::from_str(analysis_json).unwrap();
        save(
            path,
            &DiarizationCache::new(
                identity,
                analysis,
                transcript,
                CoverageProfile::from_audio(&vec![0.1; 16_000]),
                None,
                None,
            ),
        )
        .unwrap();
    }

    #[test]
    fn identity_serialization_has_explicit_fields_and_no_threshold() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("audio.m4a");
        std::fs::write(&input, b"audio").unwrap();

        let serialized = serde_json::to_value(identity(&input)).unwrap();
        let object = serialized.as_object().unwrap();
        let mut fields = object.keys().cloned().collect::<Vec<_>>();
        fields.sort();
        assert_eq!(
            fields,
            vec![
                "input_sha256".to_string(),
                "language".to_string(),
                "model".to_string(),
                "prompt_sha256".to_string(),
                "schema_version".to_string(),
            ]
        );
        assert_eq!(object["schema_version"], serde_json::json!(4));
        assert!(!object.contains_key("threshold"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn freshly_recomputed_identity_reuses_cached_intermediates() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("audio.m4a");
        std::fs::write(&input, b"audio").unwrap();
        let cache_path = dir.join("analysis.json");
        let analysis_json = r#"{"raw_segments":[[0.0,1.0,0]],"embeddings":[]}"#;
        let transcript = vec![(0.0, 1.0, "cached transcript".to_string())];
        write_cache_with_payload(
            &cache_path,
            identity(&input),
            analysis_json,
            transcript.clone(),
        );

        let recomputed = identity_for(&input, "sv", "kb-whisper-large", Some("Grimnir"));
        let CacheLookup::Hit(hit) = load(&cache_path, &recomputed).unwrap() else {
            panic!("expected recomputed identity to hit");
        };
        assert_eq!(hit.transcript, transcript);
        let expected_analysis: serde_json::Value = serde_json::from_str(analysis_json).unwrap();
        assert_eq!(
            serde_json::to_value(&hit.analysis).unwrap(),
            expected_analysis
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_and_missing_prompt_are_the_same_identity() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("audio.m4a");
        std::fs::write(&input, b"audio").unwrap();
        let cache_path = dir.join("analysis.json");
        let without_prompt = identity_for(&input, "sv", "kb-whisper-large", None);
        let empty_prompt = identity_for(&input, "sv", "kb-whisper-large", Some(""));
        assert_eq!(without_prompt, empty_prompt);
        write_minimal_cache(&cache_path, without_prompt);
        assert!(matches!(
            load(&cache_path, &empty_prompt).unwrap(),
            CacheLookup::Hit(_)
        ));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn language_model_and_prompt_changes_miss_without_reason_matching() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("audio.m4a");
        std::fs::write(&input, b"audio").unwrap();
        let cache_path = dir.join("analysis.json");
        let baseline = identity(&input);
        write_minimal_cache(&cache_path, baseline.clone());

        for changed in [
            identity_for(&input, "en", "kb-whisper-large", Some("Grimnir")),
            identity_for(&input, "sv", "kb-whisper-base", Some("Grimnir")),
            identity_for(&input, "sv", "kb-whisper-large", Some("Other")),
            identity_for(&input, "sv", "kb-whisper-large", Some(" Grimnir")),
        ] {
            assert!(matches!(
                load(&cache_path, &changed).unwrap(),
                CacheLookup::Miss(_)
            ));
            assert!(matches!(
                load_for_rediarization(&cache_path, &changed).unwrap(),
                CacheLookup::Miss(_)
            ));
        }

        let grimnir_hash =
            identity_for(&input, "sv", "kb-whisper-large", Some("Grimnir")).prompt_sha256;
        let other_hash =
            identity_for(&input, "sv", "kb-whisper-large", Some("Other")).prompt_sha256;
        let leading_space_hash =
            identity_for(&input, "sv", "kb-whisper-large", Some(" Grimnir")).prompt_sha256;
        assert_ne!(grimnir_hash, other_hash);
        assert_ne!(grimnir_hash, leading_space_hash);

        let stored_nonempty_path = dir.join("stored-nonempty.json");
        let expected_none = identity_for(&input, "sv", "kb-whisper-large", None);
        write_minimal_cache(
            &stored_nonempty_path,
            identity_for(&input, "sv", "kb-whisper-large", Some("Grimnir")),
        );
        assert!(matches!(
            load(&stored_nonempty_path, &expected_none).unwrap(),
            CacheLookup::Miss(_)
        ));
        assert!(matches!(
            load_for_rediarization(&stored_nonempty_path, &expected_none).unwrap(),
            CacheLookup::Miss(_)
        ));

        let stored_none_path = dir.join("stored-none.json");
        write_minimal_cache(&stored_none_path, expected_none);
        let expected_nonempty = identity_for(&input, "sv", "kb-whisper-large", Some("Grimnir"));
        assert!(matches!(
            load(&stored_none_path, &expected_nonempty).unwrap(),
            CacheLookup::Miss(_)
        ));
        assert!(matches!(
            load_for_rediarization(&stored_none_path, &expected_nonempty).unwrap(),
            CacheLookup::Miss(_)
        ));

        assert!(matches!(
            load(&cache_path, &baseline).unwrap(),
            CacheLookup::Hit(_)
        ));

        let _ = std::fs::remove_dir_all(dir);
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
        assert!(matches!(
            load_for_rediarization(&cache_path, &changed).unwrap(),
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
                old_identity.clone(),
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
        old_json
            .as_object_mut()
            .unwrap()
            .remove("analysis_identity");
        std::fs::write(&cache_path, serde_json::to_vec(&old_json).unwrap()).unwrap();

        assert!(matches!(
            load(&cache_path, &expected).unwrap(),
            CacheLookup::Miss(_)
        ));
        assert!(matches!(
            load_for_rediarization(&cache_path, &expected).unwrap(),
            CacheLookup::Miss(_)
        ));
        assert!(matches!(
            load(&cache_path, &old_identity).unwrap(),
            CacheLookup::Miss(_)
        ));
        assert!(matches!(
            load_for_rediarization(&cache_path, &old_identity).unwrap(),
            CacheLookup::Miss(_)
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_analysis_identity_misses_normal_reuse_but_hits_rediarization() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("audio.m4a");
        std::fs::write(&input, b"audio").unwrap();
        let cache_path = dir.join("analysis.json");
        let expected = identity(&input);
        write_minimal_cache(&cache_path, expected.clone());
        let mut json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cache_path).unwrap()).unwrap();
        json["analysis_identity"] = serde_json::json!({
            "segmentation_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "embedding_sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "min_segment": 0.3,
            "min_gap": 0.5,
        });
        std::fs::write(&cache_path, serde_json::to_vec(&json).unwrap()).unwrap();

        assert!(matches!(
            load(&cache_path, &expected).unwrap(),
            CacheLookup::Miss(_)
        ));
        assert!(matches!(
            load_for_rediarization(&cache_path, &expected).unwrap(),
            CacheLookup::Hit(_)
        ));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn every_stale_analysis_dependency_misses_normal_reuse_but_rediarizes() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("audio.m4a");
        std::fs::write(&input, b"audio").unwrap();
        let cache_path = dir.join("analysis.json");
        let expected = identity(&input);

        for (field, value) in [
            (
                "segmentation_sha256",
                serde_json::json!(
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                ),
            ),
            (
                "embedding_sha256",
                serde_json::json!(
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                ),
            ),
            ("min_segment", serde_json::json!(0.31)),
            ("min_gap", serde_json::json!(0.51)),
        ] {
            write_minimal_cache(&cache_path, expected.clone());
            let mut json: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&cache_path).unwrap()).unwrap();
            json["analysis_identity"][field] = value;
            std::fs::write(&cache_path, serde_json::to_vec(&json).unwrap()).unwrap();

            assert!(matches!(
                load(&cache_path, &expected).unwrap(),
                CacheLookup::Miss(_)
            ));
            assert!(matches!(
                load_for_rediarization(&cache_path, &expected).unwrap(),
                CacheLookup::Hit(_)
            ));
        }

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_or_malformed_analysis_identity_never_hits() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("audio.m4a");
        std::fs::write(&input, b"audio").unwrap();
        let cache_path = dir.join("analysis.json");
        let expected = identity(&input);

        for mutation in [
            serde_json::json!(null),
            serde_json::json!({
                "segmentation_sha256": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                "embedding_sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "min_segment": 0.3,
                "min_gap": 0.5,
            }),
            serde_json::json!({
                "segmentation_sha256": "short",
                "embedding_sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "min_segment": 0.3,
                "min_gap": 0.5,
            }),
            serde_json::json!({
                "segmentation_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "embedding_sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "min_segment": -0.1,
                "min_gap": 0.5,
            }),
            serde_json::json!({
                "segmentation_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "embedding_sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "min_segment": 0.3,
                "min_gap": -0.1,
            }),
        ] {
            write_minimal_cache(&cache_path, expected.clone());
            let mut json: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&cache_path).unwrap()).unwrap();
            json["analysis_identity"] = mutation;
            std::fs::write(&cache_path, serde_json::to_vec(&json).unwrap()).unwrap();

            assert!(!matches!(
                load(&cache_path, &expected),
                Ok(CacheLookup::Hit(_))
            ));
            assert!(!matches!(
                load_for_rediarization(&cache_path, &expected),
                Ok(CacheLookup::Hit(_))
            ));
        }

        write_minimal_cache(&cache_path, expected.clone());
        let mut missing: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cache_path).unwrap()).unwrap();
        missing.as_object_mut().unwrap().remove("analysis_identity");
        std::fs::write(&cache_path, serde_json::to_vec(&missing).unwrap()).unwrap();
        assert!(!matches!(
            load(&cache_path, &expected),
            Ok(CacheLookup::Hit(_))
        ));
        assert!(!matches!(
            load_for_rediarization(&cache_path, &expected),
            Ok(CacheLookup::Hit(_))
        ));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_payload_is_rejected_by_both_loaders() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("audio.m4a");
        std::fs::write(&input, b"audio").unwrap();
        let cache_path = dir.join("analysis.json");
        let expected = identity(&input);

        write_minimal_cache(&cache_path, expected.clone());
        let mut invalid_analysis: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cache_path).unwrap()).unwrap();
        invalid_analysis["analysis"]["raw_segments"] = serde_json::json!([[1.0, 0.0, 0]]);
        std::fs::write(&cache_path, serde_json::to_vec(&invalid_analysis).unwrap()).unwrap();
        assert!(load(&cache_path, &expected).is_err());
        assert!(load_for_rediarization(&cache_path, &expected).is_err());

        write_minimal_cache(&cache_path, expected.clone());
        let mut invalid_transcript: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cache_path).unwrap()).unwrap();
        invalid_transcript["transcript"] = serde_json::json!([[0.0, 2.0, "outside"]]);
        std::fs::write(
            &cache_path,
            serde_json::to_vec(&invalid_transcript).unwrap(),
        )
        .unwrap();
        assert!(load(&cache_path, &expected).is_err());
        assert!(load_for_rediarization(&cache_path, &expected).is_err());

        std::fs::write(&cache_path, b"{").unwrap();
        assert!(load(&cache_path, &expected).is_err());
        assert!(load_for_rediarization(&cache_path, &expected).is_err());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_cache_errors_do_not_echo_payload_text() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("audio.m4a");
        std::fs::write(&input, b"audio").unwrap();
        let cache_path = dir.join("analysis.json");
        let expected = identity(&input);
        let marker = "CACHE_SECRET_MARKER";
        std::fs::write(
            &cache_path,
            format!(r#"{{"analysis":"{marker}","transcript":[}}"#),
        )
        .unwrap();

        for result in [
            load(&cache_path, &expected),
            load_for_rediarization(&cache_path, &expected),
        ] {
            let error = match result {
                Err(error) => error,
                Ok(_) => panic!("malformed cache must fail"),
            };
            assert!(error.to_string().contains("invalid JSON"));
            assert!(!error.to_string().contains(marker));
        }

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn loaders_reject_oversized_and_non_regular_cache_paths() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("audio.m4a");
        std::fs::write(&input, b"audio").unwrap();
        let expected = identity(&input);

        let oversized = dir.join("oversized.json");
        let file = File::create(&oversized).unwrap();
        file.set_len(MAX_CACHE_BYTES + 1).unwrap();
        drop(file);
        for result in [
            load(&oversized, &expected),
            load_for_rediarization(&oversized, &expected),
        ] {
            let error = match result {
                Err(error) => error,
                Ok(_) => panic!("oversized cache must fail"),
            };
            assert!(error.to_string().contains("exceeds the 256 MiB size limit"));
        }

        let directory = dir.join("cache-directory");
        std::fs::create_dir(&directory).unwrap();
        for result in [
            load(&directory, &expected),
            load_for_rediarization(&directory, &expected),
        ] {
            let error = match result {
                Err(error) => error,
                Ok(_) => panic!("non-regular cache path must fail"),
            };
            assert!(error.to_string().contains("not a regular file"));
        }

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_new_refuses_existing_destination_without_replacing_it() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("audio.m4a");
        std::fs::write(&input, b"audio").unwrap();
        let cache_path = dir.join("analysis.json");
        let identity = identity(&input);
        write_minimal_cache(&cache_path, identity.clone());
        let original = std::fs::read(&cache_path).unwrap();
        let cache = serde_json::from_slice::<DiarizationCache>(&original).unwrap();

        let error = save_new(&cache_path, &cache).expect_err("existing output must be protected");
        assert!(error.to_string().contains("output already exists"));
        assert_eq!(std::fs::read(&cache_path).unwrap(), original);
        assert!(
            !std::fs::read_dir(&dir).unwrap().flatten().any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".analysis.json."))
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_new_refuses_hardlinked_input_without_overwriting_either_link() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("audio.m4a");
        let output = dir.join("analysis.json");
        std::fs::write(&input, b"input must survive").unwrap();
        std::fs::hard_link(&input, &output).unwrap();
        let cache = DiarizationCache::new(
            identity(&input),
            serde_json::from_str(r#"{"raw_segments":[],"embeddings":[]}"#).unwrap(),
            Vec::new(),
            CoverageProfile::from_audio(&[]),
            None,
            None,
        );

        let error = save_new(&output, &cache).expect_err("hardlinked output must be protected");
        assert!(error.to_string().contains("output already exists"));
        assert_eq!(std::fs::read(&input).unwrap(), b"input must survive");
        assert_eq!(std::fs::read(&output).unwrap(), b"input must survive");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_new_uses_private_permissions() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("audio.m4a");
        let output = dir.join("analysis.json");
        std::fs::write(&input, b"audio").unwrap();
        let cache = DiarizationCache::new(
            identity(&input),
            serde_json::from_str(r#"{"raw_segments":[],"embeddings":[]}"#).unwrap(),
            Vec::new(),
            CoverageProfile::from_audio(&[]),
            None,
            None,
        );
        save_new(&output, &cache).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&output).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        let _ = std::fs::remove_dir_all(dir);
    }
}
