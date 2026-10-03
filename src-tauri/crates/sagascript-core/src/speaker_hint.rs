//! Optional speaker-count hint for diarization (issue #305).
//!
//! The hint lives outside the `diarization` feature because the meeting document and the
//! reprocessing plan record it. It is a plain value: validation here, interpretation in
//! `diarization::clustering`.

use serde::{Deserialize, Serialize};

/// What the user knows about how many people speak in a recording.
///
/// Either an exact count, or a minimum and/or maximum. All fields absent means "no hint"; callers
/// normally hold `Option<SpeakerCountHint>` and use `None` for that.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeakerCountHint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<usize>,
    /// Only with `exact`: reach the count even if that merges clearly different voices. Without
    /// it, reducing the count stops at the merge-distance limit and reports the shortfall.
    #[serde(default, skip_serializing_if = "is_false")]
    pub force: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// What a hinted run delivered: whether the final speaker count satisfies the hint, and the count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakerHintOutcome {
    pub satisfied: bool,
    pub delivered: usize,
}

impl SpeakerCountHint {
    /// Exactly `n` speakers.
    pub fn exact(n: usize) -> Self {
        Self { exact: Some(n), ..Self::default() }
    }

    /// Between `min` and `max` speakers; either bound may be omitted.
    pub fn range(min: Option<usize>, max: Option<usize>) -> Self {
        Self { min, max, ..Self::default() }
    }

    /// Exactly `n` speakers even if that merges clearly different voices.
    pub fn forced(n: usize) -> Self {
        Self { exact: Some(n), force: true, ..Self::default() }
    }

    /// Build a validated hint from the optional CLI/UI values. `Ok(None)` means no hint.
    pub fn from_parts(
        exact: Option<usize>,
        min: Option<usize>,
        max: Option<usize>,
        force: bool,
    ) -> Result<Option<Self>, String> {
        let hint = Self { exact, min, max, force };
        hint.validate()?;
        Ok(if hint.is_empty() { None } else { Some(hint) })
    }

    pub fn is_empty(&self) -> bool {
        self.exact.is_none() && self.min.is_none() && self.max.is_none()
    }

    /// Every count must be at least 1, `min <= max`, and `exact` excludes `min` and `max`.
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [("speakers", self.exact), ("min-speakers", self.min), ("max-speakers", self.max)] {
            if value == Some(0) {
                return Err(format!("{name} must be at least 1"));
            }
        }
        if self.exact.is_some() && (self.min.is_some() || self.max.is_some()) {
            return Err("speakers cannot be combined with min-speakers or max-speakers".to_string());
        }
        if self.force && self.exact.is_none() {
            return Err("force applies only to an exact speaker count".to_string());
        }
        if let (Some(min), Some(max)) = (self.min, self.max) {
            if min > max {
                return Err(format!("min-speakers ({min}) must not exceed max-speakers ({max})"));
            }
        }
        Ok(())
    }

    /// Whether a final speaker count satisfies the hint.
    pub fn is_satisfied_by(&self, count: usize) -> bool {
        let (lo, hi) = self.bounds();
        count >= lo && hi.is_none_or(|hi| count <= hi)
    }

    /// Inclusive `(lowest, highest)` cluster count the hint allows. An exact count is
    /// `(n, Some(n))`; with no minimum the lowest is 1.
    pub fn bounds(&self) -> (usize, Option<usize>) {
        match self.exact {
            Some(n) => (n, Some(n)),
            None => (self.min.unwrap_or(1), self.max),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_rules() {
        assert_eq!(SpeakerCountHint::from_parts(None, None, None, false), Ok(None));
        assert!(SpeakerCountHint::from_parts(Some(0), None, None, false).is_err());
        assert!(SpeakerCountHint::from_parts(None, Some(0), None, false).is_err());
        assert!(SpeakerCountHint::from_parts(None, None, Some(0), false).is_err());
        assert!(SpeakerCountHint::from_parts(Some(2), Some(1), None, false).is_err());
        assert!(SpeakerCountHint::from_parts(Some(2), None, Some(3), false).is_err());
        assert!(SpeakerCountHint::from_parts(None, Some(3), Some(2), false).is_err());
        assert!(SpeakerCountHint::from_parts(None, Some(2), Some(2), false).unwrap().is_some());
        assert_eq!(SpeakerCountHint::exact(3).bounds(), (3, Some(3)));
        assert_eq!(SpeakerCountHint::range(None, Some(4)).bounds(), (1, Some(4)));
        assert_eq!(SpeakerCountHint::range(Some(2), None).bounds(), (2, None));
    }

    #[test]
    fn force_requires_exact_and_satisfaction_follows_bounds() {
        assert!(SpeakerCountHint::from_parts(Some(2), None, None, true).unwrap().unwrap().force);
        assert!(SpeakerCountHint::from_parts(None, Some(2), None, true).is_err());
        assert!(SpeakerCountHint::exact(3).is_satisfied_by(3));
        assert!(!SpeakerCountHint::exact(3).is_satisfied_by(4));
        assert!(SpeakerCountHint::range(Some(2), Some(4)).is_satisfied_by(4));
        assert!(!SpeakerCountHint::range(Some(2), None).is_satisfied_by(1));
        assert_eq!(serde_json::to_string(&SpeakerCountHint::forced(2)).unwrap(), r#"{"exact":2,"force":true}"#);
    }

    #[test]
    fn serializes_only_set_fields() {
        let json = serde_json::to_string(&SpeakerCountHint::exact(2)).unwrap();
        assert_eq!(json, r#"{"exact":2}"#);
        let back: SpeakerCountHint = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SpeakerCountHint::exact(2));
    }
}
