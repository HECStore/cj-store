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

/// Default maximum number of trades to keep in memory.
/// Trades older than this limit may be archived or removed.
/// This prevents unbounded memory growth on startup.
/// Can be overridden in config.json via the `max_trades_in_memory` field.
#[allow(dead_code)] // fallback constant for config loading
pub const DEFAULT_MAX_TRADES_IN_MEMORY: usize = 50_000;

/// Number of days to retain trade files on disk.
/// Trades older than this are candidates for archival/deletion.
#[allow(dead_code)] // retention window used by archive_old_trades
pub const TRADE_RETENTION_DAYS: i64 = 365;

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
    pub item: String,
    /// Quantity of items traded
    pub amount: i32,
    /// Currency amount involved (diamonds)
    pub amount_currency: f64,
    /// UUID of the user who executed the trade
    pub user_uuid: String,
    /// When the trade was executed
    pub timestamp: DateTime<Utc>,
}

#[allow(dead_code)] // persistence/archival API kept as cohesive surface
impl Trade {
    // Directory where all individual trade files will be stored
    const TRADES_DIR: &str = "data/trades";

    /// Helper method to create a new trade with current timestamp
    pub fn new(
        trade_type: TradeType,
        item: String,
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
    // The timestamp doubles as the filename, so two trades created in the
    // same nanosecond would collide — in practice this is fine since
    // `Utc::now()` is monotonic per process and trades are created serially.
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
        if let Some(parent_dir) = path.parent() {
            if !parent_dir.exists() {
                fs::create_dir_all(parent_dir)?;
            }
        }

        let json_str = serde_json::to_string_pretty(self)?;
        write_atomic(&path, &json_str)?;
        Ok(())
    }

    /// Loads a single `Trade` from `data/trades/{timestamp}.json`.
    /// Reserved for future tooling/debugging.
    pub fn load(timestamp: &DateTime<Utc>) -> io::Result<Self> {
        let path = Self::get_trade_file_path(timestamp);

        if path.exists() {
            let json_str = fs::read_to_string(&path)?;
            let trade: Self = serde_json::from_str(&json_str)?;
            Ok(trade)
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Trade file not found: {}", path.display()),
            ))
        }
    }

    /// Loads all `Trade`s by reading every JSON file in the `data/trades/` directory.
    /// Files that cannot be deserialized are skipped with a warning.
    /// If the directory does not exist, it returns an empty `Vec<Trade>`.
    /// Returns trades sorted by timestamp (oldest first).
    /// 
    /// **Memory limit**: Only loads the most recent DEFAULT_MAX_TRADES_IN_MEMORY trades.
    /// Older trades remain on disk but aren't loaded into memory.
    /// Use `load_all_with_limit` for a custom limit.
    pub fn load_all() -> io::Result<Vec<Self>> {
        Self::load_all_with_limit(DEFAULT_MAX_TRADES_IN_MEMORY)
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

        // Count files first for logging
        let file_count = fs::read_dir(dir_path)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            .count();

        for entry in fs::read_dir(dir_path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() && path.extension().is_some_and(|ext| ext == "json") {
                match fs::read_to_string(&path) {
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
        }

        // Sort trades by timestamp (oldest first)
        trades.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        
        // Limit to max_trades (keep most recent).
        // Since trades are sorted oldest-first, skipping the first N entries
        // discards the oldest and retains the most recent `max_trades`.
        if trades.len() > max_trades {
            let original_count = trades.len();
            trades = trades.into_iter()
                .skip(original_count - max_trades)
                .collect();
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
    
    /// Get count of trades currently in memory.
    pub fn count(trades: &[Self]) -> usize {
        trades.len()
    }
    
    /// Archive old trades to a separate file and remove individual trade files.
    /// This is useful for maintenance to reduce file count in the trades directory.
    /// 
    /// **Note**: This is a utility function for manual maintenance.
    /// It's not called automatically during normal operation.
    pub fn archive_old_trades(trades: &mut Vec<Self>, days_to_keep: i64) -> io::Result<usize> {
        let cutoff = Utc::now() - chrono::Duration::days(days_to_keep);
        let dir_path = Path::new(Self::TRADES_DIR);
        
        // Find trades older than cutoff
        let (old_trades, recent_trades): (Vec<_>, Vec<_>) = trades
            .drain(..)
            .partition(|t| t.timestamp < cutoff);
        
        // Put recent trades back
        *trades = recent_trades;
        
        let archived_count = old_trades.len();
        
        if archived_count > 0 {
            // Create archive file
            let archive_path = dir_path.join(format!("archive_{}.json", Utc::now().format("%Y%m%d_%H%M%S")));
            let json_str = serde_json::to_string_pretty(&old_trades)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            write_atomic(&archive_path, &json_str)?;
            
            // Remove individual old trade files
            for trade in &old_trades {
                let trade_path = Self::get_trade_file_path(&trade.timestamp);
                if trade_path.exists() {
                    let _ = fs::remove_file(&trade_path);
                }
            }
            
            tracing::info!(
                "Archived {} trades older than {} days to {}",
                archived_count,
                days_to_keep,
                archive_path.display()
            );
        }
        
        Ok(archived_count)
    }

    /// Saves a Vec of `Trade`s, where each `Trade` is saved to its own file
    /// in the `data/trades/` directory using the `trade.save()` method.
    /// This method overwrites existing files and then removes any orphaned files.
    ///
    /// The orphan cleanup ensures the on-disk set matches `trades` exactly,
    /// so callers can use this to synchronize after in-memory deletions.
    pub fn save_all(trades: &Vec<Self>) -> io::Result<()> {
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
                if path.is_file() && path.extension().is_some_and(|ext| ext == "json") {
                    if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                        if !expected_files.contains(filename) {
                            fs::remove_file(&path)?;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
