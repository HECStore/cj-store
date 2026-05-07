//! Trade History Management
//!
//! Trades represent executed transactions with timestamps.
//! Each trade is stored as an individual file in `data/trades/`.
//! Old trades can be pruned to limit storage growth.
//!
//! The maximum number of trades loaded into memory can be configured in
//! `data/config.json` via the `max_trades_in_memory` field. The default is 50,000.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::fsutil::write_atomic;

/// The category of a recorded trade.
///
/// Customer-facing transactions (`Buy`, `Sell`) are distinguished from
/// administrative adjustments so that reports can exclude bookkeeping
/// entries from sales figures.
#[derive(Debug, PartialEq, Serialize, Deserialize, Default, Clone)]
pub enum TradeType {
    /// Customer bought an item from the store (store -> customer).
    #[default]
    Buy,
    /// Customer sold an item to the store (customer -> store).
    Sell,
    /// Admin adjustment: stock added without a customer transaction.
    AddStock,
    /// Admin adjustment: stock removed without a customer transaction.
    RemoveStock,
    /// User deposited currency into their store balance.
    DepositBalance,
    /// User withdrew currency from their store balance.
    WithdrawBalance,
    /// Admin adjustment: currency added to the store's treasury.
    AddCurrency,
    /// Admin adjustment: currency removed from the store's treasury.
    RemoveCurrency,
}

/// A single executed trade. Persisted one-file-per-trade in
/// `data/trades/{timestamp}.json` so a new trade is a single atomic write and
/// no existing history is rewritten on append.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize, Clone)]
pub struct Trade {
    pub trade_type: TradeType,
    pub item: crate::types::ItemId,
    pub amount: i32,
    /// Currency (diamonds) exchanged.
    pub amount_currency: f64,
    pub user_uuid: String,
    pub timestamp: DateTime<Utc>,
}

impl Trade {
    const TRADES_DIR: &str = "data/trades";

    /// Construct a trade stamped with the current wall-clock time. The timestamp
    /// is authoritative — it becomes the on-disk filename.
    pub fn new(
        trade_type: TradeType,
        item: crate::types::ItemId,
        amount: i32,
        amount_currency: f64,
        user_uuid: String,
    ) -> Self {
        Self {
            trade_type,
            item,
            amount,
            amount_currency,
            user_uuid,
            timestamp: Utc::now(),
        }
    }

    // Colons are reserved on Windows (NTFS), so RFC3339 timestamps must have
    // them replaced before use as a filename.
    fn get_trade_file_path(timestamp: &DateTime<Utc>) -> PathBuf {
        Self::get_trade_file_path_in_dir(Path::new(Self::TRADES_DIR), timestamp)
    }

    /// Directory-parameterized form of `get_trade_file_path`. Tests target
    /// this directly with a `tempfile::tempdir()` so they don't have to
    /// touch `data/trades/`.
    fn get_trade_file_path_in_dir(dir_path: &Path, timestamp: &DateTime<Utc>) -> PathBuf {
        let timestamp_str = timestamp.to_rfc3339().replace(':', "-");
        dir_path.join(format!("{timestamp_str}.json"))
    }

    /// Saves this single `Trade` to `data/trades/{timestamp}.json`, creating
    /// the directory if needed.
    ///
    /// Retained as a one-liner over `save_in_dir` for symmetry with the
    /// other `Type::save` methods on the storage types and as a future
    /// callsite if a single-trade write path is needed. Production code
    /// reaches the same logic through `save_all` → `save_all_in_dir` →
    /// `save_in_dir`, so this wrapper has no live callers — `#[allow(dead_code)]`
    /// keeps it available without a warning.
    #[allow(dead_code)]
    pub fn save(&self) -> io::Result<()> {
        self.save_in_dir(Path::new(Self::TRADES_DIR))
    }

