//! Native, reviewable diarization reference documents.
//!
//! This module deliberately has no audio or model access.  A reference only
//! becomes scoring input through explicitly verified intervals carrying human
//! review evidence.  Candidate and unknown intervals can remove regions from
//! the verified UEM, but can never become speaker turns implicitly.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::diarization_evaluation::{ScoringRegion, SpeakerTurn};

const MAX_DURATION_SECONDS: f64 = 14_400.0;
const MAX_INTERVALS: usize = 100_000;
const MAX_WINDOWS: usize = 100_000;
const MAX_SPEAKERS: usize = 64;
const MAX_ID_LENGTH: usize = 128;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReferenceError {
    #[error("invalid reference field: {0}")]
    Invalid(&'static str),
    #[error("reference interval count exceeds {MAX_INTERVALS}")]
    TooManyIntervals,
    #[error("reference window count exceeds {MAX_WINDOWS}")]
    TooManyWindows,
    #[error("reference speaker count exceeds {MAX_SPEAKERS}")]
    TooManySpeakers,
    #[error("unknown window ID: {0}")]
    UnknownWindow(String),
    #[error("split selection contains an unknown window ID: {0}")]
    UnknownSplitWindow(String),
    #[error("verified interval {0} has incompatible overlap")]
    IncompatibleVerifiedOverlap(usize),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReferenceStatus {
    Candidate,
    Verified,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReferenceActivity {
    Speech,
    Silence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceEvidence {
    pub kind: String,
    pub artifact: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceWindow {
    pub id: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceInterval {
    pub start: f64,
    pub end: f64,
    pub speakers: Vec<String>,
    pub status: ReferenceStatus,
    #[serde(default)]
    pub activity: Option<ReferenceActivity>,
    #[serde(default)]
    pub evidence: Vec<ReferenceEvidence>,
    #[serde(default)]
    pub reviewer: Option<String>,
    #[serde(default)]
    pub reviewed_at: Option<String>,
    #[serde(default)]
    pub window_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceDocument {
    pub source_sha256: String,
    pub duration_seconds: f64,
    pub speakers: Vec<String>,
    #[serde(default)]
    pub windows: Vec<ReferenceWindow>,
    pub intervals: Vec<ReferenceInterval>,
}

impl ReferenceDocument {
    /// Validate the complete document without changing it.
    pub fn validate(&self) -> Result<(), ReferenceError> {
        validate_source_hash(&self.source_sha256)?;
        if !self.duration_seconds.is_finite()
            || self.duration_seconds <= 0.0
            || self.duration_seconds > MAX_DURATION_SECONDS
        {
            return Err(ReferenceError::Invalid("duration_seconds"));
        }
        if self.speakers.len() > MAX_SPEAKERS {
            return Err(ReferenceError::TooManySpeakers);
        }
        let mut known_speakers = BTreeSet::new();
        for speaker in &self.speakers {
            validate_identifier(speaker, "speaker ID")?;
            if !known_speakers.insert(speaker) {
                return Err(ReferenceError::Invalid("duplicate speaker ID"));
            }
        }
        if self.windows.len() > MAX_WINDOWS {
            return Err(ReferenceError::TooManyWindows);
        }
        let mut windows = BTreeMap::new();
        for window in &self.windows {
            validate_identifier(&window.id, "window ID")?;
            validate_interval(window.start, window.end, self.duration_seconds, "window")?;
            if windows.insert(window.id.as_str(), window).is_some() {
                return Err(ReferenceError::Invalid("duplicate window ID"));
            }
        }
        let ordered_windows = sorted_windows(&self.windows);
        for pair in ordered_windows.windows(2) {
            if pair[1].start < pair[0].end {
                return Err(ReferenceError::Invalid("overlapping windows"));
            }
        }
        if self.intervals.len() > MAX_INTERVALS {
            return Err(ReferenceError::TooManyIntervals);
        }
        for (index, interval) in self.intervals.iter().enumerate() {
            validate_interval(
                interval.start,
                interval.end,
                self.duration_seconds,
                "interval",
            )?;
            validate_interval_speakers(interval, &known_speakers)?;
            match interval.status {
                ReferenceStatus::Unknown => {
                    if !interval.speakers.is_empty() {
                        return Err(ReferenceError::Invalid("unknown interval speakers"));
                    }
                }
                ReferenceStatus::Candidate | ReferenceStatus::Verified => {
                    if effective_activity(interval) == ReferenceActivity::Speech
                        && interval.speakers.is_empty()
                    {
                        return Err(ReferenceError::Invalid("speech interval speakers"));
                    }
                    if effective_activity(interval) == ReferenceActivity::Silence
                        && !interval.speakers.is_empty()
                    {
                        return Err(ReferenceError::Invalid("silence interval speakers"));
                    }
                }
            }
            if let Some(window_id) = &interval.window_id {
                validate_identifier(window_id, "interval window ID")?;
                let window = windows
                    .get(window_id.as_str())
                    .ok_or_else(|| ReferenceError::UnknownWindow(window_id.clone()))?;
                if interval.start < window.start || interval.end > window.end {
                    return Err(ReferenceError::Invalid("interval outside window"));
                }
            } else if !windows.is_empty() && containing_window(interval, &ordered_windows).is_none()
            {
                return Err(ReferenceError::Invalid("interval must fit one window"));
            }
            validate_interval_review(index, interval)?;
        }
        let mut verified: Vec<_> = self
            .intervals
            .iter()
            .enumerate()
            .filter(|(_, interval)| interval.status == ReferenceStatus::Verified)
            .collect();
        verified.sort_by(|(_, left), (_, right)| {
            left.start
                .total_cmp(&right.start)
                .then(left.end.total_cmp(&right.end))
                .then(left.speakers.cmp(&right.speakers))
        });
        let mut active_end = 0.0;
        let mut active_index = None;
        let mut active_speakers = None;
        for (original_index, interval) in verified {
            if interval.start < active_end {
                if active_speakers.as_ref() != Some(&speaker_set(interval)) {
                    return Err(ReferenceError::IncompatibleVerifiedOverlap(
                        active_index.expect("an active interval has an index"),
                    ));
                }
                active_end = active_end.max(interval.end);
            } else {
                active_end = interval.end;
                active_index = Some(original_index);
                active_speakers = Some(speaker_set(interval));
            }
        }
        Ok(())
    }

    /// Return only explicitly verified speech, coalescing overlapping or
    /// adjacent duplicate speaker-set intervals within the same window.
    pub fn verified_turns(&self) -> Result<Vec<SpeakerTurn>, ReferenceError> {
        self.validate()?;
        let mut turns: Vec<_> = self
            .intervals
            .iter()
            .filter(|interval| {
                interval.status == ReferenceStatus::Verified
                    && effective_activity(interval) == ReferenceActivity::Speech
            })
            .collect();
        turns.sort_by(|left, right| {
            left.window_id
                .cmp(&right.window_id)
                .then(left.start.total_cmp(&right.start))
                .then(left.end.total_cmp(&right.end))
                .then_with(|| {
                    let mut left_speakers = left.speakers.clone();
                    let mut right_speakers = right.speakers.clone();
                    left_speakers.sort();
                    right_speakers.sort();
                    left_speakers.cmp(&right_speakers)
                })
        });

        let mut output: Vec<(Option<&str>, f64, f64, Vec<String>)> = Vec::new();
        for interval in turns {
            let mut speakers = interval.speakers.clone();
            speakers.sort();
            if let Some(last) = output.last_mut() {
                if last.0 == interval.window_id.as_deref()
                    && last.3 == speakers
                    && interval.start <= last.2
                {
                    last.2 = last.2.max(interval.end);
                    continue;
                }
            }
            output.push((
                interval.window_id.as_deref(),
                interval.start,
                interval.end,
                speakers,
            ));
        }
        output.sort_by(|left, right| {
            left.1
                .total_cmp(&right.1)
                .then(left.2.total_cmp(&right.2))
                .then(left.3.cmp(&right.3))
        });
        Ok(output
            .into_iter()
            .map(|(_, start, end, speakers)| SpeakerTurn {
                start,
                end,
                speakers,
            })
            .collect())
    }

    /// UEM is the union of verified speech and verified silence, minus every
    /// candidate or unknown interval.  This keeps unreviewed contradictions
    /// out of the scored domain while retaining explicitly verified silence.
    pub fn verified_uem(&self) -> Result<Vec<ScoringRegion>, ReferenceError> {
        self.validate()?;
        let base = self
            .intervals
            .iter()
            .filter(|interval| interval.status == ReferenceStatus::Verified)
            .map(|interval| (interval.start, interval.end));
        let holes = self
            .intervals
            .iter()
            .filter(|interval| {
                matches!(
                    interval.status,
                    ReferenceStatus::Candidate | ReferenceStatus::Unknown
                )
            })
            .map(|interval| (interval.start, interval.end));
        Ok(subtract_regions(base, holes)
            .into_iter()
            .map(|(start, end)| ScoringRegion { start, end })
            .collect())
    }

    /// Select complete supplied windows for a split.  Intervals are retained
    /// only when their explicit or uniquely inferred window is selected.
    pub fn for_split(&self, window_ids: &BTreeSet<String>) -> Result<Self, ReferenceError> {
        self.validate()?;
        let known: BTreeSet<_> = self
            .windows
            .iter()
            .map(|window| window.id.as_str())
            .collect();
        for id in window_ids {
            if !known.contains(id.as_str()) {
                return Err(ReferenceError::UnknownSplitWindow(id.clone()));
            }
        }
        let mut selected = self.clone();
        let ordered_windows = sorted_windows(&self.windows);
        selected
            .windows
            .retain(|window| window_ids.contains(&window.id));
        selected.intervals = self
            .intervals
            .iter()
            .filter_map(|interval| {
                let resolved = interval.window_id.clone().or_else(|| {
                    containing_window(interval, &ordered_windows).map(|window| window.id.clone())
                });
                resolved
                    .filter(|window_id| window_ids.contains(window_id))
                    .map(|window_id| {
                        let mut copy = interval.clone();
                        copy.window_id = Some(window_id);
                        copy
                    })
            })
            .collect();
        Ok(selected)
    }

    pub fn export_rttm(&self) -> Result<String, ReferenceError> {
        let mut output = String::new();
        for interval in self.verified_turns()? {
            let duration = interval.end - interval.start;
            for speaker in interval.speakers {
                output.push_str(&format!(
                    "SPEAKER {} 1 {:.9} {:.9} <NA> <NA> {} <NA> <NA>\n",
                    self.source_sha256, interval.start, duration, speaker
                ));
            }
        }
        Ok(output)
    }

    pub fn export_uem(&self) -> Result<String, ReferenceError> {
        let mut output = String::new();
        for region in self.verified_uem()? {
            output.push_str(&format!(
                "{} 1 {:.9} {:.9}\n",
                self.source_sha256, region.start, region.end
            ));
        }
        Ok(output)
    }
}

fn validate_source_hash(value: &str) -> Result<(), ReferenceError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Err(ReferenceError::Invalid("source_sha256"))
    } else {
        Ok(())
    }
}

fn validate_identifier(value: &str, field: &'static str) -> Result<(), ReferenceError> {
    if value.is_empty()
        || value.len() > MAX_ID_LENGTH
        || value.chars().any(char::is_whitespace)
        || value.chars().any(char::is_control)
    {
        Err(ReferenceError::Invalid(field))
    } else {
        Ok(())
    }
}

fn validate_interval(
    start: f64,
    end: f64,
    duration: f64,
    field: &'static str,
) -> Result<(), ReferenceError> {
    if !start.is_finite() || !end.is_finite() || start < 0.0 || start >= end || end > duration {
        Err(ReferenceError::Invalid(field))
    } else {
        Ok(())
    }
}

fn validate_interval_speakers(
    interval: &ReferenceInterval,
    known: &BTreeSet<&String>,
) -> Result<(), ReferenceError> {
    let mut ids = BTreeSet::new();
    for speaker in &interval.speakers {
        validate_identifier(speaker, "interval speaker ID")?;
        if !known.contains(speaker) {
            return Err(ReferenceError::Invalid("unknown interval speaker"));
        }
        if !ids.insert(speaker) {
            return Err(ReferenceError::Invalid("duplicate interval speaker"));
        }
    }
    for evidence in &interval.evidence {
        validate_identifier(&evidence.kind, "evidence kind")?;
        if evidence.artifact.is_empty() || evidence.artifact.chars().any(char::is_control) {
            return Err(ReferenceError::Invalid("evidence artifact"));
        }
    }
    Ok(())
}

fn validate_interval_review(
    index: usize,
    interval: &ReferenceInterval,
) -> Result<(), ReferenceError> {
    if interval.status == ReferenceStatus::Verified {
        let reviewer = interval
            .reviewer
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or(ReferenceError::Invalid("verified reviewer"))?;
        let reviewer_kind = reviewer.trim().to_ascii_lowercase();
        if matches!(
            reviewer_kind.as_str(),
            "model"
                | "system"
                | "automatic"
                | "automated"
                | "auto"
                | "consensus"
                | "assistant"
                | "unknown"
                | "bot"
        ) {
            return Err(ReferenceError::Invalid("verified reviewer human identity"));
        }
        if interval.evidence.is_empty() {
            return Err(ReferenceError::Invalid("verified evidence"));
        }
        validate_timestamp(
            interval
                .reviewed_at
                .as_deref()
                .ok_or(ReferenceError::Invalid("verified reviewed_at"))?,
        )?;
    } else if let Some(reviewer) = &interval.reviewer {
        if reviewer.trim().is_empty() {
            return Err(ReferenceError::Invalid("reviewer"));
        }
    }
    if let Some(reviewed_at) = &interval.reviewed_at {
        validate_timestamp(reviewed_at)?;
    }
    if index >= MAX_INTERVALS {
        return Err(ReferenceError::TooManyIntervals);
    }
    Ok(())
}

fn validate_timestamp(value: &str) -> Result<(), ReferenceError> {
    let bytes = value.as_bytes();
    if bytes.len() < 20 || !matches!(bytes[10], b'T' | b' ') {
        return Err(ReferenceError::Invalid("reviewed_at ISO-8601 timestamp"));
    }
    let year = parse_digits(bytes, 0, 4)?;
    let month = parse_digits(bytes, 5, 2)?;
    let day = parse_digits(bytes, 8, 2)?;
    if year == 0 || bytes[4] != b'-' || bytes[7] != b'-' || !(1..=12).contains(&month) {
        return Err(ReferenceError::Invalid("reviewed_at ISO-8601 date"));
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day == 0 || day > month_days[month as usize - 1] {
        return Err(ReferenceError::Invalid("reviewed_at ISO-8601 date"));
    }
    let hour = parse_digits(bytes, 11, 2)?;
    let minute = parse_digits(bytes, 14, 2)?;
    let second = parse_digits(bytes, 17, 2)?;
    if bytes[13] != b':' || bytes[16] != b':' || hour > 23 || minute > 59 || second > 59 {
        return Err(ReferenceError::Invalid("reviewed_at ISO-8601 time"));
    }
    let mut index = 19;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == start {
            return Err(ReferenceError::Invalid("reviewed_at ISO-8601 fraction"));
        }
    }
    match bytes.get(index) {
        Some(b'Z') if index + 1 == bytes.len() => Ok(()),
        Some(b'+') | Some(b'-') => {
            let remaining = &bytes[index + 1..];
            if remaining.len() != 4 && remaining.len() != 5 {
                return Err(ReferenceError::Invalid("reviewed_at ISO-8601 timezone"));
            }
            let (hour_index, minute_index) = if remaining.len() == 4 {
                (0, 2)
            } else {
                if remaining[2] != b':' {
                    return Err(ReferenceError::Invalid("reviewed_at ISO-8601 timezone"));
                }
                (0, 3)
            };
            let zone_hour = parse_digits(remaining, hour_index, 2)?;
            let zone_minute = parse_digits(remaining, minute_index, 2)?;
            if zone_hour > 23 || zone_minute > 59 {
                Err(ReferenceError::Invalid("reviewed_at ISO-8601 timezone"))
            } else {
                Ok(())
            }
        }
        _ => Err(ReferenceError::Invalid("reviewed_at timezone required")),
    }
}

fn parse_digits(value: &[u8], start: usize, count: usize) -> Result<u32, ReferenceError> {
    if start + count > value.len() || !value[start..start + count].iter().all(u8::is_ascii_digit) {
        return Err(ReferenceError::Invalid("reviewed_at ISO-8601 timestamp"));
    }
    Ok(value[start..start + count]
        .iter()
        .fold(0, |value, digit| value * 10 + u32::from(digit - b'0')))
}

fn effective_activity(interval: &ReferenceInterval) -> ReferenceActivity {
    interval.activity.unwrap_or(ReferenceActivity::Speech)
}

fn speaker_set(interval: &ReferenceInterval) -> BTreeSet<&str> {
    interval.speakers.iter().map(String::as_str).collect()
}

fn containing_window<'a>(
    interval: &ReferenceInterval,
    windows: &[&'a ReferenceWindow],
) -> Option<&'a ReferenceWindow> {
    let mut left = 0;
    let mut right = windows.len();
    while left < right {
        let middle = left + (right - left) / 2;
        if windows[middle].start <= interval.start {
            left = middle + 1;
        } else {
            right = middle;
        }
    }
    let candidate = left.checked_sub(1)?;
    let window = windows[candidate];
    (interval.end <= window.end).then_some(window)
}

fn subtract_regions(
    base: impl IntoIterator<Item = (f64, f64)>,
    holes: impl IntoIterator<Item = (f64, f64)>,
) -> Vec<(f64, f64)> {
    let base = merge_regions(base);
    let holes = merge_regions(holes);
    let mut output = Vec::new();
    let mut hole_index = 0;
    for (start, end) in base {
        let mut cursor = start;
        while hole_index < holes.len() && holes[hole_index].1 <= cursor {
            hole_index += 1;
        }
        let mut current_hole = hole_index;
        while current_hole < holes.len() {
            let (hole_start, hole_end) = holes[current_hole];
            if hole_start >= end {
                break;
            }
            if hole_start > cursor {
                output.push((cursor, hole_start.min(end)));
            }
            cursor = cursor.max(hole_end);
            if cursor >= end {
                break;
            }
            current_hole += 1;
        }
        if current_hole > hole_index {
            hole_index = current_hole;
        }
        if cursor < end {
            output.push((cursor, end));
        }
    }
    output
}

fn sorted_windows(windows: &[ReferenceWindow]) -> Vec<&ReferenceWindow> {
    let mut ordered: Vec<_> = windows.iter().collect();
    ordered.sort_by(|left, right| {
        left.start
            .total_cmp(&right.start)
            .then(left.end.total_cmp(&right.end))
            .then(left.id.cmp(&right.id))
    });
    ordered
}

fn merge_regions(regions: impl IntoIterator<Item = (f64, f64)>) -> Vec<(f64, f64)> {
    let mut sorted: Vec<_> = regions
        .into_iter()
        .filter(|(start, end)| start < end)
        .collect();
    sorted.sort_by(|left, right| left.0.total_cmp(&right.0).then(left.1.total_cmp(&right.1)));
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (start, end) in sorted {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_timezone_is_an_exact_iso_offset() {
        for offset in ["Z", "+02:00", "-0500"] {
            assert!(
                validate_timestamp(&format!("2026-10-08T10:00:00{offset}")).is_ok(),
                "{offset}"
            );
        }
        for offset in ["+02000", "+02:000", "+2400", "+02:60", "+2:00"] {
            assert!(
                validate_timestamp(&format!("2026-10-08T10:00:00{offset}")).is_err(),
                "{offset}"
            );
        }
    }

    fn document(intervals: Vec<ReferenceInterval>) -> ReferenceDocument {
        ReferenceDocument {
            source_sha256: "a".repeat(64),
            duration_seconds: 10.0,
            speakers: vec!["alice".into(), "bob".into()],
            windows: vec![ReferenceWindow {
                id: "w1".into(),
                start: 0.0,
                end: 10.0,
            }],
            intervals,
        }
    }

    fn verified(start: f64, end: f64, speakers: &[&str]) -> ReferenceInterval {
        ReferenceInterval {
            start,
            end,
            speakers: speakers.iter().map(|speaker| (*speaker).into()).collect(),
            status: ReferenceStatus::Verified,
            activity: None,
            evidence: vec![ReferenceEvidence {
                kind: "human_review".into(),
                artifact: "local-note-1".into(),
            }],
            reviewer: Some("SyntheticReviewer".into()),
            reviewed_at: Some("2025-01-02T03:04:05Z".into()),
            window_id: Some("w1".into()),
        }
    }

    fn unknown(start: f64, end: f64) -> ReferenceInterval {
        ReferenceInterval {
            start,
            end,
            speakers: vec![],
            status: ReferenceStatus::Unknown,
            activity: None,
            evidence: vec![],
            reviewer: None,
            reviewed_at: None,
            window_id: Some("w1".into()),
        }
    }

    #[test]
    fn verified_outputs_and_empty_candidate_do_not_promote_candidate() {
        let mut candidate = verified(3.0, 4.0, &["bob"]);
        candidate.status = ReferenceStatus::Candidate;
        candidate.evidence.clear();
        candidate.reviewer = None;
        candidate.reviewed_at = None;
        let reference = document(vec![verified(0.0, 2.0, &["alice"]), candidate]);
        assert_eq!(reference.verified_turns().unwrap().len(), 1);
        let rttm = reference.export_rttm().unwrap();
        assert!(rttm.contains("alice"));
        assert!(!rttm.contains("bob"));
        assert!(reference.export_uem().unwrap().contains("0.000000000"));
    }

    #[test]
    fn silence_is_scored_but_unknown_subtracts_from_uem() {
        let mut silence = verified(2.0, 5.0, &[]);
        silence.activity = Some(ReferenceActivity::Silence);
        let reference = document(vec![
            verified(0.0, 2.0, &["alice"]),
            silence,
            unknown(4.0, 6.0),
            verified(5.0, 8.0, &["alice"]),
        ]);
        let uem = reference.verified_uem().unwrap();
        assert_eq!(
            uem,
            vec![
                ScoringRegion {
                    start: 0.0,
                    end: 4.0
                },
                ScoringRegion {
                    start: 6.0,
                    end: 8.0
                }
            ]
        );
        assert_eq!(reference.verified_turns().unwrap().len(), 2);
    }

    #[test]
    fn duplicate_sets_union_and_different_verified_overlap_rejects() {
        let mut duplicate = verified(1.0, 3.0, &["alice"]);
        duplicate.start = 0.0;
        let reference = document(vec![verified(0.0, 2.0, &["alice"]), duplicate]);
        assert_eq!(reference.verified_turns().unwrap()[0].end, 3.0);
        let incompatible = document(vec![
            verified(0.0, 2.0, &["alice"]),
            verified(1.0, 3.0, &["bob"]),
        ]);
        assert!(matches!(
            incompatible.validate(),
            Err(ReferenceError::IncompatibleVerifiedOverlap(_))
        ));
    }

    #[test]
    fn overlapping_equal_speaker_sets_allow_reordered_labels() {
        let reference = document(vec![
            verified(0.0, 2.0, &["alice", "bob"]),
            verified(1.0, 3.0, &["bob", "alice"]),
        ]);
        assert!(reference.validate().is_ok());
        assert_eq!(reference.verified_turns().unwrap()[0].end, 3.0);
    }

    #[test]
    fn nested_different_speaker_overlap_is_rejected_by_sorted_sweep() {
        let reference = document(vec![
            verified(0.0, 5.0, &["alice"]),
            verified(1.0, 2.0, &["alice"]),
            verified(2.0, 4.0, &["bob"]),
        ]);
        assert!(matches!(
            reference.validate(),
            Err(ReferenceError::IncompatibleVerifiedOverlap(_))
        ));
    }

    #[test]
    fn region_subtraction_uses_sorted_holes_across_disjoint_base_regions() {
        assert_eq!(
            subtract_regions(
                [(0.0, 1.0), (2.0, 3.0), (4.0, 5.0)],
                [(0.5, 2.5), (4.5, 6.0)],
            ),
            vec![(0.0, 0.5), (2.5, 3.0), (4.0, 4.5)]
        );
    }

    #[test]
    fn inferred_window_lookup_handles_many_input_ordered_windows() {
        let mut windows: Vec<_> = (0..128)
            .map(|index| ReferenceWindow {
                id: format!("w{index}"),
                start: index as f64,
                end: index as f64 + 0.75,
            })
            .collect();
        windows.reverse();
        let mut interval = verified(127.0, 127.5, &["alice"]);
        interval.window_id = None;
        let reference = ReferenceDocument {
            source_sha256: "a".repeat(64),
            duration_seconds: 128.0,
            speakers: vec!["alice".into(), "bob".into()],
            windows,
            intervals: vec![interval],
        };
        let selected = reference
            .for_split(&BTreeSet::from(["w127".into()]))
            .unwrap();
        assert_eq!(selected.intervals[0].window_id.as_deref(), Some("w127"));
    }

    #[test]
    fn strict_validation_covers_source_human_evidence_timestamp_and_bounds() {
        let mut bad = document(vec![verified(0.0, 2.0, &["alice"])]);
        bad.source_sha256 = "A".repeat(64);
        assert!(bad.validate().is_err());
        let mut bad = document(vec![verified(0.0, 2.0, &["alice"])]);
        bad.intervals[0].reviewer = Some("model".into());
        assert!(bad.validate().is_err());
        let mut bad = document(vec![verified(0.0, 2.0, &["alice"])]);
        bad.intervals[0].reviewed_at = Some("2025-02-29T03:04:05".into());
        assert!(bad.validate().is_err());
        let mut bad = document(vec![verified(0.0, 2.0, &["alice"])]);
        bad.intervals[0].end = 11.0;
        assert!(bad.validate().is_err());
    }

    #[test]
    fn split_selects_whole_windows_and_rejects_unknown_ids() {
        let mut reference = document(vec![verified(0.0, 2.0, &["alice"])]);
        reference.windows[0].end = 4.0;
        reference.windows.push(ReferenceWindow {
            id: "w2".into(),
            start: 5.0,
            end: 10.0,
        });
        reference.intervals.push(verified(5.0, 6.0, &["bob"]));
        reference.intervals[1].window_id = Some("w2".into());
        let selected = reference.for_split(&BTreeSet::from(["w2".into()])).unwrap();
        assert_eq!(selected.windows.len(), 1);
        assert_eq!(selected.intervals.len(), 1);
        assert_eq!(selected.intervals[0].speakers, vec!["bob"]);
        assert!(reference
            .for_split(&BTreeSet::from(["missing".into()]))
            .is_err());
    }
}
