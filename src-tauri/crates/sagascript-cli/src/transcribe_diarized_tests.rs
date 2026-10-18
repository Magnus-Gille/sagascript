//! The diarized path of a file run, driven by a scripted backend and a
//! scripted analyzer (#343): the cache under `CachePolicy::Normal` and
//! `RefreshNew`, the analysis that fills it, and what a failure or a cancel
//! leaves behind.
//!
//! No ONNX models and no Whisper weights. The audio is a synthetic WAV the real
//! decoder reads, the cache is a real file in a temporary directory, and the
//! clustering, merging and assembling are the real ones, run over a canned
//! two-speaker analysis. The process-level contract (the real binary, exit
//! status, error codes) is in `tests/transcribe_contract.rs`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sagascript_core::diarization::embedding::EMBEDDING_DIM;
use sagascript_core::diarization::{DiarizationAnalysis, DiarizationTimings, DiarizeConfig};
use sagascript_core::transcription::DiarizationTranscriptionTimings;

use super::*;
use crate::diarization_cache::{load, CacheIdentity, CacheLookup};
use crate::transcribe::diarize::{
    analyze_in_sequence, analyze_overlapped, AnalysisOutcome, DiarizationAnalyzer,
};

/// Long enough for two speakers of five and six seconds.
const SECONDS: usize = 12;

// ---- the scripted analyzer ----------------------------------------------------

/// Two speakers, 0-6 s and 6-11 s, with orthogonal embeddings: the clustering
/// keeps them apart at the default threshold.
fn two_speakers() -> DiarizationAnalysis {
    let mut first = vec![0.0_f32; EMBEDDING_DIM];
    first[0] = 1.0;
    let mut second = vec![0.0_f32; EMBEDDING_DIM];
    second[1] = 1.0;
    serde_json::from_value(json!({
        "raw_segments": [[0.0, 6.0, 0], [6.0, 11.0, 1]],
        "embeddings": [[0, first], [1, second]],
    }))
    .expect("the canned analysis parses")
}

/// What Whisper says in the diarized run: one word per speaker, and the
/// coarse segment coverage is judged on.
fn words() -> DiarizationTranscription {
    DiarizationTranscription {
        segments: vec![
            (0.5, 1.0, "hello".to_string()),
            (6.5, 7.0, "there".to_string()),
        ],
        coverage_segments: vec![spoken(0.0, 11.0, " hello there")],
        timings: DiarizationTranscriptionTimings::default(),
    }
}

struct ScriptedAnalyzer {
    installed: bool,
    failures: Mutex<VecDeque<DictationError>>,
    /// Run when the analysis is asked, before it answers.
    during: Mutex<Option<Hook>>,
    /// The samples of every analysis asked for, in order.
    asked: Mutex<Vec<usize>>,
}

impl ScriptedAnalyzer {
    fn new() -> Self {
        Self {
            installed: true,
            failures: Mutex::new(VecDeque::new()),
            during: Mutex::new(None),
            asked: Mutex::new(Vec::new()),
        }
    }

    fn without_models(mut self) -> Self {
        self.installed = false;
        self
    }

    fn failing(self, error: DictationError) -> Self {
        self.failures.lock().unwrap().push_back(error);
        self
    }

    fn during(self, hook: impl FnOnce() -> Result<(), DictationError> + Send + 'static) -> Self {
        *self.during.lock().unwrap() = Some(Box::new(hook));
        self
    }

    /// How many analyses were asked for.
    fn calls(&self) -> usize {
        self.asked.lock().unwrap().len()
    }
}

impl DiarizationAnalyzer for ScriptedAnalyzer {
    fn models_downloaded(&self) -> bool {
        self.installed
    }

    fn analyze(
        &self,
        audio: &[f32],
        _: &DiarizeConfig,
    ) -> Result<(DiarizationAnalysis, DiarizationTimings), DictationError> {
        self.asked.lock().unwrap().push(audio.len());
        let during = self.during.lock().unwrap().take();
        if let Some(during) = during {
            during()?;
        }
        let failure = self.failures.lock().unwrap().pop_front();
        match failure {
            Some(error) => Err(error),
            None => Ok((two_speakers(), DiarizationTimings::default())),
        }
    }
}

