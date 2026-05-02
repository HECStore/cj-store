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
