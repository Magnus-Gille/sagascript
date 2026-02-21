pub mod config;
pub mod models;
pub mod record;
pub mod transcribe;

use std::io::{self, Write};
use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Generator, Shell};

#[derive(Parser)]
#[command(
    name = "sagascript",
    version,
    about = "Low-latency dictation app",
    long_about = "\
Sagascript is a privacy-first dictation app that transcribes speech to text \
using local Whisper models. It runs as a macOS menu bar app (GUI) or as a \
standalone CLI tool.

When invoked without a subcommand, Sagascript launches the GUI. \
Use any subcommand below to operate in CLI mode instead.

Workflow:
  1. Download a model:   sagascript download-model base.en
  2. Transcribe a file:  sagascript transcribe recording.wav
  3. Or record live:      sagascript record

Supported languages: English (en), Swedish (sv), Norwegian (no), Auto-detect (auto).
Models are downloaded from HuggingFace and stored locally.

NOTE: Auto-detect uses a generic multilingual model which is less accurate \
than the dedicated language models (KBLab for Swedish, NbAiLab for Norwegian). \
For best results, set a specific language.",
    after_long_help = "\
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
  RUST_LOG    Set log level (default: warn for CLI). Example: RUST_LOG=info"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Transcribe an audio/video file
    #[command(
        long_about = "\
Transcribe an audio or video file to text using a local Whisper model.

The file is decoded to 16 kHz mono PCM, then processed by the selected \
Whisper model. Supports WAV, MP3, M4A, AAC, MP4, MOV, OGG, WebM, and FLAC.

By default, uses the language and model from your persisted settings \
(see 'sagascript config list'). Override with --language and --model.

NOTE: --language auto uses a generic multilingual model which is less \
accurate than the dedicated language models. For best results, specify \
a language explicitly (en, sv, no).",
        after_long_help = "\
EXAMPLES:
  # Basic transcription (uses configured language/model)
  sagascript transcribe meeting.wav

  # Transcribe in Swedish with a specific model
  sagascript transcribe tal.m4a --language sv --model kb-whisper-base

  # Output as JSON (includes metadata)
  sagascript transcribe podcast.mp3 --json

  # Transcribe and copy to clipboard
  sagascript transcribe note.wav --clipboard

  # Pipe-friendly: JSON to jq
  sagascript transcribe call.wav --json | jq -r .text"
    )]
    Transcribe(transcribe::TranscribeArgs),

    /// Record from microphone and transcribe
    #[command(
        long_about = "\
Record audio from the default microphone and transcribe it.

Recording continues until you press Ctrl+C, or until --duration seconds \
have elapsed. The captured audio is then transcribed using the selected model.

Use --output to save the raw audio as a WAV file without transcribing \
(useful for capturing audio to process later with 'sagascript transcribe').

NOTE: --language auto uses a generic multilingual model which is less \
accurate than the dedicated language models. For best results, specify \
a language explicitly (en, sv, no).",
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
Swedish uses KBLab models, and Norwegian uses NbAiLab models. \
Use --language to filter the list.

The DOWNLOADED column shows whether each model is already available locally.",
        after_long_help = "\
EXAMPLES:
  # List all models
  sagascript list-models

  # List only Swedish models
  sagascript list-models --language sv

  # List English models
  sagascript list-models --language en"
    )]
    ListModels(models::ListModelsArgs),

    /// Download a whisper model
    #[command(
        long_about = "\
Download a Whisper model from HuggingFace to the local model directory.

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
  Multilingual: tiny, base"
    )]
    DownloadModel(models::DownloadModelArgs),

    /// Manage settings (list, get, set, reset, path)
    #[command(
        long_about = "\
View and modify Sagascript settings. Settings are persisted to a JSON file \
and take effect immediately (the GUI hot-reloads changes made via CLI).

Available setting keys:
  language           Language for transcription (en, sv, no, auto)
  whisper_model      Whisper model ID (e.g. base.en, kb-whisper-base)
  hotkey_mode        Hotkey behavior: push (push-to-talk) or toggle
  show_overlay       Show recording overlay (true/false)
  auto_paste         Auto-paste transcription result (true/false)
  auto_select_model  Auto-select best model for language (true/false)
  hotkey             Global hotkey shortcut (e.g. Control+Shift+Space)",
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

  # Reset a single setting to its default
  sagascript config reset language

  # Reset ALL settings to defaults
  sagascript config reset

  # Print the settings file path (for manual editing)
  sagascript config path"
    )]
    Config(config::ConfigArgs),

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
        Command::Transcribe(args) => transcribe::run(args),
        Command::Record(args) => record::run(args),
        Command::ListModels(args) => models::list(args),
        Command::DownloadModel(args) => rt.block_on(models::download(args)),
        Command::Config(args) => config::run(args),
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

fn formats() {
    use crate::audio::decoder::SUPPORTED_EXTENSIONS;

    println!("Supported audio/video formats:");
    for ext in SUPPORTED_EXTENSIONS {
        println!("  .{ext}");
    }
}

fn generate_completions<G: Generator>(gen: G) {
    clap_complete::generate(gen, &mut Cli::command(), "sagascript", &mut io::stdout());
}

fn generate_manpages(dir: Option<PathBuf>) -> Result<(), crate::error::DictationError> {
    let cmd = Cli::command();

    let map_err = |e: io::Error| {
        crate::error::DictationError::SettingsError(format!("Failed to generate man pages: {e}"))
    };

    match dir {
        Some(dir) => {
            std::fs::create_dir_all(&dir).map_err(|e| {
                crate::error::DictationError::SettingsError(format!(
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

    fn get_help_text(args: &[&str]) -> String {
        let err = Cli::command()
            .try_get_matches_from(args)
            .unwrap_err();
        err.to_string()
    }

    fn get_long_help(cmd: &clap::Command) -> String {
        cmd.clone().render_long_help().to_string()
    }

    #[test]
    fn root_help_contains_examples() {
        let help = get_long_help(&Cli::command());
        assert!(help.contains("EXAMPLES:"), "root help should contain EXAMPLES section");
        assert!(help.contains("sagascript transcribe"), "root help should show transcribe example");
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
                assert_eq!(args.file, PathBuf::from("file.wav"));
                assert!(args.language.is_none());
                assert!(args.model.is_none());
                assert!(!args.json);
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
        ]).unwrap();
        match cli.command.unwrap() {
            Command::Transcribe(args) => {
                assert_eq!(args.file, PathBuf::from("meeting.mp3"));
                assert_eq!(args.language.as_deref(), Some("sv"));
                assert_eq!(args.model.as_deref(), Some("kb-whisper-base"));
                assert!(args.json);
                assert!(args.clipboard);
            }
            _ => panic!("expected Transcribe"),
        }
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
            }
            _ => panic!("expected Record"),
        }
    }

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
        ]).unwrap();
        match cli.command.unwrap() {
            Command::Record(args) => {
                assert_eq!(args.language.as_deref(), Some("no"));
                assert_eq!(args.model.as_deref(), Some("nb-whisper-base"));
                assert!((args.duration.unwrap() - 30.5).abs() < f64::EPSILON);
                assert_eq!(args.output.as_deref(), Some("capture.wav"));
                assert!(args.json);
                assert!(args.clipboard);
            }
            _ => panic!("expected Record"),
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
