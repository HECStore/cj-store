//! # Configuration Management
//!
//! Loads and manages configuration from `data/config.json`.
//! Creates default config if file doesn't exist.
//! Validates configuration values on load.
//!
//! ## Configurable Timeouts and Limits
//! The following values can be customized in config.json:
//! - `trade_timeout_ms`: Timeout for trade operations (default: 45000ms)
//! - `pathfinding_timeout_ms`: Timeout for pathfinding (default: 60000ms)
//! - `max_orders`: Maximum orders to keep in memory (default: 10000)
//! - `max_trades_in_memory`: Maximum trades to load into memory (default: 50000)
//! - `autosave_interval_secs`: Minimum interval between autosaves (default: 2s)

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

use crate::types::Position;
use crate::fsutil::write_atomic;
use crate::constants::{FEE_MIN, FEE_MAX};

/// Application configuration loaded from `data/config.json`.
///
/// **Auto-creation**: If config file doesn't exist, creates default values.
///
/// **Validation**: On load, validates:
/// - `fee`: Must be between 0.0 and 1.0 (0% to 100%)
/// - `account_email`: Must not be empty (when connecting)
/// - `server_address`: Must not be empty
/// - `position`: Coordinates should be reasonable (within Minecraft limits)
/// - Timeout values: Must be positive
///
/// **Fields**:
/// - `position`: Storage origin (where node 0 is located)
/// - `fee`: Trading fee rate (e.g., 0.125 = 12.5%)
/// - `account_email`: Microsoft account for Azalea login
/// - `server_address`: Minecraft server hostname
/// - `buffer_chest_position`: Optional chest for bot to dump inventory items
/// - `trade_timeout_ms`: Timeout for trade operations (default: 45000ms)
/// - `pathfinding_timeout_ms`: Timeout for pathfinding (default: 60000ms)
/// - `max_orders`: Maximum orders in memory (default: 10000)
/// - `max_trades_in_memory`: Maximum trades to load (default: 50000)
/// - `autosave_interval_secs`: Autosave interval (default: 2s)
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Storage origin position (where node 0 is located)
    pub position: Position,
    /// Trading fee rate (applied to buy: `price * (1 + fee)`, sell: `price * (1 - fee)`)
    /// Must be between 0.0 (0%) and 1.0 (100%)
    pub fee: f64,
    /// Microsoft account email for Azalea authentication
    pub account_email: String,
    /// Minecraft server hostname (e.g., "corejourney.org")
    pub server_address: String,
    /// Optional buffer chest position (bot can dump inventory here if full)
    #[serde(default)]
    pub buffer_chest_position: Option<Position>,
    
    // === Configurable Timeouts and Limits ===
    
    /// Timeout for trade operations in milliseconds (default: 45000)
    #[serde(default = "default_trade_timeout_ms")]
    pub trade_timeout_ms: u64,
    /// Timeout for pathfinding operations in milliseconds (default: 60000)
    #[serde(default = "default_pathfinding_timeout_ms")]
    pub pathfinding_timeout_ms: u64,
    /// Maximum number of orders to keep in memory (default: 10000)
    #[serde(default = "default_max_orders")]
    pub max_orders: usize,
    /// Maximum number of trades to load into memory on startup (default: 50000)
    #[serde(default = "default_max_trades_in_memory")]
    pub max_trades_in_memory: usize,
    /// Minimum interval between autosaves in seconds (default: 2)
    #[serde(default = "default_autosave_interval_secs")]
    pub autosave_interval_secs: u64,
}

// Default value functions for serde
fn default_trade_timeout_ms() -> u64 { 45_000 }
fn default_pathfinding_timeout_ms() -> u64 { 60_000 }
fn default_max_orders() -> usize { 10_000 }
fn default_max_trades_in_memory() -> usize { 50_000 }
fn default_autosave_interval_secs() -> u64 { 2 }

impl Config {
    /// Validate configuration values.
    /// 
    /// # Returns
    /// * `Ok(())` if all values are valid
    /// * `Err(message)` describing what's invalid
    pub fn validate(&self) -> Result<(), String> {
        // Accumulate all errors rather than failing fast, so the user sees
        // every problem with their config in one run instead of fixing them
        // one at a time across multiple startup attempts.
        let mut errors = Vec::new();

        // Validate fee
        if self.fee < FEE_MIN || self.fee > FEE_MAX {
            errors.push(format!(
                "fee must be between {} and {} (got {})",
                FEE_MIN, FEE_MAX, self.fee
            ));
        }
        if !self.fee.is_finite() {
            errors.push("fee must be a finite number".to_string());
        }
        
        // Validate account_email (warn if empty, don't fail).
        // Empty email is tolerated here so operators can generate a default
        // config on first run and fill in credentials afterward without the
        // whole load failing. Authentication itself will surface the error
        // later if they try to actually connect.
        if self.account_email.trim().is_empty() {
            // This is a warning, not an error - bot may fail to connect
            eprintln!("Warning: account_email is empty in config - bot will fail to authenticate");
        } else if !self.account_email.contains('@') {
            errors.push(format!(
                "account_email doesn't look like an email address: {}",
                self.account_email
            ));
        }
        
        // Validate server_address. Accept either a bare hostname / IPv4, or
        // host:port, with only characters legal in a Minecraft server address
        // (letters, digits, '.', '-', ':'). Rejects whitespace, schemes like
        // "https://", and trailing paths — all common copy-paste mistakes.
        let addr = self.server_address.trim();
        if addr.is_empty() {
            errors.push("server_address cannot be empty".to_string());
        } else if addr.contains("://") || addr.contains('/') {
            errors.push(format!(
                "server_address must be a bare host or host:port (no scheme/path): {}",
                self.server_address
            ));
        } else if addr.chars().any(|c| c.is_whitespace()) {
            errors.push(format!(
                "server_address must not contain whitespace: {:?}",
                self.server_address
            ));
        } else if !addr.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == ':') {
            errors.push(format!(
                "server_address contains unsupported characters: {}",
                self.server_address
            ));
        } else if let Some((_, port)) = addr.rsplit_once(':') {
            // If a port was provided, make sure it's a valid u16.
            // `rsplit_once(':')` also matches bare IPv6-like inputs, but since
            // we reject ':' beyond the host:port form above (would fail the
            // charset check for IPv6 brackets), this is fine.
            if port.parse::<u16>().is_err() {
                errors.push(format!(
                    "server_address port must be a number 0-65535: {}",
                    self.server_address
                ));
            }
        }
        