// ---- a run --------------------------------------------------------------------

/// A recording and the cache path of its runs, in a temporary directory.
struct Fixture {
    _dir: TempDir,
    file: PathBuf,
    cache: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new();
        let file = tone(&dir, "meeting.wav", SECONDS);
        let cache = dir.join("cache.json");
        Self {
            _dir: dir,
            file,
            cache,
        }
    }

    /// The names in the directory, sorted: the recording, the cache, and
    /// nothing left over from a write.
    fn files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.file.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn cache_bytes(&self) -> Vec<u8> {
        std::fs::read(&self.cache).expect("the cache exists")
    }

    /// What the cache says about a run of `model` on this recording.
    fn lookup(&self, model: WhisperModel) -> CacheLookup {
        let identity = CacheIdentity::for_input_with_options(
            &self.file,
            "en",
            model_id_string(model),
            None,
            0,
            true,
            false,
        )
        .expect("the identity of the recording");
        load(&self.cache, &identity).expect("the cache loads")
    }
}

/// One diarized run: which model, which cache policy, and whether
/// `--diarize-cache` is given.
#[derive(Clone, Copy)]
struct Job {
    model: WhisperModel,
    policy: CachePolicy,
    with_cache: bool,
}

impl Job {
    fn normal() -> Self {
        Self {
            model: WhisperModel::TinyEn,
            policy: CachePolicy::Normal,
            with_cache: true,
        }
    }

    fn refresh() -> Self {
        Self {
            policy: CachePolicy::RefreshNew,
            ..Self::normal()
        }
    }

    fn with_model(self, model: WhisperModel) -> Self {
        Self { model, ..self }
    }

    fn without_cache(self) -> Self {
        Self {
            with_cache: false,
            ..self
        }
    }
}

struct Diarized {
    result: Result<FileTranscription, DictationError>,
    stderr: String,
}

impl Diarized {
    fn json(self) -> Value {
        self.result.expect("the run succeeds").json
    }
}

fn diarize(
    fx: &Fixture,
    job: Job,
    backend: &ScriptedBackend,
    analyzer: &ScriptedAnalyzer,
    control: Option<&MeetingControl<'_>>,
) -> Diarized {
    let cache = fx.cache.display().to_string();
    let flags: &[&str] = if job.with_cache {
        &["--diarize", "--diarize-cache", &cache]
    } else {
        &["--diarize"]
    };
    let args = args_for(flags);
    let (out, captured) = Out::capture();
    let mut model_loaded = false;
    let result = transcribe_file(
        &FileRequest {
            args: &args,
            file: &fx.file,
            stored: &Settings::default(),
            language: Language::English,
            model: job.model,
            glossary: &Glossary::default(),
            correction_vocabulary: &[],
            control,
            cache_policy: job.policy,
        },
        Services { backend, analyzer },
        &mut model_loaded,
        &out,
    );
    Diarized {
        result,
        stderr: captured.stderr(),
    }
}

fn run(fx: &Fixture, job: Job, backend: &ScriptedBackend, analyzer: &ScriptedAnalyzer) -> Diarized {
    diarize(fx, job, backend, analyzer, None)
}

/// A backend that answers the Whisper half `times` times.
fn whisper(times: usize) -> ScriptedBackend {
    ScriptedBackend::new(Vec::new()).diarizing((0..times).map(|_| Ok(words())).collect())
}

fn without_performance(mut json: Value) -> Value {
    json.as_object_mut()
        .expect("the result is an object")
        .remove("performance");
    json
}

/// Whether stderr says anything about the diarization cache (the
/// `cache_hit=` of the closing line is not that).
fn mentions_the_cache(stderr: &str) -> bool {
    stderr.to_lowercase().contains("diarization cache")
}

/// The index of the first stderr line that contains `needle`.
fn line_of(stderr: &str, needle: &str) -> usize {
    stderr
        .lines()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no line contains {needle:?}:\n{stderr}"))
}

