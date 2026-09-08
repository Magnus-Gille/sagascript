use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use sagascript_core::diarization::DiarizationAnalysis;
use sagascript_core::meeting::{MeetingSegmentInput, MeetingSpeaker, MeetingTranscript};
use sagascript_core::meeting_reprocess_plan::ReprocessingMode;
use sagascript_core::meeting_review::MeetingReview;
use sagascript_core::settings::{Language, WhisperModel};
use sagascript_core::transcription::Glossary;
use sha2::{Digest, Sha256};

use super::{execute_with, plan_reprocessing, ReprocessingInput, WorkBackend};
use crate::diarization_cache::{self, CacheIdentity, DiarizationCache};
use crate::transcribe::{MeetingControl, MeetingPhase};
use sagascript_core::diarization::DiarizeConfig as CoreDiarizeConfig;
use sagascript_core::error::DictationError;

#[derive(Default)]
struct CountingBackend {
    decode_calls: usize,
    analyze_calls: usize,
    full_calls: usize,
    fail_decode: bool,
    fail_analyze: bool,
    fail_full: bool,
}

impl WorkBackend for CountingBackend {
    fn decode(
        &mut self,
        _audio: &Path,
        _control: Option<&MeetingControl<'_>>,
    ) -> Result<Vec<f32>, DictationError> {
        self.decode_calls += 1;
        if self.fail_decode {
            return Err(DictationError::TranscriptionFailed(
                "synthetic decode failure".to_string(),
            ));
        }
        Ok(vec![0.1; 16_000])
    }

    fn analyze(
        &mut self,
        _audio: &[f32],
        _config: &CoreDiarizeConfig,
    ) -> Result<DiarizationAnalysis, DictationError> {
        self.analyze_calls += 1;
        if self.fail_analyze {
            return Err(DictationError::TranscriptionFailed(
                "synthetic analysis failure".to_string(),
            ));
        }
        Ok(
            serde_json::from_str(r#"{"raw_segments":[],"embeddings":[]}"#)
                .expect("empty synthetic analysis is valid"),
        )
    }

    fn full(
        &mut self,
        input: &ReprocessingInput<'_>,
        _threshold: f32,
        _cache_output: Option<&Path>,
        _control: Option<&MeetingControl<'_>>,
    ) -> Result<MeetingTranscript, DictationError> {
        self.full_calls += 1;
        if self.fail_full {
            return Err(DictationError::TranscriptionFailed(
                "synthetic full failure".to_string(),
            ));
        }
        Ok(input.previous.original.clone())
    }
}

struct Fixture {
    root: PathBuf,
    audio: PathBuf,
    cache: PathBuf,
    previous: MeetingReview,
    glossary: Glossary,
}

impl Fixture {
    fn new(with_cache: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "sagascript-meeting-reprocessing-tests-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create synthetic fixture directory");
        let audio = root.join("synthetic-audio.bin");
        let cache = root.join("diarization-cache.json");
        std::fs::write(&audio, b"synthetic audio bytes").expect("write synthetic audio");

        let source_sha256 = sha256_file(&audio);
        let previous = MeetingReview::new(
            MeetingTranscript::new(
                source_sha256.clone(),
                "en",
                "base",
                1.0,
                vec![MeetingSegmentInput {
                    start: 0.0,
                    end: 1.0,
                    text: "cached transcript".to_string(),
                    speaker: "SPEAKER_0".to_string(),
                }],
                vec![MeetingSpeaker {
                    id: "SPEAKER_0".to_string(),
                    label: "SPEAKER_0".to_string(),
                }],
            )
            .expect("synthetic transcript is valid"),
        )
        .expect("synthetic review is valid");

        if with_cache {
            let identity = CacheIdentity::for_source_sha256(&source_sha256, "en", "base", None)
                .expect("synthetic cache identity is valid");
            let analysis: DiarizationAnalysis =
                serde_json::from_str(r#"{"raw_segments":[],"embeddings":[]}"#)
                    .expect("empty synthetic analysis is valid");
            let cache_data = DiarizationCache::new(
                identity,
                analysis,
                vec![(0.0, 1.0, "cached transcript".to_string())],
                sagascript_core::transcription::diagnostics::CoverageProfile::from_audio(
                    &vec![0.1; 16_000],
                ),
                None,
                None,
            );
            diarization_cache::save(&cache, &cache_data).expect("save synthetic cache");
        }

        Self {
            root,
            audio,
            cache,
            previous,
            glossary: Glossary::default(),
        }
    }

