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
        let timestamp_str = timestamp.to_rfc3339().replace(':', "-");
        PathBuf::from(Self::TRADES_DIR).join(format!("{timestamp_str}.json"))
    }

    /// Saves this single `Trade` to `data/trades/{timestamp}.json`, creating
    /// the directory if needed.
    pub fn save(&self) -> io::Result<()> {
        let path = Self::get_trade_file_path(&self.timestamp);

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

        let mut skipped = 0usize;
        for path in paths_to_load {
            match fs::read_to_string(path) {
                Ok(json_str) => match serde_json::from_str::<Self>(&json_str) {
                    Ok(trade) => {
                        trades.push(trade);
                    }
                    Err(e) => {
                        skipped += 1;
                        tracing::warn!("[Trade] skipping malformed {}: {e}", path.display());
                    }
                },
                Err(e) => {
                    skipped += 1;
                    tracing::warn!("[Trade] skipping unreadable {}: {e}", path.display());
                }
            }
        }

        // Trades are already in chronological order due to sorted filenames,
        // but sort by timestamp field to be safe against any filename anomalies.
        trades.sort_by_key(|a| a.timestamp);

        tracing::info!(
            "[Trade] loaded {} of {} trades (limit {}, skipped {})",
            trades.len(),
            file_count,
            max_trades,
            skipped,
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
        // Refuse to proceed with an empty vec — writing zero expected files
        // would cause the orphan-cleanup loop below to delete every trade on disk.
        if trades.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "save_all called with an empty trades vec; refusing to wipe the trades directory",
            ));
        }

        let dir_path = Path::new(Self::TRADES_DIR);

        if !dir_path.exists() {
            fs::create_dir_all(dir_path)?;
        }

        let mut expected_files = std::collections::HashSet::new();

        for trade in trades {
            trade.save()?;
            let timestamp_str = trade.timestamp.to_rfc3339().replace(':', "-");
            let filename = format!("{timestamp_str}.json");
            expected_files.insert(filename);
        }

        let mut removed = 0usize;
        if dir_path.exists() {
            for entry in fs::read_dir(dir_path)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_file() && path.extension().is_some_and(|ext| ext == "json")
                    && let Some(filename) = path.file_name().and_then(|n| n.to_str())
                        && !expected_files.contains(filename) {
                            fs::remove_file(&path)?;
                            removed += 1;
                        }
            }
        }

        tracing::info!(
            "[Trade] save_all: wrote {} trades, cleaned {} orphan files",
            trades.len(),
            removed,
        );
        Ok(())
    }
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
        let err = Trade::save_all(&Vec::new()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("empty trades vec"));
    }

    #[test]
    fn get_trade_file_path_replaces_colons_for_windows() {
        let ts: DateTime<Utc> = "2024-01-02T03:04:05Z".parse().unwrap();
        let p = Trade::get_trade_file_path(&ts);
        let name = p.file_name().unwrap().to_string_lossy();
        assert!(!name.contains(':'), "file name should have no colons: {name}");
        assert!(name.ends_with(".json"));
    }
}
