//! The Pianissimo path of a file run, driven by a scripted engine host (#343):
//! the checks and their order, context biasing, the per-file result, failures
//! and cancellation.
//!
//! `run_pianissimo_batch` takes the engine host through [`PianissimoHost`] and
//! the started engine through [`PianissimoEngine`], so none of this needs the
//! engine-host binary or its model. Audio is a synthetic WAV the real decoder
//! reads. The process-level tests against the real engine-host protocol are in
//! `tests/` and run on macOS only.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use sagascript_core::transcription::engine_host::{BoostReport, BoostSpec, WindowTiming};
use sagascript_core::transcription::pianissimo_backend::{
    PianissimoResult, PianissimoWord, WarmInfo, UNSUPPORTED_MESSAGE,
};

use super::*;
use crate::transcribe::pianissimo::{PianissimoEngine, PianissimoHost};

// ---- the scripted engine --------------------------------------------------------

/// One scripted answer to `transcribe`.
struct EngineReply {
    /// Run when the engine is asked, before it answers (to cancel, say).
    before: Option<Box<dyn FnOnce() + Send>>,
    /// The percent values handed to the progress callback, in order.
    progress: Vec<u8>,
    outcome: Result<PianissimoResult, DictationError>,
}

impl EngineReply {
    /// Answers with `text`, one word per whitespace-separated token.
    fn says(text: &str) -> Self {
        let words = text
            .split_whitespace()
            .enumerate()
            .map(|(index, word)| PianissimoWord {
                word: word.to_string(),
                start: index as f64 * 0.5,
                end: index as f64 * 0.5 + 0.4,
            })
            .collect();
        Self {
            before: None,
            progress: vec![0, 40, 100],
            outcome: Ok(PianissimoResult {
                text: text.to_string(),
                words,
                window_timings: vec![WindowTiming {
                    index: 0,
                    start_sample: 0,
                    end_sample: 24_000,
                    tokens: 3,
                    preprocess_ms: 4,
                    encode_ms: 500,
                    decode_ms: 70,
                    boost_us: 0,
                    round_trip_ms: 580,
                }],
            }),
        }
    }

    fn fails(error: DictationError) -> Self {
        Self {
            before: None,
            progress: Vec::new(),
            outcome: Err(error),
        }
    }
}

struct FakeEngine {
    name: Option<String>,
    warm: WarmInfo,
    warm_up_failure: Mutex<Option<DictationError>>,
    replies: Mutex<VecDeque<EngineReply>>,
    /// The samples of every transcription asked for, in order.
    transcribed: Mutex<Vec<usize>>,
    /// Every `set_boost`, in order.
    boosts: Mutex<Vec<Option<BoostSpec>>>,
    report: BoostReport,
    aborts: AtomicUsize,
    warmed: AtomicUsize,
}

impl FakeEngine {
    fn new(replies: Vec<EngineReply>) -> Arc<Self> {
        Arc::new(Self {
            name: Some("coreml".to_string()),
            warm: WarmInfo {
                was_warm: false,
                load_seconds: 0.75,
            },
            warm_up_failure: Mutex::new(None),
            replies: Mutex::new(replies.into()),
            transcribed: Mutex::new(Vec::new()),
            boosts: Mutex::new(Vec::new()),
            report: BoostReport::default(),
            aborts: AtomicUsize::new(0),
            warmed: AtomicUsize::new(0),
        })
    }

    fn with(replies: Vec<EngineReply>, edit: impl FnOnce(&mut Self)) -> Arc<Self> {
        let mut engine = Arc::try_unwrap(Self::new(replies)).ok().expect("one owner");
        edit(&mut engine);
        Arc::new(engine)
    }

    fn transcriptions(&self) -> usize {
        self.transcribed.lock().unwrap().len()
    }

    fn aborts(&self) -> usize {
        self.aborts.load(Ordering::SeqCst)
    }
}

impl PianissimoEngine for FakeEngine {
    fn set_boost(&self, boost: Option<BoostSpec>) {
        self.boosts.lock().unwrap().push(boost);
    }

