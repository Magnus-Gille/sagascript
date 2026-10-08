//! Offline, file-only diarization reference and evaluation commands.
//!
//! This module deliberately does not open audio, load models, read settings,
//! or write files.  Core validation remains the authority for native JSON
//! documents and score calculation.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use sagascript_core::diarization_evaluation::{
    evaluate, EvaluationOptions, ScoringRegion, SpeakerTurn,
};
use sagascript_core::diarization_reference::{ReferenceDocument, ReferenceStatus};
use sagascript_core::diarization_report::DiarizationReport;
use sagascript_core::error::DictationError;
use sagascript_core::meeting::MeetingTranscript;

const MAX_INPUT_BYTES: usize = 24 * 1024 * 1024;
const MAX_LINES: usize = 1_000_000;

#[derive(Args, Debug)]
pub struct DiarizationArgs {
    #[command(subcommand)]
    pub action: DiarizationAction,
}

#[derive(Subcommand, Debug)]
pub enum DiarizationAction {
    /// Validate a native reference and report review counts.
    ReferenceValidate { input: PathBuf },
    /// Export verified native reference intervals as RTTM or UEM.
    ReferenceExport {
        input: PathBuf,
        #[arg(long, value_enum)]
        format: ReferenceFormat,
    },
    /// Validate and inspect a native diarization report.
    Inspect { report: PathBuf },
    /// Export the report's acoustic activity without transcript text.
    ExportActivity {
        report: PathBuf,
        #[arg(long, value_enum)]
        format: ActivityFormat,
    },
    /// Evaluate a native reference or RTTM against a native report/transcript.
    Evaluate {
        #[arg(long)]
        reference: PathBuf,
        #[arg(long)]
        hypothesis: PathBuf,
        #[arg(long)]
        uem: Option<PathBuf>,
        #[arg(long, default_value_t = 0.0)]
        collar: f64,
        #[arg(long, value_enum, default_value_t = EvaluationLayer::Acoustic)]
        layer: EvaluationLayer,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum ReferenceFormat {
    Rttm,
    Uem,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum ActivityFormat {
    Json,
    Rttm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EvaluationLayer {
    Acoustic,
    Transcript,
}

#[derive(Debug)]
struct InputFile {
    bytes: Vec<u8>,
    sha256: String,
}

#[derive(Debug)]
struct ParsedReference {
    turns: Vec<SpeakerTurn>,
    uem: Option<Vec<ScoringRegion>>,
    source_sha256: Option<String>,
    recording_id: Option<String>,
    native: bool,
    duration_seconds: f64,
}

#[derive(Debug)]
struct ParsedHypothesis {
    turns: Vec<SpeakerTurn>,
    source_sha256: String,
    immutable_provenance: bool,
}

#[derive(Serialize)]
struct EvaluationReceipt<'a> {
    schema_version: u8,
    source_sha256: Option<&'a str>,
    reference_file_sha256: &'a str,
    hypothesis_file_sha256: &'a str,
    uem_file_sha256: Option<&'a str>,
    build_version: &'static str,
    build_revision: &'static str,
    layer: EvaluationLayer,
    collar_seconds: f64,
    metrics: sagascript_core::diarization_evaluation::EvaluationReport,
    quality_adoption_ready: bool,
    measurement_only: bool,
    reasons: Vec<String>,
}

pub fn run(args: DiarizationArgs) -> Result<(), DictationError> {
    let output = match args.action {
        DiarizationAction::ReferenceValidate { input } => reference_validate(&input)?,
        DiarizationAction::ReferenceExport { input, format } => reference_export(&input, format)?,
        DiarizationAction::Inspect { report } => inspect_report(&report)?,
        DiarizationAction::ExportActivity { report, format } => export_activity(&report, format)?,
        DiarizationAction::Evaluate {
            reference,
            hypothesis,
            uem,
            collar,
            layer,
        } => evaluate_files(&reference, &hypothesis, uem.as_deref(), collar, layer)?,
    };
    write_stdout(&output)
}

fn reference_validate(path: &Path) -> Result<String, DictationError> {
    let input = read_input(path)?;
    let document = parse_reference_json(&input.bytes)?;
    document.validate().map_err(core_error)?;
    let counts = document
        .intervals
        .iter()
        .fold([0usize; 3], |mut counts, interval| {
            match interval.status {
                ReferenceStatus::Candidate => counts[0] += 1,
                ReferenceStatus::Verified => counts[1] += 1,
                ReferenceStatus::Unknown => counts[2] += 1,
            }
            counts
        });
    let verified_uem = document.verified_uem().map_err(core_error)?;
    json_string(&json!({
        "schemaVersion": 1,
        "status": "valid",
        "sourceSha256": document.source_sha256,
        "durationSeconds": document.duration_seconds,
        "intervals": document.intervals.len(),
        "candidateIntervals": counts[0],
        "verifiedIntervals": counts[1],
        "unknownIntervals": counts[2],
        "verifiedUemRegions": verified_uem.len(),
        "gold": false,
        "note": "Validation does not make a reference gold; verified intervals still require the caller's review policy."
    }))
}

fn reference_export(path: &Path, format: ReferenceFormat) -> Result<String, DictationError> {
    let input = read_input(path)?;
    let document = parse_reference_json(&input.bytes)?;
    document.validate().map_err(core_error)?;
    match format {
        ReferenceFormat::Rttm => document.export_rttm().map_err(core_error),
        ReferenceFormat::Uem => document.export_uem().map_err(core_error),
    }
}

fn inspect_report(path: &Path) -> Result<String, DictationError> {
    let input = read_input(path)?;
    let report = parse_report(&input.bytes)?;
    report.validate().map_err(core_error)?;
    serde_json::to_string_pretty(&report).map_err(|error| cli_error(error.to_string()))
}

fn export_activity(path: &Path, format: ActivityFormat) -> Result<String, DictationError> {
    let input = read_input(path)?;
    let report = parse_report(&input.bytes)?;
    report.validate().map_err(core_error)?;
    match format {
        ActivityFormat::Json => serde_json::to_string_pretty(&report.activity)
            .map_err(|error| cli_error(error.to_string())),
        ActivityFormat::Rttm => report.activity_rttm().map_err(core_error),
    }
}

fn evaluate_files(
    reference_path: &Path,
    hypothesis_path: &Path,
    uem_path: Option<&Path>,
    collar: f64,
    layer: EvaluationLayer,
) -> Result<String, DictationError> {
    if !collar.is_finite() || collar < 0.0 {
        return Err(cli_error("--collar must be finite and non-negative"));
    }
    let reference_file = read_input(reference_path)?;
    let hypothesis_file = read_input(hypothesis_path)?;
    let reference = parse_reference(&reference_file.bytes, uem_path.is_some())?;
    let hypothesis = parse_hypothesis(&hypothesis_file.bytes, layer)?;

    if let (Some(reference_hash), true) = (reference.source_sha256.as_deref(), reference.native) {
        if reference_hash != hypothesis.source_sha256 {
            return Err(cli_error(
                "native reference and hypothesis source_sha256 do not match",
            ));
        }
    }
    let uem_file = uem_path.map(read_input).transpose()?;
    let uem = if let Some(file) = &uem_file {
        parse_uem(
            utf8(&file.bytes)?,
            reference.recording_id.as_deref(),
            reference.duration_seconds,
        )?
    } else if let Some(uem) = reference.uem.clone() {
        uem
    } else {
        return Err(cli_error(
            "RTTM references require --uem; score only explicitly supplied speech is refused",
        ));
    };

    if uem.is_empty() {
        return Err(cli_error("scoring UEM is empty"));
    }
    let metrics = evaluate(
        &reference.turns,
        &hypothesis.turns,
        &uem,
        EvaluationOptions {
            collar_seconds: collar,
        },
    )
    .map_err(core_error)?;

    let mut reasons = Vec::new();
    let measurement_only = !reference.native || !hypothesis.immutable_provenance;
    if !reference.native {
        reasons.push(
            "RTTM reference is recording-ID bound, not cryptographically source-bound".to_owned(),
        );
    }
    if !hypothesis.immutable_provenance {
        reasons.push("hypothesis lacks immutable provenance".to_owned());
    }
    reasons.push("quality adoption integration is owned by the caller".to_owned());
    let receipt = EvaluationReceipt {
        schema_version: 1,
        source_sha256: reference
            .source_sha256
            .as_deref()
            .or(Some(hypothesis.source_sha256.as_str())),
        reference_file_sha256: &reference_file.sha256,
        hypothesis_file_sha256: &hypothesis_file.sha256,
        uem_file_sha256: uem_file.as_ref().map(|file| file.sha256.as_str()),
        build_version: env!("CARGO_PKG_VERSION"),
        build_revision: crate::GIT_HASH,
        layer,
        collar_seconds: collar,
        metrics,
        quality_adoption_ready: false,
        measurement_only,
        reasons,
    };
    serde_json::to_string_pretty(&receipt).map_err(|error| cli_error(error.to_string()))
}

fn parse_reference(bytes: &[u8], has_uem: bool) -> Result<ParsedReference, DictationError> {
    let text = utf8(bytes)?;
    if text.trim_start().starts_with('{') {
        let document: ReferenceDocument = serde_json::from_str(text).map_err(json_error)?;
        document.validate().map_err(core_error)?;
        let turns = document.verified_turns().map_err(core_error)?;
        let uem = (!has_uem)
            .then(|| document.verified_uem())
            .transpose()
            .map_err(core_error)?;
        return Ok(ParsedReference {
            turns,
            uem,
            source_sha256: Some(document.source_sha256),
            recording_id: None,
            native: true,
            duration_seconds: document.duration_seconds,
        });
    }
    let (recording_id, turns, duration_seconds) = parse_rttm(text)?;
    Ok(ParsedReference {
        turns,
        uem: None,
        source_sha256: None,
        recording_id: Some(recording_id),
        native: false,
        duration_seconds,
    })
}

fn parse_hypothesis(
    bytes: &[u8],
    layer: EvaluationLayer,
) -> Result<ParsedHypothesis, DictationError> {
    let value: Value = serde_json::from_str(utf8(bytes)?).map_err(json_error)?;
    if let Ok(report) = serde_json::from_value::<DiarizationReport>(value.clone()) {
        report.validate().map_err(core_error)?;
        if layer == EvaluationLayer::Transcript {
            return Err(cli_error(
                "a native diarization report has acoustic activity only; use --layer acoustic",
            ));
        }
        let turns = report
            .activity
            .iter()
            .map(|span| SpeakerTurn {
                start: span.start,
                end: span.end,
                speakers: span.speakers.clone(),
            })
            .collect();
        return Ok(ParsedHypothesis {
            turns,
            source_sha256: report.source_sha256.clone(),
            immutable_provenance: true,
        });
    }

    let (transcript_value, report, transcript_modified, has_provenance) =
        meeting_value_and_report(value)?;
    let transcript: MeetingTranscript =
        serde_json::from_value(transcript_value).map_err(json_error)?;
    transcript.validate().map_err(core_error)?;
    if transcript_modified {
        return Err(cli_error(
            "corrected or modified meeting transcripts cannot be evaluated",
        ));
    }
    if layer == EvaluationLayer::Acoustic {
        let report = report.as_ref().ok_or_else(|| {
            cli_error("hypothesis has no acoustic diarization report; use --layer transcript")
        })?;
        report.validate().map_err(core_error)?;
        if report.source_sha256 != transcript.source_sha256 {
            return Err(cli_error(
                "embedded diarization report source_sha256 does not match meeting transcript",
            ));
        }
        let turns = report
            .activity
            .iter()
            .map(|span| SpeakerTurn {
                start: span.start,
                end: span.end,
                speakers: span.speakers.clone(),
            })
            .collect();
        return Ok(ParsedHypothesis {
            turns,
            source_sha256: transcript.source_sha256.clone(),
            immutable_provenance: has_provenance,
        });
    }
    let turns = transcript
        .segments
        .iter()
        .map(|segment| SpeakerTurn {
            start: segment.start,
            end: segment.end,
            speakers: vec![segment.speaker.clone()],
        })
        .collect();
    Ok(ParsedHypothesis {
        turns,
        source_sha256: transcript.source_sha256,
        immutable_provenance: has_provenance,
    })
}

fn meeting_value_and_report(
    value: Value,
) -> Result<(Value, Option<DiarizationReport>, bool, bool), DictationError> {
    let mut transcript_value = value;
    let modified_value = transcript_value.get("transcript_modified");
    let has_provenance = modified_value.is_some();
    let transcript_modified = transcript_value
        .get("transcript_modified")
        .and_then(Value::as_bool)
        .ok_or_else(|| cli_error("transcript_modified must be a boolean when present"))
        .unwrap_or(false);
    let report_value = transcript_value
        .as_object_mut()
        .and_then(|object| object.remove("diarization"));
    transcript_value
        .as_object_mut()
        .and_then(|object| object.remove("transcript_modified"));
    let report_modified = report_value
        .as_ref()
        .and_then(|value| value.get("transcript_modified"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let report = report_value
        .map(|value| serde_json::from_value::<DiarizationReport>(value).map_err(json_error))
        .transpose()?;
    let report_present = report.is_some();
    Ok((
        transcript_value,
        report,
        transcript_modified || report_modified,
        has_provenance || report_present,
    ))
}

fn parse_reference_json(bytes: &[u8]) -> Result<ReferenceDocument, DictationError> {
    serde_json::from_str(utf8(bytes)?).map_err(json_error)
}

fn parse_report(bytes: &[u8]) -> Result<DiarizationReport, DictationError> {
    serde_json::from_str(utf8(bytes)?).map_err(json_error)
}

fn parse_rttm(text: &str) -> Result<(String, Vec<SpeakerTurn>, f64), DictationError> {
    let mut recording_id = None;
    let mut turns = Vec::new();
    let mut seen = BTreeSet::new();
    let mut duration: f64 = 0.0;
    for (index, line) in text.lines().enumerate() {
        if index >= MAX_LINES {
            return Err(cli_error("RTTM contains too many lines"));
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.is_empty() || fields[0].starts_with('#') {
            continue;
        }
        if fields.len() < 8 || fields[0] != "SPEAKER" {
            return Err(cli_error(format!("malformed RTTM line {}", index + 1)));
        }
        if fields[2] != "1" {
            return Err(cli_error(format!(
                "unsupported RTTM channel at line {}",
                index + 1
            )));
        }
        let id = fields[1];
        if id.is_empty() || id == "<NA>" {
            return Err(cli_error(format!(
                "invalid RTTM recording ID at line {}",
                index + 1
            )));
        }
        if let Some(existing) = &recording_id {
            if existing != id {
                return Err(cli_error("RTTM contains multiple recording IDs"));
            }
        } else {
            recording_id = Some(id.to_owned());
        }
        let start = parse_nonnegative(fields[3], "RTTM start", index)?;
        let duration_seconds = parse_nonnegative(fields[4], "RTTM duration", index)?;
        if duration_seconds == 0.0 {
            return Err(cli_error(format!(
                "RTTM zero duration at line {}",
                index + 1
            )));
        }
        let end = start + duration_seconds;
        if !end.is_finite() || end > 14_400.0 {
            return Err(cli_error(format!(
                "RTTM interval out of bounds at line {}",
                index + 1
            )));
        }
        let speaker = fields[7];
        if speaker.is_empty() || speaker == "<NA>" {
            return Err(cli_error(format!(
                "invalid RTTM speaker at line {}",
                index + 1
            )));
        }
        let key = (start.to_bits(), end.to_bits(), speaker.to_owned());
        if !seen.insert(key) {
            return Err(cli_error(format!(
                "duplicate RTTM interval at line {}",
                index + 1
            )));
        }
        duration = duration.max(end);
        turns.push(SpeakerTurn {
            start,
            end,
            speakers: vec![speaker.to_owned()],
        });
    }
    let recording_id = recording_id.ok_or_else(|| cli_error("RTTM contains no SPEAKER records"))?;
    Ok((recording_id, turns, duration))
}

fn parse_uem(
    text: &str,
    expected_recording_id: Option<&str>,
    duration: f64,
) -> Result<Vec<ScoringRegion>, DictationError> {
    let mut recording_id = None;
    let mut regions = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if index >= MAX_LINES {
            return Err(cli_error("UEM contains too many lines"));
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.is_empty() || fields[0].starts_with('#') {
            continue;
        }
        if fields.len() != 4 {
            return Err(cli_error(format!("malformed UEM line {}", index + 1)));
        }
        if fields[1] != "1" {
            return Err(cli_error(format!(
                "unsupported UEM channel at line {}",
                index + 1
            )));
        }
        let id = fields[0];
        if id.is_empty() || id == "<NA>" {
            return Err(cli_error(format!(
                "invalid UEM recording ID at line {}",
                index + 1
            )));
        }
        if let Some(existing) = &recording_id {
            if existing != id {
                return Err(cli_error("UEM contains multiple recording IDs"));
            }
        } else {
            recording_id = Some(id.to_owned());
        }
        if let Some(expected) = expected_recording_id {
            if expected != id {
                return Err(cli_error("UEM recording ID does not match RTTM reference"));
            }
        }
        let start = parse_nonnegative(fields[2], "UEM start", index)?;
        let end = parse_nonnegative(fields[3], "UEM end", index)?;
        if start >= end || end > duration {
            return Err(cli_error(format!(
                "UEM interval out of bounds at line {}",
                index + 1
            )));
        }
        regions.push(ScoringRegion { start, end });
    }
    if recording_id.is_none() {
        return Err(cli_error("UEM contains no regions"));
    }
    Ok(regions)
}

fn parse_nonnegative(value: &str, kind: &str, index: usize) -> Result<f64, DictationError> {
    let parsed = value
        .parse::<f64>()
        .map_err(|_| cli_error(format!("invalid {kind} at line {}", index + 1)))?;
    if !parsed.is_finite() || parsed < 0.0 {
        return Err(cli_error(format!("invalid {kind} at line {}", index + 1)));
    }
    Ok(parsed)
}

fn read_input(path: &Path) -> Result<InputFile, DictationError> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| cli_error(format!("could not inspect input: {error}")))?;
    if metadata.len() > MAX_INPUT_BYTES as u64 {
        return Err(cli_error(format!(
            "input exceeds {} MiB limit",
            MAX_INPUT_BYTES / (1024 * 1024)
        )));
    }
    let file =
        File::open(path).map_err(|error| cli_error(format!("could not open input: {error}")))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| cli_error(format!("could not read input: {error}")))?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(cli_error(format!(
            "input exceeds {} MiB limit",
            MAX_INPUT_BYTES / (1024 * 1024)
        )));
    }
    let sha256 = hex_digest(&bytes);
    Ok(InputFile { bytes, sha256 })
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn utf8(bytes: &[u8]) -> Result<&str, DictationError> {
    std::str::from_utf8(bytes).map_err(|_| cli_error("input is not valid UTF-8"))
}

fn json_string(value: &Value) -> Result<String, DictationError> {
    serde_json::to_string_pretty(value).map_err(|error| cli_error(error.to_string()))
}

fn json_error(error: impl std::fmt::Display) -> DictationError {
    cli_error(format!("invalid JSON document: {error}"))
}

fn core_error(error: impl std::fmt::Display) -> DictationError {
    cli_error(error.to_string())
}

fn cli_error(message: impl Into<String>) -> DictationError {
    DictationError::TranscriptionFailed(message.into())
}

fn write_stdout(output: &str) -> Result<(), DictationError> {
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    stdout
        .write_all(output.as_bytes())
        .and_then(|_| stdout.write_all(b"\n"))
        .map_err(|error| cli_error(format!("could not write output: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn write(value: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(value.as_bytes()).unwrap();
        file
    }

    #[test]
    fn rttm_and_uem_parse_and_score() {
        let rttm = write("SPEAKER rec 1 0.000 1.000 <NA> <NA> A <NA> <NA>\n");
        let uem = write("rec 1 0.000 1.000\n");
        let (recording, turns, duration) =
            parse_rttm(utf8(&read_input(rttm.path()).unwrap().bytes).unwrap()).unwrap();
        assert_eq!(recording, "rec");
        assert_eq!(duration, 1.0);
        let regions = parse_uem(
            utf8(&read_input(uem.path()).unwrap().bytes).unwrap(),
            Some("rec"),
            duration,
        )
        .unwrap();
        let report = evaluate(
            &turns,
            &turns,
            &regions,
            EvaluationOptions {
                collar_seconds: 0.0,
            },
        )
        .unwrap();
        assert_eq!(report.der, Some(0.0));
    }

    #[test]
    fn malformed_rttm_and_multiple_ids_are_rejected() {
        assert!(parse_rttm("SPEAKER rec 1 0 nope <NA> <NA> A <NA> <NA>").is_err());
        assert!(parse_rttm(
            "SPEAKER a 1 0 1 <NA> <NA> A <NA> <NA>\nSPEAKER b 1 0 1 <NA> <NA> A <NA> <NA>"
        )
        .is_err());
    }

    #[test]
    fn source_hash_and_activity_errors_are_explicit() {
        let report = serde_json::json!({"schema_version": 1});
        assert!(parse_report(serde_json::to_string(&report).unwrap().as_bytes()).is_err());
        assert!(parse_uem("rec 1 2 1", Some("rec"), 2.0).is_err());
    }

    #[test]
    fn candidate_reference_is_valid_but_never_gold() {
        let document = serde_json::json!({
            "source_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "duration_seconds": 1.0,
            "speakers": ["A"],
            "windows": [],
            "intervals": [{"start": 0.0, "end": 1.0, "speakers": ["A"], "status": "candidate"}]
        });
        let file = write(&serde_json::to_string(&document).unwrap());
        let output = reference_validate(file.path()).unwrap();
        let output: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(output["status"], "valid");
        assert_eq!(output["verifiedIntervals"], 0);
        assert_eq!(output["gold"], false);
    }

    #[test]
    fn silence_false_alarm_and_uem_hole_are_scored_by_core() {
        let reference = Vec::new();
        let hypothesis = vec![SpeakerTurn {
            start: 0.0,
            end: 1.0,
            speakers: vec!["A".to_owned()],
        }];
        let uem = vec![ScoringRegion {
            start: 0.0,
            end: 1.0,
        }];
        let report = evaluate(
            &reference,
            &hypothesis,
            &uem,
            EvaluationOptions {
                collar_seconds: 0.0,
            },
        )
        .unwrap();
        assert_eq!(report.reference_speech_seconds, 0.0);
        assert_eq!(report.false_alarm_seconds, 1.0);
        assert!(report.der.is_none());

        let hole = parse_uem("rec 1 1 2", Some("rec"), 2.0).unwrap();
        assert_eq!(
            hole,
            vec![ScoringRegion {
                start: 1.0,
                end: 2.0
            }]
        );
    }

    #[test]
    fn transcript_layer_requires_immutable_envelope_and_acoustic_report() {
        let transcript = serde_json::json!({
            "schema_version": 1,
            "source_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "language": "en",
            "model": "base.en",
            "duration_seconds": 1.0,
            "segments": [{"id": "seg-000001", "start": 0.0, "end": 1.0, "text": "hello", "speaker": "speaker-1"}],
            "speakers": [{"id": "speaker-1", "label": "Speaker 1"}]
        });
        let bytes = serde_json::to_vec(&transcript).unwrap();
        assert!(parse_hypothesis(&bytes, EvaluationLayer::Acoustic).is_err());
        let parsed = parse_hypothesis(&bytes, EvaluationLayer::Transcript).unwrap();
        assert!(!parsed.immutable_provenance);
        assert_eq!(parsed.turns.len(), 1);

        let mut corrected = transcript;
        corrected["transcript_modified"] = Value::Bool(true);
        assert!(parse_hypothesis(
            serde_json::to_string(&corrected).unwrap().as_bytes(),
            EvaluationLayer::Transcript
        )
        .is_err());
    }

    #[test]
    fn native_source_mismatch_is_rejected_before_scoring() {
        let reference = write(
            r#"{"source_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","duration_seconds":1.0,"speakers":["A"],"windows":[],"intervals":[{"start":0.0,"end":1.0,"speakers":["A"],"status":"candidate"}]}"#,
        );
        let hypothesis = write(
            &serde_json::to_string(&report_json(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ))
            .unwrap(),
        );
        let error = evaluate_files(
            reference.path(),
            hypothesis.path(),
            None,
            0.0,
            EvaluationLayer::Acoustic,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("source_sha256"));
    }

    fn report_json(source_sha256: &str) -> Value {
        serde_json::json!({
            "schema_version": 1,
            "source_sha256": source_sha256,
            "duration_seconds": 1.0,
            "build_revision": "test",
            "build_version": "test",
            "parameters": {"threshold": 0.5, "min_segment_seconds": 0.1, "min_gap_seconds": 0.1, "min_speaker_seconds": 0.1, "absorb_max_distance": 0.5, "hint_merge_max_distance": 0.5},
            "activity": [{"start": 0.0, "end": 1.0, "speakers": ["A"]}],
            "regions": [],
            "attributions": []
        })
    }
}
