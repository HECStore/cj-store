//! Read-only view over `data/trades/*.json`.
//!
//! A trade is one file per record (filename = the RFC3339 timestamp with
//! colons replaced by dashes; see [`crate::types::Trade::save`]). That
//! lets us prune by `since` BEFORE opening any file: parse the filename,
//! drop survivors that are older than the cursor, deserialize the rest.
//!
//! Why not call `crate::types::Trade::load_all_with_limit`: it returns
//! the last N filenames with no content filter. Asking it for "the
//! latest iron trade" returns wrong answers when iron is older than N.
//! Filename-level pruning + content filtering on survivors is the only
//! correct shape for these queries.

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Trade history directory. Mirrors `crate::types::Trade::TRADES_DIR`
/// but owned by chat so the chat module never imports `crate::types::`.
pub const TRADES_DIR: &str = "data/trades";

/// Cap on how many trade files we'll actually `read_to_string` +
/// deserialize per scan. The full directory listing is still collected
/// and lex-sorted (newest-first) so prune-by-`since` and ordering stay
/// correct; this only bounds the WORK done after the prune.
///
/// The bound is sized for expected p99 trade volume per query window
/// against the chat-pipeline thread occupancy budget: an unbounded sync
/// scan on a tokio blocking thread can occupy a worker for several
/// seconds at 50K+ trades, which starves other chat tools sharing the
/// blocking pool. 5000 deserializations is well under one second on a
/// commodity disk and keeps the `query_trades` tail latency predictable
/// while still covering the head of history that the model actually
/// asks about. When the cap is hit before `limit` matches accumulate,
/// the scanner signals truncation via the `scan_truncated` bool.
pub const MAX_DESERIALIZE: usize = 5000;

/// Minimal deserializer for one trade JSON file.
///
/// `trade_type` is kept as `String` — we don't pull in
/// `crate::types::TradeType` because that would re-anchor the chat
/// module on `crate::types::*`. The trade bot serializes the enum as
/// a Pascal-case variant string (`"Buy"`, `"Sell"`, `"AddStock"`, ...);
/// chat just round-trips the string.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TradeView {
    pub trade_type: String,
    pub item: String,
    pub amount: i32,
    pub amount_currency: f64,
    pub user_uuid: String,
    pub timestamp: DateTime<Utc>,
}

/// Filters applied during a trade scan. Every field is optional; the
/// scan returns trades that match every populated filter.
///
/// Filename-level prune: only `since` can be applied without opening
/// files — the others require deserialization.
#[derive(Debug, Default, Clone)]
pub struct TradeFilter {
    /// Only trades strictly newer than this timestamp.
    pub since: Option<DateTime<Utc>>,
    /// Exact item-id match, case-insensitive (compared after
    /// `minecraft:` prefix is stripped).
    pub item: Option<String>,
    /// Exact UUID match (case-insensitive).
    pub user_uuid: Option<String>,
    /// Exact `trade_type` match (Pascal-case, e.g. `"Buy"`).
    pub trade_type: Option<String>,
}

/// Load up to `limit` trades matching `filter`, newest-first.
///
/// Filename-level prune by `since` runs before any file is opened.
/// Content filters are applied after deserialization. Files that fail
/// to deserialize are skipped silently — the trade bot's autosave
/// window can leave a partial file briefly.
///
/// `limit` is capped at 50 by the chat tool layer; this function
/// trusts the cap is already applied.
pub async fn scan_filtered(
    filter: TradeFilter,
    limit: usize,
) -> Result<(Vec<TradeView>, bool), String> {
    tokio::task::spawn_blocking(move || {
        scan_filtered_in_dir(std::path::Path::new(TRADES_DIR), filter, limit)
    })
    .await
    .map_err(|e| format!("scan_filtered join: {e}"))?
}

