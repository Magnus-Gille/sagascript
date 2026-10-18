//! Sampled language-region detection for long auto-language files (#343).

use super::language_detection::LanguageProbe;
use crate::output::{errln, Out};
use sagascript_core::error::DictationError;
use sagascript_core::settings::Language;
use sagascript_core::transcription::diagnostics::{
    analyze_language_windows, LanguageDetection, LanguageRegionDiagnostics, LanguageWindow,
};

const LANGUAGE_WINDOW_SECONDS: usize = 20;

const LANGUAGE_WINDOW_SAMPLES: usize = LANGUAGE_WINDOW_SECONDS * 16_000;

const LANGUAGE_REDETECTION_MIN_SECONDS: usize = 60;

pub(super) const LANGUAGE_REDETECTION_MAX_WINDOWS: usize = 60;

const LANGUAGE_WINDOW_MIN_RMS: f64 = 0.0015;

/// Re-evaluate only long auto-language files. The sampling cap keeps detection
/// bounded for multi-hour sources; this function is called independently for
/// each source when native batch mode invokes the single-file pipeline.
pub(super) fn detect_file_language_regions(
    out: &Out,
    backend: &dyn LanguageProbe,
    audio: &[f32],
    configured_language: Language,
) -> Result<Option<LanguageRegionDiagnostics>, DictationError> {
    detect_language_regions_with(out, audio, configured_language, |window| {
        backend.detect_language(window)
    })
}

pub(super) fn detect_language_regions_with(
    out: &Out,
    audio: &[f32],
    configured_language: Language,
    mut detect: impl FnMut(&[f32]) -> Result<Option<LanguageDetection>, DictationError>,
) -> Result<Option<LanguageRegionDiagnostics>, DictationError> {
    let windows = language_window_plan(audio.len(), configured_language);
    if windows.is_empty() {
        return Ok(None);
    }

    errln!(
        out,
        "Checking {} sampled window(s) for sustained language changes...",
        windows.len()
    );
    let mut detections = Vec::new();
    for window in windows {
        let samples = &audio[window.start_sample..window.end_sample];
        if samples.len() < LANGUAGE_WINDOW_SAMPLES / 2
            || root_mean_square(samples) < LANGUAGE_WINDOW_MIN_RMS
        {
            continue;
        }
        let Some(detection) = detect(samples)? else {
            continue;
        };
        detections.push(LanguageWindow {
            sequence: window.sequence,
            start: window.start_sample as f64 / 16_000.0,
            end: window.end_sample as f64 / 16_000.0,
            language: detection.language,
            probability: detection.probability,
        });
    }

    Ok(Some(analyze_language_windows(&detections)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PlannedLanguageWindow {
    sequence: usize,
    start_sample: usize,
    end_sample: usize,
}

pub(super) fn language_window_plan(
    audio_samples: usize,
    configured_language: Language,
) -> Vec<PlannedLanguageWindow> {
    if configured_language != Language::Auto
        || audio_samples < LANGUAGE_REDETECTION_MIN_SECONDS * 16_000
    {
        return Vec::new();
    }

    let total_windows = audio_samples.div_ceil(LANGUAGE_WINDOW_SAMPLES);
    let step = total_windows.div_ceil(LANGUAGE_REDETECTION_MAX_WINDOWS);
    (0..total_windows)
        .step_by(step.max(1))
        .enumerate()
        .map(|(sequence, source_window)| {
            let start_sample = source_window * LANGUAGE_WINDOW_SAMPLES;
            PlannedLanguageWindow {
                sequence,
                start_sample,
                end_sample: (start_sample + LANGUAGE_WINDOW_SAMPLES).min(audio_samples),
            }
        })
        .collect()
}

fn root_mean_square(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples
        .iter()
        .map(|&sample| f64::from(sample) * f64::from(sample))
        .sum::<f64>()
        / samples.len() as f64)
        .sqrt()
}
