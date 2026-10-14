//! Explicit CLI operations for selective meeting reprocessing proposals.
//!
//! Proposal files are deliberately separate from active review files.  This
//! module only reads validated JSON and publishes user-selected outputs; it
//! does not infer, select, or persist any work plan.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use sagascript_core::error::DictationError;
use sagascript_core::meeting::MeetingTranscript;
use sagascript_core::meeting_reprocess_proposal::{
    MeetingReprocessingProposal, ProposalError, MAX_SERIALIZED_PROPOSAL_BYTES,
};
use sagascript_core::meeting_review::{CorrectionOperation, MeetingReview};
use serde::{de::DeserializeOwned, Serialize};

use crate::output::Out;

const MAX_REVIEW_BYTES: usize = 24 * 1024 * 1024;
const MAX_TRANSCRIPT_BYTES: usize = 24 * 1024 * 1024;
const MAX_OPERATIONS_BYTES: usize = 24 * 1024 * 1024;
const MAX_IO_CONTEXT_CHARS: usize = 160;

#[derive(Args, Debug)]
pub struct ProposalArgs {
    #[command(subcommand)]
    pub action: ProposalAction,
}

#[derive(Subcommand, Debug)]
pub enum ProposalAction {
    /// Create an unresolved proposal from an existing review and transcript.
    New {
        previous: PathBuf,
        #[arg(long)]
        proposed: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Inspect a saved proposal as JSON.
    Inspect { input: PathBuf },
    /// Preview automatic and unresolved correction migration as JSON.
    Preview { input: PathBuf },
    /// Replace one correction disposition with explicit operations.
    Resolve {
        input: PathBuf,
        #[arg(long = "expected-revision")]
        expected_revision: String,
        #[arg(long)]
        index: usize,
        #[arg(long)]
        operations: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Accept a fully resolved proposal against the current active review.
    Accept {
        input: PathBuf,
        #[arg(long = "current-review")]
        current_review: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
}

pub fn run(args: ProposalArgs, out: &Out) -> Result<(), DictationError> {
    match args.action {
        ProposalAction::New {
            previous,
            proposed,
            output,
        } => {
            let previous: MeetingReview = read_json(&previous, MAX_REVIEW_BYTES, "review")?;
            let proposed: MeetingTranscript =
                read_json(&proposed, MAX_TRANSCRIPT_BYTES, "transcript")?;
            let proposal = MeetingReprocessingProposal::new(previous, proposed)
                .map_err(proposal_transform_error)?;
            write_json_new(&output, &proposal)
        }
        ProposalAction::Inspect { input } => {
            let proposal: MeetingReprocessingProposal =
                read_json(&input, MAX_SERIALIZED_PROPOSAL_BYTES, "proposal")?;
            write_stdout_json(&proposal, out)
        }
        ProposalAction::Preview { input } => {
            let proposal: MeetingReprocessingProposal =
                read_json(&input, MAX_SERIALIZED_PROPOSAL_BYTES, "proposal")?;
            let preview = proposal.preview().map_err(proposal_transform_error)?;
            write_stdout_json(&preview, out)
        }
        ProposalAction::Resolve {
            input,
            expected_revision,
            index,
            operations,
            output,
        } => {
            let proposal: MeetingReprocessingProposal =
                read_json(&input, MAX_SERIALIZED_PROPOSAL_BYTES, "proposal")?;
            let operations: Vec<CorrectionOperation> =
                read_json(&operations, MAX_OPERATIONS_BYTES, "operations")?;
            let next = proposal
                .resolve(&expected_revision, index, operations)
                .map_err(proposal_transform_error)?;
            write_json_new(&output, &next)
        }
        ProposalAction::Accept {
            input,
            current_review,
            output,
        } => {
            let proposal: MeetingReprocessingProposal =
                read_json(&input, MAX_SERIALIZED_PROPOSAL_BYTES, "proposal")?;
            // Load the active review immediately before checking acceptance.  A
            // caller must choose this path explicitly; the proposal's old
            // review is never treated as the current revision implicitly.
            let current: MeetingReview =
                read_json(&current_review, MAX_REVIEW_BYTES, "current review")?;
            let accepted = proposal
                .accept(&current.revision)
                .map_err(proposal_transform_error)?;
            write_json_new(&output, &accepted)
        }
    }
}

pub(crate) fn read_json<T: DeserializeOwned>(
    path: &Path,
    limit: usize,
    kind: &str,
) -> Result<T, DictationError> {
    let bytes = read_bounded(path, limit, kind)?;
    serde_json::from_slice(&bytes).map_err(|_| {
        DictationError::FileDecodeError(format!(
            "meeting {kind} input is invalid JSON or fails its schema validation"
        ))
    })
}

fn read_bounded(path: &Path, limit: usize, kind: &str) -> Result<Vec<u8>, DictationError> {
    let metadata = fs::metadata(path)
        .map_err(|error| io_error("meeting input could not be inspected", error))?;
    if !metadata.is_file() {
        return Err(DictationError::FileDecodeError(format!(
            "meeting {kind} input must be a regular file"
        )));
    }
    let file =
        File::open(path).map_err(|error| io_error("meeting input could not be opened", error))?;
    let metadata = file
        .metadata()
        .map_err(|error| io_error("opened meeting input could not be inspected", error))?;
    if !metadata.is_file() {
        return Err(DictationError::FileDecodeError(format!(
            "meeting {kind} input must be a regular file"
        )));
    }
    if metadata.len() > limit as u64 {
        return Err(DictationError::FileDecodeError(format!(
            "meeting {kind} input exceeds its size limit"
        )));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("meeting input could not be read", error))?;
    if bytes.len() > limit {
        return Err(DictationError::FileDecodeError(format!(
            "meeting {kind} input exceeds its size limit"
        )));
    }
    Ok(bytes)
}

pub(crate) fn write_json_new<T: Serialize>(path: &Path, value: &T) -> Result<(), DictationError> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| serialization_error())?;
    bytes.push(b'\n');
    write_atomic_new(path, &bytes)
}

/// Read a validated proposal selected by a caller such as the desktop UI.
pub fn read_proposal(path: &Path) -> Result<MeetingReprocessingProposal, DictationError> {
    read_json(path, MAX_SERIALIZED_PROPOSAL_BYTES, "proposal")
}

fn write_atomic_new(path: &Path, bytes: &[u8]) -> Result<(), DictationError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent_metadata = fs::metadata(parent)
        .map_err(|error| io_error("meeting output folder could not be inspected", error))?;
    if !parent_metadata.is_dir() {
        return Err(DictationError::FileDecodeError(
            "meeting output folder must be a directory".into(),
        ));
    }