    fn warm_up(&self, _: &AtomicBool) -> Result<WarmInfo, DictationError> {
        self.warmed.fetch_add(1, Ordering::SeqCst);
        match self.warm_up_failure.lock().unwrap().take() {
            Some(error) => Err(error),
            None => Ok(self.warm),
        }
    }

    fn transcribe(
        &self,
        samples: &[f32],
        progress: &dyn Fn(u8),
        _: &AtomicBool,
    ) -> Result<PianissimoResult, DictationError> {
        self.transcribed.lock().unwrap().push(samples.len());
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("the run asked for more transcriptions than the test scripted");
        if let Some(before) = reply.before {
            before();
        }
        for percent in reply.progress {
            progress(percent);
        }
        reply.outcome
    }

    fn engine_name(&self) -> Option<String> {
        self.name.clone()
    }

    fn boost_report(&self) -> BoostReport {
        self.report
    }

    fn request_abort(&self) {
        self.aborts.fetch_add(1, Ordering::SeqCst);
    }
}

struct FakeHost {
    supported: bool,
    installed: bool,
    engine: Arc<FakeEngine>,
    start_failure: Mutex<Option<DictationError>>,
    starts: AtomicUsize,
}

impl FakeHost {
    fn new(engine: Arc<FakeEngine>) -> Self {
        Self {
            supported: true,
            installed: true,
            engine,
            start_failure: Mutex::new(None),
            starts: AtomicUsize::new(0),
        }
    }

    fn starts(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }
}

impl PianissimoHost for FakeHost {
    fn runtime_supported(&self) -> bool {
        self.supported
    }

    fn model_installed(&self) -> bool {
        self.installed
    }

    fn start(&self, cancelled: &AtomicBool) -> Result<Arc<dyn PianissimoEngine>, DictationError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        // Like the real one: attaching fails at once for a run already cancelled.
        if cancelled.load(Ordering::SeqCst) {
            return Err(DictationError::Cancelled);
        }
        match self.start_failure.lock().unwrap().take() {
            Some(error) => Err(error),
            None => Ok(self.engine.clone()),
        }
    }
}

// ---- a run ---------------------------------------------------------------------------

struct Job<'a> {
    flags: &'a [&'a str],
    language: Language,
    glossary: Glossary,
}

impl Default for Job<'_> {
    fn default() -> Self {
        Self {
            flags: &[],
            language: Language::Swedish,
            glossary: Glossary::default(),
        }
    }
}

fn run_with_out(
    job: &Job<'_>,
    files: &[PathBuf],
    host: &FakeHost,
    canceller: &crate::cancel::Canceller,
    out: &Out,
) -> Result<(), DictationError> {
    let args = args_for(job.flags);
    run_pianissimo_batch(
        &PianissimoBatch {
            args: &args,
            files,
            language: job.language,
            glossary: &job.glossary,
            clipboard: &NoClipboard,
        },
        host,
        canceller,
        out,
    )
}

fn run_pianissimo(job: &Job<'_>, files: &[PathBuf], host: &FakeHost) -> Run {
    run_pianissimo_cancellable(job, files, host, &crate::cancel::Canceller::new())
}

fn run_pianissimo_cancellable(
    job: &Job<'_>,
    files: &[PathBuf],
    host: &FakeHost,
    canceller: &crate::cancel::Canceller,
) -> Run {
    let (out, captured) = Out::capture();
    let result = run_with_out(job, files, host, canceller, &out);
    Run {
        stdout: captured.stdout(),
        stderr: captured.stderr(),
        result,
    }
}

fn one_file(dir: &TempDir) -> Vec<PathBuf> {
    vec![tone(dir, "a.wav", 3)]
}

fn items(run: &Run) -> Vec<Value> {
    run.stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("a jsonl item"))
        .collect()
}

// ---- the result --------------------------------------------------------------------

