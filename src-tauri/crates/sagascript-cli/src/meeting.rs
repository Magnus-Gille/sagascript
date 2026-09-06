//! Read-only CLI operations for validated meeting transcript documents.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use sagascript_core::error::DictationError;
use sagascript_core::meeting::{MeetingExportFormat, MeetingTranscript};

const MAX_INPUT_BYTES: usize = 24 * 1024 * 1024;
const MAX_IO_CONTEXT_CHARS: usize = 160;

#[derive(Args, Debug)]
pub struct MeetingArgs {
    #[command(subcommand)]
    pub action: MeetingAction,
}

#[derive(Subcommand, Debug)]
pub enum MeetingAction {
    /// Emit the validated document as JSON.
    Inspect { input: PathBuf },
    /// Export the validated document without modifying the input.
    Export {
        input: PathBuf,
        #[arg(long, value_enum)]
        format: MeetingFormat,
    },
    /// Return a copy with one speaker label changed.
    Rename {
        input: PathBuf,
        #[arg(long)]
        speaker: String,
        #[arg(long)]
        label: String,
    },
    /// Return a copy with one speaker merged into another.
    Merge {
        input: PathBuf,
        #[arg(long = "from")]
        from_id: String,
        #[arg(long = "into")]
        into_id: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum MeetingFormat {
    Plain,
    Markdown,
    Json,
    Srt,
    Vtt,
}

impl From<MeetingFormat> for MeetingExportFormat {
    fn from(format: MeetingFormat) -> Self {
        match format {
            MeetingFormat::Plain => Self::Plain,
            MeetingFormat::Markdown => Self::Markdown,
            MeetingFormat::Json => Self::Json,
            MeetingFormat::Srt => Self::Srt,
            MeetingFormat::Vtt => Self::Vtt,
        }
    }
}

pub fn run(args: MeetingArgs) -> Result<(), DictationError> {
    let (input, operation) = match args.action {
        MeetingAction::Inspect { input } => (input, Operation::Inspect),
        MeetingAction::Export { input, format } => (input, Operation::Export(format)),
        MeetingAction::Rename {
            input,
            speaker,
            label,
        } => (input, Operation::Rename { speaker, label }),
        MeetingAction::Merge {
            input,
            from_id,
            into_id,
        } => (input, Operation::Merge { from_id, into_id }),
    };
    let document = read_document(&input)?;
    let output = apply_operation(&document, operation)?;
    write_stdout(&output)
}

#[derive(Debug)]
enum Operation {
    Inspect,
    Export(MeetingFormat),
    Rename { speaker: String, label: String },
    Merge { from_id: String, into_id: String },
}

fn read_document(path: &Path) -> Result<MeetingTranscript, DictationError> {
    let bytes = read_input(path)?;
    serde_json::from_slice(&bytes).map_err(|_| {
        DictationError::FileDecodeError(
            "meeting input is invalid JSON or fails meeting schema validation".to_string(),
        )
    })
}

fn read_input(path: &Path) -> Result<Vec<u8>, DictationError> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| io_error("meeting input could not be inspected", error))?;
    if !metadata.is_file() {
        return Err(DictationError::FileDecodeError("meeting input must be a regular file".into()));
    }
    let file =
        File::open(path).map_err(|error| io_error("meeting input could not be opened", error))?;
    let metadata = file.metadata()
        .map_err(|error| io_error("opened meeting input could not be inspected", error))?;
    if !metadata.is_file() {
        return Err(DictationError::FileDecodeError("meeting input must be a regular file".into()));
    }
    if metadata.len() > MAX_INPUT_BYTES as u64 {
        return Err(DictationError::FileDecodeError(
            "meeting input exceeds the 24 MiB limit".to_string(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("meeting input could not be read", error))?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(DictationError::FileDecodeError(
            "meeting input exceeds the 24 MiB limit".to_string(),
        ));
    }
    Ok(bytes)
}

fn apply_operation(
    document: &MeetingTranscript,
    operation: Operation,
) -> Result<String, DictationError> {
    match operation {
        Operation::Inspect => serialize_document(document),
        Operation::Export(format) => document
            .export(format.into())
            .map_err(|_| transform_error()),
        Operation::Rename { speaker, label } => document
            .rename_speaker(&speaker, label)
            .map_err(|_| transform_error())
            .and_then(|next| serialize_document(&next)),
        Operation::Merge { from_id, into_id } => document
            .merge_speakers(&from_id, &into_id)
            .map_err(|_| transform_error())
            .and_then(|next| serialize_document(&next)),
    }
}

fn serialize_document(document: &MeetingTranscript) -> Result<String, DictationError> {
    serde_json::to_string(document).map_err(|_| transform_error())
}

fn transform_error() -> DictationError {
    DictationError::TranscriptionFailed(
        "meeting operation rejected by validated document contract".to_string(),
    )
}

fn write_stdout(output: &str) -> Result<(), DictationError> {
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(output.as_bytes())
        .and_then(|_| stdout.write_all(b"\n"))
        .map_err(|error| io_error("meeting output could not be written", error))
}

fn io_error(action: &str, error: io::Error) -> DictationError {
    let context = error
        .to_string()
        .chars()
        .take(MAX_IO_CONTEXT_CHARS)
        .collect::<String>();
    DictationError::FileDecodeError(format!("{action}: {context}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use sagascript_core::meeting::{MeetingSegmentInput, MeetingSpeaker};

    use crate::Cli;

    #[test]
    fn meeting_input_requires_a_regular_file_before_reading() {
        let directory = std::env::temp_dir().join(format!("sagascript-meeting-input-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let result = read_input(&directory);
        std::fs::remove_dir(&directory).unwrap();
        assert!(result.unwrap_err().to_string().contains("regular file"));
    }

    fn document() -> MeetingTranscript {
        MeetingTranscript::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "en",
            "model",
            4.0,
            vec![MeetingSegmentInput {
                start: 0.0,
                end: 1.0,
                text: "hello".into(),
                speaker: "speaker-1".into(),
            }],
            vec![MeetingSpeaker {
                id: "speaker-1".into(),
                label: "Chair".into(),
            }],
        )
        .expect("fixture is valid")
    }

    #[test]
    fn parses_all_meeting_commands_and_formats() {
        let inspect = Cli::try_parse_from(["sagascript", "meeting", "inspect", "input.json"])
            .expect("inspect parses");
        assert!(matches!(inspect.command, Some(crate::Command::Meeting(_))));

        for format in ["plain", "markdown", "json", "srt", "vtt"] {
            let parsed = Cli::try_parse_from([
                "sagascript",
                "meeting",
                "export",
                "input.json",
                "--format",
                format,
            ])
            .expect("format parses");
            assert!(matches!(parsed.command, Some(crate::Command::Meeting(_))));
        }

        Cli::try_parse_from([
            "sagascript",
            "meeting",
            "rename",
            "input.json",
            "--speaker",
            "speaker-1",
            "--label",
            "Chair",
        ])
        .expect("rename parses");
        Cli::try_parse_from([
            "sagascript",
            "meeting",
            "merge",
            "input.json",
            "--from",
            "speaker-2",
            "--into",
            "speaker-1",
        ])
        .expect("merge parses");
    }

    #[test]
    fn rejects_invalid_format_and_unknown_output_option() {
        assert!(Cli::try_parse_from([
            "sagascript",
            "meeting",
            "export",
            "input.json",
            "--format",
            "html",
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "sagascript",
            "meeting",
            "inspect",
            "input.json",
            "--output",
            "out.json",
        ])
        .is_err());
    }

    #[test]
    fn synthetic_export_matches_core_export_exactly() {
        let document = document();
        for format in [
            MeetingFormat::Plain,
            MeetingFormat::Markdown,
            MeetingFormat::Json,
            MeetingFormat::Srt,
            MeetingFormat::Vtt,
        ] {
            let cli_output = apply_operation(&document, Operation::Export(format)).unwrap();
            let core_output = document.export(format.into()).unwrap();
            assert_eq!(cli_output, core_output);
        }
    }

    #[test]
    fn malformed_document_is_content_free_and_oversized_input_is_rejected() {
        let malformed_path = std::env::temp_dir().join(format!(
            "sagascript-meeting-malformed-{}.json",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&malformed_path, b"{\"secret\":\"do-not-echo\"}").expect("fixture write");
        let error = read_document(&malformed_path).unwrap_err();
        let _ = std::fs::remove_file(&malformed_path);
        assert!(error.to_string().contains("invalid JSON"));
        assert!(!error.to_string().contains("do-not-echo"));

        let path =
            std::env::temp_dir().join(format!("sagascript-meeting-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&path, vec![b'x'; MAX_INPUT_BYTES + 1]).expect("fixture write");
        let error = read_input(&path).unwrap_err();
        let _ = std::fs::remove_file(&path);
        assert!(error.to_string().contains("24 MiB limit"));
    }

    #[test]
    fn rename_and_merge_do_not_modify_input_document() {
        let document = document();
        let original = document.clone();
        let renamed = apply_operation(
            &document,
            Operation::Rename {
                speaker: "speaker-1".into(),
                label: "New Chair".into(),
            },
        )
        .unwrap();
        assert!(renamed.contains("New Chair"));
        assert_eq!(document, original);

        let merge_error = apply_operation(
            &document,
            Operation::Merge {
                from_id: "missing".into(),
                into_id: "speaker-1".into(),
            },
        )
        .unwrap_err();
        assert!(merge_error
            .to_string()
            .contains("meeting operation rejected"));
        assert_eq!(document, original);
    }
}
