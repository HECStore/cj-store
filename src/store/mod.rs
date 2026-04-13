//! # Store - Authoritative State Management
//!
//! The Store is the **single source of truth** for all store state:
//! - Users (balances, operator status)
//! - Trading pairs (item/currency reserves)
//! - Orders (audit log)
//! - Trades (execution history)
//! - Storage (nodes, chests, shulker contents)

pub mod handlers;
pub mod journal;
pub mod orders;
pub mod pricing;
pub mod queue;
pub mod rate_limit;
pub mod rollback;
pub mod state;
pub mod trade_state;
pub mod utils;

use std::collections::{HashMap, VecDeque};
use std::io;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::messages::{BotInstruction, BotMessage, ChestSyncReport, StoreMessage};
use crate::types::{ItemId, Order, Pair, Storage, Trade, User};

use self::queue::OrderQueue;
use self::rate_limit::RateLimiter;

/// The Store: authoritative state manager for the entire system.
///
/// **Ownership**: Owns all mutable state (users, pairs, orders, trades, storage).
/// Only the Store task mutates this state (single-threaded access via message loop).
pub struct Store {
    /// Configuration (storage origin, fee rate, bot credentials)
    pub config: Config,
    /// Trading pairs: item -> Pair (reserves and stock)
    pub pairs: HashMap<String, Pair>,
    /// Users: UUID -> User (balance, operator status)
    pub users: HashMap<String, User>,
    /// Order audit log (all executed buy/sell/deposit/withdraw orders)
    pub orders: VecDeque<Order>,
    /// Trade history (executed trades)
    pub trades: Vec<Trade>,
    /// Physical storage (nodes, chests, shulker contents)
    pub storage: Storage,
    /// Dirty flag: true if state changed since last save
    pub(crate) dirty: bool,

    /// Channel to send instructions to the bot
    pub(crate) bot_tx: mpsc::Sender<BotInstruction>,

    // ========================================================================
    // Order Queue System
    // ========================================================================
    
    /// Queue of pending orders waiting to be processed
    pub order_queue: OrderQueue,
    /// Rate limiter for anti-spam protection
    pub rate_limiter: RateLimiter,
    /// Flag to prevent concurrent order processing
    pub processing_order: bool,
    /// The trade currently being processed, tracked as a formal state machine.
    /// `None` when idle; set to `Some(TradeState::Queued(..))` when an order
    /// is popped and advanced through phases until a terminal state.
    pub current_trade: Option<trade_state::TradeState>,
}

impl Store {
    /// Creates a new `Store` instance, loading the configuration.
    pub async fn new(bot_tx: mpsc::Sender<BotInstruction>) -> io::Result<Self> {

        let config = Config::load()?;
        let mut pairs = Pair::load_all()?;
        
        // Normalize all pair item IDs to ensure consistent lookup
        // This strips "minecraft:" prefix from item names for cleaner storage/display
        // Also filters out invalid pairs (empty item names)
        //
        // Normalization happens at load time (not lookup time) so that the in-memory
        // HashMap key, the Pair.item field, and the on-disk filename all agree on the
        // same canonical form. This avoids subtle bugs where e.g. "minecraft:diamond"
        // and "diamond" would be treated as distinct pairs.
        let mut normalized_pairs = HashMap::new();
        let mut needs_save = false;
        for (old_key, mut pair) in pairs.drain() {
            // Skip pairs with empty item names
            if pair.item.trim().is_empty() {
                warn!("Skipping pair with empty item name (file key: {})", old_key);
                needs_save = true; // Will remove invalid pair file
                continue;
            }
            let normalized_item = utils::normalize_item_id(&pair.item);
            // Skip pairs that normalize to empty
            if normalized_item.is_empty() {
                warn!("Skipping pair with invalid item name '{}' (normalized to empty)", pair.item);
                needs_save = true; // Will remove invalid pair file
                continue;
            }
            // If the item was not normalized (e.g., had minecraft: prefix), we need to update it and save
            if old_key != normalized_item {
                warn!("Normalizing pair item name from '{}' to '{}'", old_key, normalized_item);
                needs_save = true;
            }
            // Update the pair's item field to normalized form (without minecraft: prefix)
            pair.item = ItemId::from_normalized(normalized_item.clone());
            // Insert with normalized key
            normalized_pairs.insert(normalized_item, pair);
        }
        let pairs = normalized_pairs;
        
        let users = User::load_all()?;
        
        // Orders are session-only - start fresh on each restart.
        //
        // Rationale: an Order represents an in-flight user request that is tied to
        // the live bot session (player connectivity, chest state, queue position).
        // Replaying a half-finished order across restarts would risk double-charging
        // users or desyncing against actual chest contents. Trades (the settled
        // audit log) and pair reserves ARE persisted - only the transient order
        // log is dropped. The stale file on disk is removed so operators inspecting
        // data/ don't mistake it for live state.
        let orders_file = std::path::Path::new("data/orders.json");
        if orders_file.exists() {
            if let Err(e) = std::fs::remove_file(orders_file) {
                warn!("Failed to clear orders.json on startup: {}", e);
            }
        }
        let orders = std::collections::VecDeque::new();
        
        let trades = Trade::load_all_with_limit(config.max_trades_in_memory)?;
        let mut storage = Storage::load(&config.position)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        // If storage is empty, auto-create node 0
        if storage.nodes.is_empty() {
            info!("Storage empty, auto-creating node 0");
            let node = storage.add_node();
            if let Err(e) = node.save() {
                warn!("Failed to save auto-created node 0: {}", e);
            }
        }

        // Load order queue from disk (persistent across restarts)
        let order_queue = match OrderQueue::load() {
            Ok(queue) => queue,
            Err(e) => {
                warn!("Failed to load order queue, starting fresh: {}", e);
                OrderQueue::new()
            }
        };

        let rate_limiter = RateLimiter::new();

        info!(
            "Store initialized successfully with {} pairs, {} users, {} orders, {} nodes",
            pairs.len(),
            users.len(),
            orders.len(),
            storage.nodes.len()
        );

        Ok(Store {
            config,
            pairs,
            users,
            orders,
            trades,
            storage,
            dirty: needs_save, // Mark dirty if pairs were normalized (will save on first autosave)
            bot_tx,
            order_queue,
            rate_limiter,
            processing_order: false,
            current_trade: None,
        })
    }

