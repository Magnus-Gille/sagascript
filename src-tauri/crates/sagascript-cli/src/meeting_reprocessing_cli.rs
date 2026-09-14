//! CLI adapter for explicit meeting reprocessing plans and proposals.
//!
//! This layer only parses arguments, resolves the same settings inputs as the
//! transcription command, and delegates work to the validated reprocessing
//! orchestration API.  It never replaces an existing review or output file.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use sagascript_core::error::DictationError;
use sagascript_core::meeting_reprocess_plan::{ReprocessingMode, ReprocessingPlan};
use sagascript_core::meeting_review::MeetingReview;
use sagascript_core::settings::{Language, Settings, WhisperModel};
use sagascript_core::transcription::{Glossary, WhisperBackend};
use serde_json::json;

use crate::meeting_proposal::{read_json, write_json_new};
use crate::meeting_reprocessing::{
    execute_reprocessing, plan_reprocessing, ReprocessingInput, ReprocessingResult,
};

const MAX_REPROCESSING_JSON_BYTES: usize = 24 * 1024 * 1024;

#[derive(Args, Debug)]
pub struct ReprocessingArgs {
    #[command(subcommand)]
    pub action: ReprocessingAction,
}

#[derive(Subcommand, Debug)]
pub enum ReprocessingAction {
    /// Create a validated plan without decoding audio or loading a model.
    Plan(PlanArgs),
    /// Execute exactly one previously saved plan and publish a new proposal.
    Execute(ExecuteArgs),
}

#[derive(Args, Debug)]
pub struct PlanArgs {
    /// Explicit local audio input.
    pub audio: PathBuf,
    /// Existing review whose corrections must be preserved.
    #[arg(long = "previous-review")]
    pub previous_review: PathBuf,
    /// Reprocessing mode.
    #[arg(long, value_enum, default_value_t = ReprocessingModeArg::Recluster)]
    pub mode: ReprocessingModeArg,
    /// Agglomerative clustering threshold, in the inclusive range 0.0..=2.0.
    #[arg(long, value_parser = parse_threshold, default_value = "0.75")]
    pub threshold: f32,
    /// Existing cache for selective modes.
    #[arg(long)]
    pub cache: Option<PathBuf>,
    #[command(flatten)]
    pub runtime: RuntimeArgs,
    /// New plan artifact; an existing path is rejected.
    #[arg(long)]
    pub output: PathBuf,
}

#[derive(Args, Debug)]
pub struct ExecuteArgs {
    /// Previously saved, validated plan artifact.
    pub plan: PathBuf,
    /// Explicit local audio input.
    pub audio: PathBuf,
    /// Existing review whose corrections must be preserved.
    #[arg(long = "previous-review")]
    pub previous_review: PathBuf,
    /// Exact plan revision to execute.
    #[arg(long = "expected-revision")]
    pub expected_revision: String,
    /// Existing cache for selective modes.
    #[arg(long)]
    pub cache: Option<PathBuf>,
    /// New cache artifact, only for full recomputation.
    #[arg(long)]
    pub cache_output: Option<PathBuf>,
    #[command(flatten)]
    pub runtime: RuntimeArgs,
    /// New proposal artifact; an existing path is rejected.
    #[arg(long)]
    pub output: PathBuf,
}

