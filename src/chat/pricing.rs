//! Per-model price table loaded from `data/chat/pricing.json`.
//!
//! Decoupling pricing from the binary means changes to Anthropic rates can
//! land via an operator config edit instead of a code release.
//! Defaults live here and are written to disk on first run if the file is
//! missing.
//!
//! Rates are USD per **million** tokens, matching how Anthropic publishes
//! them. `usd_for_tokens` accepts raw token counts and returns USD.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::fsutil::write_atomic;

pub const PRICING_FILE: &str = "data/chat/pricing.json";

/// Per-model rates in USD per million tokens. The classifier uses Haiku
/// rates; the composer uses Opus rates. Cache-write and cache-read are
/// optional; if missing we fall back to base input rate (less accurate
/// but never panics).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelRates {
    pub input_per_million: f64,
    pub output_per_million: f64,
    #[serde(default)]
    pub cache_write_per_million: Option<f64>,
    #[serde(default)]
    pub cache_read_per_million: Option<f64>,
}

/// Top-level shape of `pricing.json`. Keys are Anthropic model IDs (e.g.
/// `"claude-opus-4-7"`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PricingTable {
    pub version: u32,
    pub rates: HashMap<String, ModelRates>,
}

const VERSION: u32 = 1;

impl PricingTable {
    /// Default table shipped with the binary. Numbers are placeholder
    /// list prices that operators are expected to override; the file
    /// header explicitly invites that. Sourced as of late-2025 published
    /// rates; if these are wrong on day-one they are wrong by a small
    /// constant factor and the daily USD cap is the real
    /// safety net.
    pub fn default_table() -> Self {
        let mut rates = HashMap::new();
        rates.insert(
            "claude-opus-4-7".to_string(),
            ModelRates {
                input_per_million: 15.0,
                output_per_million: 75.0,
                cache_write_per_million: Some(18.75),
                cache_read_per_million: Some(1.5),
            },
        );
        rates.insert(
            "claude-haiku-4-5-20251001".to_string(),
            ModelRates {
                input_per_million: 1.0,
                output_per_million: 5.0,
                cache_write_per_million: Some(1.25),
                cache_read_per_million: Some(0.1),
            },
        );
        Self {
            version: VERSION,
            rates,
        }
    }

    /// Load the on-disk pricing table, creating the default if missing.
    /// Unparseable / wrong-version files are NOT silently overwritten —
    /// they are logged and the in-memory default is returned, leaving the
    /// disk file alone for the operator to fix.
    pub fn load_or_create() -> std::io::Result<Self> {
        let path = Path::new(PRICING_FILE);
        if !path.exists() {
            let table = Self::default_table();
            let json = serde_json::to_string_pretty(&table)?;
            write_atomic(path, &json)?;
            info!(path = %path.display(), "created default pricing.json");
            return Ok(table);
        }
        let s = fs::read_to_string(path)?;
        match serde_json::from_str::<PricingTable>(&s) {
            Ok(t) if t.version == VERSION => Ok(t),
            Ok(_) | Err(_) => {
                warn!(
                    path = %path.display(),
                    "pricing.json unparseable or wrong version; using built-in defaults (file unchanged)"
                );
                Ok(Self::default_table())
            }
        }
    }

    /// Compute USD cost for a given model + token spend. Returns 0.0 if
    /// the model is unknown — better to under-report cost than crash on
    /// a freshly-deployed model name; the daily-cap meter logs unknowns
    /// once at warn level.
    pub fn usd_for_tokens(
        &self,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> f64 {
        let Some(r) = self.rates.get(model) else {
            return 0.0;
        };
        let input_usd = (input_tokens as f64) * r.input_per_million / 1_000_000.0;
        let output_usd = (output_tokens as f64) * r.output_per_million / 1_000_000.0;
        input_usd + output_usd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_table_includes_opus_and_haiku() {
        let t = PricingTable::default_table();
        assert_eq!(t.version, VERSION);
        assert!(t.rates.contains_key("claude-opus-4-7"));
        assert!(t.rates.contains_key("claude-haiku-4-5-20251001"));
    }

    #[test]
    fn usd_for_tokens_scales_linearly_in_input() {
        let t = PricingTable::default_table();
        let one_m = t.usd_for_tokens("claude-opus-4-7", 1_000_000, 0);
        let two_m = t.usd_for_tokens("claude-opus-4-7", 2_000_000, 0);
        assert!((two_m - 2.0 * one_m).abs() < 1e-9);
    }

    #[test]
    fn usd_for_tokens_combines_input_and_output() {
        let t = PricingTable::default_table();
        let cost = t.usd_for_tokens("claude-opus-4-7", 1_000, 100);
        // Should equal sum of separate calls.
        let i = t.usd_for_tokens("claude-opus-4-7", 1_000, 0);
        let o = t.usd_for_tokens("claude-opus-4-7", 0, 100);
        assert!((cost - (i + o)).abs() < 1e-12);
    }

    #[test]
    fn usd_for_tokens_returns_zero_for_unknown_model() {
        let t = PricingTable::default_table();
        let cost = t.usd_for_tokens("not-a-real-model", 1_000_000, 1_000_000);
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn pricing_table_round_trips_through_json() {
        let t = PricingTable::default_table();
        let json = serde_json::to_string(&t).unwrap();
        let back: PricingTable = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn classifier_haiku_costs_less_than_composer_opus() {
        // Sanity check that the default-shipped rates are in the right
        // relative order — operators eyeballing the JSON can spot a typo
        // immediately because Haiku must be cheaper than Opus.
        let t = PricingTable::default_table();
        let h = t.usd_for_tokens("claude-haiku-4-5-20251001", 1_000_000, 0);
        let o = t.usd_for_tokens("claude-opus-4-7", 1_000_000, 0);
        assert!(h < o, "Haiku must be cheaper than Opus per million input tokens");
    }
}
