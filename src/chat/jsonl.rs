//! Shared JSONL helpers for the per-day chat-data files
//! (`data/chat/history/<date>.jsonl`,
//! `data/chat/decisions/<date>.jsonl`, etc.).
//!
//! Centralizing the timestamp format and per-day file-path construction
//! here keeps every call site producing byte-identical output, so a
//! later refactor can't accidentally drift one stream's format relative
//! to another.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use tracing::warn;

/// Sentinel returned by [`iso_utc_millis`] when a `SystemTime` falls
/// outside chrono's representable range. The Unix-epoch string is
/// chosen so downstream JSONL parsers / log greppers still see a
/// well-formed RFC-3339 timestamp; a corrupt persisted timestamp logs
/// loud (via [`warn!`]) instead of panicking the audit-write call.
const ISO_UTC_MILLIS_SENTINEL: &str = "1970-01-01T00:00:00.000Z";

/// Format a `SystemTime` as a UTC ISO-8601 string with millisecond
/// precision and a trailing `Z`. This is the canonical `ts` shape for
/// every per-day JSONL stream under `data/chat/`.
///
/// **Fallible-but-infallible-looking**: if `t` predates the Unix
/// epoch or otherwise overflows chrono's representable range (corrupt
/// persisted timestamp — e.g. a structure deserialized from an
/// untrusted disk file with a wild seconds field), we log a
/// [`warn!`] and return [`ISO_UTC_MILLIS_SENTINEL`] instead of
/// panicking. The audit-write call sites uniformly invoke this helper
/// from non-error paths where a panic would abort an unrelated
/// player-chat handler; a sentinel keeps the JSONL record well-formed
/// while the warn line surfaces the corruption to the operator.
///
/// Note: `state.rs::iso_utc` deliberately diverges — it takes
/// `DateTime<Utc>` and emits `SecondsFormat::Secs` for `state.json`
/// fields. Do not unify the two without auditing every state.json reader.
pub(crate) fn iso_utc_millis(t: SystemTime) -> String {
    match system_time_to_chrono(t) {
        Some(dt) => dt.to_rfc3339_opts(SecondsFormat::Millis, true),
        None => {
            warn!(
                target: "chat::jsonl",
                "iso_utc_millis: SystemTime out of chrono range; using sentinel"
            );
            ISO_UTC_MILLIS_SENTINEL.to_string()
        }
    }
}

/// Convert a `SystemTime` to `DateTime<Utc>` without panicking on a
/// corrupt input. Handles both the pre-epoch case (`t < UNIX_EPOCH`)
/// and the post-epoch overflow case
/// (`(secs, nanos)` outside chrono's representable range).
fn system_time_to_chrono(t: SystemTime) -> Option<DateTime<Utc>> {
    let d = t.duration_since(UNIX_EPOCH).ok()?;
    let secs: i64 = d.as_secs().try_into().ok()?;
    let nanos: u32 = d.subsec_nanos();
    DateTime::<Utc>::from_timestamp(secs, nanos)
}

/// Same canonical millisecond-precision UTC ISO-8601 string, but for an
/// already-derived `DateTime<Utc>`. Use when the caller already holds a
/// `chrono::Utc::now()` value and would otherwise have to round-trip
/// through `SystemTime`.
pub(crate) fn iso_utc_millis_dt(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Per-day JSONL file path: `<dir>/<UTC-date>.jsonl` for `t`.
///
/// Mirrors [`iso_utc_millis`]'s out-of-range guard: a corrupt
/// `SystemTime` that would otherwise panic the `Into<DateTime<Utc>>`
/// conversion is logged via [`warn!`] and routed to a sentinel epoch
/// date (`1970-01-01`) so the audit-write call still produces a
/// well-formed path rather than aborting the caller. The sentinel
/// filename surfaces the corruption to the operator in directory
/// listings.
pub(crate) fn day_file(dir: &Path, t: SystemTime) -> PathBuf {
    match system_time_to_chrono(t) {
        Some(dt) => dir.join(format!("{}.jsonl", dt.date_naive())),
        None => {
            warn!(
                target: "chat::jsonl",
                "day_file: SystemTime out of chrono range; using epoch sentinel date"
            );
            dir.join("1970-01-01.jsonl")
        }
    }
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
    use std::time::Duration;

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
        assert!(p1.ends_with("2024-01-15.jsonl"), "day_file path = {:?}", p1);
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
    fn iso_utc_millis_returns_sentinel_for_pre_epoch_input() {
        // A `SystemTime` earlier than UNIX_EPOCH (e.g. a corrupt
        // persisted-timestamp field deserialized from disk) MUST NOT
        // panic the helper. The pre-fix `DateTime<Utc> = t.into()`
        // would crash on this input on platforms where SystemTime can
        // represent pre-epoch instants. The fix returns a sentinel and
        // logs a warn; assert the sentinel shape so the audit JSONL
        // stays parseable.
        let pre_epoch = UNIX_EPOCH - Duration::from_secs(1);
        let s = iso_utc_millis(pre_epoch);
        assert_eq!(s, "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn day_file_returns_epoch_sentinel_for_pre_epoch_input() {
        // Same out-of-range guard as `iso_utc_millis`: a corrupt
        // pre-epoch SystemTime should route to the epoch-date sentinel
        // filename, not panic.
        let dir = Path::new("data/chat/history");
        let pre_epoch = UNIX_EPOCH - Duration::from_secs(1);
        let p = day_file(dir, pre_epoch);
        assert!(
            p.ends_with("1970-01-01.jsonl"),
            "pre-epoch path should be sentinel, got: {:?}",
            p,
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
