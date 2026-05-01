//! Per-model price table loaded from `data/chat/pricing.json`.
//!
//! Decoupling pricing from the binary means changes to Anthropic rates can
//! land via an operator config edit instead of a code release.
//! Defaults live here and are written to disk on first run if the file is
//! missing.
//!
//! Rates are USD per **million** tokens, matching how Anthropic publishes
//! them. `usd_for_tokens` accepts raw token counts and returns USD.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::fsutil::write_atomic;

pub const PRICING_FILE: &str = "data/chat/pricing.json";

/// Tracks model names that `usd_for_call` has already warned about, so the
/// "unknown model, billing $0.00" log fires exactly once per process per
/// model rather than spamming on every call.
static SEEN_UNKNOWN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

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
            "claude-sonnet-4-6".to_string(),
            ModelRates {
                input_per_million: 3.0,
                output_per_million: 15.0,
                cache_write_per_million: Some(3.75),
                cache_read_per_million: Some(0.3),
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
            Ok(t) if t.version == VERSION => {
                for (name, r) in &t.rates {
                    let cw = r.cache_write_per_million.unwrap_or(0.0);
                    let cr = r.cache_read_per_million.unwrap_or(0.0);
                    if !r.input_per_million.is_finite()
                        || r.input_per_million < 0.0
                        || !r.output_per_million.is_finite()
                        || r.output_per_million < 0.0
                        || !cw.is_finite()
                        || cw < 0.0
                        || !cr.is_finite()
                        || cr < 0.0
                    {
                        warn!(
                            path = %path.display(),
                            model = %name,
                            "pricing.json has invalid rates (non-finite or negative); using built-in defaults (file unchanged)"
                        );
                        return Ok(Self::default_table());
                    }
                }
                Ok(t)
            }
            Ok(t) if t.version > VERSION => {
                warn!(
                    path = %path.display(),
                    found = t.version,
                    expected = VERSION,
                    "pricing.json was written by a newer build; falling back to built-in defaults (file unchanged)"
                );
                Ok(Self::default_table())
            }
            Ok(t) => {
                warn!(
                    path = %path.display(),
                    found = t.version,
                    expected = VERSION,
                    "pricing.json version mismatch; using built-in defaults (file unchanged)"
                );
                Ok(Self::default_table())
            }
            Err(e) => {
                warn!(
                    path = %path.display(),
                    error = %e,
                    "pricing.json parse error; using built-in defaults (file unchanged)"
                );
                Ok(Self::default_table())
            }
        }
    }

    /// Compute USD cost for a given model + token spend. Returns 0.0 if
    /// the model is unknown — better to under-report cost than crash on
    /// a freshly-deployed model name; this function logs unknowns once
    /// at warn level (see [`usd_for_call`]).
    ///
    /// Cache tokens are NOT charged here. Use [`usd_for_call`] when the
    /// API response carries `cache_creation_input_tokens` /
    /// `cache_read_input_tokens` — that path bills cache writes at the
    /// premium rate and cache reads at the discount rate so the daily
    /// USD cap reflects what Anthropic will actually invoice.
    pub fn usd_for_tokens(
        &self,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> f64 {
        self.usd_for_call(model, input_tokens, output_tokens, 0, 0)
    }

    /// Like [`usd_for_tokens`] but also bills cache-creation and
    /// cache-read tokens. Anthropic reports them separately from
    /// `input_tokens`; ignoring them lets the meter UNDERESTIMATE real
    /// spend (cache writes cost 1.25-2× the base input rate) and the
    /// `daily_dollar_cap_usd` safety net would silently overrun.
    ///
    /// Falls back to the base input rate when a model's
    /// `cache_write_per_million` / `cache_read_per_million` is not
    /// declared — optimistic for cache writes (under-bills relative to
    /// the true ~1.25-2× rate), pessimistic for cache reads (over-bills
    /// relative to the true ~0.1× rate), but never panics on a
    /// freshly-deployed model.
    ///
    /// Unknown models bill as $0.00 and this function logs them once at
    /// warn level so a misconfigured composer model is visible to the
    /// operator instead of silently disabling the daily USD cap.
    pub fn usd_for_call(
        &self,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_input_tokens: u64,
        cache_read_input_tokens: u64,
    ) -> f64 {
        let Some(r) = self.rates.get(model) else {
            let seen = SEEN_UNKNOWN.get_or_init(|| Mutex::new(HashSet::new()));
            if let Ok(mut set) = seen.lock()
                && set.insert(model.to_string())
            {
                warn!(
                    model = %model,
                    "pricing: unknown model, billing as $0.00 — add it to data/chat/pricing.json to enforce the daily USD cap"
                );
            }
            return 0.0;
        };
        let cache_write_rate = r
            .cache_write_per_million
            .unwrap_or(r.input_per_million);
        let cache_read_rate = r
            .cache_read_per_million
            .unwrap_or(r.input_per_million);
        let input_usd = (input_tokens as f64) * r.input_per_million / 1_000_000.0;
        let output_usd = (output_tokens as f64) * r.output_per_million / 1_000_000.0;
        let cache_write_usd =
            (cache_creation_input_tokens as f64) * cache_write_rate / 1_000_000.0;
        let cache_read_usd =
            (cache_read_input_tokens as f64) * cache_read_rate / 1_000_000.0;
        let total = input_usd + output_usd + cache_write_usd + cache_read_usd;
        if total.is_finite() && total >= 0.0 { total } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_table_includes_opus_sonnet_and_haiku() {
        let t = PricingTable::default_table();
        assert_eq!(t.version, VERSION);
        assert!(t.rates.contains_key("claude-opus-4-7"));
        assert!(t.rates.contains_key("claude-sonnet-4-6"));
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
    fn usd_for_call_bills_cache_creation_at_premium_rate() {
        // Cache creation is more expensive than base input; verify
        // 1M cache-write tokens cost more than 1M plain input tokens
        // for Opus (defaults: input=15.0, cache_write=18.75).
        let t = PricingTable::default_table();
        let plain = t.usd_for_call("claude-opus-4-7", 1_000_000, 0, 0, 0);
        let with_cache_write = t.usd_for_call("claude-opus-4-7", 0, 0, 1_000_000, 0);
        assert!(
            with_cache_write > plain,
            "cache_write must cost more than base input ({with_cache_write} vs {plain})"
        );
    }

    #[test]
    fn usd_for_call_bills_cache_read_at_discount_rate() {
        // Cache reads are far cheaper than base input.
        let t = PricingTable::default_table();
        let plain = t.usd_for_call("claude-opus-4-7", 1_000_000, 0, 0, 0);
        let cache_read = t.usd_for_call("claude-opus-4-7", 0, 0, 0, 1_000_000);
        assert!(
            cache_read < plain,
            "cache_read must cost less than base input ({cache_read} vs {plain})"
        );
        assert!(cache_read > 0.0, "cache_read should still be billed");
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

    #[test]
    fn classifier_haiku_costs_less_than_composer_sonnet() {
        // The composer model swapped from Opus to Sonnet 4.6; Haiku must
        // remain cheaper than the live composer model so daily-cap math
        // and the classifier-as-cheap-prefilter assumption still hold.
        let t = PricingTable::default_table();
        let h = t.usd_for_tokens("claude-haiku-4-5-20251001", 1_000_000, 0);
        let s = t.usd_for_tokens("claude-sonnet-4-6", 1_000_000, 0);
        assert!(h < s, "Haiku must be cheaper than Sonnet per million input tokens");
    }

    #[test]
    fn sonnet_costs_less_than_opus() {
        // Pricing sanity: Sonnet 4.6 is the new default composer
        // precisely because it's cheaper than Opus 4.7 at comparable
        // capability for chat workloads.
        let t = PricingTable::default_table();
        let s = t.usd_for_call("claude-sonnet-4-6", 1_000_000, 200_000, 0, 0);
        let o = t.usd_for_call("claude-opus-4-7", 1_000_000, 200_000, 0, 0);
        assert!(s < o, "Sonnet must be cheaper than Opus on a typical I/O mix");
    }
}
