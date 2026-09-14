use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::settings::Language;

use super::TranscriptSegment;

pub const MATERIAL_GAP_SECONDS: f64 = 5.0;
const SAMPLE_RATE_HZ: f64 = 16_000.0;
const FRAME_SAMPLES: usize = 320;
const MIN_SPEECH_RMS: f64 = 0.0015;
const MIN_UNCOVERED_SPEECH_SECONDS: f64 = 1.0;
const MIN_UNCOVERED_SPEECH_RATIO: f64 = 0.10;
const STRONG_LANGUAGE_PROBABILITY: f32 = 0.90;
const REPETITION_MAX_NGRAM_TOKENS: usize = 4;
const REPETITION_MIN_OCCURRENCES: usize = 8;
const REPETITION_MIN_TOKENS: usize = 16;
const REPETITION_MIN_DURATION_SECONDS: f64 = 20.0;
const REPETITION_MAX_UNIQUE_TOKEN_RATIO: f64 = 0.25;
const LANGUAGE_REGION_MIN_WINDOWS: usize = 2;
const LANGUAGE_REGION_MIN_DURATION_SECONDS: f64 = 40.0;
const LANGUAGE_REGION_MIN_PROBABILITY: f32 = 0.90;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UncoveredSpan {
    pub start: f64,
    pub end: f64,
    pub duration: f64,
    pub speech_seconds: f64,
    pub speech_ratio: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptionWarning {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CoverageDiagnostics {
    pub coverage_ratio: f64,
    pub uncovered_spans: Vec<UncoveredSpan>,
    pub warnings: Vec<TranscriptionWarning>,
}

/// Reusable audio-only input for transcript coverage checks.
///
/// Persisting this compact frame map lets callers re-check coverage after
/// changing timestamp grouping without decoding the source audio again.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CoverageProfile {
    audio_samples: usize,
    speech_frames: Vec<bool>,
}

impl CoverageProfile {
    pub fn from_audio(audio: &[f32]) -> Self {
        Self {
            audio_samples: audio.len(),
            speech_frames: detect_speech_frames(audio),
        }
    }

    pub fn duration_seconds(&self) -> f64 {
        self.audio_samples as f64 / SAMPLE_RATE_HZ
    }

    pub fn validate(&self) -> bool {
        self.speech_frames.len() == self.audio_samples.div_ceil(FRAME_SAMPLES)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RepetitionSpan {
    pub start: f64,
    pub end: f64,
    pub duration: f64,
    pub pattern: String,
    pub repetitions: usize,
    pub token_count: usize,
    pub unique_token_ratio: f64,
    pub min_no_speech_prob: f32,
    pub max_no_speech_prob: f32,
    pub segment_indices: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RepetitionDiagnostics {
    pub spans: Vec<RepetitionSpan>,
    pub warnings: Vec<TranscriptionWarning>,
}

impl RepetitionDiagnostics {
    pub fn quarantines_segment(&self, segment_index: usize) -> bool {
        self.spans
            .iter()
            .any(|span| span.segment_indices.contains(&segment_index))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LanguageDetection {
    pub language: String,
    pub probability: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LanguageWindow {
    pub sequence: usize,
    pub start: f64,
    pub end: f64,
    pub language: String,
    pub probability: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LanguageRegion {
    pub start: f64,
    pub end: f64,
    pub language: String,
    pub probability: f32,
    pub window_count: usize,
    pub stable: bool,
    pub first_sequence: usize,
    pub last_sequence: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LanguageRegionDiagnostics {
    pub regions: Vec<LanguageRegion>,
    pub warnings: Vec<TranscriptionWarning>,
}

pub fn analyze_coverage(audio: &[f32], segments: &[TranscriptSegment]) -> CoverageDiagnostics {
    analyze_coverage_profile(&CoverageProfile::from_audio(audio), segments)
}

pub fn analyze_coverage_profile(
    profile: &CoverageProfile,
    segments: &[TranscriptSegment],
) -> CoverageDiagnostics {
    let duration = profile.duration_seconds();
    let speech_frames = &profile.speech_frames;
    let intervals = merged_segment_intervals(segments, duration);

    let mut total_speech_frames = 0usize;
    let mut covered_speech_frames = 0usize;
    for (index, &is_speech) in speech_frames.iter().enumerate() {
        if !is_speech {
            continue;
        }
        total_speech_frames += 1;
        let midpoint = (index * FRAME_SAMPLES + FRAME_SAMPLES / 2) as f64 / SAMPLE_RATE_HZ;
        if intervals
            .iter()
            .any(|&(start, end)| midpoint >= start && midpoint < end)
        {
            covered_speech_frames += 1;
        }
    }

    let coverage_ratio = if total_speech_frames == 0 {
        1.0
    } else {
        covered_speech_frames as f64 / total_speech_frames as f64
    };

    let mut uncovered_spans = Vec::new();
    for (start, end) in uncovered_intervals(&intervals, duration) {
        let gap_duration = end - start;
        if gap_duration < MATERIAL_GAP_SECONDS {
            continue;
        }
        let speech_frame_count = speech_frames
            .iter()
            .enumerate()
            .filter(|&(index, is_speech)| {
                if !*is_speech {
                    return false;
                }
                let midpoint = (index * FRAME_SAMPLES + FRAME_SAMPLES / 2) as f64 / SAMPLE_RATE_HZ;
                midpoint >= start && midpoint < end
            })
            .count();
        let speech_seconds = (speech_frame_count * FRAME_SAMPLES) as f64 / SAMPLE_RATE_HZ;
        let speech_ratio = if gap_duration > 0.0 {
            (speech_seconds / gap_duration).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if speech_seconds >= MIN_UNCOVERED_SPEECH_SECONDS
            && speech_ratio >= MIN_UNCOVERED_SPEECH_RATIO
        {
            uncovered_spans.push(UncoveredSpan {
                start,
                end,
                duration: gap_duration,
                speech_seconds,
                speech_ratio,
            });
        }
    }

    let warnings = uncovered_spans
        .iter()
        .map(|span| TranscriptionWarning {
            code: "uncovered_speech".to_string(),
            message: format!(
                "Transcript has no segment from {:.2}s to {:.2}s ({:.2}s), but {:.2}s of that span appears to contain speech.",
                span.start, span.end, span.duration, span.speech_seconds
            ),
            start: Some(span.start),
            end: Some(span.end),
        })
        .collect();

    CoverageDiagnostics {
        coverage_ratio,
        uncovered_spans,
        warnings,
    }
}

/// Detect exact, sustained ordinary-word loops across Whisper segments.
///
/// The thresholds deliberately require both many repetitions and a long time
/// span. Short rhetorical repetition remains trusted, while a long loop is
/// quarantined even when Whisper reports a contradictory low `no_speech_prob`.
pub fn analyze_repetition(segments: &[TranscriptSegment]) -> RepetitionDiagnostics {
    let tokens = transcript_tokens(segments);
    let mut spans = Vec::new();
    let mut token_index = 0usize;

    while token_index < tokens.len() {
        let mut best: Option<RepetitionCandidate> = None;
        let remaining = tokens.len() - token_index;
        let max_ngram = REPETITION_MAX_NGRAM_TOKENS.min(remaining / REPETITION_MIN_OCCURRENCES);

        for ngram_len in 1..=max_ngram {
            let pattern = &tokens[token_index..token_index + ngram_len];
            let mut repetitions = 1usize;
            while token_index + (repetitions + 1) * ngram_len <= tokens.len() {
                let next_start = token_index + repetitions * ngram_len;
                let next_end = next_start + ngram_len;
                if tokens[next_start..next_end]
                    .iter()
                    .map(|token| token.word.as_str())
                    .eq(pattern.iter().map(|token| token.word.as_str()))
                {
                    repetitions += 1;
                } else {
                    break;
                }
            }

            let repeated_tokens = repetitions * ngram_len;
            if repetitions < REPETITION_MIN_OCCURRENCES || repeated_tokens < REPETITION_MIN_TOKENS {
                continue;
            }

            let candidate_tokens = &tokens[token_index..token_index + repeated_tokens];
            let unique_tokens = candidate_tokens
                .iter()
                .map(|token| token.word.as_str())
                .collect::<HashSet<_>>()
                .len();
            let unique_token_ratio = unique_tokens as f64 / repeated_tokens as f64;
            if unique_token_ratio > REPETITION_MAX_UNIQUE_TOKEN_RATIO {
                continue;
            }

            let first_segment = candidate_tokens[0].segment_index;
            let last_segment = candidate_tokens[repeated_tokens - 1].segment_index;
            let start = segments[first_segment].start;
            let end = segments[last_segment].end;
            if !start.is_finite()
                || !end.is_finite()
                || end - start < REPETITION_MIN_DURATION_SECONDS
            {
                continue;
            }

            let candidate = RepetitionCandidate {
                ngram_len,
                repetitions,
                repeated_tokens,
                unique_token_ratio,
            };
            if best.as_ref().is_none_or(|current| {
                candidate.repeated_tokens > current.repeated_tokens
                    || (candidate.repeated_tokens == current.repeated_tokens
                        && candidate.ngram_len < current.ngram_len)
            }) {
                best = Some(candidate);
            }
        }

        let Some(candidate) = best else {
            token_index += 1;
            continue;
        };
        let candidate_tokens = &tokens[token_index..token_index + candidate.repeated_tokens];
        let mut segment_indices = Vec::new();
        for token in candidate_tokens {
            if segment_indices.last() != Some(&token.segment_index) {
                segment_indices.push(token.segment_index);
            }
        }
        let start = segments[segment_indices[0]].start;
        let end = segments[*segment_indices.last().expect("candidate has a segment")].end;
        let min_no_speech_prob = segment_indices
            .iter()
            .map(|&index| segments[index].no_speech_prob)
            .min_by(f32::total_cmp)
            .unwrap_or(0.0);
        let max_no_speech_prob = segment_indices
            .iter()
            .map(|&index| segments[index].no_speech_prob)
            .max_by(f32::total_cmp)
            .unwrap_or(0.0);
        let pattern = candidate_tokens[..candidate.ngram_len]
            .iter()
            .map(|token| token.word.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        spans.push(RepetitionSpan {
            start,
            end,
            duration: end - start,
            pattern,
            repetitions: candidate.repetitions,
            token_count: candidate.repeated_tokens,
            unique_token_ratio: candidate.unique_token_ratio,
            min_no_speech_prob,
            max_no_speech_prob,
            segment_indices,
        });
        token_index += candidate.repeated_tokens;
    }

    let warnings = spans
        .iter()
        .map(|span| TranscriptionWarning {
            code: "repetitive_hallucination".to_string(),
            message: format!(
                "Quarantined suspected degenerate output from {:.2}s to {:.2}s: pattern {:?} repeated {} times across {} tokens (unique-token ratio {:.3}). Repetition evidence overrides contradictory no_speech_prob {:.3}..{:.3}.",
                span.start,
                span.end,
                span.pattern,
                span.repetitions,
                span.token_count,
                span.unique_token_ratio,
                span.min_no_speech_prob,
                span.max_no_speech_prob
            ),
            start: Some(span.start),
            end: Some(span.end),
        })
        .collect();

    RepetitionDiagnostics { spans, warnings }
}

/// Consolidate sampled language detections into conservative sustained regions.
/// Missing sequence numbers (for example, a skipped silent window) break a
/// region. A single foreign-language sample remains visible but unstable and
/// cannot trigger a mixed-language warning.
pub fn analyze_language_windows(windows: &[LanguageWindow]) -> LanguageRegionDiagnostics {
    let mut sorted = windows.to_vec();
    sorted.sort_by(|left, right| {
        left.sequence
            .cmp(&right.sequence)
            .then_with(|| left.start.total_cmp(&right.start))
    });

    let mut regions: Vec<LanguageRegion> = Vec::new();
    for window in sorted {
        if !window.start.is_finite()
            || !window.end.is_finite()
            || window.end <= window.start
            || !window.probability.is_finite()
        {
            continue;
        }
        if let Some(region) = regions.last_mut() {
            if region.language.eq_ignore_ascii_case(&window.language)
                && window.sequence == region.last_sequence + 1
            {
                let probability_sum = region.probability * region.window_count as f32;
                region.end = region.end.max(window.end);
                region.window_count += 1;
                region.last_sequence = window.sequence;
                region.probability =
                    (probability_sum + window.probability) / region.window_count as f32;
                continue;
            }
        }
        regions.push(LanguageRegion {
            start: window.start,
            end: window.end,
            language: window.language.to_lowercase(),
            probability: window.probability,
            window_count: 1,
            stable: false,
            first_sequence: window.sequence,
            last_sequence: window.sequence,
        });
    }

    for region in &mut regions {
        region.stable = is_supported_language(&region.language)
            && region.window_count >= LANGUAGE_REGION_MIN_WINDOWS
            && region.end - region.start >= LANGUAGE_REGION_MIN_DURATION_SECONDS
            && region.probability >= LANGUAGE_REGION_MIN_PROBABILITY;
    }

    let stable_regions: Vec<&LanguageRegion> =
        regions.iter().filter(|region| region.stable).collect();
    let warnings = stable_regions
        .first()
        .map(|initial| {
            stable_regions
                .iter()
                .skip(1)
                .filter(|region| !region.language.eq_ignore_ascii_case(&initial.language))
                .map(|region| TranscriptionWarning {
                    code: "mixed_language_audio".to_string(),
                    message: format!(
                        "Auto language detection found a sustained change from {} to {} between {:.2}s and {:.2}s (p={:.3}, {} windows). Mixed-language transcription uses one global decoder language; split the recording or transcribe each part with an explicit --language.",
                        initial.language,
                        region.language,
                        region.start,
                        region.end,
                        region.probability,
                        region.window_count
                    ),
                    start: Some(region.start),
                    end: Some(region.end),
                })
                .collect()
        })
        .unwrap_or_default();

    LanguageRegionDiagnostics { regions, warnings }
}

fn is_supported_language(language: &str) -> bool {
    matches!(language, "en" | "sv" | "no" | "fi")
}

#[derive(Debug)]
struct TranscriptToken {
    word: String,
    segment_index: usize,
}

#[derive(Debug)]
struct RepetitionCandidate {
    ngram_len: usize,
    repetitions: usize,
    repeated_tokens: usize,
    unique_token_ratio: f64,
}

fn transcript_tokens(segments: &[TranscriptSegment]) -> Vec<TranscriptToken> {
    let mut tokens = Vec::new();
    for (segment_index, segment) in segments.iter().enumerate() {
        for raw_word in segment
            .text
            .split(|character: char| {
                !character.is_alphanumeric() && character != '\'' && character != '’'
            })
            .filter(|word| !word.is_empty())
        {
            let word = raw_word.trim_matches(['\'', '’']).to_lowercase();
            if !word.is_empty() {
                tokens.push(TranscriptToken {
                    word,
                    segment_index,
                });
            }
        }
    }
    tokens
}

pub fn language_mismatch_warning(
    configured: Language,
    detected: &LanguageDetection,
) -> Option<TranscriptionWarning> {
    let configured_code = configured.whisper_code()?;
    if detected.probability < STRONG_LANGUAGE_PROBABILITY
        || detected.language.eq_ignore_ascii_case(configured_code)
    {
        return None;
    }

    let detected_name = language_name(&detected.language);
    let recommendation = match detected.language.as_str() {
        "en" | "sv" | "no" | "fi" => format!("--language {}", detected.language),
        _ => "--language auto".to_string(),
    };
    Some(TranscriptionWarning {
        code: "language_mismatch".to_string(),
        message: format!(
            "Configured language is {} (`{configured_code}`), but the audio strongly appears to be {detected_name} (`{}`, p={:.3}). Re-run with `{recommendation}`; the saved setting was not changed.",
            configured.display_name(),
            detected.language,
            detected.probability
        ),
        start: None,
        end: None,
    })
}

fn detect_speech_frames(audio: &[f32]) -> Vec<bool> {
    let rms_values: Vec<f64> = audio
        .chunks(FRAME_SAMPLES)
        .map(|frame| {
            let mean_square = frame
                .iter()
                .map(|&sample| f64::from(sample) * f64::from(sample))
                .sum::<f64>()
                / frame.len().max(1) as f64;
            mean_square.sqrt()
        })
        .collect();
    if rms_values.is_empty() {
        return Vec::new();
    }

    let mut sorted = rms_values.clone();
    sorted.sort_by(f64::total_cmp);
    let low = percentile(&sorted, 0.10);
    let high = percentile(&sorted, 0.90);
    if high < MIN_SPEECH_RMS {
        return vec![false; rms_values.len()];
    }

    let dynamic_range = (high - low).max(0.0);
    let threshold = if dynamic_range >= MIN_SPEECH_RMS {
        low + dynamic_range * 0.25
    } else {
        high * 0.5
    }
    .max(high * 0.15)
    .max(MIN_SPEECH_RMS);

    rms_values.into_iter().map(|rms| rms >= threshold).collect()
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index]
}

fn merged_segment_intervals(segments: &[TranscriptSegment], duration: f64) -> Vec<(f64, f64)> {
    let mut intervals: Vec<(f64, f64)> = segments
        .iter()
        .filter_map(|segment| {
            if !segment.start.is_finite() || !segment.end.is_finite() {
                return None;
            }
            let start = segment.start.clamp(0.0, duration);
            let end = segment.end.clamp(start, duration);
            (end > start).then_some((start, end))
        })
        .collect();
    intervals.sort_by(|left, right| left.0.total_cmp(&right.0));

    let mut merged: Vec<(f64, f64)> = Vec::with_capacity(intervals.len());
    for (start, end) in intervals {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
}

fn uncovered_intervals(intervals: &[(f64, f64)], duration: f64) -> Vec<(f64, f64)> {
    let mut gaps = Vec::new();
    let mut cursor = 0.0;
    for &(start, end) in intervals {
        if start > cursor {
            gaps.push((cursor, start));
        }
        cursor = cursor.max(end);
    }
    if cursor < duration {
        gaps.push((cursor, duration));
    }
    gaps
}

fn language_name(code: &str) -> &str {
    match code {
        "en" => "English",
        "sv" => "Swedish",
        "no" => "Norwegian",
        "fi" => "Finnish",
        _ => code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: usize = 16_000;

    fn segment(start: f64, end: f64) -> TranscriptSegment {
        TranscriptSegment {
            start,
            end,
            text: " speech".to_string(),
            avg_logprob: Some(-0.1),
            no_speech_prob: 0.0,
        }
    }

    fn voiced_audio(duration: f64, voiced_spans: &[(f64, f64)]) -> Vec<f32> {
        let sample_count = (duration * SAMPLE_RATE as f64) as usize;
        (0..sample_count)
            .map(|index| {
                let seconds = index as f64 / SAMPLE_RATE as f64;
                if voiced_spans
                    .iter()
                    .any(|&(start, end)| seconds >= start && seconds < end)
                {
                    let phase = seconds * 220.0 * std::f64::consts::TAU;
                    (phase.sin() * 0.1) as f32
                } else {
                    0.0
                }
            })
            .collect()
    }

    #[test]
    fn reports_material_uncovered_speech_across_whisper_windows() {
        let audio = voiced_audio(70.0, &[(0.0, 70.0)]);
        let diagnostics = analyze_coverage(&audio, &[segment(0.0, 25.0), segment(55.0, 70.0)]);

        assert_eq!(diagnostics.uncovered_spans.len(), 1);
        let gap = &diagnostics.uncovered_spans[0];
        assert_eq!((gap.start, gap.end, gap.duration), (25.0, 55.0, 30.0));
        assert!(gap.speech_ratio > 0.9);
        assert!(diagnostics.coverage_ratio < 0.6);
        assert_eq!(diagnostics.warnings[0].code, "uncovered_speech");
    }

    #[test]
    fn persisted_coverage_profile_matches_direct_audio_analysis() {
        let audio = voiced_audio(70.0, &[(0.0, 70.0)]);
        let segments = [segment(0.0, 25.0), segment(55.0, 70.0)];
        let direct = analyze_coverage(&audio, &segments);
        let encoded = serde_json::to_string(&CoverageProfile::from_audio(&audio)).unwrap();
        let profile: CoverageProfile = serde_json::from_str(&encoded).unwrap();

        assert!(profile.validate());
        assert_eq!(analyze_coverage_profile(&profile, &segments), direct);
    }

    #[test]
    fn ignores_material_timestamp_gap_that_is_silent() {
        let audio = voiced_audio(20.0, &[(0.0, 6.0), (14.0, 20.0)]);
        let diagnostics = analyze_coverage(&audio, &[segment(0.0, 6.0), segment(14.0, 20.0)]);

        assert!(diagnostics.uncovered_spans.is_empty());
        assert!(diagnostics.warnings.is_empty());
        assert!(diagnostics.coverage_ratio > 0.99);
    }

    #[test]
    fn does_not_warn_for_short_uncovered_speech() {
        let audio = voiced_audio(12.0, &[(0.0, 12.0)]);
        let diagnostics = analyze_coverage(&audio, &[segment(0.0, 4.0), segment(7.0, 12.0)]);

        assert!(diagnostics.uncovered_spans.is_empty());
        assert!(diagnostics.warnings.is_empty());
        assert!(diagnostics.coverage_ratio < 0.8);
    }

    #[test]
    fn reports_all_speech_missing_when_no_segments_exist() {
        let audio = voiced_audio(8.0, &[(0.0, 8.0)]);
        let diagnostics = analyze_coverage(&audio, &[]);

        assert_eq!(diagnostics.uncovered_spans.len(), 1);
        assert_eq!(diagnostics.uncovered_spans[0].duration, 8.0);
        assert_eq!(diagnostics.coverage_ratio, 0.0);
    }

    #[test]
    fn strong_supported_language_mismatch_is_actionable() {
        let warning = language_mismatch_warning(
            Language::Swedish,
            &LanguageDetection {
                language: "en".to_string(),
                probability: 0.995,
            },
        )
        .expect("strong mismatch should warn");

        assert_eq!(warning.code, "language_mismatch");
        assert!(warning.message.contains("--language en"));
        assert!(warning.message.contains("Swedish"));
    }

    #[test]
    fn finnish_language_region_is_stable() {
        let windows = vec![
            language_window(0, 0.0, "fi", 0.98),
            language_window(1, 20.0, "fi", 0.97),
        ];

        let diagnostics = analyze_language_windows(&windows);

        assert_eq!(diagnostics.regions.len(), 1);
        assert!(diagnostics.regions[0].stable);
        assert!(diagnostics.warnings.is_empty());
    }

    #[test]
    fn finnish_language_mismatch_recommends_explicit_language() {
        let warning = language_mismatch_warning(
            Language::English,
            &LanguageDetection {
                language: "fi".to_string(),
                probability: 0.995,
            },
        )
        .expect("strong Finnish mismatch should warn");

        assert!(warning.message.contains("Finnish"));
        assert!(warning.message.contains("--language fi"));
    }

    #[test]
    fn weak_or_matching_language_detection_does_not_warn() {
        assert!(language_mismatch_warning(
            Language::Swedish,
            &LanguageDetection {
                language: "en".to_string(),
                probability: 0.70,
            },
        )
        .is_none());
        assert!(language_mismatch_warning(
            Language::English,
            &LanguageDetection {
                language: "en".to_string(),
                probability: 0.999,
            },
        )
        .is_none());
    }

    #[test]
    fn quarantines_long_ordinary_word_loop_despite_confident_no_speech_scores() {
        let segments: Vec<TranscriptSegment> =
            serde_json::from_str(include_str!("../../tests/fixtures/ordinary-word-loop.json"))
                .expect("shareable repetition fixture should parse");

        let diagnostics = analyze_repetition(&segments);

        assert_eq!(diagnostics.spans.len(), 1);
        let span = &diagnostics.spans[0];
        assert_eq!((span.start, span.end), (120.0, 160.0));
        assert_eq!(span.pattern, "thank you");
        assert_eq!(span.repetitions, 16);
        assert_eq!(span.segment_indices, (0..8).collect::<Vec<_>>());
        assert_eq!(diagnostics.warnings[0].code, "repetitive_hallucination");
        assert!(diagnostics.warnings[0].message.contains("no_speech_prob"));
        let machine_output = serde_json::to_value(&diagnostics).unwrap();
        assert_eq!(machine_output["spans"][0]["start"], 120.0);
        assert_eq!(machine_output["spans"][0]["end"], 160.0);
        assert_eq!(
            machine_output["warnings"][0]["code"],
            "repetitive_hallucination"
        );
    }

    #[test]
    fn preserves_short_or_rhetorical_repetition() {
        let rhetorical = TranscriptSegment {
            start: 0.0,
            end: 18.0,
            text: " Thank you, thank you, thank you.".to_string(),
            avg_logprob: Some(-0.1),
            no_speech_prob: 0.01,
        };
        let fast_repetition = TranscriptSegment {
            start: 20.0,
            end: 28.0,
            text: " yes yes yes yes yes yes yes yes".to_string(),
            avg_logprob: Some(-0.1),
            no_speech_prob: 0.01,
        };

        assert!(analyze_repetition(&[rhetorical]).spans.is_empty());
        assert!(analyze_repetition(&[fast_repetition]).spans.is_empty());
    }

    #[test]
    fn warns_for_two_stable_languages_separated_by_silence() {
        let windows: Vec<LanguageWindow> = serde_json::from_str(include_str!(
            "../../tests/fixtures/mixed-language-windows.json"
        ))
        .expect("shareable mixed-language fixture should parse");

        let diagnostics = analyze_language_windows(&windows);

        assert_eq!(diagnostics.regions.len(), 2);
        assert!(diagnostics.regions.iter().all(|region| region.stable));
        assert_eq!(diagnostics.regions[0].language, "en");
        assert_eq!(diagnostics.regions[1].language, "sv");
        assert_eq!(diagnostics.warnings.len(), 1);
        assert_eq!(diagnostics.warnings[0].code, "mixed_language_audio");
        assert_eq!(
            (diagnostics.warnings[0].start, diagnostics.warnings[0].end),
            (Some(60.0), Some(100.0))
        );
    }

    #[test]
    fn isolated_foreign_window_does_not_flap_or_warn() {
        let windows = vec![
            language_window(0, 0.0, "en", 0.98),
            language_window(1, 20.0, "en", 0.97),
            language_window(2, 40.0, "sv", 0.99),
            language_window(3, 60.0, "en", 0.96),
            language_window(4, 80.0, "en", 0.95),
        ];

        let diagnostics = analyze_language_windows(&windows);

        assert!(diagnostics.warnings.is_empty());
        assert!(!diagnostics.regions[1].stable);
    }

    #[test]
    fn language_diagnostics_are_independent_per_source_file() {
        let english_source = vec![
            language_window(0, 0.0, "en", 0.98),
            language_window(1, 20.0, "en", 0.97),
        ];
        let swedish_source = vec![
            language_window(0, 0.0, "sv", 0.98),
            language_window(1, 20.0, "sv", 0.97),
        ];

        assert!(analyze_language_windows(&english_source)
            .warnings
            .is_empty());
        assert!(analyze_language_windows(&swedish_source)
            .warnings
            .is_empty());
    }

    fn language_window(
        sequence: usize,
        start: f64,
        language: &str,
        probability: f32,
    ) -> LanguageWindow {
        LanguageWindow {
            sequence,
            start,
            end: start + 20.0,
            language: language.to_string(),
            probability,
        }
    }
}
