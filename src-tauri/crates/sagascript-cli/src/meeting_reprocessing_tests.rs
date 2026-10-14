use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sagascript_core::diarization::DiarizationAnalysis;
use sagascript_core::meeting::MeetingTranscript;
use sagascript_core::meeting_reprocess_plan::ReprocessingMode;
use sagascript_core::meeting_review::MeetingReview;
use sagascript_core::settings::{Language, WhisperModel};
use sagascript_core::test_support::meeting_transcript;
use sagascript_core::transcription::Glossary;
use sagascript_testsupport::{sha256_hex, TempDir};

use sagascript_core::test_support::FaultyFs;

use super::{
    execute_with, execute_with_model_check, plan_reprocessing, plan_reprocessing_with_model_check,
    CommitError, PendingPublication, ReprocessingInput, WorkBackend,
};
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
    cancel_during_analyze: bool,
    last_full_hint: Option<sagascript_core::speaker_hint::SpeakerCountHint>,
    /// Behave like the real full pipeline: save a new cache at the path it is
    /// given with `save_new`, as `transcribe_file` does mid-inference.
    write_cache: bool,
    /// Return a transcript of a different recording (a post-inference failure).
    wrong_source_identity: bool,
    /// Raise the cancellation flag after the cache was written.
    cancel_after_full: bool,
    /// Runs after the cache was written and before `full` returns: a stand-in
    /// for whatever happens to the outside world while inference runs.
    during_full: Option<Box<dyn FnMut()>>,
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
        control: Option<&MeetingControl<'_>>,
    ) -> Result<DiarizationAnalysis, DictationError> {
        self.analyze_calls += 1;
        if self.fail_analyze {
            return Err(DictationError::TranscriptionFailed(
                "synthetic analysis failure".to_string(),
            ));
        }
        if self.cancel_during_analyze {
            let control = control.expect("mid-analysis cancellation needs control");
            control.cancellation.store(true, Ordering::Release);
            control.check()?;
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
        speaker_hint: Option<sagascript_core::speaker_hint::SpeakerCountHint>,
        cache_output: Option<&Path>,
        control: Option<&MeetingControl<'_>>,
    ) -> Result<MeetingTranscript, DictationError> {
        self.full_calls += 1;
        self.last_full_hint = speaker_hint;
        if self.fail_full {
            return Err(DictationError::TranscriptionFailed(
                "synthetic full failure".to_string(),
            ));
        }
        if let (true, Some(path)) = (self.write_cache, cache_output) {
            let source = &input.previous.original.source_sha256;
            diarization_cache::save_new(path, &synthetic_cache(source))?;
        }
        if let Some(during_full) = self.during_full.as_mut() {
            during_full();
        }
        if self.cancel_after_full {
            control
                .expect("cancelling after inference needs control")
                .cancellation
                .store(true, Ordering::Release);
        }
        let mut transcript = input.previous.original.clone();
        if self.wrong_source_identity {
            transcript.source_sha256 = "f".repeat(64);
        }
        Ok(transcript)
    }
}

