use std::path::PathBuf;
use std::time::Instant;

use clap::Args;
use serde::Serialize;

use sagascript_core::audio::decoder::decode_audio_file;
use sagascript_core::error::DictationError;
use sagascript_core::settings::{Language, Settings, WhisperModel};
use sagascript_core::transcription::model;
use sagascript_core::transcription::whisper_backend::{DictationTimings, WhisperBackend};
use sagascript_core::transcription::TranscribeOptions;

use super::transcribe::{model_id_string, parse_language};

const SAMPLE_RATE_HZ: f64 = 16_000.0;

/// Benchmark the live dictation inference path on one decoded audio fixture.
#[derive(Debug, Args)]
pub struct BenchmarkDictationArgs {
    /// Audio/video fixture to decode once and transcribe repeatedly.
    #[arg(required = true, value_name = "INPUT")]
    pub input: PathBuf,

    /// Explicit language to use (en, sv, or no).
    #[arg(
        long,
        required = true,
        value_name = "LANG",
        value_parser = parse_benchmark_language
    )]
    pub language: String,

    /// Number of warm in-process samples to collect (2–30).
    #[arg(
        long,
        default_value_t = 5,
        value_parser = clap::value_parser!(u32).range(2..=30)
    )]
    pub iterations: u32,

    /// Fail if any warm sample's total inference time exceeds this many ms.
    #[arg(long, value_name = "MS", value_parser = parse_max_warm_ms)]
    pub max_warm_ms: Option<f64>,

    /// Require this token in every cold and warm transcript without printing text.
    #[arg(long = "expect-word", value_name = "WORD")]
    pub expect_word: Option<String>,
}

#[derive(Debug, Serialize)]
struct TimingPercentiles {
    p50: f64,
    p95: f64,
}

#[derive(Debug, Serialize)]
struct ColdRun {
    model_ms: f64,
    inference_ms: f64,
    total_ms: f64,
    model_cached: bool,
    text_nonempty_count: usize,
}

#[derive(Debug, Serialize)]
struct WarmSamples {
    count: usize,
    text_nonempty_count: usize,
    model_ms: TimingPercentiles,
    inference_ms: TimingPercentiles,
    total_ms: TimingPercentiles,
}

#[derive(Debug, Serialize)]
struct BenchmarkOutput {
    build_version: &'static str,
    language: &'static str,
    model: &'static str,
    duration_seconds: f64,
    decode_duration_ms: f64,
    cold_run: ColdRun,
    warm_samples: WarmSamples,
}

#[derive(Debug, Clone, Copy)]
struct SampleTiming {
    model_ms: f64,
    inference_ms: f64,
    total_ms: f64,
}