#[test]
fn a_batch_writes_each_item_when_its_file_is_done_with_the_pianissimo_keys() {
    let dir = TempDir::new();
    let files = three_files(&dir)[..2].to_vec();
    let engine = FakeEngine::new(vec![
        EngineReply::says("hej världen"),
        EngineReply::says("tack"),
    ]);
    let host = FakeHost::new(engine.clone());

    let run = run_pianissimo(
        &Job {
            flags: &["--jsonl"],
            ..Job::default()
        },
        &files,
        &host,
    );

    run.result.as_ref().expect("the batch succeeds");
    let items = items(&run);
    assert_eq!(items.len(), 2);
    let (first, second) = (&items[0]["result"], &items[1]["result"]);
    assert_eq!(items[0]["status"], "ok");
    assert_eq!(first["text"], "hej världen");
    assert_eq!(first["model"], "pianissimo-sv");
    assert_eq!(first["language"], "sv");
    assert_eq!(first["segments"].as_array().unwrap().len(), 2);
    assert_eq!(first["segments"][1]["text"], "världen");
    assert_eq!(first["segments"][1]["start"], 0.5);
    assert_eq!(first["duration_seconds"], 3.0);
    assert_eq!(second["text"], "tack");
    // The first file carries the load; the second finds the model warm.
    assert_eq!(first["performance"]["model_load_seconds"], 0.75);
    assert_eq!(second["performance"]["model_load_seconds"], 0.0);
    assert_eq!(second["performance"]["model_verification_seconds"], 0.0);
    assert_eq!(first["performance"]["engine"], "coreml");
    assert_eq!(first["performance"]["engine_warm"], false);
    assert_eq!(second["performance"]["engine_warm"], true);
    assert_eq!(first["performance"]["windows"][0]["encode_ms"], 500);
    assert_eq!(first["performance"]["windows"][0]["audio_seconds"], 1.5);
    let lines = run.human_lines();
    assert!(lines.contains(&format!("[1/2] {}", files[0].display()).as_str()));
    assert!(lines.contains(&format!("[2/2] {}", files[1].display()).as_str()));
    // One start and one warm-up for the whole batch.
    assert_eq!(host.starts(), 1);
    assert_eq!(engine.warmed.load(Ordering::SeqCst), 1);
    assert_eq!(*engine.transcribed.lock().unwrap(), [48_000, 48_000]);
}

#[test]
fn an_engine_that_was_already_warm_is_reported_warm_from_the_first_file() {
    let dir = TempDir::new();
    let engine = FakeEngine::with(vec![EngineReply::says("hej")], |engine| {
        engine.warm.was_warm = true;
    });
    let run = run_pianissimo(
        &Job {
            flags: &["--json"],
            ..Job::default()
        },
        &one_file(&dir),
        &FakeHost::new(engine),
    );

    let result: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(result["performance"]["engine_warm"], true);
}

#[test]
fn an_engine_that_never_connected_is_reported_unknown() {
    let dir = TempDir::new();
    let engine = FakeEngine::with(vec![EngineReply::says("hej")], |engine| {
        engine.name = None;
    });
    let run = run_pianissimo(
        &Job {
            flags: &["--json"],
            ..Job::default()
        },
        &one_file(&dir),
        &FakeHost::new(engine),
    );

    let result: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(result["performance"]["engine"], "unknown");
}

#[test]
fn the_plain_text_is_the_transcript_after_the_glossary() {
    let dir = TempDir::new();
    let files = one_file(&dir);
    let engine = FakeEngine::new(vec![EngineReply::says("jag använder open router")]);
    let run = run_pianissimo(
        &Job {
            glossary: Glossary::parse("OpenRouter = open router"),
            ..Job::default()
        },
        &files,
        &FakeHost::new(engine),
    );

    assert_eq!(run.stdout, "jag använder OpenRouter\n");
}

#[test]
fn the_json_reports_the_glossary_corrections() {
    let dir = TempDir::new();
    let engine = FakeEngine::new(vec![EngineReply::says("jag använder open router")]);
    let run = run_pianissimo(
        &Job {
            flags: &["--json"],
            glossary: Glossary::parse("OpenRouter = open router"),
            ..Job::default()
        },
        &one_file(&dir),
        &FakeHost::new(engine),
    );

    let result: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(result["text"], "jag använder OpenRouter");
    assert_eq!(
        result["vocabulary_corrections"][0]["original"],
        "open router"
    );
    assert_eq!(
        result["vocabulary_corrections"][0]["replacement"],
        "OpenRouter"
    );
}

