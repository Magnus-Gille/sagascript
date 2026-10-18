//! The Whisper path of a file run, driven by a scripted backend (#343).
//!
//! `transcribe_file` and `run_whisper_batch` take their backend through
//! [`InferenceBackend`], so a successful run can be pinned without model
//! weights: the progress events, the human lines, the `--json` result object,
//! the glossary and hint corrections, the language-mismatch warning and the
//! three output modes of a batch. Audio is a synthetic WAV the real decoder
//! reads; the backend answers with segments the test scripts.
//!
//! The process-level contract (the real binary, exit status, error codes) is in
//! `tests/transcribe_contract.rs`.

use std::collections::VecDeque;
use std::sync::Mutex;

use clap::Parser;
use sagascript_core::audio::wav::encode_wav;
use sagascript_core::transcription::diagnostics::LanguageDetection;
#[cfg(feature = "diarization")]
use sagascript_core::transcription::DiarizationTranscription;
use sagascript_testsupport::TempDir;
use serde_json::{json, Value};

use super::inference::InferenceBackend;
use super::language_detection::LanguageProbe;
use super::*;
use sagascript_core::transcription::{ContextProfile, TranscribeOptions, TranscriptSegment};

// ---- the scripted backend -----------------------------------------------------

/// A step a test runs in the middle of a call, which can fail the call.
#[cfg(feature = "diarization")]
type Hook = Box<dyn FnOnce() -> Result<(), DictationError> + Send>;

/// What `transcribe_segments` was called with.
#[derive(Debug, PartialEq)]
struct Transcribed {
    samples: usize,
    language: Language,
    prompt: Option<String>,
    beam_size: u32,
    temperature_fallback: bool,
    vad: bool,
    segment_timestamps: bool,
    parallel_chunks: usize,
}

/// A call the run made on the backend, in order.
#[derive(Debug, PartialEq)]
enum Call {
    Load(WhisperModel, ContextProfile),
    Detect {
        samples: usize,
    },
    EnsureVadModel,
    Transcribe(Transcribed),
    /// The Whisper half of a diarized run.
    #[cfg(feature = "diarization")]
    TranscribeForDiarization {
        samples: usize,
        prompt: Option<String>,
    },
}

/// One scripted answer to `transcribe_segments`.
struct Reply {
    /// Run when the backend is asked, before it answers (to cancel, say).
    before: Option<Box<dyn FnOnce() + Send>>,
    /// Whisper's percent values, handed to the progress callback in order.
    progress: Vec<i32>,
    outcome: Result<Vec<TranscriptSegment>, DictationError>,
}