fn seed(fx: &Fixture, model: WhisperModel) {
    let outcome = run(
        fx,
        Job::normal().with_model(model),
        &whisper(1),
        &ScriptedAnalyzer::new(),
    );
    outcome.result.expect("the seeding run succeeds");
}

// ---- CachePolicy::Normal ---------------------------------------------------------

#[test]
fn a_miss_computes_the_analysis_and_writes_the_cache_and_the_next_run_reads_it() {
    let fx = Fixture::new();
    let backend = whisper(1);
    let analyzer = ScriptedAnalyzer::new();

    let first = run(&fx, Job::normal(), &backend, &analyzer);
    let miss = format!("Saved reusable diarization cache: {}", fx.cache.display());
    assert!(
        line_of(
            &first.stderr,
            "Diarization cache miss (not found); computing analysis."
        ) < line_of(&first.stderr, "Decoding")
    );
    assert!(
        line_of(
            &first.stderr,
            "Running speaker diarization and Whisper timestamps"
        ) < line_of(&first.stderr, &miss)
    );
    let first = first.json();
    assert_eq!(analyzer.calls(), 1);
    assert_eq!(first["performance"]["cache_hit"], false);
    assert_eq!(first["speakers"].as_array().unwrap().len(), 2);
    assert!(matches!(
        fx.lookup(WhisperModel::TinyEn),
        CacheLookup::Hit(_)
    ));
    let calls_after_the_miss = backend.calls().len();

    let second = run(&fx, Job::normal(), &backend, &analyzer);
    let reuse = format!("Reusing diarization cache: {}", fx.cache.display());
    assert!(second.stderr.contains(&reuse), "{}", second.stderr);
    assert!(second.stderr.contains("Audio (cached): 12.0s"));
    assert!(!second.stderr.contains("Decoding"));
    assert!(!second.stderr.contains("Saved reusable"));
    let second = second.json();
    assert_eq!(second["performance"]["cache_hit"], true);
    assert_eq!(analyzer.calls(), 1, "a hit does not analyse again");
    assert_eq!(
        backend.calls().len(),
        calls_after_the_miss,
        "a hit loads no model, checks no language and transcribes nothing"
    );
    assert_eq!(without_performance(first), without_performance(second));
}

#[test]
fn a_cache_written_for_another_model_is_a_miss_and_is_replaced() {
    let fx = Fixture::new();
    seed(&fx, WhisperModel::TinyEn);
    let tiny = fx.cache_bytes();
    let backend = whisper(1);
    let analyzer = ScriptedAnalyzer::new();

    let run = run(
        &fx,
        Job::normal().with_model(WhisperModel::Base),
        &backend,
        &analyzer,
    );

    assert!(run.stderr.contains(
        "Diarization cache miss (input, model, language, prompt, or schema changed); computing analysis."
    ));
    assert!(run.stderr.contains(&format!(
        "Saved reusable diarization cache: {}",
        fx.cache.display()
    )));
    assert_eq!(run.json()["performance"]["cache_hit"], false);
    assert_eq!(analyzer.calls(), 1);
    assert_ne!(
        fx.cache_bytes(),
        tiny,
        "the miss is written over the old cache"
    );
    assert!(matches!(fx.lookup(WhisperModel::Base), CacheLookup::Hit(_)));
    assert!(matches!(
        fx.lookup(WhisperModel::TinyEn),
        CacheLookup::Miss(_)
    ));
    assert_eq!(fx.files(), ["cache.json", "meeting.wav"]);
}

#[test]
fn without_a_cache_flag_nothing_is_read_or_written() {
    let fx = Fixture::new();
    let analyzer = ScriptedAnalyzer::new();

    let run = run(&fx, Job::normal().without_cache(), &whisper(1), &analyzer);

    assert!(!mentions_the_cache(&run.stderr), "{}", run.stderr);
    assert_eq!(run.json()["performance"]["cache_hit"], false);
    assert_eq!(analyzer.calls(), 1);
    assert_eq!(fx.files(), ["meeting.wav"]);
}