#[test]
fn progress_events_narrate_decoding_transcribing_and_completion() {
    let dir = TempDir::new();
    let engine = FakeEngine::new(vec![EngineReply::says("hej")]);
    let run = run_pianissimo(
        &Job {
            flags: &["--progress-json"],
            ..Job::default()
        },
        &one_file(&dir),
        &FakeHost::new(engine),
    );

    run.result.as_ref().expect("the run succeeds");
    assert_eq!(
        run.phases(),
        ["decoding", "resampling", "transcribing", "completed"]
    );
    // The batch says 0, then the engine reports its own 0, 40 and 100.
    assert_eq!(
        run.percents("transcribing"),
        [Some(0), Some(0), Some(40), Some(100)]
    );
    assert_eq!(run.percents("completed"), [Some(100)]);
    assert_eq!(run.percents("decoding").first(), Some(&Some(0)));
}

// ---- the checks ---------------------------------------------------------------------

#[test]
fn the_checks_run_in_a_fixed_order_and_none_of_them_starts_the_host() {
    let dir = TempDir::new();
    let files = one_file(&dir);
    let wrong_flags: [&[&str]; 5] = [
        &["--vad"],
        &["--beam", "2"],
        &["--parallel", "2"],
        &["--prompt", "x"],
        &["--json", "--correct-hints"],
    ];
    // (language, flags, supported, installed) -> the first message that applies.
    let mut cases: Vec<(Language, &[&str], bool, bool, &str)> = vec![
        // The language comes first, whatever else is wrong.
        (
            Language::English,
            wrong_flags[0],
            false,
            false,
            "Pianissimo supports Swedish files only",
        ),
        (Language::Swedish, &[], false, false, UNSUPPORTED_MESSAGE),
        (
            Language::Swedish,
            &[],
            true,
            false,
            "Pianissimo is not downloaded. Run: sagascript download-model pianissimo-sv",
        ),
    ];
    // A decoder option comes before the platform and the model.
    for flags in wrong_flags {
        cases.push((
            Language::Swedish,
            flags,
            false,
            false,
            "Pianissimo does not support --vad, --beam, --parallel, --correct-hints, or decoder --prompt/--prompt-file",
        ));
    }
    for (language, flags, supported, installed, message) in cases {
        let engine = FakeEngine::new(Vec::new());
        let mut host = FakeHost::new(engine);
        host.supported = supported;
        host.installed = installed;

        let run = run_pianissimo(
            &Job {
                flags,
                language,
                ..Job::default()
            },
            &files,
            &host,
        );

        let error = run.result.expect_err("the checks fail");
        assert!(error.to_string().contains(message), "{flags:?}: {error}");
        assert_eq!(host.starts(), 0, "{flags:?}: the host is not started");
        assert_eq!(run.stdout, "");
    }
}

#[test]
fn a_glossary_with_hints_is_told_they_apply_after_transcription() {
    let dir = TempDir::new();
    let note = "Pianissimo does not use Whisper decoder hints; saved glossary replacements still apply after transcription.";
    let with_hints = run_pianissimo(
        &Job {
            glossary: Glossary::parse("OpenRouter = open router"),
            ..Job::default()
        },
        &one_file(&dir),
        &FakeHost::new(FakeEngine::new(vec![EngineReply::says("hej")])),
    );
    let without = run_pianissimo(
        &Job::default(),
        &one_file(&dir),
        &FakeHost::new(FakeEngine::new(vec![EngineReply::says("hej")])),
    );

    let lines = with_hints.human_lines();
    assert_eq!(lines.iter().filter(|line| **line == note).count(), 1);
    assert_eq!(lines[0], note, "before the first file");
    assert!(!without.stderr.contains("decoder hints"));
}

// ---- context biasing -----------------------------------------------------------------

fn terms_file(dir: &TempDir, lines: &[String]) -> String {
    let path = dir.join("terms.txt");
    std::fs::write(&path, lines.join("\n")).unwrap();
    path.display().to_string()
}