impl Reply {
    fn says(segments: Vec<TranscriptSegment>) -> Self {
        Self {
            before: None,
            progress: vec![0, 1, 40, 100],
            outcome: Ok(segments),
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

struct ScriptedBackend {
    installed: bool,
    detection: Option<LanguageDetection>,
    detection_failures: Mutex<VecDeque<DictationError>>,
    load_failures: Mutex<VecDeque<DictationError>>,
    replies: Mutex<VecDeque<Reply>>,
    /// What the Whisper half of a diarized run answers, in order.
    #[cfg(feature = "diarization")]
    diarizations: Mutex<VecDeque<Result<DiarizationTranscription, DictationError>>>,
    /// Run when the Whisper half of a diarized run is asked, before it answers.
    #[cfg(feature = "diarization")]
    during_diarization: Mutex<Option<Hook>>,
    calls: Mutex<Vec<Call>>,
}

impl ScriptedBackend {
    fn new(replies: Vec<Reply>) -> Self {
        Self {
            installed: true,
            detection: None,
            detection_failures: Mutex::new(VecDeque::new()),
            load_failures: Mutex::new(VecDeque::new()),
            replies: Mutex::new(replies.into()),
            #[cfg(feature = "diarization")]
            diarizations: Mutex::new(VecDeque::new()),
            #[cfg(feature = "diarization")]
            during_diarization: Mutex::new(None),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Runs `hook` when the Whisper half of a diarized run is asked, before it answers.
    #[cfg(feature = "diarization")]
    fn during_diarization(
        self,
        hook: impl FnOnce() -> Result<(), DictationError> + Send + 'static,
    ) -> Self {
        *self.during_diarization.lock().unwrap() = Some(Box::new(hook));
        self
    }

    /// Scripts the answers of the Whisper half of a diarized run.
    #[cfg(feature = "diarization")]
    fn diarizing(self, replies: Vec<Result<DiarizationTranscription, DictationError>>) -> Self {
        *self.diarizations.lock().unwrap() = replies.into();
        self
    }

    fn detecting(mut self, language: &str, probability: f32) -> Self {
        self.detection = Some(LanguageDetection {
            language: language.to_string(),
            probability,
        });
        self
    }

    fn calls(&self) -> std::sync::MutexGuard<'_, Vec<Call>> {
        self.calls.lock().unwrap()
    }

    fn transcriptions(&self) -> Vec<Transcribed> {
        std::mem::take(&mut *self.calls())
            .into_iter()
            .filter_map(|call| match call {
                Call::Transcribe(transcribed) => Some(transcribed),
                _ => None,
            })
            .collect()
    }
}

impl LanguageProbe for ScriptedBackend {
    fn detect_language(&self, audio: &[f32]) -> Result<Option<LanguageDetection>, DictationError> {
        self.calls().push(Call::Detect {
            samples: audio.len(),
        });
        match self.detection_failures.lock().unwrap().pop_front() {
            Some(error) => Err(error),
            None => Ok(self.detection.clone()),
        }
    }
}

impl InferenceBackend for ScriptedBackend {
    fn is_downloaded(&self, _: WhisperModel) -> bool {
        self.installed
    }

    fn load_model(
        &self,
        model: WhisperModel,
        profile: ContextProfile,
    ) -> Result<(), DictationError> {
        self.calls().push(Call::Load(model, profile));
        match self.load_failures.lock().unwrap().pop_front() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn ensure_vad_model(&self) -> Result<PathBuf, DictationError> {
        self.calls().push(Call::EnsureVadModel);
        Ok(PathBuf::from("silero-vad.bin"))
    }

    fn transcribe_segments(
        &self,
        audio: &[f32],
        language: Language,
        options: &TranscribeOptions,
        mut on_progress: Box<dyn FnMut(i32) + Send>,
        on_encode_start: &dyn Fn(),
    ) -> Result<Vec<TranscriptSegment>, DictationError> {
        self.calls().push(Call::Transcribe(Transcribed {
            samples: audio.len(),
            language,
            prompt: options.prompt.clone(),
            beam_size: options.beam_size,
            temperature_fallback: options.temperature_fallback,
            vad: options.vad_model_path.is_some(),
            segment_timestamps: options.segment_timestamps,
            parallel_chunks: options.parallel_chunks,
        }));
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("the run asked for more transcriptions than the test scripted");
        if let Some(before) = reply.before {
            before();
        }
        on_encode_start();
        for percent in reply.progress {
            on_progress(percent);
        }
        reply.outcome
    }

    #[cfg(feature = "diarization")]
    fn transcribe_for_diarization(
        &self,
        audio: &[f32],
        _: Language,
        prompt: Option<&str>,
        _: &TranscribeOptions,
    ) -> Result<DiarizationTranscription, DictationError> {
        self.calls().push(Call::TranscribeForDiarization {
            samples: audio.len(),
            prompt: prompt.map(str::to_string),
        });
        let during = self.during_diarization.lock().unwrap().take();
        if let Some(during) = during {
            during()?;
        }
        self.diarizations
            .lock()
            .unwrap()
            .pop_front()
            .expect("the run asked for more diarization transcriptions than the test scripted")
    }
}

// ---- fixtures -----------------------------------------------------------------

#[derive(Parser)]
struct TestCli {
    #[command(flatten)]
    args: TranscribeArgs,
}

fn args_for(flags: &[&str]) -> TranscribeArgs {
    let mut argv = vec!["sagascript", "input.wav"];
    argv.extend_from_slice(flags);
    TestCli::try_parse_from(argv).expect("flags parse").args
}

/// A 16 kHz mono 16-bit WAV of `seconds` of a constant-energy signal, so every
/// frame counts as speech to the coverage check.
fn tone(dir: &TempDir, name: &str, seconds: usize) -> PathBuf {
    let samples: Vec<f32> = (0..seconds * 16_000)
        .map(|index| if index % 2 == 0 { 0.3 } else { -0.3 })
        .collect();
    let path = dir.join(name);
    std::fs::write(&path, encode_wav(&samples)).expect("write the WAV fixture");
    path
}

fn spoken(start: f64, end: f64, text: &str) -> TranscriptSegment {
    TranscriptSegment {
        start,
        end,
        text: text.to_string(),
        avg_logprob: Some(-0.313),
        no_speech_prob: 0.01,
    }
}

/// What a finished run wrote.
struct Run {
    stdout: String,
    stderr: String,
    result: Result<(), DictationError>,
}

impl Run {
    /// stderr without the `--progress-json` events.
    fn human_lines(&self) -> Vec<&str> {
        self.stderr
            .lines()
            .filter(|line| !line.starts_with('{'))
            .collect()
    }

    /// The `--progress-json` events, in order.
    fn events(&self) -> Vec<Value> {
        self.stderr
            .lines()
            .filter(|line| line.starts_with('{'))
            .map(|line| serde_json::from_str(line).expect("an event is JSON"))
            .collect()
    }

    /// The phases of the events, consecutive repeats collapsed.
    fn phases(&self) -> Vec<String> {
        let mut phases: Vec<String> = Vec::new();
        for event in self.events() {
            let phase = event["phase"].as_str().expect("an event has a phase");
            if phases.last().map(String::as_str) != Some(phase) {
                phases.push(phase.to_string());
            }
        }
        phases
    }

    /// The percent values of every event of `phase`.
    fn percents(&self, phase: &str) -> Vec<Option<u64>> {
        self.events()
            .iter()
            .filter(|event| event["phase"] == phase)
            .map(|event| event["percent"].as_u64())
            .collect()
    }

    /// The run as one reader of stderr sees it: human lines as they are, each
    /// event as `event:<phase>[:<percent>]`, consecutive `decoding` and
    /// `resampling` events (one per decoded block) collapsed to the first.
    fn trace(&self) -> Vec<String> {
        let mut trace: Vec<String> = Vec::new();
        for line in self.stderr.lines() {
            let entry = if line.starts_with('{') {
                let event: Value = serde_json::from_str(line).expect("an event is JSON");
                let phase = event["phase"].as_str().expect("an event has a phase");
                match event["percent"].as_u64() {
                    Some(percent) => format!("event:{phase}:{percent}"),
                    None => format!("event:{phase}"),
                }
            } else {
                line.to_string()
            };
            let is_block = |entry: &str| {
                entry.starts_with("event:decoding:") || entry.starts_with("event:resampling:")
            };
            if is_block(&entry) {
                let phase = entry.split(':').nth(1);
                let repeats = trace
                    .last()
                    .is_some_and(|last| is_block(last) && last.split(':').nth(1) == phase);
                // Keep the first and the last percent of a run of blocks.
                if repeats && entry.ends_with(":100") {
                    trace.push(entry);
                } else if repeats {
                    continue;
                } else {
                    trace.push(entry);
                }
            } else {
                trace.push(entry);
            }
        }
        trace
    }
}

struct Plan<'a> {
    flags: &'a [&'a str],
    stored: Settings,
    language: Language,
    model: WhisperModel,
    glossary: Glossary,
    vocabulary: Vec<String>,
}

impl Default for Plan<'_> {
    fn default() -> Self {
        Self {
            flags: &[],
            stored: Settings::default(),
            language: Language::English,
            model: WhisperModel::Base,
            glossary: Glossary::default(),
            vocabulary: Vec::new(),
        }
    }
}

fn run_batch_with(
    plan: &Plan<'_>,
    files: &[PathBuf],
    backend: &ScriptedBackend,
    canceller: &crate::cancel::Canceller,
) -> Run {
    let (out, captured) = Out::capture();
    let args = args_for(plan.flags);
    let result = run_whisper_batch(
        &WhisperBatch {
            args: &args,
            files,
            stored: &plan.stored,
            language: plan.language,
            model: plan.model,
            glossary: &plan.glossary,
            correction_vocabulary: &plan.vocabulary,
        },
        Services {
            backend,
            #[cfg(feature = "diarization")]
            analyzer: &OnnxAnalyzer,
        },
        canceller,
        &out,
    );
    Run {
        stdout: captured.stdout(),
        stderr: captured.stderr(),
        result,
    }
}

fn run_batch(plan: &Plan<'_>, files: &[PathBuf], backend: &ScriptedBackend) -> Run {
    run_batch_with(plan, files, backend, &crate::cancel::Canceller::new())
}

fn transcript(text: &str) -> Reply {
    Reply::says(vec![spoken(0.0, 3.0, text)])
}

// ---- one file ------------------------------------------------------------------

#[test]
fn a_plain_run_narrates_every_phase_in_order_and_writes_the_transcript() {
    let dir = TempDir::new();
    let file = tone(&dir, "a.wav", 3);
    let backend = ScriptedBackend::new(vec![transcript(" Hello there.")]);
    let run = run_batch(
        &Plan {
            flags: &["--progress-json"],
            ..Plan::default()
        },
        std::slice::from_ref(&file),
        &backend,
    );

    run.result.as_ref().expect("the run succeeds");
    assert_eq!(run.stdout, "Hello there.\n");
    let shown = file.display();
    assert_eq!(
        run.trace(),
        vec![
            format!("[1/1] {shown}"),
            format!("Decoding {shown}..."),
            "event:decoding:0".to_string(),
            "event:decoding:100".to_string(),
            "event:resampling:0".to_string(),
            "event:resampling:100".to_string(),
            "Audio: 3.0s, 48000 samples".to_string(),
            "event:loading".to_string(),
            "Loading model: Whisper Base...".to_string(),
            "event:language_detection".to_string(),
            "Beam search: width 5".to_string(),
            "Preparing Whisper states...".to_string(),
            "event:preparing".to_string(),
            "Transcribing...".to_string(),
            "Encoding audio...".to_string(),
            "event:encoding".to_string(),
            // Whisper's 0 and 1 are not reported; the rest is clamped to 0-100.
            "event:transcribing:40".to_string(),
            "event:transcribing:100".to_string(),
            "event:finalizing".to_string(),
            "event:completed:100".to_string(),
        ]
    );
}

#[test]
fn whispers_percent_is_clamped_and_values_up_to_one_are_not_reported() {
    let dir = TempDir::new();
    let file = tone(&dir, "a.wav", 3);
    let backend = ScriptedBackend::new(vec![Reply {
        progress: vec![-5, 0, 1, 2, 99, 250],
        ..transcript(" Hello there.")
    }]);
    let run = run_batch(
        &Plan {
            flags: &["--progress-json"],
            ..Plan::default()
        },
        &[file],
        &backend,
    );

    run.result.as_ref().expect("the run succeeds");
    assert_eq!(
        run.percents("transcribing"),
        vec![Some(2), Some(99), Some(100)]
    );
}

#[test]
fn every_event_names_its_step_of_three() {
    let dir = TempDir::new();
    let file = tone(&dir, "a.wav", 3);
    let backend = ScriptedBackend::new(vec![transcript(" Hello there.")]);
    let run = run_batch(
        &Plan {
            flags: &["--progress-json"],
            ..Plan::default()
        },
        &[file],
        &backend,
    );

    let mut steps: Vec<(String, u64)> = Vec::new();
    for event in run.events() {
        assert_eq!(event["event"], "transcription_progress");
        assert_eq!(event["steps_total"], 3);
        assert!(event["elapsed_ms"].is_u64());
        assert!(event.get("decoder").is_none(), "{event}");
        let entry = (
            event["phase"].as_str().unwrap().to_string(),
            event["step"].as_u64().unwrap(),
        );
        if steps.last() != Some(&entry) {
            steps.push(entry);
        }
    }
    let expected: Vec<(&str, u64)> = vec![
        ("decoding", 1),
        ("resampling", 2),
        ("loading", 2),
        ("language_detection", 2),
        ("preparing", 2),
        ("encoding", 2),
        ("transcribing", 3),
        ("finalizing", 3),
        ("completed", 3),
    ];
    assert_eq!(
        steps,
        expected
            .into_iter()
            .map(|(phase, step)| (phase.to_string(), step))
            .collect::<Vec<_>>()
    );
}

#[test]
fn without_progress_json_only_the_human_lines_are_written() {
    let dir = TempDir::new();
    let file = tone(&dir, "a.wav", 3);
    let backend = ScriptedBackend::new(vec![transcript(" Hello there.")]);
    let run = run_batch(&Plan::default(), std::slice::from_ref(&file), &backend);

    run.result.as_ref().expect("the run succeeds");
    let shown = file.display();
    assert_eq!(
        run.stderr.lines().collect::<Vec<_>>(),
        vec![
            format!("[1/1] {shown}"),
            format!("Decoding {shown}..."),
            "Audio: 3.0s, 48000 samples".to_string(),
            "Loading model: Whisper Base...".to_string(),
            "Beam search: width 5".to_string(),
            "Preparing Whisper states...".to_string(),
            "Transcribing...".to_string(),
            "Encoding audio...".to_string(),
        ]
    );
}

#[test]
fn a_file_over_ten_seconds_draws_a_bar_where_a_short_one_prints_a_line() {
    let dir = TempDir::new();
    let long = tone(&dir, "long.wav", 11);
    let backend = ScriptedBackend::new(vec![Reply::says(vec![spoken(0.0, 11.0, " Long.")])]);
    let run = run_batch(
        &Plan {
            flags: &["--progress-json"],
            ..Plan::default()
        },
        &[long],
        &backend,
    );

    run.result.as_ref().expect("the run succeeds");
    assert_eq!(run.stdout, "Long.\n");
    assert!(!run.human_lines().contains(&"Transcribing..."));
    assert_eq!(run.percents("transcribing"), vec![Some(40), Some(100)]);
    assert!(run.human_lines().contains(&"Encoding audio..."));
    assert_eq!(run.phases().last().map(String::as_str), Some("completed"));
}

#[test]
fn the_json_result_has_the_documented_keys_and_values() {
    let dir = TempDir::new();
    let file = tone(&dir, "a.wav", 3);
    let backend = ScriptedBackend::new(vec![transcript(" Hello there.")]).detecting("en", 0.99);
    let run = run_batch(
        &Plan {
            flags: &["--json"],
            ..Plan::default()
        },
        std::slice::from_ref(&file),
        &backend,
    );

    run.result.as_ref().expect("the run succeeds");
    let result: Value = serde_json::from_str(&run.stdout).expect("--json is one document");
    assert_eq!(
        result,
        json!({
            "text": "Hello there.",
            "segments": [{
                "start": 0.0,
                "end": 3.0,
                "text": "Hello there.",
                "avg_logprob": -0.313_f32,
                "no_speech_prob": 0.01_f32,
                "quarantined": false,
            }],
            "language": "en",
            "model": "base",
            "file": file.display().to_string(),
            "duration_seconds": 3.0,
            "coverage_ratio": 1.0,
            "uncovered_spans": [],
            "repetition_spans": [],
            "detected_language": {"language": "en", "probability": 0.99_f32},
            "language_redetection_enabled": false,
            "language_regions": null,
            "warnings": [],
            "vocabulary_corrections": [],
        })
    );
    // The documented key set, in the order serde_json writes it.
    let keys: Vec<&str> = result
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec![
            "coverage_ratio",
            "detected_language",
            "duration_seconds",
            "file",
            "language",
            "language_redetection_enabled",
            "language_regions",
            "model",
            "repetition_spans",
            "segments",
            "text",
            "uncovered_spans",
            "vocabulary_corrections",
            "warnings",
        ]
    );
}

#[test]
fn a_segment_text_is_trimmed_in_the_json_and_a_gap_in_the_transcript_is_a_warning() {
    let dir = TempDir::new();
    // The tone is speech-like all through; the transcript covers the first two seconds only.
    let file = tone(&dir, "a.wav", 12);
    let backend = ScriptedBackend::new(vec![Reply::says(vec![spoken(0.0, 2.0, "  Only this. ")])]);
    let run = run_batch(
        &Plan {
            flags: &["--json"],
            ..Plan::default()
        },
        &[file],
        &backend,
    );

    run.result.as_ref().expect("the run succeeds");
    let result: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(result["segments"][0]["text"], "Only this.");
    assert_eq!(result["text"], "Only this.");
    assert_eq!(result["duration_seconds"], 12.0);
    let spans = result["uncovered_spans"].as_array().unwrap();
    assert_eq!(spans.len(), 1, "{result}");
    assert_eq!(spans[0]["start"], 2.0);
    assert_eq!(spans[0]["end"], 12.0);
    assert!((result["coverage_ratio"].as_f64().unwrap() - 2.0 / 12.0).abs() < 0.01);
    let message = "Transcript has no segment from 2.00s to 12.00s (10.00s), but 10.00s of that \
                   span appears to contain speech.";
    assert_eq!(
        result["warnings"],
        json!([{"code": "uncovered_speech", "message": message, "start": 2.0, "end": 12.0}])
    );
    // The same warning is a human line on stderr.
    let line = format!("Warning [uncovered_speech]: {message}");
    assert!(run.human_lines().contains(&line.as_str()), "{}", run.stderr);
}

// ---- the language check ---------------------------------------------------------

#[test]
fn a_strong_language_mismatch_warns_on_stderr_and_in_the_json() {
    let dir = TempDir::new();
    let file = tone(&dir, "a.wav", 3);
    let backend = ScriptedBackend::new(vec![transcript(" Hej.")]).detecting("sv", 0.97);
    let run = run_batch(
        &Plan {
            flags: &["--json", "--progress-json"],
            ..Plan::default()
        },
        &[file],
        &backend,
    );

    run.result.as_ref().expect("the run succeeds");
    let message = "Configured language is English (`en`), but the audio strongly appears to be \
                   Swedish (`sv`, p=0.970). Re-run with `--language sv`; the saved setting was \
                   not changed.";
    let result: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(
        result["detected_language"],
        json!({"language": "sv", "probability": 0.97_f32})
    );
    assert_eq!(
        result["warnings"],
        json!([{"code": "language_mismatch", "message": message}])
    );
    // The warning comes after "finalizing" and before "completed".
    let trace = run.trace();
    let at = |entry: &str| trace.iter().position(|line| line == entry).unwrap();
    let warning = trace
        .iter()
        .position(|line| line == &format!("Warning [language_mismatch]: {message}"))
        .expect("the warning is on stderr");
    assert!(at("event:finalizing") < warning && warning < at("event:completed:100"));
}

#[test]
fn a_detection_that_agrees_or_is_weak_adds_no_warning() {
    let dir = TempDir::new();
    let file = tone(&dir, "a.wav", 3);
    for (language, probability) in [("en", 0.99), ("sv", 0.70)] {
        let backend =
            ScriptedBackend::new(vec![transcript(" Hello.")]).detecting(language, probability);
        let run = run_batch(
            &Plan {
                flags: &["--json"],
                ..Plan::default()
            },
            std::slice::from_ref(&file),
            &backend,
        );
        let result: Value = serde_json::from_str(&run.stdout).unwrap();
        assert_eq!(result["warnings"], json!([]), "{language} {probability}");
        assert_eq!(result["detected_language"]["language"], language);
        assert!(
            !run.stderr.contains("Warning"),
            "{language} {probability}: {}",
            run.stderr
        );
    }
}

#[test]
fn an_english_only_model_is_never_asked_to_detect_a_language() {
    let dir = TempDir::new();
    let file = tone(&dir, "a.wav", 3);
    let backend = ScriptedBackend::new(vec![transcript(" Hello.")]).detecting("sv", 0.99);
    let run = run_batch(
        &Plan {
            flags: &["--json"],
            model: WhisperModel::TinyEn,
            ..Plan::default()
        },
        &[file],
        &backend,
    );

    let result: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(result["detected_language"], Value::Null);
    assert_eq!(result["warnings"], json!([]));
    assert!(!backend
        .calls()
        .iter()
        .any(|call| matches!(call, Call::Detect { .. })));
}

#[test]
fn a_language_check_that_cannot_run_is_a_warning_and_the_file_goes_on() {
    let dir = TempDir::new();
    let file = tone(&dir, "a.wav", 3);
    let backend = ScriptedBackend::new(vec![transcript(" Hello there.")]);
    backend
        .detection_failures
        .lock()
        .unwrap()
        .push_back(DictationError::TranscriptionFailed("probe broke".into()));
    let run = run_batch(
        &Plan {
            flags: &["--json"],
            ..Plan::default()
        },
        &[file],
        &backend,
    );

    run.result.as_ref().expect("the run succeeds");
    assert!(run.human_lines().contains(
        &"Warning: local language detection was unavailable: Transcription failed: probe broke"
    ));
    let result: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(result["text"], "Hello there.");
    assert_eq!(result["detected_language"], Value::Null);
    assert_eq!(result["warnings"], json!([]));
}

// ---- corrections ------------------------------------------------------------------

#[test]
fn glossary_aliases_correct_the_text_and_are_reported() {
    let dir = TempDir::new();
    let file = tone(&dir, "a.wav", 3);
    let glossary = Glossary::parse("OpenRouter = open router");
    let backend = ScriptedBackend::new(vec![Reply::says(vec![TranscriptSegment {
        avg_logprob: None,
        ..spoken(0.0, 3.0, " Jag använder open router varje dag.")
    }])]);
    let run = run_batch(
        &Plan {
            flags: &["--json"],
            glossary: glossary.clone(),
            ..Plan::default()
        },
        &[file],
        &backend,
    );

    run.result.as_ref().expect("the run succeeds");
    let result: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(result["text"], "Jag använder OpenRouter varje dag.");
    assert_eq!(
        result["segments"][0]["text"],
        "Jag använder OpenRouter varje dag."
    );
    assert_eq!(
        result["vocabulary_corrections"],
        json!([{
            "original": "open router",
            "replacement": "OpenRouter",
            "segment_index": 0,
            "avg_logprob": null,
            "no_speech_prob": 0.01_f32,
            "method": "explicit_alias",
        }])
    );
    // The glossary also primes the decoder.
    assert_eq!(
        backend.transcriptions()[0].prompt,
        glossary.decoder_prompt()
    );
}

#[test]
fn hint_correction_repairs_a_one_edit_miss_only_when_asked_to() {
    let dir = TempDir::new();
    let file = tone(&dir, "a.wav", 3);
    let glossary = Glossary::parse("Grimnir");
    let heard = || Reply::says(vec![spoken(0.0, 3.0, " Grimner och Grimner.")]);

    let backend = ScriptedBackend::new(vec![heard()]);
    let run = run_batch(
        &Plan {
            flags: &["--json", "--correct-hints"],
            glossary: glossary.clone(),
            vocabulary: glossary.single_word_terms(),
            ..Plan::default()
        },
        std::slice::from_ref(&file),
        &backend,
    );
    let result: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(result["text"], "Grimnir och Grimnir.");
    let corrections = result["vocabulary_corrections"].as_array().unwrap();
    assert_eq!(corrections.len(), 2, "{result}");
    for correction in corrections {
        assert_eq!(correction["original"], "Grimner");
        assert_eq!(correction["replacement"], "Grimnir");
        assert_eq!(correction["method"], "fuzzy_one_edit");
        assert_eq!(correction["segment_index"], 0);
        assert_eq!(correction["avg_logprob"], json!(-0.313_f32));
    }

    // The vocabulary is given, but the flag is not: the text stays as heard.
    let backend = ScriptedBackend::new(vec![heard()]);
    let run = run_batch(
        &Plan {
            flags: &["--json"],
            glossary: glossary.clone(),
            vocabulary: glossary.single_word_terms(),
            ..Plan::default()
        },
        &[file],
        &backend,
    );
    let result: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(result["text"], "Grimner och Grimner.");
    assert_eq!(result["vocabulary_corrections"], json!([]));
}

// ---- what the backend is asked for -------------------------------------------------

#[test]
fn the_decoder_options_follow_the_flags_and_say_so_on_stderr() {
    let dir = TempDir::new();
    let file = tone(&dir, "a.wav", 3);
    let cases: [(&[&str], u32, usize, &[&str]); 5] = [
        // File runs default to a beam search of 5 (the saved beam is 0).
        (&[], 5, 1, &["Beam search: width 5"]),
        (&["--beam", "0"], 0, 1, &[]),
        (&["--beam", "2"], 2, 1, &["Beam search: width 2"]),
        (
            &["--beam", "3", "--parallel", "3"],
            3,
            3,
            &["Beam search: width 3", "Parallel Whisper states: 3"],
        ),
        (
            &["--parallel", "2", "--beam", "0"],
            0,
            2,
            &["Parallel Whisper states: 2"],
        ),
    ];
    for (flags, beam_size, parallel_chunks, announced) in cases {
        let backend = ScriptedBackend::new(vec![transcript(" Hi.")]);
        let run = run_batch(
            &Plan {
                flags,
                glossary: Glossary::parse("Sara, Estrid"),
                ..Plan::default()
            },
            std::slice::from_ref(&file),
            &backend,
        );
        run.result.as_ref().expect("the run succeeds");
        assert_eq!(
            backend.transcriptions(),
            vec![Transcribed {
                samples: 48_000,
                language: Language::English,
                prompt: Glossary::parse("Sara, Estrid").decoder_prompt(),
                beam_size,
                temperature_fallback: true,
                vad: false,
                segment_timestamps: true,
                parallel_chunks,
            }],
            "{flags:?}"
        );
        let lines = run.human_lines();
        for line in ["Beam search: width", "Parallel Whisper states:"] {
            let said: Vec<&&str> = lines.iter().filter(|l| l.starts_with(line)).collect();
            let expected: Vec<&&str> = announced.iter().filter(|l| l.starts_with(line)).collect();
            assert_eq!(said, expected, "{flags:?}");
        }
        assert!(!lines.contains(&"VAD: enabled"), "{flags:?}");
    }
}

#[test]
fn voice_activity_detection_follows_the_flags_and_then_the_saved_setting() {
    let dir = TempDir::new();
    let file = tone(&dir, "a.wav", 3);
    let cases: [(bool, &[&str], bool); 4] = [
        (false, &[], false),
        (false, &["--vad"], true),
        (true, &[], true),
        (true, &["--no-vad"], false),
    ];
    for (saved, flags, expected) in cases {
        let backend = ScriptedBackend::new(vec![transcript(" Hi.")]);
        let run = run_batch(
            &Plan {
                flags,
                stored: Settings {
                    vad_enabled: saved,
                    ..Settings::default()
                },
                ..Plan::default()
            },
            std::slice::from_ref(&file),
            &backend,
        );
        run.result.as_ref().expect("the run succeeds");
        let context = format!("saved={saved} flags={flags:?}");
        assert_eq!(backend.transcriptions()[0].vad, expected, "{context}");
        let lines = run.human_lines();
        let position = |text: &str| lines.iter().position(|line| *line == text);
        assert_eq!(
            position("Verifying Silero VAD model...").is_some(),
            expected,
            "{context}: {lines:?}"
        );
        assert_eq!(position("VAD: enabled").is_some(), expected, "{context}");
        if expected {
            // Announced after the beam search, checked before the model is prepared.
            assert!(
                position("Verifying Silero VAD model...") < position("Beam search: width 5"),
                "{context}: {lines:?}"
            );
            assert!(
                position("Beam search: width 5") < position("VAD: enabled")
                    && position("VAD: enabled") < position("Preparing Whisper states..."),
                "{context}: {lines:?}"
            );
        }
    }
}

#[test]
fn the_model_is_loaded_for_ordinary_file_runs_with_the_flash_attention_profile() {
    let dir = TempDir::new();
    let file = tone(&dir, "a.wav", 3);
    let backend = ScriptedBackend::new(vec![transcript(" Hi.")]).detecting("en", 0.99);
    run_batch(&Plan::default(), &[file], &backend)
        .result
        .expect("the run succeeds");

    assert_eq!(
        *backend.calls(),
        vec![
            Call::Load(WhisperModel::Base, ContextProfile::FlashAttention),
            Call::Detect { samples: 48_000 },
            Call::Transcribe(Transcribed {
                samples: 48_000,
                language: Language::English,
                prompt: None,
                beam_size: 5,
                temperature_fallback: true,
                vad: false,
                segment_timestamps: true,
                parallel_chunks: 1,
            }),
        ]
    );
}

// ---- a batch -------------------------------------------------------------------------

fn three_files(dir: &TempDir) -> Vec<PathBuf> {
    ["a.wav", "b.wav", "c.wav"]
        .iter()
        .map(|name| tone(dir, name, 3))
        .collect()
}

#[test]
fn a_batch_loads_the_model_once_and_writes_a_header_per_file() {
    let dir = TempDir::new();
    let files = three_files(&dir)[..2].to_vec();
    let backend = ScriptedBackend::new(vec![transcript(" First."), transcript(" Second.")])
        .detecting("en", 0.99);
    let run = run_batch(&Plan::default(), &files, &backend);

    run.result.as_ref().expect("the run succeeds");
    assert_eq!(
        run.stdout,
        format!(
            "==> {} <==\nFirst.\n==> {} <==\nSecond.\n",
            files[0].display(),
            files[1].display()
        )
    );
    let lines = run.human_lines();
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.starts_with("Loading model"))
            .count(),
        1,
        "{lines:?}"
    );
    assert!(lines.contains(&format!("[1/2] {}", files[0].display()).as_str()));
    assert!(lines.contains(&format!("[2/2] {}", files[1].display()).as_str()));
    let kinds: Vec<&str> = backend
        .calls()
        .iter()
        .map(|call| match call {
            Call::Load(..) => "load",
            Call::Detect { .. } => "detect",
            Call::EnsureVadModel => "vad",
            Call::Transcribe(_) => "transcribe",
            #[cfg(feature = "diarization")]
            Call::TranscribeForDiarization { .. } => "transcribe-for-diarization",
        })
        .collect();
    assert_eq!(
        kinds,
        ["load", "detect", "transcribe", "detect", "transcribe"]
    );
}