#[derive(Args, Debug)]
pub struct RuntimeArgs {
    /// Language for transcription.
    #[arg(long, conflicts_with = "profile")]
    pub language: Option<String>,
    /// Use this profile's language and scoped dictionary.
    #[arg(long, conflicts_with = "language")]
    pub profile: Option<String>,
    /// Whisper model ID.
    #[arg(long)]
    pub model: Option<String>,
    /// One-run glossary prompt.
    #[arg(long, conflicts_with = "prompt_file")]
    pub prompt: Option<String>,
    /// Read the one-run glossary prompt from a file.
    #[arg(long, conflicts_with = "prompt")]
    pub prompt_file: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum ReprocessingModeArg {
    Recluster,
    Rediarize,
    Full,
}

impl From<ReprocessingModeArg> for ReprocessingMode {
    fn from(mode: ReprocessingModeArg) -> Self {
        match mode {
            ReprocessingModeArg::Recluster => Self::Recluster,
            ReprocessingModeArg::Rediarize => Self::Rediarize,
            ReprocessingModeArg::Full => Self::Full,
        }
    }
}

#[derive(Debug)]
struct RuntimeInputs {
    stored: Settings,
    language: Language,
    model: WhisperModel,
    glossary: Glossary,
}

pub fn run(args: ReprocessingArgs) -> Result<(), DictationError> {
    match args.action {
        ReprocessingAction::Plan(args) => run_plan(args),
        ReprocessingAction::Execute(args) => run_execute(args),
    }
}

fn run_plan(args: PlanArgs) -> Result<(), DictationError> {
    ensure_output_absent(&args.output)?;
    let runtime = resolve_runtime(&args.runtime)?;
    let previous: MeetingReview = read_json(
        &args.previous_review,
        MAX_REPROCESSING_JSON_BYTES,
        "previous review",
    )?;
    let input = ReprocessingInput {
        audio: &args.audio,
        cache: args.cache.as_deref(),
        previous: &previous,
        language: runtime.language,
        model: runtime.model,
        glossary: &runtime.glossary,
    };
    let plan = plan_reprocessing(&input, args.mode.into(), args.threshold, None)?;
    write_json_new(&args.output, &plan)
}

fn run_execute(args: ExecuteArgs) -> Result<(), DictationError> {
    ensure_output_absent(&args.output)?;
    if let Some(cache_output) = args.cache_output.as_deref() {
        ensure_output_absent(cache_output)?;
        ensure_distinct_outputs(&args.output, cache_output)?;
    }
    let plan: ReprocessingPlan =
        read_json(&args.plan, MAX_REPROCESSING_JSON_BYTES, "reprocessing plan")?;
    let previous: MeetingReview = read_json(
        &args.previous_review,
        MAX_REPROCESSING_JSON_BYTES,
        "previous review",
    )?;
    let runtime = resolve_runtime(&args.runtime)?;
    let input = ReprocessingInput {
        audio: &args.audio,
        cache: args.cache.as_deref(),
        previous: &previous,
        language: runtime.language,
        model: runtime.model,
        glossary: &runtime.glossary,
    };
    // Construction is cheap and does not load model weights.  The execution
    // API performs the lazy model load only when the validated plan needs it.
    let whisper = WhisperBackend::new();
    let result = execute_reprocessing(
        &plan,
        &args.expected_revision,
        &input,
        &runtime.stored,
        &whisper,
        args.cache_output.as_deref(),
        None,
    )?;
    ensure_previous_revision_unchanged(&args.previous_review, &previous.revision)?;
    publish_result(&args.output, result)
}

fn publish_result(path: &Path, result: ReprocessingResult) -> Result<(), DictationError> {
    let ReprocessingResult {
        proposal,
        required_work,
        timings,
    } = result;
    write_json_new(path, &proposal)?;
    let diagnostics = json!({
        "required_work": required_work,
        "timings": timings,
    });
    let serialized = serde_json::to_string(&diagnostics).map_err(|_| {
        DictationError::TranscriptionFailed(
            "reprocessing diagnostics could not be serialized".to_string(),
        )
    })?;
    eprintln!("{serialized}");
    Ok(())
}

fn resolve_runtime(args: &RuntimeArgs) -> Result<RuntimeInputs, DictationError> {
    let stored = sagascript_core::settings::store::load();
    let profile = args
        .profile
        .as_deref()
        .map(|profile_id| crate::transcribe::resolve_profile(&stored, profile_id))
        .transpose()?;
    let language = match (&profile, &args.language) {
        (Some(profile), _) => profile.language,
        (None, Some(language)) => crate::transcribe::parse_language(language)?,
        (None, None) => stored.language,
    };
    let model = crate::transcribe::resolve_effective_model(
        args.model.as_deref(),
        language,
        stored.auto_select_model,
        stored.whisper_model,
    )?;
    let glossary = crate::transcribe::effective_glossary(
        &stored,
        args.profile.as_deref(),
        args.prompt.as_deref(),
        args.prompt_file.as_deref(),
    )?;
    Ok(RuntimeInputs {
        stored,
        language,
        model,
        glossary,
    })
}

fn ensure_output_absent(path: &Path) -> Result<(), DictationError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(DictationError::FileDecodeError(
            "meeting output already exists; no existing file was changed".to_string(),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DictationError::FileDecodeError(format!(
            "meeting output could not be inspected: {}",
            error
        ))),
    }
}

fn ensure_distinct_outputs(left: &Path, right: &Path) -> Result<(), DictationError> {
    let left = output_identity(left);
    let right = output_identity(right);
    if left == right {
        return Err(DictationError::SettingsError(
            "meeting proposal and cache outputs must be different paths".to_string(),
        ));
    }
    Ok(())
}