pub fn run(args: BenchmarkDictationArgs) -> Result<(), DictationError> {
    let expected_word = match args.expect_word.as_deref().map(str::trim) {
        Some("") => {
            return Err(DictationError::SettingsError(
                "--expect-word must not be empty".to_string(),
            ));
        }
        other => other,
    };
    let language = parse_language(&args.language)?;
    let language_code = language_code(language);
    let model = WhisperModel::recommended(language);

    // This command is deliberately read-only with respect to settings and
    // model storage. A missing recommended model is a clear preflight error;
    // benchmark runs must never download it implicitly.
    if !model::is_model_downloaded(model) {
        return Err(DictationError::TranscriptionFailed(format!(
            "Recommended model '{}' is not downloaded. Run: sagascript download-model {}",
            model.display_name(),
            model_id_string(model)
        )));
    }

    let decode_started = Instant::now();
    let audio = decode_audio_file(&args.input)?;
    let decode_duration_ms = decode_started.elapsed().as_secs_f64() * 1000.0;
    if audio.is_empty() {
        return Err(DictationError::FileDecodeError(
            "Input decoded to no audio samples".to_string(),
        ));
    }
    let duration_seconds = audio.len() as f64 / SAMPLE_RATE_HZ;

    // Settings::default is the live dictation baseline. In particular this
    // keeps greedy beam_size=0 and temperature fallback enabled, while
    // explicitly leaving private glossary/VAD state out of the fixture run.
    let defaults = Settings::default();
    let options = TranscribeOptions {
        prompt: None,
        beam_size: defaults.beam_size,
        temperature_fallback: defaults.temperature_fallback,
        vad_model_path: None,
        segment_timestamps: false,
        parallel_chunks: 1,
    };

    let backend = WhisperBackend::new();
    let mut validation_error: Option<String> = None;

    let cold_started = Instant::now();
    let mut cold_timings = DictationTimings::default();
    let cold_text = backend.transcribe_live_dictation(
        model,
        &audio,
        language,
        &options,
        &mut cold_timings,
    )?;
    let cold_total_ms = cold_started.elapsed().as_secs_f64() * 1000.0;
    validate_text(
        "cold run",
        &cold_text,
        expected_word,
        &mut validation_error,
    );

    let cold_run = ColdRun {
        model_ms: cold_timings.model_ms,
        inference_ms: cold_timings.inference_ms,
        total_ms: cold_total_ms,
        model_cached: cold_timings.model_cached,
        text_nonempty_count: usize::from(!cold_text.trim().is_empty()),
    };

    let mut samples = Vec::with_capacity(args.iterations as usize);
    let mut warm_text_nonempty_count = 0usize;
    let mut budget_exceeded = None;
    for iteration in 0..args.iterations {
        let started = Instant::now();
        let mut timings = DictationTimings::default();
        let text = backend.transcribe_live_dictation(
            model,
            &audio,
            language,
            &options,
            &mut timings,
        )?;
        let total_ms = started.elapsed().as_secs_f64() * 1000.0;
        let text_nonempty = !text.trim().is_empty();
        warm_text_nonempty_count += usize::from(text_nonempty);
        validate_text(
            &format!("warm run {}", iteration + 1),
            &text,
            expected_word,
            &mut validation_error,
        );
        if exceeds_budget(total_ms, args.max_warm_ms) && budget_exceeded.is_none() {
            budget_exceeded = Some((
                iteration + 1,
                total_ms,
                args.max_warm_ms.expect("budget is present after exceeds_budget"),
            ));
        }
        samples.push(SampleTiming {
            model_ms: timings.model_ms,
            inference_ms: timings.inference_ms,
            total_ms,
        });
    }

    let output = BenchmarkOutput {
        build_version: super::LONG_VERSION,
        language: language_code,
        model: model_id_string(model),
        duration_seconds,
        decode_duration_ms,
        cold_run,
        warm_samples: WarmSamples {
            count: samples.len(),
            text_nonempty_count: warm_text_nonempty_count,
            model_ms: percentiles(&samples, |sample| sample.model_ms),
            inference_ms: percentiles(&samples, |sample| sample.inference_ms),
            total_ms: percentiles(&samples, |sample| sample.total_ms),
        },
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&output).expect("benchmark output serializes")
    );

    if let Some((iteration, elapsed_ms, max_ms)) = budget_exceeded {
        return Err(DictationError::TranscriptionFailed(format!(
            "Warm run {iteration} exceeded --max-warm-ms ({elapsed_ms:.2} ms > {max_ms:.2} ms)"
        )));
    }
    if let Some(error) = validation_error {
        return Err(DictationError::TranscriptionFailed(error));
    }

    Ok(())
}

fn language_code(language: Language) -> &'static str {
    match language {
        Language::English => "en",
        Language::Swedish => "sv",
        Language::Norwegian => "no",
        Language::Auto => unreachable!("benchmark language parser excludes auto"),
    }
}

fn parse_benchmark_language(value: &str) -> Result<String, String> {
    match value {
        "en" | "sv" | "no" => Ok(value.to_string()),
        _ => Err("language must be one of: en, sv, no".to_string()),
    }
}

fn parse_max_warm_ms(value: &str) -> Result<f64, String> {
    let parsed = value
        .parse::<f64>()
        .map_err(|_| "max-warm-ms must be a finite non-negative number".to_string())?;
    if parsed.is_finite() && parsed >= 0.0 {
        Ok(parsed)
    } else {
        Err("max-warm-ms must be a finite non-negative number".to_string())
    }
}

fn exceeds_budget(total_ms: f64, max_warm_ms: Option<f64>) -> bool {
    max_warm_ms.is_some_and(|max_ms| total_ms > max_ms)
}

fn contains_expected_token(text: &str, expected_word: &str) -> bool {
    let expected_lower = expected_word.to_lowercase();
    text.split(|character: char| !character.is_alphanumeric())
        .any(|token| !token.is_empty() && token.to_lowercase() == expected_lower)
}