#[test]
fn a_batch_in_json_mode_writes_one_array_after_the_last_file() {
    let dir = TempDir::new();
    let files = three_files(&dir)[..2].to_vec();
    let backend = ScriptedBackend::new(vec![transcript(" First."), transcript(" Second.")]);
    let run = run_batch(
        &Plan {
            flags: &["--json"],
            ..Plan::default()
        },
        &files,
        &backend,
    );

    run.result.as_ref().expect("the run succeeds");
    let items: Value = serde_json::from_str(&run.stdout).unwrap();
    let items = items.as_array().expect("several inputs make an array");
    assert_eq!(items.len(), 2);
    for (item, (file, text)) in items.iter().zip(files.iter().zip(["First.", "Second."])) {
        assert_eq!(item["status"], "ok");
        assert_eq!(item["source"], file.display().to_string());
        assert_eq!(item["result"]["text"], text);
        assert_eq!(item["result"]["file"], file.display().to_string());
    }
}

#[test]
fn a_batch_in_jsonl_mode_writes_each_item_when_its_file_is_done() {
    let dir = TempDir::new();
    let files = three_files(&dir)[..2].to_vec();
    let backend = ScriptedBackend::new(vec![transcript(" First."), transcript(" Second.")]);
    let run = run_batch(
        &Plan {
            flags: &["--jsonl"],
            ..Plan::default()
        },
        &files,
        &backend,
    );

    run.result.as_ref().expect("the run succeeds");
    let lines: Vec<Value> = run
        .stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["result"]["text"], "First.");
    assert_eq!(lines[1]["result"]["text"], "Second.");
}

