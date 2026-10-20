//! The RTTM and UEM text formats of the scoring tools (#374): the reference turns and the scored
//! regions that `sagascript diarization evaluate` reads from files.
//!
//! Both parsers take the text, have no I/O and refuse what the scoring would misread: more than
//! one recording ID, another channel than 1, a zero or out-of-bounds interval, a duplicate turn.
//! Every refusal is a [`DictationError::TranscriptionFailed`] naming the line.

use std::collections::BTreeSet;

use crate::diarization::eval::evaluation::{ScoringRegion, SpeakerTurn};
use crate::error::DictationError;

const MAX_LINES: usize = 1_000_000;

/// Parse an RTTM file: the recording ID, the speaker turns and the end of the last turn.
pub fn parse_rttm(text: &str) -> Result<(String, Vec<SpeakerTurn>, f64), DictationError> {
    let mut recording_id = None;
    let mut turns = Vec::new();
    let mut seen = BTreeSet::new();
    let mut duration: f64 = 0.0;
    for (index, line) in text.lines().enumerate() {
        if index >= MAX_LINES {
            return Err(invalid("RTTM contains too many lines"));
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.is_empty() || fields[0].starts_with('#') {
            continue;
        }
        if fields.len() < 8 || fields[0] != "SPEAKER" {
            return Err(invalid(format!("malformed RTTM line {}", index + 1)));
        }
        if fields[2] != "1" {
            return Err(invalid(format!(
                "unsupported RTTM channel at line {}",
                index + 1
            )));
        }
        let id = fields[1];
        if id.is_empty() || id == "<NA>" {
            return Err(invalid(format!(
                "invalid RTTM recording ID at line {}",
                index + 1
            )));
        }
        if let Some(existing) = &recording_id {
            if existing != id {
                return Err(invalid("RTTM contains multiple recording IDs"));
            }
        } else {
            recording_id = Some(id.to_owned());
        }
        let start = parse_nonnegative(fields[3], "RTTM start", index)?;
        let duration_seconds = parse_nonnegative(fields[4], "RTTM duration", index)?;
        if duration_seconds == 0.0 {
            return Err(invalid(format!("RTTM zero duration at line {}", index + 1)));
        }
        let end = start + duration_seconds;
        if !end.is_finite() || end > 14_400.0 {
            return Err(invalid(format!(
                "RTTM interval out of bounds at line {}",
                index + 1
            )));
        }
        let speaker = fields[7];
        if speaker.is_empty() || speaker == "<NA>" {
            return Err(invalid(format!(
                "invalid RTTM speaker at line {}",
                index + 1
            )));
        }
        let key = (start.to_bits(), end.to_bits(), speaker.to_owned());
        if !seen.insert(key) {
            return Err(invalid(format!(
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
    let recording_id = recording_id.ok_or_else(|| invalid("RTTM contains no SPEAKER records"))?;
    Ok((recording_id, turns, duration))
}

/// Parse a UEM file into the scored regions of one recording, each inside `duration` and, with
/// `expected_recording_id`, naming that recording.
pub fn parse_uem(
    text: &str,
    expected_recording_id: Option<&str>,
    duration: f64,
) -> Result<Vec<ScoringRegion>, DictationError> {
    let mut recording_id = None;
    let mut regions = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if index >= MAX_LINES {
            return Err(invalid("UEM contains too many lines"));
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.is_empty() || fields[0].starts_with('#') {
            continue;
        }
        if fields.len() != 4 {
            return Err(invalid(format!("malformed UEM line {}", index + 1)));
        }
        if fields[1] != "1" {
            return Err(invalid(format!(
                "unsupported UEM channel at line {}",
                index + 1
            )));
        }
        let id = fields[0];
        if id.is_empty() || id == "<NA>" {
            return Err(invalid(format!(
                "invalid UEM recording ID at line {}",
                index + 1
            )));
        }
        if let Some(existing) = &recording_id {
            if existing != id {
                return Err(invalid("UEM contains multiple recording IDs"));
            }
        } else {
            recording_id = Some(id.to_owned());
        }
        if let Some(expected) = expected_recording_id {
            if expected != id {
                return Err(invalid("UEM recording ID does not match RTTM reference"));
            }
        }
        let start = parse_nonnegative(fields[2], "UEM start", index)?;
        let end = parse_nonnegative(fields[3], "UEM end", index)?;
        if start >= end || end > duration {
            return Err(invalid(format!(
                "UEM interval out of bounds at line {}",
                index + 1
            )));
        }
        regions.push(ScoringRegion { start, end });
    }
    if recording_id.is_none() {
        return Err(invalid("UEM contains no regions"));
    }
    Ok(regions)
}

fn parse_nonnegative(value: &str, kind: &str, index: usize) -> Result<f64, DictationError> {
    let parsed = value
        .parse::<f64>()
        .map_err(|_| invalid(format!("invalid {kind} at line {}", index + 1)))?;
    if !parsed.is_finite() || parsed < 0.0 {
        return Err(invalid(format!("invalid {kind} at line {}", index + 1)));
    }
    Ok(parsed)
}

fn invalid(message: impl Into<String>) -> DictationError {
    DictationError::TranscriptionFailed(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(result: Result<impl std::fmt::Debug, DictationError>) -> String {
        result.expect_err("refused").to_string()
    }

    #[test]
    fn rttm_gives_the_recording_the_turns_and_the_end_of_the_last_one() {
        let text = "# a comment\n\nSPEAKER rec 1 0.000 1.500 <NA> <NA> A <NA> <NA>\n\
                    SPEAKER rec 1 2.000 0.500 <NA> <NA> B <NA> <NA>\n";
        let (recording, turns, duration) = parse_rttm(text).expect("valid");
        assert_eq!(recording, "rec");
        assert_eq!(duration, 2.5);
        assert_eq!(
            turns,
            vec![
                SpeakerTurn {
                    start: 0.0,
                    end: 1.5,
                    speakers: vec!["A".into()]
                },
                SpeakerTurn {
                    start: 2.0,
                    end: 2.5,
                    speakers: vec!["B".into()]
                },
            ]
        );
    }

    #[test]
    fn rttm_refuses_what_the_scoring_would_misread() {
        let cases = [
            ("SPEAKER rec 1\n", "malformed RTTM line 1"),
            (
                "SPEAKER rec 2 0 1 <NA> <NA> A <NA> <NA>\n",
                "unsupported RTTM channel at line 1",
            ),
            (
                "SPEAKER <NA> 1 0 1 <NA> <NA> A <NA> <NA>\n",
                "invalid RTTM recording ID at line 1",
            ),
            (
                "SPEAKER a 1 0 1 <NA> <NA> A <NA> <NA>\nSPEAKER b 1 1 1 <NA> <NA> A <NA> <NA>\n",
                "RTTM contains multiple recording IDs",
            ),
            (
                "SPEAKER rec 1 x 1 <NA> <NA> A <NA> <NA>\n",
                "invalid RTTM start at line 1",
            ),
            (
                "SPEAKER rec 1 0 0 <NA> <NA> A <NA> <NA>\n",
                "RTTM zero duration at line 1",
            ),
            (
                "SPEAKER rec 1 0 99999 <NA> <NA> A <NA> <NA>\n",
                "RTTM interval out of bounds at line 1",
            ),
            (
                "SPEAKER rec 1 0 1 <NA> <NA> <NA> <NA> <NA>\n",
                "invalid RTTM speaker at line 1",
            ),
            (
                "SPEAKER rec 1 0 1 <NA> <NA> A <NA> <NA>\nSPEAKER rec 1 0 1 <NA> <NA> A <NA> <NA>\n",
                "duplicate RTTM interval at line 2",
            ),
            ("# only a comment\n", "RTTM contains no SPEAKER records"),
        ];
        for (text, expected) in cases {
            assert!(message(parse_rttm(text)).contains(expected), "{text:?}");
        }
    }

    #[test]
    fn rttm_stops_at_the_line_limit() {
        let text = "\n".repeat(MAX_LINES + 1);
        assert!(message(parse_rttm(&text)).contains("RTTM contains too many lines"));
    }

    #[test]
    fn uem_regions_stay_inside_the_duration_and_name_the_recording() {
        let regions =
            parse_uem("rec 1 0.5 1.5\n# note\nrec 1 1.5 2.0\n", Some("rec"), 2.0).expect("valid");
        assert_eq!(
            regions,
            vec![
                ScoringRegion {
                    start: 0.5,
                    end: 1.5
                },
                ScoringRegion {
                    start: 1.5,
                    end: 2.0
                },
            ]
        );
        let cases = [
            ("rec 1 0 1 extra\n", "malformed UEM line 1"),
            ("rec 2 0 1\n", "unsupported UEM channel at line 1"),
            ("<NA> 1 0 1\n", "invalid UEM recording ID at line 1"),
            (
                "rec 1 0 1\nother 1 1 2\n",
                "UEM contains multiple recording IDs",
            ),
            (
                "other 1 0 1\n",
                "UEM recording ID does not match RTTM reference",
            ),
            ("rec 1 a b\n", "invalid UEM start at line 1"),
            ("rec 1 2 1\n", "UEM interval out of bounds at line 1"),
            ("rec 1 0 3\n", "UEM interval out of bounds at line 1"),
            ("", "UEM contains no regions"),
        ];
        for (text, expected) in cases {
            assert!(
                message(parse_uem(text, Some("rec"), 2.0)).contains(expected),
                "{text:?}"
            );
        }
    }
}