        // Validate position (Minecraft coordinate limits: -30,000,000 to 30,000,000 for X/Z).
        // This is the vanilla world border maximum; values beyond it almost
        // certainly indicate a config typo rather than a legitimate location.
        const COORD_LIMIT: i32 = 30_000_000;
        if self.position.x.abs() > COORD_LIMIT || self.position.z.abs() > COORD_LIMIT {
            errors.push(format!(
                "position coordinates exceed Minecraft limits (|x|, |z| must be <= {}): ({}, {}, {})",
                COORD_LIMIT, self.position.x, self.position.y, self.position.z
            ));
        }
        // Y coordinate typically -64 to 320 in modern Minecraft
        if self.position.y < -64 || self.position.y > 320 {
            // Warning only - some servers have different limits
            eprintln!(
                "Warning: position Y coordinate ({}) is outside typical range (-64 to 320)",
                self.position.y
            );
        }
        
        // Validate buffer_chest_position if present
        if let Some(ref buffer_pos) = self.buffer_chest_position {
            if buffer_pos.x.abs() > COORD_LIMIT || buffer_pos.z.abs() > COORD_LIMIT {
                errors.push(format!(
                    "buffer_chest_position coordinates exceed limits: ({}, {}, {})",
                    buffer_pos.x, buffer_pos.y, buffer_pos.z
                ));
            }
        }
        
        // Validate timeout values
        if self.trade_timeout_ms == 0 {
            errors.push("trade_timeout_ms must be greater than 0".to_string());
        }
        if self.pathfinding_timeout_ms == 0 {
            errors.push("pathfinding_timeout_ms must be greater than 0".to_string());
        }
        if self.autosave_interval_secs == 0 {
            errors.push("autosave_interval_secs must be greater than 0".to_string());
        }
        
        // Validate limits
        if self.max_orders == 0 {
            errors.push("max_orders must be greater than 0".to_string());
        }
        if self.max_trades_in_memory == 0 {
            errors.push("max_trades_in_memory must be greater than 0".to_string());
        }
        
        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!("Config validation failed:\n  - {}", errors.join("\n  - ")))
        }
    }
    
    /// Loads configuration from `data/config.json`.
    /// If the file doesn't exist, it creates a default `config.json` file
    /// and then loads from it.
    /// 
    /// Validates configuration after loading and returns an error if invalid.
    pub fn load() -> io::Result<Self> {
        let path = "data/config.json";
        let config_path = Path::new(path);

        let config = if config_path.exists() {
            // File exists, proceed with loading.
            // Note: missing fields for timeouts/limits are filled in by
            // serde defaults (see `#[serde(default = ...)]`), so older
            // configs from before those fields existed still load cleanly.
            let json_str = fs::read_to_string(config_path)?;
            let config: Config = serde_json::from_str(&json_str)?;
            config
        } else {
            // File doesn't exist, create a default config and save it
            let default_config = Config {
                position: Position::default(),
                fee: 0.125, // 12.5% fee (matches README example)
                account_email: String::new(),
                server_address: String::from("corejourney.org"),
                buffer_chest_position: None,
                // Configurable timeouts and limits (use defaults)
                trade_timeout_ms: default_trade_timeout_ms(),
                pathfinding_timeout_ms: default_pathfinding_timeout_ms(),
                max_orders: default_max_orders(),
                max_trades_in_memory: default_max_trades_in_memory(),
                autosave_interval_secs: default_autosave_interval_secs(),
            };

            // Ensure the directory exists
            if let Some(parent_dir) = config_path.parent() {
                if !parent_dir.exists() {
                    fs::create_dir_all(parent_dir)?;
                }
            }

            // Serialize the default config to a pretty JSON string
            let json_str = serde_json::to_string_pretty(&default_config)?;

            // Write the default config to the file
            write_atomic(config_path, &json_str)?;

            println!("Created default config file at: {}", path);

            default_config
        };
        
        // Validate loaded config
        if let Err(e) = config.validate() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, e));
        }
        
        Ok(config)
    }
}
