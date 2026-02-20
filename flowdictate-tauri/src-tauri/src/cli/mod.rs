pub mod models;
pub mod record;
pub mod transcribe;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "flowdictate", version, about = "Low-latency dictation app")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Transcribe an audio/video file
    Transcribe(transcribe::TranscribeArgs),

    /// Record from microphone and transcribe
    Record(record::RecordArgs),

    /// List available whisper models
    ListModels(models::ListModelsArgs),

    /// Download a whisper model
    DownloadModel(models::DownloadModelArgs),

    /// List supported audio/video file formats
    Formats,
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
        Command::Formats => {
            formats();
            Ok(())
        }
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