#[test]
fn a_failing_file_leaves_the_others_written_and_fails_the_batch() {
    let dir = TempDir::new();
    let files = three_files(&dir);
    let backend = ScriptedBackend::new(vec![
        transcript(" First."),
        Reply::fails(DictationError::TranscriptionFailed("worker died".into())),
        transcript(" Third."),
    ]);
    let run = run_batch(&Plan::default(), &files, &backend);

    assert_eq!(
        run.stdout,
        format!(
            "==> {} <==\nFirst.\n==> {} <==\nThird.\n",
            files[0].display(),
            files[2].display()
        )
    );
    assert!(run.human_lines().contains(
        &format!(
            "Error transcribing {}: Transcription failed: worker died",
            files[1].display()
        )
        .as_str()
    ));
    let error = run.result.expect_err("one of three failed");
    assert_eq!(
        error.to_string(),
        "Transcription failed: 1 of 3 batch item(s) failed"
    );
    assert_eq!(backend.transcriptions().len(), 3);
}

#[test]
fn a_failing_file_is_an_error_item_in_json_mode() {
    let dir = TempDir::new();
    let files = three_files(&dir);
    let backend = ScriptedBackend::new(vec![
        transcript(" First."),
        Reply::fails(DictationError::TranscriptionFailed("worker died".into())),
        transcript(" Third."),
    ]);
    let run = run_batch(
        &Plan {
            flags: &["--json"],
            ..Plan::default()
        },
        &files,
        &backend,
    );

    let items: Value = serde_json::from_str(&run.stdout).unwrap();
    let statuses: Vec<&str> = items
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["ok", "error", "ok"]);
    assert_eq!(items[1]["source"], files[1].display().to_string());
    assert_eq!(items[1]["error"], "Transcription failed: worker died");
    assert!(run.result.is_err());
}

