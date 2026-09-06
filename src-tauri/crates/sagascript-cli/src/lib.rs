pub(crate) mod benchmark_config;
mod benchmark_quality;
pub mod config;
pub mod benchmark_dictation;
pub mod glossary;
pub mod latency;
pub mod models;
pub mod open;
pub mod presenter;
// Live recording is optional (`record` feature, on by default) so a pure
// batch-transcribe build (`--no-default-features`) carries no audio-capture
// stack — on Linux, no cpal/ALSA.
#[cfg(feature = "diarization")]
mod diarization_cache;
#[cfg(feature = "record")]
pub mod record;
pub mod transcribe;

use std::io::{self, Write};
use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand};

/// Default CLI logging keeps application warnings/errors but suppresses native
/// Whisper/GGML messages below error. Those libraries classify routine Metal
/// capability probes as warnings, so a plain `warn` filter still produces
/// thousands of non-actionable lines on long machine-readable runs.
pub const NATIVE_LOG_SUPPRESSION: &str =
    "whisper_rs::whisper_logging_hook=error,whisper_rs::ggml_logging_hook=error";
pub const DEFAULT_CLI_LOG_FILTER: &str =
    "warn,whisper_rs::whisper_logging_hook=error,whisper_rs::ggml_logging_hook=error";

#[cfg(target_os = "windows")]
const WINDOWS_INFERENCE_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Preserve an application's requested log level without accidentally opting
/// into noisy native Whisper/GGML diagnostics. Native logging is enabled only
/// when the filter names `whisper_rs` explicitly.
pub fn effective_log_filter(configured: Option<&str>, default_level: &str) -> String {
    let configured = configured.map(str::trim).filter(|value| !value.is_empty());
    match configured {
        Some(filter) if filter.contains("whisper_rs") => filter.to_owned(),
        Some(filter) => format!("{filter},{NATIVE_LOG_SUPPRESSION}"),
        None if default_level == "warn" => DEFAULT_CLI_LOG_FILTER.to_owned(),
        None => format!("{default_level},{NATIVE_LOG_SUPPRESSION}"),
    }
}
use clap_complete::{Generator, Shell};

pub(crate) fn set_transcription_progress(progress: &indicatif::ProgressBar, percentage: i32) {
    progress.set_position(percentage.clamp(0, 100) as u64);
}

/// `--version` deliberately includes the source revision and build date. A
/// semantic version alone cannot distinguish a stale app-bundle executable
/// from a rebuilt release candidate with the same pre-release version.
pub const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (git ",
    env!("SAGASCRIPT_CLI_GIT_HASH"),
    ", built ",
    env!("SAGASCRIPT_CLI_BUILD_DATE"),
    ")"
);

// Root help text is feature-aware: a batch-only build (`--no-default-features`)
// has no `record` subcommand, so the workflow/examples must not advertise it.
#[cfg(feature = "record")]
const ROOT_LONG_ABOUT: &str = "\
Sagascript is a privacy-first dictation app that transcribes speech to text \
using local Whisper models. It runs as a macOS menu bar app (GUI) or as a \
standalone CLI tool.

When invoked without a subcommand, the desktop build launches the GUI; \
the headless CLI build prints this help. Use any subcommand below to \
operate in CLI mode.

Workflow:
  1. Download a model:   sagascript download-model base.en
  2. Transcribe a file:  sagascript transcribe recording.wav
  3. Or record live:      sagascript record

Supported languages: English (en), Swedish (sv), Norwegian (no), Finnish (fi), Auto-detect (auto).
Models are downloaded from pinned publisher revisions and stored locally.

NOTE: Auto-detect uses a generic multilingual model which is less accurate \
than the dedicated language models (KBLab for Swedish, NbAiLab for Norwegian). \
Finnish uses the generic multilingual Base model by default. \
For best results, set a specific language.";

#[cfg(not(feature = "record"))]
const ROOT_LONG_ABOUT: &str = "\
Sagascript is a privacy-first dictation app that transcribes speech to text \
using local Whisper models. This batch-transcription build operates on \
audio/video files (live recording is not included).

When invoked without a subcommand, the desktop build launches the GUI; \
the headless CLI build prints this help. Use any subcommand below to \
operate in CLI mode.

Workflow:
  1. Download a model:   sagascript download-model base.en
  2. Transcribe a file:  sagascript transcribe recording.wav

Supported languages: English (en), Swedish (sv), Norwegian (no), Finnish (fi), Auto-detect (auto).
Models are downloaded from pinned publisher revisions and stored locally.

NOTE: Auto-detect uses a generic multilingual model which is less accurate \
than the dedicated language models (KBLab for Swedish, NbAiLab for Norwegian). \
Finnish uses the generic multilingual Base model by default. \
For best results, set a specific language.";

#[cfg(feature = "record")]
const ROOT_AFTER_LONG_HELP: &str = "\
EXAMPLES:
  # Transcribe an audio file with auto-detected language
  sagascript transcribe meeting.mp3 --language auto

  # Record from microphone for 30 seconds, copy result to clipboard
  sagascript record --duration 30 --clipboard

  # List all available models for Swedish
  sagascript list-models --language sv

  # Download and use a specific model
  sagascript download-model kb-whisper-base
  sagascript transcribe tal.wav --model kb-whisper-base

  # View and change settings
  sagascript config list
  sagascript config set language sv
  sagascript config set hotkey 'Option+Space'

  # Generate shell completions
  sagascript completions zsh > ~/.zfunc/_sagascript

ENVIRONMENT:
  RUST_LOG    Set log level (default: warn for CLI). Example: RUST_LOG=info";