// ---- CachePolicy::RefreshNew --------------------------------------------------------

#[test]
fn a_refresh_creates_the_cache_when_nothing_is_there_and_says_nothing_about_it() {
    let fx = Fixture::new();
    let analyzer = ScriptedAnalyzer::new();

    let run = run(&fx, Job::refresh(), &whisper(1), &analyzer);

    // The caller that asked for a refresh publishes the cache, and says so.
    assert!(!mentions_the_cache(&run.stderr), "{}", run.stderr);
    assert_eq!(run.json()["performance"]["cache_hit"], false);
    assert_eq!(analyzer.calls(), 1);
    let CacheLookup::Hit(cache) = fx.lookup(WhisperModel::TinyEn) else {
        panic!("the refresh left a cache the next run can read");
    };
    assert_eq!(cache.transcript, words().segments);
    assert_eq!(cache.coverage_segments.as_ref().map(Vec::len), Some(1));
    assert_eq!(fx.files(), ["cache.json", "meeting.wav"]);
}

#[test]
fn a_refresh_never_reads_a_matching_cache_and_does_not_replace_it() {
    let fx = Fixture::new();
    seed(&fx, WhisperModel::TinyEn);
    let seeded = fx.cache_bytes();
    let analyzer = ScriptedAnalyzer::new();

    let refreshed = run(&fx, Job::refresh(), &whisper(1), &analyzer);

    // It recomputed (the cache would have been a hit) ...
    assert_eq!(analyzer.calls(), 1);
    assert!(
        !refreshed.stderr.contains("Reusing"),
        "{}",
        refreshed.stderr
    );
    assert!(
        !refreshed.stderr.contains("cache miss"),
        "{}",
        refreshed.stderr
    );
    // ... and then refused to write over what was there.
    let error = refreshed
        .result
        .expect_err("an existing cache is not replaced");
    assert!(
        error
            .to_string()
            .contains("Diarization cache output already exists"),
        "{error}"
    );
    assert_eq!(fx.cache_bytes(), seeded);
    assert_eq!(fx.files(), ["cache.json", "meeting.wav"]);
}

// ---- failures and cancels leave the previous cache --------------------------------

/// Seeds a cache, then runs `job` over it with a Base model (a miss under
/// Normal; ignored by RefreshNew) and a failure injected by `arrange`.
fn a_run_that_fails_leaves_the_previous_cache(
    job: Job,
    arrange: impl FnOnce() -> (ScriptedBackend, ScriptedAnalyzer),
    expected: &str,
) {
    let fx = Fixture::new();
    seed(&fx, WhisperModel::TinyEn);
    let seeded = fx.cache_bytes();
    let (backend, analyzer) = arrange();

    let run = run(&fx, job.with_model(WhisperModel::Base), &backend, &analyzer);

    let error = run.result.expect_err("the run fails");
    assert!(error.to_string().contains(expected), "{error}");
    assert_eq!(fx.cache_bytes(), seeded, "the previous cache is as it was");
    assert!(matches!(
        fx.lookup(WhisperModel::TinyEn),
        CacheLookup::Hit(_)
    ));
    assert_eq!(
        fx.files(),
        ["cache.json", "meeting.wav"],
        "no staging file is left"
    );
}

#[test]
fn a_failed_analysis_leaves_the_previous_cache_as_it_was() {
    for job in [Job::normal(), Job::refresh()] {
        a_run_that_fails_leaves_the_previous_cache(
            job,
            || {
                (
                    whisper(1),
                    ScriptedAnalyzer::new().failing(DictationError::DiarizationError(
                        "segmentation model crashed".to_string(),
                    )),
                )
            },
            "segmentation model crashed",
        );
    }
}

#[test]
fn a_failed_whisper_half_leaves_the_previous_cache_as_it_was() {
    for job in [Job::normal(), Job::refresh()] {
        a_run_that_fails_leaves_the_previous_cache(
            job,
            || {
                (
                    ScriptedBackend::new(Vec::new()).diarizing(vec![Err(
                        DictationError::TranscriptionFailed(
                            "whisper ran out of memory".to_string(),
                        ),
                    )]),
                    ScriptedAnalyzer::new(),
                )
            },
            "whisper ran out of memory",
        );
    }
}

