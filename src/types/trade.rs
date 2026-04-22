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

/// Represents a single executed trade with timestamp.
/// 
/// Each trade is persisted to its own file in `data/trades/{timestamp}.json`.
/// This allows for efficient appending and historical queries.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize, Clone)]
pub struct Trade {
    /// Type of trade (buy, sell, deposit, withdraw, etc.)
    pub trade_type: TradeType,
    /// Item involved in the trade
    pub item: crate::types::ItemId,
    /// Quantity of items traded
    pub amount: i32,
    /// Currency amount involved (diamonds)
    pub amount_currency: f64,
    /// UUID of the user who executed the trade
    pub user_uuid: String,
    /// When the trade was executed
    pub timestamp: DateTime<Utc>,
}

impl Trade {
    // Directory where all individual trade files will be stored
    const TRADES_DIR: &str = "data/trades";

    /// Helper method to create a new trade with current timestamp
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

    // Helper function to get the file path for a single trade.
    // The timestamp doubles as the filename, so two trades created at the
    // exact same nanosecond would collide — in practice this is fine since
    // trades are created serially and sub-nanosecond collisions are not expected.
    fn get_trade_file_path(timestamp: &DateTime<Utc>) -> PathBuf {
        // Colons are reserved on Windows (NTFS) filesystems, so RFC3339
        // timestamps must have them replaced before use as a filename.
        let timestamp_str = timestamp.to_rfc3339().replace(':', "-");
        PathBuf::from(Self::TRADES_DIR).join(format!("{}.json", timestamp_str))
    }

    /// Saves this single `Trade` instance to `data/trades/{timestamp}.json`.
    /// Creates the 'data/trades' directory if it doesn't exist.
    pub fn save(&self) -> io::Result<()> {
        let path = Self::get_trade_file_path(&self.timestamp);

        // Ensure the directory exists
        if let Some(parent_dir) = path.parent()
            && !parent_dir.exists() {
                fs::create_dir_all(parent_dir)?;
            }

        let json_str = serde_json::to_string_pretty(self)?;
        write_atomic(&path, &json_str)?;
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

        for path in paths_to_load {
            match fs::read_to_string(path) {
                Ok(json_str) => match serde_json::from_str::<Self>(&json_str) {
                    Ok(trade) => {
                        trades.push(trade);
                    }
                    Err(e) => tracing::warn!(
                        "Could not deserialize trade from {}: {}",
                        path.display(),
                        e
                    ),
                },
                Err(e) => tracing::warn!("Could not read trade file {}: {}", path.display(), e),
            }
        }

        // Trades are already in chronological order due to sorted filenames,
        // but sort by timestamp field to be safe against any filename anomalies.
        trades.sort_by_key(|a| a.timestamp);

        if file_count > max_trades {
            tracing::info!(
                "Loaded {} of {} trades into memory (limited to {})",
                trades.len(),
                file_count,
                max_trades
            );
        } else {
            tracing::info!("Loaded {} trades from disk", trades.len());
        }
        
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

        // Ensure the directory exists
        if !dir_path.exists() {
            fs::create_dir_all(dir_path)?;
        }

        // Keep track of files that should exist after saving
        let mut expected_files = std::collections::HashSet::new();

        // Save each trade individually using the individual trade.save() method
        for trade in trades {
            trade.save()?;
            let timestamp_str = trade.timestamp.to_rfc3339().replace(':', "-");
            let filename = format!("{}.json", timestamp_str);
            expected_files.insert(filename);
        }

        // Remove any files that shouldn't exist anymore
        if dir_path.exists() {
            for entry in fs::read_dir(dir_path)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_file() && path.extension().is_some_and(|ext| ext == "json")
                    && let Some(filename) = path.file_name().and_then(|n| n.to_str())
                        && !expected_files.contains(filename) {
                            fs::remove_file(&path)?;
                        }
            }
        }

        Ok(())
    }
}