#[test]
fn context_biasing_sends_the_glossary_terms_and_the_terms_file() {
    let dir = TempDir::new();
    let terms = terms_file(&dir, &["Estrid".to_string(), "Grimnir".to_string()]);
    let engine = FakeEngine::new(vec![EngineReply::says("hej")]);
    let host = FakeHost::new(engine.clone());

    let run = run_pianissimo(
        &Job {
            flags: &["--glossary-boost", "2.5", "--boost-terms-file", &terms],
            glossary: Glossary::parse("OpenRouter = open router"),
            ..Job::default()
        },
        &one_file(&dir),
        &host,
    );

    run.result.as_ref().expect("the run succeeds");
    let boosts = engine.boosts.lock().unwrap();
    assert_eq!(boosts.len(), 1);
    let spec = boosts[0].as_ref().expect("a dictionary is set");
    assert_eq!(spec.terms, ["OpenRouter", "Estrid", "Grimnir"]);
    assert_eq!(spec.weight, 2.5);
    assert!(run
        .human_lines()
        .contains(&"Context biasing requested: 3 terms, weight 2.5"));
}

#[test]
fn context_biasing_without_terms_or_weight_leaves_decoding_unchanged() {
    let dir = TempDir::new();
    let engine = FakeEngine::new(vec![EngineReply::says("hej")]);

    let run = run_pianissimo(
        &Job {
            flags: &["--glossary-boost", "0"],
            glossary: Glossary::parse("OpenRouter = open router"),
            ..Job::default()
        },
        &one_file(&dir),
        &FakeHost::new(engine.clone()),
    );

    run.result.as_ref().expect("the run succeeds");
    assert!(run
        .human_lines()
        .contains(&"Context biasing requested but no terms/weight; decoding unchanged."));
    assert!(engine.boosts.lock().unwrap().is_empty());
}

#[test]
fn context_biasing_says_what_it_dropped() {
    let dir = TempDir::new();
    let many: Vec<String> = (0..600).map(|index| format!("term{index}")).collect();
    let too_long = "x".repeat(70);
    let mut lines = many.clone();
    lines.push(too_long);
    let terms = terms_file(&dir, &lines);
    let expected = BoostSpec::new(lines.clone(), 2.0).expect("a dictionary");
    assert!(expected.truncated > 0 && expected.too_long == 1);

    let run = run_pianissimo(
        &Job {
            flags: &["--glossary-boost", "2", "--boost-terms-file", &terms],
            ..Job::default()
        },
        &one_file(&dir),
        &FakeHost::new(FakeEngine::new(vec![EngineReply::says("hej")])),
    );

    let lines = run.human_lines();
    assert!(lines.contains(
        &format!(
            "Warning: only the first {} distinct terms are used; {} more were ignored.",
            expected.terms.len(),
            expected.truncated
        )
        .as_str()
    ));
    assert!(lines.contains(&"Warning: 1 term(s) longer than 64 characters were ignored."));
}

#[test]
fn an_unreadable_terms_file_ends_the_run_before_the_model_is_warmed() {
    let dir = TempDir::new();
    let missing = dir.join("missing-terms.txt").display().to_string();
    let engine = FakeEngine::new(Vec::new());

    let run = run_pianissimo(
        &Job {
            flags: &["--glossary-boost", "2", "--boost-terms-file", &missing],
            ..Job::default()
        },
        &one_file(&dir),
        &FakeHost::new(engine.clone()),
    );

    let error = run.result.expect_err("the terms file cannot be read");
    assert!(
        error.to_string().contains("Cannot read --boost-terms-file"),
        "{error}"
    );
    assert_eq!(engine.warmed.load(Ordering::SeqCst), 0);
    assert_eq!(engine.transcriptions(), 0);
}

