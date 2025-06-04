use std::{
    fs, io,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum TradeType {
    #[default] // Mark Buy as the default variant
    Buy,
    Sell,
    AddStock,
    RemoveStock,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Trade {
    pub trade_type: TradeType,
    pub item: String,
    pub amount: i32,
    pub amount_currency: f64,
    pub user_uuid: String, // maybe also name or even User
    pub timestamp: DateTime<Utc>,
}

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

    // Helper function to get the file path for a single trade
    fn get_trade_file_path(timestamp: &DateTime<Utc>) -> PathBuf {
        // Format timestamp as RFC3339 string and replace colons with dashes for filesystem compatibility
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
        fs::write(&path, json_str)?;
        Ok(())
    }

    /// Loads a single `Trade` from `data/trades/{timestamp}.json`.
    /// Returns an `io::Error` with `ErrorKind::NotFound` if the file does not exist.
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
    pub fn load_all() -> io::Result<Vec<Self>> {
        let dir_path = Path::new(Self::TRADES_DIR);
        let mut trades = Vec::new();

        if !dir_path.exists() {
            println!(
                "Trades directory not found at {}. Returning an empty Vec.",
                dir_path.display()
            );
            return Ok(Vec::new());
        }

        for entry in fs::read_dir(dir_path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() && path.extension().map_or(false, |ext| ext == "json") {
                match fs::read_to_string(&path) {
                    Ok(json_str) => match serde_json::from_str::<Self>(&json_str) {
                        Ok(trade) => {
                            trades.push(trade);
                        }
                        Err(e) => eprintln!(
                            "Warning: Could not deserialize trade from {}: {}",
                            path.display(),
                            e
                        ),
                    },
                    Err(e) => eprintln!("Warning: Could not read file {}: {}", path.display(), e),
                }
            }
        }

        // Sort trades by timestamp (oldest first)
        trades.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        Ok(trades)
    }

    /// Saves a Vec of `Trade`s, where each `Trade` is saved to its own file
    /// in the `data/trades/` directory using the `trade.save()` method.
    /// This method overwrites existing files and then removes any orphaned files.
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
                if path.is_file() && path.extension().map_or(false, |ext| ext == "json") {
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