    let temporary = parent.join(format!(
        ".sagascript-meeting-proposal-{}.tmp",
        uuid::Uuid::new_v4()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options
        .open(&temporary)
        .map_err(|error| io_error("meeting output could not be created", error))?;
    let result = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::hard_link(&temporary, path));
    drop(file);
    let cleanup = fs::remove_file(&temporary);
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(DictationError::FileDecodeError(
                "meeting output already exists; no existing file was changed".into(),
            ))
        }
        (Err(error), Ok(())) => Err(io_error("meeting output could not be published", error)),
        (Ok(()), Err(error)) => Err(io_error(
            "meeting output was saved but its private temporary file could not be removed",
            error,
        )),
        (Err(error), Err(cleanup_error)) => Err(io_error(
            "meeting output could not be published or its private temporary file removed",
            io::Error::new(
                error.kind(),
                format!("{}; cleanup: {}", error, cleanup_error),
            ),
        )),
    }
}

fn write_stdout_json<T: Serialize>(value: &T, out: &Out) -> Result<(), DictationError> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| serialization_error())?;
    bytes.push(b'\n');
    Ok(out.bytes(&bytes)?)
}

fn proposal_transform_error(_error: ProposalError) -> DictationError {
    DictationError::TranscriptionFailed(
        "meeting proposal operation rejected by validated proposal contract".into(),
    )
}

