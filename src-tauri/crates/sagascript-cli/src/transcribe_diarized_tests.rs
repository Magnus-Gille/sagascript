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
use crate::transcribe::diarize::CachePolicy;
use crate::transcribe::diarize::{
    analyze_in_sequence, analyze_overlapped, AnalysisOutcome, DiarizationAnalyzer,
};
use crate::transcribe::plan::{CacheRequest, DiarizationFlags};
use sagascript_core::diarization::cache::{load, CacheIdentity, CacheLookup};

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
    progress_events: bool,
}

impl Job {
    fn normal() -> Self {
        Self {
            model: WhisperModel::TinyEn,
            policy: CachePolicy::Normal,
            with_cache: true,
            progress_events: false,
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

    /// With `--progress-json`.
    fn with_progress_events(self) -> Self {
        Self {
            progress_events: true,
            ..self
        }
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
    let (out, captured) = Out::capture();
    let result = diarize_in(
        fx,
        job,
        Services {
            backend,
            analyzer,
            canceller: &crate::cancel::Canceller::new(),
        },
        control,
        &out,
    );
    Diarized {
        result,
        stderr: captured.stderr(),
    }
}

/// One diarized file with the given services and output.
fn diarize_in(
    fx: &Fixture,
    job: Job,
    services: Services<'_>,
    control: Option<&MeetingControl<'_>>,
    out: &Out,
) -> Result<FileTranscription, DictationError> {
    let options = FileOptions {
        progress_json: job.progress_events,
        diarization: Some(DiarizationFlags {
            cache: job.with_cache.then(|| CacheRequest {
                path: fx.cache.clone(),
                policy: job.policy,
            }),
            ..DiarizationFlags::default()
        }),
        ..FileOptions::default()
    };
    let mut model_loaded = false;
    transcribe_file(
        &FileRequest {
            options: &options,
            file: &fx.file,
            stored: &Settings::default(),
            language: Language::English,
            model: job.model,
            glossary: &Glossary::default(),
            correction_vocabulary: &[],
            control,
        },
        services,
        &mut model_loaded,
        out,
    )
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

// ---- the command line's own run: the batch, the sink and the options ------------------------

/// The command line's batch (`run_whisper_batch`) over the fixture's recording.
fn cli_run(
    fx: &Fixture,
    flags: &[&str],
    backend: &ScriptedBackend,
    analyzer: &ScriptedAnalyzer,
) -> Run {
    let args = args_for(flags);
    let (out, captured) = Out::capture();
    let result = run_whisper_batch(
        &WhisperBatch {
            args: &args,
            files: std::slice::from_ref(&fx.file),
            stored: &Settings::default(),
            language: Language::English,
            model: WhisperModel::TinyEn,
            glossary: &Glossary::default(),
            correction_vocabulary: &[],
            clipboard: &NoClipboard,
        },
        Services {
            backend,
            analyzer,
            canceller: &crate::cancel::Canceller::new(),
        },
        &out,
    );
    Run {
        stdout: captured.stdout(),
        stderr: captured.stderr(),
        result,
    }
}

#[test]
fn a_command_line_run_reads_and_writes_the_cache_under_the_normal_policy() {
    let fx = Fixture::new();
    let backend = whisper(1);
    let analyzer = ScriptedAnalyzer::new();
    let cache = fx.cache.display().to_string();
    let flags = ["--diarize", "--diarize-cache", &cache, "--json"];

    let first = cli_run(&fx, &flags, &backend, &analyzer);
    first.result.as_ref().expect("the miss succeeds");
    let miss: Value = serde_json::from_str(&first.stdout).unwrap();
    assert_eq!(miss["performance"]["cache_hit"], false);
    assert!(first.stderr.contains("Saved reusable diarization cache"));

    let second = cli_run(&fx, &flags, &backend, &analyzer);
    second.result.as_ref().expect("the hit succeeds");
    let hit: Value = serde_json::from_str(&second.stdout).unwrap();
    assert_eq!(hit["performance"]["cache_hit"], true);
    assert!(second.stderr.contains("Reusing diarization cache"));
    assert_eq!(analyzer.calls(), 1);
    assert_eq!(without_performance(miss), without_performance(hit));
}

#[test]
fn a_command_line_meeting_run_writes_the_shared_document_and_narrates_the_phases() {
    let fx = Fixture::new();
    let analyzer = ScriptedAnalyzer::new();

    let run = cli_run(
        &fx,
        &["--diarize", "--meeting-json", "--progress-json"],
        &whisper(1),
        &analyzer,
    );

    run.result.as_ref().expect("the run succeeds");
    let document: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(document["speakers"].as_array().unwrap().len(), 2);
    assert_eq!(document["model"], "tiny.en");
    assert!(document["diarization"].is_object());
    // The phases of a computed run, in order; the diarized ones carry no
    // step number or percent but the decoder settings.
    let phases = run.phases();
    let position = |name: &str| {
        phases
            .iter()
            .position(|phase| phase == name)
            .unwrap_or_else(|| panic!("no {name} event in {phases:?}"))
    };
    assert!(position("decoding") < position("loading"));
    assert!(position("loading") < position("analyzing"));
    assert!(position("analyzing") < position("clustering"));
    assert!(position("clustering") < position("finalizing"));
    assert!(position("finalizing") < position("completed"));
    let analyzing = run
        .events()
        .into_iter()
        .find(|event| event["phase"] == "analyzing")
        .unwrap();
    assert!(analyzing["step"].is_null() && analyzing["percent"].is_null());
    assert_eq!(analyzing["decoder"]["beam_size"], 0);
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

#[test]
fn a_cancel_while_decoding_stops_before_the_analysis_and_writes_no_cache() {
    for job in [Job::normal(), Job::refresh()] {
        let fx = Fixture::new();
        let canceller = Arc::new(crate::cancel::Canceller::new());
        let backend = whisper(1);
        let analyzer = ScriptedAnalyzer::new();
        let out = CancelOnMention::out(FIRST_DECODING_EVENT, &canceller);

        let result = diarize_in(
            &fx,
            job.with_progress_events(),
            Services {
                backend: &backend,
                analyzer: &analyzer,
                canceller: &canceller,
            },
            None,
            &out,
        );

        assert!(
            matches!(result, Err(DictationError::Cancelled)),
            "{result:?}"
        );
        assert_eq!(analyzer.calls(), 0, "the analysis never started");
        assert!(backend.calls().is_empty(), "no model, no Whisper half");
        assert_eq!(fx.files(), ["meeting.wav"], "no cache, no staging file");
    }
}

#[test]
fn a_cancel_while_decoding_keeps_a_previous_cache() {
    let fx = Fixture::new();
    seed(&fx, WhisperModel::TinyEn);
    let seeded = fx.cache_bytes();
    let canceller = Arc::new(crate::cancel::Canceller::new());
    let backend = whisper(1);
    let analyzer = ScriptedAnalyzer::new();
    let out = CancelOnMention::out(FIRST_DECODING_EVENT, &canceller);

    // Another model: a miss, so the file is decoded.
    let result = diarize_in(
        &fx,
        Job::normal()
            .with_model(WhisperModel::Base)
            .with_progress_events(),
        Services {
            backend: &backend,
            analyzer: &analyzer,
            canceller: &canceller,
        },
        None,
        &out,
    );

    assert!(
        matches!(result, Err(DictationError::Cancelled)),
        "{result:?}"
    );
    assert!(backend.calls().is_empty(), "the model was never loaded");
    assert_eq!(fx.cache_bytes(), seeded);
    assert_eq!(fx.files(), ["cache.json", "meeting.wav"]);
}

// ---- the meeting entry points: typed options, no command line --------------------------------

use crate::transcribe::args::validated_speaker_hint;
use crate::transcribe::meeting::{
    meeting_options, stable_file_sha256, transcribe_meeting_file_inner,
};
use sagascript_core::diarization::speaker_hint::SpeakerCountHint;
use sagascript_core::meeting::MeetingTranscript;

/// One meeting run as the app and reprocessing make it: the options of
/// `meeting_options`, the given backend and analyzer.
fn meeting(
    fx: &Fixture,
    options: &FileOptions,
    backend: &ScriptedBackend,
    analyzer: &ScriptedAnalyzer,
    control: Option<&MeetingControl<'_>>,
) -> Result<MeetingTranscript, DictationError> {
    transcribe_meeting_file_inner(
        &fx.file,
        &Settings::default(),
        Language::English,
        WhisperModel::TinyEn,
        &Glossary::default(),
        Services {
            backend,
            analyzer,
            canceller: &crate::cancel::Canceller::new(),
        },
        control,
        options,
    )
}

fn default_meeting_options() -> FileOptions {
    meeting_options(sagascript_core::diarization::DEFAULT_THRESHOLD, None, None)
}

#[test]
fn a_meeting_run_returns_the_validated_document_of_the_recording() {
    let fx = Fixture::new();
    let analyzer = ScriptedAnalyzer::new();

    let document = meeting(
        &fx,
        &default_meeting_options(),
        &whisper(1),
        &analyzer,
        None,
    )
    .expect("the meeting run succeeds");

    document.validate().expect("the document validates");
    assert_eq!(document.speakers.len(), 2);
    assert_eq!(
        document.source_sha256,
        stable_file_sha256(&fx.file, None).unwrap()
    );
    assert_eq!(document.language, "en");
    assert_eq!(document.model, "tiny.en");
    assert_eq!(document.duration_seconds, SECONDS as f64);
    assert!(document.diarization.is_some());
    assert!(document.speaker_hint.is_none());
    assert_eq!(analyzer.calls(), 1);
    // The meeting options carry no cache.
    assert_eq!(fx.files(), ["meeting.wav"]);
}

#[test]
fn the_threshold_reaches_the_clustering() {
    let fx = Fixture::new();
    let speakers = |threshold: f32| {
        meeting(
            &fx,
            &meeting_options(threshold, None, None),
            &whisper(1),
            &ScriptedAnalyzer::new(),
            None,
        )
        .expect("the meeting run succeeds")
        .speakers
        .len()
    };

    assert_eq!(speakers(0.34), 2);
    assert_eq!(speakers(1.1), 1);
}

#[test]
fn the_speaker_hint_reaches_the_clustering_and_is_recorded() {
    let fx = Fixture::new();
    let run = |hint: Option<SpeakerCountHint>| {
        meeting(
            &fx,
            &meeting_options(sagascript_core::diarization::DEFAULT_THRESHOLD, hint, None),
            &whisper(1),
            &ScriptedAnalyzer::new(),
            None,
        )
        .expect("the meeting run succeeds")
    };

    // Two clearly different voices: asking for one does not merge them, and
    // the document says the hint fell short ...
    let exact = run(Some(SpeakerCountHint::exact(1)));
    assert_eq!(exact.speakers.len(), 2);
    assert_eq!(exact.speaker_hint, Some(SpeakerCountHint::exact(1)));
    assert_eq!(exact.speaker_hint_satisfied, Some(false));
    assert_eq!(exact.speaker_hint_delivered, Some(2));
    // ... unless the hint is forced.
    let forced = run(Some(SpeakerCountHint::forced(1)));
    assert_eq!(forced.speakers.len(), 1);
    assert_eq!(forced.speaker_hint, Some(SpeakerCountHint::forced(1)));
    assert_eq!(forced.speaker_hint_satisfied, Some(true));
    let range = run(Some(SpeakerCountHint::range(Some(2), None)));
    assert_eq!(range.speaker_hint_satisfied, Some(true));
    assert_eq!(range.speaker_hint_delivered, Some(2));
}

#[test]
fn the_meeting_options_hold_the_hint_as_the_flags_would() {
    let hints = [
        None,
        Some(SpeakerCountHint::exact(3)),
        Some(SpeakerCountHint::range(Some(2), Some(4))),
        Some(SpeakerCountHint::range(None, Some(2))),
        Some(SpeakerCountHint::forced(2)),
    ];
    for hint in hints {
        let options = meeting_options(0.5, hint, None);
        let flags = options.diarization.expect("a meeting run is diarized");
        assert_eq!(validated_speaker_hint(&flags.speaker_count).unwrap(), hint);
        assert_eq!(flags.threshold, 0.5);
        assert!(flags.meeting_json);
        assert!(flags.cache.is_none());
        assert!(!flags.diagnostics && !flags.exclusive_speech_embeddings);
    }
    // Nothing but the diarized pipeline is switched on.
    let options = meeting_options(0.5, None, None);
    assert!(!options.progress_json && !options.correct_hints);
    assert!(!options.decoder.vad && !options.decoder.no_vad);
    assert!(options.decoder.beam_size.is_none() && options.decoder.parallel.is_none());
    // An empty hint is no hint, as an empty set of flags is.
    let empty = meeting_options(0.5, Some(SpeakerCountHint::default()), None);
    let flags = empty.diarization.unwrap();
    assert_eq!(validated_speaker_hint(&flags.speaker_count).unwrap(), None);
}

#[test]
fn a_hint_that_does_not_validate_fails_after_the_model_load_and_before_the_analysis() {
    let fx = Fixture::new();
    let analyzer = ScriptedAnalyzer::new();
    let backend = whisper(1);
    let backwards = SpeakerCountHint {
        min: Some(4),
        max: Some(2),
        ..SpeakerCountHint::default()
    };

    let error = meeting(
        &fx,
        &meeting_options(
            sagascript_core::diarization::DEFAULT_THRESHOLD,
            Some(backwards),
            None,
        ),
        &backend,
        &analyzer,
        None,
    )
    .expect_err("an invalid hint fails the run");

    assert_eq!(
        error.to_string(),
        "Settings error: --speakers: min-speakers (4) must not exceed max-speakers (2)"
    );
    assert_eq!(analyzer.calls(), 0);
    // An English-only model needs no language check; it was loaded, for the
    // diarized profile, before the hint was looked at.
    assert!(
        matches!(
            backend.calls()[..],
            [Call::Load(
                WhisperModel::TinyEn,
                ContextProfile::TokenAlignment
            )]
        ),
        "{:?}",
        backend.calls()
    );
}

#[test]
fn a_full_meeting_run_stages_a_new_cache_and_never_replaces_one() {
    let fx = Fixture::new();
    let staged = meeting_options(
        sagascript_core::diarization::DEFAULT_THRESHOLD,
        None,
        Some(CacheRequest {
            path: fx.cache.clone(),
            policy: CachePolicy::RefreshNew,
        }),
    );
    let analyzer = ScriptedAnalyzer::new();

    meeting(&fx, &staged, &whisper(1), &analyzer, None).expect("the staging run succeeds");
    assert!(matches!(
        fx.lookup(WhisperModel::TinyEn),
        CacheLookup::Hit(_)
    ));
    let staged_bytes = fx.cache_bytes();

    // A second full run recomputes (it never reads the cache) and refuses to replace it.
    let error = meeting(&fx, &staged, &whisper(1), &analyzer, None)
        .expect_err("the staged cache is not replaced");
    assert!(
        error
            .to_string()
            .contains("Diarization cache output already exists"),
        "{error}"
    );
    assert_eq!(analyzer.calls(), 2);
    assert_eq!(fx.cache_bytes(), staged_bytes);
    assert_eq!(fx.files(), ["cache.json", "meeting.wav"]);
}

#[test]
fn a_meeting_job_reports_its_phases_in_order() {
    let fx = Fixture::new();
    let cancelled = AtomicBool::new(false);
    let phases = Mutex::new(Vec::new());
    let progress = |phase| {
        let mut seen = phases.lock().unwrap();
        // Each checkpoint reports once; a phase with two checkpoints is one phase.
        if seen.last() != Some(&phase) {
            seen.push(phase);
        }
    };
    let control = MeetingControl {
        cancellation: &cancelled,
        progress: &progress,
    };

    meeting(
        &fx,
        &default_meeting_options(),
        &whisper(1),
        &ScriptedAnalyzer::new(),
        Some(&control),
    )
    .expect("the meeting run succeeds");

    assert_eq!(
        *phases.lock().unwrap(),
        [
            MeetingPhase::Preparing,
            MeetingPhase::Decoding,
            MeetingPhase::LoadingModel,
            MeetingPhase::Analyzing,
            MeetingPhase::Clustering,
            MeetingPhase::Finalizing,
        ]
    );
}

// ---- a cache that cannot be trusted -----------------------------------------------------------

use crate::transcribe::source::CachedRun;

#[test]
fn a_cache_without_coarse_coverage_segments_is_a_miss_and_is_recomputed() {
    let fx = Fixture::new();
    seed(&fx, WhisperModel::TinyEn);
    // The same cache, as an older writer left it: the field is gone.
    let mut cache: Value = serde_json::from_slice(&fx.cache_bytes()).unwrap();
    cache["coverage_segments"] = Value::Null;
    std::fs::write(&fx.cache, serde_json::to_vec(&cache).unwrap()).unwrap();
    assert!(matches!(
        fx.lookup(WhisperModel::TinyEn),
        CacheLookup::Miss("coarse coverage segments are missing")
    ));
    let analyzer = ScriptedAnalyzer::new();

    let run = run(&fx, Job::normal(), &whisper(1), &analyzer);

    assert!(run.stderr.contains(
        "Diarization cache miss (coarse coverage segments are missing); computing analysis."
    ));
    assert_eq!(run.json()["performance"]["cache_hit"], false);
    assert_eq!(analyzer.calls(), 1);
    // The run wrote a cache that has them, and the next run reads it.
    assert!(matches!(
        fx.lookup(WhisperModel::TinyEn),
        CacheLookup::Hit(_)
    ));
    assert_eq!(fx.files(), ["cache.json", "meeting.wav"]);
}

#[test]
fn a_hit_without_coverage_segments_is_a_typed_error_not_a_panic() {
    // Unreachable through the loader (see the test above); `from_hit` still
    // answers it with a diarization error.
    let fx = Fixture::new();
    seed(&fx, WhisperModel::TinyEn);
    let CacheLookup::Hit(mut hit) = fx.lookup(WhisperModel::TinyEn) else {
        panic!("the seeded cache is a hit");
    };
    assert!(CachedRun::from_hit((*hit).clone()).is_ok());
    hit.coverage_segments = None;

    let error = CachedRun::from_hit(*hit)
        .err()
        .expect("a hit without coverage segments is refused");

    assert!(
        matches!(error, DictationError::DiarizationError(_)),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "Diarization error: A validated diarization cache has no coverage segments"
    );
}

#[test]
fn a_cache_path_that_is_the_recording_is_refused_before_any_work() {
    let fx = Fixture::new();
    let options = FileOptions {
        diarization: Some(DiarizationFlags {
            cache: Some(CacheRequest {
                path: fx.file.clone(),
                policy: CachePolicy::Normal,
            }),
            ..DiarizationFlags::default()
        }),
        ..FileOptions::default()
    };
    let backend = whisper(1);
    let analyzer = ScriptedAnalyzer::new();
    let (out, captured) = Out::capture();
    let mut model_loaded = false;

    let error = transcribe_file(
        &FileRequest {
            options: &options,
            file: &fx.file,
            stored: &Settings::default(),
            language: Language::English,
            model: WhisperModel::TinyEn,
            glossary: &Glossary::default(),
            correction_vocabulary: &[],
            control: None,
        },
        Services {
            backend: &backend,
            analyzer: &analyzer,
            canceller: &crate::cancel::Canceller::new(),
        },
        &mut model_loaded,
        &out,
    )
    .expect_err("the recording is not a cache");

    assert_eq!(
        error.to_string(),
        "Settings error: --diarize-cache must not point to the input recording"
    );
    assert!(backend.calls().is_empty() && analyzer.calls() == 0);
    assert!(!captured.stderr().contains("Decoding"));
    assert_eq!(fx.files(), ["meeting.wav"]);
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
        Services {
            backend,
            analyzer,
            canceller: &crate::cancel::Canceller::new(),
        },
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
