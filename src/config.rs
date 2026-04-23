//! Configuration loaded from `data/config.json`, auto-created with defaults
//! on first run and re-validated on every (hot-)reload.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;
use tracing::{info, warn};

use crate::types::Position;
use crate::fsutil::write_atomic;
use crate::constants::{FEE_MIN, FEE_MAX, TRADE_TIMEOUT_MS, PATHFINDING_TIMEOUT_MS};

/// Application configuration. See [`Config::validate`] for the invariants
/// each field must satisfy; missing `#[serde(default = ...)]` fields are
/// filled in from the `default_*` functions below so older configs still
/// load cleanly after new fields are added.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Storage origin position (where node 0 is located).
    pub position: Position,
    /// Trading fee rate applied as `price * (1 + fee)` on buy and
    /// `price * (1 - fee)` on sell. Must be in `[FEE_MIN, FEE_MAX]`.
    pub fee: f64,
    /// Microsoft account email for Azalea authentication. Empty is tolerated
    /// at load so a default config can be generated on first run, but
    /// authentication will later fail if the bot tries to connect.
    pub account_email: String,
    /// Minecraft server hostname or `host:port` (e.g., "corejourney.org").
    pub server_address: String,
    /// Optional buffer chest where the bot dumps inventory items when full.
    #[serde(default)]
    pub buffer_chest_position: Option<Position>,

    #[serde(default = "default_trade_timeout_ms")]
    pub trade_timeout_ms: u64,
    #[serde(default = "default_pathfinding_timeout_ms")]
    pub pathfinding_timeout_ms: u64,
    #[serde(default = "default_max_orders")]
    pub max_orders: usize,
    #[serde(default = "default_max_trades_in_memory")]
    pub max_trades_in_memory: usize,
    #[serde(default = "default_autosave_interval_secs")]
    pub autosave_interval_secs: u64,
}

// Timeout defaults defer to the canonical constants so the value lives in
// exactly one place; the `max_*` and `autosave_*` defaults have no
// corresponding constant and are hard-coded here.
fn default_trade_timeout_ms() -> u64 { TRADE_TIMEOUT_MS }
fn default_pathfinding_timeout_ms() -> u64 { PATHFINDING_TIMEOUT_MS }
fn default_max_orders() -> usize { 10_000 }
fn default_max_trades_in_memory() -> usize { 50_000 }
fn default_autosave_interval_secs() -> u64 { 2 }

impl Config {
    /// Validates every field and returns a single error message listing
    /// every problem found (not just the first), so an operator fixing a
    /// broken config sees all issues in one pass.
    pub fn validate(&self) -> Result<(), String> {
        let mut errors = Vec::new();

        if self.fee < FEE_MIN || self.fee > FEE_MAX {
            errors.push(format!(
                "fee must be between {} and {} (got {})",
                FEE_MIN, FEE_MAX, self.fee
            ));
        }
        if !self.fee.is_finite() {
            errors.push("fee must be a finite number".to_string());
        }

        // Empty email is a warning, not an error, so the default config
        // generated on first run loads cleanly; auth will fail later if
        // the operator tries to connect without filling it in. Routed
        // through `tracing::warn!` so hot-reloads under the config watcher
        // reach the log file, not just stderr.
        if self.account_email.trim().is_empty() {
            warn!("account_email is empty in config - bot will fail to authenticate");
        } else if !self.account_email.contains('@') {
            errors.push(format!(
                "account_email doesn't look like an email address: {}",
                self.account_email
            ));
        }
        
        // Accept a bare hostname / IPv4 or `host:port` using only characters
        // legal in a Minecraft server address (alnum, '.', '-', ':'). Rejects
        // whitespace, `scheme://`, and trailing paths — all common copy-paste
        // mistakes that would otherwise fail at connect time with a less
        // obvious error.
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
        } else if let Some((host, port)) = addr.rsplit_once(':') {
            // Without this host check, `":25565"` passes the outer is_empty
            // test but produces a bare-colon address every resolver rejects.
            if host.is_empty() {
                errors.push(format!(
                    "server_address host is empty: {}",
                    self.server_address
                ));
            }
            if port.parse::<u16>().is_err() {
                errors.push(format!(
                    "server_address port must be a number 0-65535: {}",
                    self.server_address
                ));
            }
        }
        