fn synthetic_cache(source_sha256: &str) -> DiarizationCache {
    let identity = CacheIdentity::for_source_sha256(source_sha256, "en", "base", None)
        .expect("synthetic cache identity is valid");
    let analysis: DiarizationAnalysis =
        serde_json::from_str(r#"{"raw_segments":[],"embeddings":[]}"#)
            .expect("empty synthetic analysis is valid");
    DiarizationCache::new(
        identity,
        analysis,
        vec![(0.0, 1.0, "cached transcript".to_string())],
        sagascript_core::transcription::diagnostics::CoverageProfile::from_audio(&vec![
            0.1;
            16_000
        ]),
        None,
        None,
    )
}

struct Fixture {
    _root: TempDir,
    audio: PathBuf,
    cache: PathBuf,
    previous: MeetingReview,
    glossary: Glossary,
}

impl Fixture {
    fn new(with_cache: bool) -> Self {
        let root = TempDir::new();
        let audio = root.join("synthetic-audio.bin");
        let cache = root.join("diarization-cache.json");
        std::fs::write(&audio, b"synthetic audio bytes").expect("write synthetic audio");

        let source_sha256 = sha256_hex(std::fs::read(&audio).expect("read synthetic audio"));
        let previous = MeetingReview::new(
            meeting_transcript()
                .source_sha256(source_sha256.clone())
                .model("base")
                .duration(1.0)
                .segment(0.0, 1.0, "cached transcript", "SPEAKER_0")
                .speaker("SPEAKER_0", "SPEAKER_0")
                .build(),
        )
        .expect("synthetic review is valid");

        if with_cache {
            diarization_cache::save(&cache, &synthetic_cache(&source_sha256))
                .expect("save synthetic cache");
        }

        Self {
            _root: root,
            audio,
            cache,
            previous,
            glossary: Glossary::default(),
        }
    }

    fn root(&self) -> &Path {
        &self._root
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

fn assert_counts(backend: &CountingBackend, decode: usize, analyze: usize, full: usize) {
    assert_eq!(backend.decode_calls, decode, "unexpected decode call count");
    assert_eq!(
        backend.analyze_calls, analyze,
        "unexpected analyze call count"
    );
    assert_eq!(backend.full_calls, full, "unexpected full call count");
}

#[test]
fn selective_reprocessing_preserves_explicit_decoder_policy_and_acoustic_report() {
    let mut fixture = Fixture::new(true);
    let options = sagascript_core::transcription::TranscribeOptions {
        beam_size: 2,
        temperature_fallback: true,
        vad_model_path: Some("synthetic-vad-path".into()),
        segment_timestamps: true,
        ..Default::default()
    };
    let decoder: sagascript_core::diarization_report::DecoderEvidence =
        sagascript_core::transcription::whisper_backend::effective_diarization_decoder_config(
            &options,
        )
        .unwrap()
        .into();
    let analysis: DiarizationAnalysis =
        serde_json::from_str(r#"{"raw_segments":[],"embeddings":[],"region_activity_support":[]}"#)
            .unwrap();
    let config = CoreDiarizeConfig {
        exclusive_speech_embeddings: true,
        ..Default::default()
    };
    let mut report = analysis
        .report(
            &[],
            &config,
            vec![],
            true,
            fixture.previous.original.source_sha256.clone(),
            1.0,
            "synthetic-previous-build".into(),
            "test".into(),
        )
        .unwrap();
    report.decoder = Some(decoder.clone());
    fixture.previous = MeetingReview::new(
        fixture
            .previous
            .original
            .clone()
            .with_diarization(report)
            .unwrap(),
    )
    .unwrap();
    let identity = CacheIdentity::for_source_sha256_with_options(
        &fixture.previous.original.source_sha256,
        "en",
        "base",
        None,
        2,
        true,
        true,
    )
    .unwrap();
    let mut cache: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&fixture.cache).unwrap()).unwrap();
    cache["identity"] = serde_json::to_value(identity).unwrap();
    cache["analysis_identity"]["exclusive_speech_embeddings"] = true.into();
    cache["analysis"] = serde_json::to_value(analysis).unwrap();
    std::fs::write(&fixture.cache, serde_json::to_vec(&cache).unwrap()).unwrap();
    let input = fixture.input(Some(&fixture.cache));
    let plan = plan_reprocessing(&input, ReprocessingMode::Recluster, 0.75, None).unwrap();
    let mut backend = CountingBackend::default();
    let result = execute_with(&plan, &plan.revision, &input, None, None, &mut backend).unwrap();
    let proposed = &result.proposal.proposed;
    assert_eq!(proposed.schema_version, 2);
    let report = proposed.diarization.as_ref().unwrap();
    assert_eq!(report.decoder.as_ref(), Some(&decoder));
    assert!(report.parameters.exclusive_speech_embeddings);
    assert!(report.diagnostics_included);
    assert!(!report.transcript_modified);
    assert_eq!(report.asr_segments.len(), 1);
    assert_counts(&backend, 0, 0, 0);
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
        plan_reprocessing_with_model_check(&input, ReprocessingMode::Rediarize, 0.75, None, || {
            true
        })
        .expect("rediarize plan");
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

#[test]
fn speaker_hint_flows_through_plan_execution_and_persistence_in_every_mode() {
    use sagascript_core::meeting_reprocess_plan::ReprocessingPlan;
    use sagascript_core::meeting_reprocess_proposal::MeetingReprocessingProposal;
    use sagascript_core::speaker_hint::SpeakerCountHint;

    let hint = SpeakerCountHint::exact(2);
    let fixture = Fixture::new(true);
    let selective = fixture.input(Some(&fixture.cache));
    let full_input = fixture.input(None);
    for mode in [
        ReprocessingMode::Recluster,
        ReprocessingMode::Rediarize,
        ReprocessingMode::Full,
    ] {
        let input = if mode == ReprocessingMode::Full {
            &full_input
        } else {
            &selective
        };
        let plan = plan_reprocessing_with_model_check(input, mode, 0.75, None, || true)
            .expect("plan")
            .with_speaker_hint(Some(hint))
            .expect("hinted plan");
        // The plan is persisted and re-read before execution, as the CLI does.
        let plan: ReprocessingPlan =
            serde_json::from_str(&serde_json::to_string(&plan).expect("plan json"))
                .expect("plan reads back");
        assert_eq!(plan.speaker_hint, Some(hint));
        let mut backend = CountingBackend::default();
        let result = execute_with(&plan, &plan.revision, input, None, None, &mut backend)
            .expect("hinted execution");
        assert_eq!(
            backend.last_full_hint,
            (mode == ReprocessingMode::Full).then_some(hint),
            "{mode:?}"
        );
        let proposed = &result.proposal.proposed;
        assert_eq!(proposed.speaker_hint, Some(hint), "{mode:?}");
        if mode != ReprocessingMode::Full {
            // The synthetic analysis has no speech, so the requested 2 cannot be delivered.
            assert_eq!(proposed.speaker_hint_satisfied, Some(false), "{mode:?}");
            assert_eq!(proposed.speaker_hint_delivered, Some(0), "{mode:?}");
        }
        let reread: MeetingReprocessingProposal =
            serde_json::from_str(&serde_json::to_string(&result.proposal).expect("proposal json"))
                .expect("proposal reads back");
        assert_eq!(&reread, &result.proposal);
        // A plan without a hint is unchanged in revision and in bytes.
        let plain = plan_reprocessing_with_model_check(input, mode, 0.75, None, || true)
            .expect("plain plan");
        assert!(!serde_json::to_string(&plain)
            .expect("json")
            .contains("speaker_hint"));
        assert_ne!(plain.revision, plan.revision);
    }
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
fn missing_rediarization_models_fail_before_decode_or_analysis() {
    let fixture = Fixture::new(true);
    let input = fixture.input(Some(&fixture.cache));

    assert!(plan_reprocessing_with_model_check(
        &input,
        ReprocessingMode::Rediarize,
        0.75,
        None,
        || false,
    )
    .is_err());

    let plan =
        plan_reprocessing_with_model_check(&input, ReprocessingMode::Rediarize, 0.75, None, || {
            true
        })
        .expect("synthetic rediarize plan");
    let mut backend = CountingBackend::default();
    let error = execute_with_model_check(
        &plan,
        &plan.revision,
        &input,
        None,
        None,
        || false,
        &mut backend,
    )
    .expect_err("missing models must reject execution");
    assert!(error.to_string().contains("download-model diarization"));
    assert_counts(&backend, 0, 0, 0);
}

#[test]
fn cancellation_and_analysis_failure_return_no_proposal_or_review_mutation() {
    let fixture = Fixture::new(true);
    let input = fixture.input(Some(&fixture.cache));
    let plan =
        plan_reprocessing_with_model_check(&input, ReprocessingMode::Rediarize, 0.75, None, || {
            true
        })
        .expect("rediarize plan");
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

#[test]
fn cancellation_during_analysis_returns_no_proposal_or_review_mutation() {
    let fixture = Fixture::new(true);
    let input = fixture.input(Some(&fixture.cache));
    let plan =
        plan_reprocessing_with_model_check(&input, ReprocessingMode::Rediarize, 0.75, None, || {
            true
        })
        .expect("rediarize plan");
    let previous = fixture.previous.clone();
    let cancellation = AtomicBool::new(false);
    let progress = |_phase: MeetingPhase| {};
    let control = MeetingControl {
        cancellation: &cancellation,
        progress: &progress,
    };
    let mut backend = CountingBackend {
        cancel_during_analyze: true,
        ..Default::default()
    };

    assert!(execute_with(
        &plan,
        &plan.revision,
        &input,
        None,
        Some(&control),
        &mut backend,
    )
    .is_err());
    assert_counts(&backend, 1, 1, 0);
    assert_eq!(fixture.previous, previous);
}

// Publication of the new cache (#342). The fake `full` saves the cache where
// it is told to, as the real pipeline does, so these tests see exactly what a
// caller of `execute_with` leaves on disk when a run ends.

/// Every entry of `dir`, hidden ones included, sorted.
fn directory_names(dir: &Path) -> Vec<String> {
    let mut names = std::fs::read_dir(dir)
        .expect("list directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn full_plan(fixture: &Fixture) -> sagascript_core::meeting_reprocess_plan::ReprocessingPlan {
    plan_reprocessing(&fixture.input(None), ReprocessingMode::Full, 0.75, None).expect("full plan")
}

#[test]
fn failure_after_inference_leaves_no_cache_output_and_no_staging_file() {
    let fixture = Fixture::new(false);
    let input = fixture.input(None);
    let plan = full_plan(&fixture);
    let cache_output = fixture.root().join("new-cache.json");
    let mut backend = CountingBackend {
        write_cache: true,
        wrong_source_identity: true,
        ..Default::default()
    };

    let error = execute_with(
        &plan,
        &plan.revision,
        &input,
        Some(&cache_output),
        None,
        &mut backend,
    )
    .expect_err("inference returned a different recording");

    assert!(
        error.to_string().contains("different source identity"),
        "{error}"
    );
    assert_counts(&backend, 0, 0, 1);
    assert_eq!(directory_names(fixture.root()), ["synthetic-audio.bin"]);
}

#[test]
fn rerun_after_a_failure_before_publication_reuses_the_same_cache_output() {
    let fixture = Fixture::new(false);
    let input = fixture.input(None);
    let plan = full_plan(&fixture);
    let cache_output = fixture.root().join("new-cache.json");
    let mut failing = CountingBackend {
        write_cache: true,
        wrong_source_identity: true,
        ..Default::default()
    };
    execute_with(
        &plan,
        &plan.revision,
        &input,
        Some(&cache_output),
        None,
        &mut failing,
    )
    .expect_err("first run fails after inference");

    let mut retry = CountingBackend {
        write_cache: true,
        ..Default::default()
    };
    execute_with(
        &plan,
        &plan.revision,
        &input,
        Some(&cache_output),
        None,
        &mut retry,
    )
    .expect("the identical command succeeds on the second attempt");

    assert_counts(&retry, 0, 0, 1);
    assert_eq!(
        directory_names(fixture.root()),
        ["new-cache.json", "synthetic-audio.bin"]
    );
    let identity = CacheIdentity::for_source_sha256(
        &fixture.previous.original.source_sha256,
        "en",
        "base",
        None,
    )
    .expect("identity");
    assert!(matches!(
        diarization_cache::load_with_analysis_policy(&cache_output, &identity, false),
        Ok(diarization_cache::CacheLookup::Hit(_))
    ));
}

#[test]
fn cancellation_after_the_cache_was_written_leaves_no_cache_output() {
    let fixture = Fixture::new(false);
    let input = fixture.input(None);
    let plan = full_plan(&fixture);
    let cache_output = fixture.root().join("new-cache.json");
    let cancellation = AtomicBool::new(false);
    let progress = |_phase: MeetingPhase| {};
    let control = MeetingControl {
        cancellation: &cancellation,
        progress: &progress,
    };
    let mut backend = CountingBackend {
        write_cache: true,
        cancel_after_full: true,
        ..Default::default()
    };

    execute_with(
        &plan,
        &plan.revision,
        &input,
        Some(&cache_output),
        Some(&control),
        &mut backend,
    )
    .expect_err("cancelled before finalizing");

    assert_eq!(directory_names(fixture.root()), ["synthetic-audio.bin"]);
}

#[test]
fn existing_cache_output_is_refused_before_inference_and_left_untouched() {
    let fixture = Fixture::new(false);
    let input = fixture.input(None);
    let plan = full_plan(&fixture);
    let cache_output = fixture.root().join("existing-cache.json");
    std::fs::write(&cache_output, b"user data").expect("existing file");
    let mut backend = CountingBackend {
        write_cache: true,
        ..Default::default()
    };

    let error = execute_with(
        &plan,
        &plan.revision,
        &input,
        Some(&cache_output),
        None,
        &mut backend,
    )
    .expect_err("an existing output is never replaced");

    assert!(error.to_string().contains("already exists"), "{error}");
    assert_counts(&backend, 0, 0, 0);
    assert_eq!(std::fs::read(&cache_output).expect("read"), b"user data");
    assert_eq!(
        directory_names(fixture.root()),
        ["existing-cache.json", "synthetic-audio.bin"]
    );
}

/// A full run on the fake pipeline, validated, with both outputs staged and
/// nothing published yet.
fn staged_full_run(fixture: &Fixture, cache_output: &Path, proposal: &Path) -> PendingPublication {
    let plan = full_plan(fixture);
    let mut backend = CountingBackend {
        write_cache: true,
        ..Default::default()
    };
    execute_with_model_check(
        &plan,
        &plan.revision,
        &fixture.input(None),
        Some(cache_output),
        None,
        || true,
        &mut backend,
    )
    .expect("validated run")
    .stage_proposal(proposal)
    .expect("staged proposal")
}

#[test]
fn staging_publishes_nothing_and_commit_publishes_the_cache_then_the_proposal() {
    let fixture = Fixture::new(false);
    let cache_output = fixture.root().join("new-cache.json");
    let proposal = fixture.root().join("proposal.json");

    let pending = staged_full_run(&fixture, &cache_output, &proposal);
    assert_eq!(
        directory_names(fixture.root())
            .iter()
            .filter(|name| !name.starts_with('.'))
            .collect::<Vec<_>>(),
        ["synthetic-audio.bin"],
        "staged outputs are not visible under their final names"
    );

    let receipt = pending.commit().expect("commit");

    assert_eq!(receipt.cache.as_deref(), Some(cache_output.as_path()));
    assert_eq!(receipt.proposal, proposal);
    assert!(receipt.leftovers.is_empty());
    assert_eq!(
        directory_names(fixture.root()),
        ["new-cache.json", "proposal.json", "synthetic-audio.bin"]
    );
    let published = crate::meeting_proposal::read_proposal(&proposal).expect("valid proposal");
    assert_eq!(published, receipt_proposal(&fixture));
}

fn receipt_proposal(
    fixture: &Fixture,
) -> sagascript_core::meeting_reprocess_proposal::MeetingReprocessingProposal {
    sagascript_core::meeting_reprocess_proposal::MeetingReprocessingProposal::new(
        fixture.previous.clone(),
        fixture.previous.original.clone(),
    )
    .expect("the proposal of the fake run")
}

#[test]
fn a_failed_first_publication_publishes_nothing() {
    let fixture = Fixture::new(false);
    let cache_output = fixture.root().join("new-cache.json");
    let proposal = fixture.root().join("proposal.json");
    let pending = staged_full_run(&fixture, &cache_output, &proposal);

    let error = pending
        .commit_with(&FaultyFs::new().failing_link(1))
        .expect_err("the cache link fails");

    assert!(
        matches!(error, CommitError::NothingPublished(_)),
        "{error:?}"
    );
    assert!(error
        .to_string()
        .contains("Failed to commit new diarization cache"));
    assert_eq!(directory_names(fixture.root()), ["synthetic-audio.bin"]);
}

#[test]
fn a_failed_second_publication_leaves_a_complete_cache_and_no_proposal() {
    let fixture = Fixture::new(false);
    let cache_output = fixture.root().join("new-cache.json");
    let proposal = fixture.root().join("proposal.json");
    let pending = staged_full_run(&fixture, &cache_output, &proposal);

    let error = pending
        .commit_with(&FaultyFs::new().failing_link(2))
        .expect_err("the proposal link fails");

    let CommitError::CacheOnly {
        cache,
        proposal: missing,
        cause,
    } = error
    else {
        panic!("the cache was already published");
    };
    assert_eq!(
        (cache.as_path(), missing.as_path()),
        (cache_output.as_path(), proposal.as_path())
    );
    assert!(
        cause.to_string().contains("could not be published"),
        "{cause}"
    );
    // The cache is the whole partial result: complete, loadable, and alone.
    assert_eq!(
        directory_names(fixture.root()),
        ["new-cache.json", "synthetic-audio.bin"]
    );
    let identity = CacheIdentity::for_source_sha256(
        &fixture.previous.original.source_sha256,
        "en",
        "base",
        None,
    )
    .expect("identity");
    assert!(matches!(
        diarization_cache::load_with_analysis_policy(&cache_output, &identity, false),
        Ok(diarization_cache::CacheLookup::Hit(_))
    ));
}

#[test]
fn the_cache_left_by_a_failed_second_publication_recovers_the_proposal_without_inference() {
    let fixture = Fixture::new(false);
    let cache_output = fixture.root().join("new-cache.json");
    let proposal = fixture.root().join("proposal.json");
    staged_full_run(&fixture, &cache_output, &proposal)
        .commit_with(&FaultyFs::new().failing_link(2))
        .expect_err("the proposal link fails");

    // The documented recovery: a selective plan from the published cache.
    let input = fixture.input(Some(&cache_output));
    let plan = plan_reprocessing(&input, ReprocessingMode::Recluster, 0.75, None)
        .expect("the published cache is a valid selective input");
    let mut backend = CountingBackend::default();
    let receipt = execute_with_model_check(
        &plan,
        &plan.revision,
        &input,
        None,
        None,
        || true,
        &mut backend,
    )
    .expect("selective run")
    .stage_proposal(&proposal)
    .expect("stage")
    .commit()
    .expect("the same proposal path is free, so recovery publishes it");

    assert_counts(&backend, 0, 0, 0);
    assert_eq!(receipt.cache, None);
    assert_eq!(
        directory_names(fixture.root()),
        ["new-cache.json", "proposal.json", "synthetic-audio.bin"]
    );
    crate::meeting_proposal::read_proposal(&proposal).expect("valid recovered proposal");
}

#[test]
fn a_proposal_destination_that_appears_before_the_commit_publishes_nothing() {
    let fixture = Fixture::new(false);
    let cache_output = fixture.root().join("new-cache.json");
    let proposal = fixture.root().join("proposal.json");
    let pending = staged_full_run(&fixture, &cache_output, &proposal);
    std::fs::write(&proposal, b"someone else's file").expect("their file");

    let error = pending.commit().expect_err("the destination is taken");

    assert!(
        matches!(error, CommitError::NothingPublished(_)),
        "{error:?}"
    );
    assert!(error.to_string().contains("already exists"), "{error}");
    assert_eq!(
        std::fs::read(&proposal).expect("kept"),
        b"someone else's file"
    );
    assert_eq!(
        directory_names(fixture.root()),
        ["proposal.json", "synthetic-audio.bin"]
    );
}

#[test]
fn a_cache_destination_that_appears_before_the_commit_publishes_nothing() {
    let fixture = Fixture::new(false);
    let cache_output = fixture.root().join("new-cache.json");
    let proposal = fixture.root().join("proposal.json");
    let pending = staged_full_run(&fixture, &cache_output, &proposal);
    std::fs::write(&cache_output, b"someone else's file").expect("their file");

    let error = pending
        .commit()
        .expect_err("the cache destination is taken");

    assert!(
        matches!(error, CommitError::NothingPublished(_)),
        "{error:?}"
    );
    assert!(error.to_string().contains("already exists"), "{error}");
    assert_eq!(
        std::fs::read(&cache_output).expect("kept"),
        b"someone else's file"
    );
    assert_eq!(
        directory_names(fixture.root()),
        ["new-cache.json", "synthetic-audio.bin"],
        "no proposal, and the staged proposal is removed"
    );
}

#[test]
fn dropping_staged_outputs_removes_them() {
    let fixture = Fixture::new(false);
    let pending = staged_full_run(
        &fixture,
        &fixture.root().join("new-cache.json"),
        &fixture.root().join("proposal.json"),
    );
    assert_eq!(
        directory_names(fixture.root()).len(),
        3,
        "audio and two staging files"
    );

    drop(pending);

    assert_eq!(directory_names(fixture.root()), ["synthetic-audio.bin"]);
}

#[test]
fn a_staging_file_that_cannot_be_removed_is_reported_and_both_outputs_are_published() {
    let fixture = Fixture::new(false);
    let pending = staged_full_run(
        &fixture,
        &fixture.root().join("new-cache.json"),
        &fixture.root().join("proposal.json"),
    );
    let disk = FaultyFs::new().failing_remove();
    let receipt = pending.commit_with(&disk).expect("both are published");

    assert_eq!(receipt.leftovers.len(), 2);
    assert!(receipt.cache.is_some());
    for output in ["new-cache.json", "proposal.json"] {
        assert!(fixture.root().join(output).is_file(), "{output}");
    }
}

#[test]
fn a_run_killed_between_the_publications_is_detectable_and_its_staged_proposal_usable() {
    let fixture = Fixture::new(false);
    let cache_output = fixture.root().join("new-cache.json");
    let proposal = fixture.root().join("proposal.json");
    let PendingPublication {
        cache,
        proposal: staged_proposal,
        ..
    } = staged_full_run(&fixture, &cache_output, &proposal);
    cache
        .expect("a cache was requested")
        .publish()
        .expect("first publication");
    // A kill never runs destructors: the staged proposal stays behind.
    let survivor = staged_proposal.staged_path().to_path_buf();
    std::mem::forget(staged_proposal);

    // Detection: the cache exists, the proposal does not, and the survivor is listed.
    assert!(cache_output.is_file() && !proposal.exists());
    assert_eq!(
        sagascript_core::fsutil::staging_leftovers(&proposal),
        std::slice::from_ref(&survivor)
    );
    // Recovery by hand: it was complete before the cache was published.
    crate::meeting_proposal::read_proposal(&survivor).expect("the survivor is a valid proposal");
    std::fs::rename(&survivor, &proposal).expect("rename the survivor");
    crate::meeting_proposal::read_proposal(&proposal).expect("valid proposal");
}

#[test]
fn a_cache_output_that_appears_during_inference_is_never_replaced_by_finish() {
    let fixture = Fixture::new(false);
    let input = fixture.input(None);
    let plan = full_plan(&fixture);
    let cache_output = fixture.root().join("new-cache.json");
    let taken = cache_output.clone();
    let mut backend = CountingBackend {
        write_cache: true,
        during_full: Some(Box::new(move || {
            std::fs::write(&taken, b"someone else's file").expect("their file");
        })),
        ..Default::default()
    };

    let error = execute_with(
        &plan,
        &plan.revision,
        &input,
        Some(&cache_output),
        None,
        &mut backend,
    )
    .expect_err("the destination is taken");

    assert!(error.to_string().contains("already exists"), "{error}");
    assert_eq!(
        std::fs::read(&cache_output).expect("kept"),
        b"someone else's file"
    );
    assert_eq!(
        directory_names(fixture.root()),
        ["new-cache.json", "synthetic-audio.bin"]
    );
}

#[test]
fn a_run_that_produced_no_cache_publishes_nothing() {
    let fixture = Fixture::new(false);
    let input = fixture.input(None);
    let plan = full_plan(&fixture);
    let cache_output = fixture.root().join("new-cache.json");
    let mut backend = CountingBackend::default();

    let error = execute_with(
        &plan,
        &plan.revision,
        &input,
        Some(&cache_output),
        None,
        &mut backend,
    )
    .expect_err("the fake wrote no cache");

    assert!(
        error
            .to_string()
            .contains("did not produce the requested cache"),
        "{error}"
    );
    assert_eq!(directory_names(fixture.root()), ["synthetic-audio.bin"]);
}

#[test]
fn finish_keeps_the_validated_result_when_only_the_staging_file_cannot_be_removed() {
    use sagascript_core::test_support::FaultyFs;
    let fixture = Fixture::new(false);
    let input = fixture.input(None);
    let plan = full_plan(&fixture);
    let cache_output = fixture.root().join("new-cache.json");
    let mut backend = CountingBackend {
        write_cache: true,
        ..Default::default()
    };
    let staged = execute_with_model_check(
        &plan,
        &plan.revision,
        &input,
        Some(&cache_output),
        None,
        || true,
        &mut backend,
    )
    .expect("validated run");

    let result = staged
        .finish_with(&FaultyFs::new().failing_remove())
        .expect("the cache is published, so the validated proposal is kept");

    assert_eq!(result.proposal, receipt_proposal(&fixture));
    let identity = CacheIdentity::for_source_sha256(
        &fixture.previous.original.source_sha256,
        "en",
        "base",
        None,
    )
    .expect("identity");
    assert!(matches!(
        diarization_cache::load_with_analysis_policy(&cache_output, &identity, false),
        Ok(diarization_cache::CacheLookup::Hit(_))
    ));
    // The rerun with the same path is refused only because the cache exists now.
    assert!(directory_names(fixture.root()).contains(&"new-cache.json".to_string()));
}

/// Everything `tracing` logs while `work` runs, as text.
fn logged_while(work: impl FnOnce()) -> String {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);
    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("log buffer").extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let buffer = Buffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, work);
    let logged = buffer.0.lock().expect("log buffer").clone();
    String::from_utf8_lossy(&logged).into_owned()
}

#[test]
fn a_staging_file_left_after_finish_is_logged_without_the_users_path() {
    use sagascript_core::test_support::FaultyFs;
    let fixture = Fixture::new(false);
    let input = fixture.input(None);
    let plan = full_plan(&fixture);
    let cache_output = fixture.root().join("new-cache.json");
    let mut backend = CountingBackend {
        write_cache: true,
        ..Default::default()
    };
    let staged = execute_with_model_check(
        &plan,
        &plan.revision,
        &input,
        Some(&cache_output),
        None,
        || true,
        &mut backend,
    )
    .expect("validated run");

    let logged = logged_while(|| {
        staged
            .finish_with(&FaultyFs::new().failing_remove())
            .expect("published");
    });

    assert!(logged.contains("WARN"), "{logged}");
    assert!(logged.contains("could not be removed"), "{logged}");
    let root = fixture.root().display().to_string();
    assert!(!logged.contains(&root), "{logged}");
}