    /// Main event loop for the Store.
    ///
    /// Processes messages and orders sequentially to ensure reliable bot operations.
    /// Order processing is NOT cancelled when new messages arrive - orders run to
    /// completion before the next message is processed.
    ///
    /// **Message handling**: Quick commands (balance, price, help, queue status) execute
    /// immediately. Order commands (buy, sell, deposit, withdraw) are queued and processed
    /// one at a time to ensure reliable bot operations.
    ///
    /// **Shutdown behavior**: When a `CliMessage::Shutdown` is received, the Store:
    /// 1. Handles the shutdown message (signals Bot, waits for confirmation, saves data)
    /// 2. Breaks from the loop immediately (doesn't wait for channel closure)
    /// 3. Performs final cleanup (saves data again as safety measure, drops bot_tx)
    ///
    /// See README.md "Graceful Shutdown" section for the complete shutdown sequence.
    pub async fn run(
        mut self,
        mut store_rx: mpsc::Receiver<StoreMessage>,
        bot_tx: mpsc::Sender<BotInstruction>,
    ) {
        info!("Store started (autosave every {}s)", self.config.autosave_interval_secs);
        let mut last_save = tokio::time::Instant::now();
        let min_save_interval = tokio::time::Duration::from_secs(self.config.autosave_interval_secs);

        // Main event loop. Each iteration either drains one order from the queue
        // OR blocks on one incoming message - never both concurrently. Orders are
        // given strict priority over messages (see PRIORITY 1/2 below) so that an
        // in-flight trade cannot be starved or interrupted by chatty players.
        loop {
            // Periodic state logging for debugging stuck conditions
            if !self.order_queue.is_empty() || self.processing_order {
                debug!("[Store] Loop state: processing_order={} queue_len={}",
                       self.processing_order, self.order_queue.len());
                if let Some(ref trade) = self.current_trade {
                    debug!("[Store] Current trade: {}", trade);
                }
            }

            // PRIORITY 1: Process queued orders first (if any and not already processing)
            // This ensures order processing runs to COMPLETION before handling new messages.
            // Previously, using tokio::select! would CANCEL order processing when messages
            // arrived, causing the oneshot channel receiver to be dropped mid-operation.
            //
            // The ordering here is deliberate: we poll the order queue on every loop
            // iteration BEFORE calling store_rx.recv(). Any messages that arrive while
            // an order is executing simply accumulate in the channel buffer and are
            // picked up on a later iteration once the queue drains.
            if !self.processing_order && !self.order_queue.is_empty() {
                debug!("[Store] Starting order processing (queue_len={})", self.order_queue.len());
                self.process_next_order().await;

                // ALWAYS save after order completion for data integrity
                // (trades, stock updates must not be lost due to crash).
                if self.dirty {
                    if let Err(e) = state::save(&self) {
                        error!("[Store] Autosave failed: {}", e);
                    } else {
                        last_save = tokio::time::Instant::now();
                        self.dirty = false;
                    }
                }
                
                // Continue loop to check for more orders before blocking on messages
                continue;
            }

            // PRIORITY 2: Wait for incoming messages (blocking)
            // Only reached when no orders are being processed
            let msg = store_rx.recv().await;
            match msg {
                Some(message) => {
                    debug!("[Store] Received message: {:?}", std::mem::discriminant(&message));
                    let is_shutdown = matches!(&message, StoreMessage::FromCli(crate::messages::CliMessage::Shutdown { .. }));

                    match message {
                        StoreMessage::FromBot(bot_msg) => {
                            if let Err(e) = self.handle_bot_message(bot_msg).await {
                                error!("Error handling bot message: {}", e);
                            }
                        }
                        StoreMessage::FromCli(cli_msg) => {
                            if let Err(e) = handlers::cli::handle_cli_message(&mut self, cli_msg).await {
                                error!("[Store] Error handling CLI message: {}", e);
                            }
                        }
                    }

                    if is_shutdown {
                        info!("[Store] Shutdown handled, exiting main loop");
                        break;
                    }

                    // Debounced autosave: save at most every `min_save_interval`.
                    if self.dirty && last_save.elapsed() >= min_save_interval {
                        if let Err(e) = state::save(&self) {
                            error!("[Store] Autosave failed: {}", e);
                        } else {
                            last_save = tokio::time::Instant::now();
                            self.dirty = false;
                        }
                    }
                }
                None => {
                    info!("[Store] Channel closed, exiting");
                    break;
                }
            }
        }

        // Final cleanup: save and drop channels
        if let Err(e) = state::save(&self) {
            error!("[Store] Final save failed: {}", e);
        }
        drop(bot_tx);
        info!("[Store] Shutdown complete");
    }

