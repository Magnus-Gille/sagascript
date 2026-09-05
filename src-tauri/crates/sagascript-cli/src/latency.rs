use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use clap::{Args, ValueEnum};
use serde::Serialize;
use serde_json::Value;

use sagascript_core::error::DictationError;
use sagascript_core::settings::{Language, WhisperModel};

use crate::transcribe::model_id_string;

mod percentile;

use percentile::percentile_ms;

const MAX_INPUT_BYTES: usize = 32 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 1024 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 256;
const METRIC_BOUNDARY: &str = "paste_call_completion_not_visible_text";

const METRIC_FIELDS: [MetricField; 7] = [
    MetricField::KeyUpToCaptureStopped,
    MetricField::ModelLoad,
    MetricField::KeyUpToModelReady,
    MetricField::Whisper,
    MetricField::KeyUpToWhisperComplete,
    MetricField::KeyUpToPasteCompleted,
    MetricField::Total,
];

const CONFIG_FIELDS: [&str; 6] = [
    "model",
    "language",
    "modelWasWarm",
    "beamSize",
    "temperatureFallback",
    "vadEnabled",
];

const PASTE_OUTCOMES: [&str; 6] = [
    "disabled",
    "dispatch_failed",
    "succeeded",
    "failed",
    "completion_dropped",
    "timed_out",
];

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum LengthBucket {
    Short,
    Medium,
    Long,
    #[value(skip)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Report {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u8,
    #[serde(rename = "reporterVersion")]
    pub reporter_version: &'static str,
    #[serde(rename = "metricBoundary")]
    pub metric_boundary: &'static str,
    #[serde(rename = "sourceBuild")]
    pub source_build: Option<&'static str>,
    #[serde(rename = "inputRecords")]
    pub input_records: usize,
    #[serde(rename = "phaseRecords")]
    pub phase_records: usize,
    #[serde(rename = "captureWithoutPhases")]
    pub capture_without_phases: usize,
    pub groups: Vec<Group>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Group {
    pub model: Option<String>,
    pub language: Option<String>,
    #[serde(rename = "modelWasWarm")]
    pub model_was_warm: Option<bool>,
    #[serde(rename = "beamSize")]
    pub beam_size: Option<u32>,
    #[serde(rename = "temperatureFallback")]
    pub temperature_fallback: Option<bool>,
    #[serde(rename = "vadEnabled")]
    pub vad_enabled: Option<bool>,
    pub outcome: String,
    #[serde(rename = "pasteOutcome")]
    pub paste_outcome: Option<String>,
    #[serde(rename = "lengthBucket")]
    pub length_bucket: LengthBucket,
    pub samples: usize,
    pub phases: PhaseStatsSet,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct PhaseStatsSet {
    #[serde(rename = "keyUpToCaptureStoppedMs")]
    pub key_up_to_capture_stopped_ms: MetricStats,
    #[serde(rename = "modelLoadMs")]
    pub model_load_ms: MetricStats,
    #[serde(rename = "keyUpToModelReadyMs")]
    pub key_up_to_model_ready_ms: MetricStats,
    #[serde(rename = "whisperMs")]
    pub whisper_ms: MetricStats,
    #[serde(rename = "keyUpToWhisperCompleteMs")]
    pub key_up_to_whisper_complete_ms: MetricStats,
    #[serde(rename = "keyUpToPasteCompletedMs")]
    pub key_up_to_paste_completed_ms: MetricStats,
    #[serde(rename = "totalMs")]
    pub total_ms: MetricStats,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct MetricStats {
    pub count: usize,
    #[serde(rename = "numericCount")]
    pub numeric_count: usize,
    #[serde(rename = "nullCount")]
    pub null_count: usize,
    #[serde(rename = "missingCount")]
    pub missing_count: usize,
    #[serde(rename = "p50Ms")]
    pub p50_ms: Option<f64>,
    #[serde(rename = "p95Ms")]
    pub p95_ms: Option<f64>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct GroupKey {
    model: Option<String>,
    language: Option<String>,
    model_was_warm: Option<bool>,
    beam_size: Option<u32>,
    temperature_fallback: Option<bool>,
    vad_enabled: Option<bool>,
    outcome: String,
    paste_outcome: Option<String>,
    length_bucket: LengthBucket,
}

#[derive(Debug, Default)]
struct GroupAccumulator {
    samples: usize,
    metrics: [MetricAccumulator; 7],
}

#[derive(Debug, Default)]
struct MetricAccumulator {
    count: usize,
    numeric_count: usize,
    null_count: usize,
    missing_count: usize,
    values: Vec<f64>,
}

#[derive(Debug, Clone)]
struct CaptureRecord {
    audio_duration_ms: f64,
}

#[derive(Debug, Clone)]
struct PhaseRecord {
    key: CorrelationKey,
    config: PhaseConfig,
    metrics: [MetricValue; 7],
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct CorrelationKey {
    app_session: String,
    dictation_session: String,
}

#[derive(Debug, Clone)]
struct PhaseConfig {
    model: Option<String>,
    language: Option<String>,
    model_was_warm: Option<bool>,
    beam_size: Option<u32>,
    temperature_fallback: Option<bool>,
    vad_enabled: Option<bool>,
    outcome: String,
    paste_outcome: Option<String>,
}

#[derive(Debug, Clone)]
enum MetricValue {
    Missing,
    Null,
    Numeric(f64),
}

#[derive(Debug, Clone, Copy)]
enum MetricField {
    KeyUpToCaptureStopped,
    ModelLoad,
    KeyUpToModelReady,
    Whisper,
    KeyUpToWhisperComplete,
    KeyUpToPasteCompleted,
    Total,
}

impl MetricField {
    const fn key(self) -> &'static str {
        match self {
            Self::KeyUpToCaptureStopped => "keyUpToCaptureStoppedMs",
            Self::ModelLoad => "modelLoadMs",
            Self::KeyUpToModelReady => "keyUpToModelReadyMs",
            Self::Whisper => "whisperMs",
            Self::KeyUpToWhisperComplete => "keyUpToWhisperCompleteMs",
            Self::KeyUpToPasteCompleted => "keyUpToPasteCompletedMs",
            Self::Total => "totalMs",
        }
    }
}

#[derive(Debug, Args)]
pub struct LatencyReportArgs {
    /// Explicit copied JSONL log; the live/default log path is never selected implicitly.
    #[arg(long, value_name = "PATH")]
    pub input: PathBuf,

    /// Select the audio-length cohort for the explicit warm-paste budget check.
    #[arg(long, value_enum, requires = "max_warm_p95_ms")]
    pub budget_length: Option<LengthBucket>,

    /// Maximum permitted warm paste-call p95 in milliseconds.
    #[arg(long, value_name = "MS", requires = "budget_length")]
    pub max_warm_p95_ms: Option<f64>,

    /// Minimum numeric paste-call samples required per eligible group.
    #[arg(long, default_value_t = 20, requires = "budget_length")]
    pub min_samples: usize,
}

/// Summarize explicitly supplied JSONL without consulting app state, settings,
/// models, the microphone, or a network service.
pub fn summarize_reader<R: BufRead>(mut reader: R) -> Result<Report, String> {
    let mut total_bytes = 0usize;
    let mut line_number = 0usize;
    let mut input_records = 0usize;
    let mut captures = HashMap::<CorrelationKey, CaptureRecord>::new();
    let mut phases = HashMap::<CorrelationKey, PhaseRecord>::new();

    while let Some(line) = read_bounded_line(&mut reader, &mut total_bytes, line_number + 1)? {
        line_number += 1;
        let value: Value = serde_json::from_slice(&line).map_err(|error| {
            format!(
                "line {line_number}: invalid JSON ({:?} at JSON line {} column {})",
                error.classify(),
                error.line(),
                error.column()
            )
        })?;
        if !value.is_object() {
            return Err(format!(
                "line {line_number}: JSON record must be an object"
            ));
        }
        input_records += 1;

        let Some(event) = value.get("event").and_then(Value::as_str) else {
            continue;
        };
        match event {
            "capture_stopped" => {
                let key = parse_correlation_key(&value, line_number)?;
                let data = parse_data_object(&value, line_number)?;
                let audio_duration_ms =
                    parse_nonnegative_number(data, "audioDurationMs", line_number)?;
                if captures
                    .insert(key, CaptureRecord { audio_duration_ms })
                    .is_some()
                {
                    return Err(format!(
                        "line {line_number}: duplicate capture_stopped correlation"
                    ));
                }
            }
            "dictation_phase_timings" => {
                let key = parse_correlation_key(&value, line_number)?;
                let data = parse_data_object(&value, line_number)?;
                let phase = parse_phase(data, key.clone(), line_number)?;
                if phases.insert(key, phase).is_some() {
                    return Err(format!(
                        "line {line_number}: duplicate dictation_phase_timings correlation"
                    ));
                }
            }
            _ => {}
        }
    }

    let capture_without_phases = captures
        .keys()
        .filter(|key| !phases.contains_key(*key))
        .count();
    let mut groups = BTreeMap::<GroupKey, GroupAccumulator>::new();

    for phase in phases.values() {
        let length_bucket = captures
            .get(&phase.key)
            .map(|capture| length_bucket(capture.audio_duration_ms))
            .unwrap_or(LengthBucket::Unknown);
        let key = GroupKey {
            model: phase.config.model.clone(),
            language: phase.config.language.clone(),
            model_was_warm: phase.config.model_was_warm,
            beam_size: phase.config.beam_size,
            temperature_fallback: phase.config.temperature_fallback,
            vad_enabled: phase.config.vad_enabled,
            outcome: phase.config.outcome.clone(),
            paste_outcome: phase.config.paste_outcome.clone(),
            length_bucket,
        };
        let accumulator = groups.entry(key).or_default();
        accumulator.samples += 1;
        for (index, value) in phase.metrics.iter().enumerate() {
            accumulator.metrics[index].add(value);
        }
    }

    let groups = groups
        .into_iter()
        .map(|(key, accumulator)| {
            Ok(Group {
                model: key.model,
                language: key.language,
                model_was_warm: key.model_was_warm,
                beam_size: key.beam_size,
                temperature_fallback: key.temperature_fallback,
                vad_enabled: key.vad_enabled,
                outcome: key.outcome,
                paste_outcome: key.paste_outcome,
                length_bucket: key.length_bucket,
                samples: accumulator.samples,
                phases: PhaseStatsSet::from_accumulators(accumulator.metrics)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(Report {
        schema_version: 1,
        reporter_version: crate::LONG_VERSION,
        metric_boundary: METRIC_BOUNDARY,
        source_build: None,
        input_records,
        phase_records: phases.len(),
        capture_without_phases,
        groups,
    })
}

impl PhaseStatsSet {
    fn from_accumulators(metrics: [MetricAccumulator; 7]) -> Result<Self, String> {
        let mut metrics = metrics.into_iter();
        Ok(Self {
            key_up_to_capture_stopped_ms: metrics.next().expect("fixed metric count").finish()?,
            model_load_ms: metrics.next().expect("fixed metric count").finish()?,
            key_up_to_model_ready_ms: metrics.next().expect("fixed metric count").finish()?,
            whisper_ms: metrics.next().expect("fixed metric count").finish()?,
            key_up_to_whisper_complete_ms: metrics.next().expect("fixed metric count").finish()?,
            key_up_to_paste_completed_ms: metrics.next().expect("fixed metric count").finish()?,
            total_ms: metrics.next().expect("fixed metric count").finish()?,
        })
    }
}

impl MetricAccumulator {
    fn add(&mut self, value: &MetricValue) {
        self.count += 1;
        match value {
            MetricValue::Missing => self.missing_count += 1,
            MetricValue::Null => self.null_count += 1,
            MetricValue::Numeric(value) => {
                self.numeric_count += 1;
                self.values.push(*value);
            }
        }
    }

    fn finish(self) -> Result<MetricStats, String> {
        let p50_ms = percentile_ms(&self.values, 50).map_err(|_| "invalid percentile input")?;
        let p95_ms = percentile_ms(&self.values, 95).map_err(|_| "invalid percentile input")?;
        Ok(MetricStats {
            count: self.count,
            numeric_count: self.numeric_count,
            null_count: self.null_count,
            missing_count: self.missing_count,
            p50_ms,
            p95_ms,
        })
    }
}

fn parse_phase(
    data: &serde_json::Map<String, Value>,
    key: CorrelationKey,
    line: usize,
) -> Result<PhaseRecord, String> {
    let outcome = required_string(data, "outcome", line)?;
    if !matches!(outcome.as_str(), "no_speech" | "success" | "error") {
        return Err(format!(
            "line {line}: field outcome has an unsupported value"
        ));
    }

    let paste_outcome = optional_paste_outcome(data, line)?;
    let has_config = CONFIG_FIELDS.iter().any(|field| data.contains_key(*field));
    let config = if outcome == "no_speech" && !has_config {
        if paste_outcome.is_some() {
            return Err(format!(
                "line {line}: field pasteOutcome is only valid for success"
            ));
        }
        PhaseConfig {
            model: None,
            language: None,
            model_was_warm: None,
            beam_size: None,
            temperature_fallback: None,
            vad_enabled: None,
            outcome: outcome.clone(),
            paste_outcome: None,
        }
    } else {
        let model = normalize_model(required_string(data, "model", line)?, line)?;
        let language = normalize_language(required_string(data, "language", line)?, line)?;
        let model_was_warm = required_bool(data, "modelWasWarm", line)?;
        let beam_size = required_u32(data, "beamSize", line)?;
        let temperature_fallback = required_bool(data, "temperatureFallback", line)?;
        let vad_enabled = required_bool(data, "vadEnabled", line)?;
        if outcome == "success" && paste_outcome.is_none() {
            return Err(format!(
                "line {line}: field pasteOutcome is required for success"
            ));
        }
        if outcome != "success" && paste_outcome.is_some() {
            return Err(format!(
                "line {line}: field pasteOutcome is only valid for success"
            ));
        }
        PhaseConfig {
            model: Some(model),
            language: Some(language),
            model_was_warm: Some(model_was_warm),
            beam_size: Some(beam_size),
            temperature_fallback: Some(temperature_fallback),
            vad_enabled: Some(vad_enabled),
            outcome: outcome.clone(),
            paste_outcome,
        }
    };

    let metrics = METRIC_FIELDS.map(|field| parse_metric(data, field, line));
    let mut parsed = [
        MetricValue::Missing,
        MetricValue::Missing,
        MetricValue::Missing,
        MetricValue::Missing,
        MetricValue::Missing,
        MetricValue::Missing,
        MetricValue::Missing,
    ];
    for (target, result) in parsed.iter_mut().zip(metrics) {
        *target = result?;
    }

    Ok(PhaseRecord {
        key,
        config,
        metrics: parsed,
    })
}

fn parse_metric(
    data: &serde_json::Map<String, Value>,
    field: MetricField,
    line: usize,
) -> Result<MetricValue, String> {
    let Some(value) = data.get(field.key()) else {
        return Ok(MetricValue::Missing);
    };
    if value.is_null() {
        return Ok(MetricValue::Null);
    }
    Ok(MetricValue::Numeric(parse_nonnegative_number(
        data,
        field.key(),
        line,
    )?))
}

fn parse_correlation_key(value: &Value, line: usize) -> Result<CorrelationKey, String> {
    Ok(CorrelationKey {
        app_session: bounded_identifier(value, "appSession", line)?,
        dictation_session: bounded_identifier(value, "dictationSession", line)?,
    })
}

fn bounded_identifier(value: &Value, field: &str, line: usize) -> Result<String, String> {
    let Some(value) = value.get(field).and_then(Value::as_str) else {
        return Err(format!(
            "line {line}: field {field} must be a non-empty bounded string"
        ));
    };
    if value.trim().is_empty() || value.len() > MAX_IDENTIFIER_BYTES {
        return Err(format!(
            "line {line}: field {field} must be a non-empty bounded string"
        ));
    }
    Ok(value.to_owned())
}

fn parse_data_object(
    value: &Value,
    line: usize,
) -> Result<&serde_json::Map<String, Value>, String> {
    value
        .get("data")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("line {line}: field data must be an object"))
}

fn required_string(
    data: &serde_json::Map<String, Value>,
    field: &str,
    line: usize,
) -> Result<String, String> {
    data.get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("line {line}: field {field} must be a string"))
}

fn required_bool(
    data: &serde_json::Map<String, Value>,
    field: &str,
    line: usize,
) -> Result<bool, String> {
    data.get(field)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("line {line}: field {field} must be a boolean"))
}

fn required_u32(
    data: &serde_json::Map<String, Value>,
    field: &str,
    line: usize,
) -> Result<u32, String> {
    data.get(field)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| format!("line {line}: field {field} must be a non-negative integer"))
}

fn parse_nonnegative_number(
    data: &serde_json::Map<String, Value>,
    field: &str,
    line: usize,
) -> Result<f64, String> {
    let Some(value) = data.get(field).and_then(Value::as_f64) else {
        return Err(format!(
            "line {line}: field {field} must be a finite non-negative number"
        ));
    };
    if !value.is_finite() || value < 0.0 {
        return Err(format!(
            "line {line}: field {field} must be a finite non-negative number"
        ));
    }
    Ok(if value == 0.0 { 0.0 } else { value })
}

fn optional_paste_outcome(
    data: &serde_json::Map<String, Value>,
    line: usize,
) -> Result<Option<String>, String> {
    let Some(value) = data.get("pasteOutcome") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Some(value) = value.as_str() else {
        return Err(format!(
            "line {line}: field pasteOutcome has an unsupported value"
        ));
    };
    if !PASTE_OUTCOMES.contains(&value) {
        return Err(format!(
            "line {line}: field pasteOutcome has an unsupported value"
        ));
    }
    Ok(Some(value.to_owned()))
}

fn normalize_language(value: String, line: usize) -> Result<String, String> {
    let languages = [
        (Language::English, "en"),
        (Language::Swedish, "sv"),
        (Language::Norwegian, "no"),
        (Language::Auto, "auto"),
    ];
    languages
        .into_iter()
        .find(|(language, _)| language.display_name() == value)
        .map(|(_, code)| code.to_owned())
        .ok_or_else(|| format!("line {line}: field language has an unsupported value"))
}

fn normalize_model(value: String, line: usize) -> Result<String, String> {
    let languages = [
        Language::English,
        Language::Swedish,
        Language::Norwegian,
        Language::Auto,
    ];
    let mut known = Vec::new();
    for language in languages {
        for &model in WhisperModel::models_for_language(language) {
            if !known.contains(&model) {
                known.push(model);
            }
        }
    }
    known
        .into_iter()
        .find(|model| model.display_name() == value)
        .map(model_id_string)
        .map(str::to_owned)
        .ok_or_else(|| format!("line {line}: field model has an unsupported value"))
}

fn length_bucket(audio_duration_ms: f64) -> LengthBucket {
    if audio_duration_ms <= 5_000.0 {
        LengthBucket::Short
    } else if audio_duration_ms <= 15_000.0 {
        LengthBucket::Medium
    } else {
        LengthBucket::Long
    }
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    total_bytes: &mut usize,
    line_number: usize,
) -> Result<Option<Vec<u8>>, String> {
    let mut line = Vec::with_capacity(4096);
    let mut ended_with_newline = false;
    loop {
        let buffer = reader
            .fill_buf()
            .map_err(|error| {
                format!(
                    "line {line_number}: input read failed ({:?})",
                    error.kind()
                )
            })?;
        if buffer.is_empty() {
            break;
        }
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(buffer.len(), |index| index + 1);
        if total_bytes
            .checked_add(take)
            .is_none_or(|total| total > MAX_INPUT_BYTES)
        {
            return Err("input exceeds the 32 MiB limit".to_string());
        }
        if line
            .len()
            .checked_add(take)
            .is_none_or(|length| length > MAX_LINE_BYTES + 2)
        {
            return Err(format!(
                "line {line_number}: input line exceeds the 1 MiB limit"
            ));
        }
        line.extend_from_slice(&buffer[..take]);
        reader.consume(take);
        *total_bytes += take;
        if newline.is_some() {
            ended_with_newline = true;
            break;
        }
    }

    if line.is_empty() {
        return Ok(None);
    }
    if ended_with_newline {
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
    }
    if line.len() > MAX_LINE_BYTES {
        return Err(format!(
            "line {line_number}: input line exceeds the 1 MiB limit"
        ));
    }
    Ok(Some(line))
}

fn budget_passes(
    report: &Report,
    length: LengthBucket,
    threshold: f64,
    min_samples: usize,
) -> bool {
    let mut eligible_groups = 0usize;
    for group in &report.groups {
        if group.length_bucket != length
            || group.outcome != "success"
            || group.model_was_warm != Some(true)
            || group.paste_outcome.as_deref() != Some("succeeded")
        {
            continue;
        }
        eligible_groups += 1;
        let metric = &group.phases.key_up_to_paste_completed_ms;
        if metric.numeric_count < min_samples
            || metric.numeric_count != metric.count
            || metric.p95_ms.is_none_or(|p95| p95 > threshold)
        {
            return false;
        }
    }
    eligible_groups > 0
}

fn excluded_warm_success_samples(report: &Report, length: LengthBucket) -> usize {
    report
        .groups
        .iter()
        .filter(|group| {
            group.length_bucket == length
                && group.outcome == "success"
                && group.model_was_warm == Some(true)
                && group.paste_outcome.as_deref() != Some("succeeded")
        })
        .map(|group| group.samples)
        .sum()
}

pub fn run(args: LatencyReportArgs) -> Result<(), DictationError> {
    if args.min_samples == 0 {
        return Err(DictationError::SettingsError(
            "min-samples must be positive".to_string(),
        ));
    }
    if args.budget_length.is_some() != args.max_warm_p95_ms.is_some() {
        return Err(DictationError::SettingsError(
            "budget-length and max-warm-p95-ms must be supplied together".to_string(),
        ));
    }
    if matches!(args.budget_length, Some(LengthBucket::Unknown)) {
        return Err(DictationError::SettingsError(
            "budget-length must be short, medium, or long".to_string(),
        ));
    }
    if let Some(threshold) = args.max_warm_p95_ms {
        if !threshold.is_finite() || threshold < 0.0 {
            return Err(DictationError::SettingsError(
                "max-warm-p95-ms must be finite and non-negative".to_string(),
            ));
        }
    }

    let file = File::open(&args.input).map_err(|error| {
        DictationError::FileDecodeError(format!(
            "latency-report input could not be opened ({:?})",
            error.kind()
        ))
    })?;
    let report = summarize_reader(BufReader::new(file)).map_err(DictationError::FileDecodeError)?;

    let budget = match (args.budget_length, args.max_warm_p95_ms) {
        (Some(length), Some(threshold)) => Some((
            length,
            threshold,
            args.min_samples,
            budget_passes(&report, length, threshold, args.min_samples),
            excluded_warm_success_samples(&report, length),
        )),
        (None, None) => None,
        _ => unreachable!("budget arguments validated above"),
    };

    let mut output = serde_json::to_value(&report).map_err(|_| {
        DictationError::TranscriptionFailed("latency report serialization failed".to_string())
    })?;
    if let Some((length, threshold, min_samples, passed, excluded_samples)) = budget {
        output["budget"] = serde_json::json!({
            "lengthBucket": length,
            "maxWarmP95Ms": threshold,
            "minSamples": min_samples,
            "passed": passed,
            "excludedWarmSuccessSamples": excluded_samples,
        });
    }
    let encoded = serde_json::to_string(&output).map_err(|_| {
        DictationError::TranscriptionFailed("latency report serialization failed".to_string())
    })?;
    println!("{encoded}");

    if output
        .get("budget")
        .and_then(|budget| budget.get("passed"))
        .and_then(Value::as_bool)
        == Some(false)
    {
        return Err(DictationError::SettingsError(
            "latency budget check failed".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn row(event: &str, data: &str, app: &str, session: &str) -> String {
        format!(
            r#"{{"appSession":"{app}","dictationSession":"{session}","event":"{event}","data":{data}}}"#
        )
    }

    fn capture(app: &str, session: &str, duration: u32) -> String {
        row(
            "capture_stopped",
            &format!(r#"{{"audioDurationMs":{duration}}}"#),
            app,
            session,
        )
    }

    fn phase(app: &str, session: &str, outcome: &str, paste: Option<&str>, ms: u32) -> String {
        let paste = paste.map_or(String::new(), |value| {
            format!(r#", "pasteOutcome":"{value}""#)
        });
        row(
            "dictation_phase_timings",
            &format!(
                r#"{{"outcome":"{outcome}","model":"Whisper Base (EN)","language":"English","modelWasWarm":true,"beamSize":0,"temperatureFallback":true,"vadEnabled":false,"keyUpToCaptureStoppedMs":1,"modelLoadMs":2,"keyUpToModelReadyMs":3,"whisperMs":4,"keyUpToWhisperCompleteMs":5,"keyUpToPasteCompletedMs":{ms},"totalMs":{ms}{paste}}}"#
            ),
            app,
            session,
        )
    }

    #[test]
    fn summarizes_order_independent_join_and_normalizes_metadata() {
        let input = format!(
            "{}\n{}\n{}\n",
            phase("app-a", "dict-a", "success", Some("succeeded"), 42),
            r#"{"event":"unrelated","data":{"text":"must not leak"}}"#,
            capture("app-a", "dict-a", 5_000),
        );
        let report = summarize_reader(Cursor::new(input)).unwrap();
        assert_eq!(report.input_records, 3);
        assert_eq!(report.phase_records, 1);
        assert_eq!(report.capture_without_phases, 0);
        assert_eq!(report.groups[0].model.as_deref(), Some("base.en"));
        assert_eq!(report.groups[0].language.as_deref(), Some("en"));
        assert_eq!(report.groups[0].length_bucket, LengthBucket::Short);
        assert_eq!(
            report.groups[0].phases.key_up_to_paste_completed_ms.p95_ms,
            Some(42.0)
        );
    }

    #[test]
    fn accepts_early_no_speech_and_counts_missing_metrics() {
        let input = format!(
            "{}\n{}\n",
            capture("app-a", "dict-a", 0),
            row(
                "dictation_phase_timings",
                r#"{"outcome":"no_speech","keyUpToCaptureStoppedMs":10,"modelLoadMs":null,"whisperMs":null,"keyUpToPasteCompletedMs":null,"totalMs":10}"#,
                "app-a",
                "dict-a",
            ),
        );
        let report = summarize_reader(Cursor::new(input)).unwrap();
        let group = &report.groups[0];
        assert_eq!(group.model, None);
        assert_eq!(group.phases.key_up_to_whisper_complete_ms.missing_count, 1);
        assert_eq!(group.phases.model_load_ms.null_count, 1);
    }

    #[test]
    fn rejects_duplicate_pairs_unknown_names_and_bad_metrics_without_echoing_values() {
        let duplicate = format!(
            "{}\n{}\n",
            capture("app-a", "dict-a", 100),
            capture("app-a", "dict-a", 200)
        );
        let error = summarize_reader(Cursor::new(duplicate)).unwrap_err();
        assert!(error.contains("duplicate capture_stopped"));
        assert!(!error.contains("app-a"));

        let mut unknown = phase("app-a", "dict-a", "success", Some("succeeded"), 1);
        unknown = unknown.replace("Whisper Base (EN)", "secret-model");
        let error = summarize_reader(Cursor::new(unknown)).unwrap_err();
        assert!(error.contains("field model"));
        assert!(!error.contains("secret-model"));

        let bad = row(
            "dictation_phase_timings",
            r#"{"outcome":"error","model":"Whisper Base (EN)","language":"English","modelWasWarm":true,"beamSize":0,"temperatureFallback":true,"vadEnabled":false,"totalMs":true}"#,
            "app-a",
            "dict-a",
        );
        let error = summarize_reader(Cursor::new(bad)).unwrap_err();
        assert!(error.contains("field totalMs"));
        assert!(!error.contains("true"));
    }

    #[test]
    fn enforces_boundaries_and_capture_without_phase_count() {
        let input = format!(
            "{}\n{}\n{}\n{}\n",
            capture("app-a", "dict-a", 5_000),
            capture("app-b", "dict-b", 15_000),
            capture("app-c", "dict-c", 15_001),
            row(
                "dictation_phase_timings",
                r#"{"outcome":"no_speech","keyUpToCaptureStoppedMs":1,"totalMs":1}"#,
                "app-a",
                "dict-a",
            ),
        );
        let report = summarize_reader(Cursor::new(input)).unwrap();
        assert_eq!(report.capture_without_phases, 2);
        assert_eq!(report.groups[0].length_bucket, LengthBucket::Short);

        let too_long = format!(
            "{{\"event\":\"unrelated\",\"data\":\"{}\"}}",
            "x".repeat(MAX_LINE_BYTES)
        );
        assert!(summarize_reader(Cursor::new(too_long)).is_err());
    }

    #[test]
    fn budget_requires_warm_successful_paste_groups_and_numeric_samples() {
        let report = summarize_reader(Cursor::new(format!(
            "{}\n{}\n",
            capture("app-a", "dict-a", 100),
            phase("app-a", "dict-a", "success", Some("succeeded"), 20)
        )))
        .unwrap();
        assert!(budget_passes(&report, LengthBucket::Short, 20.0, 1));
        assert!(!budget_passes(&report, LengthBucket::Short, 19.0, 1));
        assert!(!budget_passes(&report, LengthBucket::Medium, 20.0, 1));
    }
}
