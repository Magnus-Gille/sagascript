//! Headless entry point: CLI-only binary (no Tauri / GUI). Used for the
//! batch-transcription build (e.g. on Linux). A bare invocation (no
//! subcommand) prints help, since there is no GUI to launch.

use clap::CommandFactory;
use tracing_subscriber::EnvFilter;

fn main() {
    // Warn-level logging to stderr keeps stdout clean for piped output.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    match sagascript_cli::try_parse() {
        Some(parsed) => sagascript_cli::run(parsed),
        None => {
            // No subcommand and no GUI to fall back to — show help and exit non-zero.
            let _ = sagascript_cli::Cli::command().print_help();
            println!();
            std::process::exit(2);
        }
    }
}
