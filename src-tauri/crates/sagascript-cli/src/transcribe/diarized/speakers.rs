//! The speaker steps of a diarized run (#343): clustering the analysis into
//! speaker segments, and merging the words into them.

use std::time::Instant;

use sagascript_core::diarization::merge::merge_with_diagnostics;
use sagascript_core::diarization::{
    cluster_with_outcome, DiarizationAnalysis, DiarizeConfig, DiarizedSegment, SpeakerSegment,
    TimestampedSegment,
};
use sagascript_core::diarization_report::DiarizationReport;
use sagascript_core::error::DictationError;
use sagascript_core::speaker_hint::SpeakerHintOutcome;
use sagascript_core::transcription::TranscriptSegment;

use super::super::meeting::{
    meeting_checkpoint, verify_meeting_source_unchanged, MeetingControl, MeetingPhase,
};
use super::super::plan::{DiarizationPlan, FileRequest};
use super::super::progress::Progress;
use crate::output::{errln, Out};

/// What [`cluster_speakers`] needs.
pub(super) struct ClusterRequest<'a> {
    pub(super) analysis: &'a DiarizationAnalysis,
    pub(super) config: &'a DiarizeConfig,
    pub(super) control: Option<&'a MeetingControl<'a>>,
    pub(super) progress: &'a Progress,
}

/// The speaker segments of an analysis, and how the speaker-count hint came out.
pub(super) struct Clustered {
    pub(super) speaker_segments: Vec<SpeakerSegment>,
    pub(super) hint_outcome: Option<SpeakerHintOutcome>,
    pub(super) seconds: f64,
}

/// Groups the analysis into speaker segments, applying the speaker-count hint.
pub(super) fn cluster_speakers(
    step: &ClusterRequest<'_>,
    out: &Out,
) -> Result<Clustered, DictationError> {
    let ClusterRequest {
        analysis,
        config,
        control,
        progress,
    } = *step;
    let clustering_started = Instant::now();
    progress.meeting_phase("clustering");
    meeting_checkpoint(control, MeetingPhase::Clustering)?;
    let (speaker_segments, hint_outcome) = cluster_with_outcome(analysis, config)?;
    meeting_checkpoint(control, MeetingPhase::Clustering)?;
    let seconds = clustering_started.elapsed().as_secs_f64();
    errln!(out, "Found {} speaker segment(s)", speaker_segments.len());
    if std::env::var("SAGA_DIAR_DEBUG").is_ok() {
        for s in &speaker_segments {
            errln!(out, "DIARSEG\t{:.3}\t{:.3}\t{}", s.start, s.end, s.speaker);
        }
    }
    Ok(Clustered {
        speaker_segments,
        hint_outcome,
        seconds,
    })
}

/// What [`merge_speakers`] needs.
pub(super) struct MergeRequest<'a> {
    pub(super) request: &'a FileRequest<'a>,
    pub(super) plan: &'a DiarizationPlan,
    pub(super) analysis: &'a DiarizationAnalysis,
    pub(super) speakers: &'a [SpeakerSegment],
    /// Word timings: `(start, end, text)`.
    pub(super) words: Vec<(f64, f64, String)>,
    pub(super) coverage_segments: &'a [TranscriptSegment],
    pub(super) hint_outcome: Option<SpeakerHintOutcome>,
    pub(super) config: &'a DiarizeConfig,
    pub(super) duration: f64,
    pub(super) progress: &'a Progress,
}

/// The words merged into the speakers, with the report that explains it.
pub(super) struct Merged {
    pub(super) diarized: Vec<DiarizedSegment>,
    pub(super) report: DiarizationReport,
    /// When the merge began: the performance report counts its diagnostics
    /// from here to the end of the shaping.
    pub(super) started: Instant,
}

/// Merges the speakers into the words and builds the diarization report,
/// checking it and the source file one last time.
pub(super) fn merge_speakers(step: MergeRequest<'_>, out: &Out) -> Result<Merged, DictationError> {
    let MergeRequest {
        request,
        plan,
        analysis,
        speakers,
        words,
        coverage_segments,
        hint_outcome,
        config,
        duration,
        progress,
    } = step;
    let FileRequest {
        args,
        file,
        control,
        ..
    } = *request;
    errln!(out, "Got {} word/segment(s) for merging", words.len());
    if std::env::var("SAGA_DIAR_DEBUG").is_ok() {
        for (st, en, tx) in &words {
            errln!(out, "WORD\t{:.3}\t{:.3}\t{}", st, en, tx.replace('\t', " "));
        }
    }

    progress.meeting_phase("finalizing");
    let started = Instant::now();
    let transcript: Vec<TimestampedSegment> = words
        .into_iter()
        .map(|(start, end, text)| TimestampedSegment { start, end, text })
        .collect();

    let (diarized, attributions) = merge_with_diagnostics(speakers, &transcript);
    let mut report = analysis.report(
        speakers,
        config,
        attributions,
        args.diarize_diagnostics,
        plan.source_sha256.clone(),
        duration,
        crate::GIT_HASH.into(),
        env!("CARGO_PKG_VERSION").into(),
    )?;
    report.decoder = Some(plan.decoder_evidence.clone());
    report.speaker_hint_outcome = hint_outcome;
    report.asr_segments = coverage_segments
        .iter()
        .filter(|_| args.diarize_diagnostics)
        .map(
            |segment| sagascript_core::diarization_report::AsrSegmentEvidence {
                start: segment.start,
                end: segment.end,
                avg_logprob: segment.avg_logprob,
                no_speech_prob: Some(segment.no_speech_prob),
            },
        )
        .collect();
    report
        .validate()
        .map_err(|error| DictationError::DiarizationError(error.to_string()))?;
    verify_meeting_source_unchanged(file, &plan.source_sha256, control)?;
    Ok(Merged {
        diarized,
        report,
        started,
    })
}
