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

/// Type of queued order for the order queue system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueuedOrderType {
    /// Buy items from the store
    Buy,
    /// Sell items to the store
    Sell,
    /// Deposit diamonds to balance
    Deposit {
        /// Specific amount to deposit, or None for flexible deposit
        amount: Option<f64>,
    },
    /// Withdraw diamonds from balance
    Withdraw {
        /// Specific amount to withdraw, or None for full balance
        amount: Option<f64>,
    },
}

/// An item with quantity, used in trade negotiations.
#[derive(Debug, Clone)]
pub struct TradeItem {
    /// Item identifier (e.g., "minecraft:diamond")
    pub item: String,
    /// Quantity of items
    pub amount: i32,
}

/// Report of chest contents after a chest operation.
///
/// Sent from Bot to Store to sync authoritative state.
/// Contains per-slot item counts for the entire chest (54 slots).
#[derive(Debug, Clone)]
pub struct ChestSyncReport {
    /// Chest ID (calculated as `node_id * 4 + chest_index`)
    pub chest_id: i32,
    /// Item type stored in this chest
    pub item: String,
    /// Per-slot item counts (length 54, one per shulker box slot)
    pub amounts: Vec<i32>,
}

/// Actions that can be performed on a chest.
#[derive(Debug, Clone)]
pub enum ChestAction {
    /// Place `amount` of `item` into the chest, optionally attributed to a player.
    Deposit {
        item: String,
        amount: i32,
        /// Player the items originated from, or `None` for system-initiated deposits.
        from_player: Option<String>,
        /// Item's stack size (1, 16, or 64) for capacity calculation
        stack_size: i32,
    },
    /// Take `amount` of `item` out of the chest, optionally delivered to a player.
    Withdraw {
        item: String,
        amount: i32,
        /// Player to receive the items, or `None` if withdrawal is not player-bound.
        to_player: Option<String>,
        /// Item's stack size (1, 16, or 64) for capacity calculation
        stack_size: i32,
    },
}

/// Messages sent to the Store from other components.
///
/// The Store multiplexes its inbox over this enum so a single channel can
/// serve both the in-game Bot and the operator CLI.
pub enum StoreMessage {
    /// Message originating from the in-game Bot task.
    FromBot(BotMessage),
    /// Message originating from the operator CLI task.
    FromCli(CliMessage),
    /// Request the Store to hot-reload its in-memory config from
    /// `data/config.json`. Sent by the file watcher task when the config
    /// file changes on disk. Only a subset of fields is applied live —
    /// see `Store::reload_config` for the accepted fields and the warning
    /// emitted when a restart-only field is edited.
    ReloadConfig(crate::config::Config),
}

/// Messages from Bot to Store.
pub enum BotMessage {
    /// Player sent a command (e.g., "/msg HECStore buy cobblestone 256").
    PlayerCommand {
        /// In-game name of the player who issued the command.
        player_name: String,
        /// Raw command text as received, minus the whisper prefix.
        command: String,
    },
}

