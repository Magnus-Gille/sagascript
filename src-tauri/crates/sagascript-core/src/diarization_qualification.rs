//! Native qualification gates for human-reviewed diarization references.
//!
//! This module consumes a validated ReferenceDocument plus a separate,
//! frozen split manifest. It performs no media access and never promotes
//! candidate or unknown labels into verified reference intervals.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::diarization_reference::{
    ReferenceActivity, ReferenceDocument, ReferenceError, ReferenceInterval, ReferenceStatus,
};

const SPLITS: [&str; 3] = ["train", "dev", "eval"];
const HUMAN_REVIEWER_FORBIDDEN: [&str; 9] = [
    "model",
    "system",
    "automatic",
    "automated",
    "auto",
    "consensus",
    "assistant",
    "unknown",
    "bot",
];

#[derive(Debug, Error, PartialEq)]
pub enum QualificationError {
    #[error("invalid qualification input: {0}")]
    Invalid(String),
    #[error("reference validation failed: {0}")]
    Reference(#[from] ReferenceError),
    #[error("qualification JSON serialization failed")]
    Serialization,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualificationFailure {
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CoverageMetrics {
    pub reviewed_coverage_seconds: f64,
    pub human_identified_speech_seconds: f64,
    pub verified_speaker_time_seconds: f64,
    pub verified_time_seconds: f64,
    pub unknown_speech_excluded_seconds: f64,
    pub candidate_speech_excluded_seconds: f64,
    pub reviewed_speech_coverage_ratio: f64,
    pub verified_interval_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpeakerCoverage {
    pub human_identified_speech_seconds: f64,
    pub verified_speaker_time_seconds: f64,
    pub eval_represented: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentityReport {
    pub id: Option<String>,
    pub sha256: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualificationCoverage {
    pub overall: CoverageMetrics,
    pub by_split: BTreeMap<String, CoverageMetrics>,
    pub by_stratum: BTreeMap<String, CoverageMetrics>,
    pub by_speaker: BTreeMap<String, SpeakerCoverage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualificationReport {
    pub ready_for_quality_adoption: bool,
    pub status: String,
    pub failures: Vec<QualificationFailure>,
    pub reference_identity: IdentityReport,
    pub source_identity: IdentityReport,
    pub frozen_policy_identity: IdentityReport,
    pub frozen_split_identity: IdentityReport,
    pub coverage: QualificationCoverage,
    pub unknown_speech_excluded_seconds: f64,
    pub candidate_speech_excluded_seconds: f64,
    pub minimum_eval_reviewed_speech_coverage: f64,
}

#[derive(Debug, Clone)]
struct ManifestWindow {
    id: String,
    start: f64,
    end: f64,
    split: String,
    stratum: Option<String>,
}

#[derive(Debug, Clone)]
struct Manifest {
    reference_id: String,
    source_sha256: String,
    reference_hash_matches: bool,
    policy_id: Option<String>,
    policy_version: Option<String>,
    policy_frozen: bool,
    split_id: String,
    frozen: bool,
    windows: Vec<ManifestWindow>,
    policy_sha256: Option<String>,
    split_sha256: String,
}

/// Return the Python-compatible identity of a normalized reference document.
pub fn reference_identity(reference_json: &Value) -> Result<String, QualificationError> {
    let (_, normalized) = normalize_reference(reference_json)?;
    let canonical = canonical_json(&normalized)?;
    Ok(sha256_hex(canonical.as_bytes()))
}

/// Qualify a reference against a separate frozen split manifest.
pub fn qualify(
    reference_json: &Value,
    manifest_json: &Value,
    minimum_coverage: f64,
) -> Result<QualificationReport, QualificationError> {
    if !minimum_coverage.is_finite() || !(0.90..=1.0).contains(&minimum_coverage) {
        return Err(QualificationError::Invalid(
            "minimum coverage must be finite and between 0.90 and 1.0".into(),
        ));
    }
    let (reference, normalized) = normalize_reference(reference_json)?;
    let manifest = parse_manifest(manifest_json, &reference, &normalized)?;
    let mut failures = Vec::new();

    if manifest.source_sha256 != reference.source_sha256 {
        failures.push(failure(
            "source-hash-mismatch",
            "manifest source hash differs from the reference source",
        ));
    }
    if !manifest.reference_hash_matches {
        failures.push(failure(
            "reference-hash-mismatch",
            "manifest reference hash differs from the normalized reference",
        ));
    }
    if manifest.policy_id.is_none() || manifest.policy_version.is_none() || !manifest.policy_frozen
    {
        failures.push(failure(
            "stale-policy",
            "a frozen reference policy identity is required",
        ));
    }
    if !manifest.frozen {
        failures.push(failure(
            "stale-split",
            "split manifest must be marked frozen",
        ));
    }
    if !manifest.windows.iter().any(|window| window.split == "dev")
        || !manifest.windows.iter().any(|window| window.split == "eval")
    {
        failures.push(failure(
            "split-separation",
            "frozen dev and eval windows must both be present",
        ));
    }

    let mut assignments = BTreeMap::new();
    let mut strata = BTreeSet::new();
    for window in &manifest.windows {
        assignments.insert(window.id.as_str(), window);
        if let Some(stratum) = &window.stratum {
            strata.insert(stratum.clone());
        }
    }
    let mut by_split: BTreeMap<String, Vec<&ReferenceInterval>> = SPLITS
        .iter()
        .map(|split| ((*split).to_string(), Vec::new()))
        .collect();
    let mut by_stratum: BTreeMap<String, Vec<&ReferenceInterval>> = strata
        .iter()
        .map(|stratum| (stratum.clone(), Vec::new()))
        .collect();
    let mut by_speaker: BTreeMap<String, Vec<&ReferenceInterval>> = reference
        .speakers
        .iter()
        .map(|speaker| (speaker.clone(), Vec::new()))
        .collect();

    let mut verified = Vec::new();
    let mut has_candidate = false;
    let mut unknown_unreviewed = false;
    let mut partition_failures = BTreeSet::new();
    let mut intervals_by_window: BTreeMap<String, Vec<&ReferenceInterval>> = BTreeMap::new();
    for interval in &reference.intervals {
        let window_id = resolve_window_id(interval, &reference);
        if let Some(window_id) = window_id {
            intervals_by_window
                .entry(window_id.clone())
                .or_default()
                .push(interval);
            if let Some(window) = assignments.get(window_id.as_str()) {
                by_split
                    .get_mut(&window.split)
                    .expect("manifest split validated")
                    .push(interval);
                if let Some(stratum) = &window.stratum {
                    by_stratum
                        .get_mut(stratum)
                        .expect("manifest stratum initialized")
                        .push(interval);
                }
            }
        }
        match interval.status {
            ReferenceStatus::Verified => {
                if effective_activity(interval) == ReferenceActivity::Speech {
                    verified.push(interval);
                    for speaker in &interval.speakers {
                        by_speaker
                            .get_mut(speaker)
                            .expect("reference speaker validated")
                            .push(interval);
                    }
                }
            }
            ReferenceStatus::Candidate => has_candidate = true,
            ReferenceStatus::Unknown => {
                if !reviewed_unknown(interval) {
                    unknown_unreviewed = true;
                }
            }
        }
    }
    if verified.is_empty() {
        failures.push(failure(
            "zero-verified",
            "no human-verified intervals are available",
        ));
    }
    if has_candidate {
        failures.push(failure(
            "candidate-unreviewed",
            "candidate intervals, including candidate silence, prevent quality adoption",
        ));
    }
    if unknown_unreviewed {
        failures.push(failure(
            "unknown-unreviewed",
            "unknown intervals need human reviewer, timestamp, and evidence",
        ));
    }
    for window in &manifest.windows {
        let mut selected = intervals_by_window
            .get(window.id.as_str())
            .cloned()
            .unwrap_or_default();
        selected.sort_by(|left, right| {
            left.start
                .total_cmp(&right.start)
                .then(left.end.total_cmp(&right.end))
        });
        let mut cursor = window.start;
        let mut overlap = false;
        let mut incomplete = selected.is_empty();
        for interval in selected {
            if interval.start < cursor {
                overlap = true;
            }
            if interval.start > cursor {
                incomplete = true;
            }
            cursor = cursor.max(interval.end);
        }
        if cursor < window.end {
            incomplete = true;
        }
        if overlap {
            partition_failures.insert("overlapping-partition");
        }
        if incomplete {
            partition_failures.insert("incomplete-partition");
        }
    }
    for code in partition_failures {
        failures.push(failure(
            code,
            "selected window annotations must be a complete disjoint partition",
        ));
    }

    let overall_intervals = reference.intervals.iter().collect::<Vec<_>>();
    let overall = bucket_metrics(&overall_intervals);
    let split_metrics = by_split
        .iter()
        .map(|(split, intervals)| (split.clone(), bucket_metrics(intervals)))
        .collect::<BTreeMap<_, _>>();
    let eval = split_metrics.get("eval").expect("eval bucket initialized");
    if eval.reviewed_speech_coverage_ratio < minimum_coverage {
        failures.push(failure(
            "low-eval-coverage",
            format!(
                "eval reviewed speech coverage is below {:.0}%",
                minimum_coverage * 100.0
            ),
        ));
    }
    let stratum_metrics = by_stratum
        .iter()
        .map(|(stratum, intervals)| (stratum.clone(), bucket_metrics(intervals)))
        .collect::<BTreeMap<_, _>>();
    let speaker_metrics = by_speaker
        .iter()
        .map(|(speaker, intervals)| {
            let seconds = union_seconds(intervals.iter().copied());
            let speaker_time = intervals
                .iter()
                .map(|interval| interval.end - interval.start)
                .sum::<f64>();
            let eval_represented = intervals.iter().any(|interval| {
                resolve_window_id(interval, &reference)
                    .and_then(|id| {
                        assignments
                            .get(id.as_str())
                            .map(|window| window.split == "eval")
                    })
                    .unwrap_or(false)
            });
            (
                speaker.clone(),
                SpeakerCoverage {
                    human_identified_speech_seconds: round9(seconds),
                    verified_speaker_time_seconds: round9(speaker_time),
                    eval_represented,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    for (speaker, metrics) in &speaker_metrics {
        if !metrics.eval_represented {
            failures.push(failure(
                "missing-eval-speaker",
                format!("eval has no verified speech for {speaker}"),
            ));
        }
    }
    for (stratum, intervals) in &by_stratum {
        let represented = intervals.iter().any(|interval| {
            interval.status == ReferenceStatus::Verified
                && effective_activity(interval) == ReferenceActivity::Speech
                && resolve_window_id(interval, &reference)
                    .and_then(|id| {
                        assignments
                            .get(id.as_str())
                            .map(|window| window.split == "eval")
                    })
                    .unwrap_or(false)
        });
        if !represented {
            failures.push(failure(
                "missing-eval-stratum",
                format!("eval has no verified speech for {stratum}"),
            ));
        }
    }

    let unknown_excluded = overall.unknown_speech_excluded_seconds;
    let candidate_excluded = overall.candidate_speech_excluded_seconds;
    Ok(QualificationReport {
        ready_for_quality_adoption: failures.is_empty(),
        status: if failures.is_empty() {
            "gold"
        } else {
            "not_ready"
        }
        .into(),
        failures,
        reference_identity: IdentityReport {
            id: Some(manifest.reference_id),
            sha256: Some(reference_identity(&normalized)?),
            version: None,
        },
        source_identity: IdentityReport {
            id: None,
            sha256: Some(reference.source_sha256.clone()),
            version: None,
        },
        frozen_policy_identity: IdentityReport {
            id: manifest.policy_id,
            sha256: manifest.policy_sha256,
            version: manifest.policy_version,
        },
        frozen_split_identity: IdentityReport {
            id: Some(manifest.split_id),
            sha256: Some(manifest.split_sha256),
            version: None,
        },
        coverage: QualificationCoverage {
            overall,
            by_split: split_metrics,
            by_stratum: stratum_metrics,
            by_speaker: speaker_metrics,
        },
        unknown_speech_excluded_seconds: unknown_excluded,
        candidate_speech_excluded_seconds: candidate_excluded,
        minimum_eval_reviewed_speech_coverage: minimum_coverage,
    })
}

fn parse_manifest(
    value: &Value,
    reference: &ReferenceDocument,
    normalized: &Value,
) -> Result<Manifest, QualificationError> {
    let object = value
        .as_object()
        .ok_or_else(|| QualificationError::Invalid("manifest must be an object".into()))?;
    let allowed = [
        "reference_id",
        "reference_sha256",
        "source_sha256",
        "policy",
        "split_id",
        "frozen",
        "windows",
        "seed",
        "ratios",
        "counts",
        "policy_id",
    ];
    reject_unknown(object, &allowed)?;
    let reference_id = required_identifier(object, "reference_id")?;
    let reference_sha256 = required_hash(object, "reference_sha256")?;
    let source_sha256 = required_hash(object, "source_sha256")?;
    let split_id = required_identifier(object, "split_id")?;
    let frozen = object.get("frozen") == Some(&Value::Bool(true));
    let reference_hash_matches =
        reference_sha256 == reference_identity_from_normalized(normalized)?;
    let (policy_id, policy_version, policy_frozen, policy_sha256) =
        if let Some(policy_value) = object.get("policy").filter(|value| !value.is_null()) {
            let policy = policy_value
                .as_object()
                .ok_or_else(|| QualificationError::Invalid("policy must be an object".into()))?;
            reject_unknown(policy, &["id", "version", "frozen"])?;
            let policy_id = policy
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty() && !value.chars().any(char::is_whitespace))
                .map(str::to_string);
            let policy_version = policy
                .get("version")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string);
            let policy_frozen = policy.get("frozen") == Some(&Value::Bool(true));
            let policy_sha256 = Some(sha256_hex(
                canonical_json(&Value::Object(policy.clone()))?.as_bytes(),
            ));
            (policy_id, policy_version, policy_frozen, policy_sha256)
        } else {
            (None, None, false, None)
        };
    let raw_windows = object
        .get("windows")
        .and_then(Value::as_array)
        .ok_or_else(|| QualificationError::Invalid("manifest windows must be an array".into()))?;
    let reference_windows: BTreeMap<_, _> = reference
        .windows
        .iter()
        .map(|window| (window.id.as_str(), window))
        .collect();
    let mut seen = BTreeSet::new();
    let mut windows = Vec::with_capacity(raw_windows.len());
    for raw in raw_windows {
        let raw = raw.as_object().ok_or_else(|| {
            QualificationError::Invalid("manifest window must be an object".into())
        })?;
        reject_unknown(raw, &["id", "start", "end", "split", "stratum"])?;
        let id = required_identifier(raw, "id")?;
        if !seen.insert(id.clone()) {
            return Err(QualificationError::Invalid(
                "duplicate manifest window".into(),
            ));
        }
        let start = required_f64(raw, "start")?;
        let end = required_f64(raw, "end")?;
        let split = raw
            .get("split")
            .and_then(Value::as_str)
            .filter(|split| SPLITS.contains(split))
            .ok_or_else(|| QualificationError::Invalid("window split".into()))?
            .to_string();
        let stratum = raw
            .get("stratum")
            .map(|value| {
                value
                    .as_str()
                    .filter(|stratum| {
                        !stratum.is_empty() && !stratum.chars().any(char::is_whitespace)
                    })
                    .map(str::to_string)
                    .ok_or_else(|| QualificationError::Invalid("window stratum".into()))
            })
            .transpose()?;
        let reference_window = reference_windows
            .get(id.as_str())
            .ok_or_else(|| QualificationError::Invalid(format!("unknown manifest window {id}")))?;
        if reference_window.start != start || reference_window.end != end {
            return Err(QualificationError::Invalid(
                "manifest window boundary mismatch".into(),
            ));
        }
        windows.push(ManifestWindow {
            id,
            start,
            end,
            split,
            stratum,
        });
    }
    if seen.len() != reference_windows.len() {
        return Err(QualificationError::Invalid(
            "manifest must assign every reference window".into(),
        ));
    }
    let split_sha256 = sha256_hex(canonical_json(value)?.as_bytes());
    Ok(Manifest {
        reference_id,
        source_sha256,
        reference_hash_matches,
        policy_id,
        policy_version,
        policy_frozen,
        split_id,
        frozen,
        windows,
        policy_sha256,
        split_sha256,
    })
}

fn normalize_reference(value: &Value) -> Result<(ReferenceDocument, Value), QualificationError> {
    let object = value
        .as_object()
        .ok_or_else(|| QualificationError::Invalid("reference must be an object".into()))?;
    reject_unknown(
        object,
        &[
            "source_sha256",
            "duration_seconds",
            "speakers",
            "windows",
            "intervals",
        ],
    )?;
    let reference: ReferenceDocument = serde_json::from_value(value.clone())
        .map_err(|error| QualificationError::Invalid(error.to_string()))?;
    reference.validate()?;
    let intervals = object
        .get("intervals")
        .and_then(Value::as_array)
        .ok_or_else(|| QualificationError::Invalid("intervals must be an array".into()))?;
    let mut normalized_intervals = Vec::with_capacity(intervals.len());
    for (index, raw) in intervals.iter().enumerate() {
        let mut normalized = raw
            .as_object()
            .ok_or_else(|| {
                QualificationError::Invalid(format!("interval {index} must be an object"))
            })?
            .clone();
        let interval = reference
            .intervals
            .get(index)
            .ok_or_else(|| QualificationError::Invalid("interval count".into()))?;
        normalized.insert("start".into(), f64_value(interval.start)?);
        normalized.insert("end".into(), f64_value(interval.end)?);
        normalized.insert(
            "speakers".into(),
            Value::Array(
                interval
                    .speakers
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
        if !normalized.contains_key("evidence") {
            normalized.insert("evidence".into(), Value::Array(Vec::new()));
        }
        if !normalized.contains_key("activity") && interval.status != ReferenceStatus::Unknown {
            normalized.insert("activity".into(), Value::String("speech".into()));
        }
        if !normalized.contains_key("window_id") && !reference.windows.is_empty() {
            if let Some(window) = containing_window(interval, &reference.windows) {
                normalized.insert("window_id".into(), Value::String(window.id.clone()));
            }
        }
        normalized_intervals.push(Value::Object(normalized));
    }
    let mut normalized = Map::new();
    normalized.insert(
        "source_sha256".into(),
        Value::String(reference.source_sha256.clone()),
    );
    normalized.insert(
        "duration_seconds".into(),
        f64_value(reference.duration_seconds)?,
    );
    normalized.insert(
        "speakers".into(),
        Value::Array(
            reference
                .speakers
                .iter()
                .cloned()
                .map(Value::String)
                .collect(),
        ),
    );
    normalized.insert(
        "windows".into(),
        Value::Array(
            reference
                .windows
                .iter()
                .map(|window| {
                    let mut value = Map::new();
                    value.insert("id".into(), Value::String(window.id.clone()));
                    value.insert(
                        "start".into(),
                        f64_value(window.start).expect("validated finite"),
                    );
                    value.insert(
                        "end".into(),
                        f64_value(window.end).expect("validated finite"),
                    );
                    Value::Object(value)
                })
                .collect(),
        ),
    );
    normalized.insert("intervals".into(), Value::Array(normalized_intervals));
    Ok((reference, Value::Object(normalized)))
}

fn reference_identity_from_normalized(normalized: &Value) -> Result<String, QualificationError> {
    Ok(sha256_hex(canonical_json(normalized)?.as_bytes()))
}

fn reject_unknown(object: &Map<String, Value>, allowed: &[&str]) -> Result<(), QualificationError> {
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(QualificationError::Invalid(format!(
            "unsupported field {key}"
        )));
    }
    Ok(())
}

fn required_identifier(
    object: &Map<String, Value>,
    key: &str,
) -> Result<String, QualificationError> {
    let value = object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_whitespace))
        .ok_or_else(|| QualificationError::Invalid(format!("{key} identity is required")))?;
    Ok(value.to_string())
}

fn required_hash(object: &Map<String, Value>, key: &str) -> Result<String, QualificationError> {
    let value = required_identifier(object, key)?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(QualificationError::Invalid(format!(
            "{key} must be lowercase SHA-256"
        )));
    }
    Ok(value)
}

fn required_f64(object: &Map<String, Value>, key: &str) -> Result<f64, QualificationError> {
    object
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or_else(|| QualificationError::Invalid(format!("{key} must be finite")))
}

fn f64_value(value: f64) -> Result<Value, QualificationError> {
    Number::from_f64(value)
        .map(Value::Number)
        .ok_or_else(|| QualificationError::Invalid("non-finite number".into()))
}

fn canonical_json(value: &Value) -> Result<String, QualificationError> {
    let mut output = String::new();
    write_canonical(value, &mut output)?;
    Ok(output)
}

fn write_canonical(value: &Value, output: &mut String) -> Result<(), QualificationError> {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(&canonical_number(value)?),
        Value::String(value) => output.push_str(&escape_ascii(value)),
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_canonical(value, output)?;
            }
            output.push(']');
        }
        Value::Object(values) => {
            output.push('{');
            for (index, (key, value)) in values
                .iter()
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .enumerate()
            {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(&escape_ascii(key));
                output.push(':');
                write_canonical(value, output)?;
            }
            output.push('}');
        }
    }
    Ok(())
}

fn canonical_number(value: &Number) -> Result<String, QualificationError> {
    let raw = value.to_string();
    let number = value.as_f64().ok_or(QualificationError::Serialization)?;
    if number == 0.0 {
        return Ok(if number.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        });
    }
    let scientific = number.abs() < 1.0e-4 || number.abs() >= 1.0e16;
    let (mantissa, exponent) = split_number(&raw)?;
    if scientific {
        Ok(format_scientific(&mantissa, exponent))
    } else {
        Ok(expand_number(&mantissa, exponent))
    }
}

fn split_number(raw: &str) -> Result<(String, i32), QualificationError> {
    if let Some((mantissa, exponent)) = raw.split_once(['e', 'E']) {
        Ok((
            mantissa.to_string(),
            exponent
                .parse::<i32>()
                .map_err(|_| QualificationError::Serialization)?,
        ))
    } else {
        Ok((raw.to_string(), 0))
    }
}

fn format_scientific(mantissa: &str, exponent: i32) -> String {
    let negative = mantissa.starts_with('-');
    let unsigned = mantissa.trim_start_matches('-');
    let digits = unsigned.replace('.', "");
    let first = digits.find(|character: char| character != '0').unwrap_or(0);
    let significant = digits[first..].trim_end_matches('0');
    let decimal_exponent =
        exponent + (unsigned.find('.').unwrap_or(unsigned.len()) as i32 - first as i32 - 1);
    let mut output = String::new();
    if negative {
        output.push('-');
    }
    output.push(significant.as_bytes()[0] as char);
    if significant.len() > 1 {
        output.push('.');
        output.push_str(&significant[1..]);
    }
    output.push('e');
    output.push(if decimal_exponent >= 0 { '+' } else { '-' });
    output.push_str(&format!("{:02}", decimal_exponent.abs()));
    output
}

fn expand_number(mantissa: &str, exponent: i32) -> String {
    if exponent == 0 {
        return mantissa.to_string();
    }
    let negative = mantissa.starts_with('-');
    let mantissa = mantissa.trim_start_matches('-');
    let mut digits = mantissa.replace('.', "");
    let decimal = mantissa.find('.').unwrap_or(mantissa.len()) as i32 + exponent;
    let output = if decimal <= 0 {
        format!("0.{}{}", "0".repeat((-decimal) as usize), digits)
    } else if decimal as usize >= digits.len() {
        digits.push_str(&"0".repeat(decimal as usize - digits.len()));
        digits
    } else {
        let index = decimal as usize;
        let suffix = digits.split_off(index);
        format!("{digits}.{suffix}")
    };
    if negative {
        format!("-{output}")
    } else {
        output
    }
}

fn escape_ascii(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                output.push_str(&format!("\\u{:04x}", character as u32))
            }
            character if character.is_ascii() => output.push(character),
            character if (character as u32) <= 0xffff => {
                output.push_str(&format!("\\u{:04x}", character as u32))
            }
            character => {
                let code = character as u32 - 0x1_0000;
                output.push_str(&format!(
                    "\\u{:04x}\\u{:04x}",
                    0xd800 + (code >> 10),
                    0xdc00 + (code & 0x3ff)
                ));
            }
        }
    }
    output.push('"');
    output
}