#[test]
fn the_boost_report_is_told_after_the_batch_only_when_biasing_was_asked_for() {
    let dir = TempDir::new();
    let flags: &[&str] = &["--glossary-boost", "2"];
    let glossary = Glossary::parse("OpenRouter = open router");
    let run_with = |report: BoostReport, flags: &[&str]| {
        let engine = FakeEngine::with(vec![EngineReply::says("hej")], |engine| {
            engine.report = report;
        });
        run_pianissimo(
            &Job {
                flags,
                glossary: glossary.clone(),
                ..Job::default()
            },
            &one_file(&dir),
            &FakeHost::new(engine),
        )
    };

    let ignored = run_with(
        BoostReport {
            windows: 5,
            inactive_windows: 2,
            unusable_windows: 0,
            trie_build_us: 0,
        },
        flags,
    );
    assert!(ignored
        .stderr
        .contains("Warning: Context biasing was NOT applied to 2 of 5 windows"));

    let active = run_with(
        BoostReport {
            windows: 4,
            inactive_windows: 0,
            unusable_windows: 0,
            trie_build_us: 1_500,
        },
        flags,
    );
    assert!(active
        .human_lines()
        .contains(&"Context biasing active on 4 window(s); trie build 1.5 ms."));

    let quiet = run_with(BoostReport::default(), flags);
    assert!(!quiet.stderr.contains("Context biasing active"));
    assert!(!quiet.stderr.contains("NOT applied"));

    let not_asked = run_with(
        BoostReport {
            windows: 5,
            inactive_windows: 2,
            unusable_windows: 0,
            trie_build_us: 0,
        },
        &[],
    );
    assert!(!not_asked.stderr.contains("Context biasing"));
}

// ---- failures and cancellation -----------------------------------------------------