        // Vanilla world border maximum; values beyond it almost certainly
        // indicate a config typo rather than a legitimate location.
        const COORD_LIMIT: i32 = 30_000_000;
        if self.position.x.abs() > COORD_LIMIT || self.position.z.abs() > COORD_LIMIT {
            errors.push(format!(
                "position coordinates exceed Minecraft limits (|x|, |z| must be <= {}): ({}, {}, {})",
                COORD_LIMIT, self.position.x, self.position.y, self.position.z
            ));
        }
        // Y outside the modern vanilla build range is a warning (not an
        // error) because datapack/modded servers legitimately extend it.
        // Routed through `tracing::warn!` so a hot-reload warning lands in
        // the log file — the config watcher runs after the tracing
        // subscriber is installed, so stderr writes would be missed.
        if self.position.y < -64 || self.position.y > 320 {
            warn!(
                "position Y coordinate ({}) is outside typical range (-64 to 320)",
                self.position.y
            );
        }

        if let Some(ref buffer_pos) = self.buffer_chest_position
            && (buffer_pos.x.abs() > COORD_LIMIT || buffer_pos.z.abs() > COORD_LIMIT) {
                errors.push(format!(
                    "buffer_chest_position coordinates exceed limits: ({}, {}, {})",
                    buffer_pos.x, buffer_pos.y, buffer_pos.z
                ));
            }

        if self.trade_timeout_ms == 0 {
            errors.push("trade_timeout_ms must be greater than 0".to_string());
        }
        if self.pathfinding_timeout_ms == 0 {
            errors.push("pathfinding_timeout_ms must be greater than 0".to_string());
        }
        if self.autosave_interval_secs == 0 {
            errors.push("autosave_interval_secs must be greater than 0".to_string());
        }

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
    