fn sha256_hex(value: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn resolve_window_id(
    interval: &ReferenceInterval,
    reference: &ReferenceDocument,
) -> Option<String> {
    interval.window_id.clone().or_else(|| {
        reference
            .windows
            .iter()
            .find(|window| interval.start >= window.start && interval.end <= window.end)
            .map(|window| window.id.clone())
    })
}

fn containing_window<'a>(
    interval: &ReferenceInterval,
    windows: &'a [crate::diarization_reference::ReferenceWindow],
) -> Option<&'a crate::diarization_reference::ReferenceWindow> {
    windows
        .iter()
        .find(|window| interval.start >= window.start && interval.end <= window.end)
}

fn effective_activity(interval: &ReferenceInterval) -> ReferenceActivity {
    interval.activity.unwrap_or(ReferenceActivity::Speech)
}

fn reviewed_unknown(interval: &ReferenceInterval) -> bool {
    let reviewer = interval.reviewer.as_deref().map(str::trim).unwrap_or("");
    !reviewer.is_empty()
        && !HUMAN_REVIEWER_FORBIDDEN.contains(&reviewer.to_ascii_lowercase().as_str())
        && interval
            .reviewed_at
            .as_deref()
            .is_some_and(valid_timestamp_shape)
        && !interval.evidence.is_empty()
}