/// Messages from CLI to Store.
pub enum CliMessage {
    /// Request all user balances.
    QueryBalances {
        respond_to: oneshot::Sender<Vec<User>>,
    },
    /// Request all pairs.
    QueryPairs {
        respond_to: oneshot::Sender<Vec<crate::types::Pair>>,
    },
    /// Request the current fee rate from config.
    QueryFee {
        respond_to: oneshot::Sender<f64>,
    },
    /// Set operator status for a user.
    SetOperator {
        username_or_uuid: String,
        is_operator: bool,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Add a new node (WITHOUT physical validation - operator responsibility).
    /// Use `AddNodeWithValidation` for bot-based validation.
    AddNode {
        respond_to: oneshot::Sender<Result<i32, String>>,
    },
    /// Add a new node WITH bot-based physical validation.
    /// Bot will navigate to the calculated position and verify:
    /// 1. All 4 chests exist and can be opened
    /// 2. Each chest slot contains a shulker box
    /// Only adds the node if all checks pass.
    AddNodeWithValidation {
        respond_to: oneshot::Sender<Result<i32, String>>,
    },
    /// Remove a node (validates node can be removed).
    RemoveNode {
        node_id: i32,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Add a new pair (stocks set to zero).
    AddPair {
        item_name: String,
        /// Stack size for this item (1, 16, or 64)
        stack_size: i32,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Remove a pair (validates pair can be removed).
    RemovePair {
        item_name: String,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Query storage state (nodes and chest allocation).
    QueryStorage {
        respond_to: oneshot::Sender<crate::types::Storage>,
    },
    /// Query recent trades.
    QueryTrades {
        limit: usize,
        respond_to: oneshot::Sender<Vec<crate::types::Trade>>,
    },
    /// Request bot restart.
    RestartBot {
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Audit state invariants (and optionally repair safe issues).
    AuditState {
        repair: bool,
        respond_to: oneshot::Sender<Vec<String>>,
    },
    /// Discover storage nodes by having the bot physically visit positions.
    /// 
    /// Bot will iterate through node positions (0, 1, 2, ...) until it finds
    /// a position without valid chests. Each discovered node is validated
    /// and added to storage.
    /// 
    /// Returns the number of nodes discovered.
    DiscoverStorage {
        respond_to: oneshot::Sender<Result<usize, String>>,
    },
    /// Signal graceful shutdown.
    Shutdown {
        respond_to: oneshot::Sender<()>,
    },
    /// Clear stuck order processing state.
    /// 
    /// This resets `processing_order` to false and clears `current_trade`,
    /// allowing the queue to continue processing. Use when an order gets
    /// stuck due to timeout or other issues.
    /// 
    /// Returns the order that was stuck (if any).
    ClearStuckOrder {
        respond_to: oneshot::Sender<Option<String>>,
    },
}

/// Instructions from Store to Bot.
///
/// Every variant that needs a result carries a `oneshot::Sender` so the Store
/// can `await` the Bot's outcome while remaining fully async.
pub enum BotInstruction {
    /// Whisper a message to a player.
    Whisper {
        target: String,
        message: String,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Send a public chat message.
    /// 
    /// Navigate to chest, perform action, then read chest contents and return a sync report.
    InteractWithChestAndSync {
        target_chest: Chest,
        node_position: crate::types::Position,
        action: ChestAction,
        respond_to: oneshot::Sender<Result<ChestSyncReport, String>>,
    },
    /// Perform a full trade via the server trade GUI.
    ///
    /// `bot_offers` are items the bot will place in its 12 slots (left side).
    /// `player_offers` are items the bot expects the player to place in their 12 slots (right side).
    /// 
    /// Returns `Ok(Vec<TradeItem>)` with the actual items received from the player.
    /// This allows the caller to handle partial payments (e.g., player offers fewer diamonds
    /// than requested, with the rest covered by their balance).
    /// 
    /// **Validation modes**:
    /// - `require_exact_amount`: If true, reject trades where player offers MORE than expected.
    ///   Use for sell orders where exact quantity is required.
    /// - `flexible_validation`: If true, accept any amount >= 1 of expected items (ignore amount field).
    ///   Use for deposit commands without a specified amount.
    TradeWithPlayer {
        target_username: String,
        bot_offers: Vec<TradeItem>,
        player_offers: Vec<TradeItem>,
        /// If true, reject trades where player offers more than expected amount
        require_exact_amount: bool,
        /// If true, accept any amount >= 1 of expected items (flexible deposit mode)
        flexible_validation: bool,
        respond_to: oneshot::Sender<Result<Vec<TradeItem>, String>>,
    },
    /// Validate a node position by physically checking chests exist and contain shulkers.
    /// 
    /// Bot will:
    /// 1. Navigate to the node position
    /// 2. Attempt to open each of the 4 chests
    /// 3. Verify each chest slot contains a shulker box
    /// 
    /// Returns Ok(()) if all checks pass, Err(description) otherwise.
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
    /// Shutdown the bot gracefully.
    Shutdown {
        respond_to: oneshot::Sender<()>,
    },
}
