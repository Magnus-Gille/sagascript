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
}

impl SpeakerCountHint {
    /// Exactly `n` speakers.
    pub fn exact(n: usize) -> Self {
        Self { exact: Some(n), min: None, max: None }
    }

    /// Between `min` and `max` speakers; either bound may be omitted.
    pub fn range(min: Option<usize>, max: Option<usize>) -> Self {
        Self { exact: None, min, max }
    }

    /// Build a validated hint from the three optional CLI/UI values. `Ok(None)` means no hint.
    pub fn from_parts(
        exact: Option<usize>,
        min: Option<usize>,
        max: Option<usize>,
    ) -> Result<Option<Self>, String> {
        let hint = Self { exact, min, max };
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
        if let (Some(min), Some(max)) = (self.min, self.max) {
            if min > max {
                return Err(format!("min-speakers ({min}) must not exceed max-speakers ({max})"));
            }
        }
        Ok(())
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
        assert_eq!(SpeakerCountHint::from_parts(None, None, None), Ok(None));
        assert!(SpeakerCountHint::from_parts(Some(0), None, None).is_err());
        assert!(SpeakerCountHint::from_parts(None, Some(0), None).is_err());
        assert!(SpeakerCountHint::from_parts(None, None, Some(0)).is_err());
        assert!(SpeakerCountHint::from_parts(Some(2), Some(1), None).is_err());
        assert!(SpeakerCountHint::from_parts(Some(2), None, Some(3)).is_err());
        assert!(SpeakerCountHint::from_parts(None, Some(3), Some(2)).is_err());
        assert!(SpeakerCountHint::from_parts(None, Some(2), Some(2)).unwrap().is_some());
        assert_eq!(SpeakerCountHint::exact(3).bounds(), (3, Some(3)));
        assert_eq!(SpeakerCountHint::range(None, Some(4)).bounds(), (1, Some(4)));
        assert_eq!(SpeakerCountHint::range(Some(2), None).bounds(), (2, None));
    }

    #[test]
    fn serializes_only_set_fields() {
        let json = serde_json::to_string(&SpeakerCountHint::exact(2)).unwrap();
        assert_eq!(json, r#"{"exact":2}"#);
        let back: SpeakerCountHint = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SpeakerCountHint::exact(2));
    }
}