#[cfg(not(feature = "record"))]
const ROOT_AFTER_LONG_HELP: &str = "\
EXAMPLES:
  # Transcribe an audio file with auto-detected language
  sagascript transcribe meeting.mp3 --language auto

  # List all available models for Swedish
  sagascript list-models --language sv

  # Download and use a specific model
  sagascript download-model kb-whisper-base
  sagascript transcribe tal.wav --model kb-whisper-base

  # View and change settings
  sagascript config list
  sagascript config set language sv
  sagascript config set hotkey 'Option+Space'

  # Generate shell completions
  sagascript completions zsh > ~/.zfunc/_sagascript

ENVIRONMENT:
  RUST_LOG    Set log level (default: warn for CLI). Example: RUST_LOG=info";

#[derive(Parser)]
#[command(
    name = "sagascript",
    version,
    long_version = LONG_VERSION,
    about = "Low-latency dictation app",
    long_about = ROOT_LONG_ABOUT,
    after_long_help = ROOT_AFTER_LONG_HELP
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Benchmark cold and warm in-process live dictation inference
    #[command(
        long_about = "\
Benchmark the live dictation inference path on one supplied audio/video fixture.\
 The fixture is decoded once, then transcribed once as a cold run and repeatedly\
 through one in-process backend for warm timing samples. The command uses the\
 recommended model for the selected language unless --model is supplied, and\
 never downloads a model or changes saved settings. Decoder overrides are\
 local to this invocation. Timings end at the inference call, not visible text.\n\n\
 Normal JSON output contains timings and counts, never transcript text.\
 --quality-output explicitly writes plaintext transcripts to a NEW local file\
 in an existing directory (0600 on Unix; parent ACL on Windows). Keep it out of\
 shared or synced folders. Inputs are limited to 128 MiB and 120 decoded seconds\
 for this export. --allow-empty captures silence cases without judging quality.\
 A report's cli_checks_passed is not an accuracy or adoption verdict.",
        after_long_help = "\
EXAMPLES:\n  sagascript benchmark-dictation test-audio/english-jfk.wav --language en\n  sagascript benchmark-dictation sample.wav --language sv --iterations 10 --max-warm-ms 250\n  sagascript benchmark-dictation sample.wav --language en --expect-word hello"
    )]
    BenchmarkDictation(benchmark_dictation::BenchmarkDictationArgs),

    /// Transcribe audio/video files or directories
    #[command(
        long_about = "\
Transcribe audio/video files or directories using one shared local Whisper model.

Each file is decoded independently to 16 kHz mono PCM. Directories include \
WAV, MP3, M4A, AAC, MP4, MOV, QTA, OGG, WebM, and FLAC files in deterministic \
path order; use --recursive to include subdirectories.

With more than one input, --json emits an array and --jsonl emits one compact \
{source,status,result|error} object per line. Item failures do not hide later \
results, but the command exits non-zero after the batch; --fail-fast stops early.

By default, uses the language and model from your persisted settings \
(see 'sagascript config list'). Override with --language and --model.

NOTE: --language auto uses a generic multilingual model which is less \
accurate than the dedicated language models. Finnish uses the generic \
multilingual Base model by default.",
        after_long_help = "\
EXAMPLES:
  # Basic transcription (uses configured language/model)
  sagascript transcribe meeting.wav

  # Transcribe in Swedish with a specific model
  sagascript transcribe tal.m4a --language sv --model kb-whisper-base

  # Transcribe in Finnish with the generic multilingual Base model
  sagascript transcribe tal.m4a --language fi --model base

  # Output as JSON (includes metadata)
  sagascript transcribe podcast.mp3 --json

  # Transcribe and copy to clipboard
  sagascript transcribe note.wav --clipboard

  # Pipe-friendly: JSON to jq
  sagascript transcribe call.wav --json | jq -r .text

  # Batch a directory with streaming machine-readable output
  sagascript transcribe recordings/ --recursive --jsonl"
    )]
    Transcribe(transcribe::TranscribeArgs),

    /// Summarize copied live-dictation latency JSONL without launching the app
    #[command(
        long_about = "Summarize explicitly copied dictation_phase_timings JSONL. Valid input emits JSON and exits zero. An explicit budget failure emits the JSON report and exits nonzero; invalid input or arguments exit nonzero without JSON. This command never reads the default log directory, starts the app, captures audio, loads a model, changes settings, or contacts a remote service.",
        after_long_help = "EXAMPLES:\n  sagascript latency-report --input /tmp/sagascript.log\n  sagascript latency-report --input /tmp/sagascript.log --budget-length short --max-warm-p95-ms 800 --min-samples 20"
    )]
    LatencyReport(latency::LatencyReportArgs),

    /// Record from microphone and transcribe
    #[cfg(feature = "record")]
    #[command(
        long_about = "\
Record audio from the default microphone and transcribe it.

Recording continues until you press Ctrl+C, or until --duration seconds \
have elapsed. The captured audio is then transcribed using the selected model.

Use --output to save the raw audio as a WAV file without transcribing \
(useful for capturing audio to process later with 'sagascript transcribe').

NOTE: --language auto uses a generic multilingual model which is less \
accurate than the dedicated language models. Finnish uses the generic \
multilingual Base model by default.",
        after_long_help = "\
EXAMPLES:
  # Record until Ctrl+C, then transcribe
  sagascript record

  # Record for 10 seconds in Norwegian
  sagascript record --duration 10 --language no

  # Save raw audio without transcribing
  sagascript record --output capture.wav

  # Record, transcribe, and copy to clipboard
  sagascript record --clipboard

  # Record with JSON output
  sagascript record --duration 5 --json"
    )]
    Record(record::RecordArgs),

    /// List available whisper models
    #[command(
        long_about = "\
List all available Whisper models with their size and download status.

