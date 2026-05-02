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

    /// Chat AI module configuration. Defaults disable the module entirely so
    /// existing operators are unaffected; see [`ChatConfig`] for the full
    /// schema. The full plan documented in CHAT.md lists every knob;
    /// this skeleton only includes the fields needed for the wiring phase.
    #[serde(default)]
    pub chat: ChatConfig,
}

/// Chat module configuration. Disabled by default. See CHAT.md for
/// the full design and field-by-field rationale; every knob defaults to
/// the value documented in the plan.
///
/// Adding a field here requires updating only this struct: every
/// constructor in tests reads from `ChatConfig::default()`, and on-disk
/// configs use serde defaults so older `data/config.json` files keep
/// loading after a field is added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatConfig {
    #[serde(default = "default_chat_enabled")]
    pub enabled: bool,
    #[serde(default = "default_chat_dry_run")]
    pub dry_run: bool,
    #[serde(default = "default_chat_api_key_env")]
    pub api_key_env: String,
    #[serde(default = "default_chat_composer_model")]
    pub composer_model: String,
    #[serde(default = "default_chat_classifier_model")]
    pub classifier_model: String,
    /// Sampling temperature for the composer call. Anthropic accepts
    /// `0.0..=1.0`; values outside that range are clamped at request
    /// time. `None` (omitted in JSON) sends no temperature field, so
    /// the API uses the model's own default (1.0 for current Sonnet
    /// / Opus). Default is `0.8` — slightly tighter than 1.0 keeps the
    /// persona voice consistent across replies without flattening
    /// variation.
    #[serde(default = "default_chat_composer_temperature")]
    pub composer_temperature: Option<f32>,
    /// Sampling temperature for the classifier. Default `0.0` because
    /// the classifier must emit a single JSON object — determinism
    /// matters more than variety. Override with `null` to fall back to
    /// the API default if a future classifier model needs spread.
    #[serde(default = "default_chat_classifier_temperature")]
    pub classifier_temperature: Option<f32>,
    #[serde(default)]
    pub persona_seed: String,

    #[serde(default = "default_chat_command_prefixes")]
    pub command_prefixes: Vec<String>,
    #[serde(default = "default_chat_command_typo_max_distance")]
    pub command_typo_max_distance: u32,

    // Caps
    #[serde(default = "default_chat_daily_input_token_cap")]
    pub daily_input_token_cap: u64,
    #[serde(default = "default_chat_daily_output_token_cap")]
    pub daily_output_token_cap: u64,
    #[serde(default = "default_chat_daily_classifier_token_cap")]
    pub daily_classifier_token_cap: u64,
    #[serde(default = "default_chat_daily_dollar_cap_usd")]
    pub daily_dollar_cap_usd: f64,
    #[serde(default)]
    pub acknowledge_high_spend: bool,

    // Classifier gating
    #[serde(default = "default_chat_recent_speaker_secs")]
    pub recent_speaker_secs: u32,
    #[serde(default = "default_chat_classifier_sample_rate")]
    pub classifier_sample_rate: f32,
    #[serde(default = "default_chat_classifier_per_sender_per_minute")]
    pub classifier_per_sender_per_minute: u32,
    #[serde(default = "default_chat_classifier_min_confidence")]
    pub classifier_min_confidence: f32,
    #[serde(default = "default_chat_classifier_context_messages")]
    pub classifier_context_messages: u32,

    /// `(start_hour_utc, end_hour_utc)`; both in `[0, 24)`. Wrap-around
    /// when `start > end` (overnight). `start == end` is treated as
    /// "always on" (same as `None`).
    #[serde(default)]
    pub active_hours_utc: Option<(u32, u32)>,

    // Pacing
    #[serde(default = "default_chat_min_silence_secs")]
    pub min_silence_secs: u32,
    #[serde(default = "default_chat_max_replies_per_minute")]
    pub max_replies_per_minute: u32,
    #[serde(default = "default_chat_typing_delay_base_ms")]
    pub typing_delay_base_ms: u32,
    #[serde(default = "default_chat_typing_delay_per_char_ms")]
    pub typing_delay_per_char_ms: u32,
    #[serde(default = "default_chat_typing_delay_jitter_ms")]
    pub typing_delay_jitter_ms: u32,
    #[serde(default = "default_chat_typing_delay_floor_ms")]
    pub typing_delay_floor_ms: u32,
    #[serde(default = "default_chat_typing_delay_max_ms")]
    pub typing_delay_max_ms: u32,

    // Memory growth
    #[serde(default = "default_chat_adjustments_max_bullets")]
    pub adjustments_max_bullets: u32,
    #[serde(default = "default_chat_player_memory_max_bytes")]
    pub player_memory_max_bytes: u32,
    #[serde(default = "default_chat_update_bullet_max_chars")]
    pub update_bullet_max_chars: u32,
    #[serde(default = "default_chat_update_self_memory_max_per_day")]
    pub update_self_memory_max_per_day: u32,
    #[serde(default = "default_chat_memory_max_inferred_bullets")]
    pub memory_max_inferred_bullets: u32,

    // Rate limiter
    #[serde(default = "default_chat_composer_rpm_max")]
    pub composer_rpm_max: u32,
    #[serde(default = "default_chat_classifier_rpm_max")]
    pub classifier_rpm_max: u32,
    #[serde(default = "default_chat_composer_itpm_max")]
    pub composer_itpm_max: u32,
    #[serde(default = "default_chat_classifier_itpm_max")]
    pub classifier_itpm_max: u32,
    #[serde(default = "default_chat_rate_limit_wait_max_secs")]
    pub rate_limit_wait_max_secs: u32,

    // web_fetch / web_search
    #[serde(default = "default_chat_web_fetch_max_bytes")]
    pub web_fetch_max_bytes: u32,
    #[serde(default = "default_chat_web_fetch_daily_max")]
    pub web_fetch_daily_max: u32,
    #[serde(default)]
    pub web_fetch_enabled: bool,
    #[serde(default = "default_chat_web_search_enabled")]
    pub web_search_enabled: bool,

    // Cross-player firewall
    #[serde(default)]
    pub cross_player_reads: bool,

    // Store-read tools (query_trades / get_pair / get_user_balance).
    // Disabled by default — operators opt in. Cross-player balance
    // lookups get their own switch because balance is more sensitive
    // than the per-player memory bullets `cross_player_reads` covers.
    #[serde(default)]
    pub tools_store_enabled: bool,
    #[serde(default = "default_chat_tools_store_max_calls_per_turn")]
    pub tools_store_max_calls_per_turn: u32,
    #[serde(default = "default_chat_tools_store_trade_query_max_results")]
    pub tools_store_trade_query_max_results: u32,
    #[serde(default)]
    pub tools_store_cross_player_balance_lookups: bool,

    // Reflection
    #[serde(default = "default_chat_reflection_max_pending")]
    pub reflection_max_pending: u32,
    #[serde(default = "default_chat_reflection_idle_trigger_secs")]
    pub reflection_idle_trigger_secs: u32,
    #[serde(default = "default_chat_reflection_min_interval_secs")]
    pub reflection_min_interval_secs: u32,
    #[serde(default = "default_chat_reflection_min_distinct_senders")]
    pub reflection_min_distinct_senders: u32,
    #[serde(default = "default_chat_reflection_min_distinct_triggers")]
    pub reflection_min_distinct_triggers: u32,

    // History / JSONL caps
    #[serde(default = "default_chat_tools_history_max_bytes")]
    pub tools_history_max_bytes: u32,
    #[serde(default = "default_chat_history_max_line_bytes")]
    pub history_max_line_bytes: u32,

    // Trust-3 / archives
    #[serde(default = "default_chat_trust3_max_days")]
    pub trust3_max_days: u32,
    #[serde(default = "default_chat_persona_archive_max")]
    pub persona_archive_max: u32,
    #[serde(default = "default_chat_archive_max_bytes")]
    pub archive_max_bytes: u32,

    // UUID resolution
    #[serde(default = "default_chat_uuid_resolve_queue_max")]
    pub uuid_resolve_queue_max: u32,

    // Composer context windows
    #[serde(default = "default_chat_composer_context_messages")]
    pub composer_context_messages: u32,
    #[serde(default = "default_chat_composer_max_tool_iterations")]
    pub composer_max_tool_iterations: u32,
    #[serde(default = "default_chat_composer_max_chars")]
    pub composer_max_chars: u32,

    // Spam
    #[serde(default = "default_chat_spam_msgs_per_window")]
    pub spam_msgs_per_window: u32,
    #[serde(default = "default_chat_spam_window_secs")]
    pub spam_window_secs: u32,
    #[serde(default = "default_chat_spam_cooldown_secs")]
    pub spam_cooldown_secs: u32,

    // History / tools
    #[serde(default = "default_chat_history_search_max_days")]
    pub history_search_max_days: u32,
    #[serde(default = "default_chat_history_retention_days")]
    pub history_retention_days: u32,
    #[serde(default = "default_chat_decisions_retention_days")]
    pub decisions_retention_days: u32,
    #[serde(default = "default_chat_hash_uuids_in_decisions")]
    pub hash_uuids_in_decisions: bool,

    // Backoffs
    #[serde(default = "default_chat_moderation_backoff_secs")]
    pub moderation_backoff_secs: u32,
    #[serde(default = "default_chat_persona_regen_cooldown_secs")]
    pub persona_regen_cooldown_secs: u32,
    /// Seconds the chat task pauses composer dispatch after Anthropic
    /// returns a 429/5xx that blew through the in-call retry budget.
    /// Zero disables the cooldown (every event re-races the throttled
    /// bucket); the default lets the upstream window reset before we try
    /// again.
    #[serde(default = "default_chat_composer_throttle_backoff_secs")]
    pub composer_throttle_backoff_secs: u32,

    // Proactive thread continuation
    /// When `true`, the chat task fires a periodic tick that may
    /// initiate a composer turn even if no new chat event arrived —
    /// letting the bot keep an active conversation alive (drive a
    /// thread, ask a follow-up, change subject) when the partner has
    /// gone quiet for a stretch. Disabled by default; opt-in.
    #[serde(default)]
    pub proactive_threading_enabled: bool,
    /// Seconds between proactive-tick checks. Each check evaluates all
    /// gates and rolls the probability — most checks decide to do
    /// nothing. Default 30 s.
    #[serde(default = "default_chat_proactive_tick_secs")]
    pub proactive_tick_secs: u32,
    /// A proactive tick will not fire unless the bot has been silent
    /// for at least this many seconds. Prevents the bot from speaking
    /// twice in quick succession.
    #[serde(default = "default_chat_proactive_min_secs_since_bot")]
    pub proactive_min_secs_since_bot: u32,
    /// A proactive tick will not fire until this many seconds have
    /// passed since the conversation partner's last message. Lets a
    /// real reply land first if one is coming.
    #[serde(default = "default_chat_proactive_min_secs_since_partner")]
    pub proactive_min_secs_since_partner: u32,
    /// Above this gap, the conversation is considered dead — proactive
    /// ticks do not try to revive it. Default 5 minutes.
    #[serde(default = "default_chat_proactive_max_secs_since_partner")]
    pub proactive_max_secs_since_partner: u32,
    /// Probability (0-100) that a tick which passes every other gate
    /// actually fires the composer. The remaining percent stays
    /// silent. Default 20 % — enough to nudge a thread along without
    /// flooding.
    #[serde(default = "default_chat_proactive_probability_pct")]
    pub proactive_probability_pct: u32,
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self {
            enabled: default_chat_enabled(),
            dry_run: default_chat_dry_run(),
            api_key_env: default_chat_api_key_env(),
            composer_model: default_chat_composer_model(),
            classifier_model: default_chat_classifier_model(),
            composer_temperature: default_chat_composer_temperature(),
            classifier_temperature: default_chat_classifier_temperature(),
            persona_seed: String::new(),
            command_prefixes: default_chat_command_prefixes(),
            command_typo_max_distance: default_chat_command_typo_max_distance(),
            daily_input_token_cap: default_chat_daily_input_token_cap(),
            daily_output_token_cap: default_chat_daily_output_token_cap(),
            daily_classifier_token_cap: default_chat_daily_classifier_token_cap(),
            daily_dollar_cap_usd: default_chat_daily_dollar_cap_usd(),
            acknowledge_high_spend: false,
            recent_speaker_secs: default_chat_recent_speaker_secs(),
            classifier_sample_rate: default_chat_classifier_sample_rate(),
            classifier_per_sender_per_minute: default_chat_classifier_per_sender_per_minute(),
            classifier_min_confidence: default_chat_classifier_min_confidence(),
            classifier_context_messages: default_chat_classifier_context_messages(),
            active_hours_utc: None,
            min_silence_secs: default_chat_min_silence_secs(),
            max_replies_per_minute: default_chat_max_replies_per_minute(),
            typing_delay_base_ms: default_chat_typing_delay_base_ms(),
            typing_delay_per_char_ms: default_chat_typing_delay_per_char_ms(),
            typing_delay_jitter_ms: default_chat_typing_delay_jitter_ms(),
            typing_delay_floor_ms: default_chat_typing_delay_floor_ms(),
            typing_delay_max_ms: default_chat_typing_delay_max_ms(),
            adjustments_max_bullets: default_chat_adjustments_max_bullets(),
            player_memory_max_bytes: default_chat_player_memory_max_bytes(),
            update_bullet_max_chars: default_chat_update_bullet_max_chars(),
            update_self_memory_max_per_day: default_chat_update_self_memory_max_per_day(),
            memory_max_inferred_bullets: default_chat_memory_max_inferred_bullets(),
            composer_rpm_max: default_chat_composer_rpm_max(),
            classifier_rpm_max: default_chat_classifier_rpm_max(),
            composer_itpm_max: default_chat_composer_itpm_max(),
            classifier_itpm_max: default_chat_classifier_itpm_max(),
            rate_limit_wait_max_secs: default_chat_rate_limit_wait_max_secs(),
            web_fetch_max_bytes: default_chat_web_fetch_max_bytes(),
            web_fetch_daily_max: default_chat_web_fetch_daily_max(),
            web_fetch_enabled: false,
            web_search_enabled: default_chat_web_search_enabled(),
            cross_player_reads: false,
            tools_store_enabled: false,
            tools_store_max_calls_per_turn: default_chat_tools_store_max_calls_per_turn(),
            tools_store_trade_query_max_results:
                default_chat_tools_store_trade_query_max_results(),
            tools_store_cross_player_balance_lookups: false,
            reflection_max_pending: default_chat_reflection_max_pending(),
            reflection_idle_trigger_secs: default_chat_reflection_idle_trigger_secs(),
            reflection_min_interval_secs: default_chat_reflection_min_interval_secs(),
            reflection_min_distinct_senders: default_chat_reflection_min_distinct_senders(),
            reflection_min_distinct_triggers: default_chat_reflection_min_distinct_triggers(),
            tools_history_max_bytes: default_chat_tools_history_max_bytes(),
            history_max_line_bytes: default_chat_history_max_line_bytes(),
            trust3_max_days: default_chat_trust3_max_days(),
            persona_archive_max: default_chat_persona_archive_max(),
            archive_max_bytes: default_chat_archive_max_bytes(),
            uuid_resolve_queue_max: default_chat_uuid_resolve_queue_max(),
            composer_context_messages: default_chat_composer_context_messages(),
            composer_max_tool_iterations: default_chat_composer_max_tool_iterations(),
            composer_max_chars: default_chat_composer_max_chars(),
            spam_msgs_per_window: default_chat_spam_msgs_per_window(),
            spam_window_secs: default_chat_spam_window_secs(),
            spam_cooldown_secs: default_chat_spam_cooldown_secs(),
            history_search_max_days: default_chat_history_search_max_days(),
            history_retention_days: default_chat_history_retention_days(),
            decisions_retention_days: default_chat_decisions_retention_days(),
            hash_uuids_in_decisions: default_chat_hash_uuids_in_decisions(),
            moderation_backoff_secs: default_chat_moderation_backoff_secs(),
            persona_regen_cooldown_secs: default_chat_persona_regen_cooldown_secs(),
            composer_throttle_backoff_secs: default_chat_composer_throttle_backoff_secs(),
            proactive_threading_enabled: false,
            proactive_tick_secs: default_chat_proactive_tick_secs(),
            proactive_min_secs_since_bot: default_chat_proactive_min_secs_since_bot(),
            proactive_min_secs_since_partner: default_chat_proactive_min_secs_since_partner(),
            proactive_max_secs_since_partner: default_chat_proactive_max_secs_since_partner(),
            proactive_probability_pct: default_chat_proactive_probability_pct(),
        }
    }
}

