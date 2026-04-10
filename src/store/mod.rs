//! # Store - Authoritative State Management
//!
//! The Store is the **single source of truth** for all store state:
//! - Users (balances, operator status)
//! - Trading pairs (item/currency reserves)
//! - Orders (audit log)
//! - Trades (execution history)
//! - Storage (nodes, chests, shulker contents)

pub mod handlers;
pub mod orders;
pub mod pricing;
pub mod queue;
pub mod rate_limit;
pub mod state;
pub mod utils;

use std::collections::{HashMap, VecDeque};
use std::io;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::messages::{BotInstruction, BotMessage, ChestSyncReport, StoreMessage};
use crate::types::{Order, Pair, Storage, Trade, User};

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
    /// The order currently being processed (for status reporting)
    pub current_order: Option<queue::QueuedOrder>,
}

impl Store {
    /// Creates a new `Store` instance, loading the configuration.
    pub async fn new(bot_tx: mpsc::Sender<BotInstruction>) -> io::Result<Self> {
        info!("Initializing new Store instance");

        let config = Config::load()?;
        let mut pairs = Pair::load_all()?;
        
        // Normalize all pair item IDs to ensure consistent lookup
        // This strips "minecraft:" prefix from item names for cleaner storage/display
        // Also filters out invalid pairs (empty item names)
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
            pair.item = normalized_item.clone();
            // Insert with normalized key
            normalized_pairs.insert(normalized_item, pair);
        }
        let pairs = normalized_pairs;
        
        let users = User::load_all()?;
        
        // Orders are session-only - start fresh on each restart
        // Clear any existing orders.json file to avoid confusion
        let orders_file = std::path::Path::new("data/orders.json");
        if orders_file.exists() {
            if let Err(e) = std::fs::remove_file(orders_file) {
                warn!("Failed to clear orders.json on startup: {}", e);
            } else {
                info!("Cleared orders.json on startup (orders are session-only)");
            }
        }
        let orders = std::collections::VecDeque::new();
        
        let trades = Trade::load_all_with_limit(config.max_trades_in_memory)?;
        let mut storage = Storage::load(&config.position)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        // If storage is empty, auto-create node 0
        if storage.nodes.is_empty() {
            info!("Storage is empty, auto-creating node 0");
            let node = storage.add_node();
            // Node 0 reserved chests are already set in Node::new:
            // - Chest 0: diamond (currency storage)
            // - Chest 1: overflow (failsafe for unknown/leftover items)
            // Save the node
            if let Err(e) = node.save() {
                warn!("Failed to save auto-created node 0: {}", e);
            }
            info!("Node 0 auto-created successfully (chest 0: diamond, chest 1: overflow)");
        }

        // Load order queue from disk (persistent across restarts)
        let order_queue = match OrderQueue::load() {
            Ok(queue) => {
                if queue.len() > 0 {
                    info!("Order queue loaded from disk: {} pending orders", queue.len());
                } else {
                    info!("Order queue initialized (empty)");
                }
                queue
            }
            Err(e) => {
                warn!("Failed to load order queue, starting fresh: {}", e);
                OrderQueue::new()
            }
        };

        // Initialize rate limiter
        let rate_limiter = RateLimiter::new();
        info!("Rate limiter initialized");

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
            current_order: None,
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
        info!("Store started and listening for messages (with order queue system)");
        let mut last_save = tokio::time::Instant::now();
        let min_save_interval = tokio::time::Duration::from_secs(self.config.autosave_interval_secs);
        info!("Autosave interval: {} seconds", self.config.autosave_interval_secs);