    /// Process the next order from the queue
    ///
    /// This method is called by the main store loop when there are orders waiting
    /// and no order is currently being processed. It sets `processing_order = true`
    /// at the start and `false` at the end to prevent concurrent order execution.
    ///
    /// Note: The order handlers (buy, sell, deposit, withdraw) send their own
    /// messages to the player, so we only send the "Now processing" notification
    /// here and log the result for debugging purposes.
    async fn process_next_order(&mut self) {
        // Pop the next order
        let order = match self.order_queue.pop() {
            Some(o) => o,
            None => {
                warn!("[Store] Queue was empty when trying to pop");
                return;
            }
        };

        self.processing_order = true;
        self.current_trade = Some(trade_state::TradeState::new(order.clone()));

        // Notify user that their order is being processed
        let processing_msg = format!("Now processing: {}...", order.description());
        if let Err(e) = utils::send_message_to_player(self, &order.username, &processing_msg).await {
            warn!("[Store] Failed to notify user {} of order start: {}", order.username, e);
        }

        // Execute the order (handlers send their own completion/error messages)
        let result = orders::execute_queued_order(self, &order).await;

        if let Err(error_msg) = &result {
            error!("[Store] Order #{} failed: {}", order.id, error_msg);
        }

        self.processing_order = false;
        self.current_trade = None;
        self.dirty = true;
    }

    /// Advance the in-flight trade through the state machine.
    ///
    /// Takes a closure that receives the current `TradeState` by value and
    /// returns the next state.  If no trade is active the call is a no-op
    /// (logged at debug level).
    pub(crate) fn advance_trade(&mut self, transition: impl FnOnce(trade_state::TradeState) -> trade_state::TradeState) {
        if let Some(state) = self.current_trade.take() {
            let next = transition(state);
            debug!("[Store] Trade advanced to: {}", next.phase());
            self.current_trade = Some(next);
        } else {
            debug!("[Store] advance_trade called with no active trade (no-op)");
        }
    }

    /// Handle messages from the bot
    async fn handle_bot_message(&mut self, message: BotMessage) -> Result<(), String> {
        match message {
            BotMessage::PlayerCommand {
                player_name,
                command,
            } => {
                info!("Processing command from {}: {}", player_name, command);
                handlers::player::handle_player_command(self, &player_name, &command).await
            }
        }
    }

    /// Apply chest sync report from bot (merges bot-reported slot counts into storage)
    pub(crate) fn apply_chest_sync(&mut self, report: ChestSyncReport) -> Result<(), String> {
        state::apply_chest_sync(self, report)
    }

    /// Get node position for a given chest_id
    pub(crate) fn get_node_position(&self, chest_id: i32) -> crate::types::Position {
        utils::get_node_position(self, chest_id)
    }

    /// Build a fully in-memory `Store` for integration tests.
    ///
    /// Bypasses all disk I/O (`Config::load`, `Pair::load_all`, `Storage::load`,
    /// `Trade::load_all`, `OrderQueue::load`) so tests can exercise handler
    /// logic without touching `data/`. Callers supply their own bot channel
    /// and fabricate `pairs`/`users`/`storage` inline.
    #[cfg(test)]
    pub fn new_for_test(
        bot_tx: mpsc::Sender<BotInstruction>,
        config: crate::config::Config,
        pairs: HashMap<String, Pair>,
        users: HashMap<String, User>,
        storage: crate::types::Storage,
    ) -> Self {
        Store {
            config,
            pairs,
            users,
            orders: VecDeque::new(),
            trades: Vec::new(),
            storage,
            dirty: false,
            bot_tx,
            order_queue: queue::OrderQueue::new(),
            rate_limiter: RateLimiter::new(),
            processing_order: false,
            current_trade: None,
        }
    }
}