    /// Loads configuration from `data/config.json`, creating it with
    /// defaults if missing, and validates the result.
    ///
    /// The auto-create-on-missing behavior is load-bearing: the config
    /// watcher in `main.rs` explicitly guards against a transient deletion
    /// triggering a silent default-overwrite by checking file existence
    /// before calling this — do not remove that guard without coordinating
    /// with the watcher.
    pub fn load() -> io::Result<Self> {
        let path = "data/config.json";
        let config_path = Path::new(path);

        let config = if config_path.exists() {
            let json_str = fs::read_to_string(config_path)?;
            match serde_json::from_str::<Config>(&json_str) {
                Ok(cfg) => cfg,
                Err(e) => {
                    warn!(path = %path, error = %e, "failed to parse config JSON");
                    return Err(e.into());
                }
            }
        } else {
            let default_config = Config {
                position: Position::default(),
                fee: 0.125, // matches the README example
                account_email: String::new(),
                server_address: String::from("corejourney.org"),
                buffer_chest_position: None,
                trade_timeout_ms: default_trade_timeout_ms(),
                pathfinding_timeout_ms: default_pathfinding_timeout_ms(),
                max_orders: default_max_orders(),
                max_trades_in_memory: default_max_trades_in_memory(),
                autosave_interval_secs: default_autosave_interval_secs(),
            };

            if let Some(parent_dir) = config_path.parent()
                && !parent_dir.exists() {
                    fs::create_dir_all(parent_dir)?;
                }

            let json_str = serde_json::to_string_pretty(&default_config)?;
            write_atomic(config_path, &json_str)?;

            info!(path = %path, "created default config file");

            default_config
        };

        if let Err(e) = config.validate() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, e));
        }

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> Config {
        Config {
            position: Position { x: 0, y: 64, z: 0 },
            fee: 0.125,
            account_email: "operator@example.com".to_string(),
            server_address: "corejourney.org".to_string(),
            buffer_chest_position: None,
            trade_timeout_ms: default_trade_timeout_ms(),
            pathfinding_timeout_ms: default_pathfinding_timeout_ms(),
            max_orders: default_max_orders(),
            max_trades_in_memory: default_max_trades_in_memory(),
            autosave_interval_secs: default_autosave_interval_secs(),
        }
    }

    #[test]
    fn default_timeout_fns_match_canonical_constants() {
        assert_eq!(default_trade_timeout_ms(), TRADE_TIMEOUT_MS);
        assert_eq!(default_pathfinding_timeout_ms(), PATHFINDING_TIMEOUT_MS);
    }

    #[test]
    fn default_limit_fns_return_documented_values() {
        assert_eq!(default_max_orders(), 10_000);
        assert_eq!(default_max_trades_in_memory(), 50_000);
        assert_eq!(default_autosave_interval_secs(), 2);
    }

    #[test]
    fn valid_config_passes_validation() {
        assert!(valid_config().validate().is_ok());
    }

    #[test]
    fn fee_at_lower_bound_is_accepted() {
        let mut c = valid_config();
        c.fee = FEE_MIN;
        assert!(c.validate().is_ok());
    }

    #[test]
    fn fee_at_upper_bound_is_accepted() {
        let mut c = valid_config();
        c.fee = FEE_MAX;
        assert!(c.validate().is_ok());
    }

    #[test]
    fn fee_below_minimum_is_rejected() {
        let mut c = valid_config();
        c.fee = -0.0001;
        let err = c.validate().unwrap_err();
        assert!(err.contains("fee"), "expected fee error, got: {err}");
    }

    #[test]
    fn fee_above_maximum_is_rejected() {
        let mut c = valid_config();
        c.fee = 1.0001;
        let err = c.validate().unwrap_err();
        assert!(err.contains("fee"), "expected fee error, got: {err}");
    }

    #[test]
    fn fee_nan_is_rejected_as_non_finite() {
        let mut c = valid_config();
        c.fee = f64::NAN;
        let err = c.validate().unwrap_err();
        assert!(err.contains("finite"), "expected finite error, got: {err}");
    }

    #[test]
    fn empty_account_email_is_tolerated() {
        // Load-bearing: default config has empty email and must validate.
        let mut c = valid_config();
        c.account_email = String::new();
        assert!(c.validate().is_ok());
    }

    #[test]
    fn account_email_without_at_sign_is_rejected() {
        let mut c = valid_config();
        c.account_email = "not-an-email".to_string();
        let err = c.validate().unwrap_err();
        assert!(err.contains("account_email"), "got: {err}");
    }

    #[test]
    fn empty_server_address_is_rejected() {
        let mut c = valid_config();
        c.server_address = String::new();
        let err = c.validate().unwrap_err();
        assert!(err.contains("server_address"), "got: {err}");
    }

    #[test]
    fn server_address_with_scheme_is_rejected() {
        let mut c = valid_config();
        c.server_address = "https://corejourney.org".to_string();
        let err = c.validate().unwrap_err();
        assert!(err.contains("scheme/path"), "got: {err}");
    }

    #[test]
    fn server_address_with_path_is_rejected() {
        let mut c = valid_config();
        c.server_address = "corejourney.org/play".to_string();
        let err = c.validate().unwrap_err();
        assert!(err.contains("scheme/path"), "got: {err}");
    }

    #[test]
    fn server_address_with_whitespace_is_rejected() {
        let mut c = valid_config();
        c.server_address = "core journey.org".to_string();
        let err = c.validate().unwrap_err();
        assert!(err.contains("whitespace"), "got: {err}");
    }

    #[test]
    fn server_address_with_host_port_is_accepted() {
        let mut c = valid_config();
        c.server_address = "corejourney.org:25565".to_string();
        assert!(c.validate().is_ok());
    }

    #[test]
    fn server_address_with_empty_host_before_port_is_rejected() {
        let mut c = valid_config();
        c.server_address = ":25565".to_string();
        let err = c.validate().unwrap_err();
        assert!(err.contains("host is empty"), "got: {err}");
    }

    #[test]
    fn server_address_with_non_numeric_port_is_rejected() {
        let mut c = valid_config();
        c.server_address = "corejourney.org:abcd".to_string();
        let err = c.validate().unwrap_err();
        assert!(err.contains("port"), "got: {err}");
    }

    #[test]
    fn server_address_with_underscore_is_rejected() {
        let mut c = valid_config();
        c.server_address = "core_journey.org".to_string();
        let err = c.validate().unwrap_err();
        assert!(err.contains("unsupported characters"), "got: {err}");
    }

    #[test]
    fn position_at_world_border_is_accepted() {
        let mut c = valid_config();
        c.position = Position { x: 30_000_000, y: 64, z: -30_000_000 };
        assert!(c.validate().is_ok());
    }

    #[test]
    fn position_one_beyond_world_border_is_rejected() {
        let mut c = valid_config();
        c.position = Position { x: 30_000_001, y: 64, z: 0 };
        let err = c.validate().unwrap_err();
        assert!(err.contains("position coordinates"), "got: {err}");
    }

    #[test]
    fn position_z_beyond_negative_world_border_is_rejected() {
        let mut c = valid_config();
        c.position = Position { x: 0, y: 64, z: -30_000_001 };
        let err = c.validate().unwrap_err();
        assert!(err.contains("position coordinates"), "got: {err}");
    }

    #[test]
    fn unusual_y_coordinate_warns_but_validates() {
        // Y outside -64..=320 is warn-only because modded servers extend it.
        let mut c = valid_config();
        c.position = Position { x: 0, y: 500, z: 0 };
        assert!(c.validate().is_ok());
        c.position = Position { x: 0, y: -200, z: 0 };
        assert!(c.validate().is_ok());
    }

    #[test]
    fn buffer_chest_beyond_world_border_is_rejected() {
        let mut c = valid_config();
        c.buffer_chest_position = Some(Position { x: 40_000_000, y: 64, z: 0 });
        let err = c.validate().unwrap_err();
        assert!(err.contains("buffer_chest_position"), "got: {err}");
    }

    #[test]
    fn buffer_chest_inside_world_border_is_accepted() {
        let mut c = valid_config();
        c.buffer_chest_position = Some(Position { x: 100, y: 70, z: -200 });
        assert!(c.validate().is_ok());
    }

    #[test]
    fn zero_trade_timeout_is_rejected() {
        let mut c = valid_config();
        c.trade_timeout_ms = 0;
        let err = c.validate().unwrap_err();
        assert!(err.contains("trade_timeout_ms"), "got: {err}");
    }

    #[test]
    fn zero_pathfinding_timeout_is_rejected() {
        let mut c = valid_config();
        c.pathfinding_timeout_ms = 0;
        let err = c.validate().unwrap_err();
        assert!(err.contains("pathfinding_timeout_ms"), "got: {err}");
    }

    #[test]
    fn zero_autosave_interval_is_rejected() {
        let mut c = valid_config();
        c.autosave_interval_secs = 0;
        let err = c.validate().unwrap_err();
        assert!(err.contains("autosave_interval_secs"), "got: {err}");
    }

    #[test]
    fn zero_max_orders_is_rejected() {
        let mut c = valid_config();
        c.max_orders = 0;
        let err = c.validate().unwrap_err();
        assert!(err.contains("max_orders"), "got: {err}");
    }

    #[test]
    fn zero_max_trades_in_memory_is_rejected() {
        let mut c = valid_config();
        c.max_trades_in_memory = 0;
        let err = c.validate().unwrap_err();
        assert!(err.contains("max_trades_in_memory"), "got: {err}");
    }

    #[test]
    fn multiple_violations_are_all_reported() {
        let mut c = valid_config();
        c.fee = 2.0;
        c.server_address = String::new();
        c.max_orders = 0;
        let err = c.validate().unwrap_err();
        assert!(err.contains("fee"), "got: {err}");
        assert!(err.contains("server_address"), "got: {err}");
        assert!(err.contains("max_orders"), "got: {err}");
    }

    #[test]
    fn serde_defaults_fill_missing_tuning_fields() {
        // Older configs predating the tuning fields must still deserialize.
        let json = r#"{
            "position": {"x": 0, "y": 64, "z": 0},
            "fee": 0.125,
            "account_email": "operator@example.com",
            "server_address": "corejourney.org"
        }"#;
        let cfg: Config = serde_json::from_str(json).expect("should deserialize");
        assert_eq!(cfg.trade_timeout_ms, default_trade_timeout_ms());
        assert_eq!(cfg.pathfinding_timeout_ms, default_pathfinding_timeout_ms());
        assert_eq!(cfg.max_orders, default_max_orders());
        assert_eq!(cfg.max_trades_in_memory, default_max_trades_in_memory());
        assert_eq!(cfg.autosave_interval_secs, default_autosave_interval_secs());
        assert!(cfg.buffer_chest_position.is_none());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        // `deny_unknown_fields` catches typos that would otherwise silently
        // fall back to defaults.
        let json = r#"{
            "position": {"x": 0, "y": 64, "z": 0},
            "fee": 0.125,
            "account_email": "operator@example.com",
            "server_address": "corejourney.org",
            "typoed_field": 123
        }"#;
        assert!(serde_json::from_str::<Config>(json).is_err());
    }
}