fn serialization_error() -> DictationError {
    DictationError::TranscriptionFailed(
        "meeting proposal could not be serialized by its validated contract".into(),
    )
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
    use sagascript_core::meeting_review::{CorrectionFile, MeetingReview, REVIEW_SCHEMA_VERSION};
    use sagascript_core::test_support::meeting_transcript;
    use sagascript_testsupport::TempDir;
    use serde_json::json;

    fn transcript(source: &str, first_text: &str) -> MeetingTranscript {
        meeting_transcript()
            .source_sha256(source)
            .duration(2.0)
            .segment(0.0, 1.0, first_text, "speaker-a")
            .segment(1.0, 2.0, "world", "speaker-a")
            .speaker("speaker-a", "Alice")
            .build()
    }

    fn edit(segment_id: &str, text: &str) -> CorrectionOperation {
        CorrectionOperation::EditSegment {
            segment_id: segment_id.into(),
            text: Some(text.into()),
            speaker_id: None,
        }
    }

    fn previous_review() -> MeetingReview {
        let review = MeetingReview::new(transcript(&"a".repeat(64), "hello")).expect("review");
        review
            .apply_corrections(&CorrectionFile {
                schema_version: REVIEW_SCHEMA_VERSION,
                source_sha256: review.original.source_sha256.clone(),
                original_revision: review.original_revision.clone(),
                expected_revision: review.revision.clone(),
                operations: vec![edit("seg-000001", "old correction")],
            })
            .expect("correction")
    }

    fn write_fixture<T: Serialize>(path: &Path, value: &T) {
        fs::write(path, serde_json::to_vec(value).expect("json")).expect("fixture");
    }

    #[test]
    fn proposal_roundtrip_resolve_preview_and_accept() {
        let temp = TempDir::new();
        let previous_path = temp.join("previous.json");
        let proposed_path = temp.join("proposed.json");
        let proposal_path = temp.join("proposal.json");
        let resolved_path = temp.join("resolved.json");
        let current_path = temp.join("current.json");
        let accepted_path = temp.join("accepted.json");
        let operations_path = temp.join("operations.json");

        let previous = previous_review();
        let proposed = transcript(&"a".repeat(64), "hello");
        write_fixture(&previous_path, &previous);
        write_fixture(&proposed_path, &proposed);
        run(
            ProposalArgs {
                action: ProposalAction::New {
                    previous: previous_path,
                    proposed: proposed_path,
                    output: proposal_path.clone(),
                },
            },
            &Out::capture().0,
        )
        .expect("new");
        let proposal: MeetingReprocessingProposal =
            read_json(&proposal_path, MAX_SERIALIZED_PROPOSAL_BYTES, "proposal").expect("proposal");
        assert_eq!(proposal.resolutions, vec![None]);

        let operations = vec![edit("seg-000001", "new correction")];
        write_fixture(&operations_path, &operations);
        run(
            ProposalArgs {
                action: ProposalAction::Resolve {
                    input: proposal_path,
                    expected_revision: proposal.revision.clone(),
                    index: 0,
                    operations: operations_path,
                    output: resolved_path.clone(),
                },
            },
            &Out::capture().0,
        )
        .expect("resolve");
        let resolved: MeetingReprocessingProposal =
            read_json(&resolved_path, MAX_SERIALIZED_PROPOSAL_BYTES, "proposal").expect("resolved");
        let preview = resolved.preview().expect("preview");
        assert!(preview.steps.iter().all(
            |step| step.status == sagascript_core::meeting_reprocess::MigrationStatus::Applied
        ));

        write_fixture(&current_path, &previous);
        run(
            ProposalArgs {
                action: ProposalAction::Accept {
                    input: resolved_path,
                    current_review: current_path.clone(),
                    output: accepted_path.clone(),
                },
            },
            &Out::capture().0,
        )
        .expect("accept");
        let accepted: MeetingReview =
            read_json(&accepted_path, MAX_REVIEW_BYTES, "review").expect("accepted");
        assert_eq!(accepted.batches.len(), 1);
        assert_eq!(
            accepted.materialize().expect("materialize").segments[0].text,
            "new correction"
        );
    }

    #[test]
    fn conflict_and_stale_current_review_leave_destination_absent() {
        let temp = TempDir::new();
        let previous = previous_review();
        let proposal = MeetingReprocessingProposal::new(
            previous.clone(),
            transcript(&"b".repeat(64), "hello"),
        )
        .expect("proposal");
        let proposal_path = temp.join("proposal.json");
        let current_path = temp.join("current.json");
        let output_path = temp.join("accepted.json");
        write_fixture(&proposal_path, &proposal);
        write_fixture(&current_path, &previous);
        let error = run(
            ProposalArgs {
                action: ProposalAction::Accept {
                    input: proposal_path,
                    current_review: current_path.clone(),
                    output: output_path.clone(),
                },
            },
            &Out::capture().0,
        )
        .expect_err("unresolved conflict");
        assert!(error.to_string().contains("proposal operation rejected"));
        assert!(!output_path.exists());

        let changed = previous
            .apply_corrections(&CorrectionFile {
                schema_version: REVIEW_SCHEMA_VERSION,
                source_sha256: previous.original.source_sha256.clone(),
                original_revision: previous.original_revision.clone(),
                expected_revision: previous.revision.clone(),
                operations: vec![edit("seg-000002", "concurrent")],
            })
            .expect("concurrent review");

        let resolved = proposal
            .resolve(&proposal.revision, 0, vec![edit("seg-000001", "explicit")])
            .expect("explicit resolution");
        let resolved_path = temp.join("resolved.json");
        write_fixture(&resolved_path, &resolved);
        write_fixture(&current_path, &changed);
        let error = run(
            ProposalArgs {
                action: ProposalAction::Accept {
                    input: resolved_path,
                    current_review: current_path,
                    output: output_path.clone(),
                },
            },
            &Out::capture().0,
        )
        .expect_err("stale current review");
        assert!(error.to_string().contains("proposal operation rejected"));
        assert!(!output_path.exists());
    }

    #[test]
    fn existing_destination_and_hardlink_alias_are_unchanged() {
        let temp = TempDir::new();
        let destination = temp.join("destination.json");
        fs::write(&destination, b"keep me").expect("destination");
        let error = write_atomic_new(&destination, b"replace me").expect_err("existing");
        assert!(error.to_string().contains("already exists"));
        assert_eq!(fs::read(&destination).expect("read"), b"keep me");

        let source = temp.join("source.json");
        let alias = temp.join("alias.json");
        fs::write(&source, b"source").expect("source");
        fs::hard_link(&source, &alias).expect("hardlink");
        let error = write_atomic_new(&alias, b"new").expect_err("hardlink alias");
        assert!(error.to_string().contains("already exists"));
        assert_eq!(fs::read(&source).expect("source preserved"), b"source");
        assert_eq!(fs::read(&alias).expect("alias preserved"), b"source");
    }

    #[test]
    fn io_failure_preserves_input_and_successful_output_is_private() {
        let temp = TempDir::new();
        let input = temp.join("input.json");
        let output = temp.join("missing").join("proposal.json");
        fs::write(&input, b"unchanged").expect("input");
        let error = write_atomic_new(&output, b"output").expect_err("missing parent");
        assert!(error.to_string().contains("output folder"));
        assert_eq!(fs::read(&input).expect("input preserved"), b"unchanged");

        let saved = temp.join("saved.json");
        write_json_new(&saved, &json!({"ok": true})).expect("saved");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&saved).expect("metadata").permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