#[test]
fn fail_fast_stops_at_the_first_failure_and_does_not_decode_the_rest() {
    let dir = TempDir::new();
    let files = three_files(&dir);
    let backend = ScriptedBackend::new(vec![
        transcript(" First."),
        Reply::fails(DictationError::TranscriptionFailed("worker died".into())),
        transcript(" Third."),
    ]);
    let run = run_batch(
        &Plan {
            flags: &["--fail-fast"],
            ..Plan::default()
        },
        &files,
        &backend,
    );

    assert_eq!(
        run.result.expect_err("the second failed").to_string(),
        "Transcription failed: 1 of 2 batch item(s) failed"
    );
    assert_eq!(backend.transcriptions().len(), 2);
    assert!(!run.stderr.contains(&files[2].display().to_string()));
}

#[test]
fn a_cancel_ends_the_batch_keeps_earlier_output_and_writes_no_partial_result() {
    let dir = TempDir::new();
    let files = three_files(&dir);
    let canceller = std::sync::Arc::new(crate::cancel::Canceller::new());
    let cancel_now = {
        let canceller = canceller.clone();
        Box::new(move || {
            canceller.request();
        }) as Box<dyn FnOnce() + Send>
    };
    let backend = ScriptedBackend::new(vec![
        transcript(" First."),
        Reply {
            before: Some(cancel_now),
            progress: vec![40],
            outcome: Err(DictationError::Cancelled),
        },
        transcript(" Never."),
    ]);
    let run = run_batch_with(
        &Plan {
            flags: &["--jsonl"],
            ..Plan::default()
        },
        &files,
        &backend,
        &canceller,
    );

    assert!(matches!(run.result, Err(DictationError::Cancelled)));
    // The first file's item was written when it finished; nothing for the second.
    let lines: Vec<Value> = run
        .stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 1, "{}", run.stdout);
    assert_eq!(lines[0]["result"]["text"], "First.");
    assert_eq!(backend.transcriptions().len(), 2, "the third never started");
    assert!(!run.stderr.contains("Error transcribing"));
}