fn output_identity(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let Some(filename) = path.file_name() else {
        return path.to_path_buf();
    };
    fs::canonicalize(parent)
        .map(|parent| parent.join(filename))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn ensure_previous_revision_unchanged(
    path: &Path,
    expected_revision: &str,
) -> Result<(), DictationError> {
    let current: MeetingReview = read_json(path, MAX_REPROCESSING_JSON_BYTES, "previous review")?;
    if current.revision != expected_revision {
        return Err(DictationError::SettingsError(
            "previous review changed during reprocessing; no proposal was published".to_string(),
        ));
    }
    Ok(())
}

fn parse_threshold(value: &str) -> Result<f32, String> {
    let threshold = value
        .parse::<f32>()
        .map_err(|_| format!("'{value}' is not a valid threshold"))?;
    if !threshold.is_finite() || !(0.0..=2.0).contains(&threshold) {
        return Err(format!(
            "threshold must be a finite number between 0.0 and 2.0, got '{value}'"
        ));
    }
    Ok(threshold)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use sagascript_core::meeting::{MeetingSegmentInput, MeetingSpeaker, MeetingTranscript};
    use sagascript_core::meeting_review::MeetingReview;
    use sha2::{Digest, Sha256};

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(flatten)]
        args: ReprocessingArgs,
    }

    fn synthetic_review(source: &str, text: &str) -> MeetingReview {
        MeetingReview::new(
            MeetingTranscript::new(
                source,
                "en",
                "base",
                1.0,
                vec![MeetingSegmentInput {
                    start: 0.0,
                    end: 1.0,
                    text: text.to_string(),
                    speaker: "speaker-a".to_string(),
                }],
                vec![MeetingSpeaker {
                    id: "speaker-a".to_string(),
                    label: "Alice".to_string(),
                }],
            )
            .expect("synthetic transcript"),
        )
        .expect("synthetic review")
    }

    fn temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "sagascript-reprocessing-cli-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&path).expect("synthetic temp directory");
        path
    }

    #[test]
    fn parser_accepts_plan_and_execute_surfaces() {
        let plan = TestCli::try_parse_from([
            "sagascript",
            "plan",
            "audio.wav",
            "--previous-review",
            "review.json",
            "--mode",
            "full",
            "--threshold",
            "1.25",
            "--output",
            "plan.json",
        ])
        .expect("plan parses");
        assert!(matches!(plan.args.action, ReprocessingAction::Plan(_)));

        let execute = TestCli::try_parse_from([
            "sagascript",
            "execute",
            "plan.json",
            "audio.wav",
            "--previous-review",
            "review.json",
            "--expected-revision",
            &"a".repeat(64),
            "--output",
            "proposal.json",
        ])
        .expect("execute parses");
        assert!(matches!(
            execute.args.action,
            ReprocessingAction::Execute(_)
        ));
    }

    #[test]
    fn synthetic_full_plan_is_provenance_only() {
        let root = temp_dir();
        let audio = root.join("audio.bin");
        fs::write(&audio, b"synthetic audio bytes").expect("audio");
        let source = format!("{:x}", Sha256::digest(b"synthetic audio bytes"));
        let review = synthetic_review(&source, "hello");
        let glossary = Glossary::default();
        let input = ReprocessingInput {
            audio: &audio,
            cache: None,
            previous: &review,
            language: Language::English,
            model: WhisperModel::Base,
            glossary: &glossary,
        };

        let plan = plan_reprocessing(&input, ReprocessingMode::Full, 0.75, None)
            .expect("synthetic full plan");
        assert_eq!(plan.mode, ReprocessingMode::Full);
        assert_eq!(plan.context.source_sha256, source);
        assert_eq!(plan.context.cache_sha256, None);
        assert!(plan.required_work.transcription);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn existing_output_is_rejected_before_input_work() {
        let root = temp_dir();
        let output = root.join("existing.json");
        fs::write(&output, b"keep me").expect("existing output");
        let args = PlanArgs {
            audio: root.join("missing-audio"),
            previous_review: root.join("missing-review"),
            mode: ReprocessingModeArg::Full,
            threshold: 0.75,
            cache: None,
            runtime: RuntimeArgs {
                language: None,
                profile: None,
                model: None,
                prompt: None,
                prompt_file: None,
            },
            output: output.clone(),
        };
        let error = run_plan(args).expect_err("existing output must fail");
        assert!(error.to_string().contains("output already exists"));
        assert_eq!(
            fs::read(&output).expect("existing output remains"),
            b"keep me"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn proposal_and_cache_outputs_cannot_alias() {
        let root = temp_dir();
        let proposal = root.join("proposal.json");
        let cache = root.join(".").join("proposal.json");
        let error = ensure_distinct_outputs(&proposal, &cache)
            .expect_err("aliased output paths must fail before inference");
        assert!(error.to_string().contains("different paths"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stale_previous_revision_is_rejected_before_publication() {
        let root = temp_dir();
        let review_path = root.join("review.json");
        let old = synthetic_review(&"a".repeat(64), "old");
        let new = synthetic_review(&"a".repeat(64), "new");
        fs::write(
            &review_path,
            serde_json::to_vec(&new).expect("new review JSON"),
        )
        .expect("write changed review");
        let error = ensure_previous_revision_unchanged(&review_path, &old.revision)
            .expect_err("changed review must fail");
        assert!(error.to_string().contains("changed during reprocessing"));
        assert_eq!(
            serde_json::from_slice::<MeetingReview>(&fs::read(&review_path).expect("review"))
                .expect("valid review")
                .revision,
            new.revision
        );
        let _ = fs::remove_dir_all(root);
    }
}
