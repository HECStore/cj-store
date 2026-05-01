//! Runtime state mirror and token meter — `data/chat/state.json`.
//!
//! State.json is the persistent runtime mirror: per-day token spend,
//! moderation backoff timers, last-replied-at per player, etc. It is
//! atomically rewritten via [`crate::fsutil::write_atomic`] on every
//! mutation. **Operator-editable when the chat task is stopped. NOT
//! LLM-writable through any tool.**
//!
//! See CHAT.md for the schema and for the token-meter semantics.
//!
//! ## Token-meter daily reset
//!
//! Lazy reset, in-flight attribution: every increment compares
//! `last_meter_day_utc` against today's UTC date and zeros counters
//! before adding new usage if they differ. Tokens count against the day
//! in which the call **started**, not finished — the started-day is
//! captured at call dispatch and used for attribution. This is monotonic-
//! clock-jump-safe (a backward jump won't reset) because we compare
//! calendar days, not durations.

use std::fs;
use std::io;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::fsutil::write_atomic;

pub const STATE_FILE: &str = "data/chat/state.json";

const STATE_VERSION: u32 = 1;

/// Token usage per phase. Counters are zeroed by [`ChatState::roll_to_today`]
/// when the calendar UTC day changes.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TokensToday {
    pub composer_input: u64,
    pub composer_output: u64,
    pub classifier_input: u64,
    pub classifier_output: u64,
    pub estimated_usd: f64,
}

/// Verdict from a single cap check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapVerdict {
    /// Below every cap; the call may proceed.
    Ok,
    /// USD cap would be exceeded.
    UsdCap,
    /// Composer per-day input or output cap would be exceeded.
    ComposerCap,
    /// Classifier per-day cap would be exceeded.
    ClassifierCap,
}

/// Persistent runtime state. Lives at `data/chat/state.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatState {
    pub version: u32,
    /// UTC date (YYYY-MM-DD) the meter was last rolled forward.
    pub last_meter_day_utc: String,
    pub tokens_today: TokensToday,
    /// Last-known bot username; used as a tentative self-echo filter for
    /// events that arrived during the pre-Init window.
    pub last_known_bot_username: Option<String>,
    /// Operator-set pause flag. The chat task observes this at the top of
    /// the decision pipeline and short-circuits when set.
    pub paused: bool,
    /// Runtime override of `chat.dry_run`. When true, replies compose
    /// but are never sent — independent of the static config flag.
    pub dry_run_runtime_override: bool,
    pub moderation_backoff_until: Option<String>,
    pub model_404_backoff_until: Option<String>,
    /// Set when the composer hits an Anthropic-side 429 / 5xx after the
    /// in-call retry budget is exhausted. Composer dispatch short-circuits
    /// while the timer is in the future so we don't keep slamming a
    /// throttled bucket with fresh requests. Cleared automatically once
    /// the timestamp is in the past.
    #[serde(default)]
    pub composer_throttle_backoff_until: Option<String>,
    pub persona_regen_cooldown_until: Option<String>,
    /// Number of history events dropped today by the publisher-side
    /// `try_send` path.
    pub history_drops_today: u64,
    /// Web-fetch calls made today; gate for `chat.web_fetch_daily_max`.
    /// Reset at the same daily boundary as the token meter.
    #[serde(default)]
    pub web_fetches_today: u32,
    /// CHAT.md — day (YYYY-MM-DD UTC) the retention sweep last fired.
    /// Used by the "first event each new UTC day" auto-trigger.
    #[serde(default)]
    pub last_sweep_day: Option<String>,
    /// CHAT.md — last reflection pass start time (ISO UTC). Used to
    /// enforce `reflection_min_interval_secs`.
    #[serde(default)]
    pub last_reflection_at: Option<String>,
    /// CHAT.md — bullets queued today by `update_self_memory`.
    /// Bounded by `chat.update_self_memory_max_per_day`.
    #[serde(default)]
    pub update_self_memory_today: u32,
    /// Snapshot of the last composer call — `Chat: status` displays
    /// this so operators can see the latest cost without grepping
    /// the decision log.
    #[serde(default)]
    pub last_composer_call: Option<LastCallSummary>,
    /// Last persona regeneration timestamp (ISO UTC). Surfaced in
    /// `Chat: status`.
    #[serde(default)]
    pub last_persona_regenerated_at: Option<String>,
    /// Cached copy of the last successfully-written serialized JSON.
    /// Used by [`ChatState::save`] to short-circuit redundant atomic
    /// rewrites when no field has actually changed since the last save.
    /// Never serialized to disk — the file is documented as
    /// operator-editable only when the chat task is stopped, so the
    /// cache stays in sync with disk for the lifetime of the process.
    #[serde(skip)]
    last_saved_json: Option<String>,
}