/// Inner sync helper, exposed at module scope so tests can point at a
/// temp dir. Production code calls [`scan_filtered`] which threads
/// the constant `TRADES_DIR`.
///
/// Returns `(trades, scan_truncated)`. `scan_truncated` is `true` when
/// the scan stopped because [`MAX_DESERIALIZE`] files were opened
/// before `limit` matches accumulated; the returned `trades` then
/// covers only the head of history.
pub fn scan_filtered_in_dir(
    dir: &std::path::Path,
    filter: TradeFilter,
    limit: usize,
) -> Result<(Vec<TradeView>, bool), String> {
    if !dir.exists() {
        return Ok((Vec::new(), false));
    }

    // The filename is `<rfc3339>.json` with `:` replaced by `-`. The
    // replacement preserves lexicographic-equals-chronological order
    // (the only `:`s in an RFC3339 UTC timestamp are inside the
    // strictly-positional time and timezone fields, all of which sort
    // identically under `-`). To prune by `since` we compare filename
    // strings directly: `filename > since_str` ⇒ `trade.timestamp >
    // since` (modulo the trailing `.json` and any sub-second component
    // that is also lex-monotonic).
    //
    // LOAD-BEARING BYTE-ORDERING INVARIANT: chrono's `to_rfc3339()`
    // (no `_opts`) emits the UTC suffix as `+00:00` (not `Z`). After
    // the `:`→`-` swap the suffix becomes `+00-00`. The sub-second
    // separator is `.`. Relevant byte values:
    //   `+` = 0x2B  <  `.` = 0x2E  <  `'0'..='9'` = 0x30..=0x39
    // This means a stem with NO fractional part (`…T10:00:00+00:00`)
    // sorts BEFORE a stem with one (`…T10:00:00.5+00:00`) sorts BEFORE
    // a stem at the next whole second (`…T10:00:01+00:00`), which is
    // chronological order. Sub-second precision can be mixed freely
    // between filenames and lex-order still matches time-order.
    //
    // WARNING: do NOT switch to `to_rfc3339_opts(SecondsFormat::Secs,
    // true)` — that emits `Z` (0x5A), which is GREATER than every
    // digit, so a `Z`-suffixed whole-second stem sorts AFTER a stem
    // for the next whole second under sub-second precision (or after
    // a fractional stem in the same second), silently breaking the
    // prune at the second boundary. The mixed-precision tests in this
    // file are pinned to fail if that swap is ever introduced.
    let since_filename_prefix = filter
        .since
        .as_ref()
        .map(|t| t.to_rfc3339().replace(':', "-"));

    let mut filename_paths: Vec<std::path::PathBuf> = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read_dir trades: {e}"))?;
    for ent in entries.flatten() {
        let path = ent.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let stem = match name.strip_suffix(".json") {
            Some(s) => s,
            None => continue,
        };
        if let Some(ref prefix) = since_filename_prefix {
            // Strict-greater: `trade.timestamp > since`. Compare the
            // stem (the timestamp portion) against the prefix.
            if stem.as_bytes() <= prefix.as_bytes() {
                continue;
            }
        }
        filename_paths.push(path);
    }

    // Newest-first: filenames sort lexicographically the same as
    // timestamps, so reverse-sort puts the freshest trades first.
    filename_paths.sort_by(|a, b| b.file_name().cmp(&a.file_name()));

    // Normalize the item filter: strip a leading `minecraft:` so a
    // caller using either form matches the on-disk JSON (`Trade::item`
    // round-trips through `ItemId` which strips the prefix). Lowercase
    // it as defense-in-depth so the comparison below is symmetric with
    // the user-uuid filter and resilient to future callers that haven't
    // already normalized via `validate_item_id`.
    let item_filter = filter.item.as_ref().map(|s| {
        s.strip_prefix("minecraft:")
            .unwrap_or(s.as_str())
            .to_ascii_lowercase()
    });
    let user_uuid_filter_lc = filter.user_uuid.as_ref().map(|s| s.to_lowercase());
    let trade_type_filter = filter.trade_type.clone();

    let mut out: Vec<TradeView> = Vec::new();
    // Count files actually opened (read_to_string attempted), not just
    // iterated — the cap protects against deserialization cost, which is
    // the dominant per-file work. We bail BEFORE the next read once the
    // cap is reached and `limit` is still unmet, signalling truncation.
    let mut deserialized: usize = 0;
    let mut scan_truncated = false;
    for path in filename_paths {
        if out.len() >= limit {
            break;
        }
        if deserialized >= MAX_DESERIALIZE {
            scan_truncated = true;
            break;
        }
        deserialized += 1;
        let body = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            // write_atomic uses a rename-into-place, so a NotFound
            // races a concurrent rename; skip silently. Anything else
            // (PermissionDenied, InvalidData, …) is real corruption that
            // operators need to see — do not swallow.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                tracing::warn!(
                    "[chat/trade] read failed for {}: {} ({:?})",
                    path.display(),
                    e,
                    e.kind()
                );
                continue;
            }
        };
        let trade: TradeView = match serde_json::from_str(&body) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("[chat/trade] parse failed for {}: {e}", path.display());
                continue;
            }
        };
        if let Some(ref it) = item_filter
            && !trade.item.eq_ignore_ascii_case(it)
        {
            continue;
        }
        if let Some(ref uu) = user_uuid_filter_lc
            && !trade.user_uuid.eq_ignore_ascii_case(uu)
        {
            continue;
        }
        if let Some(ref tt) = trade_type_filter
            && trade.trade_type != *tt
        {
            continue;
        }
        out.push(trade);
    }
    Ok((out, scan_truncated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};

    fn write_trade(dir: &std::path::Path, ts: DateTime<Utc>, json: &str) {
        let stem = ts.to_rfc3339().replace(':', "-");
        let path = dir.join(format!("{stem}.json"));
        std::fs::write(&path, json).unwrap();
    }

    fn fixture_dir(tag: &str) -> std::path::PathBuf {
        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "cj-store-trade-view-{}-{}-{tag}",
            std::process::id(),
            nanos,
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn scan_filters_by_item_and_returns_newest_first() {
        let dir = fixture_dir("item-newest");
        let t1 = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        let t2 = Utc.with_ymd_and_hms(2026, 1, 2, 10, 0, 0).unwrap();
        let t3 = Utc.with_ymd_and_hms(2026, 1, 3, 10, 0, 0).unwrap();
        write_trade(
            &dir,
            t1,
            r#"{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-01T10:00:00Z"}"#,
        );
        write_trade(
            &dir,
            t2,
            r#"{"trade_type":"Buy","item":"diamond","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-02T10:00:00Z"}"#,
        );
        write_trade(
            &dir,
            t3,
            r#"{"trade_type":"Buy","item":"iron_ingot","amount":2,"amount_currency":4.0,"user_uuid":"u","timestamp":"2026-01-03T10:00:00Z"}"#,
        );
        let f = TradeFilter {
            item: Some("iron_ingot".to_string()),
            ..Default::default()
        };
        let (out, truncated) = scan_filtered_in_dir(&dir, f, 10).unwrap();
        assert!(!truncated);
        assert_eq!(out.len(), 2);
        // Newest first.
        assert_eq!(out[0].timestamp, t3);
        assert_eq!(out[1].timestamp, t1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_item_filter_is_case_insensitive() {
        // Defense-in-depth: even though `validate_item_id` lowercases at
        // the chat-tool callsite, this scanner accepts an upper-case
        // filter against a lower-case on-disk trade. Symmetric with the
        // user-uuid filter, which is already case-insensitive.
        let dir = fixture_dir("item-case");
        let t1 = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        write_trade(
            &dir,
            t1,
            r#"{"trade_type":"Buy","item":"diamond","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-01T10:00:00Z"}"#,
        );
        let f = TradeFilter {
            item: Some("DIAMOND".to_string()),
            ..Default::default()
        };
        let (out, truncated) = scan_filtered_in_dir(&dir, f, 10).unwrap();
        assert!(!truncated);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].timestamp, t1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_prunes_by_since_at_filename_level() {
        let dir = fixture_dir("since-prune");
        let t1 = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        let t2 = Utc.with_ymd_and_hms(2026, 1, 2, 10, 0, 0).unwrap();
        let t3 = Utc.with_ymd_and_hms(2026, 1, 3, 10, 0, 0).unwrap();
        write_trade(
            &dir,
            t1,
            r#"{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-01T10:00:00Z"}"#,
        );
        write_trade(
            &dir,
            t2,
            r#"{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-02T10:00:00Z"}"#,
        );
        write_trade(
            &dir,
            t3,
            r#"{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-03T10:00:00Z"}"#,
        );
        let f = TradeFilter {
            since: Some(t1),
            ..Default::default()
        };
        // Strict greater-than semantics: t1 itself is excluded.
        let (out, truncated) = scan_filtered_in_dir(&dir, f, 10).unwrap();
        assert!(!truncated);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|t| t.timestamp > t1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_keeps_subsecond_trade_when_since_lacks_subsecond() {
        // Regression: the lex-prune fast path must keep a sub-second
        // trade when `since` is on the same whole second but has no
        // fractional component. After the `:`→`-` swap the stems are:
        //   trade:  2026-01-01T10-00-00.123456789+00-00
        //   since:  2026-01-01T10-00-00+00-00
        // Byte comparison at the first differing position is `.` (0x2E)
        // vs `+` (0x2B) — the trade stem is greater, so it survives.
        let dir = fixture_dir("subsec-keep");
        let ts = Utc
            .with_ymd_and_hms(2026, 1, 1, 10, 0, 0)
            .unwrap()
            .with_nanosecond(123_456_789)
            .unwrap();
        write_trade(
            &dir,
            ts,
            r#"{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-01T10:00:00.123456789Z"}"#,
        );
        let since = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        let f = TradeFilter {
            since: Some(since),
            ..Default::default()
        };
        let (out, truncated) = scan_filtered_in_dir(&dir, f, 10).unwrap();
        assert!(!truncated);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].timestamp, ts);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_distinguishes_two_subsecond_trades_in_same_second() {
        // Two trades in the same whole second, distinguishable only
        // by their fractional part. `since` = the earlier one, so
        // strict-greater excludes it and keeps the later one.
        let dir = fixture_dir("subsec-pair");
        let early = Utc
            .with_ymd_and_hms(2026, 1, 1, 10, 0, 0)
            .unwrap()
            .with_nanosecond(1_000)
            .unwrap();
        let late = Utc
            .with_ymd_and_hms(2026, 1, 1, 10, 0, 0)
            .unwrap()
            .with_nanosecond(999_999_000)
            .unwrap();
        write_trade(
            &dir,
            early,
            r#"{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-01T10:00:00.000001Z"}"#,
        );
        write_trade(
            &dir,
            late,
            r#"{"trade_type":"Buy","item":"iron_ingot","amount":2,"amount_currency":4.0,"user_uuid":"u","timestamp":"2026-01-01T10:00:00.999999Z"}"#,
        );
        let f = TradeFilter {
            since: Some(early),
            ..Default::default()
        };
        let (out, truncated) = scan_filtered_in_dir(&dir, f, 10).unwrap();
        assert!(!truncated);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].timestamp, late);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_orders_mixed_precision_newest_first() {
        // Three trades, mixed precision, all within ~1s. The lex sort
        // on filenames must produce chronological newest-first order:
        //   T10:00:01           (whole second)
        //   T10:00:00.500…      (fractional, same second as #3)
        //   T10:00:00           (whole second, oldest)
        // This pins the `+` < `.` < digit byte ordering invariant.
        let dir = fixture_dir("mixed-prec-order");
        let t_whole_early = Utc.with_ymd_and_hms(2026, 1, 2, 10, 0, 0).unwrap();
        let t_frac = Utc
            .with_ymd_and_hms(2026, 1, 2, 10, 0, 0)
            .unwrap()
            .with_nanosecond(500_000_000)
            .unwrap();
        let t_whole_late = Utc.with_ymd_and_hms(2026, 1, 2, 10, 0, 1).unwrap();
        write_trade(
            &dir,
            t_whole_early,
            r#"{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-02T10:00:00Z"}"#,
        );
        write_trade(
            &dir,
            t_frac,
            r#"{"trade_type":"Buy","item":"iron_ingot","amount":2,"amount_currency":4.0,"user_uuid":"u","timestamp":"2026-01-02T10:00:00.500000000Z"}"#,
        );
        write_trade(
            &dir,
            t_whole_late,
            r#"{"trade_type":"Buy","item":"iron_ingot","amount":3,"amount_currency":6.0,"user_uuid":"u","timestamp":"2026-01-02T10:00:01Z"}"#,
        );
        let (out, truncated) = scan_filtered_in_dir(&dir, TradeFilter::default(), 10).unwrap();
        assert!(!truncated);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].timestamp, t_whole_late);
        assert_eq!(out[1].timestamp, t_frac);
        assert_eq!(out[2].timestamp, t_whole_early);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_with_subsecond_since_returns_strict_greater_only() {
        // Same fixture as `scan_orders_mixed_precision_newest_first`,
        // but with `since` set to the fractional middle trade. Strict-
        // greater excludes both the `.500` itself and the older whole-
        // second trade; only the later whole-second trade survives.
        let dir = fixture_dir("subsec-since");
        let t_whole_early = Utc.with_ymd_and_hms(2026, 1, 2, 10, 0, 0).unwrap();
        let t_frac = Utc
            .with_ymd_and_hms(2026, 1, 2, 10, 0, 0)
            .unwrap()
            .with_nanosecond(500_000_000)
            .unwrap();
        let t_whole_late = Utc.with_ymd_and_hms(2026, 1, 2, 10, 0, 1).unwrap();
        write_trade(
            &dir,
            t_whole_early,
            r#"{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-02T10:00:00Z"}"#,
        );
        write_trade(
            &dir,
            t_frac,
            r#"{"trade_type":"Buy","item":"iron_ingot","amount":2,"amount_currency":4.0,"user_uuid":"u","timestamp":"2026-01-02T10:00:00.500000000Z"}"#,
        );
        write_trade(
            &dir,
            t_whole_late,
            r#"{"trade_type":"Buy","item":"iron_ingot","amount":3,"amount_currency":6.0,"user_uuid":"u","timestamp":"2026-01-02T10:00:01Z"}"#,
        );
        let f = TradeFilter {
            since: Some(t_frac),
            ..Default::default()
        };
        let (out, truncated) = scan_filtered_in_dir(&dir, f, 10).unwrap();
        assert!(!truncated);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].timestamp, t_whole_late);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_filters_by_trade_type_and_user_uuid() {
        let dir = fixture_dir("type-uuid");
        let t1 = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        let t2 = Utc.with_ymd_and_hms(2026, 1, 2, 10, 0, 0).unwrap();
        let t3 = Utc.with_ymd_and_hms(2026, 1, 3, 10, 0, 0).unwrap();
        write_trade(
            &dir,
            t1,
            r#"{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"AAAA","timestamp":"2026-01-01T10:00:00Z"}"#,
        );
        write_trade(
            &dir,
            t2,
            r#"{"trade_type":"Sell","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"BBBB","timestamp":"2026-01-02T10:00:00Z"}"#,
        );
        write_trade(
            &dir,
            t3,
            r#"{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"BBBB","timestamp":"2026-01-03T10:00:00Z"}"#,
        );
        let f = TradeFilter {
            trade_type: Some("Buy".to_string()),
            user_uuid: Some("bbbb".to_string()), // case-insensitive
            ..Default::default()
        };
        let (out, truncated) = scan_filtered_in_dir(&dir, f, 10).unwrap();
        assert!(!truncated);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].timestamp, t3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_respects_limit() {
        let dir = fixture_dir("limit");
        for d in 1..=5 {
            let ts = Utc.with_ymd_and_hms(2026, 1, d, 10, 0, 0).unwrap();
            let json = format!(
                r#"{{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-{d:02}T10:00:00Z"}}"#
            );
            write_trade(&dir, ts, &json);
        }
        let (out, truncated) = scan_filtered_in_dir(&dir, TradeFilter::default(), 2).unwrap();
        assert!(!truncated);
        assert_eq!(out.len(), 2);
        // Newest first: day-5 then day-4.
        use chrono::Datelike;
        assert_eq!(out[0].timestamp.day(), 5);
        assert_eq!(out[1].timestamp.day(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_strips_minecraft_prefix_from_item_filter() {
        let dir = fixture_dir("prefix-item");
        let t1 = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        write_trade(
            &dir,
            t1,
            r#"{"trade_type":"Buy","item":"diamond","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-01T10:00:00Z"}"#,
        );
        let f = TradeFilter {
            item: Some("minecraft:diamond".to_string()),
            ..Default::default()
        };
        let (out, truncated) = scan_filtered_in_dir(&dir, f, 10).unwrap();
        assert!(!truncated);
        assert_eq!(out.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_skips_malformed_json() {
        let dir = fixture_dir("malformed");
        let t1 = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        let t2 = Utc.with_ymd_and_hms(2026, 1, 2, 10, 0, 0).unwrap();
        write_trade(
            &dir,
            t1,
            r#"{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-01T10:00:00Z"}"#,
        );
        write_trade(&dir, t2, "not json");
        let (out, truncated) = scan_filtered_in_dir(&dir, TradeFilter::default(), 10).unwrap();
        assert!(!truncated);
        assert_eq!(out.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_returns_empty_when_dir_missing() {
        let dir = std::env::temp_dir().join(format!(
            "cj-store-trade-view-missing-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        ));
        // Do NOT create the dir.
        let (out, truncated) = scan_filtered_in_dir(&dir, TradeFilter::default(), 10).unwrap();
        assert!(!truncated);
        assert!(out.is_empty());
    }

    #[test]
    fn scan_signals_truncation_when_max_deserialize_hit() {
        // Write MAX_DESERIALIZE + 5 trades, all matching the filter so
        // every file would deserialize. With `limit` larger than the
        // cap, the scan must stop at MAX_DESERIALIZE files and signal
        // truncation. We use a small synthetic timestamp range to keep
        // the test fast — the cap behaves identically regardless of
        // timestamp density.
        let dir = fixture_dir("max-deserialize");
        let total = MAX_DESERIALIZE + 5;
        for i in 0..total {
            // Spread across seconds within a day to keep filenames
            // unique without overflowing nanoseconds.
            let ts = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
                + chrono::Duration::seconds(i as i64);
            let json = format!(
                r#"{{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"{}"}}"#,
                ts.to_rfc3339(),
            );
            write_trade(&dir, ts, &json);
        }
        // Ask for more matches than the cap; scan must stop at the cap.
        let (out, truncated) =
            scan_filtered_in_dir(&dir, TradeFilter::default(), MAX_DESERIALIZE + 10).unwrap();
        assert!(truncated, "expected scan_truncated when cap reached");
        assert_eq!(out.len(), MAX_DESERIALIZE);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
