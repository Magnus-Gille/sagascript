//! The last step of a diarized run (#343): the merged words as the `--json`
//! object or the shared meeting document.

use std::collections::HashSet;
use std::time::Instant;

use sagascript_core::diarization::merge::consolidate;
use sagascript_core::diarization::DiarizedSegment;
use sagascript_core::diarization_report::DiarizationReport;
use sagascript_core::error::DictationError;
use sagascript_core::meeting::MeetingTranscript;
use sagascript_core::settings::Language;
use sagascript_core::speaker_hint::{SpeakerCountHint, SpeakerHintOutcome};
use sagascript_core::transcription::diagnostics::{analyze_coverage_profile, CoverageProfile};
use sagascript_core::transcription::{
    normalize_nonspeech_markers, Glossary, GlossaryCorrection, TranscriptSegment,
};

use super::super::diarize::DiarizationPerformance;
use super::super::inference::LanguageEvidence;
use super::super::meeting::{
    meeting_checkpoint, meeting_transcript_from_plain_segments, MeetingPhase,
};
use super::super::output::{
    assemble_diarized_plain_text, combined_warnings, emit_warnings,
    prepare_diarized_plain_segments, FileTranscription,
};
use super::super::plan::{DiarizationFlags, FileRequest};
use super::super::progress::Progress;
use super::super::resolve::model_id_string;
use super::speakers::Merged;
use crate::output::{errln, Out};

/// What [`assemble_result`] needs.
pub(super) struct AssembleRequest<'a> {
    pub(super) request: &'a FileRequest<'a>,
    pub(super) flags: &'a DiarizationFlags,
    pub(super) source_sha256: String,
    pub(super) duration: f64,
    pub(super) coverage_profile: &'a CoverageProfile,
    pub(super) language_evidence: &'a LanguageEvidence,
    pub(super) coverage_segments: &'a [TranscriptSegment],
    pub(super) speaker_hint: Option<SpeakerCountHint>,
    pub(super) hint_outcome: Option<SpeakerHintOutcome>,
    pub(super) merged: Merged,
    pub(super) progress: &'a Progress,
    pub(super) started: Instant,
}

/// The diarized words as text: what the plain output and the meeting document
/// are made of, and what the `--json` object lists.
struct DiarizedText {
    /// Without no-speech markers, consolidated, glossary applied: the plain
    /// output and the meeting document.
    plain_segments: Vec<DiarizedSegment>,
    /// Consolidated per speaker, glossary applied: `segments` of `--json`.
    segments: Vec<DiarizedSegment>,
    glossary_corrections: Vec<GlossaryCorrection>,
    /// The speakers, in the order they first speak.
    speakers: Vec<String>,
    plain: String,
}

/// Shapes the merged words into text. No output, no clock: the same input
/// gives the same text.
fn shape_text(
    diarized: &[DiarizedSegment],
    language: Language,
    glossary: &Glossary,
) -> DiarizedText {
    let plain_segments = prepare_diarized_plain_segments(diarized, language, glossary);
    let mut segments = consolidate(diarized);
    for segment in &mut segments {
        segment.text = normalize_nonspeech_markers(&segment.text, language);
    }
    let fragments = segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>();
    let (corrected_fragments, projected_corrections) = glossary.correct_fragments(&fragments);
    for (segment, text) in segments.iter_mut().zip(corrected_fragments) {
        segment.text = text;
    }
    let glossary_corrections = projected_corrections
        .into_iter()
        .map(|(_, correction)| correction)
        .collect::<Vec<_>>();
    let speakers: Vec<String> = {
        let mut seen = HashSet::new();
        segments
            .iter()
            .map(|segment| segment.speaker.clone())
            .filter(|speaker| seen.insert(speaker.clone()))
            .collect()
    };
    let plain = assemble_diarized_plain_text(&plain_segments);
    DiarizedText {
        plain_segments,
        segments,
        glossary_corrections,
        speakers,
        plain,
    }
}