/// Snapshot of a single API call for `Chat: status`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LastCallSummary {
    pub at_utc: String,
    pub usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl Default for ChatState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            last_meter_day_utc: today_utc(),
            tokens_today: TokensToday::default(),
            last_known_bot_username: None,
            paused: false,
            dry_run_runtime_override: false,
            moderation_backoff_until: None,
            model_404_backoff_until: None,
            composer_throttle_backoff_until: None,
            persona_regen_cooldown_until: None,
            history_drops_today: 0,
            web_fetches_today: 0,
            last_sweep_day: None,
            last_reflection_at: None,
            update_self_memory_today: 0,
            last_composer_call: None,
            last_persona_regenerated_at: None,
            last_saved_json: None,
        }
    }
}

fn today_utc() -> String {
    Utc::now().format("%Y-%m-%d").to_string()
}

impl ChatState {
    /// Load `state.json`, falling back to the default on any error. The
    /// loader refuses to start on unknown versions and instead returns
    /// the default — operators see a warning in the log.
    ///
    /// Version-mismatch handling differs by direction:
    /// - **Future-version files** (`version > STATE_VERSION`, i.e. written
    ///   by a newer build) are renamed to `<original>.bak.v<found_version>`
    ///   before returning the default, so a subsequent save by this older
    ///   binary cannot silently clobber the future-format data. If the
    ///   rename fails the loader still returns the default and startup
    ///   proceeds (the next save will overwrite).
    /// - **Older-version files** and **unparseable files** are left in
    ///   place — those have a sensible upgrade story / are useful for
    ///   inspection.
    pub fn load_or_default() -> io::Result<Self> {
        let p = Path::new(STATE_FILE);
        if !p.exists() {
            return Ok(Self::default());
        }
        let s = fs::read_to_string(p)?;
        match serde_json::from_str::<ChatState>(&s) {
            Ok(state) if state.version == STATE_VERSION => Ok(state),
            Ok(other) if other.version > STATE_VERSION => {
                // Future-version file: move it aside so the older binary's
                // first save doesn't overwrite future-format data.
                let sidecar = format!("{}.bak.v{}", STATE_FILE, other.version);
                match fs::rename(p, &sidecar) {
                    Ok(()) => {
                        warn!(
                            on_disk_version = other.version,
                            expected = STATE_VERSION,
                            sidecar = %sidecar,
                            "state.json was written by a newer build; moved aside and starting fresh"
                        );
                    }
                    Err(e) => {
                        warn!(
                            on_disk_version = other.version,
                            expected = STATE_VERSION,
                            error = %e,
                            sidecar = %sidecar,
                            "state.json was written by a newer build; failed to move aside, starting fresh anyway (next save will overwrite)"
                        );
                    }
                }
                Ok(Self::default())
            }
            Ok(other) => {
                warn!(
                    on_disk_version = other.version,
                    expected = STATE_VERSION,
                    "state.json version mismatch; ignoring on-disk state and starting fresh"
                );
                Ok(Self::default())
            }
            Err(e) => {
                warn!(error = %e, "state.json unparseable; ignoring");
                Ok(Self::default())
            }
        }
    }