#[test]
fn a_failing_file_leaves_the_others_written_and_fails_the_batch() {
    let dir = TempDir::new();
    let files = three_files(&dir);
    let engine = FakeEngine::new(vec![
        EngineReply::says("ett"),
        EngineReply::fails(DictationError::TranscriptionFailed("window failed".into())),
        EngineReply::says("tre"),
    ]);

    let run = run_pianissimo(
        &Job {
            flags: &["--jsonl"],
            ..Job::default()
        },
        &files,
        &FakeHost::new(engine.clone()),
    );

    let items = items(&run);
    let statuses: Vec<&str> = items
        .iter()
        .map(|item| item["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["ok", "error", "ok"]);
    assert_eq!(items[1]["error"], "Transcription failed: window failed");
    assert!(run.human_lines().contains(
        &format!(
            "Error transcribing {}: Transcription failed: window failed",
            files[1].display()
        )
        .as_str()
    ));
    assert_eq!(
        run.result.unwrap_err().to_string(),
        "Transcription failed: 1 of 3 batch item(s) failed"
    );
}

#[test]
fn fail_fast_stops_at_the_first_failure() {
    let dir = TempDir::new();
    let engine = FakeEngine::new(vec![
        EngineReply::fails(DictationError::TranscriptionFailed("window failed".into())),
        EngineReply::says("aldrig"),
    ]);

    let run = run_pianissimo(
        &Job {
            flags: &["--jsonl", "--fail-fast"],
            ..Job::default()
        },
        &three_files(&dir),
        &FakeHost::new(engine.clone()),
    );

    assert_eq!(items(&run).len(), 1);
    assert_eq!(engine.transcriptions(), 1);
    assert_eq!(
        run.result.unwrap_err().to_string(),
        "Transcription failed: 1 of 1 batch item(s) failed"
    );
}

#[test]
fn a_cancel_during_transcription_ends_the_batch_and_aborts_the_engine() {
    let dir = TempDir::new();
    let canceller = Arc::new(crate::cancel::Canceller::new());
    let cancel_now = {
        let canceller = canceller.clone();
        Box::new(move || {
            canceller.request();
        }) as Box<dyn FnOnce() + Send>
    };
    let engine = FakeEngine::new(vec![
        EngineReply::says("ett"),
        EngineReply {
            before: Some(cancel_now),
            progress: Vec::new(),
            outcome: Err(DictationError::Cancelled),
        },
        EngineReply::says("tre"),
    ]);

    let run = run_pianissimo_cancellable(
        &Job {
            flags: &["--jsonl"],
            ..Job::default()
        },
        &three_files(&dir),
        &FakeHost::new(engine.clone()),
        &canceller,
    );

    assert!(matches!(run.result, Err(DictationError::Cancelled)));
    // The first file's item was written when it finished; nothing for the second.
    assert_eq!(items(&run).len(), 1, "{}", run.stdout);
    assert_eq!(engine.transcriptions(), 2, "the third never started");
    assert_eq!(engine.aborts(), 1, "the engine is told to stop");
    assert!(!run.stderr.contains("Error transcribing"));
}

#[test]
fn a_cancel_while_decoding_stops_before_the_engine_is_asked() {
    let dir = TempDir::new();
    let canceller = Arc::new(crate::cancel::Canceller::new());
    let engine = FakeEngine::new(vec![EngineReply::says("aldrig")]);
    let host = FakeHost::new(engine.clone());
    let out = CancelOnMention::out(FIRST_DECODING_EVENT, &canceller);
    // The first decoding event is written just before the decoder starts, so
    // the cancel lands inside the decode, not between files.
    let result = run_with_out(
        &Job {
            flags: &["--progress-json", "--jsonl"],
            ..Job::default()
        },
        &one_file(&dir),
        &host,
        &canceller,
        &out,
    );

    assert!(
        matches!(result, Err(DictationError::Cancelled)),
        "{result:?}"
    );
    assert_eq!(engine.transcriptions(), 0, "the engine was never asked");
    assert_eq!(engine.aborts(), 1);
}

#[test]
fn the_abort_hook_is_gone_when_the_run_ends() {
    let dir = TempDir::new();
    let canceller = crate::cancel::Canceller::new();
    let engine = FakeEngine::new(vec![EngineReply::says("hej")]);

    let run = run_pianissimo_cancellable(
        &Job::default(),
        &one_file(&dir),
        &FakeHost::new(engine.clone()),
        &canceller,
    );
    run.result.as_ref().expect("the run succeeds");
    canceller.request();

    assert_eq!(
        engine.aborts(),
        0,
        "a later cancel does not reach this engine"
    );
}

#[test]
fn a_run_that_is_already_cancelled_never_attaches_to_the_engine() {
    let dir = TempDir::new();
    let canceller = crate::cancel::Canceller::new();
    canceller.request();
    let engine = FakeEngine::new(vec![EngineReply::says("aldrig")]);

    let run = run_pianissimo_cancellable(
        &Job::default(),
        &one_file(&dir),
        &FakeHost::new(engine.clone()),
        &canceller,
    );

    assert!(matches!(run.result, Err(DictationError::Cancelled)));
    assert_eq!(engine.warmed.load(Ordering::SeqCst), 0);
    assert_eq!(run.stdout, "");
}

#[test]
fn a_host_that_fails_to_start_ends_the_run_before_the_model_is_warmed() {
    let dir = TempDir::new();
    let engine = FakeEngine::new(Vec::new());
    let host = FakeHost::new(engine.clone());
    *host.start_failure.lock().unwrap() = Some(DictationError::TranscriptionFailed(
        "engine host missing".into(),
    ));

    let run = run_pianissimo(&Job::default(), &one_file(&dir), &host);

    assert_eq!(
        run.result.unwrap_err().to_string(),
        "Transcription failed: engine host missing"
    );
    assert_eq!(engine.warmed.load(Ordering::SeqCst), 0);
}

#[test]
fn a_warm_up_that_fails_or_is_cancelled_ends_the_run_with_no_output() {
    let dir = TempDir::new();
    for failure in [
        DictationError::Cancelled,
        DictationError::TranscriptionFailed("model failed to load".into()),
    ] {
        let message = failure.to_string();
        let engine = FakeEngine::with(vec![EngineReply::says("aldrig")], |engine| {
            *engine.warm_up_failure.lock().unwrap() = Some(failure);
        });

        let run = run_pianissimo(
            &Job {
                flags: &["--json"],
                ..Job::default()
            },
            &one_file(&dir),
            &FakeHost::new(engine.clone()),
        );

        assert_eq!(run.result.unwrap_err().to_string(), message);
        assert_eq!(run.stdout, "");
        assert_eq!(engine.transcriptions(), 0);
    }
}
