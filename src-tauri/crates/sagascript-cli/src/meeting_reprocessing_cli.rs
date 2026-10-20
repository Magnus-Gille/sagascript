//! CLI adapter for explicit meeting reprocessing plans and proposals.
//!
//! This layer only parses arguments, resolves the same settings inputs as the
//! transcription command, and delegates work to the validated reprocessing
//! orchestration API.  It never replaces an existing review or output file.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use sagascript_core::error::DictationError;
use sagascript_core::fsutil::{PublishFs, RealFs};
use sagascript_core::meeting::reprocess::plan::{ReprocessingMode, ReprocessingPlan};
use sagascript_core::meeting::review::MeetingReview;
use sagascript_core::run_context::RunContext;
use sagascript_core::settings::{FileModel, Language, Settings, WhisperModel};
use sagascript_core::transcription::{Glossary, WhisperBackend};
use serde_json::json;

use crate::meeting_proposal::write_json_new;
use crate::meeting_reprocessing::NativeBackend;
use crate::output::{errln, Out};
use crate::run_request::RunRequest;
use crate::speaker_args::SpeakerCountArgs;
use sagascript_core::meeting::reprocess::io::{ensure_output_absent, publish_error, read_json};
use sagascript_core::meeting::reprocess::orchestrate::{
    ensure_distinct_outputs, execute_with_model_check, plan_reprocessing, CommitError,
    PublishReceipt, ReprocessingInput, WorkBackend,
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
    #[arg(long, value_parser = parse_threshold, default_value = "0.34")]
    pub threshold: f32,
    #[command(flatten)]
    pub speaker_count: SpeakerCountArgs,
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

impl<'a> From<&'a RuntimeArgs> for RunRequest<'a> {
    fn from(args: &'a RuntimeArgs) -> Self {
        Self {
            profile: args.profile.as_deref(),
            language: args.language.as_deref(),
            prompt: args.prompt.as_deref(),
            prompt_file: args.prompt_file.as_deref(),
        }
    }
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

pub fn run(args: ReprocessingArgs, out: &Out) -> Result<(), DictationError> {
    match args.action {
        ReprocessingAction::Plan(args) => run_plan(args, out),
        ReprocessingAction::Execute(args) => run_execute(args, out),
    }
}

fn run_plan(args: PlanArgs, out: &Out) -> Result<(), DictationError> {
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
    let speaker_hint = args
        .speaker_count
        .hint()
        .map_err(|message| DictationError::SettingsError(format!("--speakers: {message}")))?;
    let plan = plan_reprocessing(&input, args.mode.into(), args.threshold, None)?
        .with_speaker_hint(speaker_hint)
        .map_err(|error| DictationError::SettingsError(error.to_string()))?;
    write_json_new(&args.output, &plan, out)
}

fn run_execute(args: ExecuteArgs, out: &Out) -> Result<(), DictationError> {
    ensure_outputs_ready(&args, out)?;
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
    execute_and_publish(
        &args,
        &plan,
        &input,
        sagascript_core::diarization::model::all_models_downloaded,
        &mut NativeBackend {
            stored: &runtime.stored,
            whisper: &whisper,
        },
        &RealFs,
        out,
    )
}

/// The checks that cost nothing, before any input is read or inference starts:
/// both outputs are free, distinct and in usable folders, and no earlier run
/// was interrupted between its two publications.
fn ensure_outputs_ready(args: &ExecuteArgs, out: &Out) -> Result<(), DictationError> {
    let Some(cache_output) = args.cache_output.as_deref() else {
        ensure_output_absent(&args.output)?;
        check_output_folder(&args.output)?;
        warn_about_staging_leftovers(&args.output, out);
        return Ok(());
    };
    if exists(cache_output) && !exists(&args.output) {
        return Err(interrupted_run_error(args, cache_output));
    }
    ensure_output_absent(&args.output)?;
    ensure_output_absent(cache_output)?;
    ensure_distinct_outputs(&args.output, cache_output)?;
    check_output_folder(&args.output)?;
    check_output_folder(cache_output)?;
    warn_about_staging_leftovers(&args.output, out);
    warn_about_staging_leftovers(cache_output, out);
    Ok(())
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn check_output_folder(path: &Path) -> Result<(), DictationError> {
    sagascript_core::fsutil::check_destination_folder(path).map_err(|error| publish_error(&error))
}

/// Private staging files are hidden siblings of an output that a killed run
/// left behind. Nothing reads them, so they are only worth a warning.
fn warn_about_staging_leftovers(output: &Path, out: &Out) {
    let found = sagascript_core::fsutil::staging_leftovers(output);
    if !found.is_empty() {
        let list = found
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        errln!(
            out,
            "Warning: an interrupted run left private staging file(s) beside {}: {list}. \
             Sagascript never reads them; delete them when you no longer need them.",
            output.display()
        );
    }
}

/// How to continue from a cache that is published when its proposal is not.
/// The selective plan expects the decoder the previous review recorded, while
/// the full run wrote the cache with the current settings, so the advice
/// states that condition and the refusal that follows without it.
fn recovery_advice(args: &ExecuteArgs, cache: &Path) -> String {
    format!(
        "If your language/profile, model and prompt options and your beam size, VAD and temperature \
         settings are unchanged, recover without recomputing: plan a selective run from that cache, \
         `sagascript meeting reprocess plan '{audio}' --previous-review '{review}' --mode recluster \
         --cache '{cache}' --output NEW_PLAN`, then run `execute` on it with `--cache '{cache}'` and \
         `--output '{proposal}'`, repeating your options in both. If the plan is refused (selective \
         cache miss, incompatible decoder provenance), or you changed those settings, choose new \
         output paths to compute again. See docs/meeting-reprocessing.md, \"Interrupted publication\".",
        audio = args.audio.display(),
        review = args.previous_review.display(),
        cache = cache.display(),
        proposal = args.output.display(),
    )
}

/// The cache output exists and the proposal does not: the state an interrupted
/// run leaves, since the proposal is published last.
fn interrupted_run_error(args: &ExecuteArgs, cache: &Path) -> DictationError {
    let staged = sagascript_core::fsutil::staging_leftovers(&args.output);
    let survivor = if staged.is_empty() {
        String::new()
    } else {
        let list = staged
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            " A staged proposal survived ({list}); it was complete before the cache was published, \
             so after `sagascript meeting proposal inspect` accepts it you can rename it to the proposal path."
        )
    };
    DictationError::FileDecodeError(format!(
        "the cache output {} already exists but the proposal output {} does not: an earlier run was \
         probably interrupted before it published its proposal. Nothing was changed.{survivor} {}",
        cache.display(),
        args.output.display(),
        recovery_advice(args, cache)
    ))
}

/// Run the plan on `backend`, then publish the outputs: the new cache first,
/// the proposal last. Everything that can fail (inference, the checks after it,
/// serialization, writing both files) comes before the first publication, so a
/// failure leaves no output and the same command can run again. Only a failure
/// of the final publication leaves a complete cache without a proposal; see
/// `meeting_reprocessing` and docs/meeting-reprocessing.md.
fn execute_and_publish(
    args: &ExecuteArgs,
    plan: &ReprocessingPlan,
    input: &ReprocessingInput<'_>,
    models_available: impl FnOnce() -> bool,
    backend: &mut impl WorkBackend,
    ops: &dyn PublishFs,
    out: &Out,
) -> Result<(), DictationError> {
    // What earlier runs left; `ensure_outputs_ready` has warned about it.
    let before = staging_files_beside(args);
    let staged = execute_with_model_check(
        plan,
        &args.expected_revision,
        input,
        args.cache_output.as_deref(),
        None,
        crate::GIT_HASH,
        models_available,
        backend,
    )?;
    let pending = staged.stage_proposal(&args.output)?;
    ensure_previous_revision_unchanged(&args.previous_review, &input.previous.revision)?;
    let receipt = pending
        .commit_with(ops)
        .map_err(|error| publication_error(error, args))?;
    // Staging files the receipt knows about, plus any the pipeline itself left
    // beside the outputs during the run (they are not in the receipt).
    let mut leftovers = receipt
        .leftovers
        .iter()
        .map(|leftover| leftover.path.clone())
        .collect::<BTreeSet<_>>();
    leftovers.extend(staging_files_beside(args).difference(&before).cloned());
    report_receipt(receipt, &leftovers, out)
}

/// The private staging files that exist beside either output.
fn staging_files_beside(args: &ExecuteArgs) -> BTreeSet<PathBuf> {
    let mut found = sagascript_core::fsutil::staging_leftovers(&args.output);
    if let Some(cache) = args.cache_output.as_deref() {
        found.extend(sagascript_core::fsutil::staging_leftovers(cache));
    }
    found.into_iter().collect()
}

/// The error for a commit that did not publish everything.
fn publication_error(error: CommitError, args: &ExecuteArgs) -> DictationError {
    match error {
        CommitError::NothingPublished(cause) => cause,
        CommitError::CacheOnly {
            cache,
            proposal,
            cause,
        } => DictationError::FileDecodeError(format!(
            "{cause}. The new cache WAS published at {} and is complete, but no proposal exists at {}; \
             no existing file was changed. {}",
            cache.display(),
            proposal.display(),
            recovery_advice(args, &cache)
        )),
    }
}

fn report_receipt(
    receipt: PublishReceipt,
    leftovers: &BTreeSet<PathBuf>,
    out: &Out,
) -> Result<(), DictationError> {
    if let Some(cache) = &receipt.cache {
        errln!(out, "Saved reusable diarization cache: {}", cache.display());
    }
    if !leftovers.is_empty() {
        let list = leftovers
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        errln!(
            out,
            "Warning: the outputs were published, but private temporary files beside them could not \
             be removed. They hold the same content as the outputs; delete them: {list}"
        );
    }
    let diagnostics = json!({
        "required_work": receipt.required_work,
        "timings": receipt.timings,
    });
    let serialized = serde_json::to_string(&diagnostics).map_err(|_| {
        DictationError::TranscriptionFailed(
            "reprocessing diagnostics could not be serialized".to_string(),
        )
    })?;
    errln!(out, "{serialized}");
    Ok(())
}

fn resolve_runtime(args: &RuntimeArgs) -> Result<RuntimeInputs, DictationError> {
    resolve_runtime_from(args, sagascript_core::machine::load())
}

/// [`resolve_runtime`] against given settings, so tests resolve fixtures.
fn resolve_runtime_from(
    args: &RuntimeArgs,
    stored: Settings,
) -> Result<RuntimeInputs, DictationError> {
    let request = RunRequest::from(args);
    let selection = request.select(&stored)?;
    // The model is this command's own choice: reprocessing decodes with
    // Whisper only. It is resolved before the prompt file is read, so an
    // unusable model is reported before an unreadable file.
    let model = match crate::transcribe::resolve_file_model_for_run(
        args.model.as_deref(),
        selection.language,
        &stored,
        true,
    )? {
        FileModel::Whisper(model) => model,
        FileModel::PianissimoOriginal => {
            return Err(DictationError::SettingsError(
                "Meeting reprocessing requires a Whisper model; select one with --model".into(),
            ))
        }
    };
    let RunContext {
        language, glossary, ..
    } = request.finish(&stored, selection)?;
    Ok(RuntimeInputs {
        stored,
        language,
        model,
        glossary,
    })
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
    use sagascript_core::meeting::control::MeetingControl;
    use sagascript_core::meeting::reprocess::orchestrate::{CommitError, WorkBackend};
    use sagascript_core::meeting::review::MeetingReview;
    use sagascript_core::test_support::meeting_transcript;
    use sagascript_testsupport::{sha256_hex, TempDir};

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(flatten)]
        args: ReprocessingArgs,
    }

    #[test]
    fn plan_accepts_speaker_count_flags() {
        let base = [
            "x",
            "a.wav",
            "--previous-review",
            "r.json",
            "--output",
            "o.json",
            "--language",
            "en",
        ];
        let parse = |extra: &[&str]| {
            <PlanArgs as clap::Args>::augment_args(clap::Command::new("x"))
                .try_get_matches_from(base.iter().chain(extra.iter()).copied())
        };
        assert!(parse(&["--speakers", "2"]).is_ok());
        assert!(parse(&["--min-speakers", "2", "--max-speakers", "3"]).is_ok());
        assert!(parse(&["--speakers", "0"]).is_err());
        assert!(parse(&["--speakers", "2", "--min-speakers", "1"]).is_err());
    }

    #[test]
    fn default_threshold_matches_diarization_default() {
        // Pins the clap default literal against the core default.
        let default_literal = <PlanArgs as clap::Args>::augment_args(clap::Command::new("x"))
            .get_matches_from([
                "x",
                "a.wav",
                "--previous-review",
                "r.json",
                "--output",
                "o.json",
                "--language",
                "en",
            ])
            .get_one::<f32>("threshold")
            .copied();
        assert_eq!(default_literal, Some(0.34));
        #[cfg(feature = "diarization")]
        assert_eq!(
            default_literal,
            Some(sagascript_core::diarization::DEFAULT_THRESHOLD)
        );
    }

    fn synthetic_review(source: &str, text: &str) -> MeetingReview {
        MeetingReview::new(
            meeting_transcript()
                .source_sha256(source)
                .model("base")
                .duration(1.0)
                .segment(0.0, 1.0, text, "speaker-a")
                .speaker("speaker-a", "Alice")
                .build(),
        )
        .expect("synthetic review")
    }

    #[derive(Debug, Parser)]
    struct RuntimeCli {
        #[command(flatten)]
        args: RuntimeArgs,
    }

    fn runtime_args(flags: &[&str]) -> RuntimeArgs {
        RuntimeCli::try_parse_from(["x"].into_iter().chain(flags.iter().copied()))
            .expect("runtime flags parse")
            .args
    }

    fn resolve(flags: &[&str]) -> Result<RuntimeInputs, DictationError> {
        resolve_runtime_from(&runtime_args(flags), Settings::default())
    }

    #[test]
    fn reprocessing_rejects_models_it_cannot_decode_with() {
        // Reprocessing is Whisper-only; the shared resolver does not choose
        // the model, so each command's own limits stay in force (#346).
        for (flags, code, message) in [
            (
                &["--model", "pianissimo-sv", "--language", "sv"][..],
                "settings_error",
                "Settings error: Meeting reprocessing requires a Whisper model; select one with --model",
            ),
            (
                &["--model", "pianissimo-sv"][..],
                "pianissimo_unavailable",
                "Settings error: Pianissimo Original is only available for Swedish file transcription",
            ),
            (
                &["--model", "bogus"][..],
                "invalid_setting_value",
                "Settings error: Unknown file model preference 'bogus'",
            ),
            (
                &["--model", "auto"][..],
                "settings_error",
                "Settings error: --model needs a concrete model ID; omit it to use the file default",
            ),
        ] {
            let error = resolve(flags).expect_err(&format!("{flags:?} must be rejected"));
            assert!(
                matches!(
                    error,
                    DictationError::SettingsError(_) | DictationError::Settings(_)
                ),
                "{flags:?}: {error:?}"
            );
            assert_eq!(error.code(), code, "{flags:?}");
            assert_eq!(error.to_string(), message, "{flags:?}");
        }
    }

    #[test]
    fn reprocessing_accepts_whisper_models_and_takes_the_language_from_the_request() {
        let inputs = resolve(&["--model", "base.en"]).unwrap();
        assert_eq!(inputs.model, WhisperModel::BaseEn);
        assert_eq!(inputs.language, Language::English);

        let inputs = resolve(&["--model", "kb-whisper-base", "--language", "sv"]).unwrap();
        assert_eq!(inputs.model, WhisperModel::KbWhisperBase);
        assert_eq!(inputs.language, Language::Swedish);
        // No `--model` is covered by the real-binary test in
        // `tests/run_context_cli.rs`: an automatic choice looks at the models
        // downloaded on this machine, which a unit test must not touch.
    }

    #[test]
    fn the_model_is_decided_before_the_prompt_file_is_read() {
        // Both are wrong at once: the model is reported, as before the
        // resolver was shared. `transcribe` has the opposite order and keeps it.
        let missing = "no-such-folder/hint.txt";
        for (flags, message) in [
            (
                &["--model", "bogus", "--prompt-file", missing][..],
                "Settings error: Unknown file model preference 'bogus'",
            ),
            (
                &[
                    "--model",
                    "pianissimo-sv",
                    "--language",
                    "sv",
                    "--prompt-file",
                    missing,
                ][..],
                "Settings error: Meeting reprocessing requires a Whisper model; select one with --model",
            ),
        ] {
            let error = resolve(flags).unwrap_err();
            assert_eq!(error.to_string(), message, "{flags:?}");
        }
        let error = resolve(&["--model", "base.en", "--prompt-file", missing]).unwrap_err();
        assert_eq!(error.code(), "file_decode_error");
        assert!(error.to_string().starts_with(
            "File decode error: Failed to read prompt file 'no-such-folder/hint.txt': "
        ));
    }

    #[test]
    fn an_unknown_profile_is_reported_before_the_model_and_the_prompt_file() {
        let error = resolve(&[
            "--profile",
            "nope",
            "--model",
            "bogus",
            "--prompt-file",
            "no-such-folder/hint.txt",
        ])
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Settings error: Unknown dictation profile 'nope'"
        );
    }

    #[test]
    fn the_runtime_keeps_the_settings_it_resolved_against() {
        let settings = Settings {
            initial_prompt: "marker".to_string(),
            ..Settings::default()
        };
        let inputs =
            resolve_runtime_from(&runtime_args(&["--model", "base.en"]), settings).unwrap();
        assert_eq!(inputs.stored.initial_prompt, "marker");
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
        let root = TempDir::new();
        let audio = root.join("audio.bin");
        fs::write(&audio, b"synthetic audio bytes").expect("audio");
        let source = sha256_hex(b"synthetic audio bytes");
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
    }

    #[test]
    fn existing_output_is_rejected_before_input_work() {
        let root = TempDir::new();
        let output = root.join("existing.json");
        fs::write(&output, b"keep me").expect("existing output");
        let args = PlanArgs {
            audio: root.join("missing-audio"),
            previous_review: root.join("missing-review"),
            mode: ReprocessingModeArg::Full,
            threshold: 0.75,
            speaker_count: SpeakerCountArgs::default(),
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
        let error = run_plan(args, &Out::capture().0).expect_err("existing output must fail");
        assert!(error.to_string().contains("output already exists"));
        assert_eq!(
            fs::read(&output).expect("existing output remains"),
            b"keep me"
        );
    }

    #[test]
    fn proposal_and_cache_outputs_cannot_alias() {
        let root = TempDir::new();
        let proposal = root.join("proposal.json");
        let cache = root.join(".").join("proposal.json");
        let error = ensure_distinct_outputs(&proposal, &cache)
            .expect_err("aliased output paths must fail before inference");
        assert!(error.to_string().contains("different paths"));
    }

    // The two outputs of `execute` (#342): what each way a run can end leaves on
    // disk. The fake `full` saves the new cache where it is told to and then
    // runs a hook, as the real pipeline does mid-inference.

    struct FakeFull {
        during_inference: Option<Box<dyn FnMut()>>,
    }

    impl WorkBackend for FakeFull {
        fn decode(
            &mut self,
            _audio: &Path,
            _control: Option<&MeetingControl<'_>>,
        ) -> Result<Vec<f32>, DictationError> {
            Err(DictationError::TranscriptionFailed("not used".into()))
        }

        fn analyze(
            &mut self,
            _audio: &[f32],
            _config: &sagascript_core::diarization::DiarizeConfig,
            _control: Option<&MeetingControl<'_>>,
        ) -> Result<sagascript_core::diarization::DiarizationAnalysis, DictationError> {
            Err(DictationError::TranscriptionFailed("not used".into()))
        }

        fn full(
            &mut self,
            input: &ReprocessingInput<'_>,
            _threshold: f32,
            _speaker_hint: Option<sagascript_core::diarization::speaker_hint::SpeakerCountHint>,
            cache_output: Option<&Path>,
            _control: Option<&MeetingControl<'_>>,
        ) -> Result<sagascript_core::meeting::MeetingTranscript, DictationError> {
            if let Some(path) = cache_output {
                let identity =
                    sagascript_core::diarization::cache::CacheIdentity::for_source_sha256(
                        &input.previous.original.source_sha256,
                        "en",
                        "base",
                        None,
                    )?;
                let analysis = serde_json::from_str(r#"{"raw_segments":[],"embeddings":[]}"#)
                    .expect("empty synthetic analysis");
                let cache = sagascript_core::diarization::cache::DiarizationCache::new(
                    identity,
                    analysis,
                    vec![(0.0, 1.0, "hello".to_string())],
                    sagascript_core::transcription::diagnostics::CoverageProfile::from_audio(
                        &[0.1; 16_000],
                    ),
                    None,
                    None,
                );
                sagascript_core::diarization::cache::save_new(path, &cache)?;
            }
            if let Some(hook) = self.during_inference.as_mut() {
                hook();
            }
            Ok(input.previous.original.clone())
        }
    }

    struct ExecuteFixture {
        root: TempDir,
        review: MeetingReview,
        plan: ReprocessingPlan,
        args: ExecuteArgs,
        glossary: Glossary,
    }

    impl ExecuteFixture {
        fn new() -> Self {
            let root = TempDir::new();
            let audio = root.join("audio.bin");
            fs::write(&audio, b"synthetic audio bytes").expect("audio");
            let review = synthetic_review(&sha256_hex(b"synthetic audio bytes"), "hello");
            let review_path = root.join("review.json");
            fs::write(&review_path, serde_json::to_vec(&review).expect("review")).expect("write");
            let glossary = Glossary::default();
            let input = ReprocessingInput {
                audio: &audio,
                cache: None,
                previous: &review,
                language: Language::English,
                model: WhisperModel::Base,
                glossary: &glossary,
            };
            let plan = plan_reprocessing(&input, ReprocessingMode::Full, 0.75, None).expect("plan");
            let args = ExecuteArgs {
                plan: root.join("plan.json"),
                audio,
                previous_review: review_path,
                expected_revision: plan.revision.clone(),
                cache: None,
                cache_output: Some(root.join("cache.json")),
                runtime: RuntimeArgs {
                    language: None,
                    profile: None,
                    model: None,
                    prompt: None,
                    prompt_file: None,
                },
                output: root.join("proposal.json"),
            };
            Self {
                root,
                review,
                plan,
                args,
                glossary,
            }
        }

        fn names(&self) -> Vec<String> {
            let mut names = fs::read_dir(&self.root)
                .expect("list")
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

        /// `run_execute` without the settings lookup: its output checks, then the work
        /// and the publication.
        fn run(&self, backend: &mut FakeFull) -> (Result<(), DictationError>, String) {
            let (out, captured) = Out::capture();
            let result =
                ensure_outputs_ready(&self.args, &out).and_then(|()| self.execute(backend, &out));
            (result, captured.stderr())
        }

        /// The work and the publication, as `run` does after the output checks.
        fn execute(&self, backend: &mut FakeFull, out: &Out) -> Result<(), DictationError> {
            self.execute_on(backend, &RealFs, out)
        }

        /// [`execute`](Self::execute) on the given filesystem operations.
        fn execute_on(
            &self,
            backend: &mut FakeFull,
            ops: &dyn PublishFs,
            out: &Out,
        ) -> Result<(), DictationError> {
            let input = ReprocessingInput {
                audio: &self.args.audio,
                cache: None,
                previous: &self.review,
                language: Language::English,
                model: WhisperModel::Base,
                glossary: &self.glossary,
            };
            execute_and_publish(&self.args, &self.plan, &input, || true, backend, ops, out)
        }
    }

    fn quiet() -> FakeFull {
        FakeFull {
            during_inference: None,
        }
    }

    #[test]
    fn success_publishes_the_cache_and_the_proposal_and_nothing_else() {
        let fixture = ExecuteFixture::new();
        let (result, stderr) = fixture.run(&mut quiet());
        result.expect("execute");
        assert_eq!(
            fixture.names(),
            ["audio.bin", "cache.json", "proposal.json", "review.json"]
        );
        let diagnostics: serde_json::Value =
            serde_json::from_str(stderr.lines().last().expect("diagnostics line")).expect("json");
        assert!(diagnostics.get("required_work").is_some());
        assert!(diagnostics.get("timings").is_some());
    }

    #[test]
    fn leftovers_after_a_successful_run_are_named_on_stderr_and_the_run_succeeds() {
        use sagascript_core::test_support::FaultyFs;
        let fixture = ExecuteFixture::new();
        let (out, captured) = Out::capture();

        fixture
            .execute_on(&mut quiet(), &FaultyFs::new().failing_remove(), &out)
            .expect("both outputs are published, so the run succeeds");

        let cache = fixture.args.cache_output.clone().expect("cache output");
        let mut leftovers = sagascript_core::fsutil::staging_leftovers(&cache);
        leftovers.extend(sagascript_core::fsutil::staging_leftovers(
            &fixture.args.output,
        ));
        assert_eq!(leftovers.len(), 2, "{leftovers:?}");
        let stderr = captured.stderr();
        for leftover in &leftovers {
            assert!(stderr.contains(&leftover.display().to_string()), "{stderr}");
        }
        assert!(!stderr.contains("next `execute`"), "{stderr}");
        assert_eq!(
            fixture
                .names()
                .iter()
                .filter(|name| name.ends_with(".tmp"))
                .count(),
            2
        );
    }

    #[test]
    fn a_staging_file_left_by_the_pipeline_itself_is_reported_too() {
        let fixture = ExecuteFixture::new();
        let cache = fixture.args.cache_output.clone().expect("cache output");
        // What a pipeline's own staging file, killed or unremovable, looks like.
        let mut backend = FakeFull {
            during_inference: Some(Box::new(move || {
                let leftover =
                    sagascript_core::fsutil::StagedFile::create(&cache, b"x").expect("leftover");
                std::mem::forget(leftover);
            })),
        };
        let (out, captured) = Out::capture();

        fixture
            .execute_on(&mut backend, &RealFs, &out)
            .expect("the run succeeds");

        let cache = fixture.args.cache_output.clone().expect("cache output");
        let leftovers = sagascript_core::fsutil::staging_leftovers(&cache);
        assert_eq!(leftovers.len(), 1, "{leftovers:?}");
        let stderr = captured.stderr();
        assert!(
            stderr.contains(&leftovers[0].display().to_string()),
            "{stderr}"
        );
    }

    #[test]
    fn review_changed_during_inference_publishes_nothing() {
        let fixture = ExecuteFixture::new();
        let review_path = fixture.args.previous_review.clone();
        let edited = synthetic_review(&"a".repeat(64), "edited meanwhile");
        let edited_bytes = serde_json::to_vec(&edited).expect("edited review");
        let expected = edited_bytes.clone();
        let mut backend = FakeFull {
            during_inference: Some(Box::new(move || {
                fs::write(&review_path, &edited_bytes).expect("concurrent edit");
            })),
        };

        let (result, _) = fixture.run(&mut backend);

        let error = result.expect_err("the review changed");
        assert!(
            error.to_string().contains("changed during reprocessing"),
            "{error}"
        );
        assert_eq!(fixture.names(), ["audio.bin", "review.json"]);
        assert_eq!(
            fs::read(&fixture.args.previous_review).expect("review"),
            expected
        );
    }

    #[test]
    fn rerun_after_the_review_changed_reuses_the_same_output_names() {
        let fixture = ExecuteFixture::new();
        let review_path = fixture.args.previous_review.clone();
        let original = fs::read(&review_path).expect("original review");
        let restore = original.clone();
        let mut backend = FakeFull {
            during_inference: Some(Box::new({
                let review_path = review_path.clone();
                move || {
                    let edited = synthetic_review(&"a".repeat(64), "edited meanwhile");
                    fs::write(&review_path, serde_json::to_vec(&edited).expect("edited"))
                        .expect("concurrent edit");
                }
            })),
        };
        fixture.run(&mut backend).0.expect_err("the review changed");
        fs::write(&review_path, restore).expect("review is back as planned");

        let (result, _) = fixture.run(&mut quiet());

        result.expect("the identical command succeeds the second time");
        assert_eq!(
            fixture.names(),
            ["audio.bin", "cache.json", "proposal.json", "review.json"]
        );
    }

    #[test]
    fn unwritable_proposal_destination_publishes_nothing() {
        let mut fixture = ExecuteFixture::new();
        fixture.args.output = fixture.root.join("missing-folder").join("proposal.json");

        // Straight to the work: the early output checks are not what is tested.
        let (out, _) = Out::capture();
        let error = fixture
            .execute(&mut quiet(), &out)
            .expect_err("the proposal cannot be written");

        assert!(error.to_string().contains("output folder"), "{error}");
        assert_eq!(fixture.names(), ["audio.bin", "review.json"]);
    }

    #[test]
    fn proposal_output_created_during_inference_is_never_overwritten() {
        let fixture = ExecuteFixture::new();
        let proposal_path = fixture.args.output.clone();
        let mut backend = FakeFull {
            during_inference: Some(Box::new({
                let proposal_path = proposal_path.clone();
                move || fs::write(&proposal_path, b"someone else's file").expect("their file")
            })),
        };

        let (result, _) = fixture.run(&mut backend);

        let error = result.expect_err("the destination appeared");
        assert!(error.to_string().contains("already exists"), "{error}");
        assert_eq!(
            fs::read(&proposal_path).expect("kept"),
            b"someone else's file"
        );
        assert_eq!(
            fixture.names(),
            ["audio.bin", "proposal.json", "review.json"]
        );
    }

    #[test]
    fn an_earlier_run_interrupted_between_its_publications_is_detected_before_any_work() {
        let fixture = ExecuteFixture::new();
        fs::write(fixture.args.cache_output.as_ref().expect("cache"), b"cache").expect("cache");
        let (out, _) = Out::capture();

        let error = ensure_outputs_ready(&fixture.args, &out).expect_err("interrupted run");

        let message = error.to_string();
        assert!(message.contains("probably interrupted"), "{message}");
        assert!(message.contains("--mode recluster"), "{message}");
        assert!(message.contains("cache.json"), "{message}");
        assert!(!message.contains("survived"), "{message}");
        assert_eq!(fixture.names(), ["audio.bin", "cache.json", "review.json"]);

        // A staged proposal that outlived the run is pointed out.
        let staged = sagascript_core::fsutil::StagedFile::create(&fixture.args.output, b"{}")
            .expect("staged proposal");
        std::mem::forget(staged);
        let message = ensure_outputs_ready(&fixture.args, &out)
            .expect_err("interrupted run")
            .to_string();
        assert!(message.contains("A staged proposal survived"), "{message}");
    }

    #[test]
    fn both_outputs_existing_is_not_an_interrupted_run() {
        let fixture = ExecuteFixture::new();
        fs::write(fixture.args.cache_output.as_ref().expect("cache"), b"cache").expect("cache");
        fs::write(&fixture.args.output, b"proposal").expect("proposal");
        let (out, _) = Out::capture();

        let message = ensure_outputs_ready(&fixture.args, &out)
            .expect_err("exists")
            .to_string();

        assert!(message.contains("output already exists"), "{message}");
        assert!(!message.contains("interrupted"), "{message}");
    }

    #[test]
    fn unusable_output_folders_are_rejected_before_any_work() {
        let mut fixture = ExecuteFixture::new();
        let (out, _) = Out::capture();
        fixture.args.output = fixture.root.join("missing").join("proposal.json");
        let message = ensure_outputs_ready(&fixture.args, &out)
            .expect_err("missing proposal folder")
            .to_string();
        assert!(message.contains("output folder"), "{message}");

        fixture.args.output = fixture.root.join("proposal.json");
        fixture.args.cache_output = Some(fixture.root.join("missing").join("cache.json"));
        let message = ensure_outputs_ready(&fixture.args, &out)
            .expect_err("missing cache folder")
            .to_string();
        assert!(message.contains("output folder"), "{message}");
        assert_eq!(fixture.names(), ["audio.bin", "review.json"]);
    }

    #[test]
    fn staging_files_left_by_a_killed_run_are_warned_about_not_fatal() {
        let fixture = ExecuteFixture::new();
        let staged = sagascript_core::fsutil::StagedFile::create(&fixture.args.output, b"{}")
            .expect("staged proposal");
        let survivor = staged.staged_path().to_path_buf();
        std::mem::forget(staged);
        let (out, captured) = Out::capture();

        ensure_outputs_ready(&fixture.args, &out).expect("a stray staging file blocks nothing");

        let stderr = captured.stderr();
        assert!(
            stderr.contains("interrupted run left private staging"),
            "{stderr}"
        );
        assert!(stderr.contains(&survivor.display().to_string()), "{stderr}");
    }

    #[test]
    fn a_failure_after_the_cache_was_published_names_the_cache_and_the_way_back() {
        let fixture = ExecuteFixture::new();
        let cache = fixture.root.join("cache.json");
        let error = publication_error(
            CommitError::CacheOnly {
                cache: cache.clone(),
                proposal: fixture.args.output.clone(),
                cause: DictationError::FileDecodeError(
                    "meeting output could not be published: boom".into(),
                ),
            },
            &fixture.args,
        );

        let message = error.to_string();
        assert!(message.contains("boom"), "{message}");
        assert!(message.contains("WAS published"), "{message}");
        assert!(message.contains(&cache.display().to_string()), "{message}");
        assert!(
            message.contains("no existing file was changed"),
            "{message}"
        );
        assert!(message.contains("--mode recluster"), "{message}");
        assert!(
            message.contains("beam size, VAD and temperature"),
            "{message}"
        );
        assert!(
            message.contains("incompatible decoder provenance"),
            "{message}"
        );
        assert!(!message.contains("rediarize"), "{message}");
        assert!(
            message.contains(&format!("'{}'", fixture.args.audio.display())),
            "{message}"
        );
    }

    #[test]
    fn stale_previous_revision_is_rejected_before_publication() {
        let root = TempDir::new();
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
    }
}