#[test]
fn a_cancel_during_the_analysis_stops_before_the_cache_is_written() {
    for job in [Job::normal(), Job::refresh()] {
        // A fresh cache path: nothing may appear.
        let fx = Fixture::new();
        let cancelled = Arc::new(AtomicBool::new(false));
        let analyzer = ScriptedAnalyzer::new().during({
            let cancelled = cancelled.clone();
            move || {
                cancelled.store(true, Ordering::Release);
                Ok(())
            }
        });
        let phases = Mutex::new(Vec::new());
        let progress = |phase| phases.lock().unwrap().push(phase);
        let control = MeetingControl {
            cancellation: &cancelled,
            progress: &progress,
        };

        let run = diarize(&fx, job, &whisper(1), &analyzer, Some(&control));

        assert!(
            matches!(run.result, Err(DictationError::Cancelled)),
            "{:?}",
            run.result
        );
        assert_eq!(analyzer.calls(), 1);
        assert_eq!(
            fx.files(),
            ["meeting.wav"],
            "a cancelled run writes no cache"
        );
        let phases = phases.lock().unwrap();
        assert_eq!(phases.last(), Some(&MeetingPhase::Analyzing));
        assert!(!phases.contains(&MeetingPhase::Clustering), "{phases:?}");
        assert!(
            cancelled.load(Ordering::Acquire),
            "the cancel is never cleared"
        );
    }
}

#[test]
fn a_cancel_during_the_analysis_keeps_a_previous_cache() {
    let fx = Fixture::new();
    seed(&fx, WhisperModel::TinyEn);
    let seeded = fx.cache_bytes();
    let cancelled = Arc::new(AtomicBool::new(false));
    let analyzer = ScriptedAnalyzer::new().during({
        let cancelled = cancelled.clone();
        move || {
            cancelled.store(true, Ordering::Release);
            Ok(())
        }
    });
    let progress = |_| {};
    let control = MeetingControl {
        cancellation: &cancelled,
        progress: &progress,
    };

    let run = diarize(
        &fx,
        Job::normal().with_model(WhisperModel::Base),
        &whisper(1),
        &analyzer,
        Some(&control),
    );

    assert!(matches!(run.result, Err(DictationError::Cancelled)));
    assert_eq!(fx.cache_bytes(), seeded);
    assert_eq!(fx.files(), ["cache.json", "meeting.wav"]);
}

// ---- the models -----------------------------------------------------------------------

#[test]
fn a_miss_needs_the_models_but_a_hit_does_not() {
    let fx = Fixture::new();
    let analyzer = ScriptedAnalyzer::new().without_models();

    let miss = run(&fx, Job::normal(), &whisper(1), &analyzer);

    assert!(miss
        .stderr
        .contains("Diarization cache miss (not found); computing analysis."));
    assert!(!miss.stderr.contains("Decoding"), "the file is not decoded");
    let error = miss.result.expect_err("a miss without the models fails");
    assert_eq!(
        error.to_string(),
        "Diarization error: Diarization models not found. Run: sagascript download-model diarization"
    );
    assert_eq!(analyzer.calls(), 0);
    assert_eq!(fx.files(), ["meeting.wav"]);

    seed(&fx, WhisperModel::TinyEn);
    let hit = run(&fx, Job::normal(), &whisper(0), &analyzer);
    assert_eq!(hit.json()["performance"]["cache_hit"], true);
    assert_eq!(analyzer.calls(), 0);
}

// ---- the two halves of the analysis ------------------------------------------------------

/// `analyze_overlapped` or `analyze_in_sequence`.
type Strategy = fn(
    &[f32],
    Services<'_>,
    Language,
    Option<&str>,
    &DiarizeConfig,
    &TranscribeOptions,
) -> Result<AnalysisOutcome, DictationError>;