        loop {
            // Periodic state logging for debugging stuck conditions
            if !self.order_queue.is_empty() || self.processing_order {
                debug!("[Store] Loop state: processing_order={} queue_len={}",
                       self.processing_order, self.order_queue.len());
                if let Some(ref order) = self.current_order {
                    debug!("[Store] Current order: #{} {} for {}", order.id, order.description(), order.username);
                }
            }

            // PRIORITY 1: Process queued orders first (if any and not already processing)
            // This ensures order processing runs to COMPLETION before handling new messages.
            // Previously, using tokio::select! would CANCEL order processing when messages
            // arrived, causing the oneshot channel receiver to be dropped mid-operation.
            if !self.processing_order && !self.order_queue.is_empty() {
                debug!("[Store] Starting order processing (queue_len={})", self.order_queue.len());
                self.process_next_order().await;
                info!("[Store] Order processing cycle complete, queue size: {}", self.order_queue.len());
                
                // ALWAYS save after order completion for data integrity
                // (trades, stock updates must not be lost due to crash)
                if self.dirty {
                    info!("[Store] Saving after order completion for data integrity");
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
                    info!("[Store] Received message: {:?}", std::mem::discriminant(&message));
                    // Check if this is a shutdown message before moving it
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
                            info!("[Store] CLI message handling complete");
                        }
                    }

                    // If this was a shutdown message, break from the loop after handling it
                    if is_shutdown {
                        info!("[Store] Shutdown message handled, breaking from main loop to exit");
                        break;
                    }

                    // Debounced autosave: save at most every `min_save_interval`.
                    if self.dirty && last_save.elapsed() >= min_save_interval {
                        info!("[Store] Autosave triggered (dirty flag set, {}s since last save)", last_save.elapsed().as_secs());
                        if let Err(e) = state::save(&self) {
                            error!("[Store] Autosave failed: {}", e);
                        } else {
                            info!("[Store] Autosave completed successfully");
                            last_save = tokio::time::Instant::now();
                            self.dirty = false;
                        }
                    }
                }
                None => {
                    info!("[Store] store_rx.recv() returned None - channel closed (all senders dropped)");
                    break;
                }
            }
        }

        // Exiting main loop - perform final cleanup
        info!("[Store] Exiting main loop, performing final cleanup");
        info!("[Store] Final cleanup: Saving store data one final time (safety check)");
        
        // Save one final time as a safety measure (shutdown handler already saved, but this ensures we're up to date)
        if let Err(e) = state::save(&self) {
            error!("[Store] Final cleanup: Failed to save store data: {}", e);
        } else {
            info!("[Store] Final cleanup: Store data saved successfully");
        }

        // Drop bot_tx (bot should already be shut down, but this ensures cleanup)
        info!("[Store] Final cleanup: Dropping bot_tx channel");
        drop(bot_tx);
        info!("[Store] Final cleanup: bot_tx dropped");
        
        info!("[Store] Store task shutdown complete, task exiting");
    }

    /// Process the next order from the queue
    ///
    /// This method is called by the select! loop when there are orders waiting
    /// and no order is currently being processed. It sets `processing_order = true`
    /// at the start and `false` at the end to prevent concurrent order execution.
    ///
    /// Note: The order handlers (buy, sell, deposit, withdraw) send their own
    /// messages to the player, so we only send the "Now processing" notification
    /// here and log the result for debugging purposes.
    async fn process_next_order(&mut self) {
        info!("[Store] === PROCESS_NEXT_ORDER STARTING ===");
        info!("[Store] Current state: processing_order={}, queue_len={}", 
              self.processing_order, self.order_queue.len());
        
        // Pop the next order
        let order = match self.order_queue.pop() {
            Some(o) => {
                info!("[Store] Popped order #{} from queue: {} for {}", o.id, o.description(), o.username);
                o
            }
            None => {
                warn!("[Store] Queue was empty when trying to pop (race condition?) - aborting process_next_order");
                return;
            }
        };

        info!("[Store] Setting processing_order=true, current_order=Some(#{})", order.id);
        self.processing_order = true;
        self.current_order = Some(order.clone());
        info!(
            "[Store] Processing order #{}: {} for {} (queued at {})",
            order.id,
            order.description(),
            order.username,
            order.queued_at
        );

        // Notify user that their order is being processed
        let processing_msg = format!("Now processing: {}...", order.description());
        info!("[Store] Sending 'now processing' notification to {}", order.username);
        if let Err(e) = utils::send_message_to_player(self, &order.username, &processing_msg).await {
            warn!("[Store] Failed to notify user {} of order start: {}", order.username, e);
        }

        // Execute the order (handlers send their own completion/error messages)
        info!("[Store] Calling execute_queued_order for order #{}", order.id);
        let start_time = std::time::Instant::now();
        let result = orders::execute_queued_order(self, &order).await;
        let elapsed = start_time.elapsed();
        info!("[Store] execute_queued_order returned after {:.2}s", elapsed.as_secs_f64());

        // Log result for debugging (handlers already messaged the player)
        match &result {
            Ok(success_msg) => {
                info!(
                    "[Store] Order #{} COMPLETED successfully in {:.2}s: {}",
                    order.id, elapsed.as_secs_f64(), success_msg
                );
            }
            Err(error_msg) => {
                error!(
                    "[Store] Order #{} FAILED after {:.2}s: {}",
                    order.id, elapsed.as_secs_f64(), error_msg
                );
            }
        }

        info!("[Store] Resetting processing state: processing_order=false, current_order=None");
        self.processing_order = false;
        self.current_order = None;
        self.dirty = true;
        info!("[Store] === PROCESS_NEXT_ORDER COMPLETE === (queue_len={})", self.order_queue.len());
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

    /// Apply chest sync report from bot (overwrites storage with bot-reported truth)
    pub(crate) fn apply_chest_sync(&mut self, report: ChestSyncReport) -> Result<(), String> {
        state::apply_chest_sync(self, report)
    }

    /// Get node position for a given chest_id
    pub(crate) fn get_node_position(&self, chest_id: i32) -> crate::types::Position {
        utils::get_node_position(self, chest_id)
    }
}
