//! Exact, offline diarization scoring primitives.
//!
//! The scorer works on interval events rather than a fixed frame grid.  Each
//! speaker's overlapping turns are first replaced by their interval union, so
//! repeated tuples cannot double count speaker time.  UEM regions are sorted
//! and checked for overlap; overlapping UEM is rejected rather than silently
//! changing the requested scoring domain.  A collar is subtracted from the
//! UEM around every boundary of every reference speaker union.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_TIME_SECONDS: f64 = 14_400.0;
const MAX_TURNS: usize = 100_000;
const MAX_SPEAKERS: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeakerTurn {
    pub start: f64,
    pub end: f64,
    pub speakers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScoringRegion {
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct EvaluationOptions {
    pub collar_seconds: f64,
}

#[derive(Debug, Error, PartialEq)]
pub enum EvaluationError {
    #[error("{kind} contains more than {MAX_TURNS} turns")]
    TooManyTurns { kind: &'static str },
    #[error("more than {MAX_SPEAKERS} distinct speaker IDs were supplied")]
    TooManySpeakers,
    #[error("invalid {kind} interval at index {index}: start={start}, end={end}")]
    InvalidInterval {
        kind: &'static str,
        index: usize,
        start: f64,
        end: f64,
    },
    #[error("{kind} interval at index {index} has an invalid speaker list")]
    InvalidSpeakerList { kind: &'static str, index: usize },
    #[error("UEM regions overlap at normalized index {index}")]
    OverlappingUem { index: usize },
    #[error("collar_seconds must be finite and non-negative")]
    InvalidCollar,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ErrorTotals {
    pub duration_seconds: f64,
    pub reference_speaker_seconds: f64,
    pub miss_seconds: f64,
    pub false_alarm_seconds: f64,
    pub confusion_seconds: f64,
    pub der: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegionEvaluation {
    pub region: ScoringRegion,
    pub errors: ErrorTotals,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvaluationReport {
    /// Total scored wall-clock time after UEM and collar subtraction.
    pub duration_seconds: f64,
    pub reference_speech_seconds: f64,
    pub hypothesis_speech_seconds: f64,
    pub miss_seconds: f64,
    pub false_alarm_seconds: f64,
    pub confusion_seconds: f64,
    /// None when the scored reference has no speech time.
    pub der: Option<f64>,
    /// Mean reference-speaker Jaccard error. None when no reference speaker
    /// has speech time in the scored domain.
    pub jer: Option<f64>,
    pub speaker_mapping: BTreeMap<String, String>,
    pub speech_precision: Option<f64>,
    pub speech_recall: Option<f64>,
    pub overlap_precision: Option<f64>,
    pub overlap_recall: Option<f64>,
    pub overlap: ErrorTotals,
    pub non_overlap: ErrorTotals,
    /// Errors for each original UEM region, using the same global mapping.
    pub per_region: Vec<RegionEvaluation>,
}

#[derive(Debug, Clone, Copy)]
struct Interval {
    start: f64,
    end: f64,
}

#[derive(Debug, Clone)]
struct ActiveSegment {
    region_index: usize,
    start: f64,
    end: f64,
    reference: Vec<usize>,
    hypothesis: Vec<usize>,
}

#[derive(Debug, Clone, Copy)]
struct Event {
    time: f64,
    speaker: usize,
    start: bool,
}

pub fn evaluate(
    reference: &[SpeakerTurn],
    hypothesis: &[SpeakerTurn],
    uem: &[ScoringRegion],
    options: EvaluationOptions,
) -> Result<EvaluationReport, EvaluationError> {
    validate_turn_count(reference.len(), "reference")?;
    validate_turn_count(hypothesis.len(), "hypothesis")?;
    if !options.collar_seconds.is_finite()
        || options.collar_seconds < 0.0
        || options.collar_seconds > MAX_TIME_SECONDS
    {
        return Err(EvaluationError::InvalidCollar);
    }

    let reference = normalize_turns(reference, "reference")?;
    let hypothesis = normalize_turns(hypothesis, "hypothesis")?;
    let uem = normalize_uem(uem)?;

    let mut ids = BTreeSet::new();
    ids.extend(reference.keys().cloned());
    ids.extend(hypothesis.keys().cloned());
    if ids.len() > MAX_SPEAKERS {
        return Err(EvaluationError::TooManySpeakers);
    }

    let ref_ids: Vec<String> = reference.keys().cloned().collect();
    let hyp_ids: Vec<String> = hypothesis.keys().cloned().collect();
    let ref_index: BTreeMap<String, usize> = ref_ids
        .iter()
        .enumerate()
        .map(|(index, id)| (id.clone(), index))
        .collect();
    let hyp_index: BTreeMap<String, usize> = hyp_ids
        .iter()
        .enumerate()
        .map(|(index, id)| (id.clone(), index))
        .collect();

    let scored_uem = subtract_collars(&uem, &reference, options.collar_seconds);
    let segments = build_segments(
        &scored_uem,
        &uem,
        &reference,
        &hypothesis,
        &ref_index,
        &hyp_index,
    );
    let weights = overlap_weights(&segments, ref_ids.len(), hyp_ids.len());
    let assignment = maximum_assignment(&weights, hyp_ids.len());
    let mapped_hyp: Vec<Option<usize>> = assignment
        .into_iter()
        .enumerate()
        .map(|(hyp, reference)| {
            reference.and_then(|reference| (weights[reference][hyp] > 0.0).then_some(reference))
        })
        .collect();

    let mut mapping = BTreeMap::new();
    for (hyp, reference) in mapped_hyp.iter().enumerate() {
        if let Some(reference) = reference {
            mapping.insert(hyp_ids[hyp].clone(), ref_ids[*reference].clone());
        }
    }

    let mut report = EvaluationReport {
        duration_seconds: 0.0,
        reference_speech_seconds: 0.0,
        hypothesis_speech_seconds: 0.0,
        miss_seconds: 0.0,
        false_alarm_seconds: 0.0,
        confusion_seconds: 0.0,
        der: None,
        jer: None,
        speaker_mapping: mapping,
        speech_precision: None,
        speech_recall: None,
        overlap_precision: None,
        overlap_recall: None,
        overlap: ErrorTotals::default(),
        non_overlap: ErrorTotals::default(),
        per_region: uem
            .iter()
            .cloned()
            .map(|region| RegionEvaluation {
                region,
                errors: ErrorTotals::default(),
            })
            .collect(),
    };

    let mut speech_intersection = 0.0;
    let mut overlap_intersection = 0.0;
    let mut reference_by_speaker = vec![0.0; ref_ids.len()];
    let mut hypothesis_by_reference = vec![0.0; ref_ids.len()];
    let mut intersection_by_speaker = vec![0.0; ref_ids.len()];

    for segment in &segments {
        let duration = segment.end - segment.start;
        let n_reference = segment.reference.len();
        let n_hypothesis = segment.hypothesis.len();
        let correct = segment
            .hypothesis
            .iter()
            .filter_map(|hyp| mapped_hyp[*hyp])
            .filter(|reference| segment.reference.contains(reference))
            .count();
        let miss = (n_reference.saturating_sub(n_hypothesis)) as f64 * duration;
        let false_alarm = (n_hypothesis.saturating_sub(n_reference)) as f64 * duration;
        let confusion = (n_reference.min(n_hypothesis).saturating_sub(correct)) as f64 * duration;

        report.duration_seconds += duration;
        report.reference_speech_seconds += (n_reference > 0) as u8 as f64 * duration;
        report.hypothesis_speech_seconds += (n_hypothesis > 0) as u8 as f64 * duration;
        report.miss_seconds += miss;
        report.false_alarm_seconds += false_alarm;
        report.confusion_seconds += confusion;
        if n_reference > 0 && n_hypothesis > 0 {
            speech_intersection += duration;
        }
        if n_reference >= 2 && n_hypothesis >= 2 {
            overlap_intersection += duration;
        }

        for reference in &segment.reference {
            reference_by_speaker[*reference] += duration;
        }
        for hyp in &segment.hypothesis {
            if let Some(reference) = mapped_hyp[*hyp] {
                hypothesis_by_reference[reference] += duration;
            }
        }
        for hyp in &segment.hypothesis {
            if let Some(reference) = mapped_hyp[*hyp] {
                if segment.reference.contains(&reference) {
                    intersection_by_speaker[reference] += duration;
                }
            }
        }

        let target = if n_reference >= 2 {
            &mut report.overlap
        } else {
            &mut report.non_overlap
        };
        add_error(target, duration, n_reference, miss, false_alarm, confusion);
        if let Some(region) = report.per_region.get_mut(segment.region_index) {
            add_error(
                &mut region.errors,
                duration,
                n_reference,
                miss,
                false_alarm,
                confusion,
            );
        }
    }

    finish_error_totals(&mut report.overlap);
    finish_error_totals(&mut report.non_overlap);
    report.der = der(
        report.miss_seconds,
        report.false_alarm_seconds,
        report.confusion_seconds,
        report.reference_speech_seconds,
    );
    report.speech_precision = ratio(speech_intersection, report.hypothesis_speech_seconds);
    report.speech_recall = ratio(speech_intersection, report.reference_speech_seconds);

    let reference_overlap = report.overlap.duration_seconds;
    let hypothesis_overlap = segments
        .iter()
        .filter(|segment| segment.hypothesis.len() >= 2)
        .map(|segment| segment.end - segment.start)
        .sum::<f64>();
    report.overlap_precision = ratio(overlap_intersection, hypothesis_overlap);
    report.overlap_recall = ratio(overlap_intersection, reference_overlap);
    for region in &mut report.per_region {
        finish_error_totals(&mut region.errors);
    }

    let mut jer_sum = 0.0;
    let mut jer_count = 0;
    for (index, reference_duration) in reference_by_speaker.into_iter().enumerate() {
        if reference_duration > 0.0 {
            let union = reference_duration + hypothesis_by_reference[index]
                - intersection_by_speaker[index];
            jer_sum += if union > 0.0 {
                1.0 - intersection_by_speaker[index] / union
            } else {
                0.0
            };
            jer_count += 1;
        }
    }
    report.jer = (jer_count > 0).then_some(jer_sum / jer_count as f64);

    Ok(report)
}

fn validate_turn_count(count: usize, kind: &'static str) -> Result<(), EvaluationError> {
    if count > MAX_TURNS {
        Err(EvaluationError::TooManyTurns { kind })
    } else {
        Ok(())
    }
}

fn validate_time(
    kind: &'static str,
    index: usize,
    start: f64,
    end: f64,
) -> Result<(), EvaluationError> {
    if !start.is_finite()
        || !end.is_finite()
        || start < 0.0
        || end > MAX_TIME_SECONDS
        || start >= end
    {
        return Err(EvaluationError::InvalidInterval {
            kind,
            index,
            start,
            end,
        });
    }
    Ok(())
}

fn normalize_turns(
    turns: &[SpeakerTurn],
    kind: &'static str,
) -> Result<BTreeMap<String, Vec<Interval>>, EvaluationError> {
    let mut intervals: BTreeMap<String, Vec<Interval>> = BTreeMap::new();
    for (index, turn) in turns.iter().enumerate() {
        validate_time(kind, index, turn.start, turn.end)?;
        if turn.speakers.is_empty()
            || turn.speakers.iter().any(|speaker| speaker.is_empty())
            || turn.speakers.iter().collect::<BTreeSet<_>>().len() != turn.speakers.len()
        {
            return Err(EvaluationError::InvalidSpeakerList { kind, index });
        }
        for speaker in &turn.speakers {
            intervals
                .entry(speaker.clone())
                .or_default()
                .push(Interval {
                    start: turn.start,
                    end: turn.end,
                });
        }
    }
    for speaker_intervals in intervals.values_mut() {
        speaker_intervals.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.end.total_cmp(&b.end)));
        let mut union: Vec<Interval> = Vec::with_capacity(speaker_intervals.len());
        for interval in speaker_intervals.drain(..) {
            if let Some(last) = union.last_mut() {
                if interval.start <= last.end {
                    last.end = last.end.max(interval.end);
                    continue;
                }
            }
            union.push(interval);
        }
        *speaker_intervals = union;
    }
    Ok(intervals)
}

fn normalize_uem(uem: &[ScoringRegion]) -> Result<Vec<ScoringRegion>, EvaluationError> {
    let mut normalized = uem.to_vec();
    for (index, region) in normalized.iter().enumerate() {
        validate_time("UEM", index, region.start, region.end)?;
    }
    normalized.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.end.total_cmp(&b.end)));
    for index in 1..normalized.len() {
        if normalized[index].start < normalized[index - 1].end {
            return Err(EvaluationError::OverlappingUem { index });
        }
    }
    Ok(normalized)
}

fn subtract_collars(
    uem: &[ScoringRegion],
    reference: &BTreeMap<String, Vec<Interval>>,
    collar: f64,
) -> Vec<ScoringRegion> {
    if collar == 0.0 {
        return uem.to_vec();
    }
    let mut exclusions = Vec::new();
    for intervals in reference.values() {
        for interval in intervals {
            for boundary in [interval.start, interval.end] {
                exclusions.push(ScoringRegion {
                    start: (boundary - collar).max(0.0),
                    end: (boundary + collar).min(MAX_TIME_SECONDS),
                });
            }
        }
    }
    exclusions.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.end.total_cmp(&b.end)));
    let mut merged: Vec<ScoringRegion> = Vec::new();
    for exclusion in exclusions {
        if let Some(last) = merged.last_mut() {
            if exclusion.start <= last.end {
                last.end = last.end.max(exclusion.end);
                continue;
            }
        }
        merged.push(exclusion);
    }
    let mut result = Vec::new();
    for region in uem {
        let mut cursor = region.start;
        for exclusion in &merged {
            if exclusion.end <= cursor {
                continue;
            }
            if exclusion.start >= region.end {
                break;
            }
            if exclusion.start > cursor {
                result.push(ScoringRegion {
                    start: cursor,
                    end: exclusion.start.min(region.end),
                });
            }
            cursor = cursor.max(exclusion.end);
            if cursor >= region.end {
                break;
            }
        }
        if cursor < region.end {
            result.push(ScoringRegion {
                start: cursor,
                end: region.end,
            });
        }
    }
    result
}

fn build_segments(
    scored_uem: &[ScoringRegion],
    original_uem: &[ScoringRegion],
    reference: &BTreeMap<String, Vec<Interval>>,
    hypothesis: &BTreeMap<String, Vec<Interval>>,
    ref_index: &BTreeMap<String, usize>,
    hyp_index: &BTreeMap<String, usize>,
) -> Vec<ActiveSegment> {
    let mut segments = Vec::new();
    for region in scored_uem {
        let region_index = original_uem
            .iter()
            .position(|original| region.start >= original.start && region.end <= original.end)
            .unwrap_or(0);
        let mut events = Vec::new();
        for (speaker, intervals) in reference {
            let index = ref_index[speaker];
            for interval in intervals {
                if interval.end > region.start && interval.start < region.end {
                    events.push(Event {
                        time: interval.start.max(region.start),
                        speaker: index,
                        start: true,
                    });
                    events.push(Event {
                        time: interval.end.min(region.end),
                        speaker: index,
                        start: false,
                    });
                }
            }
        }
        let reference_count = ref_index.len();
        for (speaker, intervals) in hypothesis {
            let index = hyp_index[speaker];
            for interval in intervals {
                if interval.end > region.start && interval.start < region.end {
                    events.push(Event {
                        time: interval.start.max(region.start),
                        speaker: reference_count + index,
                        start: true,
                    });
                    events.push(Event {
                        time: interval.end.min(region.end),
                        speaker: reference_count + index,
                        start: false,
                    });
                }
            }
        }
        events.sort_by(|a, b| a.time.total_cmp(&b.time).then(a.start.cmp(&b.start)));
        let mut active_reference = vec![false; ref_index.len()];
        let mut active_hypothesis = vec![false; hyp_index.len()];
        let mut cursor = region.start;
        let mut event_index = 0;
        while event_index < events.len() {
            let time = events[event_index].time;
            if time > cursor {
                segments.push(ActiveSegment {
                    region_index,
                    start: cursor,
                    end: time,
                    reference: active_reference
                        .iter()
                        .enumerate()
                        .filter_map(|(index, active)| active.then_some(index))
                        .collect(),
                    hypothesis: active_hypothesis
                        .iter()
                        .enumerate()
                        .filter_map(|(index, active)| active.then_some(index))
                        .collect(),
                });
            }
            while event_index < events.len() && events[event_index].time == time {
                let event = events[event_index];
                if event.speaker < reference_count {
                    active_reference[event.speaker] = event.start;
                } else {
                    active_hypothesis[event.speaker - reference_count] = event.start;
                }
                event_index += 1;
            }
            cursor = time;
        }
        if cursor < region.end {
            segments.push(ActiveSegment {
                region_index,
                start: cursor,
                end: region.end,
                reference: active_reference
                    .iter()
                    .enumerate()
                    .filter_map(|(index, active)| active.then_some(index))
                    .collect(),
                hypothesis: active_hypothesis
                    .iter()
                    .enumerate()
                    .filter_map(|(index, active)| active.then_some(index))
                    .collect(),
            });
        }
    }
    segments
}

fn overlap_weights(
    segments: &[ActiveSegment],
    ref_count: usize,
    hyp_count: usize,
) -> Vec<Vec<f64>> {
    let mut weights = vec![vec![0.0; hyp_count]; ref_count];
    for segment in segments {
        let duration = segment.end - segment.start;
        for reference in &segment.reference {
            for hypothesis in &segment.hypothesis {
                weights[*reference][*hypothesis] += duration;
            }
        }
    }
    weights
}

/// Return a reference index for each hypothesis index.  Zero-weight pairs are
/// left for the caller to treat as unmatched.  The square Hungarian assignment
/// gives a deterministic global optimum, including when windows have opposing
/// local optima.
fn maximum_assignment(weights: &[Vec<f64>], hyp_count: usize) -> Vec<Option<usize>> {
    let ref_count = weights.len();
    let size = ref_count.max(hyp_count);
    let mut result = vec![None; hyp_count];
    if size == 0 {
        return result;
    }
    let mut cost = vec![vec![0.0; size]; size];
    for row in 0..ref_count {
        for col in 0..hyp_count {
            cost[row][col] = -weights[row][col];
        }
    }
    let mut u = vec![0.0; size + 1];
    let mut v = vec![0.0; size + 1];
    let mut p = vec![0usize; size + 1];
    let mut way = vec![0usize; size + 1];
    for row in 1..=size {
        p[0] = row;
        let mut column = 0;
        let mut minv = vec![f64::INFINITY; size + 1];
        let mut used = vec![false; size + 1];
        loop {
            used[column] = true;
            let current_row = p[column];
            let mut delta = f64::INFINITY;
            let mut next_column = 0;
            for candidate in 1..=size {
                if !used[candidate] {
                    let current =
                        cost[current_row - 1][candidate - 1] - u[current_row] - v[candidate];
                    if current < minv[candidate]
                        || (current == minv[candidate] && column < way[candidate])
                    {
                        minv[candidate] = current;
                        way[candidate] = column;
                    }
                    if minv[candidate] < delta
                        || (minv[candidate] == delta && candidate < next_column)
                    {
                        delta = minv[candidate];
                        next_column = candidate;
                    }
                }
            }
            for candidate in 0..=size {
                if used[candidate] {
                    u[p[candidate]] += delta;
                    v[candidate] -= delta;
                } else {
                    minv[candidate] -= delta;
                }
            }
            column = next_column;
            if p[column] == 0 {
                break;
            }
        }
        loop {
            let previous = way[column];
            p[column] = p[previous];
            column = previous;
            if column == 0 {
                break;
            }
        }
    }
    for column in 1..=size {
        let row = p[column];
        if row > 0 && row <= ref_count && column <= hyp_count {
            result[column - 1] = Some(row - 1);
        }
    }
    result
}

fn add_error(
    total: &mut ErrorTotals,
    duration: f64,
    reference_count: usize,
    miss: f64,
    false_alarm: f64,
    confusion: f64,
) {
    total.duration_seconds += duration;
    total.reference_speaker_seconds += reference_count as f64 * duration;
    total.miss_seconds += miss;
    total.false_alarm_seconds += false_alarm;
    total.confusion_seconds += confusion;
}

fn finish_error_totals(total: &mut ErrorTotals) {
    total.der = der(
        total.miss_seconds,
        total.false_alarm_seconds,
        total.confusion_seconds,
        total.reference_speaker_seconds,
    );
}

fn der(miss: f64, false_alarm: f64, confusion: f64, denominator: f64) -> Option<f64> {
    (denominator > 0.0).then_some((miss + false_alarm + confusion) / denominator)
}

fn ratio(numerator: f64, denominator: f64) -> Option<f64> {
    (denominator > 0.0).then_some(numerator / denominator)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(start: f64, end: f64, speakers: &[&str]) -> SpeakerTurn {
        SpeakerTurn {
            start,
            end,
            speakers: speakers
                .iter()
                .map(|speaker| (*speaker).to_owned())
                .collect(),
        }
    }

    fn options(collar_seconds: f64) -> EvaluationOptions {
        EvaluationOptions { collar_seconds }
    }

    fn uem(end: f64) -> Vec<ScoringRegion> {
        vec![ScoringRegion { start: 0.0, end }]
    }

    #[test]
    fn perfect_and_permuted_labels_are_zero_error() {
        let report = evaluate(
            &[turn(0.0, 2.0, &["a"]), turn(2.0, 4.0, &["b"])],
            &[turn(0.0, 2.0, &["x"]), turn(2.0, 4.0, &["y"])],
            &uem(4.0),
            options(0.0),
        )
        .unwrap();
        assert_eq!(report.der, Some(0.0));
        assert_eq!(report.speaker_mapping["x"], "a");
        assert_eq!(report.speaker_mapping["y"], "b");
    }

    #[test]
    fn duplicate_same_speaker_turns_are_unioned() {
        let report = evaluate(
            &[turn(0.0, 2.0, &["a"]), turn(1.0, 3.0, &["a"])],
            &[turn(0.0, 3.0, &["x"])],
            &uem(3.0),
            options(0.0),
        )
        .unwrap();
        assert_eq!(report.reference_speech_seconds, 3.0);
        assert_eq!(report.der, Some(0.0));
    }

    #[test]
    fn false_speech_in_silence_is_false_alarm_with_no_der_reference() {
        let report = evaluate(&[], &[turn(1.0, 2.0, &["x"])], &uem(3.0), options(0.0)).unwrap();
        assert_eq!(report.der, None);
        assert_eq!(report.false_alarm_seconds, 1.0);
        assert_eq!(report.speech_precision, Some(0.0));
        assert_eq!(report.speech_recall, None);
    }

    #[test]
    fn missed_and_extra_speakers_are_kept_in_error_totals() {
        let missed = evaluate(
            &[turn(0.0, 1.0, &["a", "b"])],
            &[turn(0.0, 1.0, &["x"])],
            &uem(1.0),
            options(0.0),
        )
        .unwrap();
        assert_eq!(missed.miss_seconds, 1.0);
        assert_eq!(missed.false_alarm_seconds, 0.0);

        let extra = evaluate(
            &[turn(0.0, 1.0, &["a"])],
            &[turn(0.0, 1.0, &["x", "y"])],
            &uem(1.0),
            options(0.0),
        )
        .unwrap();
        assert_eq!(extra.false_alarm_seconds, 1.0);
    }

    #[test]
    fn overlap_and_holes_are_scored_exactly() {
        let report = evaluate(
            &[turn(0.0, 2.0, &["a", "b"])],
            &[turn(0.0, 2.0, &["x", "y"])],
            &[
                ScoringRegion {
                    start: 0.0,
                    end: 1.0,
                },
                ScoringRegion {
                    start: 1.5,
                    end: 2.0,
                },
            ],
            options(0.0),
        )
        .unwrap();
        assert_eq!(report.reference_speech_seconds, 1.5);
        assert_eq!(report.overlap.duration_seconds, 1.5);
        assert_eq!(report.per_region.len(), 2);
        assert_eq!(report.per_region[1].errors.duration_seconds, 0.5);
    }

    #[test]
    fn three_way_overlap_uses_speaker_time_denominator() {
        let report = evaluate(
            &[turn(0.0, 1.0, &["a", "b", "c"])],
            &[turn(0.0, 1.0, &["x", "y"])],
            &uem(1.0),
            options(0.0),
        )
        .unwrap();
        assert_eq!(report.overlap.duration_seconds, 1.0);
        assert_eq!(report.overlap.reference_speaker_seconds, 3.0);
        assert_eq!(report.miss_seconds, 1.0);
        assert_eq!(report.overlap_recall, Some(1.0));
    }

    #[test]
    fn collar_removes_reference_boundaries() {
        let reference = [turn(0.0, 2.0, &["a"]), turn(4.0, 6.0, &["a"])];
        let no_collar = evaluate(&reference, &reference, &uem(7.0), options(0.0)).unwrap();
        let collar = evaluate(&reference, &reference, &uem(7.0), options(0.25)).unwrap();
        assert_eq!(no_collar.reference_speech_seconds, 4.0);
        assert!(collar.reference_speech_seconds < no_collar.reference_speech_seconds);
        assert_eq!(collar.der, Some(0.0));
    }

    #[test]
    fn invalid_values_and_overlapping_uem_are_rejected() {
        assert!(evaluate(&[turn(f64::NAN, 1.0, &["a"])], &[], &uem(1.0), options(0.0),).is_err());
        assert!(matches!(
            evaluate(
                &[],
                &[],
                &[
                    ScoringRegion {
                        start: 0.0,
                        end: 1.0
                    },
                    ScoringRegion {
                        start: 0.5,
                        end: 2.0
                    },
                ],
                options(0.0),
            ),
            Err(EvaluationError::OverlappingUem { .. })
        ));
    }

    #[test]
    fn global_mapping_beats_per_window_greedy_choice() {
        let report = evaluate(
            &[turn(0.0, 5.0, &["a"]), turn(5.0, 10.0, &["b"])],
            &[turn(0.0, 6.0, &["x"]), turn(4.0, 10.0, &["y"])],
            &uem(10.0),
            options(0.0),
        )
        .unwrap();
        assert_eq!(report.speaker_mapping["x"], "a");
        assert_eq!(report.speaker_mapping["y"], "b");
    }
}