Models are organized by language. English uses OpenAI Whisper models, \
Swedish uses KBLab models, Norwegian uses NbAiLab models, and Finnish uses \
generic multilingual Whisper models (Base is recommended). \
Use --language to filter the list.

The DOWNLOADED column shows whether each model is already available locally.",
        after_long_help = "\
EXAMPLES:
  # List all models
  sagascript list-models

  # List only Swedish models
  sagascript list-models --language sv

  # List English models
  sagascript list-models --language en

  # List Finnish models
  sagascript list-models --language fi"
    )]
    ListModels(models::ListModelsArgs),

    /// Download a whisper model
    #[command(
        long_about = "\
Download a verified Whisper model from its pinned source to the local model directory.

Models are stored in ~/.sagascript/models/. If the model is already \
downloaded, prints its path and exits without re-downloading.

A progress indicator shows download progress. On success, prints the \
path to the downloaded model file on stdout.",
        after_long_help = "\
EXAMPLES:
  # Download the recommended English model
  sagascript download-model base.en

  # Download a Swedish model
  sagascript download-model kb-whisper-base

  # Download and verify
  sagascript download-model nb-whisper-small && echo 'Done!'

AVAILABLE MODELS:
  English:    tiny.en, base.en
  Swedish:    kb-whisper-tiny, kb-whisper-base, kb-whisper-small
  Norwegian:  nb-whisper-tiny, nb-whisper-base, nb-whisper-small
  Finnish:    base (generic multilingual default)
  Multilingual: tiny, base

DIARIZATION MODELS (requires --features diarization):
  pyannote-segmentation   Speaker segmentation (~6 MB)
  wespeaker-embedding     Speaker embeddings (~27 MB)
  diarization             Download both models at once"
    )]
    DownloadModel(models::DownloadModelArgs),

    /// Delete a downloaded model
    #[command(
        long_about = "\
Delete a previously downloaded Whisper model from disk.

Frees up disk space by removing the model file. The model can be \
re-downloaded later with 'sagascript download-model'.",
        after_long_help = "\
EXAMPLES:
  # Delete a specific model
  sagascript delete-model base.en

  # List models to see which are downloaded
  sagascript list-models"
    )]
    DeleteModel(models::DeleteModelArgs),

    /// Open or focus the installed Sagascript desktop app
    #[command(
        long_about = "\
Open the installed Sagascript desktop app or focus its Settings window when it is already running.

This is a recovery path when the menu-bar status item is unavailable. On macOS, the command asks Launch Services to open the signed app bundle; it never changes saved settings or permissions.",
        after_long_help = "EXAMPLES:\n  sagascript open"
    )]
    Open,

    /// Send a presenter start, finish, or cancel request to the installed desktop app
    #[command(
        long_about = "Send one private presenter request to the installed Sagascript desktop app. The command does not record audio, transcribe, insert text, or report completion; check the desktop status for the result.",
        after_long_help = "EXAMPLES:\n  sagascript presenter start\n  sagascript presenter start swedish\n  sagascript presenter finish\n  sagascript presenter cancel"
    )]
    Presenter(presenter::PresenterArgs),

    /// Reset first-launch onboarding (re-run setup wizard on next launch)
    #[command(
        long_about = "\
Reset the onboarding flag so the setup wizard runs again on next GUI launch.

Useful for testing or if you want to re-configure language and permissions."
    )]
    ResetOnboarding,

    /// Manage settings (list, get, set, reset, path)
    #[command(
        long_about = "\
View and modify Sagascript settings. Settings are persisted to a JSON file \
and take effect immediately (the GUI hot-reloads changes made via CLI).

Available setting keys:
  language           Language for transcription (en, sv, no, fi, auto)
  whisper_model      Whisper model ID (e.g. base.en, kb-whisper-base)
  hotkey_mode        Hotkey behavior: push (push-to-talk) or toggle
  show_overlay       Show recording overlay (true/false)
  auto_paste         Auto-paste transcription result (true/false)
  auto_select_model  Auto-select best model for language (true/false)
  hotkey             Modifier+Key; bare F13-F24 on macOS (Accessibility) or Windows",
        after_long_help = "\
EXAMPLES:
  # Show all settings with current and default values
  sagascript config list

  # Get a single setting
  sagascript config get language

  # Change language to Swedish
  sagascript config set language sv

  # Change the global hotkey
  sagascript config set hotkey 'Option+Space'
  sagascript config set hotkey F13

  # Reset a single setting to its default
  sagascript config reset language

  # Reset ALL settings to defaults
  sagascript config reset

  # Print the settings file path (for manual editing)
  sagascript config path"
    )]
    Config(config::ConfigArgs),

    /// Manage global hint terms and explicit profile-scoped aliases used by live and batch transcription
    #[command(
        long_about = "Add preferred spellings and optional exact mishearings. Global and one-run terms prime Whisper as hint-only context; deterministic aliases require a selected known profile with an explicit language. No stored text is migrated or deleted.",
        after_long_help = "EXAMPLES:\n  sagascript glossary path\n  sagascript glossary path --profile swedish\n  sagascript glossary list\n  sagascript glossary add OpenRouter\n  sagascript glossary add OpenRouter --alias 'open router' --alias 'open vrouter' --profile swedish\n  sagascript glossary add merge --alias merch --profile swedish\n  sagascript glossary suggest heard.txt --corrected corrected.txt --profile swedish\n  sagascript glossary suggest heard.txt --corrected corrected.txt --profile swedish --apply\n  sagascript glossary remove OpenRouter --profile swedish\n  sagascript glossary clear --yes"
    )]
    Glossary(glossary::GlossaryArgs),

    /// List supported audio/video file formats
    #[command(
        long_about = "\
Print all audio and video file formats that Sagascript can decode \
for transcription. These formats are supported by both the 'transcribe' \
subcommand and the GUI file-drop feature."
    )]
    Formats,

    /// Generate shell completions
    #[command(
        long_about = "\
Generate shell completion scripts for the specified shell.

