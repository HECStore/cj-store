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
    /// Exact item-id match (compared after `minecraft:` prefix is
    /// stripped).
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
) -> Result<Vec<TradeView>, String> {
    tokio::task::spawn_blocking(move || scan_filtered_in_dir(std::path::Path::new(TRADES_DIR), filter, limit))
        .await
        .map_err(|e| format!("scan_filtered join: {e}"))?
}

/// Inner sync helper, exposed at module scope so tests can point at a
/// temp dir. Production code calls [`scan_filtered`] which threads
/// the constant `TRADES_DIR`.
pub fn scan_filtered_in_dir(
    dir: &std::path::Path,
    filter: TradeFilter,
    limit: usize,
) -> Result<Vec<TradeView>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    // The filename is `<rfc3339>.json` with `:` replaced by `-`. The
    // replacement preserves lexicographic-equals-chronological order
    // (the only `:`s in an RFC3339 UTC timestamp are inside the
    // strictly-positional time and timezone fields, all of which sort
    // identically under `-`). To prune by `since` we compare filename
    // strings directly: `filename > since_str` ⇒ `trade.timestamp >
    // since` (modulo the trailing `.json` and any sub-second component
    // that is also lex-monotonic).
    let since_filename_prefix = filter
        .since
        .as_ref()
        .map(|t| t.to_rfc3339().replace(':', "-"));

    let mut filename_paths: Vec<std::path::PathBuf> = Vec::new();
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("read_dir trades: {e}"))?;
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
    // round-trips through `ItemId` which strips the prefix).
    let item_filter = filter.item.as_ref().map(|s| {
        s.strip_prefix("minecraft:").unwrap_or(s.as_str()).to_string()
    });
    let user_uuid_filter_lc = filter.user_uuid.as_ref().map(|s| s.to_lowercase());
    let trade_type_filter = filter.trade_type.clone();

    let mut out: Vec<TradeView> = Vec::new();
    for path in filename_paths {
        if out.len() >= limit {
            break;
        }
        let body = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            // write_atomic uses a rename-into-place, so a NotFound
            // races a concurrent rename; skip and move on.
            Err(_) => continue,
        };
        let trade: TradeView = match serde_json::from_str(&body) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if let Some(ref it) = item_filter
            && trade.item != *it
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
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

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
        write_trade(&dir, t1, r#"{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-01T10:00:00Z"}"#);
        write_trade(&dir, t2, r#"{"trade_type":"Buy","item":"diamond","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-02T10:00:00Z"}"#);
        write_trade(&dir, t3, r#"{"trade_type":"Buy","item":"iron_ingot","amount":2,"amount_currency":4.0,"user_uuid":"u","timestamp":"2026-01-03T10:00:00Z"}"#);
        let f = TradeFilter {
            item: Some("iron_ingot".to_string()),
            ..Default::default()
        };
        let out = scan_filtered_in_dir(&dir, f, 10).unwrap();
        assert_eq!(out.len(), 2);
        // Newest first.
        assert_eq!(out[0].timestamp, t3);
        assert_eq!(out[1].timestamp, t1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_prunes_by_since_at_filename_level() {
        let dir = fixture_dir("since-prune");
        let t1 = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        let t2 = Utc.with_ymd_and_hms(2026, 1, 2, 10, 0, 0).unwrap();
        let t3 = Utc.with_ymd_and_hms(2026, 1, 3, 10, 0, 0).unwrap();
        write_trade(&dir, t1, r#"{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-01T10:00:00Z"}"#);
        write_trade(&dir, t2, r#"{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-02T10:00:00Z"}"#);
        write_trade(&dir, t3, r#"{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-03T10:00:00Z"}"#);
        let f = TradeFilter {
            since: Some(t1),
            ..Default::default()
        };
        // Strict greater-than semantics: t1 itself is excluded.
        let out = scan_filtered_in_dir(&dir, f, 10).unwrap();
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|t| t.timestamp > t1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_filters_by_trade_type_and_user_uuid() {
        let dir = fixture_dir("type-uuid");
        let t1 = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        let t2 = Utc.with_ymd_and_hms(2026, 1, 2, 10, 0, 0).unwrap();
        let t3 = Utc.with_ymd_and_hms(2026, 1, 3, 10, 0, 0).unwrap();
        write_trade(&dir, t1, r#"{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"AAAA","timestamp":"2026-01-01T10:00:00Z"}"#);
        write_trade(&dir, t2, r#"{"trade_type":"Sell","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"BBBB","timestamp":"2026-01-02T10:00:00Z"}"#);
        write_trade(&dir, t3, r#"{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"BBBB","timestamp":"2026-01-03T10:00:00Z"}"#);
        let f = TradeFilter {
            trade_type: Some("Buy".to_string()),
            user_uuid: Some("bbbb".to_string()), // case-insensitive
            ..Default::default()
        };
        let out = scan_filtered_in_dir(&dir, f, 10).unwrap();
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
        let out = scan_filtered_in_dir(&dir, TradeFilter::default(), 2).unwrap();
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
        write_trade(&dir, t1, r#"{"trade_type":"Buy","item":"diamond","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-01T10:00:00Z"}"#);
        let f = TradeFilter {
            item: Some("minecraft:diamond".to_string()),
            ..Default::default()
        };
        let out = scan_filtered_in_dir(&dir, f, 10).unwrap();
        assert_eq!(out.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_skips_malformed_json() {
        let dir = fixture_dir("malformed");
        let t1 = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        let t2 = Utc.with_ymd_and_hms(2026, 1, 2, 10, 0, 0).unwrap();
        write_trade(&dir, t1, r#"{"trade_type":"Buy","item":"iron_ingot","amount":1,"amount_currency":2.0,"user_uuid":"u","timestamp":"2026-01-01T10:00:00Z"}"#);
        write_trade(&dir, t2, "not json");
        let out = scan_filtered_in_dir(&dir, TradeFilter::default(), 10).unwrap();
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
        let out = scan_filtered_in_dir(&dir, TradeFilter::default(), 10).unwrap();
        assert!(out.is_empty());
    }
}