/// Assembles the result: the words as text and the `--json` object, or, for
/// `--meeting-json`, the shared meeting document in its place.
pub(super) fn assemble_result(
    step: AssembleRequest<'_>,
    mut performance: DiarizationPerformance,
    out: &Out,
) -> Result<FileTranscription, DictationError> {
    let AssembleRequest {
        request,
        flags,
        source_sha256,
        duration,
        coverage_profile,
        language_evidence,
        coverage_segments,
        speaker_hint,
        hint_outcome,
        merged:
            Merged {
                diarized,
                report,
                started: merge_diagnostics_started,
            },
        progress,
        started,
    } = step;
    let FileRequest {
        file,
        language,
        model,
        glossary,
        control,
        ..
    } = *request;
    let text = shape_text(&diarized, language, glossary);
    let coverage = analyze_coverage_profile(coverage_profile, coverage_segments);
    let mut warnings = combined_warnings(
        &coverage,
        language,
        language_evidence.detected_language.as_ref(),
    );
    if let Some(diagnostics) = &language_evidence.language_regions {
        warnings.extend(diagnostics.warnings.clone());
    }
    emit_warnings(out, &warnings);

    performance.merge_diagnostics_seconds = merge_diagnostics_started.elapsed().as_secs_f64();
    let json_assembly_started = Instant::now();
    let mut json = serde_json::json!({
        "segments": text.segments,
        "speakers": text.speakers,
        "language": language,
        "model": model_id_string(model),
        "file": file.display().to_string(),
        "duration_seconds": duration,
        "coverage_ratio": coverage.coverage_ratio,
        "uncovered_spans": coverage.uncovered_spans,
        "detected_language": language_evidence.detected_language,
        "language_redetection_enabled": language_evidence.language_regions.is_some(),
        "language_regions": language_evidence.language_regions.as_ref().map(|diagnostics| &diagnostics.regions),
        "warnings": warnings,
        "vocabulary_corrections": text.glossary_corrections,
        "diarization": report,
    });
    performance.json_assembly_seconds = json_assembly_started.elapsed().as_secs_f64();
    performance.total_seconds = started.elapsed().as_secs_f64();
    json["performance"] =
        serde_json::to_value(&performance).expect("diarization performance serializes");
    errln!(
        out,
        "Diarized transcription: {:.2}s total ({:.2}x realtime), cache_hit={}",
        performance.total_seconds,
        duration / performance.total_seconds.max(f64::EPSILON),
        performance.cache_hit
    );
    if flags.meeting_json {
        meeting_checkpoint(control, MeetingPhase::Finalizing)?;
        let meeting = build_meeting_document(
            source_sha256,
            request,
            duration,
            &text.plain_segments,
            speaker_hint,
            hint_outcome,
            report,
        )?;
        let json = serde_json::to_value(&meeting).map_err(|_| {
            DictationError::TranscriptionFailed(
                "Diarized meeting transcript serialization failed".to_string(),
            )
        })?;
        meeting_checkpoint(control, MeetingPhase::Finalizing)?;
        progress.meeting_phase("completed");
        return Ok(FileTranscription {
            json,
            plain: text.plain,
            meeting: Some(meeting),
        });
    }
    progress.meeting_phase("completed");
    Ok(FileTranscription {
        json,
        plain: text.plain,
        meeting: None,
    })
}

/// The shared meeting document of a diarized run: the plain segments, the
/// speaker hint and how it came out, and the diarization report.
fn build_meeting_document(
    source_sha256: String,
    request: &FileRequest<'_>,
    duration: f64,
    plain_segments: &[DiarizedSegment],
    speaker_hint: Option<SpeakerCountHint>,
    hint_outcome: Option<SpeakerHintOutcome>,
    report: DiarizationReport,
) -> Result<MeetingTranscript, DictationError> {
    let FileRequest {
        language, model, ..
    } = *request;
    meeting_transcript_from_plain_segments(
        source_sha256,
        language,
        model,
        duration,
        plain_segments,
    )?
    .with_speaker_hint(speaker_hint)
    .with_speaker_hint_outcome(hint_outcome)
    .with_diarization(report)
    .map_err(|error| DictationError::DiarizationError(error.to_string()))
}