fn valid_timestamp_shape(value: &str) -> bool {
    value.len() >= 20
        && (value.ends_with('Z')
            || value.as_bytes().get(value.len().saturating_sub(6)) == Some(&b'+')
            || value.as_bytes().get(value.len().saturating_sub(6)) == Some(&b'-'))
}

fn failure(code: impl Into<String>, detail: impl Into<String>) -> QualificationFailure {
    QualificationFailure {
        code: code.into(),
        detail: detail.into(),
    }
}

fn round9(value: f64) -> f64 {
    (value * 1_000_000_000.0).round() / 1_000_000_000.0
}

fn union_seconds<'a>(intervals: impl IntoIterator<Item = &'a ReferenceInterval>) -> f64 {
    let mut regions: Vec<_> = intervals
        .into_iter()
        .map(|interval| (interval.start, interval.end))
        .collect();
    regions.sort_by(|left, right| left.0.total_cmp(&right.0).then(left.1.total_cmp(&right.1)));
    let mut total = 0.0;
    let mut current: Option<(f64, f64)> = None;
    for (start, end) in regions {
        if let Some((current_start, current_end)) = current.as_mut() {
            if start <= *current_end {
                *current_end = (*current_end).max(end);
            } else {
                total += *current_end - *current_start;
                current = Some((start, end));
            }
        } else {
            current = Some((start, end));
        }
    }
    total + current.map(|(start, end)| end - start).unwrap_or(0.0)
}