    pub fn save(&mut self) -> io::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        // Short-circuit if the freshly-serialized output is byte-identical
        // to the last successful write. Many event paths (system pseudo-
        // senders, blocklisted senders, paused-tick saves) flow through
        // `process_event` and call `save()` without mutating any field;
        // skipping the atomic rewrite avoids a redundant fsync per event
        // under AV/indexer pressure on Windows. Safe because state.json
        // is operator-editable only when the chat task is stopped.
        if self.last_saved_json.as_deref() == Some(json.as_str()) {
            return Ok(());
        }
        write_atomic(STATE_FILE, &json)?;
        self.last_saved_json = Some(json);
        Ok(())
    }

    /// Roll the per-day counters forward if today's UTC date is past
    /// `last_meter_day_utc`. Idempotent — calling on the same day is a
    /// no-op. Safe under monotonic clock jumps because we compare
    /// calendar days, not durations.
    pub fn roll_to_today(&mut self) {
        self.roll_to_day(&today_utc())
    }

    /// Test seam — `roll_to_today` calls this with `chrono::Utc::now()`.
    /// Forward-only: a call backdated to an earlier day (e.g. a composer
    /// call that started before midnight UTC and is now recorded after
    /// midnight, after some other call has already rolled the meter)
    /// must NOT wipe the current day's counters. The backdated usage is
    /// folded into the live day instead — slight misattribution, but no
    /// data loss.
    pub(crate) fn roll_to_day(&mut self, today: &str) {
        if today > self.last_meter_day_utc.as_str() {
            info!(
                from = %self.last_meter_day_utc,
                to = today,
                tokens_input = self.tokens_today.composer_input + self.tokens_today.classifier_input,
                tokens_output = self.tokens_today.composer_output + self.tokens_today.classifier_output,
                usd = self.tokens_today.estimated_usd,
                "rolling daily token meter forward"
            );
            self.tokens_today = TokensToday::default();
            self.history_drops_today = 0;
            self.web_fetches_today = 0;
            self.update_self_memory_today = 0;
            self.last_meter_day_utc = today.to_string();
        }
    }

    /// Check whether a composer call with the given input/output token
    /// estimate (and added USD cost) would exceed any cap.
    ///
    /// `estimated_usd_added` is the cost of THIS call only; the meter
    /// adds the historical day-total.
    pub fn would_exceed_caps_composer(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        estimated_usd_added: f64,
        config: &crate::config::ChatConfig,
    ) -> CapVerdict {
        // USD cap trips first if it's the more conservative one.
        let new_usd = self.tokens_today.estimated_usd + estimated_usd_added;
        if new_usd > config.daily_dollar_cap_usd {
            return CapVerdict::UsdCap;
        }
        let new_input = self.tokens_today.composer_input.saturating_add(input_tokens);
        let new_output = self
            .tokens_today
            .composer_output
            .saturating_add(output_tokens);
        if new_input > config.daily_input_token_cap
            || new_output > config.daily_output_token_cap
        {
            return CapVerdict::ComposerCap;
        }
        CapVerdict::Ok
    }

    pub fn would_exceed_caps_classifier(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        estimated_usd_added: f64,
        config: &crate::config::ChatConfig,
    ) -> CapVerdict {
        let new_usd = self.tokens_today.estimated_usd + estimated_usd_added;
        if new_usd > config.daily_dollar_cap_usd {
            return CapVerdict::UsdCap;
        }
        let new_total = self
            .tokens_today
            .classifier_input
            .saturating_add(input_tokens)
            .saturating_add(
                self.tokens_today
                    .classifier_output
                    .saturating_add(output_tokens),
            );
        // The classifier cap is on combined I+O tokens.
        if new_total > config.daily_classifier_token_cap {
            return CapVerdict::ClassifierCap;
        }
        CapVerdict::Ok
    }

    /// Record a composer call's actual usage. Rolls the meter to today
    /// FIRST (lazy reset, CHAT.md), then increments. Pass the day the
    /// call **started** as `started_day_utc` so usage is attributed
    /// correctly even if the day rolled over during the call.
    pub fn record_composer(
        &mut self,
        started_day_utc: &str,
        input_tokens: u64,
        output_tokens: u64,
        usd: f64,
    ) {
        self.roll_to_day(started_day_utc);
        self.tokens_today.composer_input =
            self.tokens_today.composer_input.saturating_add(input_tokens);
        self.tokens_today.composer_output =
            self.tokens_today.composer_output.saturating_add(output_tokens);
        if usd.is_finite() && usd >= 0.0 {
            self.tokens_today.estimated_usd += usd;
        }
    }

    pub fn record_classifier(
        &mut self,
        started_day_utc: &str,
        input_tokens: u64,
        output_tokens: u64,
        usd: f64,
    ) {
        self.roll_to_day(started_day_utc);
        self.tokens_today.classifier_input = self
            .tokens_today
            .classifier_input
            .saturating_add(input_tokens);
        self.tokens_today.classifier_output = self
            .tokens_today
            .classifier_output
            .saturating_add(output_tokens);
        if usd.is_finite() && usd >= 0.0 {
            self.tokens_today.estimated_usd += usd;
        }
    }
}