fn default_chat_enabled() -> bool { false }
fn default_chat_dry_run() -> bool { false }
fn default_chat_api_key_env() -> String { "ANTHROPIC_API_KEY".to_string() }
fn default_chat_composer_model() -> String { "claude-sonnet-4-6".to_string() }
fn default_chat_classifier_model() -> String { "claude-haiku-4-5-20251001".to_string() }
fn default_chat_composer_temperature() -> Option<f32> { Some(0.8) }
fn default_chat_classifier_temperature() -> Option<f32> { Some(0.0) }
fn default_chat_command_typo_max_distance() -> u32 { 2 }
fn default_chat_daily_input_token_cap() -> u64 { 2_000_000 }
fn default_chat_daily_output_token_cap() -> u64 { 200_000 }
fn default_chat_daily_classifier_token_cap() -> u64 { 500_000 }
fn default_chat_daily_dollar_cap_usd() -> f64 { 5.00 }
fn default_chat_recent_speaker_secs() -> u32 { 600 }
fn default_chat_classifier_sample_rate() -> f32 { 0.5 }
fn default_chat_classifier_per_sender_per_minute() -> u32 { 3 }
fn default_chat_classifier_min_confidence() -> f32 { 0.6 }
fn default_chat_classifier_context_messages() -> u32 { 30 }
fn default_chat_min_silence_secs() -> u32 { 6 }
fn default_chat_max_replies_per_minute() -> u32 { 4 }
fn default_chat_typing_delay_base_ms() -> u32 { 800 }
fn default_chat_typing_delay_per_char_ms() -> u32 { 60 }
fn default_chat_typing_delay_jitter_ms() -> u32 { 250 }
fn default_chat_typing_delay_floor_ms() -> u32 { 400 }
fn default_chat_typing_delay_max_ms() -> u32 { 12_000 }
fn default_chat_adjustments_max_bullets() -> u32 { 50 }
fn default_chat_player_memory_max_bytes() -> u32 { 4096 }
fn default_chat_update_bullet_max_chars() -> u32 { 280 }
fn default_chat_update_self_memory_max_per_day() -> u32 { 3 }
fn default_chat_memory_max_inferred_bullets() -> u32 { 30 }
fn default_chat_composer_rpm_max() -> u32 { 20 }
fn default_chat_classifier_rpm_max() -> u32 { 40 }
fn default_chat_composer_itpm_max() -> u32 { 25_000 }
fn default_chat_classifier_itpm_max() -> u32 { 40_000 }
fn default_chat_rate_limit_wait_max_secs() -> u32 { 5 }
fn default_chat_web_fetch_max_bytes() -> u32 { 262_144 }
fn default_chat_web_fetch_daily_max() -> u32 { 50 }
fn default_chat_web_search_enabled() -> bool { true }
fn default_chat_reflection_max_pending() -> u32 { 5 }
fn default_chat_reflection_idle_trigger_secs() -> u32 { 900 }
fn default_chat_reflection_min_interval_secs() -> u32 { 3600 }
fn default_chat_reflection_min_distinct_senders() -> u32 { 3 }
fn default_chat_reflection_min_distinct_triggers() -> u32 { 3 }
fn default_chat_tools_history_max_bytes() -> u32 { 32_768 }
fn default_chat_history_max_line_bytes() -> u32 { 65_536 }
fn default_chat_trust3_max_days() -> u32 { 30 }
fn default_chat_persona_archive_max() -> u32 { 10 }
fn default_chat_archive_max_bytes() -> u32 { 1_048_576 }
fn default_chat_uuid_resolve_queue_max() -> u32 { 1024 }
fn default_chat_composer_context_messages() -> u32 { 60 }
fn default_chat_composer_max_tool_iterations() -> u32 { 5 }
fn default_chat_composer_max_chars() -> u32 { 240 }
fn default_chat_spam_msgs_per_window() -> u32 { 5 }
fn default_chat_spam_window_secs() -> u32 { 30 }
fn default_chat_spam_cooldown_secs() -> u32 { 300 }
fn default_chat_history_search_max_days() -> u32 { 14 }
fn default_chat_history_retention_days() -> u32 { 30 }
fn default_chat_decisions_retention_days() -> u32 { 30 }
fn default_chat_hash_uuids_in_decisions() -> bool { true }
fn default_chat_moderation_backoff_secs() -> u32 { 86_400 }
fn default_chat_persona_regen_cooldown_secs() -> u32 { 86_400 }
fn default_chat_composer_throttle_backoff_secs() -> u32 { 60 }
fn default_chat_proactive_tick_secs() -> u32 { 30 }
fn default_chat_proactive_min_secs_since_bot() -> u32 { 30 }
fn default_chat_proactive_min_secs_since_partner() -> u32 { 15 }
fn default_chat_proactive_max_secs_since_partner() -> u32 { 300 }
fn default_chat_proactive_probability_pct() -> u32 { 20 }
fn default_chat_tools_store_max_calls_per_turn() -> u32 { 4 }
fn default_chat_tools_store_trade_query_max_results() -> u32 { 50 }