#[test]
fn a_cancel_in_json_mode_writes_no_document_at_all() {
    let dir = TempDir::new();
    let files = three_files(&dir);
    let canceller = std::sync::Arc::new(crate::cancel::Canceller::new());
    let cancel_now = {
        let canceller = canceller.clone();
        Box::new(move || {
            canceller.request();
        }) as Box<dyn FnOnce() + Send>
    };
    let backend = ScriptedBackend::new(vec![
        transcript(" First."),
        Reply {
            before: Some(cancel_now),
            progress: Vec::new(),
            outcome: Err(DictationError::Cancelled),
        },
    ]);
    let run = run_batch_with(
        &Plan {
            flags: &["--json"],
            ..Plan::default()
        },
        &files,
        &backend,
        &canceller,
    );

    assert!(matches!(run.result, Err(DictationError::Cancelled)));
    assert_eq!(run.stdout, "");
}

// ---- the model ---------------------------------------------------------------------------

#[test]
fn a_model_that_is_not_installed_fails_the_file_without_loading_it() {
    let dir = TempDir::new();
    let file = tone(&dir, "a.wav", 3);
    let mut backend = ScriptedBackend::new(Vec::new());
    backend.installed = false;
    let run = run_batch(&Plan::default(), std::slice::from_ref(&file), &backend);

    let message = "Transcription failed: Model 'Whisper Base' is not downloaded. \
                   Run: sagascript download-model base";
    assert!(run
        .human_lines()
        .contains(&format!("Error transcribing {}: {message}", file.display()).as_str()));
    assert!(run.result.is_err());
    assert!(backend.calls().is_empty());
}