/// Snapshot of "today UTC" used at call dispatch time.
pub fn capture_today_utc() -> String {
    today_utc()
}

/// Pretty-print an absolute UTC instant for state.json fields.
///
/// Intentionally distinct from `chat::jsonl::iso_utc_millis`: this takes
/// `DateTime<Utc>` and emits second-precision (`SecondsFormat::Secs`) for
/// state.json, whereas the JSONL helper takes `SystemTime` and emits
/// millisecond precision. Do not unify without auditing every state.json
/// reader.
pub fn iso_utc(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ChatConfig;

    fn cfg_with_caps(input: u64, output: u64, usd: f64, classifier: u64) -> ChatConfig {
        ChatConfig {
            daily_input_token_cap: input,
            daily_output_token_cap: output,
            daily_dollar_cap_usd: usd,
            daily_classifier_token_cap: classifier,
            ..ChatConfig::default()
        }
    }

    #[test]
    fn fresh_state_uses_today_utc() {
        let s = ChatState::default();
        assert_eq!(s.last_meter_day_utc, today_utc());
        assert_eq!(s.tokens_today, TokensToday::default());
    }

    #[test]
    fn roll_to_day_zeros_counters_when_date_changes() {
        let mut s = ChatState::default();
        s.tokens_today.composer_input = 100;
        s.tokens_today.composer_output = 20;
        s.tokens_today.estimated_usd = 1.5;
        s.history_drops_today = 7;
        s.last_meter_day_utc = "2025-01-01".to_string();

        s.roll_to_day("2025-01-02");

        assert_eq!(s.tokens_today, TokensToday::default());
        assert_eq!(s.history_drops_today, 0);
        assert_eq!(s.last_meter_day_utc, "2025-01-02");
    }

    #[test]
    fn roll_to_day_is_noop_when_date_unchanged() {
        let mut s = ChatState::default();
        s.tokens_today.composer_input = 50;
        let day = s.last_meter_day_utc.clone();
        s.roll_to_day(&day);
        assert_eq!(s.tokens_today.composer_input, 50);
    }

    #[test]
    fn roll_to_day_does_not_reset_on_backward_jump() {
        // A backdated record (e.g. a composer call that started before
        // midnight UTC and is recorded after another call has already
        // rolled the meter) must NOT wipe the current day's counters.
        // The backdated usage folds into the live day instead.
        let mut s = ChatState::default();
        s.tokens_today.composer_input = 50;
        s.last_meter_day_utc = "2025-01-02".to_string();
        s.roll_to_day("2025-01-01");
        assert_eq!(s.tokens_today.composer_input, 50);
        assert_eq!(s.last_meter_day_utc, "2025-01-02");
    }

    #[test]
    fn record_composer_increments_and_accumulates_usd() {
        let mut s = ChatState::default();
        let day = s.last_meter_day_utc.clone();
        s.record_composer(&day, 1000, 200, 0.05);
        assert_eq!(s.tokens_today.composer_input, 1000);
        assert_eq!(s.tokens_today.composer_output, 200);
        assert!((s.tokens_today.estimated_usd - 0.05).abs() < 1e-9);

        s.record_composer(&day, 500, 100, 0.025);
        assert_eq!(s.tokens_today.composer_input, 1500);
        assert_eq!(s.tokens_today.composer_output, 300);
        assert!((s.tokens_today.estimated_usd - 0.075).abs() < 1e-9);
    }

    #[test]
    fn record_composer_ignores_nan_usd() {
        // A NaN propagating into estimated_usd would poison the daily
        // USD cap forever (NaN comparisons are unordered, so new_usd >
        // cap evaluates false). The recorder must silently drop it.
        let mut s = ChatState::default();
        let day = s.last_meter_day_utc.clone();
        s.record_composer(&day, 1000, 200, f64::NAN);
        assert_eq!(s.tokens_today.estimated_usd, 0.0);
        assert_eq!(s.tokens_today.composer_input, 1000);
        assert_eq!(s.tokens_today.composer_output, 200);
    }

    #[test]
    fn record_composer_attributes_to_started_day() {
        // CHAT.md: tokens count against the day the call STARTED, not
        // finished. If the dispatch day was today, recording today is
        // straightforward — counters increment. The forward-only roll
        // means a backdated record folds into the current day rather
        // than resetting the meter to yesterday and losing data.
        let mut s = ChatState::default();
        s.last_meter_day_utc = "2025-01-02".to_string();
        s.tokens_today.composer_input = 1000;
        // Call started yesterday; record now. Backdated → no roll, fold
        // into today's bucket so the 1000 already accumulated isn't lost.
        s.record_composer("2025-01-01", 500, 100, 0.05);
        assert_eq!(s.tokens_today.composer_input, 1500);
        assert_eq!(s.last_meter_day_utc, "2025-01-02");
    }

    // ---- cap checks -------------------------------------------------------

    #[test]
    fn cap_check_ok_when_below_every_cap() {
        let s = ChatState::default();
        let cfg = cfg_with_caps(1_000_000, 100_000, 5.0, 500_000);
        let v = s.would_exceed_caps_composer(1000, 100, 0.01, &cfg);
        assert_eq!(v, CapVerdict::Ok);
    }

    #[test]
    fn cap_check_trips_usd_first() {
        let mut s = ChatState::default();
        s.tokens_today.estimated_usd = 4.99;
        let cfg = cfg_with_caps(10_000_000, 1_000_000, 5.0, 5_000_000);
        let v = s.would_exceed_caps_composer(100, 10, 0.10, &cfg);
        assert_eq!(v, CapVerdict::UsdCap);
    }

    #[test]
    fn cap_check_trips_composer_input_cap() {
        let mut s = ChatState::default();
        s.tokens_today.composer_input = 999_500;
        let cfg = cfg_with_caps(1_000_000, 1_000_000, 1_000_000.0, 5_000_000);
        let v = s.would_exceed_caps_composer(1_000, 100, 0.0, &cfg);
        assert_eq!(v, CapVerdict::ComposerCap);
    }

    #[test]
    fn cap_check_trips_classifier_cap() {
        let mut s = ChatState::default();
        s.tokens_today.classifier_input = 400_000;
        s.tokens_today.classifier_output = 99_500;
        let cfg = cfg_with_caps(10_000_000, 1_000_000, 1_000_000.0, 500_000);
        // Adding 1000 input pushes total to 500_500 > 500_000 cap.
        let v = s.would_exceed_caps_classifier(1_000, 100, 0.0, &cfg);
        assert_eq!(v, CapVerdict::ClassifierCap);
    }

    // ---- serde round-trip -------------------------------------------------

    #[test]
    fn state_round_trips_through_json() {
        let mut s = ChatState::default();
        s.tokens_today.composer_input = 1_234;
        s.last_known_bot_username = Some("Alice".to_string());
        s.history_drops_today = 5;
        let json = serde_json::to_string_pretty(&s).unwrap();
        let back: ChatState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
}