    /// Directory-parameterized form of `save`. Tests target this directly
    /// with a `tempfile::tempdir()` to exercise the persistence path without
    /// touching `data/trades/`.
    fn save_in_dir(&self, dir_path: &Path) -> io::Result<()> {
        let path = Self::get_trade_file_path_in_dir(dir_path, &self.timestamp);

        if let Some(parent_dir) = path.parent()
            && !parent_dir.exists() {
                fs::create_dir_all(parent_dir)?;
            }

        let json_str = serde_json::to_string_pretty(self)?;
        write_atomic(&path, &json_str)?;
        tracing::debug!("[Trade] saved {} ({:?} {} x{})", self.timestamp, self.trade_type, self.item.as_str(), self.amount);
        Ok(())
    }

    /// Loads all `Trade`s with a custom memory limit.
    /// Files that cannot be deserialized are skipped with a warning.
    /// If the directory does not exist, it returns an empty `Vec<Trade>`.
    /// Returns trades sorted by timestamp (oldest first).
    /// 
    /// **Memory limit**: Only loads the most recent `max_trades` trades.
    /// Older trades remain on disk but aren't loaded into memory.
    pub fn load_all_with_limit(max_trades: usize) -> io::Result<Vec<Self>> {
        let dir_path = Path::new(Self::TRADES_DIR);
        let mut trades = Vec::new();

        if !dir_path.exists() {
            tracing::info!(
                "Trades directory not found at {}. Starting with empty trade history.",
                dir_path.display()
            );
            return Ok(Vec::new());
        }

        // Collect .json filenames and sort them lexicographically. Because
        // filenames are RFC3339 timestamps with colons replaced by dashes,
        // lexicographic order equals chronological order. Sorting before
        // slicing lets us take only the last `max_trades` filenames and
        // deserialize only those files, avoiding a full history read.
        let mut json_paths: Vec<PathBuf> = fs::read_dir(dir_path)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|ext| ext == "json"))
            .collect();
        let file_count = json_paths.len();

        json_paths.sort();

        // Keep only the last `max_trades` paths (the most recent ones).
        let paths_to_load = if json_paths.len() > max_trades {
            &json_paths[json_paths.len() - max_trades..]
        } else {
            &json_paths[..]
        };

        let mut quarantined = 0usize;
        let mut quarantine_failed = 0usize;
        for path in paths_to_load {
            match fs::read_to_string(path) {
                Ok(json_str) => match serde_json::from_str::<Self>(&json_str) {
                    Ok(trade) => {
                        trades.push(trade);
                    }
                    Err(e) => {
                        // Quarantine failure is non-fatal: a single rename
                        // error must not abort loading tens of thousands of
                        // history files at startup. The bad file stays in
                        // place and is simply skipped this cycle.
                        if let Err(qe) = quarantine_trade_file(path, &format!("malformed: {e}")) {
                            tracing::warn!(
                                "[Trade] quarantine failed for {}: {qe}",
                                path.display(),
                            );
                            quarantine_failed += 1;
                        } else {
                            quarantined += 1;
                        }
                    }
                },
                Err(e) => {
                    if let Err(qe) = quarantine_trade_file(path, &format!("unreadable: {e}")) {
                        tracing::warn!(
                            "[Trade] quarantine failed for {}: {qe}",
                            path.display(),
                        );
                        quarantine_failed += 1;
                    } else {
                        quarantined += 1;
                    }
                }
            }
        }

        // Trades are already in chronological order due to sorted filenames,
        // but sort by timestamp field to be safe against any filename anomalies.
        trades.sort_by_key(|a| a.timestamp);

        tracing::info!(
            "[Trade] loaded {} of {} trades (limit {}, quarantined {}, quarantine_failed {})",
            trades.len(),
            file_count,
            max_trades,
            quarantined,
            quarantine_failed,
        );

        Ok(trades)
    }
    
    /// Saves a Vec of `Trade`s, where each `Trade` is saved to its own file
    /// in the `data/trades/` directory using the `trade.save()` method.
    /// This method overwrites existing files and then removes any orphaned files.
    ///
    /// The orphan cleanup ensures the on-disk set matches `trades` exactly,
    /// so callers can use this to synchronize after in-memory deletions.
    pub fn save_all(trades: &Vec<Self>) -> io::Result<()> {
        Self::save_all_in_dir(trades, Path::new(Self::TRADES_DIR))
    }

    /// Directory-parameterized form of `save_all`. The empty-vec guard lives
    /// here (not just in the public wrapper) so tests can exercise the
    /// wipe-refusal invariant directly against a temp dir; the public
    /// `save_all` is a thin one-liner over this helper.
    fn save_all_in_dir(trades: &Vec<Self>, dir_path: &Path) -> io::Result<()> {
        // Refuse an empty vec only when there are real `.json` files on disk
        // that the orphan sweep below would actually wipe. A fresh install
        // (no trades dir, or an empty/stub trades dir) is a legitimate no-op:
        // the setup-phase autosave runs before the operator has committed the
        // first Buy/Sell/Deposit/Withdraw, and erroring here would block the
        // entire dirty-flag chain (`state::save` propagates via `?`, the
        // autosave loop never clears `self.dirty`, and a shutdown then loses
        // every staged mutation). Once any trade exists on disk, an empty
        // in-memory vec is still treated as "refuse to wipe".
        if trades.is_empty() {
            let dir_has_trade_files = match fs::read_dir(dir_path) {
                Ok(read_dir) => read_dir
                    .filter_map(|entry| entry.ok())
                    .any(|entry| {
                        let path = entry.path();
                        path.is_file() && path.extension().is_some_and(|ext| ext == "json")
                    }),
                Err(e) if e.kind() == io::ErrorKind::NotFound => false,
                Err(e) => return Err(e),
            };
            if dir_has_trade_files {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "save_all called with an empty trades vec but on-disk trade files exist; refusing to wipe the trades directory",
                ));
            }
            return Ok(());
        }

        if !dir_path.exists() {
            fs::create_dir_all(dir_path)?;
        }

        let mut expected_files = std::collections::HashSet::new();
        let mut written = 0usize;
        let mut first_save_err: Option<io::Error> = None;

        for trade in trades {
            // Always populate `expected_files` regardless of save outcome so
            // a transient write failure on one trade does not cause the
            // orphan sweep below to delete that trade's existing on-disk file.
            let timestamp_str = trade.timestamp.to_rfc3339().replace(':', "-");
            let filename = format!("{timestamp_str}.json");
            expected_files.insert(filename);
            // Attempt every trade even after a previous failure: each
            // `write_atomic` is independent, so one transient hiccup must
            // not silently drop later trades. Capture only the first error
            // to surface to the caller.
            if let Err(e) = trade.save_in_dir(dir_path) {
                tracing::warn!("[Trade] save failed for {}: {e}", trade.timestamp);
                if first_save_err.is_none() {
                    first_save_err = Some(e);
                }
            } else {
                written += 1;
            }
        }

        // Orphan sweep: warn-and-continue on per-entry IO errors so a single
        // locked/transient failure doesn't abort the whole sweep, and so a
        // captured `first_save_err` always wins over a sweep-only error
        // (stale orphans self-heal next cycle; a swallowed save error makes
        // callers think state was persisted when it wasn't).
        let mut removed = 0usize;
        let mut first_sweep_err: Option<io::Error> = None;
        if dir_path.exists() {
            match fs::read_dir(dir_path) {
                Ok(read_dir) => {
                    for entry in read_dir {
                        let entry = match entry {
                            Ok(e) => e,
                            Err(e) => {
                                tracing::warn!("[Trade] orphan sweep: unreadable entry: {e}");
                                if first_sweep_err.is_none() {
                                    first_sweep_err = Some(e);
                                }
                                continue;
                            }
                        };
                        let path = entry.path();
                        if path.is_file() && path.extension().is_some_and(|ext| ext == "json")
                            && let Some(filename) = path.file_name().and_then(|n| n.to_str())
                            && !expected_files.contains(filename)
                        {
                            if let Err(e) = fs::remove_file(&path) {
                                tracing::warn!(
                                    "[Trade] orphan sweep: remove_file({}) failed: {e}",
                                    path.display(),
                                );
                                if first_sweep_err.is_none() {
                                    first_sweep_err = Some(e);
                                }
                            } else {
                                removed += 1;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "[Trade] orphan sweep: read_dir({}) failed: {e}",
                        dir_path.display(),
                    );
                    first_sweep_err = Some(e);
                }
            }
        }

        tracing::info!(
            "[Trade] save_all: wrote {} of {} trades (failed {}), cleaned {} orphan files",
            written,
            trades.len(),
            trades.len() - written,
            removed,
        );
        match first_save_err.or(first_sweep_err) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// Rename a malformed/unreadable trade file to `*.json.corrupt.<millis>` so
/// the next `save_all` orphan-cleanup cannot delete it (extension is no
/// longer `.json`) and subsequent `load_all_with_limit` calls do not retry
/// deserializing it. Mirrors `quarantine_pair_file` in `types::pair`.
fn quarantine_trade_file(path: &Path, reason: &str) -> io::Result<()> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let target = path.with_extension(format!("json.corrupt.{ts}"));
    tracing::warn!(
        "[Trade] quarantining {} ({}): renaming to {}",
        path.display(),
        reason,
        target.display(),
    );
    fs::rename(path, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ItemId;

    #[test]
    fn new_captures_current_timestamp() {
        let before = Utc::now();
        let t = Trade::new(
            TradeType::Buy,
            ItemId::new("diamond").unwrap(),
            1,
            2.5,
            "u".to_string(),
        );
        let after = Utc::now();
        assert!(t.timestamp >= before && t.timestamp <= after);
        assert_eq!(t.trade_type, TradeType::Buy);
    }

    #[test]
    fn trade_type_default_is_buy() {
        assert_eq!(TradeType::default(), TradeType::Buy);
    }

    #[test]
    fn trade_type_serde_round_trip_preserves_variant() {
        for variant in [
            TradeType::Buy,
            TradeType::Sell,
            TradeType::AddStock,
            TradeType::RemoveStock,
            TradeType::DepositBalance,
            TradeType::WithdrawBalance,
            TradeType::AddCurrency,
            TradeType::RemoveCurrency,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let back: TradeType = serde_json::from_str(&json).unwrap();
            assert_eq!(back, variant);
        }
    }

    #[test]
    fn save_all_refuses_empty_vec_to_prevent_accidental_wipe() {
        // Pre-populate a `*.json` file; an empty `trades` vec paired with
        // on-disk trade files must NOT trigger the orphan sweep that would
        // wipe them.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("2026-01-01T00-00-00Z.json");
        fs::write(&f, "{}").unwrap();

        let err = Trade::save_all_in_dir(&Vec::new(), dir.path())
            .expect_err("empty vec paired with on-disk trade file must error");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("empty trades vec"));
        assert!(f.exists(), "pre-existing trade file must survive");
    }

    #[test]
    fn save_all_with_empty_vec_and_empty_dir_is_noop() {
        // Fresh install: no trades dir / empty trades dir + empty in-memory
        // vec must be a no-op `Ok(())`, not an `InvalidInput` error. Erring
        // here would block the setup-phase autosave (the dirty flag never
        // clears, and a shutdown drops every staged mutation).
        let parent = tempfile::tempdir().unwrap();

        // (i) Missing directory: an empty vec must succeed without creating
        //     the directory (the no-op path returns before `create_dir_all`).
        let missing = parent.path().join("does_not_exist");
        Trade::save_all_in_dir(&Vec::new(), &missing)
            .expect("empty vec + missing dir must be a no-op");
        assert!(!missing.exists(), "no-op must not create the dir");

        // (ii) Existing but empty directory: also a no-op.
        let empty = parent.path().join("empty_trades");
        fs::create_dir_all(&empty).unwrap();
        Trade::save_all_in_dir(&Vec::new(), &empty)
            .expect("empty vec + empty dir must be a no-op");

        // (iii) Existing dir with only non-`.json` siblings: still a no-op
        //       (the guard only fires on real `.json` files the sweep would wipe).
        let with_sibling = parent.path().join("with_sibling");
        fs::create_dir_all(&with_sibling).unwrap();
        fs::write(with_sibling.join("README.txt"), "not a trade file").unwrap();
        Trade::save_all_in_dir(&Vec::new(), &with_sibling)
            .expect("empty vec + non-json siblings must be a no-op");
        assert!(with_sibling.join("README.txt").exists(), "non-json sibling must survive");
    }

    #[test]
    fn get_trade_file_path_replaces_colons_for_windows() {
        let ts: DateTime<Utc> = "2024-01-02T03:04:05Z".parse().unwrap();
        let p = Trade::get_trade_file_path(&ts);
        let name = p.file_name().unwrap().to_string_lossy();
        assert!(!name.contains(':'), "file name should have no colons: {name}");
        assert!(name.ends_with(".json"));
    }

    #[test]
    fn save_all_in_dir_writes_to_threaded_dir_and_sweeps_orphans_only_there() {
        // End-to-end check that `save_all_in_dir` honors `dir_path` for both
        // the per-trade write loop and the orphan sweep — neither touches the
        // real `data/trades/` directory. Without `save_in_dir` and the
        // dir-parameterized orphan sweep, this would fail (writes would land
        // under `data/trades/` and a stale file in `data/trades/` could be
        // removed by the sweep).
        let dir = tempfile::tempdir().unwrap();

        // Pre-seed a stale `.json` orphan inside the temp dir; it must be
        // swept after `save_all_in_dir` runs.
        let stale_in_temp = dir.path().join("2020-01-01T00-00-00Z.json");
        fs::write(&stale_in_temp, "{}").unwrap();

        // Pre-seed a `.json` file inside the real `data/trades/` (creating
        // the dir if necessary) that must NOT be touched by the temp-dir
        // sweep. Use a uniquely shaped filename so a real trade write to
        // `data/trades/` cannot collide with it.
        let real_dir = Path::new(Trade::TRADES_DIR);
        if !real_dir.exists() {
            fs::create_dir_all(real_dir).unwrap();
        }
        let real_canary_name = format!(
            "9999-99-99T00-00-00Z-canary-{}.json",
            std::process::id(),
        );
        let real_canary = real_dir.join(&real_canary_name);
        fs::write(&real_canary, "{}").unwrap();

        // Build a non-empty trades vec.
        let trade = Trade::new(
            TradeType::Buy,
            ItemId::new("diamond").unwrap(),
            1,
            2.5,
            "u".to_string(),
        );
        let trades = vec![trade.clone()];

        let result = Trade::save_all_in_dir(&trades, dir.path());

        // Always clean up the canary first to keep the real data dir tidy
        // even if the assertions below fail.
        let canary_still_there = real_canary.exists();
        let _ = fs::remove_file(&real_canary);

        result.expect("save_all_in_dir must succeed");

        // (i) The trade file actually lands in `dir`, not in `data/trades/`.
        let expected_in_temp =
            Trade::get_trade_file_path_in_dir(dir.path(), &trade.timestamp);
        assert!(
            expected_in_temp.exists(),
            "trade file must be written to temp dir: {}",
            expected_in_temp.display(),
        );
        let unexpected_in_real = Trade::get_trade_file_path(&trade.timestamp);
        assert!(
            !unexpected_in_real.exists(),
            "trade file must NOT have been written to data/trades/: {}",
            unexpected_in_real.display(),
        );

        // (ii) The temp-dir orphan sweep removed the pre-seeded stale file.
        assert!(
            !stale_in_temp.exists(),
            "stale orphan in temp dir must be swept",
        );

        // (iii) The canary in `data/trades/` survived (i.e., the orphan sweep
        //       did not iterate the real data dir).
        assert!(
            canary_still_there,
            "real data/trades/ canary must survive temp-dir save_all_in_dir",
        );
    }
}