fn bucket_metrics(intervals: &[&ReferenceInterval]) -> CoverageMetrics {
    let verified: Vec<_> = intervals
        .iter()
        .copied()
        .filter(|interval| interval.status == ReferenceStatus::Verified)
        .collect();
    let speech: Vec<_> = verified
        .iter()
        .copied()
        .filter(|interval| effective_activity(interval) == ReferenceActivity::Speech)
        .collect();
    let unknown: Vec<_> = intervals
        .iter()
        .copied()
        .filter(|interval| interval.status == ReferenceStatus::Unknown)
        .collect();
    let candidate: Vec<_> = intervals
        .iter()
        .copied()
        .filter(|interval| {
            interval.status == ReferenceStatus::Candidate
                && effective_activity(interval) == ReferenceActivity::Speech
        })
        .collect();
    let human = union_seconds(speech.iter().copied());
    let unknown_seconds = union_seconds(unknown.iter().copied());
    let candidate_seconds = union_seconds(candidate.iter().copied());
    let denominator = human + unknown_seconds + candidate_seconds;
    let verified_time = union_seconds(verified.iter().copied());
    CoverageMetrics {
        reviewed_coverage_seconds: round9(verified_time),
        human_identified_speech_seconds: round9(human),
        verified_speaker_time_seconds: round9(
            speech
                .iter()
                .map(|interval| (interval.end - interval.start) * interval.speakers.len() as f64)
                .sum(),
        ),
        verified_time_seconds: round9(verified_time),
        unknown_speech_excluded_seconds: round9(unknown_seconds),
        candidate_speech_excluded_seconds: round9(candidate_seconds),
        reviewed_speech_coverage_ratio: if denominator > 0.0 {
            round9(human / denominator)
        } else {
            0.0
        },
        verified_interval_count: verified.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reference() -> Value {
        json!({
            "source_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "duration_seconds": 10,
            "speakers": ["alice", "bob"],
            "windows": [{"id":"w1","start":0,"end":10}],
            "intervals": [
                {"start":0,"end":2,"speakers":["alice"],"status":"verified","evidence":[{"kind":"human_review","artifact":"note"}],"reviewer":"reviewer","reviewed_at":"2026-01-01T00:00:00Z"},
                {"start":2,"end":10,"speakers":[],"status":"unknown","evidence":[{"kind":"human_review","artifact":"note"}],"reviewer":"reviewer","reviewed_at":"2026-01-01T00:00:00Z"}
            ]
        })
    }

    fn manifest(reference: &Value) -> Value {
        json!({
            "reference_id":"r1",
            "reference_sha256":reference_identity(reference).unwrap(),
            "source_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "policy":{"id":"human-review-v1","version":"1","frozen":true},
            "split_id":"s1","frozen":true,
            "windows":[{"id":"w1","start":0,"end":10,"split":"eval","stratum":"difficult"}]
        })
    }

    #[test]
    fn identity_is_stable_and_manifest_binding_is_exact() {
        let reference = reference();
        let manifest = manifest(&reference);
        let report = qualify(&reference, &manifest, 0.9).unwrap();
        assert!(!report.ready_for_quality_adoption);
        assert!(report
            .failures
            .iter()
            .any(|failure| failure.code == "split-separation"));
        let mut changed = reference.clone();
        changed["intervals"][0]["end"] = json!(1.5);
        let report = qualify(&changed, &manifest, 0.9).unwrap();
        assert!(report
            .failures
            .iter()
            .any(|failure| failure.code == "reference-hash-mismatch"));
    }

    #[test]
    fn candidate_unknown_and_partition_gates_are_reported() {
        let mut reference = reference();
        reference["intervals"][1]["status"] = json!("candidate");
        reference["intervals"][1]["activity"] = json!("silence");
        let report = qualify(&reference, &manifest(&reference), 0.9);
        let report = report.expect("candidate annotations remain structurally valid");
        assert!(!report.ready_for_quality_adoption);
        assert!(report
            .failures
            .iter()
            .any(|failure| failure.code == "candidate-unreviewed"));
    }

    #[test]
    fn threshold_and_unknown_identity_are_strict() {
        let reference = reference();
        let manifest = manifest(&reference);
        assert!(qualify(&reference, &manifest, 0.89).is_err());
        let mut missing = manifest.clone();
        missing["split_id"] = Value::Null;
        assert!(qualify(&reference, &missing, 0.9).is_err());
    }

    #[test]
    fn identity_matches_python_canonical_numbers_unicode_and_negative_zero() {
        let reference = json!({
            "source_sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "duration_seconds": 10,
            "speakers": ["Åke", "bob"],
            "windows": [
                {"id": "tiny", "start": -0.0, "end": 1e-6},
                {"id": "rest", "start": 1e-6, "end": 10}
            ],
            "intervals": [
                {"start": -0.0, "end": 1e-6, "speakers": ["Åke"], "status": "verified", "evidence": [{"kind": "human", "artifact": "✓"}], "reviewer": "human", "reviewed_at": "2026-01-01T00:00:00Z"},
                {"start": 1e-6, "end": 10, "speakers": ["bob"], "status": "verified", "activity": null, "evidence": [{"kind": "human", "artifact": "note"}], "reviewer": "human", "reviewed_at": "2026-01-01T00:00:00Z"}
            ]
        });
        assert_eq!(
            reference_identity(&reference).unwrap(),
            "1d28c317ea35dc54d441a7cb95637e6c5f907baa81ab5850d2fef64f3dd3be08"
        );
    }

    #[test]
    fn balanced_frozen_manifest_qualifies_and_present_strata_are_eval_required() {
        let reference = json!({
            "source_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "duration_seconds": 30,
            "speakers": ["alice", "bob"],
            "windows": [
                {"id": "train", "start": 0, "end": 10},
                {"id": "dev", "start": 10, "end": 20},
                {"id": "eval", "start": 20, "end": 30}
            ],
            "intervals": [
                {"start": 0, "end": 10, "speakers": ["alice"], "status": "verified", "evidence": [{"kind": "human", "artifact": "a"}], "reviewer": "reviewer", "reviewed_at": "2026-01-01T00:00:00Z"},
                {"start": 10, "end": 20, "speakers": ["bob"], "status": "verified", "evidence": [{"kind": "human", "artifact": "b"}], "reviewer": "reviewer", "reviewed_at": "2026-01-01T00:00:00Z"},
                {"start": 20, "end": 25, "speakers": ["alice"], "status": "verified", "evidence": [{"kind": "human", "artifact": "c"}], "reviewer": "reviewer", "reviewed_at": "2026-01-01T00:00:00Z"},
                {"start": 25, "end": 30, "speakers": ["bob"], "status": "verified", "evidence": [{"kind": "human", "artifact": "d"}], "reviewer": "reviewer", "reviewed_at": "2026-01-01T00:00:00Z"}
            ]
        });
        let mut manifest = json!({
            "reference_id": "balanced-v1",
            "reference_sha256": reference_identity(&reference).unwrap(),
            "source_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "policy": {"id": "human-review-v1", "version": "1", "frozen": true},
            "split_id": "balanced-split-v1",
            "frozen": true,
            "windows": [
                {"id": "train", "start": 0, "end": 10, "split": "train", "stratum": "ordinary"},
                {"id": "dev", "start": 10, "end": 20, "split": "dev", "stratum": "ordinary"},
                {"id": "eval", "start": 20, "end": 30, "split": "eval", "stratum": "ordinary"}
            ]
        });
        let report = qualify(&reference, &manifest, 0.9).unwrap();
        assert!(report.ready_for_quality_adoption);
        assert_eq!(report.status, "gold");
        assert_eq!(
            report.coverage.by_split["eval"].human_identified_speech_seconds,
            10.0
        );

        manifest["windows"][1]["stratum"] = json!("dev-only");
        let report = qualify(&reference, &manifest, 0.9).unwrap();
        assert!(report
            .failures
            .iter()
            .any(|failure| failure.code == "missing-eval-stratum"));
    }

    #[test]
    fn canonical_numbers_follow_python_exponent_thresholds() {
        assert_eq!(
            canonical_number(&Number::from_f64(-1e-6).unwrap()).unwrap(),
            "-1e-06"
        );
        assert_eq!(
            canonical_number(&Number::from_f64(1e-5).unwrap()).unwrap(),
            "1e-05"
        );
        assert_eq!(
            canonical_number(&Number::from_f64(1.2e-4).unwrap()).unwrap(),
            "0.00012"
        );
        assert_eq!(
            canonical_number(&Number::from_f64(1e16).unwrap()).unwrap(),
            "1e+16"
        );
    }

    #[test]
    fn stale_identity_policy_split_and_empty_gold_are_reported_not_adopted() {
        let reference = reference();
        let mut stale = manifest(&reference);
        stale["source_sha256"] =
            json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        stale["policy"]["frozen"] = json!(false);
        stale["frozen"] = json!(false);
        let report = qualify(&reference, &stale, 0.9).unwrap();
        let codes = report
            .failures
            .iter()
            .map(|failure| failure.code.as_str())
            .collect::<BTreeSet<_>>();
        assert!(!report.ready_for_quality_adoption);
        assert!(codes.contains("source-hash-mismatch"));
        assert!(codes.contains("stale-policy"));
        assert!(codes.contains("stale-split"));

        let mut empty = reference.clone();
        empty["intervals"] = json!([]);
        let report = qualify(&empty, &manifest(&reference), 0.9).unwrap();
        assert!(report
            .failures
            .iter()
            .any(|failure| failure.code == "zero-verified"));
    }
}
