//! Shared JSONL helpers for the per-day chat-data files
//! (`data/chat/history/<date>.jsonl`,
//! `data/chat/decisions/<date>.jsonl`, etc.).
//!
//! Centralizing the timestamp format and per-day file-path construction
//! here keeps every call site producing byte-identical output, so a
//! later refactor can't accidentally drift one stream's format relative
//! to another.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};

/// Format a `SystemTime` as a UTC ISO-8601 string with millisecond
/// precision and a trailing `Z`. This is the canonical `ts` shape for
/// every per-day JSONL stream under `data/chat/`.
///
/// Note: `state.rs::iso_utc` deliberately diverges — it takes
/// `DateTime<Utc>` and emits `SecondsFormat::Secs` for `state.json`
/// fields. Do not unify the two without auditing every state.json reader.
pub(crate) fn iso_utc_millis(t: SystemTime) -> String {
    let dt: DateTime<Utc> = t.into();
    dt.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Same canonical millisecond-precision UTC ISO-8601 string, but for an
/// already-derived `DateTime<Utc>`. Use when the caller already holds a
/// `chrono::Utc::now()` value and would otherwise have to round-trip
/// through `SystemTime`.
pub(crate) fn iso_utc_millis_dt(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Per-day JSONL file path: `<dir>/<UTC-date>.jsonl` for `t`.
pub(crate) fn day_file(dir: &Path, t: SystemTime) -> PathBuf {
    let dt: DateTime<Utc> = t.into();
    dir.join(format!("{}.jsonl", dt.date_naive()))
}

/// Per-day JSONL file path: `<dir>/<date>.jsonl` for an already-derived
/// `NaiveDate`. Use when the caller is iterating UTC dates directly
/// (e.g. trust-ladder scans across the last N days) and would otherwise
/// have to round-trip through `SystemTime`.
pub(crate) fn day_file_for_date(dir: &Path, date: NaiveDate) -> PathBuf {
    dir.join(format!("{}.jsonl", date))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    use chrono::TimeZone;

    // 2024-01-15T10:30:00Z
    const FIXTURE_SECS: u64 = 1_705_314_600;

    #[test]
    fn iso_utc_millis_pins_millisecond_precision_with_trailing_z() {
        let t = UNIX_EPOCH + Duration::from_secs(FIXTURE_SECS);
        assert_eq!(iso_utc_millis(t), "2024-01-15T10:30:00.000Z");
    }

    #[test]
    fn iso_utc_millis_dt_matches_iso_utc_millis_byte_for_byte() {
        let t = UNIX_EPOCH + Duration::from_secs(FIXTURE_SECS);
        let dt = Utc.timestamp_opt(FIXTURE_SECS as i64, 0).unwrap();
        assert_eq!(iso_utc_millis(t), iso_utc_millis_dt(dt));
        assert_eq!(iso_utc_millis_dt(dt), "2024-01-15T10:30:00.000Z");
    }

    #[test]
    fn iso_utc_millis_renders_non_zero_millis_at_millisecond_precision() {
        let t = UNIX_EPOCH + Duration::from_millis(1_705_314_600_123);
        assert_eq!(iso_utc_millis(t), "2024-01-15T10:30:00.123Z");
    }

    #[test]
    fn day_file_and_day_file_for_date_agree_on_filename() {
        let dir = Path::new("data/chat/history");
        let t = UNIX_EPOCH + Duration::from_secs(FIXTURE_SECS);
        let date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let p1 = day_file(dir, t);
        let p2 = day_file_for_date(dir, date);
        assert!(
            p1.ends_with("2024-01-15.jsonl"),
            "day_file path = {:?}",
            p1
        );
        assert!(
            p2.ends_with("2024-01-15.jsonl"),
            "day_file_for_date path = {:?}",
            p2
        );
        assert_eq!(p1, p2);
    }

    #[test]
    fn day_file_rotates_on_utc_midnight_boundary() {
        let dir = Path::new("data/chat/history");
        // 2024-01-15T23:59:00Z — last minute of the UTC day
        let before = UNIX_EPOCH + Duration::from_secs(1_705_363_140);
        // 2024-01-16T00:01:00Z — first minute of the next UTC day
        let after = UNIX_EPOCH + Duration::from_secs(1_705_363_260);
        assert!(
            day_file(dir, before).ends_with("2024-01-15.jsonl"),
            "before-midnight path = {:?}",
            day_file(dir, before)
        );
        assert!(
            day_file(dir, after).ends_with("2024-01-16.jsonl"),
            "after-midnight path = {:?}",
            day_file(dir, after)
        );
    }

    #[test]
    fn day_file_uses_utc_not_local_time() {
        // Pin that day_file always rotates on the UTC date, never the
        // host's local date. Both instants below are on 2024-01-15 in
        // UTC, but pick times-of-day that fall on a *different* local
        // date for hosts both east and west of UTC:
        //
        //   - 23:49:55Z is 2024-01-16 in any TZ at UTC+00:11 or later
        //     (i.e. essentially every TZ east of UTC, e.g. CET/Asia).
        //   - 00:00:05Z is 2024-01-14 in any TZ at UTC-00:01 or earlier
        //     (i.e. every TZ west of UTC, e.g. all of the Americas).
        //
        // A `Local::now()` regression in day_file would route at least
        // one of these to a different filename on most CI hosts.
        let dir = Path::new("x");

        // 2024-01-15T23:49:55Z
        let east_of_utc = UNIX_EPOCH + Duration::from_secs(1_705_362_595);
        assert!(
            day_file(dir, east_of_utc).ends_with("2024-01-15.jsonl"),
            "east-of-utc path = {:?}",
            day_file(dir, east_of_utc)
        );

        // 2024-01-15T00:00:05Z
        let west_of_utc = UNIX_EPOCH + Duration::from_secs(1_705_276_805);
        assert!(
            day_file(dir, west_of_utc).ends_with("2024-01-15.jsonl"),
            "west-of-utc path = {:?}",
            day_file(dir, west_of_utc)
        );
    }
}