#[test]
fn a_failed_load_is_retried_for_the_next_file() {
    let dir = TempDir::new();
    let files = three_files(&dir)[..2].to_vec();
    let backend = ScriptedBackend::new(vec![transcript(" Second.")]);
    backend
        .load_failures
        .lock()
        .unwrap()
        .push_back(DictationError::TranscriptionFailed("out of memory".into()));
    let run = run_batch(&Plan::default(), &files, &backend);

    assert_eq!(
        run.stdout,
        format!("==> {} <==\nSecond.\n", files[1].display())
    );
    assert_eq!(
        run.result.expect_err("the first file failed").to_string(),
        "Transcription failed: 1 of 2 batch item(s) failed"
    );
    let loads = backend
        .calls()
        .iter()
        .filter(|call| matches!(call, Call::Load(..)))
        .count();
    assert_eq!(loads, 2);
}

// ---- the output step on its own --------------------------------------------------------

/// `assemble_plain_result` takes what Whisper said and returns the result: no
/// backend, no output, no clock.
mod assembling_the_plain_result {
    use sagascript_core::transcription::diagnostics::{
        LanguageRegionDiagnostics, TranscriptionWarning,
    };

    use super::*;
    use crate::transcribe::inference::LanguageEvidence;
    use crate::transcribe::output::{assemble_plain_result, PlainInput, PlainResult};

    fn evidence(
        detected_language: Option<LanguageDetection>,
        language_regions: Option<LanguageRegionDiagnostics>,
    ) -> LanguageEvidence {
        LanguageEvidence {
            model_load_seconds: 0.0,
            detected_language,
            language_regions,
            language_detection_seconds: 0.0,
        }
    }

    fn assemble(
        segments: Vec<TranscriptSegment>,
        language: Language,
        glossary: &Glossary,
        evidence: &LanguageEvidence,
    ) -> PlainResult {
        // Silence has no speech for the coverage check to miss.
        assemble_over(
            &vec![0.0_f32; 60 * 16_000],
            segments,
            language,
            glossary,
            evidence,
        )
    }

    fn assemble_over(
        audio: &[f32],
        segments: Vec<TranscriptSegment>,
        language: Language,
        glossary: &Glossary,
        evidence: &LanguageEvidence,
    ) -> PlainResult {
        assemble_plain_result(PlainInput {
            segments,
            audio,
            language,
            model: WhisperModel::Base,
            file: Path::new("clip.wav"),
            duration: audio.len() as f64 / 16_000.0,
            glossary,
            hint_vocabulary: None,
            language_evidence: evidence,
        })
    }

