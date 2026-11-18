//! Wall-clock timestamps as text (RFC 3339, millisecond precision).
//!
//! One place for the three shapes the app writes, so the format is tested once
//! (replaces direct `chrono` use; RUSTSEC-2026-0543). The pure `format_*`
//! functions take a time so tests do not depend on the clock.

use jiff::{tz::TimeZone, Timestamp, Zoned};

/// `2026-11-17T14:03:05.123Z`: UTC with a `Z` suffix.
const UTC_MILLIS: &str = "%Y-%m-%dT%H:%M:%S%.3fZ";
/// `2026-11-17T15:03:05.123+01:00`: local time with its UTC offset.
const LOCAL_MILLIS: &str = "%Y-%m-%dT%H:%M:%S%.3f%:z";

pub fn format_utc_millis(at: Timestamp) -> String {
    at.strftime(UTC_MILLIS).to_string()
}

pub fn format_local_millis(at: &Zoned) -> String {
    at.strftime(LOCAL_MILLIS).to_string()
}

/// Now, in UTC, with a `Z` suffix.
pub fn utc_now_millis() -> String {
    format_utc_millis(Timestamp::now())
}

/// Now, in the computer's time zone, with the UTC offset. Falls back to UTC
/// (offset `+00:00`) if the zone cannot be determined.
pub fn local_now_millis() -> String {
    let now = Timestamp::now();
    let zone = TimeZone::try_system().unwrap_or(TimeZone::UTC);
    format_local_millis(&now.to_zoned(zone))
}

/// Today's date in UTC, `2026-11-17`.
pub fn utc_today() -> String {
    Timestamp::now().strftime("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().expect("timestamp")
    }

    #[test]
    fn utc_has_three_fraction_digits_and_a_z() {
        assert_eq!(
            format_utc_millis(at("2026-11-17T14:03:05.1Z")),
            "2026-11-17T14:03:05.100Z"
        );
        assert_eq!(
            format_utc_millis(at("2026-11-17T14:03:05Z")),
            "2026-11-17T14:03:05.000Z"
        );
        // Truncates, as chrono's millisecond format did.
        assert_eq!(
            format_utc_millis(at("2026-11-17T14:03:05.123987Z")),
            "2026-11-17T14:03:05.123Z"
        );
    }

    #[test]
    fn local_carries_the_offset_with_a_colon() {
        let stockholm =
            at("2026-11-17T14:03:05.123Z").to_zoned(TimeZone::get("Europe/Stockholm").unwrap());
        assert_eq!(
            format_local_millis(&stockholm),
            "2026-11-17T15:03:05.123+01:00"
        );
        let summer =
            at("2026-07-01T10:00:00Z").to_zoned(TimeZone::get("Europe/Stockholm").unwrap());
        assert_eq!(
            format_local_millis(&summer),
            "2026-07-01T12:00:00.000+02:00"
        );
        let utc = at("2026-11-17T14:03:05Z").to_zoned(TimeZone::UTC);
        assert_eq!(format_local_millis(&utc), "2026-11-17T14:03:05.000+00:00");
    }

    #[test]
    fn the_clock_versions_parse_back() {
        let utc = utc_now_millis();
        assert!(
            utc.ends_with('Z') && utc.parse::<Timestamp>().is_ok(),
            "{utc}"
        );
        let local = local_now_millis();
        assert!(local.parse::<Timestamp>().is_ok(), "{local}");
        let today = utc_today();
        assert_eq!(today.len(), 10);
    }
}
