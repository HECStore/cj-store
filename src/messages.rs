//! # Inter-Task Message Types
//!
//! Defines all message types used for communication between tasks:
//! - **[`StoreMessage`]**: Messages sent to the Store (from Bot or CLI)
//! - **[`BotInstruction`]**: Instructions sent from Store to Bot
//! - **[`ChestSyncReport`]**: Chest contents report from Bot to Store
//!
//! ## Communication Flow
//!
//! ```text
//! CLI --[CliMessage]--> Store --[BotInstruction]--> Bot
//!                        ^                           |
//!                        |                           |
//!                        +---[BotMessage/Sync]-------+
//! ```

use serde::{Deserialize, Serialize};

use crate::types::{Chest, User};
use tokio::sync::oneshot;

/// A single chat line observed by the bot, structured for the chat module.
///
/// In-memory only (no Serde derives) — the chat module's history writer
/// produces its own JSON record for each event. Sender is the raw Minecraft
/// username as it appears on the wire; `content` has the chat / whisper prefix
/// already stripped (e.g. "Steve whispers: hi" → `content = "hi"`).
#[derive(Debug, Clone)]
pub struct ChatEvent {
    pub kind: ChatEventKind,
    pub sender: String,
    pub content: String,
    pub recv_at: std::time::SystemTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatEventKind {
    Public,
    Whisper,
}

/// Commands sent from the CLI (or other operator surfaces) to `chat_task`.
///
/// The chat module's command channel is intentionally separate from
/// `StoreMessage` / `BotInstruction` so the Store remains ignorant of chat —
/// see CHAT.md
#[derive(Debug)]
pub enum ChatCommand {
    /// Graceful shutdown: chat task drains in-flight work and returns.
    Shutdown {
        ack: oneshot::Sender<()>,
    },
    /// Snapshot of runtime state for the operator (CHAT.md
    /// `Chat: status`).
    Status {
        respond_to: oneshot::Sender<crate::chat::ChatStatusReport>,
    },
    /// Toggle runtime pause flag.
    SetPaused {
        paused: bool,
        respond_to: oneshot::Sender<()>,
    },
    /// Toggle runtime dry-run override (independent of `chat.dry_run`
    /// in config).
    SetDryRun {
        dry_run: bool,
        respond_to: oneshot::Sender<()>,
    },
    /// Clear moderation backoff (CHAT.md
    /// `Chat: resume after moderation backoff`).
    ClearModerationBackoff {
        respond_to: oneshot::Sender<()>,
    },
    /// Run the retention sweep on demand. Normally triggered at
    /// startup and at the first event each new UTC day.
    RunRetentionSweep {
        respond_to: oneshot::Sender<crate::chat::retention::SweepReport>,
    },
    /// Run the AI-call-out reflection pass on demand (CHAT.md
    /// `Chat: run reflection now`). Reads `pending_adjustments.jsonl`,
    /// asks Haiku to paraphrase, validates, appends to `adjustments.md`.
    RunReflection {
        respond_to: oneshot::Sender<Result<crate::chat::reflection::ReflectionOutcome, String>>,
    },
    /// Bot disconnected from the server — chat task should cancel any
    /// in-flight composer call. Sent
    /// by the bot's `Event::Disconnect` handler.
    BotDisconnected,
    /// Show the last `last_n` decision-log lines (most recent UTC day).
    ShowDecisionLog {
        last_n: usize,
        respond_to: oneshot::Sender<Result<Vec<String>, String>>,
    },
    /// Re-render the system prompt that would have been sent for a given
    /// historical event timestamp. Pure local replay; no API call.
    ReplayEvent {
        event_ts: String,
        respond_to: oneshot::Sender<Result<String, String>>,
    },
    /// Reset (delete) one player's per-player memory file.
    ResetPlayerMemory {
        username: String,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Print one player's per-player memory file contents.
    DumpPlayerMemory {
        username: String,
        respond_to: oneshot::Sender<Result<String, String>>,
    },
    /// Set or clear operator-managed `Trust: 3`. `set = false` clears,
    /// `set = true` writes the heading and `trust3_expires_at` line.
    SetOperatorTrust {
        username: String,
        set: bool,
        reason: String,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Regenerate `persona.md` from `persona.seed`. Honors a 24h
    /// cooldown stored in `state.persona_regen_cooldown_until`.
    RegeneratePersona {
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// "Right to be forgotten": purge a player's UUID from per-player
    /// file, history JSONL records, decisions JSONL records, UUID
    /// overlay sidecars, `pending_adjustments.jsonl` (live + rotated
    /// `pending_adjustments.<UTC>.jsonl` archives), rotated
    /// `pending_self_memory.<UTC>.jsonl` archives, and matching entries
    /// in `data/chat/players/_index.json`; logs the action to
    /// `operator_audit.jsonl`.
    ForgetPlayer {
        username: String,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
}

/// Type of queued order for the order queue system.
///
/// Serialized as part of the on-disk queue file, so any variant or field
/// rename is a persisted-format break (see `store/queue.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueuedOrderType {
    Buy,
    Sell,
    Deposit {
        /// Specific amount to deposit, or `None` for flexible deposit
        /// (any quantity the player offers is accepted).
        amount: Option<f64>,
    },
    Withdraw {
        /// Specific amount to withdraw, or `None` to withdraw the full balance.
        amount: Option<f64>,
    },
}

/// An item with quantity, used in trade negotiations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeItem {
    /// Item identifier (e.g., `"minecraft:diamond"`).
    pub item: String,
    pub amount: i32,
}

/// Report of chest contents after a chest operation.
///
/// Sent from Bot to Store to sync authoritative state. In-memory-only (no
/// Serde derives) so the fixed-size `amounts` array does not leak into any
/// on-disk format; the persisted `Chest.amounts` remains `Vec<i32>` and is
/// updated slot-by-slot in `apply_chest_sync`.
#[derive(Debug, Clone)]
pub struct ChestSyncReport {
    /// Chest ID, computed as `node_id * 4 + chest_index`.
    pub chest_id: i32,
    pub item: String,
    /// Per-slot item counts for the double chest — exactly 54 entries, one
    /// per slot. A value of `-1` means "bot did not inspect this slot;
    /// preserve the existing stored value" (see `apply_chest_sync`).
    pub amounts: [i32; crate::constants::DOUBLE_CHEST_SLOTS],
}

/// Actions that can be performed on a chest.
#[derive(Debug, Clone)]
pub enum ChestAction {
    Deposit {
        item: String,
        amount: i32,
        /// Player the items originated from, or `None` for system-initiated deposits.
        from_player: Option<String>,
        /// Item's stack size (1, 16, or 64) — used for chest capacity calculation.
        stack_size: i32,
    },
    Withdraw {
        item: String,
        amount: i32,
        /// Player to receive the items, or `None` if withdrawal is not player-bound.
        to_player: Option<String>,
        /// Item's stack size (1, 16, or 64) — used for chest capacity calculation.
        stack_size: i32,
    },
}

/// Messages sent to the Store from other components.
///
/// The Store multiplexes its inbox over this enum so a single channel can
/// serve both the in-game Bot and the operator CLI.
///
/// `Debug` is derived so diagnostic traces can log message shape without a
/// manual `match` at every log site; `oneshot::Sender<T>` already implements
/// `Debug`, so the nested variants below don't need hand-written impls.
#[derive(Debug)]
pub enum StoreMessage {
    FromBot(BotMessage),
    FromCli(CliMessage),
    /// Hot-reload the in-memory config from `data/config.json`. Sent by the
    /// file watcher task when the config file changes on disk. Only a subset
    /// of fields is applied live — see `Store::reload_config` for the accepted
    /// fields and the warning emitted when a restart-only field is edited.
    ReloadConfig(crate::config::Config),
}

/// Messages from Bot to Store.
#[derive(Debug)]
pub enum BotMessage {
    PlayerCommand {
        player_name: String,
        /// Raw command text as received, with the whisper prefix already stripped.
        command: String,
    },
}

/// Messages from CLI to Store.
#[derive(Debug)]
pub enum CliMessage {
    QueryBalances {
        respond_to: oneshot::Sender<Vec<User>>,
    },
    QueryPairs {
        respond_to: oneshot::Sender<Vec<crate::types::Pair>>,
    },
    QueryFee {
        respond_to: oneshot::Sender<f64>,
    },
    SetOperator {
        username_or_uuid: String,
        is_operator: bool,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Add a new node without physical validation (operator responsibility).
    /// Use [`CliMessage::AddNodeWithValidation`] for bot-based validation.
    AddNode {
        respond_to: oneshot::Sender<Result<i32, String>>,
    },
    /// Add a new node, having the bot navigate to the calculated position
    /// and verify that all 4 chests exist, open, and contain shulker boxes
    /// in every slot. The node is only added if every check passes.
    AddNodeWithValidation {
        respond_to: oneshot::Sender<Result<i32, String>>,
    },
    RemoveNode {
        node_id: i32,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    AddPair {
        item_name: String,
        /// Stack size for this item (1, 16, or 64).
        stack_size: i32,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    RemovePair {
        item_name: String,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    QueryStorage {
        respond_to: oneshot::Sender<crate::types::Storage>,
    },
    QueryTrades {
        limit: usize,
        respond_to: oneshot::Sender<Vec<crate::types::Trade>>,
    },
    RestartBot {
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Audit state invariants. If `repair` is true, safe-to-repair issues are
    /// fixed in place; otherwise the audit is read-only.
    AuditState {
        repair: bool,
        respond_to: oneshot::Sender<Vec<String>>,
    },
    /// Discover storage nodes by having the bot physically visit positions
    /// 0, 1, 2, ... until it finds one without valid chests. Each discovered
    /// node is validated and added to storage. Returns the count discovered.
    DiscoverStorage {
        respond_to: oneshot::Sender<Result<usize, String>>,
    },
    Shutdown {
        respond_to: oneshot::Sender<()>,
    },
    /// Clear stuck order processing state: resets `processing_order` to false
    /// and clears `current_trade` so the queue can resume. Returns the stuck
    /// order's identifier if one was cleared.
    ClearStuckOrder {
        respond_to: oneshot::Sender<Option<String>>,
    },
}

/// Instructions from Store to Bot.
///
/// Every variant that needs a result carries a `oneshot::Sender` so the Store
/// can `await` the Bot's outcome while remaining fully async.
#[derive(Debug)]
pub enum BotInstruction {
    Whisper {
        target: String,
        message: String,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Send a public chat line. Used by the chat module to speak in open
    /// chat. Whispers (DMs) reuse the existing `Whisper` variant.
    SendChat {
        content: String,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Navigate to a chest, perform the given action, then read chest
    /// contents and return a sync report.
    InteractWithChestAndSync {
        target_chest: Chest,
        node_position: crate::types::Position,
        action: ChestAction,
        respond_to: oneshot::Sender<Result<ChestSyncReport, String>>,
    },
    /// Perform a full trade via the server trade GUI.
    ///
    /// `bot_offers` fill the bot's 12 slots (left side); `player_offers`
    /// describe what the bot expects the player to place in their 12 slots
    /// (right side). Returns the items actually received, which may differ
    /// from `player_offers` when `flexible_validation` is set or when a sell
    /// order accepts under-offers.
    ///
    /// `require_exact_amount`: reject trades where the player offers MORE
    /// than expected (used for sells, where exact quantity is required).
    ///
    /// `flexible_validation`: accept any amount >= 1 of expected items and
    /// ignore the `amount` field (used for deposits without a specified
    /// amount).
    TradeWithPlayer {
        target_username: String,
        bot_offers: Vec<TradeItem>,
        player_offers: Vec<TradeItem>,
        require_exact_amount: bool,
        flexible_validation: bool,
        respond_to: oneshot::Sender<Result<Vec<TradeItem>, String>>,
    },
    /// Validate a node position: bot navigates there, opens each of the 4
    /// chests, and verifies every slot contains a shulker box. Returns
    /// `Ok(())` if all checks pass.
    ValidateNode {
        node_id: i32,
        node_position: crate::types::Position,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Restart the bot.
    ///
    /// Fire-and-forget: no response channel because the bot task tears itself
    /// down and is re-spawned by the supervisor, so the original sender would
    /// no longer exist to receive an ack.
    Restart,
    Shutdown {
        respond_to: oneshot::Sender<()>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    // Serde round-trip coverage for the two types in this file that cross the
    // on-disk boundary. `QueuedOrderType` is persisted as part of the order
    // queue file; `TradeItem` rides inside it. A silent rename of any variant
    // or field here would corrupt the queue on next load, so every shape
    // gets explicit coverage.

    fn roundtrip_order(order: &QueuedOrderType) -> QueuedOrderType {
        let json = serde_json::to_string(order).expect("serialize");
        serde_json::from_str(&json).expect("deserialize")
    }

    #[test]
    fn queued_order_type_buy_roundtrips_through_json() {
        let order = QueuedOrderType::Buy;
        assert!(matches!(roundtrip_order(&order), QueuedOrderType::Buy));
    }

    #[test]
    fn queued_order_type_sell_roundtrips_through_json() {
        let order = QueuedOrderType::Sell;
        assert!(matches!(roundtrip_order(&order), QueuedOrderType::Sell));
    }

    #[test]
    fn queued_order_type_deposit_with_amount_preserves_value() {
        let order = QueuedOrderType::Deposit { amount: Some(12.5) };
        match roundtrip_order(&order) {
            QueuedOrderType::Deposit { amount: Some(a) } => assert_eq!(a, 12.5),
            other => panic!("expected Deposit {{ Some(12.5) }}, got {other:?}"),
        }
    }

    #[test]
    fn queued_order_type_deposit_without_amount_preserves_none() {
        let order = QueuedOrderType::Deposit { amount: None };
        assert!(matches!(
            roundtrip_order(&order),
            QueuedOrderType::Deposit { amount: None }
        ));
    }

    #[test]
    fn queued_order_type_withdraw_with_amount_preserves_value() {
        let order = QueuedOrderType::Withdraw { amount: Some(3.0) };
        match roundtrip_order(&order) {
            QueuedOrderType::Withdraw { amount: Some(a) } => assert_eq!(a, 3.0),
            other => panic!("expected Withdraw {{ Some(3.0) }}, got {other:?}"),
        }
    }

    #[test]
    fn queued_order_type_withdraw_without_amount_preserves_none() {
        let order = QueuedOrderType::Withdraw { amount: None };
        assert!(matches!(
            roundtrip_order(&order),
            QueuedOrderType::Withdraw { amount: None }
        ));
    }

    #[test]
    fn trade_item_roundtrips_through_json_preserving_both_fields() {
        let item = TradeItem {
            item: "minecraft:diamond".to_string(),
            amount: 64,
        };
        let json = serde_json::to_string(&item).expect("serialize");
        let decoded: TradeItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.item, "minecraft:diamond");
        assert_eq!(decoded.amount, 64);
    }
}