    /// A greeting, a loop of eight "thank you" segments over forty seconds, a farewell.
    fn with_a_loop() -> Vec<TranscriptSegment> {
        let mut segments = vec![spoken(0.0, 5.0, " Hello there.")];
        for index in 0..8 {
            let start = 10.0 + index as f64 * 5.0;
            segments.push(spoken(start, start + 5.0, " Thank you. Thank you."));
        }
        segments.push(spoken(50.0, 55.0, " Goodbye."));
        segments
    }

    #[test]
    fn a_repetition_loop_is_quarantined_in_the_json_and_left_out_of_the_text() {
        let result = assemble(
            with_a_loop(),
            Language::English,
            &Glossary::default(),
            &evidence(None, None),
        );

        assert_eq!(result.text, "Hello there. Goodbye.");
        assert_eq!(result.json["text"], "Hello there. Goodbye.");
        let quarantined: Vec<bool> = result.json["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|segment| segment["quarantined"].as_bool().unwrap())
            .collect();
        assert_eq!(
            quarantined,
            [false, true, true, true, true, true, true, true, true, false]
        );
        // Every segment keeps its own words in the JSON.
        assert_eq!(result.json["segments"][1]["text"], "Thank you. Thank you.");
        let spans = result.json["repetition_spans"].as_array().unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0]["pattern"], "thank you");
        assert_eq!(spans[0]["repetitions"], 16);
        assert_eq!(spans[0]["segment_indices"], json!([1, 2, 3, 4, 5, 6, 7, 8]));
        let codes: Vec<&str> = result.warnings.iter().map(|w| w.code.as_str()).collect();
        assert_eq!(codes, ["repetitive_hallucination"]);
        assert_eq!(
            result.json["warnings"][0]["code"],
            "repetitive_hallucination"
        );
    }

    #[test]
    fn coverage_is_judged_on_the_trusted_segments_so_a_quarantined_loop_is_not_covered() {
        // Speech-like all through the 55 s; the loop at 10-50 s is set aside, so
        // what the transcript covers is the greeting (0-5 s) and the farewell
        // (50-55 s) and the 45 s between them are speech nobody transcribed.
        let audio: Vec<f32> = (0..55 * 16_000)
            .map(|index| if index % 2 == 0 { 0.3 } else { -0.3 })
            .collect();
        let result = assemble_over(
            &audio,
            with_a_loop(),
            Language::English,
            &Glossary::default(),
            &evidence(None, None),
        );

        let spans = result.json["uncovered_spans"].as_array().unwrap();
        assert_eq!(spans.len(), 1, "{}", result.json["uncovered_spans"]);
        assert_eq!(spans[0]["start"], 5.0);
        assert_eq!(spans[0]["end"], 50.0);
        assert!((result.json["coverage_ratio"].as_f64().unwrap() - 10.0 / 55.0).abs() < 0.01);
        let codes: Vec<&str> = result.warnings.iter().map(|w| w.code.as_str()).collect();
        assert_eq!(codes, ["uncovered_speech", "repetitive_hallucination"]);
        assert_eq!(result.warnings[0].start, Some(5.0));
        assert_eq!(result.warnings[0].end, Some(50.0));
        // The loop is still in the JSON, flagged, and out of the text.
        assert_eq!(result.text, "Hello there. Goodbye.");
        assert_eq!(result.json["segments"][1]["quarantined"], true);
    }

    #[test]
    fn warnings_come_in_a_fixed_order_and_regions_are_reported() {
        let region_warning = TranscriptionWarning {
            code: "mixed_language_audio".to_string(),
            message: "two languages".to_string(),
            start: Some(0.0),
            end: Some(55.0),
        };
        let regions = LanguageRegionDiagnostics {
            regions: Vec::new(),
            warnings: vec![region_warning.clone()],
        };
        let detected = LanguageDetection {
            language: "sv".to_string(),
            probability: 0.97,
        };
        let result = assemble(
            with_a_loop(),
            Language::English,
            &Glossary::default(),
            &evidence(Some(detected), Some(regions)),
        );

        // Coverage (none here), the language mismatch, the sampled regions, repetition.
        let codes: Vec<&str> = result.warnings.iter().map(|w| w.code.as_str()).collect();
        assert_eq!(
            codes,
            [
                "language_mismatch",
                "mixed_language_audio",
                "repetitive_hallucination"
            ]
        );
        assert_eq!(result.warnings[1], region_warning);
        assert_eq!(result.json["language_redetection_enabled"], true);
        assert_eq!(result.json["language_regions"], json!([]));
        assert_eq!(result.json["detected_language"]["language"], "sv");
    }

    #[test]
    fn nonspeech_markers_are_normalized_in_the_text_and_kept_in_the_segments() {
        let result = assemble(
            vec![
                spoken(0.0, 2.0, " Hej [Music] hej."),
                spoken(2.0, 4.0, " [BLANK_AUDIO]"),
            ],
            Language::Swedish,
            &Glossary::default(),
            &evidence(None, None),
        );

        assert_eq!(result.text, "Hej [MUSIK] hej.");
        assert_eq!(result.json["segments"][0]["text"], "Hej [Music] hej.");
        assert_eq!(result.json["segments"][1]["text"], "[BLANK_AUDIO]");
        assert_eq!(result.json["language"], "sv");
    }

    #[test]
    fn the_same_input_gives_the_same_result() {
        let first = assemble(
            with_a_loop(),
            Language::English,
            &Glossary::parse("Thanks = thank you"),
            &evidence(None, None),
        );
        let second = assemble(
            with_a_loop(),
            Language::English,
            &Glossary::parse("Thanks = thank you"),
            &evidence(None, None),
        );
        assert_eq!(first.json, second.json);
        assert_eq!(first.text, second.text);
        assert_eq!(first.warnings, second.warnings);
        // The glossary acted on the segments, so its corrections are in the JSON.
        assert!(!first.json["vocabulary_corrections"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}

// ---- the diarized path --------------------------------------------------------

#[cfg(feature = "diarization")]
#[path = "transcribe_diarized_tests.rs"]
mod diarized;