impl ChatConfig {
    /// Validate the chat-config invariants. Returns a single human-readable
    /// error string on failure (with every problem listed) so the operator
    /// fixes the whole config in one pass — same shape as
    /// [`Config::validate`].
    ///
    /// Checked here:
    ///
    /// - `enabled = true` requires a non-empty `persona_seed` AND the seed
    ///   must pass [`crate::chat::persona::validate_seed`]'s rejection list
    ///.
    /// - `daily_dollar_cap_usd > 30.0` requires `acknowledge_high_spend = true`
    ///.
    /// - `classifier_sample_rate` and `classifier_min_confidence` in [0,1].
    /// - `command_typo_max_distance` in [0, 4].
    pub fn validate(&self) -> Result<(), String> {
        let mut errors = Vec::new();

        if self.enabled {
            if self.persona_seed.trim().is_empty() {
                errors.push("persona_seed is required when chat.enabled = true".to_string());
            } else if let Err(e) = crate::chat::persona::validate_seed(&self.persona_seed) {
                errors.push(format!("persona_seed: {e}"));
            }
        }

        if self.daily_dollar_cap_usd > 30.0 && !self.acknowledge_high_spend {
            errors.push(format!(
                "daily_dollar_cap_usd = {:.2} requires acknowledge_high_spend = true (operator opt-in for >$30/day)",
                self.daily_dollar_cap_usd
            ));
        }
        if self.daily_dollar_cap_usd < 0.0 || !self.daily_dollar_cap_usd.is_finite() {
            errors.push("daily_dollar_cap_usd must be a non-negative finite number".to_string());
        }

        if !(0.0..=1.0).contains(&self.classifier_sample_rate) {
            errors.push(format!(
                "classifier_sample_rate must be in [0.0, 1.0] (got {})",
                self.classifier_sample_rate
            ));
        }
        if !(0.0..=1.0).contains(&self.classifier_min_confidence) {
            errors.push(format!(
                "classifier_min_confidence must be in [0.0, 1.0] (got {})",
                self.classifier_min_confidence
            ));
        }

        if self.command_typo_max_distance > 4 {
            errors.push(format!(
                "command_typo_max_distance must be in [0, 4] (got {})",
                self.command_typo_max_distance
            ));
        }

        if self.composer_max_chars > 240 {
            errors.push(format!(
                "composer_max_chars must be <= 240 (got {})",
                self.composer_max_chars
            ));
        }
        if self.min_silence_secs == 0 {
            errors.push("min_silence_secs must be >= 1".to_string());
        }
        if self.typing_delay_base_ms == 0 {
            errors.push("typing_delay_base_ms must be > 0".to_string());
        }
        if self.typing_delay_max_ms == 0 {
            errors.push("typing_delay_max_ms must be > 0".to_string());
        }
        if self.spam_window_secs == 0 {
            errors.push("spam_window_secs must be > 0".to_string());
        }
        if self.spam_cooldown_secs == 0 {
            errors.push("spam_cooldown_secs must be > 0".to_string());
        }
        if self.recent_speaker_secs == 0 {
            errors.push("recent_speaker_secs must be > 0".to_string());
        }
        if self.rate_limit_wait_max_secs == 0 {
            errors.push("rate_limit_wait_max_secs must be > 0".to_string());
        }
        if self.composer_max_tool_iterations < 2 {
            errors.push("composer_max_tool_iterations must be >= 2 (1 disables tool dispatch)".to_string());
        }
        if self.daily_input_token_cap == 0 {
            errors.push("daily_input_token_cap must be > 0".to_string());
        }
        if self.daily_output_token_cap == 0 {
            errors.push("daily_output_token_cap must be > 0".to_string());
        }
        if self.daily_classifier_token_cap == 0 {
            errors.push("daily_classifier_token_cap must be > 0".to_string());
        }
        if let Some((start, end)) = self.active_hours_utc
            && (start >= 24 || end >= 24) {
            errors.push(format!(
                "active_hours_utc components must be in [0, 24) (got {start}, {end})"
            ));
        }

        // Store-read tools. When the operator opts in, both the
        // per-turn budget and the trade-query result cap MUST be
        // positive — a 0 silently disables every call and the model
        // gets a confusing "cap reached (0/0)" on every attempt.
        if self.tools_store_enabled {
            if self.tools_store_max_calls_per_turn == 0 {
                errors.push("tools_store_max_calls_per_turn must be > 0 when tools_store_enabled".to_string());
            }
            if self.tools_store_trade_query_max_results == 0 {
                errors.push("tools_store_trade_query_max_results must be > 0 when tools_store_enabled".to_string());
            }
            if self.tools_store_trade_query_max_results > 50 {
                errors.push(format!(
                    "tools_store_trade_query_max_results must be <= 50 (the schema cap; got {})",
                    self.tools_store_trade_query_max_results,
                ));
            }
        }

        // Proactive thread continuation. Most fields are bounded by
        // `evaluate_proactive_tick`'s defensive clamping, but operator
        // typos (`probability_pct: 200`, `min > max`, zero tick period)
        // should surface at config-load time instead of becoming a
        // silent always-skip or always-fire.
        if self.proactive_threading_enabled && self.proactive_tick_secs == 0 {
            errors.push("proactive_tick_secs must be > 0 when proactive_threading_enabled".to_string());
        }
        if self.proactive_probability_pct > 100 {
            errors.push(format!(
                "proactive_probability_pct must be in [0, 100] (got {})",
                self.proactive_probability_pct
            ));
        }
        if self.proactive_min_secs_since_partner >= self.proactive_max_secs_since_partner {
            errors.push(format!(
                "proactive_min_secs_since_partner ({}) must be < proactive_max_secs_since_partner ({})",
                self.proactive_min_secs_since_partner,
                self.proactive_max_secs_since_partner,
            ));
        }
        for (name, val) in [
            ("composer_temperature", self.composer_temperature),
            ("classifier_temperature", self.classifier_temperature),
        ] {
            if let Some(t) = val
                && (!t.is_finite() || !(0.0..=1.0).contains(&t))
            {
                errors.push(format!(
                    "{name} must be a finite value in [0.0, 1.0] (got {t})"
                ));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

/// Whether the active-hours window includes the given UTC hour. Returns
/// `true` for `None` (always on), `start == end` (always on), the
/// non-wrap case (`start <= h < end`), and the wrap case (`h >= start`
/// OR `h < end`).
pub fn within_active_hours_utc(active_hours_utc: Option<(u32, u32)>, hour_utc: u32) -> bool {
    match active_hours_utc {
        None => true,
        Some((s, e)) if s == e => true,
        Some((s, e)) if s < e => hour_utc >= s && hour_utc < e,
        Some((s, e)) => hour_utc >= s || hour_utc < e, // wrap-around
    }
}
fn default_chat_command_prefixes() -> Vec<String> {
    // Kept in sync with the verbs `parse_command` recognises in
    // store::command. A unit test in chat::conversation pins this list to
    // the parser's accepted verb set so adding a new command without
    // updating this default produces a test failure rather than silent chat
    // shadowing of the new command.
    [
        // Order commands + aliases
        "buy", "b", "sell", "s",
        "deposit", "d", "withdraw", "w",
        // Quick commands + aliases
        "price", "p", "balance", "bal", "pay",
        "items", "queue", "q", "cancel", "c",
        "status", "help", "h",
        // Operator commands + aliases (operator-only at dispatch, but still
        // reach the Store rather than chat — operators expect their typos to
        // get a hint, not an AI reply).
        "additem", "ai", "removeitem", "ri",
        "addcurrency", "ac", "removecurrency", "rc",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
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

        // Chat config validation. Reads, but does not mutate, `self.chat`.
        if let Err(e) = self.chat.validate() {
            errors.push(format!("chat: {e}"));
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
                chat: ChatConfig::default(),
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
            chat: ChatConfig::default(),
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