Output is written to stdout. Redirect to a file and source it \
in your shell configuration to enable tab-completion for all \
Sagascript commands, subcommands, and options.",
        after_long_help = "\
EXAMPLES:
  # Zsh (add to ~/.zshrc or place in fpath)
  sagascript completions zsh > ~/.zfunc/_sagascript

  # Bash (add to ~/.bashrc)
  sagascript completions bash > ~/.local/share/bash-completion/completions/sagascript

  # Fish
  sagascript completions fish > ~/.config/fish/completions/sagascript.fish

  # PowerShell
  sagascript completions powershell >> $PROFILE"
    )]
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },

    /// Generate man pages
    #[command(
        long_about = "\
Generate roff man pages for Sagascript and all subcommands.

If --dir is given, writes one .1 file per command into that directory. \
Otherwise, writes the main man page to stdout.",
        after_long_help = "\
EXAMPLES:
  # View the man page directly
  sagascript manpages | man -l -

  # Generate all man pages into a directory
  sagascript manpages --dir /usr/local/share/man/man1

  # Generate into a local directory
  mkdir -p man && sagascript manpages --dir man"
    )]
    Manpages {
        /// Directory to write man page files into (one .1 file per command)
        #[arg(short, long, value_name = "DIR")]
        dir: Option<PathBuf>,
    },
}

/// Try to parse CLI args. Returns Some(Cli) if a subcommand was given, None for bare invocation (GUI mode).
pub fn try_parse() -> Option<Cli> {
    let cli = Cli::parse();
    if cli.command.is_some() {
        Some(cli)
    } else {
        None
    }
}

/// Run the CLI subcommand. Blocks until complete, then exits.
pub fn run(cli: Cli) {
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

    let result = match cli.command.unwrap() {
        Command::BenchmarkDictation(args) =>
            run_inference_command("benchmark-dictation", move || benchmark_dictation::run(args)),
        Command::Transcribe(args) =>
            run_inference_command("transcribe", move || transcribe::run(args)),
        Command::LatencyReport(args) => latency::run(args),
        #[cfg(feature = "record")]
        Command::Record(args) => run_inference_command("record", move || record::run(args)),
        Command::ListModels(args) => models::list(args),
        Command::DownloadModel(args) => rt.block_on(models::download(args)),
        Command::DeleteModel(args) => models::delete(args),
        Command::Open => open::run(),
        Command::Presenter(args) => presenter::run(args),
        Command::ResetOnboarding => {
            sagascript_core::settings::store::update(|settings| {
                settings.has_completed_onboarding = false;
            })
                .map_err(sagascript_core::error::DictationError::SettingsError)
                .map(|_| {
                    eprintln!("Onboarding reset. The setup wizard will run on next launch.");
                })
        }
        Command::Config(args) => config::run(args),
        Command::Glossary(args) => glossary::run(args),
        Command::Formats => {
            formats();
            Ok(())
        }
        Command::Completions { shell } => {
            generate_completions(shell);
            Ok(())
        }
        Command::Manpages { dir } => generate_manpages(dir),
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "windows")]
fn run_inference_command(
    command: &'static str,
    task: impl FnOnce() -> Result<(), sagascript_core::error::DictationError> + Send + 'static,
) -> Result<(), sagascript_core::error::DictationError> {
    // MSVC executables start with a much smaller main-thread stack than the
    // worker threads used by the desktop app. Whisper inference exhausted it
    // with STATUS_STACK_OVERFLOW (0xC00000FD) in the real Windows CI gate.
    std::thread::Builder::new()
        .name(format!("sagascript-cli-{command}"))
        .stack_size(WINDOWS_INFERENCE_STACK_BYTES)
        .spawn(task)
        .map_err(|error| {
            sagascript_core::error::DictationError::ApplicationLaunchError(format!(
                "Failed to start Windows {command} worker: {error}"
            ))
        })?
        .join()
        .map_err(|_| {
            sagascript_core::error::DictationError::TranscriptionFailed(format!(
                "Windows {command} worker terminated unexpectedly"
            ))
        })?
}

#[cfg(not(target_os = "windows"))]
fn run_inference_command(
    _command: &'static str,
    task: impl FnOnce() -> Result<(), sagascript_core::error::DictationError>,
) -> Result<(), sagascript_core::error::DictationError> {
    task()
}

fn formats() {
    use sagascript_core::audio::decoder::SUPPORTED_EXTENSIONS;

    println!("Supported audio/video formats:");
    for ext in SUPPORTED_EXTENSIONS {
        println!("  .{ext}");
    }
}

fn generate_completions<G: Generator>(gen: G) {
    clap_complete::generate(gen, &mut Cli::command(), "sagascript", &mut io::stdout());
}

fn generate_manpages(dir: Option<PathBuf>) -> Result<(), sagascript_core::error::DictationError> {
    let cmd = Cli::command();

    let map_err = |e: io::Error| {
        sagascript_core::error::DictationError::SettingsError(format!("Failed to generate man pages: {e}"))
    };

    match dir {
        Some(dir) => {
            std::fs::create_dir_all(&dir).map_err(|e| {
                sagascript_core::error::DictationError::SettingsError(format!(
                    "Failed to create directory '{}': {e}",
                    dir.display()
                ))
            })?;

            // Generate man pages for root command and all subcommands
            render_manpage_tree(&cmd, &dir).map_err(map_err)?;

            Ok(())
        }
        None => {
            // Write just the root man page to stdout
            let man = clap_mangen::Man::new(cmd);
            let mut buf = Vec::new();
            man.render(&mut buf).map_err(map_err)?;
            io::stdout().write_all(&buf).map_err(map_err)?;
            Ok(())
        }
    }
}