fn validate_text(
    run_name: &str,
    text: &str,
    expected_word: Option<&str>,
    validation_error: &mut Option<String>,
) {
    if text.trim().is_empty() {
        if validation_error.is_none() {
            *validation_error = Some(format!("{run_name} returned empty text"));
        }
        return;
    }
    if let Some(expected_word) = expected_word.filter(|word| !word.trim().is_empty()) {
        if !contains_expected_token(text, expected_word) && validation_error.is_none() {
            *validation_error = Some(format!("{run_name} did not contain the expected word"));
        }
    }
}

fn percentiles(
    samples: &[SampleTiming],
    value: impl Fn(&SampleTiming) -> f64,
) -> TimingPercentiles {
    let mut values: Vec<f64> = samples.iter().map(value).collect();
    debug_assert!(!values.is_empty(), "iterations validation guarantees samples");
    values.sort_by(f64::total_cmp);
    TimingPercentiles {
        p50: percentile_sorted(&values, 0.50),
        p95: percentile_sorted(&values, 0.95),
    }
}

fn percentile_sorted(values: &[f64], quantile: f64) -> f64 {
    debug_assert!(!values.is_empty());
    let position = quantile * (values.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        values[lower]
    } else {
        let fraction = position - lower as f64;
        values[lower] + (values[upper] - values[lower]) * fraction
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct BenchmarkTestCli {
        #[command(flatten)]
        args: BenchmarkDictationArgs,
    }

    #[test]
    fn clap_contract_requires_language_and_bounds_iterations() {
        let parsed = BenchmarkTestCli::try_parse_from([
            "sagascript",
            "fixture.wav",
            "--language",
            "en",
        ])
        .expect("valid benchmark arguments should parse");
        assert_eq!(parsed.args.input, PathBuf::from("fixture.wav"));
        assert_eq!(parsed.args.language, "en");
        assert_eq!(parsed.args.iterations, 5);

        for value in ["2", "30"] {
            let result = BenchmarkTestCli::try_parse_from([
                "sagascript",
                "fixture.wav",
                "--language",
                "en",
                "--iterations",
                value,
            ])
            .expect("inclusive iteration bounds should parse");
            let expected = value.parse::<u32>().expect("test bound is numeric");
            assert_eq!(result.args.iterations, expected);
        }

        for value in ["1", "31"] {
            let result = BenchmarkTestCli::try_parse_from([
                "sagascript",
                "fixture.wav",
                "--language",
                "en",
                "--iterations",
                value,
            ]);
            assert!(result.is_err(), "iterations={value} must be rejected");
        }

        let missing_language = BenchmarkTestCli::try_parse_from(["sagascript", "fixture.wav"]);
        assert!(missing_language.is_err(), "--language must be required");
    }

    #[test]
    fn clap_contract_preserves_expected_word_without_exposing_transcript() {
        let parsed = BenchmarkTestCli::try_parse_from([
            "sagascript",
            "fixture.wav",
            "--language",
            "sv",
            "--expect-word",
            "country",
        ])
        .expect("expected-word gate should parse");
        assert_eq!(parsed.args.expect_word.as_deref(), Some("country"));
    }

    #[test]
    fn benchmark_language_parser_accepts_only_public_codes() {
        assert_eq!(parse_benchmark_language("en").unwrap(), "en");
        assert_eq!(parse_benchmark_language("sv").unwrap(), "sv");
        assert_eq!(parse_benchmark_language("no").unwrap(), "no");
        assert!(parse_benchmark_language("auto").is_err());
        assert!(parse_benchmark_language("English").is_err());
    }

    #[test]
    fn warm_budget_parser_rejects_invalid_values() {
        assert_eq!(parse_max_warm_ms("12.5").unwrap(), 12.5);
        assert_eq!(parse_max_warm_ms("0").unwrap(), 0.0);
        assert!(parse_max_warm_ms("-1").is_err());
        assert!(parse_max_warm_ms("NaN").is_err());
        assert!(parse_max_warm_ms("inf").is_err());
    }

    #[test]
    fn warm_budget_is_an_inclusive_upper_bound() {
        assert!(!exceeds_budget(25.0, None));
        assert!(!exceeds_budget(25.0, Some(25.0)));
        assert!(exceeds_budget(25.001, Some(25.0)));
    }

    #[test]
    fn percentile_calculation_interpolates_p50_and_p95() {
        let samples = [
            SampleTiming {
                model_ms: 10.0,
                inference_ms: 0.0,
                total_ms: 0.0,
            },
            SampleTiming {
                model_ms: 20.0,
                inference_ms: 0.0,
                total_ms: 0.0,
            },
            SampleTiming {
                model_ms: 30.0,
                inference_ms: 0.0,
                total_ms: 0.0,
            },
            SampleTiming {
                model_ms: 40.0,
                inference_ms: 0.0,
                total_ms: 0.0,
            },
        ];
        let result = percentiles(&samples, |sample| sample.model_ms);
        assert_eq!(result.p50, 25.0);
        assert_eq!(result.p95, 38.5);
    }

    #[test]
    fn percentile_is_order_independent_and_p95_does_not_drop_max_for_two_samples() {
        let samples = [
            SampleTiming {
                model_ms: 100.0,
                inference_ms: 0.0,
                total_ms: 0.0,
            },
            SampleTiming {
                model_ms: 10.0,
                inference_ms: 0.0,
                total_ms: 0.0,
            },
        ];
        let result = percentiles(&samples, |sample| sample.model_ms);
        assert_eq!(result.p50, 55.0);
        assert_eq!(result.p95, 95.5);
    }

    #[test]
    fn empty_and_expected_word_validation_is_recorded_without_leaking_text() {
        let mut error = None;
        validate_text("warm run 1", "", Some("secret"), &mut error);
        assert_eq!(error.as_deref(), Some("warm run 1 returned empty text"));

        let mut error = None;
        validate_text("warm run 2", "a public sentence", Some("secret"), &mut error);
        assert_eq!(
            error.as_deref(),
            Some("warm run 2 did not contain the expected word")
        );
    }

    #[test]
    fn expected_word_matches_unicode_tokens_with_punctuation_and_case() {
        for (text, expected) in [
            ("Hello, WORLD!", "world"),
            ("ÅNGSTRÖM — klart", "ångström"),
            ("Hej världen.", "VÄRLDEN"),
        ] {
            assert!(
                contains_expected_token(text, expected),
                "{expected:?} should match a whole token in {text:?}"
            );
        }
        assert!(contains_expected_token("hello-world", "world"));
    }

    #[test]
    fn expected_word_does_not_match_an_embedded_partial_token() {
        assert!(!contains_expected_token("countryside", "country"));
        assert!(!contains_expected_token("concatenate", "cat"));
    }

    #[test]
    fn empty_expect_word_is_rejected_before_model_or_input_access() {
        let result = run(BenchmarkDictationArgs {
            input: PathBuf::from("missing-fixture.wav"),
            language: "en".to_string(),
            iterations: 2,
            max_warm_ms: None,
            expect_word: Some(" \t".to_string()),
        });

        assert!(matches!(
            result,
            Err(DictationError::SettingsError(message))
                if message == "--expect-word must not be empty"
        ));
    }

    #[test]
    fn serialized_report_has_expected_schema_without_input_or_transcript() {
        let report = BenchmarkOutput {
            build_version: super::super::LONG_VERSION,
            language: "en",
            model: "base.en",
            duration_seconds: 11.0,
            decode_duration_ms: 4.0,
            cold_run: ColdRun {
                model_ms: 100.0,
                inference_ms: 50.0,
                total_ms: 150.0,
                model_cached: false,
                text_nonempty_count: 1,
            },
            warm_samples: WarmSamples {
                count: 5,
                text_nonempty_count: 5,
                model_ms: TimingPercentiles { p50: 0.0, p95: 0.0 },
                inference_ms: TimingPercentiles {
                    p50: 45.0,
                    p95: 55.0,
                },
                total_ms: TimingPercentiles {
                    p50: 45.0,
                    p95: 55.0,
                },
            },
        };
        let json = serde_json::to_value(report).expect("benchmark report should serialize");

        assert_eq!(json["build_version"], super::super::LONG_VERSION);
        assert_eq!(json["language"], "en");
        assert_eq!(json["model"], "base.en");
        assert_eq!(json["duration_seconds"], 11.0);
        assert_eq!(json["decode_duration_ms"], 4.0);
        assert_eq!(json["cold_run"]["text_nonempty_count"], 1);
        assert_eq!(json["warm_samples"]["count"], 5);
        assert_eq!(json["warm_samples"]["inference_ms"]["p95"], 55.0);
        assert!(json.get("input").is_none());
        assert!(json.get("path").is_none());
        assert!(json.get("transcript").is_none());
        assert!(json.get("text").is_none());
        let serialized = json.to_string();
        assert!(!serialized.contains("fixture.wav"));
        assert!(!serialized.contains("private transcript"));
    }
}
