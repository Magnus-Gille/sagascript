//! A bounded single HTTP byte-range plan, without reading or authorizing files.
pub use sagascript_core::meeting_media::MAX_RANGE_BYTES as MAX_BODY;
#[derive(Debug, PartialEq, Eq)]
pub struct MediaRange {
    pub start: u64,
    pub length: u64,
    pub partial: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub enum RangeError {
    Invalid,
    RequiresRange,
}
pub fn plan_range(header: Option<&str>, total: u64) -> Result<MediaRange, RangeError> {
    if total == 0 {
        return Err(RangeError::Invalid);
    }
    let Some(header) = header else {
        return if total <= MAX_BODY {
            Ok(MediaRange {
                start: 0,
                length: total,
                partial: false,
            })
        } else {
            Err(RangeError::RequiresRange)
        };
    };
    if header.len() > 128 {
        return Err(RangeError::Invalid);
    }
    let (left, right) = header
        .strip_prefix("bytes=")
        .and_then(|value| value.split_once('-'))
        .ok_or(RangeError::Invalid)?;
    let number = |value: &str| -> Result<u64, RangeError> {
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(RangeError::Invalid);
        }
        value.parse().map_err(|_| RangeError::Invalid)
    };
    let (start, end) = if left.is_empty() {
        let suffix = number(right)?;
        if suffix == 0 {
            return Err(RangeError::Invalid);
        }
        (total.saturating_sub(suffix), total - 1)
    } else {
        let start = number(left)?;
        let end = if right.is_empty() {
            total - 1
        } else {
            number(right)?.min(total - 1)
        };
        if start >= total || end < start {
            return Err(RangeError::Invalid);
        }
        (start, end)
    };
    Ok(MediaRange {
        start,
        length: (end - start + 1).min(MAX_BODY),
        partial: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_body_requires_small_file() {
        assert_eq!(
            plan_range(None, 10),
            Ok(MediaRange {
                start: 0,
                length: 10,
                partial: false
            })
        );
        assert_eq!(
            plan_range(None, MAX_BODY + 1),
            Err(RangeError::RequiresRange)
        );
    }
    #[test]
    fn valid_ranges_clip_at_eof_and_bound_allocation() {
        for (header, start, length) in [
            ("bytes=2-5", 2, 4),
            ("bytes=8-99", 8, 2),
            ("bytes=7-", 7, 3),
            ("bytes=-3", 7, 3),
            ("bytes=-99", 0, 10),
        ] {
            assert_eq!(
                plan_range(Some(header), 10),
                Ok(MediaRange {
                    start,
                    length,
                    partial: true
                })
            );
        }
        assert_eq!(
            plan_range(Some("bytes=0-"), MAX_BODY * 2).unwrap().length,
            MAX_BODY
        );
    }
    #[test]
    fn rejects_malformed_and_unsatisfiable_ranges() {
        for header in [
            "bytes=",
            "bytes=-0",
            "bytes=10-",
            "bytes=3-2",
            "bytes=0-1,3-4",
            "bytes=+1-2",
            "bytes= 1-2",
            "units=0-1",
            "bytes=18446744073709551616-",
        ] {
            assert_eq!(
                plan_range(Some(header), 10),
                Err(RangeError::Invalid),
                "{header}"
            );
        }
        assert!(plan_range(None, 0).is_err());
    }
}