    fn input<'a>(&'a self, cache: Option<&'a Path>) -> ReprocessingInput<'a> {
        ReprocessingInput {
            audio: &self.audio,
            cache,
            previous: &self.previous,
            language: Language::English,
            model: WhisperModel::Base,
            glossary: &self.glossary,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn sha256_file(path: &Path) -> String {
    format!(
        "{:x}",
        Sha256::digest(std::fs::read(path).expect("read synthetic audio"))
    )
}

fn assert_counts(backend: &CountingBackend, decode: usize, analyze: usize, full: usize) {
    assert_eq!(backend.decode_calls, decode, "unexpected decode call count");
    assert_eq!(
        backend.analyze_calls, analyze,
        "unexpected analyze call count"
    );
    assert_eq!(backend.full_calls, full, "unexpected full call count");
}

#[test]
fn work_modes_invoke_only_their_selected_backend_stages() {
    let fixture = Fixture::new(true);
    let input = fixture.input(Some(&fixture.cache));
    let recluster_plan =
        plan_reprocessing(&input, ReprocessingMode::Recluster, 0.75, None).expect("recluster plan");
    let mut recluster = CountingBackend::default();
    execute_with(
        &recluster_plan,
        &recluster_plan.revision,
        &input,
        None,
        None,
        &mut recluster,
    )
    .expect("recluster execution");
    assert_counts(&recluster, 0, 0, 0);

    let rediarize_plan =
        plan_reprocessing(&input, ReprocessingMode::Rediarize, 0.75, None).expect("rediarize plan");
    let mut rediarize = CountingBackend::default();
    execute_with(
        &rediarize_plan,
        &rediarize_plan.revision,
        &input,
        None,
        None,
        &mut rediarize,
    )
    .expect("rediarize execution");
    assert_counts(&rediarize, 1, 1, 0);

    let full_input = fixture.input(None);
    let full_plan =
        plan_reprocessing(&full_input, ReprocessingMode::Full, 0.75, None).expect("full plan");
    let mut full = CountingBackend::default();
    execute_with(
        &full_plan,
        &full_plan.revision,
        &full_input,
        None,
        None,
        &mut full,
    )
    .expect("full execution");
    assert_counts(&full, 0, 0, 1);
}

fn assert_selective_failure_without_full_fallback(mutate: impl FnOnce(&Fixture)) {
    let fixture = Fixture::new(true);
    let input = fixture.input(Some(&fixture.cache));
    let plan = plan_reprocessing(&input, ReprocessingMode::Recluster, 0.75, None)
        .expect("baseline selective plan");
    let previous = fixture.previous.clone();
    mutate(&fixture);

    let mut backend = CountingBackend::default();
    assert!(execute_with(&plan, &plan.revision, &input, None, None, &mut backend,).is_err());
    assert_counts(&backend, 0, 0, 0);
    assert_eq!(
        fixture.previous, previous,
        "failed execution changed review"
    );
}

#[test]
fn selective_cache_misses_never_fallback_to_full_inference() {
    assert_selective_failure_without_full_fallback(|fixture| {
        std::fs::remove_file(&fixture.cache).expect("remove synthetic cache");
    });

    assert_selective_failure_without_full_fallback(|fixture| {
        std::fs::write(&fixture.cache, b"not valid cache JSON").expect("corrupt cache");
    });

    assert_selective_failure_without_full_fallback(|fixture| {
        let mut cache: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&fixture.cache).expect("read cache"))
                .expect("cache JSON");
        cache["analysis_identity"]["min_segment"] = serde_json::json!(0.31);
        std::fs::write(
            &fixture.cache,
            serde_json::to_vec(&cache).expect("serialize stale cache"),
        )
        .expect("write stale cache");
    });

    assert_selective_failure_without_full_fallback(|fixture| {
        std::fs::write(&fixture.audio, b"changed synthetic audio").expect("change source context");
    });
}

#[test]
fn cancellation_and_analysis_failure_return_no_proposal_or_review_mutation() {
    let fixture = Fixture::new(true);
    let input = fixture.input(Some(&fixture.cache));
    let plan =
        plan_reprocessing(&input, ReprocessingMode::Rediarize, 0.75, None).expect("rediarize plan");
    let previous = fixture.previous.clone();

    let cancellation = AtomicBool::new(true);
    let progress = |_phase: MeetingPhase| {};
    let control = MeetingControl {
        cancellation: &cancellation,
        progress: &progress,
    };
    let mut cancelled = CountingBackend::default();
    assert!(execute_with(
        &plan,
        &plan.revision,
        &input,
        None,
        Some(&control),
        &mut cancelled,
    )
    .is_err());
    assert_counts(&cancelled, 0, 0, 0);
    assert_eq!(fixture.previous, previous);

    let mut failed = CountingBackend {
        fail_analyze: true,
        ..Default::default()
    };
    assert!(execute_with(&plan, &plan.revision, &input, None, None, &mut failed,).is_err());
    assert_counts(&failed, 1, 1, 0);
    assert_eq!(fixture.previous, previous);
}