fn analysis_outcome(
    strategy: Strategy,
    backend: &ScriptedBackend,
    analyzer: &ScriptedAnalyzer,
) -> Result<AnalysisOutcome, DictationError> {
    strategy(
        &[0.1; 1_600],
        Services { backend, analyzer },
        Language::English,
        None,
        &DiarizeConfig::default(),
        &TranscribeOptions::default(),
    )
}

/// Waits (up to `limit`) for the other half to have started, having said that
/// this one has: a rendezvous that only happens when the two overlap.
fn rendezvous(
    mine: &AtomicBool,
    theirs: &AtomicBool,
    limit: Duration,
) -> Result<(), DictationError> {
    mine.store(true, Ordering::SeqCst);
    let deadline = Instant::now() + limit;
    while !theirs.load(Ordering::SeqCst) {
        if Instant::now() > deadline {
            return Err(DictationError::TranscriptionFailed(
                "the other half did not start".to_string(),
            ));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

/// A backend and an analyzer whose halves wait for each other.
fn halves_that_wait_for_each_other(limit: Duration) -> (ScriptedBackend, ScriptedAnalyzer) {
    let analysis_started = Arc::new(AtomicBool::new(false));
    let whisper_started = Arc::new(AtomicBool::new(false));
    let backend = whisper(1).during_diarization({
        let (mine, theirs) = (whisper_started.clone(), analysis_started.clone());
        move || rendezvous(&mine, &theirs, limit)
    });
    let analyzer = ScriptedAnalyzer::new()
        .during(move || rendezvous(&analysis_started, &whisper_started, limit));
    (backend, analyzer)
}

#[test]
fn the_overlapped_analysis_runs_both_halves_at_once() {
    let (backend, analyzer) = halves_that_wait_for_each_other(Duration::from_secs(10));

    let outcome = analysis_outcome(analyze_overlapped, &backend, &analyzer);

    let (_, _, transcription) = outcome.expect("both halves met");
    assert_eq!(transcription.segments, words().segments);
}

#[test]
fn the_sequential_analysis_runs_the_halves_one_after_the_other() {
    // The same pair that meets when overlapped cannot meet here: the analysis
    // runs alone first and gives up waiting.
    let (backend, analyzer) = halves_that_wait_for_each_other(Duration::from_millis(150));

    let error = analysis_outcome(analyze_in_sequence, &backend, &analyzer)
        .expect_err("the Whisper half has not started while the analysis runs");

    assert!(
        error.to_string().contains("the other half did not start"),
        "{error}"
    );
    assert!(
        backend.calls().is_empty(),
        "a failed analysis ends the run before Whisper"
    );
}

#[test]
fn the_overlapped_analysis_reports_the_analysis_error_before_the_whisper_error() {
    let backend = ScriptedBackend::new(Vec::new()).diarizing(vec![Err(
        DictationError::TranscriptionFailed("whisper failed".to_string()),
    )]);
    let analyzer = ScriptedAnalyzer::new().failing(DictationError::DiarizationError(
        "analysis failed".to_string(),
    ));

    let error = analysis_outcome(analyze_overlapped, &backend, &analyzer).unwrap_err();

    assert!(error.to_string().contains("analysis failed"), "{error}");
}

#[test]
fn the_overlapped_analysis_reports_a_whisper_error_when_the_analysis_succeeds() {
    let backend = ScriptedBackend::new(Vec::new()).diarizing(vec![Err(
        DictationError::TranscriptionFailed("whisper failed".to_string()),
    )]);

    let error =
        analysis_outcome(analyze_overlapped, &backend, &ScriptedAnalyzer::new()).unwrap_err();

    assert!(error.to_string().contains("whisper failed"), "{error}");
}

#[test]
fn a_panicking_analysis_worker_is_an_error_not_a_crash() {
    let analyzer = ScriptedAnalyzer::new().during(|| panic!("the worker panicked"));

    let error = analysis_outcome(analyze_overlapped, &whisper(1), &analyzer).unwrap_err();

    assert_eq!(
        error.to_string(),
        "Diarization error: Diarization analysis worker terminated unexpectedly"
    );
}
