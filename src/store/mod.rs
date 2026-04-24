//! # Store - Authoritative State Management
//!
//! The Store is the **single source of truth** for all store state:
//! - Users (balances, operator status)
//! - Trading pairs (item/currency reserves)
//! - Orders (audit log)
//! - Trades (execution history)
//! - Storage (nodes, chests, shulker contents)

pub mod command;
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

use std::collections::{HashMap, HashSet, VecDeque};
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
    /// Per-user dirty set: UUIDs whose balance/operator changed since the
    /// last successful save. `state::save` writes only these via
    /// `User::save_dirty`, avoiding O(N) fsyncs per trade when one order
    /// touches a single player. Mirrors the semantics of `self.dirty` but
    /// at user granularity. Cleared on successful save.
    pub(crate) dirty_users: HashSet<String>,

    /// Channel to send instructions to the bot
    pub(crate) bot_tx: mpsc::Sender<BotInstruction>,

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

        // Normalize at load time (not lookup time) so the in-memory HashMap key,
        // the Pair.item field, and the on-disk filename all agree on the same
        // canonical form. Without this, "minecraft:diamond" and "diamond" would
        // be treated as distinct pairs. Invalid / empty entries are dropped here
        // rather than being allowed to poison later lookups.
        let mut normalized_pairs = HashMap::new();
        let mut needs_save = false;
        for (old_key, mut pair) in pairs.drain() {
            if pair.item.trim().is_empty() {
                warn!("Skipping pair with empty item name (file key: {})", old_key);
                needs_save = true;
                continue;
            }
            let item_id = match ItemId::new(&pair.item) {
                Ok(id) => id,
                Err(_) => {
                    warn!("Skipping pair with invalid item name '{}' (normalized to empty)", pair.item);
                    needs_save = true;
                    continue;
                }
            };
            let normalized_item = item_id.to_string();
            if old_key != normalized_item {
                warn!("Normalizing pair item name from '{}' to '{}'", old_key, normalized_item);
                needs_save = true;
            }
            pair.item = item_id;
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
        let orders_file = std::path::Path::new(crate::types::order::ORDERS_FILE);
        if orders_file.exists() {
            let count = std::fs::read_to_string(orders_file)
                .ok()
                .and_then(|s| serde_json::from_str::<std::collections::VecDeque<Order>>(&s).ok())
                .map(|v| v.len())
                .unwrap_or(0);
            let age_secs = std::fs::metadata(orders_file)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map(|d| d.as_secs());
            match age_secs {
                Some(secs) => warn!(
                    "Clearing {} pending order(s) from previous session (file last modified {}s ago)",
                    count, secs
                ),
                None => warn!(
                    "Clearing {} pending order(s) from previous session",
                    count
                ),
            }
            if let Err(e) = std::fs::remove_file(orders_file) {
                warn!("Failed to clear orders.json on startup: {}", e);
            }
        }
        let orders = std::collections::VecDeque::new();
        
        let trades = Trade::load_all_with_limit(config.max_trades_in_memory)?;
        let mut storage = Storage::load(&config.position)
            .map_err(|e| io::Error::other(e.to_string()))?;

        if storage.nodes.is_empty() {
            info!("Storage empty, auto-creating node 0");
            let node = storage.add_node();
            if let Err(e) = node.save() {
                warn!("Failed to save auto-created node 0: {}", e);
            }
        }

        // Load order queue from disk (persistent across restarts).
        // On corruption, `OrderQueue::load` renames the bad file to a
        // `.corrupt-<stamp>` sidecar so the raw bytes survive for forensic
        // recovery before the next `save()` overwrites it.
        let order_queue = match OrderQueue::load() {
            Ok(queue) => queue,
            Err(e) => {
                error!(
                    "PENDING ORDERS LOST: failed to load order queue, starting fresh: {}",
                    e
                );
                OrderQueue::new()
            }
        };

        let rate_limiter = RateLimiter::new();

        // Detect a trade that was in flight when the previous process exited.
        // We surface the incident loudly and clear the file; automatic
        // recovery (rollback/re-queue) is deliberately out of scope here
        // because it would need to touch physical chests and trade state,
        // which must be done with the operator in the loop.
        match trade_state::load_persisted() {
            Ok(Some(state)) => {
                tracing::error!(
                    "Found interrupted trade on startup: {}. The previous session crashed mid-trade - \
                     operator should inspect in-world state (bot inventory, chests, player) before resuming.",
                    state
                );
                let _ = trade_state::clear_persisted();
            }
            Ok(None) => {}
            Err(e) => warn!("Failed to load persisted trade state: {}", e),
        }

        info!(
            "Store initialized successfully with {} pairs, {} users, {} orders, {} trades, {} nodes",
            pairs.len(),
            users.len(),
            orders.len(),
            trades.len(),
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
            dirty_users: HashSet::new(),
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
        let mut last_cleanup = tokio::time::Instant::now();
        let cleanup_interval = tokio::time::Duration::from_secs(crate::constants::CLEANUP_INTERVAL_SECS);
        let rate_limit_stale_after = std::time::Duration::from_secs(crate::constants::RATE_LIMIT_STALE_AFTER_SECS);
        // Re-read each iteration so hot-reload of `autosave_interval_secs`
        // takes effect without restart. See `Store::reload_config`.
        let mut min_save_interval = tokio::time::Duration::from_secs(self.config.autosave_interval_secs);

        // Each iteration either drains one order from the queue OR blocks on
        // one incoming message — never both concurrently. Orders take strict
        // priority (PRIORITY 1 below) so an in-flight trade cannot be starved
        // or interrupted by chatty players. A previous `tokio::select!` version
        // cancelled order processing mid-way and dropped oneshot receivers;
        // sequential polling is the fix — any message arriving during order
        // execution simply accumulates in the channel buffer.
        loop {
            if !self.order_queue.is_empty() || self.processing_order {
                debug!("[Store] Loop state: processing_order={} queue_len={}",
                       self.processing_order, self.order_queue.len());
                if let Some(ref trade) = self.current_trade {
                    debug!("[Store] Current trade: {}", trade);
                }
            }

            // Drop stale rate-limiter and UUID-cache entries so a long-running
            // instance doesn't accumulate HashMap entries for users who never
            // return. Run this at the top of every iteration — not just in the
            // message branch — because under sustained order load the `continue`
            // in PRIORITY 1 would otherwise starve cleanup indefinitely (the
            // exact scenario where memory pressure is highest).
            if last_cleanup.elapsed() >= cleanup_interval {
                self.rate_limiter.cleanup_stale(rate_limit_stale_after);
                utils::cleanup_uuid_cache();
                debug!("[Store] Periodic cleanup completed");
                last_cleanup = tokio::time::Instant::now();
            }

            // Idle autosave: if the loop has been sitting on `recv()` while a
            // prior order left `dirty = true`, the message-branch debounced
            // autosave never runs. The timer arm in PRIORITY 2 falls through
            // to here so a lingering dirty flag is flushed on the configured
            // cadence even with zero inbound traffic.
            if self.dirty && last_save.elapsed() >= min_save_interval {
                if let Err(e) = state::save(&self) {
                    error!("[Store] Autosave failed: {}", e);
                } else {
                    last_save = tokio::time::Instant::now();
                    self.dirty = false;
                    self.dirty_users.clear();
                }
            }

            // PRIORITY 1: drain an order if one is waiting.
            if !self.processing_order && !self.order_queue.is_empty() {
                debug!("[Store] Starting order processing (queue_len={})", self.order_queue.len());

                // Outer watchdog: inner operations have their own timeouts, but
                // a lost channel response or future-deadlock could still wedge
                // this future indefinitely. On timeout the future is dropped,
                // which leaves `processing_order = true` and `current_trade`
                // intact so the operator sees the order as stuck and can
                // recover via `ClearStuckOrder`.
                let order_watchdog = tokio::time::Duration::from_secs(
                    crate::constants::ORDER_HARD_TIMEOUT_SECS,
                );
                match tokio::time::timeout(order_watchdog, self.process_next_order()).await {
                    Ok(()) => {}
                    Err(_) => {
                        error!(
                            timeout_secs = order_watchdog.as_secs(),
                            "[Store] Order processing exceeded watchdog; order is stuck. \
                             Operator must use 'Clear stuck order' (CLI option 15) to recover."
                        );
                        // Persist the interrupted trade so a crash-restart can
                        // still see it, and so RECOVERY.md procedures apply.
                        if let Some(trade) = &self.current_trade {
                            if let Err(e) = trade_state::persist(trade) {
                                warn!("[Store] Failed to persist interrupted trade: {}", e);
                            }
                        }
                        self.dirty = true;
                    }
                }

                // Save eagerly after every order so trades and stock updates
                // cannot be lost to a crash before the next debounced autosave.
                if self.dirty {
                    if let Err(e) = state::save(&self) {
                        error!("[Store] Autosave failed: {}", e);
                    } else {
                        last_save = tokio::time::Instant::now();
                        self.dirty = false;
                        self.dirty_users.clear();
                    }
                }

                // Between orders, non-blockingly drain any pending messages so
                // operator Shutdown/ClearStuckOrder (and anything else) is not
                // starved behind a long queue. Without this, a backlog of N
                // orders forces Shutdown to wait N × order_time — up to ~64
                // minutes at the 128-order cap.
                let mut shutdown_requested = false;
                while let Ok(message) = store_rx.try_recv() {
                    if self.dispatch_message(message, &mut min_save_interval).await {
                        shutdown_requested = true;
                        break;
                    }
                }
                if shutdown_requested {
                    info!("[Store] Shutdown handled between orders, exiting main loop");
                    break;
                }

                continue;
            }

            // PRIORITY 2: block on the next incoming message, but also wake
            // on a timer so idle periods still get periodic cleanup and
            // autosave flushes a lingering `dirty = true`. Without the timer
            // arm, the loop parks indefinitely on `recv()` and "autosave every
            // Ns" degrades to "autosave at most every Ns, and only if a message
            // wakes us".
            //
            // Only the idle recv is raced against the timer — `process_next_order`
            // is deliberately never cancellable (see the comment near the top
            // of the loop about dropped oneshot receivers).
            let time_to_autosave = min_save_interval.saturating_sub(last_save.elapsed());
            let time_to_cleanup = cleanup_interval.saturating_sub(last_cleanup.elapsed());
            let wake_after = std::cmp::min(time_to_autosave, time_to_cleanup);

            let msg = tokio::select! {
                m = store_rx.recv() => m,
                _ = tokio::time::sleep(wake_after) => {
                    // Fall through to the top of the loop so the cleanup and
                    // autosave checks below get a chance to run.
                    continue;
                }
            };
            match msg {
                Some(message) => {
                    if self.dispatch_message(message, &mut min_save_interval).await {
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
                            self.dirty_users.clear();
                        }
                    }
                }
                None => {
                    info!("[Store] Channel closed, exiting");
                    break;
                }
            }
        }

        // Shutdown path: force a full user flush (not just `dirty_users`) so
        // the on-disk snapshot is guaranteed to mirror the full in-memory map
        // regardless of what the tracked dirty set happens to contain.
        self.dirty_users.extend(self.users.keys().cloned());
        if let Err(e) = state::save(&self) {
            error!("[Store] Final save failed: {}", e);
        }
        drop(bot_tx);
        info!("[Store] Shutdown complete");
    }

    /// Dispatch a single `StoreMessage`. Returns `true` if the message was a
    /// `Shutdown` and the caller should exit the main loop. Extracted so the
    /// main loop can call it from both the blocking-recv branch and the
    /// between-orders non-blocking drain branch.
    async fn dispatch_message(
        &mut self,
        message: StoreMessage,
        min_save_interval: &mut tokio::time::Duration,
    ) -> bool {
        debug!("[Store] Received message: {:?}", std::mem::discriminant(&message));
        let is_shutdown = matches!(
            &message,
            StoreMessage::FromCli(crate::messages::CliMessage::Shutdown { .. })
        );

        match message {
            StoreMessage::FromBot(bot_msg) => {
                if let Err(e) = self.handle_bot_message(bot_msg).await {
                    error!("Error handling bot message: {}", e);
                }
            }
            StoreMessage::FromCli(cli_msg) => {
                if let Err(e) = handlers::cli::handle_cli_message(self, cli_msg).await {
                    error!("[Store] Error handling CLI message: {}", e);
                }
            }
            StoreMessage::ReloadConfig(new_config) => {
                self.reload_config(new_config);
                *min_save_interval =
                    tokio::time::Duration::from_secs(self.config.autosave_interval_secs);
            }
        }

        is_shutdown
    }

    /// Pops and executes the next queued order, holding `processing_order`
    /// high for the duration so the main loop cannot start a second order in
    /// parallel. Order handlers (buy/sell/deposit/withdraw) whisper their own
    /// completion and error messages to the player; only the "Now processing"
    /// notification and lifecycle logging are emitted here.
    async fn process_next_order(&mut self) {
        let order = match self.order_queue.pop() {
            Some(o) => o,
            None => {
                // Invariant violation: the main loop only calls this when the
                // queue is non-empty. Surface loudly but stay running.
                warn!("[Store] Queue was empty when trying to pop");
                return;
            }
        };

        self.processing_order = true;
        self.current_trade = Some(trade_state::TradeState::new(order.clone()));

        let started = std::time::Instant::now();
        info!(
            order_id = order.id,
            player = %order.username,
            item = %order.item,
            quantity = order.quantity,
            "order processing started"
        );

        let processing_msg = format!("Now processing: {}...", order.description());
        if let Err(e) = utils::send_message_to_player(self, &order.username, &processing_msg).await {
            warn!(order_id = order.id, player = %order.username, error = %e, "failed to notify user of order start");
        }

        // Handlers send their own completion/error messages to the player.
        let result = orders::execute_queued_order(self, &order).await;

        let duration_ms = started.elapsed().as_millis() as u64;
        match &result {
            Ok(summary) => info!(
                order_id = order.id,
                player = %order.username,
                duration_ms,
                summary = %summary,
                "order processing completed"
            ),
            Err(error_msg) => error!(
                order_id = order.id,
                player = %order.username,
                duration_ms,
                error = %error_msg,
                "order processing failed"
            ),
        }

        self.processing_order = false;
        self.current_trade = None;
        // Trade reached a terminal state (either committed or failed with
        // rollback already run) - clear the on-disk mirror so a restart
        // doesn't re-detect this completed trade as interrupted.
        if let Err(e) = trade_state::clear_persisted() {
            warn!("[Store] Failed to clear persisted trade state: {}", e);
        }
        self.dirty = true;
    }

    /// Apply a reloaded config, updating only fields that are safe to change
    /// at runtime. Fields that are cached in other tasks at startup (bot-side
    /// timeouts, identity/world fields) cannot take effect without a restart;
    /// changing them logs a warning and the in-memory config keeps the old
    /// value so behavior stays consistent with what the rest of the system
    /// sees.
    ///
    /// Hot-reloadable:
    /// - `fee` — next priced order uses the new rate.
    /// - `autosave_interval_secs` — next loop iteration uses the new debounce.
    ///
    /// Restart-required (warns on change):
    /// - `trade_timeout_ms`, `pathfinding_timeout_ms` — cached in bot task.
    /// - `position`, `buffer_chest_position` — world topology.
    /// - `account_email`, `server_address` — identity / connection.
    /// - `max_orders`, `max_trades_in_memory` — capacity bounds set at load.
    pub(crate) fn reload_config(&mut self, new: Config) {
        let mut applied = Vec::new();

        if (self.config.fee - new.fee).abs() > f64::EPSILON {
            applied.push(format!("fee {} -> {}", self.config.fee, new.fee));
            self.config.fee = new.fee;
        }
        if self.config.autosave_interval_secs != new.autosave_interval_secs {
            applied.push(format!(
                "autosave_interval_secs {} -> {}",
                self.config.autosave_interval_secs, new.autosave_interval_secs
            ));
            self.config.autosave_interval_secs = new.autosave_interval_secs;
        }

        // Warn on restart-only fields that were edited.
        if self.config.trade_timeout_ms != new.trade_timeout_ms {
            warn!("Config field 'trade_timeout_ms' changed but requires restart");
        }
        if self.config.pathfinding_timeout_ms != new.pathfinding_timeout_ms {
            warn!("Config field 'pathfinding_timeout_ms' changed but requires restart");
        }
        if self.config.position != new.position {
            warn!("Config field 'position' changed but requires restart");
        }
        if self.config.buffer_chest_position != new.buffer_chest_position {
            warn!("Config field 'buffer_chest_position' changed but requires restart");
        }
        if self.config.account_email != new.account_email {
            warn!("Config field 'account_email' changed but requires restart");
        }
        if self.config.server_address != new.server_address {
            warn!("Config field 'server_address' changed but requires restart");
        }
        if self.config.max_orders != new.max_orders {
            warn!("Config field 'max_orders' changed but requires restart");
        }
        if self.config.max_trades_in_memory != new.max_trades_in_memory {
            warn!("Config field 'max_trades_in_memory' changed but requires restart");
        }

        if applied.is_empty() {
            debug!("[Store] Config reload: no hot-reloadable fields changed");
        } else {
            info!("[Store] Config reloaded: {}", applied.join(", "));
        }
    }

    /// Advance the in-flight trade through the state machine.
    ///
    /// Takes a closure that receives the current `TradeState` by value and
    /// returns the next state.  If no trade is active the call is a no-op
    /// (logged at debug level).
    pub(crate) fn advance_trade(&mut self, transition: impl FnOnce(trade_state::TradeState) -> trade_state::TradeState) {
        if let Some(state) = self.current_trade.take() {
            let next = transition(state);
            let order = next.order();
            info!(
                order_id = order.id,
                player = %order.username,
                phase = next.phase(),
                "trade state advanced"
            );
            // Mirror the new phase to disk so a crash between here and the
            // next transition leaves enough information on disk for the
            // operator to detect and investigate on restart.
            if let Err(e) = trade_state::persist(&next) {
                warn!("[Store] Failed to persist trade state: {}", e);
            }
            self.current_trade = Some(next);
        } else {
            debug!("[Store] advance_trade called with no active trade (no-op)");
        }
    }

    /// Handle messages from the bot
    async fn handle_bot_message(&mut self, message: BotMessage) -> Result<(), crate::error::StoreError> {
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

    /// Look up a pair or return a structured `UnknownPair` error.
    ///
    /// Use at call sites where the pair is expected to exist because earlier
    /// code validated it; replaces panic-prone `store.pairs.get(item).unwrap()`.
    pub(crate) fn expect_pair(&self, item: &str, context: &'static str) -> Result<&crate::types::Pair, crate::error::StoreError> {
        self.pairs.get(item).ok_or_else(|| {
            tracing::error!("Invariant violation at {context}: pair '{item}' missing");
            crate::error::StoreError::UnknownPair { item: item.to_string(), context }
        })
    }

    pub(crate) fn expect_pair_mut(&mut self, item: &str, context: &'static str) -> Result<&mut crate::types::Pair, crate::error::StoreError> {
        match self.pairs.get_mut(item) {
            Some(p) => Ok(p),
            None => {
                tracing::error!("Invariant violation at {context}: pair '{item}' missing");
                Err(crate::error::StoreError::UnknownPair { item: item.to_string(), context })
            }
        }
    }

    pub(crate) fn expect_user(&self, uuid: &str, context: &'static str) -> Result<&crate::types::User, crate::error::StoreError> {
        self.users.get(uuid).ok_or_else(|| {
            tracing::error!("Invariant violation at {context}: user '{uuid}' missing");
            crate::error::StoreError::UnknownUser { uuid: uuid.to_string(), context }
        })
    }

    pub(crate) fn expect_user_mut(&mut self, uuid: &str, context: &'static str) -> Result<&mut crate::types::User, crate::error::StoreError> {
        match self.users.get_mut(uuid) {
            Some(u) => Ok(u),
            None => {
                tracing::error!("Invariant violation at {context}: user '{uuid}' missing");
                Err(crate::error::StoreError::UnknownUser { uuid: uuid.to_string(), context })
            }
        }
    }

    /// Apply chest sync report from bot (merges bot-reported slot counts into storage)
    pub(crate) fn apply_chest_sync(&mut self, report: ChestSyncReport) -> Result<(), crate::error::StoreError> {
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
            dirty_users: HashSet::new(),
            bot_tx,
            order_queue: queue::OrderQueue::new(),
            rate_limiter: RateLimiter::new(),
            processing_order: false,
            current_trade: None,
        }
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for pure-in-memory Store helpers. The async actor loop
    //! (`Store::run`, `process_next_order`) is deliberately not tested here;
    //! it is covered end-to-end via the handler tests in `handlers/` and
    //! `orders.rs` that drive the same code paths with mock bot channels.

    use super::*;
    use crate::config::Config;
    use crate::types::{Pair, Position, Storage, User};

    fn test_config() -> Config {
        Config {
            position: Position { x: 0, y: 64, z: 0 },
            fee: 0.125,
            account_email: String::new(),
            server_address: "test".to_string(),
            buffer_chest_position: None,
            trade_timeout_ms: 5_000,
            pathfinding_timeout_ms: 5_000,
            max_orders: 1000,
            max_trades_in_memory: 1000,
            autosave_interval_secs: 10,
        }
    }

    fn make_store(pairs: HashMap<String, Pair>, users: HashMap<String, User>) -> Store {
        let (tx, _rx) = mpsc::channel::<BotInstruction>(16);
        Store::new_for_test(tx, test_config(), pairs, users, Storage::default())
    }

    // ---------- new_for_test ----------

    #[test]
    fn new_for_test_initializes_idle_store_with_supplied_collections() {
        let mut pairs = HashMap::new();
        pairs.insert(
            "iron_ingot".to_string(),
            Pair {
                item: ItemId::from_normalized("iron_ingot".to_string()),
                stack_size: 64,
                item_stock: 42,
                currency_stock: 3.5,
            },
        );
        let mut users = HashMap::new();
        users.insert(
            "u1".to_string(),
            User { uuid: "u1".to_string(), username: "alice".to_string(), balance: 10.0, operator: false },
        );

        let store = make_store(pairs, users);

        assert_eq!(store.pairs.len(), 1);
        assert_eq!(store.users.len(), 1);
        assert!(store.orders.is_empty());
        assert!(store.trades.is_empty());
        assert!(!store.dirty);
        assert!(!store.processing_order);
        assert!(store.current_trade.is_none());
        assert!(store.order_queue.is_empty());
    }

    // ---------- expect_pair / expect_user ----------

    #[test]
    fn expect_pair_returns_ref_when_present() {
        let mut pairs = HashMap::new();
        pairs.insert(
            "iron_ingot".to_string(),
            Pair {
                item: ItemId::from_normalized("iron_ingot".to_string()),
                stack_size: 64,
                item_stock: 7,
                currency_stock: 1.0,
            },
        );
        let store = make_store(pairs, HashMap::new());
        let p = store.expect_pair("iron_ingot", "test").expect("pair should exist");
        assert_eq!(p.item_stock, 7);
    }

    #[test]
    fn expect_pair_missing_returns_unknown_pair_error_with_context() {
        let store = make_store(HashMap::new(), HashMap::new());
        let err = store.expect_pair("diamond", "test_ctx").unwrap_err();
        match err {
            crate::error::StoreError::UnknownPair { item, context } => {
                assert_eq!(item, "diamond");
                assert_eq!(context, "test_ctx");
            }
            other => panic!("expected UnknownPair, got: {other:?}"),
        }
    }

    #[test]
    fn expect_pair_mut_missing_returns_unknown_pair_error_with_context() {
        let mut store = make_store(HashMap::new(), HashMap::new());
        let err = store.expect_pair_mut("diamond", "ctx_mut").unwrap_err();
        assert!(matches!(
            err,
            crate::error::StoreError::UnknownPair { ref item, context }
                if item == "diamond" && context == "ctx_mut"
        ));
    }

    #[test]
    fn expect_user_missing_returns_unknown_user_error_with_context() {
        let store = make_store(HashMap::new(), HashMap::new());
        let err = store.expect_user("uuid-1", "lookup").unwrap_err();
        match err {
            crate::error::StoreError::UnknownUser { uuid, context } => {
                assert_eq!(uuid, "uuid-1");
                assert_eq!(context, "lookup");
            }
            other => panic!("expected UnknownUser, got: {other:?}"),
        }
    }

    #[test]
    fn expect_user_mut_present_returns_mutable_ref() {
        let mut users = HashMap::new();
        users.insert(
            "u1".to_string(),
            User { uuid: "u1".to_string(), username: "alice".to_string(), balance: 5.0, operator: false },
        );
        let mut store = make_store(HashMap::new(), users);
        let u = store.expect_user_mut("u1", "test").expect("user should exist");
        u.balance = 99.0;
        assert_eq!(store.users.get("u1").unwrap().balance, 99.0);
    }

    // ---------- reload_config ----------

    #[test]
    fn reload_config_hot_applies_fee_change() {
        let mut store = make_store(HashMap::new(), HashMap::new());
        let mut new_cfg = test_config();
        new_cfg.fee = 0.25;
        store.reload_config(new_cfg);
        assert!((store.config.fee - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn reload_config_hot_applies_autosave_interval_change() {
        let mut store = make_store(HashMap::new(), HashMap::new());
        let mut new_cfg = test_config();
        new_cfg.autosave_interval_secs = 60;
        store.reload_config(new_cfg);
        assert_eq!(store.config.autosave_interval_secs, 60);
    }

    #[test]
    fn reload_config_leaves_restart_only_fields_unchanged_in_memory() {
        // Editing trade_timeout_ms / server_address at runtime must warn and
        // keep the prior in-memory value so behavior stays consistent with
        // other tasks that cached the config at startup.
        let mut store = make_store(HashMap::new(), HashMap::new());
        let original_timeout = store.config.trade_timeout_ms;
        let original_addr = store.config.server_address.clone();

        let mut new_cfg = test_config();
        new_cfg.trade_timeout_ms = original_timeout + 1_000;
        new_cfg.server_address = "elsewhere.example".to_string();
        store.reload_config(new_cfg);

        assert_eq!(store.config.trade_timeout_ms, original_timeout);
        assert_eq!(store.config.server_address, original_addr);
    }

    #[test]
    fn reload_config_noop_when_nothing_hot_reloadable_changed() {
        let mut store = make_store(HashMap::new(), HashMap::new());
        let before_fee = store.config.fee;
        let before_interval = store.config.autosave_interval_secs;
        store.reload_config(test_config());
        assert!((store.config.fee - before_fee).abs() < f64::EPSILON);
        assert_eq!(store.config.autosave_interval_secs, before_interval);
    }
}
