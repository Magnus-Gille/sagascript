//! Explicit CLI operations for selective meeting reprocessing proposals.
//!
//! Proposal files are deliberately separate from active review files.  This
//! module only reads validated JSON and publishes user-selected outputs; it
//! does not infer, select, or persist any work plan. The bounded reading and
//! the no-replace publication are core's (`meeting::reprocess::io`, #374); this
//! module adds the clap command and the stderr warning for a staging file that
//! could not be removed.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use sagascript_core::error::DictationError;
use sagascript_core::fsutil;
use sagascript_core::meeting::reprocess::io::{self as meeting_io, json_line, read_json};
use sagascript_core::meeting::reprocess::proposal::{
    MeetingReprocessingProposal, ProposalError, MAX_SERIALIZED_PROPOSAL_BYTES,
};
use sagascript_core::meeting::review::{CorrectionOperation, MeetingReview};
use sagascript_core::meeting::MeetingTranscript;
use serde::Serialize;

use crate::output::{errln, Out};

const MAX_REVIEW_BYTES: usize = 24 * 1024 * 1024;
const MAX_TRANSCRIPT_BYTES: usize = 24 * 1024 * 1024;
const MAX_OPERATIONS_BYTES: usize = 24 * 1024 * 1024;

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
            write_json_new(&output, &proposal, out)
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
            write_json_new(&output, &next, out)
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
            write_json_new(&output, &accepted, out)
        }
    }
}

pub(crate) fn write_json_new<T: Serialize>(
    path: &Path,
    value: &T,
    out: &Out,
) -> Result<(), DictationError> {
    write_atomic_new(path, &json_line(value)?, out)
}

/// Publish `bytes` as the new file `path` ([`fsutil::publish_new`]). A private
/// staging file that cannot be removed afterwards is a warning on stderr, not a
/// failure: the output is complete.
fn write_atomic_new(path: &Path, bytes: &[u8], out: &Out) -> Result<(), DictationError> {
    write_atomic_new_with(path, bytes, &fsutil::RealFs, out)
}

fn write_atomic_new_with(
    path: &Path,
    bytes: &[u8],
    ops: &dyn fsutil::PublishFs,
    out: &Out,
) -> Result<(), DictationError> {
    let published = meeting_io::write_atomic_new_with(path, bytes, ops)?;
    if published.leftover.is_some() {
        errln!(
            out,
            "Warning: {} was published, but a private temporary file beside it (a hidden \
             .NAME.UUID.tmp with the same content) could not be removed; delete it.",
            path.display()
        );
    }
    Ok(())
}

fn write_stdout_json<T: Serialize>(value: &T, out: &Out) -> Result<(), DictationError> {
    Ok(out.bytes(&json_line(value)?)?)
}

fn proposal_transform_error(_error: ProposalError) -> DictationError {
    DictationError::TranscriptionFailed(
        "meeting proposal operation rejected by validated proposal contract".into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sagascript_core::meeting::review::{CorrectionFile, MeetingReview, REVIEW_SCHEMA_VERSION};
    use sagascript_core::test_support::meeting_transcript;
    use sagascript_testsupport::TempDir;
    use serde_json::json;
    use std::fs;

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
            |step| step.status == sagascript_core::meeting::reprocess::MigrationStatus::Applied
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
        let error =
            write_atomic_new(&destination, b"replace me", &Out::capture().0).expect_err("existing");
        assert!(error.to_string().contains("already exists"));
        assert_eq!(fs::read(&destination).expect("read"), b"keep me");

        let source = temp.join("source.json");
        let alias = temp.join("alias.json");
        fs::write(&source, b"source").expect("source");
        fs::hard_link(&source, &alias).expect("hardlink");
        let error =
            write_atomic_new(&alias, b"new", &Out::capture().0).expect_err("hardlink alias");
        assert!(error.to_string().contains("already exists"));
        assert_eq!(fs::read(&source).expect("source preserved"), b"source");
        assert_eq!(fs::read(&alias).expect("alias preserved"), b"source");
    }

    #[test]
    fn a_temporary_file_that_cannot_be_removed_after_publication_is_a_warning_not_a_failure() {
        use sagascript_core::test_support::FaultyFs;
        let temp = TempDir::new();
        let destination = temp.join("out.json");
        let (out, captured) = Out::capture();

        write_atomic_new_with(
            &destination,
            b"bytes",
            &FaultyFs::new().failing_remove(),
            &out,
        )
        .expect("the output is published, so the call succeeds");

        assert_eq!(fs::read(&destination).expect("read"), b"bytes");
        let stderr = captured.stderr();
        assert!(stderr.contains("Warning"), "{stderr}");
        assert!(stderr.contains("temporary file"), "{stderr}");
    }

    #[test]
    fn io_failure_preserves_input_and_successful_output_is_private() {
        let temp = TempDir::new();
        let input = temp.join("input.json");
        let output = temp.join("missing").join("proposal.json");
        fs::write(&input, b"unchanged").expect("input");
        let error =
            write_atomic_new(&output, b"output", &Out::capture().0).expect_err("missing parent");
        assert!(error.to_string().contains("output folder"));
        assert_eq!(fs::read(&input).expect("input preserved"), b"unchanged");

        let saved = temp.join("saved.json");
        write_json_new(&saved, &json!({"ok": true}), &Out::capture().0).expect("saved");
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