fn render_manpage_tree(cmd: &clap::Command, dir: &PathBuf) -> Result<(), io::Error> {
    let man = clap_mangen::Man::new(cmd.clone());
    let name = cmd.get_name().replace(' ', "-");
    let path = dir.join(format!("{name}.1"));
    let mut file = std::fs::File::create(&path)?;
    man.render(&mut file)?;
    eprintln!("Generated: {}", path.display());

    for sub in cmd.get_subcommands() {
        if sub.get_name() == "help" {
            continue;
        }
        let mut sub = sub.clone();
        let full_name = format!("{}-{}", cmd.get_name(), sub.get_name());
        sub = sub.name(&full_name);
        render_manpage_tree(&sub, dir)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_inference_command_has_a_large_named_worker_stack() {
        run_inference_command("stack-test", || {
            let stack_probe = [0_u8; 4 * 1024 * 1024];
            assert_eq!(
                std::thread::current().name(),
                Some("sagascript-cli-stack-test")
            );
            std::hint::black_box(&stack_probe);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn log_filter_suppresses_native_diagnostics_by_default() {
        assert_eq!(effective_log_filter(None, "warn"), DEFAULT_CLI_LOG_FILTER);
        assert_eq!(
            effective_log_filter(Some("info"), "warn"),
            format!("info,{NATIVE_LOG_SUPPRESSION}")
        );
        assert_eq!(
            effective_log_filter(Some("  "), "info"),
            format!("info,{NATIVE_LOG_SUPPRESSION}")
        );
    }

    #[test]
    fn log_filter_preserves_explicit_native_diagnostics_opt_in() {
        let configured = "warn,whisper_rs=info";
        assert_eq!(
            effective_log_filter(Some(configured), "warn"),
            configured
        );
    }

    #[test]
    fn transcription_progress_bar_never_renders_outside_zero_to_one_hundred() {
        let progress = indicatif::ProgressBar::new(100);

        set_transcription_progress(&progress, 101);
        assert_eq!(progress.position(), 100);

        set_transcription_progress(&progress, -1);
        assert_eq!(progress.position(), 0);
    }

    #[test]
    fn long_version_identifies_the_exact_build() {
        assert!(LONG_VERSION.contains(env!("CARGO_PKG_VERSION")));
        assert!(LONG_VERSION.contains("git "));
        assert!(LONG_VERSION.contains("built "));

        let command = Cli::command();
        assert_eq!(command.get_long_version(), Some(LONG_VERSION));
    }

    #[test]
    fn benchmark_dictation_is_discoverable_with_gate_arguments() {
        let command = Cli::command();
        let benchmark = command
            .find_subcommand("benchmark-dictation")
            .expect("benchmark-dictation should be a root subcommand");
        assert!(benchmark.get_arguments().any(|arg| arg.get_id() == "language"));
        assert!(benchmark.get_arguments().any(|arg| arg.get_id() == "iterations"));
        assert!(benchmark.get_arguments().any(|arg| arg.get_id() == "max_warm_ms"));
        assert!(benchmark.get_arguments().any(|arg| arg.get_id() == "expect_word"));
    }

    #[test]
    fn presenter_commands_are_discoverable_and_parse_without_launching() {
        let command = Cli::command();
        let presenter = command
            .find_subcommand("presenter")
            .expect("presenter should be a root subcommand");
        let names: Vec<_> = presenter
            .get_subcommands()
            .map(|subcommand| subcommand.get_name())
            .collect();
        assert_eq!(names, ["start", "finish", "cancel"]);

        let parsed = Cli::try_parse_from(["sagascript", "presenter", "start", "swedish"])
            .expect("presenter start should parse");
        assert!(matches!(
            parsed.command,
            Some(Command::Presenter(presenter::PresenterArgs {
                action: presenter::PresenterAction::Start {
                    profile_id: Some(_)
                }
            }))
        ));
    }

    // -- Completions generation --

    #[test]
    fn completions_generate_bash() {
        let mut buf = Vec::new();
        clap_complete::generate(Shell::Bash, &mut Cli::command(), "sagascript", &mut buf);
        assert!(!buf.is_empty(), "bash completions should not be empty");
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("sagascript"), "should reference the binary name");
    }

    #[test]
    fn completions_generate_zsh() {
        let mut buf = Vec::new();
        clap_complete::generate(Shell::Zsh, &mut Cli::command(), "sagascript", &mut buf);
        assert!(!buf.is_empty(), "zsh completions should not be empty");
    }

    #[test]
    fn completions_generate_fish() {
        let mut buf = Vec::new();
        clap_complete::generate(Shell::Fish, &mut Cli::command(), "sagascript", &mut buf);
        assert!(!buf.is_empty(), "fish completions should not be empty");
    }

    #[test]
    fn completions_generate_powershell() {
        let mut buf = Vec::new();
        clap_complete::generate(Shell::PowerShell, &mut Cli::command(), "sagascript", &mut buf);
        assert!(!buf.is_empty(), "powershell completions should not be empty");
    }

    #[test]
    fn completions_generate_elvish() {
        let mut buf = Vec::new();
        clap_complete::generate(Shell::Elvish, &mut Cli::command(), "sagascript", &mut buf);
        assert!(!buf.is_empty(), "elvish completions should not be empty");
    }

    // -- Man page rendering --

    #[test]
    fn manpage_renders_root() {
        let cmd = Cli::command();
        let man = clap_mangen::Man::new(cmd);
        let mut buf = Vec::new();
        man.render(&mut buf).expect("root man page should render");
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("sagascript"), "man page should contain binary name");
    }

    #[test]
    fn manpage_renders_all_subcommands_to_dir() {
        let dir = std::env::temp_dir().join(format!("sagascript-man-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let cmd = Cli::command();
        render_manpage_tree(&cmd, &dir).expect("man page tree should render");

        // Root page must exist
        assert!(dir.join("sagascript.1").exists(), "root man page missing");

        // Subcommand pages
        let expected = [
            "sagascript-transcribe.1",
            #[cfg(feature = "record")]
            "sagascript-record.1",
            "sagascript-list-models.1",
            "sagascript-download-model.1",
            "sagascript-config.1",
            "sagascript-formats.1",
            "sagascript-completions.1",
            "sagascript-manpages.1",
        ];
        for name in expected {
            assert!(dir.join(name).exists(), "missing man page: {name}");
        }

        // Nested config subcommand pages
        let config_subs = [
            "sagascript-config-list.1",
            "sagascript-config-get.1",
            "sagascript-config-set.1",
            "sagascript-config-reset.1",
            "sagascript-config-path.1",
        ];
        for name in config_subs {
            assert!(dir.join(name).exists(), "missing config man page: {name}");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- Help text content --

    fn get_long_help(cmd: &clap::Command) -> String {
        cmd.clone().render_long_help().to_string()
    }

    #[test]
    fn root_help_contains_examples() {
        let help = get_long_help(&Cli::command());
        assert!(help.contains("EXAMPLES:"), "root help should contain EXAMPLES section");
        assert!(help.contains("sagascript transcribe"), "root help should show transcribe example");
        #[cfg(feature = "record")]
        assert!(help.contains("sagascript record"), "root help should show record example");
    }

    #[test]
    fn root_help_contains_auto_detect_caveat() {
        let help = get_long_help(&Cli::command());
        assert!(
            help.contains("Auto-detect uses a generic multilingual model"),
            "root help should warn about auto-detect accuracy: {help}"
        );
    }

    #[test]
    fn transcribe_help_contains_examples() {
        let cmd = Cli::command();
        let sub = cmd.find_subcommand("transcribe").expect("transcribe subcommand missing");
        let help = get_long_help(sub);
        assert!(help.contains("EXAMPLES:"), "transcribe help should contain EXAMPLES");
        assert!(help.contains("--json"), "transcribe help should mention --json");
    }

    #[test]
    fn transcribe_help_contains_auto_detect_caveat() {
        let cmd = Cli::command();
        let sub = cmd.find_subcommand("transcribe").unwrap();
        let help = get_long_help(sub);
        assert!(
            help.contains("auto uses a generic multilingual model"),
            "transcribe help should warn about auto-detect"
        );
    }

    #[test]
    fn config_set_help_explains_auto_paste_permission_requirement() {
        let cmd = Cli::command();
        let config = cmd.find_subcommand("config").unwrap();
        let set = config.find_subcommand("set").unwrap();
        let help = get_long_help(set);
        assert!(
            help.contains("enabling requires Accessibility approval for the installed GUI"),
            "config set help should explain the auto-paste permission requirement"
        );
    }

    /// Batch-only builds must not advertise the absent `record` subcommand
    /// anywhere in the root help (workflow, examples) — the batch contract
    /// is "no record", and stale mentions would send users to a command
    /// that doesn't exist.
    #[cfg(not(feature = "record"))]
    #[test]
    fn record_fully_absent_without_feature() {
        let mut cmd = Cli::command();
        assert!(
            cmd.find_subcommand("record").is_none(),
            "record subcommand must not exist without the record feature"
        );
        let help = cmd.render_long_help().to_string();
        assert!(
            !help.contains("sagascript record"),
            "root help must not mention 'sagascript record' without the record feature"
        );
    }

    #[cfg(feature = "record")]
    #[test]
    fn record_help_contains_examples() {
        let cmd = Cli::command();
        let sub = cmd.find_subcommand("record").expect("record subcommand missing");
        let help = get_long_help(sub);
        assert!(help.contains("EXAMPLES:"), "record help should contain EXAMPLES");
        assert!(help.contains("Ctrl+C"), "record help should mention Ctrl+C");
    }

    #[test]
    fn all_subcommands_have_long_about() {
        let cmd = Cli::command();
        for sub in cmd.get_subcommands() {
            if sub.get_name() == "help" {
                continue;
            }
            assert!(
                sub.get_long_about().is_some(),
                "subcommand '{}' is missing long_about",
                sub.get_name()
            );
        }
    }

    // -- Clap arg parsing --

    #[test]
    fn parse_transcribe_minimal() {
        let cli = Cli::try_parse_from(["sagascript", "transcribe", "file.wav"]).unwrap();
        match cli.command.unwrap() {
            Command::Transcribe(args) => {
                assert_eq!(args.files, vec![PathBuf::from("file.wav")]);
                assert!(args.language.is_none());
                assert!(args.model.is_none());
                assert!(!args.json);
                assert!(!args.jsonl);
                assert!(!args.recursive);
                assert!(!args.fail_fast);
                assert!(!args.clipboard);
            }
            other => panic!("expected Transcribe, got {:?}", std::mem::discriminant(&other)),
        }
    }

    #[test]
    fn parse_transcribe_all_flags() {
        let cli = Cli::try_parse_from([
            "sagascript", "transcribe", "meeting.mp3",
            "--language", "sv",
            "--model", "kb-whisper-base",
            "--json",
            "--clipboard",
            "--prompt", "Notre Dame, Sara",
            "--correct-hints",
        ]).unwrap();
        match cli.command.unwrap() {
            Command::Transcribe(args) => {
                assert_eq!(args.files, vec![PathBuf::from("meeting.mp3")]);
                assert_eq!(args.language.as_deref(), Some("sv"));
                assert_eq!(args.model.as_deref(), Some("kb-whisper-base"));
                assert!(args.json);
                assert!(args.clipboard);
                assert_eq!(args.prompt.as_deref(), Some("Notre Dame, Sara"));
                assert!(args.prompt_file.is_none());
                assert!(args.correct_hints);
            }
            _ => panic!("expected Transcribe"),
        }
    }

    #[cfg(feature = "diarization")]
    #[test]
    fn diarization_cache_requires_diarization() {
        assert!(Cli::try_parse_from([
            "sagascript",
            "transcribe",
            "meeting.m4a",
            "--diarize-cache",
            "analysis.json",
        ])
        .is_err());

        let cli = Cli::try_parse_from([
            "sagascript",
            "transcribe",
            "meeting.m4a",
            "--diarize",
            "--diarize-cache",
            "analysis.json",
        ])
        .unwrap();
        let Command::Transcribe(args) = cli.command.unwrap() else {
            panic!("expected Transcribe");
        };
        assert_eq!(args.diarize_cache, Some(PathBuf::from("analysis.json")));
    }

    #[test]
    fn parse_transcribe_batch_flags_and_inputs() {
        let cli = Cli::try_parse_from([
            "sagascript",
            "transcribe",
            "one.wav",
            "recordings",
            "--recursive",
            "--jsonl",
            "--fail-fast",
        ])
        .unwrap();
        match cli.command.unwrap() {
            Command::Transcribe(args) => {
                assert_eq!(
                    args.files,
                    vec![PathBuf::from("one.wav"), PathBuf::from("recordings")]
                );
                assert!(args.recursive);
                assert!(args.jsonl);
                assert!(args.fail_fast);
            }
            _ => panic!("expected Transcribe"),
        }
    }

    #[test]
    fn parse_transcribe_hint_alias() {
        // --hint is a visible alias of --prompt and populates the same field.
        let cli = Cli::try_parse_from([
            "sagascript", "transcribe", "f.wav", "--hint", "Estrid, Grimnir",
        ]).unwrap();
        match cli.command.unwrap() {
            Command::Transcribe(args) => {
                assert_eq!(args.prompt.as_deref(), Some("Estrid, Grimnir"));
            }
            _ => panic!("expected Transcribe"),
        }
    }

    #[test]
    fn parse_transcribe_prompt_file() {
        // --hint-file is a visible alias of --prompt-file.
        let cli = Cli::try_parse_from([
            "sagascript", "transcribe", "f.wav", "--hint-file", "vocab.txt",
        ]).unwrap();
        match cli.command.unwrap() {
            Command::Transcribe(args) => {
                assert_eq!(args.prompt_file, Some(PathBuf::from("vocab.txt")));
                assert!(args.prompt.is_none());
            }
            _ => panic!("expected Transcribe"),
        }
    }

    #[test]
    fn parse_transcribe_prompt_and_file_conflict() {
        // --prompt and --prompt-file are mutually exclusive.
        let result = Cli::try_parse_from([
            "sagascript", "transcribe", "f.wav",
            "--prompt", "inline", "--prompt-file", "vocab.txt",
        ]);
        assert!(result.is_err(), "expected --prompt + --prompt-file to conflict");
    }

    #[test]
    fn parse_transcribe_correct_hints_requires_json() {
        let result = Cli::try_parse_from([
            "sagascript", "transcribe", "f.wav", "--correct-hints",
        ]);
        assert!(result.is_err(), "expected --correct-hints to require --json");
    }

    #[test]
    fn parse_transcribe_short_flags() {
        let cli = Cli::try_parse_from([
            "sagascript", "transcribe", "f.wav", "-l", "en", "-m", "base.en",
        ]).unwrap();
        match cli.command.unwrap() {
            Command::Transcribe(args) => {
                assert_eq!(args.language.as_deref(), Some("en"));
                assert_eq!(args.model.as_deref(), Some("base.en"));
            }
            _ => panic!("expected Transcribe"),
        }
    }

    #[cfg(feature = "record")]
    #[test]
    fn parse_record_minimal() {
        let cli = Cli::try_parse_from(["sagascript", "record"]).unwrap();
        match cli.command.unwrap() {
            Command::Record(args) => {
                assert!(args.language.is_none());
                assert!(args.model.is_none());
                assert!(args.duration.is_none());
                assert!(args.output.is_none());
                assert!(!args.json);
                assert!(!args.clipboard);
                assert!(args.prompt.is_none());
                assert!(args.prompt_file.is_none());
            }
            _ => panic!("expected Record"),
        }
    }

    #[cfg(feature = "record")]
    #[test]
    fn parse_record_all_flags() {
        let cli = Cli::try_parse_from([
            "sagascript", "record",
            "--language", "no",
            "--model", "nb-whisper-base",
            "--duration", "30.5",
            "--output", "capture.wav",
            "--json",
            "--clipboard",
            "--hint", "Notre Dame, Sara",
        ]).unwrap();
        match cli.command.unwrap() {
            Command::Record(args) => {
                assert_eq!(args.language.as_deref(), Some("no"));
                assert_eq!(args.model.as_deref(), Some("nb-whisper-base"));
                assert!((args.duration.unwrap() - 30.5).abs() < f64::EPSILON);
                assert_eq!(args.output.as_deref(), Some("capture.wav"));
                assert!(args.json);
                assert!(args.clipboard);
                // --hint populates the same `prompt` field as --prompt.
                assert_eq!(args.prompt.as_deref(), Some("Notre Dame, Sara"));
            }
            _ => panic!("expected Record"),
        }
    }

    #[test]
    fn parse_glossary_add_with_repeated_aliases() {
        let cli = Cli::try_parse_from([
            "sagascript",
            "glossary",
            "add",
            "OpenRouter",
            "--alias",
            "open router",
            "--alias",
            "open vrouter",
        ])
        .unwrap();
        match cli.command.unwrap() {
            Command::Glossary(args) => match args.action {
                glossary::GlossaryAction::Add { term, aliases, profile } => {
                    assert_eq!(term, "OpenRouter");
                    assert_eq!(aliases, vec!["open router", "open vrouter"]);
                    assert!(profile.is_none());
                }
                _ => panic!("expected glossary add"),
            },
            _ => panic!("expected Glossary"),
        }
    }

    #[test]
    fn parse_profile_glossary_path() {
        let cli = Cli::try_parse_from([
            "sagascript",
            "glossary",
            "path",
            "--profile",
            "swedish",
        ])
        .unwrap();
        match cli.command.unwrap() {
            Command::Glossary(args) => match args.action {
                glossary::GlossaryAction::Path { profile } => {
                    assert_eq!(profile.as_deref(), Some("swedish"));
                }
                _ => panic!("expected glossary path"),
            },
            _ => panic!("expected Glossary"),
        }
    }

    #[test]
    fn parse_glossary_suggest_is_dry_run_by_default_and_profile_scoped() {
        let cli = Cli::try_parse_from([
            "sagascript",
            "glossary",
            "suggest",
            "heard.txt",
            "--corrected",
            "corrected.txt",
            "--profile",
            "swedish",
            "--json",
        ])
        .unwrap();
        match cli.command.unwrap() {
            Command::Glossary(args) => match args.action {
                glossary::GlossaryAction::Suggest {
                    heard,
                    corrected,
                    profile,
                    json,
                    apply,
                } => {
                    assert_eq!(heard, PathBuf::from("heard.txt"));
                    assert_eq!(corrected, PathBuf::from("corrected.txt"));
                    assert_eq!(profile, "swedish");
                    assert!(json);
                    assert!(!apply);
                }
                _ => panic!("expected glossary suggest"),
            },
            _ => panic!("expected Glossary"),
        }
    }

    #[test]
    fn parse_list_models_with_language() {
        let cli = Cli::try_parse_from(["sagascript", "list-models", "-l", "sv"]).unwrap();
        match cli.command.unwrap() {
            Command::ListModels(args) => {
                assert_eq!(args.language.as_deref(), Some("sv"));
            }
            _ => panic!("expected ListModels"),
        }
    }

    #[test]
    fn parse_download_model() {
        let cli = Cli::try_parse_from(["sagascript", "download-model", "base.en"]).unwrap();
        match cli.command.unwrap() {
            Command::DownloadModel(args) => {
                assert_eq!(args.model, "base.en");
            }
            _ => panic!("expected DownloadModel"),
        }
    }

    #[test]
    fn parse_open_gui() {
        let cli = Cli::try_parse_from(["sagascript", "open"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Open)));
    }

    #[test]
    fn parse_config_set() {
        let cli = Cli::try_parse_from(["sagascript", "config", "set", "language", "sv"]).unwrap();
        match cli.command.unwrap() {
            Command::Config(args) => match args.action {
                config::ConfigAction::Set { key, value } => {
                    assert_eq!(key, "language");
                    assert_eq!(value, "sv");
                }
                _ => panic!("expected ConfigAction::Set"),
            },
            _ => panic!("expected Config"),
        }
    }

    #[test]
    fn parse_config_reset_all() {
        let cli = Cli::try_parse_from(["sagascript", "config", "reset"]).unwrap();
        match cli.command.unwrap() {
            Command::Config(args) => match args.action {
                config::ConfigAction::Reset { key } => {
                    assert!(key.is_none(), "reset without key should be None");
                }
                _ => panic!("expected ConfigAction::Reset"),
            },
            _ => panic!("expected Config"),
        }
    }

    #[test]
    fn parse_config_profile_create() {
        let cli = Cli::try_parse_from([
            "sagascript", "config", "profiles", "create", "swedish",
            "--name", "Swedish", "--hotkey", "Option+Space", "--language", "sv",
        ]).unwrap();
        match cli.command.unwrap() {
            Command::Config(args) => match args.action {
                config::ConfigAction::Profiles { action: config::ProfileAction::Create { id, name, hotkey, language } } => {
                    assert_eq!(id, "swedish");
                    assert_eq!(name, "Swedish");
                    assert_eq!(hotkey, "Option+Space");
                    assert_eq!(language, "sv");
                }
                _ => panic!("expected profile create"),
            },
            _ => panic!("expected Config"),
        }
    }

    #[test]
    fn parse_completions() {
        let cli = Cli::try_parse_from(["sagascript", "completions", "zsh"]).unwrap();
        match cli.command.unwrap() {
            Command::Completions { shell } => {
                assert_eq!(shell, Shell::Zsh);
            }
            _ => panic!("expected Completions"),
        }
    }

    #[test]
    fn parse_manpages_with_dir() {
        let cli = Cli::try_parse_from(["sagascript", "manpages", "--dir", "/tmp/man"]).unwrap();
        match cli.command.unwrap() {
            Command::Manpages { dir } => {
                assert_eq!(dir, Some(PathBuf::from("/tmp/man")));
            }
            _ => panic!("expected Manpages"),
        }
    }

    #[test]
    fn parse_no_subcommand_is_none() {
        let cli = Cli::try_parse_from(["sagascript"]).unwrap();
        assert!(cli.command.is_none(), "no subcommand should yield None (GUI mode)");
    }

    #[test]
    fn parse_unknown_subcommand_is_error() {
        let result = Cli::try_parse_from(["sagascript", "nonexistent"]);
        assert!(result.is_err());
    }
}
