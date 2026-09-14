//! Read-only CLI operations for validated meeting transcript documents.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use sagascript_core::error::DictationError;
use sagascript_core::meeting::{MeetingExportFormat, MeetingTranscript};
use sagascript_core::meeting_media::MeetingAudio;
use sagascript_core::meeting_review::{CorrectionFile, MeetingReview, MeetingReviewError};

const MAX_INPUT_BYTES: usize = 24 * 1024 * 1024;
const MAX_IO_CONTEXT_CHARS: usize = 160;

#[derive(Args, Debug)]
pub struct MeetingArgs {
    #[command(subcommand)]
    pub action: MeetingAction,
}

#[derive(Subcommand, Debug)]
pub enum MeetingAction {
    /// Plan and explicitly execute selective meeting reprocessing.
    #[cfg(feature = "diarization")]
    Reprocess(Box<crate::meeting_reprocessing_cli::ReprocessingArgs>),
    /// Preserve and explicitly migrate corrections to a new machine transcript.
    Proposal(crate::meeting_proposal::ProposalArgs),
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
    /// Create or update an explicit, machine-readable meeting review.
    Review {
        #[command(subcommand)]
        action: MeetingReviewAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum MeetingReviewAction {
    /// Create a review envelope from a machine transcript.
    New { input: PathBuf },
    /// Inspect a saved review envelope as JSON.
    Inspect { input: PathBuf },
    /// Apply one explicit correction batch and emit the new review.
    Apply {
        input: PathBuf,
        #[arg(long)]
        corrections: PathBuf,
    },
    /// Remove the latest correction batch if the revision still matches.
    Undo {
        input: PathBuf,
        #[arg(long = "expected-revision")]
        expected_revision: String,
    },
    /// Restore the immutable machine transcript if the revision still matches.
    Reset {
        input: PathBuf,
        #[arg(long = "expected-revision")]
        expected_revision: String,
    },
    /// Export the materialized review without modifying the input.
    Export {
        input: PathBuf,
        #[arg(long, value_enum)]
        format: MeetingFormat,
    },
    /// Inspect explicitly selected local audio bound to the review source.
    AudioInfo {
        input: PathBuf,
        #[arg(long)]
        audio: PathBuf,
    },
    /// Read a bounded raw byte range from explicitly selected local audio.
    AudioRange {
        input: PathBuf,
        #[arg(long)]
        audio: PathBuf,
        #[arg(long, default_value_t = 0)]
        start: u64,
        #[arg(long)]
        length: u64,
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
    match args.action {
        #[cfg(feature = "diarization")]
        MeetingAction::Reprocess(args) => crate::meeting_reprocessing_cli::run(*args),
        MeetingAction::Proposal(args) => crate::meeting_proposal::run(args),
        MeetingAction::Review {
            action:
                MeetingReviewAction::AudioRange {
                    input,
                    audio,
                    start,
                    length,
                },
        } => {
            let output = run_audio_range(&input, &audio, start, length)?;
            write_bytes(&output)
        }
        action => {
            let output = execute(MeetingArgs { action })?;
            write_stdout(&output)
        }
    }
}

fn execute(args: MeetingArgs) -> Result<String, DictationError> {
    let operation = match args.action {
        #[cfg(feature = "diarization")]
        MeetingAction::Reprocess(_) => {
            return Err(DictationError::SettingsError("reprocessing requires its explicit plan/execute command path".into()));
        }
        MeetingAction::Proposal(_) => {
            return Err(DictationError::SettingsError(
                "meeting proposal requires its explicit document command path".into(),
            ));
        }
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
        MeetingAction::Review { action } => return run_review(action),
    };
    let (input, operation) = operation;
    let document = read_document(&input)?;
    apply_operation(&document, operation)
}

fn run_review(action: MeetingReviewAction) -> Result<String, DictationError> {
    match action {
        MeetingReviewAction::New { input } => {
            let document = read_document(&input)?;
            let review = MeetingReview::new(document).map_err(review_transform_error)?;
            serialize_review(&review)
        }
        MeetingReviewAction::Inspect { input } => {
            let review = read_review(&input)?;
            serialize_review(&review)
        }
        MeetingReviewAction::Apply { input, corrections } => {
            let review = read_review(&input)?;
            let corrections = read_corrections(&corrections)?;
            let next = review
                .apply_corrections(&corrections)
                .map_err(review_transform_error)?;
            serialize_review(&next)
        }
        MeetingReviewAction::Undo {
            input,
            expected_revision,
        } => {
            let review = read_review(&input)?;
            let next = review
                .undo(&expected_revision)
                .map_err(review_transform_error)?;
            serialize_review(&next)
        }
        MeetingReviewAction::Reset {
            input,
            expected_revision,
        } => {
            let review = read_review(&input)?;
            let next = review
                .reset(&expected_revision)
                .map_err(review_transform_error)?;
            serialize_review(&next)
        }
        MeetingReviewAction::Export { input, format } => {
            let review = read_review(&input)?;
            review
                .export(format.into())
                .map_err(review_transform_error)
        }
        MeetingReviewAction::AudioInfo { input, audio } => run_audio_info(&input, &audio),
        MeetingReviewAction::AudioRange { .. } => Err(DictationError::TranscriptionFailed(
            "meeting audio range requires the CLI binary output path".to_string(),
        )),
    }
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

fn read_review(path: &Path) -> Result<MeetingReview, DictationError> {
    let bytes = read_input(path)?;
    serde_json::from_slice(&bytes).map_err(|_| {
        DictationError::FileDecodeError(
            "meeting review input is invalid JSON or fails review schema validation".to_string(),
        )
    })
}

fn read_corrections(path: &Path) -> Result<CorrectionFile, DictationError> {
    let bytes = read_input(path)?;
    serde_json::from_slice(&bytes).map_err(|_| {
        DictationError::FileDecodeError(
            "meeting corrections input is invalid JSON or fails correction schema validation"
                .to_string(),
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

fn serialize_review(review: &MeetingReview) -> Result<String, DictationError> {
    serde_json::to_string(review)
        .map_err(|_| review_transform_error(MeetingReviewError::Serialization))
}

fn run_audio_info(input: &Path, audio: &Path) -> Result<String, DictationError> {
    let review = read_review(input)?;
    let audio = MeetingAudio::open(audio, &review.original.source_sha256)
        .map_err(media_transform_error)?;
    serde_json::to_string(&serde_json::json!({
        "length": audio.len(),
        "mime": audio.mime(),
    }))
    .map_err(|_| media_transform_error(()))
}

fn run_audio_range(
    input: &Path,
    audio: &Path,
    start: u64,
    length: u64,
) -> Result<Vec<u8>, DictationError> {
    let review = read_review(input)?;
    let mut audio = MeetingAudio::open(audio, &review.original.source_sha256)
        .map_err(media_transform_error)?;
    audio
        .read_range(start, length)
        .map_err(media_transform_error)
}

fn transform_error() -> DictationError {
    DictationError::TranscriptionFailed(
        "meeting operation rejected by validated document contract".to_string(),
    )
}

// MeetingReviewError displays only fixed contract diagnostics (plus bounded
// schema/field names and numeric versions); it carries no paths or transcript
// text. File and JSON parsing errors stay on the content-free paths above.
fn review_transform_error(error: MeetingReviewError) -> DictationError {
    DictationError::TranscriptionFailed(format!(
        "meeting review operation rejected by validated review contract: {error}"
    ))
}

fn media_transform_error<E>(_error: E) -> DictationError {
    DictationError::TranscriptionFailed(
        "meeting audio operation rejected by validated source-bound media contract".to_string(),
    )
}

fn write_stdout(output: &str) -> Result<(), DictationError> {
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(output.as_bytes())
        .and_then(|_| stdout.write_all(b"\n"))
        .map_err(|error| io_error("meeting output could not be written", error))
}

fn write_bytes(output: &[u8]) -> Result<(), DictationError> {
    io::stdout()
        .lock()
        .write_all(output)
        .map_err(|error| io_error("meeting audio output could not be written", error))
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
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

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

        Cli::try_parse_from(["sagascript", "meeting", "review", "new", "input.json"])
            .expect("review new parses");
        Cli::try_parse_from(["sagascript", "meeting", "review", "inspect", "review.json"])
            .expect("review inspect parses");
        Cli::try_parse_from([
            "sagascript",
            "meeting",
            "review",
            "apply",
            "review.json",
            "--corrections",
            "corrections.json",
        ])
        .expect("review apply parses");
        Cli::try_parse_from([
            "sagascript",
            "meeting",
            "review",
            "undo",
            "review.json",
            "--expected-revision",
            &"a".repeat(64),
        ])
        .expect("review undo parses");
        Cli::try_parse_from([
            "sagascript",
            "meeting",
            "review",
            "reset",
            "review.json",
            "--expected-revision",
            &"a".repeat(64),
        ])
        .expect("review reset parses");
        Cli::try_parse_from([
            "sagascript",
            "meeting",
            "review",
            "export",
            "review.json",
            "--format",
            "vtt",
        ])
        .expect("review export parses");
        Cli::try_parse_from([
            "sagascript",
            "meeting",
            "review",
            "audio-info",
            "review.json",
            "--audio",
            "recording.wav",
        ])
        .expect("review audio-info parses");
        Cli::try_parse_from([
            "sagascript",
            "meeting",
            "review",
            "audio-range",
            "review.json",
            "--audio",
            "recording.wav",
            "--start",
            "4",
            "--length",
            "8",
        ])
        .expect("review audio-range parses");
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
        assert!(Cli::try_parse_from([
            "sagascript",
            "meeting",
            "review",
            "inspect",
            "review.json",
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

    fn temp_directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "sagascript-meeting-review-cli-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&directory).expect("fixture directory");
        directory
    }

    fn review_value(json: &str) -> Value {
        serde_json::from_str(json).expect("review JSON")
    }

    fn correction_value(review: &Value, operation: Value) -> Value {
        json!({
            "schema_version": 1,
            "source_sha256": review["original"]["source_sha256"],
            "original_revision": review["original_revision"],
            "expected_revision": review["revision"],
            "operations": [operation]
        })
    }

    fn write_json(path: &Path, value: &Value) {
        std::fs::write(path, serde_json::to_vec(value).expect("fixture JSON")).expect("write");
    }

    fn new_review_fixture(directory: &Path) -> (PathBuf, PathBuf, Value) {
        let transcript_path = directory.join("machine.json");
        std::fs::write(
            &transcript_path,
            serde_json::to_vec(&document()).expect("transcript JSON"),
        )
        .expect("write transcript");
        let review_path = directory.join("review.json");
        let review_json = execute(MeetingArgs {
            action: MeetingAction::Review {
                action: MeetingReviewAction::New {
                    input: transcript_path.clone(),
                },
            },
        })
        .expect("new review");
        let review = review_value(&review_json);
        std::fs::write(&review_path, review_json).expect("write review");
        (transcript_path, review_path, review)
    }

    fn synthetic_wav() -> Vec<u8> {
        let mut bytes = vec![0u8; 44];
        bytes[0..4].copy_from_slice(b"RIFF");
        bytes[8..12].copy_from_slice(b"WAVE");
        bytes.extend(0u8..32);
        bytes
    }

    fn new_audio_review_fixture(directory: &Path) -> (PathBuf, PathBuf, Vec<u8>) {
        let audio = synthetic_wav();
        let audio_path = directory.join("recording.wav");
        std::fs::write(&audio_path, &audio).expect("write audio");
        let source_sha256 = format!("{:x}", Sha256::digest(&audio));
        let transcript = MeetingTranscript::new(
            source_sha256,
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
        .expect("audio fixture is valid");
        let transcript_path = directory.join("audio-machine.json");
        std::fs::write(
            &transcript_path,
            serde_json::to_vec(&transcript).expect("transcript JSON"),
        )
        .expect("write transcript");
        let review_path = directory.join("audio-review.json");
        let review_json = execute(MeetingArgs {
            action: MeetingAction::Review {
                action: MeetingReviewAction::New {
                    input: transcript_path,
                },
            },
        })
        .expect("new audio review");
        std::fs::write(&review_path, review_json).expect("write review");
        (review_path, audio_path, audio)
    }

    #[test]
    fn review_round_trip_supports_unicode_edit_undo_and_reset() {
        let directory = temp_directory("round-trip");
        let (_transcript_path, review_path, initial) = new_review_fixture(&directory);
        let corrections_path = directory.join("corrections.json");
        write_json(
            &corrections_path,
            &correction_value(
                &initial,
                json!({
                    "kind": "edit_segment",
                    "segment_id": "seg-000001",
                    "text": "Hej, Béatrice 👋"
                }),
            ),
        );

        let applied_json = execute(MeetingArgs {
            action: MeetingAction::Review {
                action: MeetingReviewAction::Apply {
                    input: review_path.clone(),
                    corrections: corrections_path,
                },
            },
        })
        .expect("apply review");
        let applied = review_value(&applied_json);
        let applied_revision = applied["revision"].as_str().expect("revision").to_owned();
        assert_eq!(
            applied["batches"][0]["operations"][0]["text"],
            "Hej, Béatrice 👋"
        );
        std::fs::write(&review_path, &applied_json).expect("replace fixture review");

        let exported = execute(MeetingArgs {
            action: MeetingAction::Review {
                action: MeetingReviewAction::Export {
                    input: review_path.clone(),
                    format: MeetingFormat::Plain,
                },
            },
        })
        .expect("export review");
        assert!(exported.contains("Hej, Béatrice 👋"));

        let review_export = execute(MeetingArgs {
            action: MeetingAction::Review {
                action: MeetingReviewAction::Export {
                    input: review_path.clone(),
                    format: MeetingFormat::Json,
                },
            },
        })
        .expect("export review JSON");
        let review_export = review_value(&review_export);
        assert_eq!(review_export["original_revision"], applied["original_revision"]);
        assert_eq!(review_export["batches"], applied["batches"]);

        let undone_json = execute(MeetingArgs {
            action: MeetingAction::Review {
                action: MeetingReviewAction::Undo {
                    input: review_path.clone(),
                    expected_revision: applied_revision.clone(),
                },
            },
        })
        .expect("undo review");
        let undone = review_value(&undone_json);
        assert!(undone["batches"].as_array().expect("batches").is_empty());
        assert_eq!(undone["generation"], 2);
        assert_ne!(undone["revision"], initial["revision"]);
        std::fs::write(&review_path, &applied_json).expect("restore applied fixture review");

        let reset_json = execute(MeetingArgs {
            action: MeetingAction::Review {
                action: MeetingReviewAction::Reset {
                    input: review_path.clone(),
                    expected_revision: applied_revision,
                },
            },
        })
        .expect("reset review");
        let reset = review_value(&reset_json);
        assert!(reset["batches"].as_array().expect("batches").is_empty());
        assert_eq!(reset["generation"], 2);
        assert_ne!(reset["revision"], initial["revision"]);

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn review_rejects_wrong_revision_invalid_ids_and_tampering_without_writes() {
        let directory = temp_directory("fail-closed");
        let (_transcript_path, review_path, initial) = new_review_fixture(&directory);
        let original_review_bytes = std::fs::read(&review_path).expect("read review");
        let corrections_path = directory.join("invalid-corrections.json");
        let invalid = correction_value(
            &initial,
            json!({
                "kind": "edit_segment",
                "segment_id": "missing-segment",
                "text": "not applied"
            }),
        );
        write_json(&corrections_path, &invalid);
        let original_corrections_bytes =
            std::fs::read(&corrections_path).expect("read corrections");

        let error = execute(MeetingArgs {
            action: MeetingAction::Review {
                action: MeetingReviewAction::Apply {
                    input: review_path.clone(),
                    corrections: corrections_path.clone(),
                },
            },
        })
        .expect_err("invalid segment must fail");
        assert!(error.to_string().contains("unknown meeting segment"));
        assert!(!error.to_string().contains("not applied"));
        assert_eq!(
            std::fs::read(&review_path).expect("review bytes"),
            original_review_bytes
        );
        assert_eq!(
            std::fs::read(&corrections_path).expect("correction bytes"),
            original_corrections_bytes
        );

        let wrong_revision_error = execute(MeetingArgs {
            action: MeetingAction::Review {
                action: MeetingReviewAction::Undo {
                    input: review_path.clone(),
                    expected_revision: "a".repeat(64),
                },
            },
        })
        .expect_err("wrong revision must fail");
        assert!(wrong_revision_error
            .to_string()
            .contains("correction expected revision does not match this review"));
        assert_eq!(
            std::fs::read(&review_path).expect("review bytes"),
            original_review_bytes
        );

        let mut tampered_review = initial.clone();
        tampered_review["revision"] = Value::String("0".repeat(64));
        let tampered_path = directory.join("tampered.json");
        write_json(&tampered_path, &tampered_review);
        let tampered_error = execute(MeetingArgs {
            action: MeetingAction::Review {
                action: MeetingReviewAction::Inspect {
                    input: tampered_path.clone(),
                },
            },
        })
        .expect_err("tampering must fail");
        assert!(tampered_error
            .to_string()
            .contains("review schema validation"));
        assert_eq!(
            std::fs::read(&tampered_path).expect("tampered bytes"),
            serde_json::to_vec(&tampered_review).expect("tampered JSON")
        );

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn review_audio_info_and_bounded_range_require_matching_source() {
        let directory = temp_directory("audio");
        let (review_path, audio_path, audio) = new_audio_review_fixture(&directory);
        let review_bytes = std::fs::read(&review_path).expect("review bytes");
        let audio_bytes = std::fs::read(&audio_path).expect("audio bytes");

        let info_json = execute(MeetingArgs {
            action: MeetingAction::Review {
                action: MeetingReviewAction::AudioInfo {
                    input: review_path.clone(),
                    audio: audio_path.clone(),
                },
            },
        })
        .expect("audio info");
        let info = review_value(&info_json);
        assert_eq!(info["length"].as_u64(), Some(audio.len() as u64));
        assert_eq!(info["mime"], "audio/wav");

        let range = run_audio_range(&review_path, &audio_path, 4, 8).expect("audio range");
        assert_eq!(range, audio[4..12]);
        assert_eq!(std::fs::read(&review_path).expect("review bytes"), review_bytes);
        assert_eq!(std::fs::read(&audio_path).expect("audio bytes"), audio_bytes);

        let out_of_bounds = run_audio_range(&review_path, &audio_path, 70, 8)
            .expect_err("out-of-bounds range must fail");
        assert!(out_of_bounds.to_string().contains("source-bound media contract"));

        let wrong_audio_path = directory.join("wrong.wav");
        let mut wrong_audio = audio.clone();
        wrong_audio[20] ^= 1;
        std::fs::write(&wrong_audio_path, &wrong_audio).expect("write wrong audio");
        let wrong_source = run_audio_range(&review_path, &wrong_audio_path, 4, 8)
            .expect_err("wrong source must fail");
        assert!(wrong_source.to_string().contains("source-bound media contract"));
        assert_eq!(std::fs::read(&review_path).expect("review bytes"), review_bytes);
        assert_eq!(std::fs::read(&audio_path).expect("audio bytes"), audio_bytes);

        let _ = std::fs::remove_dir_all(directory);
    }
}
