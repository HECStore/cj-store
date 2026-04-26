//! # Bot - Minecraft Client I/O
//!
//! Handles all Minecraft client interactions via Azalea:
//! - Connection management (automatic reconnect with exponential backoff)
//! - Whisper parsing (extracts player commands from chat)
//! - Trade GUI automation (full `/trade` protocol implementation)
//! - Chest I/O with shulker handling (place, open, transfer, replace)
//! - Pathfinding and navigation (walks to nodes/chests)
//!
//! ## Architecture
//!
//! **Connection**: Uses Azalea's `ClientBuilder` with event handler.
//! Spawned as a local task (Azalea requires `!Send`).
//!
//! **Reconnection**: Automatic with exponential backoff (2s → 60s max).
//! Prevents concurrent connection attempts via `AtomicBool`.

pub mod chest_io;
pub mod connection;
pub mod inventory;
pub mod navigation;
pub mod shulker;
pub mod trade;

use azalea::account::Account;
use azalea::prelude::*;
use azalea::{Client, Event};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::config::ChatConfig;
use crate::messages::{
    BotInstruction, BotMessage, ChatEvent, ChatEventKind, ChestAction, ChestSyncReport,
    StoreMessage,
};
use crate::types::Position;

#[derive(Clone, Component)]
pub struct BotState {
    pub connected: bool,
    pub store_tx: Option<mpsc::Sender<StoreMessage>>,
    pub client: Arc<RwLock<Option<Client>>>,
    pub chat_tx: Arc<broadcast::Sender<String>>,
    pub connecting: Arc<AtomicBool>,
    /// Typed chat-event broadcast — separate from the legacy `chat_tx`
    /// (which trade.rs subscribes to for trade-failure detection). Chat
    /// events are published to BOTH; see PLAN §2.2 for why splitting is
    /// load-bearing.
    pub chat_events_tx: Arc<broadcast::Sender<ChatEvent>>,
    /// Mpsc to the dedicated history writer task (PLAN §2.2 ADV11). Used
    /// with `try_send`, never `await`, so a hostile flood cannot block
    /// `bot_task`.
    pub history_tx: mpsc::Sender<ChatEvent>,
    /// Live Minecraft username, populated on `Event::Init` and cleared on
    /// `Event::Disconnect`. Read-only by chat (PLAN §2.4).
    pub bot_username: Arc<RwLock<Option<String>>>,
    /// Snapshot of chat config — `chat.enabled`, `chat.dry_run`,
    /// `chat.command_prefixes`, `chat.command_typo_max_distance` are
    /// consulted by the whisper router. Held in an `Arc` so per-event
    /// reads are zero-cost.
    pub chat_config: Arc<ChatConfig>,
    /// History-drop counter for the §2.2 try_send path. Incremented when
    /// the history mpsc is full and an event is dropped. Wrapped in
    /// `parking_lot::Mutex` (vs atomic) so the future "1 warn per minute"
    /// rate-limit logic can read+update both the counter and a timestamp
    /// atomically.
    pub history_drops: Arc<parking_lot::Mutex<u64>>,
}

impl Default for BotState {
    fn default() -> Self {
        let (chat_tx, _) = broadcast::channel(256);
        let (chat_events_tx, _) = broadcast::channel(2048);
        // Default impl is used only by tests where the receivers are not
        // observed; a small mpsc keeps the channel valid without leaking.
        let (history_tx, _history_rx) = mpsc::channel(1);
        Self {
            connected: false,
            store_tx: None,
            client: Arc::new(RwLock::new(None)),
            chat_tx: Arc::new(chat_tx),
            connecting: Arc::new(AtomicBool::new(false)),
            chat_events_tx: Arc::new(chat_events_tx),
            history_tx,
            bot_username: Arc::new(RwLock::new(None)),
            chat_config: Arc::new(ChatConfig::default()),
            history_drops: Arc::new(parking_lot::Mutex::new(0)),
        }
    }
}

#[derive(Clone)]
pub struct Bot {
    pub client: Arc<RwLock<Option<Client>>>,
    pub account: Account,
    pub server_address: String,
    pub store_tx: mpsc::Sender<StoreMessage>,
    pub chat_tx: Arc<broadcast::Sender<String>>,
    pub buffer_chest_position: Option<Position>,
    pub connecting: Arc<AtomicBool>,
    pub shutdown: Arc<AtomicBool>,
    pub client_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    /// Trade GUI timeout in milliseconds (from config).
    ///
    /// Sourced from `Config::trade_timeout_ms` so operators can tune how long
    /// the bot waits for a player to accept/complete a trade without touching
    /// source. Used by `bot::trade` when waiting for the trade menu to open.
    pub trade_timeout_ms: u64,
    /// Pathfinding budget in milliseconds (from config).
    ///
    /// Sourced from `Config::pathfinding_timeout_ms`; an upper bound on how
    /// long navigation may run before giving up. Used by `bot::navigation`
    /// across retry attempts.
    pub pathfinding_timeout_ms: u64,
    /// Persistent journal of in-flight shulker operations (crash recovery).
    ///
    /// `chest_io` writes state transitions here so a subsequent process can
    /// detect — and an operator can reconcile — any operation that was
    /// mid-flight at the moment of a crash.
    pub journal: crate::store::journal::SharedJournal,
    /// Typed chat-event broadcast (PLAN §2.2). The bot publishes parsed
    /// chat lines here for chat_task to consume.
    pub chat_events_tx: Arc<broadcast::Sender<ChatEvent>>,
    /// Mpsc to the chat history writer (PLAN §2.2). `try_send` only.
    pub history_tx: mpsc::Sender<ChatEvent>,
    /// Live Minecraft username (PLAN §2.4). `None` while disconnected or
    /// pre-Init.
    pub bot_username: Arc<RwLock<Option<String>>>,
    /// Snapshot of chat config; held in `Arc` so the whisper router and
    /// chat_task share a single allocation per process.
    pub chat_config: Arc<ChatConfig>,
    /// History-drop counter (see `BotState::history_drops`).
    pub history_drops: Arc<parking_lot::Mutex<u64>>,
    /// Critical-section gate (PLAN §2.2 / §4.8 ADV4). Set while a trade
    /// or chest IO is in flight. Read-only by chat (chat task gets a
    /// clone of the same `Arc` and observes via `.load()` only).
    pub in_critical_section: Arc<AtomicBool>,
}

/// Channels and shared state passed from `main` into [`bot_task`].
///
/// Bundled into a struct rather than a long argument list so adding new
/// chat-related shared state (Phase 1+) does not balloon every call site.
pub struct BotChannels {
    pub chat_events_tx: Arc<broadcast::Sender<ChatEvent>>,
    pub history_tx: mpsc::Sender<ChatEvent>,
    pub bot_username: Arc<RwLock<Option<String>>>,
    pub chat_config: Arc<ChatConfig>,
    pub in_critical_section: Arc<AtomicBool>,
}

impl Bot {
    // Bot::new is called from exactly one place (bot_task) with a fan-out of
    // config fields + channel handles. Wrapping them in a builder would add
    // indirection without a second caller to benefit from it.
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        account_email: String,
        server_address: String,
        store_tx: mpsc::Sender<StoreMessage>,
        chat_tx: Arc<broadcast::Sender<String>>,
        buffer_chest_position: Option<Position>,
        trade_timeout_ms: u64,
        pathfinding_timeout_ms: u64,
        journal: crate::store::journal::SharedJournal,
        channels: BotChannels,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let account = Account::microsoft(&account_email).await?;

        Ok(Self {
            client: Arc::new(RwLock::new(None)),
            account,
            server_address,
            store_tx,
            chat_tx,
            buffer_chest_position,
            connecting: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(AtomicBool::new(false)),
            client_task: Arc::new(Mutex::new(None)),
            trade_timeout_ms,
            pathfinding_timeout_ms,
            journal,
            chat_events_tx: channels.chat_events_tx,
            history_tx: channels.history_tx,
            bot_username: channels.bot_username,
            chat_config: channels.chat_config,
            history_drops: Arc::new(parking_lot::Mutex::new(0)),
            in_critical_section: channels.in_critical_section,
        })
    }

    pub async fn send_chat_message(&self, message: &str) -> Result<(), String> {
        if let Some(client) = self.client.read().await.as_ref() {
            client.chat(message);
            debug!("Sent chat message: {}", message);
            Ok(())
        } else {
            warn!(
                "send_chat_message dropped: bot not connected (message={})",
                message
            );
            Err("Bot not connected".to_string())
        }
    }

    pub async fn send_whisper(&self, target: &str, message: &str) -> Result<(), String> {
        if let Some(client) = self.client.read().await.as_ref() {
            client.chat(format!("/msg {} {}", target, message));
            debug!("Sent whisper to {}: {}", target, message);
            Ok(())
        } else {
            warn!(
                "send_whisper dropped: bot not connected (target={} message={})",
                target, message
            );
            Err("Bot not connected".to_string())
        }
    }

    /// Normalize item ID by stripping "minecraft:" prefix if present.
    ///
    /// Used to canonicalize raw item names returned by the Minecraft API so
    /// they can be compared against stored `ItemId` values (which are already
    /// prefix-free). Empty input returns an empty string; callers that need a
    /// non-empty invariant should use `ItemId::new` instead.
    ///
    /// # Examples
    /// - "minecraft:diamond" -> "diamond"
    /// - "diamond" -> "diamond"
    pub fn normalize_item_id(item: &str) -> String {
        item.strip_prefix("minecraft:").unwrap_or(item).to_string()
    }

    pub fn chat_subscribe(&self) -> broadcast::Receiver<String> {
        self.chat_tx.subscribe()
    }
}

/// Main bot task that handles instructions from the Store
#[allow(clippy::too_many_arguments)]
pub async fn bot_task(
    store_tx: mpsc::Sender<StoreMessage>,
    mut bot_rx: mpsc::Receiver<BotInstruction>,
    account_email: String,
    server_address: String,
    buffer_chest_position: Option<Position>,
    trade_timeout_ms: u64,
    pathfinding_timeout_ms: u64,
    channels: BotChannels,
) {
    let (chat_tx, _chat_rx) = broadcast::channel::<String>(256);

    // Load the operation journal and surface any leftover in-flight entry.
    //
    // A leftover entry means the previous run crashed between shulker lifecycle
    // steps. We don't attempt automatic resume (that would require verifying
    // live world state, which is easy to get wrong and leaks items); instead we
    // log prominently so an operator can reconcile, then zero the file so the
    // bot can proceed with fresh operations.
    let journal = match crate::store::journal::Journal::load() {
        Ok((journal, leftover)) => {
            if let Some(entry) = leftover {
                error!(
                    "[Bot] Crash recovery: previous run left an in-flight shulker op: op_id={} type={:?} chest_id={} slot={} state={:?} — manual reconciliation recommended",
                    entry.operation_id,
                    entry.operation_type,
                    entry.chest_id,
                    entry.slot_index,
                    entry.state
                );
            }
            let shared = std::sync::Arc::new(parking_lot::Mutex::new(journal));
            if let Err(e) = shared.lock().clear_leftover() {
                warn!("[Bot] Failed to clear journal after startup warning: {}", e);
            }
            shared
        }
        Err(e) => {
            warn!(
                "[Bot] Failed to load operation journal: {} — starting with empty journal",
                e
            );
            std::sync::Arc::new(parking_lot::Mutex::new(
                crate::store::journal::Journal::default(),
            ))
        }
    };

    let bot = match Bot::new(
        account_email,
        server_address,
        store_tx.clone(),
        Arc::new(chat_tx),
        buffer_chest_position,
        trade_timeout_ms,
        pathfinding_timeout_ms,
        journal,
        channels,
    )
    .await
    {
        Ok(bot) => bot,
        Err(e) => {
            error!("Failed to create bot: {}", e);
            return;
        }
    };

    // Connect to server (best-effort; we'll retry on failures/disconnects)
    let account = bot.account.clone();
    let server_address = bot.server_address.clone();
    if let Err(e) = connection::connect(&bot, account, server_address).await {
        error!("Failed to connect bot (will retry): {}", e);
    }

    let mut tick = tokio::time::interval(tokio::time::Duration::from_secs(1));
    // Exponential backoff for reconnect attempts: starts at 2s, doubles on each failure,
    // capped at 60s. Reset to 2s on successful reconnect.
    let mut backoff = tokio::time::Duration::from_secs(2);
    let max_backoff = tokio::time::Duration::from_secs(60);
    // Initialize last_attempt in the past so the first reconnect check can fire immediately.
    let mut last_attempt = tokio::time::Instant::now() - backoff;

    // Main event loop (+ periodic reconnect checks)
    'outer: loop {
        tokio::select! {
            _ = tick.tick() => {
                // Check shutdown flag before attempting reconnect
                if bot.shutdown.load(Ordering::SeqCst) {
                    break 'outer;
                }

                let disconnected = bot.client.read().await.is_none();
                if disconnected && last_attempt.elapsed() >= backoff {
                    info!("Bot appears disconnected; attempting reconnect");
                    last_attempt = tokio::time::Instant::now();
                    let account = bot.account.clone();
                    let server_address = bot.server_address.clone();
                    if let Err(e) = connection::connect(&bot, account, server_address).await {
                        warn!("Reconnect attempt failed: {}", e);
                        // Double backoff on failure (bounded by max_backoff) to avoid hammering the server.
                        backoff = (backoff * 2).min(max_backoff);
                    } else {
                        // Poll up to init_timeout for Event::Init to populate bot.client.
                        // connect() returns as soon as the task is spawned, but the client
                        // handle is only set once the server completes the login /
                        // configuration handshake.
                        let init_timeout = tokio::time::Duration::from_secs(20);
                        let mut ok = false;
                        let start = tokio::time::Instant::now();
                        while start.elapsed() < init_timeout {
                            if bot.client.read().await.is_some() {
                                ok = true;
                                break;
                            }
                            tokio::time::sleep(tokio::time::Duration::from_millis(crate::constants::DELAY_SHORT_MS)).await;
                        }
                        if ok {
                            // Successful reconnect: reset backoff to the initial floor.
                            backoff = tokio::time::Duration::from_secs(2);
                            info!("Bot reconnected");

                            // CRITICAL: Wait for Azalea to fully initialize all entity components
                            // The Inventory component may not be immediately available after Event::Init
                            // Without this delay, accessing inventory operations can cause a panic:
                            // "Our client is missing a required component: &azalea_entity::inventory::Inventory"
                            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                        } else {
                            backoff = (backoff * 2).min(max_backoff);
                            warn!("Reconnect attempt did not initialize in time");
                        }
                    }
                }
            }
            msg = bot_rx.recv() => {
                let Some(instruction) = msg else { break 'outer; };
                match instruction {
            BotInstruction::Whisper {
                target,
                message,
                respond_to,
            } => {
                let result = bot.send_whisper(&target, &message).await;
                // Tag bot output to history so tool-time history searches
                // can attribute messages back to the bot (PLAN §2.4 C3).
                if result.is_ok()
                    && let Some(name) = bot.bot_username.read().await.as_ref()
                {
                    crate::chat::history::append_bot_output(
                        name,
                        Some(&target),
                        &message,
                        /* is_whisper */ true,
                    );
                }
                if respond_to.send(result).is_err() {
                    warn!(
                        "[Bot] Whisper response channel dropped before ack (target={})",
                        target
                    );
                }
            }
            BotInstruction::SendChat { content, respond_to } => {
                // The chat module is responsible for the §2.2
                // critical-section gate and pacing limits — by the time a
                // SendChat reaches here, those checks have already run. The
                // bot layer is a dumb wire: send what it's given, ack the
                // result.
                let result = bot.send_chat_message(&content).await;
                if result.is_ok()
                    && let Some(name) = bot.bot_username.read().await.as_ref()
                {
                    crate::chat::history::append_bot_output(
                        name,
                        None,
                        &content,
                        /* is_whisper */ false,
                    );
                }
                if respond_to.send(result).is_err() {
                    warn!("[Bot] SendChat response channel dropped before ack");
                }
            }
            BotInstruction::InteractWithChestAndSync {
                target_chest,
                node_position,
                action,
                respond_to,
            } => {
                debug!("[Bot] Chest interaction: chest={} action={:?}", target_chest.id, action);

                let op_start = std::time::Instant::now();

                let result: Result<ChestSyncReport, String> = match navigation::go_to_chest(&bot, &target_chest, &node_position).await {
                    Err(e) => {
                        error!("[Bot] Navigation to chest {} failed: {}", target_chest.id, e);
                        Err(e)
                    }
                    Ok(()) => {
                        // Perform requested IO (only supports bot inventory direction; no direct player IO here).
                        // automated_chest_io now returns counts for processed slots (-1 for unprocessed)
                        let chest_block_pos = azalea::BlockPos::new(
                            target_chest.position.x,
                            target_chest.position.y,
                            target_chest.position.z,
                        );

                        match action.clone() {
                            ChestAction::Deposit { item, amount, from_player, stack_size } => {
                                if from_player.is_some() {
                                    error!("[Bot] Deposit from player is not supported in sync mode");
                                    Err("Deposit from player is not supported in sync mode".to_string())
                                } else {
                                    // Pass existing slot counts so chest_io can skip shulkers known to be full
                                    // (fast-path optimization to avoid opening every shulker on deposit).
                                    // Guard: only forward counts if the array is fully sized (54) AND at
                                    // least one slot is non-zero. An all-zero array is ambiguous - it could
                                    // mean "never scanned yet" rather than "confirmed empty", and treating
                                    // an unscanned chest as empty would skip valid destinations.
                                    let known_arr: Option<[i32; crate::constants::DOUBLE_CHEST_SLOTS]> =
                                        if target_chest.amounts.len() == crate::constants::DOUBLE_CHEST_SLOTS
                                            && target_chest.amounts.iter().any(|&x| x > 0)
                                        {
                                            let mut arr = [-1i32; crate::constants::DOUBLE_CHEST_SLOTS];
                                            arr.copy_from_slice(&target_chest.amounts);
                                            Some(arr)
                                        } else {
                                            None
                                        };
                                    let io_start = std::time::Instant::now();
                                    let io_result = chest_io::automated_chest_io(
                                        &bot,
                                        chest_block_pos,
                                        target_chest.id,
                                        &item,
                                        amount,
                                        "deposit",
                                        &node_position,
                                        known_arr.as_ref(),
                                        stack_size,
                                    ).await;
                                    let io_elapsed = io_start.elapsed();

                                    match io_result {
                                        Ok(amounts) => {
                                            Ok(ChestSyncReport {
                                                chest_id: target_chest.id,
                                                item,
                                                amounts,
                                            })
                                        }
                                        Err(e) => {
                                            error!("[Bot] Deposit IO FAILED after {:.2}s: {}", io_elapsed.as_secs_f64(), e);
                                            Err(e)
                                        }
                                    }
                                }
                            }
                            ChestAction::Withdraw { item, amount, to_player, stack_size } => {
                                if to_player.is_some() {
                                    error!("[Bot] Withdraw to player is not supported in sync mode");
                                    Err("Withdraw to player is not supported in sync mode".to_string())
                                } else {
                                    // Pass existing slot counts so chest_io can skip shulkers known to be empty
                                    // (fast-path optimization to avoid opening empty shulkers on withdraw).
                                    // Same all-zero ambiguity guard as the deposit path: an unscanned chest
                                    // has a zero-filled amounts array which we must NOT treat as "all empty",
                                    // otherwise we'd refuse to pull from chests that actually have stock.
                                    let known_arr: Option<[i32; crate::constants::DOUBLE_CHEST_SLOTS]> =
                                        if target_chest.amounts.len() == crate::constants::DOUBLE_CHEST_SLOTS
                                            && target_chest.amounts.iter().any(|&x| x > 0)
                                        {
                                            let mut arr = [-1i32; crate::constants::DOUBLE_CHEST_SLOTS];
                                            arr.copy_from_slice(&target_chest.amounts);
                                            Some(arr)
                                        } else {
                                            None
                                        };
                                    let io_result = chest_io::automated_chest_io(
                                        &bot,
                                        chest_block_pos,
                                        target_chest.id,
                                        &item,
                                        amount,
                                        "withdraw",
                                        &node_position,
                                        known_arr.as_ref(),
                                        stack_size,
                                    ).await;

                                    match io_result {
                                        Ok(amounts) => {
                                            Ok(ChestSyncReport {
                                                chest_id: target_chest.id,
                                                item,
                                                amounts,
                                            })
                                        }
                                        Err(e) => {
                                            error!("[Bot] Withdraw IO failed: {}", e);
                                            Err(e)
                                        }
                                    }
                                }
                            }
                        }
                    }
                };

                let op_elapsed = op_start.elapsed();
                if let Err(e) = &result {
                    error!("[Bot] Chest {} failed after {:.2}s: {}", target_chest.id, op_elapsed.as_secs_f64(), e);
                }

                if respond_to.send(result).is_err() {
                    error!("[Bot] Response channel dropped for chest {}", target_chest.id);
                }
            }
            BotInstruction::TradeWithPlayer {
                target_username,
                bot_offers,
                player_offers,
                require_exact_amount,
                flexible_validation,
                respond_to,
            } => {
                info!("[Bot] Trade with {}: bot_offers={:?} player_offers={:?}", target_username, bot_offers, player_offers);

                let trade_start = std::time::Instant::now();
                let result = trade::execute_trade_with_player(
                    &bot,
                    &target_username,
                    &bot_offers,
                    &player_offers,
                    require_exact_amount,
                    flexible_validation,
                )
                .await;
                let trade_elapsed = trade_start.elapsed();

                match &result {
                    Ok(received) => {
                        info!("[Bot] Trade completed in {:.2}s, received {:?}", trade_elapsed.as_secs_f64(), received);
                    }
                    Err(e) => {
                        error!("[Bot] Trade failed after {:.2}s: {}", trade_elapsed.as_secs_f64(), e);
                    }
                }

                if respond_to.send(result).is_err() {
                    error!("[Bot] Trade response channel dropped");
                }
            }
            BotInstruction::ValidateNode {
                node_id,
                node_position,
                respond_to,
            } => {
                // Single info! is emitted inside validate_node_physically;
                // logging here too would just double every validation run.
                let result = validate_node_physically(&bot, node_id, &node_position).await;
                if respond_to.send(result).is_err() {
                    error!("[Bot] ValidateNode response channel dropped for node {}", node_id);
                }
            }
            BotInstruction::Restart => {
                info!("Restarting bot");

                // Clear shutdown flag for restart
                bot.shutdown.store(false, Ordering::SeqCst);

                // Disconnect (but don't set shutdown flag)
                if let Err(e) = connection::disconnect(&bot, false).await {
                    error!("Error during disconnect: {}", e);
                }

                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

                let account = bot.account.clone();
                let server_address = bot.server_address.clone();
                if let Err(e) = connection::connect(&bot, account, server_address).await {
                    error!("Error during reconnect: {}", e);
                }
            }
            BotInstruction::Shutdown { respond_to } => {
                info!("[Bot] Shutdown instruction received");

                // Disconnect from server (with shutdown flag)
                if let Err(e) = connection::disconnect(&bot, true).await {
                    error!("[Bot] Shutdown: Error during disconnect: {}", e);
                }

                // Additional buffer for OS-level TCP connection closure
                tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

                // Signal shutdown complete
                let _ = respond_to.send(());
                // Don't drop store_tx here - it will be dropped in final cleanup
                // Dropping it here would cause a move error since it's used again below
                break 'outer;
            }
                }
            }
        }
    }

    // Channel closed, perform final cleanup
    info!("[Bot] Channel closed, performing final cleanup");

    if let Err(e) = connection::disconnect(&bot, true).await {
        error!("[Bot] Error during final disconnect: {}", e);
    }
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    drop(bot);
    drop(store_tx);
    info!("[Bot] Bot task shutdown complete");
}

/// Physically validate a node: walk to it, open each of the 4 chests, and
/// confirm every slot holds a shulker box.
///
/// Errors from individual chests are accumulated so a single validation run
/// reports every broken chest at once, rather than forcing the operator to
/// re-run after each fix.
async fn validate_node_physically(
    bot: &Bot,
    node_id: i32,
    node_position: &Position,
) -> Result<(), String> {
    use crate::types::Node;

    info!(
        "Validating node {} at ({}, {}, {})",
        node_id, node_position.x, node_position.y, node_position.z
    );

    // Step 1: Navigate to node position
    navigation::go_to_node(bot, node_position)
        .await
        .map_err(|e| {
            format!(
                "Node {} validation failed: could not navigate to position ({}, {}, {}): {}",
                node_id, node_position.x, node_position.y, node_position.z, e
            )
        })?;

    // Step 2: Check each of the 4 chests.
    // Errors are accumulated (not early-return) so the report lists every broken chest
    // in a single validation pass, instead of forcing the operator to re-run after each fix.
    let mut validation_errors = Vec::new();

    for chest_index in 0..4 {
        // Space chest opens apart so the server finishes processing the previous close
        // packet before we issue another open (prevents "container already open" races).
        if chest_index > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(
                crate::constants::DELAY_VALIDATION_BETWEEN_CHESTS_MS,
            ))
            .await;
        }

        let chest_pos = Node::calc_chest_position(chest_index, node_position);
        let block_pos = azalea::BlockPos::new(chest_pos.x, chest_pos.y, chest_pos.z);

        debug!(
            "Validating node {} chest {} at ({}, {}, {})",
            node_id, chest_index, chest_pos.x, chest_pos.y, chest_pos.z
        );

        // Try to open the chest using fast validation (no retries, short timeout)
        // If there's no chest at this position, we fail fast instead of waiting 45+ seconds
        match chest_io::open_chest_container_for_validation(bot, block_pos).await {
            Ok(container) => {
                // Verify contents are all shulker boxes
                match container.contents() {
                    Some(contents) => {
                        // A valid storage chest is a double chest with exactly
                        // `DOUBLE_CHEST_SLOTS` slots. Single chests or any other
                        // size indicate the block at this position isn't the
                        // expected double chest.
                        if contents.len() != crate::constants::DOUBLE_CHEST_SLOTS {
                            validation_errors.push(format!(
                                "Chest {} has {} slots (expected {})",
                                chest_index,
                                contents.len(),
                                crate::constants::DOUBLE_CHEST_SLOTS
                            ));
                        } else {
                            // Every slot must hold a shulker box - empty slots and non-shulker
                            // items both break the storage invariant this node relies on.
                            let mut non_shulker_slots = Vec::new();
                            for (slot_idx, stack) in contents.iter().enumerate() {
                                if stack.count() <= 0 {
                                    non_shulker_slots.push(format!("slot {} empty", slot_idx));
                                } else {
                                    let item_id = stack.kind().to_string();
                                    if !shulker::is_shulker_box(&item_id) {
                                        non_shulker_slots.push(format!(
                                            "slot {} has {} (not shulker)",
                                            slot_idx, item_id
                                        ));
                                    }
                                }
                            }

                            if !non_shulker_slots.is_empty() {
                                // Cap the per-chest error detail at 5 slots to keep error
                                // messages readable when a whole chest is misconfigured.
                                let issues = if non_shulker_slots.len() > 5 {
                                    format!(
                                        "{} slots missing shulkers (first 5: {})",
                                        non_shulker_slots.len(),
                                        non_shulker_slots
                                            .iter()
                                            .take(5)
                                            .cloned()
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    )
                                } else {
                                    non_shulker_slots.join(", ")
                                };
                                validation_errors
                                    .push(format!("Chest {}: {}", chest_index, issues));
                            }
                        }
                    }
                    None => {
                        validation_errors.push(format!(
                            "Chest {} opened but contents unavailable",
                            chest_index
                        ));
                    }
                }
                container.close();
                // Small delay after closing to ensure server processes it
                tokio::time::sleep(tokio::time::Duration::from_millis(
                    crate::constants::DELAY_MEDIUM_MS,
                ))
                .await;
            }
            Err(e) => {
                validation_errors.push(format!(
                    "Chest {} at ({}, {}, {}): {}",
                    chest_index, chest_pos.x, chest_pos.y, chest_pos.z, e
                ));
            }
        }
    }

    if validation_errors.is_empty() {
        info!(
            "Node {} validation passed: all 4 chests exist with 54 shulker boxes each",
            node_id
        );
        Ok(())
    } else {
        let error_msg = format!(
            "Node {} validation failed ({} issue(s)):\n  - {}",
            node_id,
            validation_errors.len(),
            validation_errors.join("\n  - ")
        );
        warn!("{}", error_msg);
        Err(error_msg)
    }
}

// Azalea's event-handler slot wants an `fn(Client, Event, State) -> impl Future<Output = ...>`
// with owned `State`, but our real handler takes `&mut BotState` so it can mutate in place.
// This thin wrapper bridges the two signatures.
pub(crate) async fn handle_event_fn(
    client: Client,
    event: Event,
    mut state: BotState,
) -> anyhow::Result<()> {
    handle_event(client, event, &mut state).await
}

async fn handle_event(client: Client, event: Event, state: &mut BotState) -> anyhow::Result<()> {
    match event {
        Event::Init => {
            info!("Bot connected and initialized!");
            state.connected = true;
            *state.client.write().await = Some(client.clone());
            state.connecting.store(false, Ordering::SeqCst);
            // PLAN §2.4: populate bot_username from the Mojang account
            // profile once login completes. The chat module observes this
            // and refuses to compose until it is `Some(_)`.
            let username = client.profile().name.clone();
            *state.bot_username.write().await = Some(username.clone());
            debug!("[Event] bot_username populated: {}", username);
        }
        Event::Chat(m) => {
            let message_text = m.message().to_string();
            tracing::debug!("Chat message received: {}", message_text);

            // Parse once. `parse_chat_line` distinguishes whisper vs public
            // chat and strips the prefix. Both branches publish to the
            // legacy `chat_tx` (trade-failure detection) AND to the typed
            // chat-events channel + history mpsc.
            let parsed = parse_chat_line(&m, &message_text);

            // Log whispers at info level (existing behavior).
            if let Some(ref p) = parsed
                && p.kind == ChatEventKind::Whisper
            {
                info!("Received whisper from {}: {}", p.sender, p.content);
            }

            // Step 1: legacy chat_tx publish FIRST — trade-failure
            // detection in trade.rs is latency-sensitive (PLAN §2.2).
            let _ = state.chat_tx.send(message_text);

            // Step 2: history try_send (durable logging path, PLAN §2.2 ADV11).
            if let Some(ref p) = parsed {
                let event = ChatEvent {
                    kind: p.kind,
                    sender: p.sender.clone(),
                    content: p.content.clone(),
                    recv_at: std::time::SystemTime::now(),
                };
                if let Err(e) = state.history_tx.try_send(event.clone()) {
                    // Channel full or closed — increment drop counter and
                    // emit at most one warn per 60 s to bound log volume
                    // under sustained flooding.
                    let mut drops = state.history_drops.lock();
                    *drops += 1;
                    let count = *drops;
                    drop(drops);
                    if count == 1 || count.is_multiple_of(60) {
                        warn!(
                            history_drops = count,
                            error = ?e,
                            "[Event] history mpsc try_send failed; durable history degraded"
                        );
                    }
                }

                // Step 3: typed broadcast for chat-decision pipeline.
                // `send` failure here means no receiver yet — that's fine
                // (chat task disabled or not yet subscribed); the failure
                // is silent on purpose.
                let _ = state.chat_events_tx.send(event);
            }

            // Step 4: whisper router. If chat is disabled or this is a
            // command-shaped whisper, forward to Store; otherwise the
            // chat module owns the response (and we don't pipe to Store
            // to avoid the §2.3 "Unknown command" double-reply).
            if let Some(p) = parsed
                && p.kind == ChatEventKind::Whisper
                && let Some(store_tx) = &state.store_tx
            {
                let route = crate::chat::conversation::route_whisper(
                    &p.content,
                    state.chat_config.enabled,
                    state.chat_config.dry_run,
                    &state.chat_config.command_prefixes,
                    state.chat_config.command_typo_max_distance,
                );
                use crate::chat::conversation::WhisperRoute;
                match route {
                    WhisperRoute::Store => {
                        let bot_message = BotMessage::PlayerCommand {
                            player_name: p.sender.clone(),
                            command: p.content.clone(),
                        };
                        if let Err(e) = store_tx
                            .send(StoreMessage::FromBot(bot_message))
                            .await
                        {
                            error!(
                                "Failed to forward player command to store (sender={} command={}): {}",
                                p.sender, p.content, e
                            );
                        }
                    }
                    WhisperRoute::Chat => {
                        // Already published to chat_events_tx above.
                        debug!(
                            "[Event] whisper from {} routed to chat module",
                            p.sender
                        );
                    }
                    WhisperRoute::Drop => {
                        debug!(
                            "[Event] whisper from {} dropped (empty/sigil-only/<2 chars)",
                            p.sender
                        );
                    }
                }
            }
        }
        Event::Disconnect(reason) => {
            warn!(
                "[Event] Bot disconnected from server - reason: {:?}",
                reason
            );
            let disconnect_time = std::time::Instant::now();
            state.connected = false;
            *state.client.write().await = None;
            // PLAN §2.4: clear bot_username so chat refuses to compose under
            // a stale identity during the reconnect window.
            *state.bot_username.write().await = None;
            state.connecting.store(false, Ordering::SeqCst);
            info!("[Event] Disconnect event processed - client cleared, flags updated");
            debug!(
                "[Event] Disconnect processing took: {:?}",
                disconnect_time.elapsed()
            );
        }
        _ => {}
    }
    Ok(())
}

/// Parsed chat line — either a whisper (DM) or a public chat message.
#[derive(Debug, Clone)]
struct ParsedChat {
    kind: ChatEventKind,
    sender: String,
    /// Already stripped of "X whispers:" / chat prefix.
    content: String,
}

/// Parse a single Azalea `ChatPacket` into a [`ParsedChat`]. Returns
/// `None` for messages with no recognizable sender — typically
/// system/server lines that aren't player chat.
fn parse_chat_line(message: &azalea::client_chat::ChatPacket, message_text: &str) -> Option<ParsedChat> {
    let sender = message.sender()?;

    // Whisper detection: look for "whispers:" anywhere in the line. This
    // is server-format-specific but matches the existing detection in
    // `handle_chat_message` (the previous implementation).
    if let Some(pos) = message_text.find("whispers:") {
        let content = message_text[pos + "whispers:".len()..].trim().to_string();
        if content.is_empty() {
            return None;
        }
        return Some(ParsedChat {
            kind: ChatEventKind::Whisper,
            sender,
            content,
        });
    }

    // Public chat: best-effort extraction of "<sender> content". If the
    // server format doesn't include the angle-bracketed prefix, fall back
    // to using the raw message text. The chat module is robust to either.
    let content = if let Some(rest) = message_text.split_once('>').map(|(_, r)| r.trim()) {
        if rest.is_empty() {
            message_text.to_string()
        } else {
            rest.to_string()
        }
    } else {
        message_text.to_string()
    };

    Some(ParsedChat {
        kind: ChatEventKind::Public,
        sender,
        content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn normalize_item_id_strips_minecraft_prefix() {
        assert_eq!(Bot::normalize_item_id("minecraft:diamond"), "diamond");
    }

    #[test]
    fn normalize_item_id_passes_unprefixed_through() {
        assert_eq!(Bot::normalize_item_id("diamond"), "diamond");
    }

    #[test]
    fn normalize_item_id_only_strips_leading_occurrence() {
        // The prefix is only stripped once at the start — a middle-of-string
        // "minecraft:" must be preserved verbatim, otherwise we'd mangle tags
        // or namespaced NBT data that happen to embed the literal substring.
        assert_eq!(
            Bot::normalize_item_id("foo_minecraft:diamond"),
            "foo_minecraft:diamond"
        );
    }

    #[test]
    fn normalize_item_id_empty_input_returns_empty() {
        // Empty input is explicitly allowed: normalize_item_id does not enforce
        // a non-empty invariant (callers that need one use ItemId::new instead).
        assert_eq!(Bot::normalize_item_id(""), "");
    }

    #[test]
    fn normalize_item_id_bare_prefix_returns_empty() {
        assert_eq!(Bot::normalize_item_id("minecraft:"), "");
    }

    #[test]
    fn bot_state_default_starts_disconnected_and_idle() {
        let state = BotState::default();
        assert!(!state.connected, "new BotState must not claim to be connected");
        assert!(state.store_tx.is_none(), "new BotState must have no store channel");
        assert!(
            !state.connecting.load(Ordering::SeqCst),
            "new BotState must not claim a connect is in flight"
        );
        // Client handle is present but holds no Client yet.
        let guard = state.client.try_read().expect("client RwLock should not be poisoned");
        assert!(guard.is_none(), "new BotState must have no attached Client");
    }

    #[test]
    fn bot_state_default_chat_channel_is_live() {
        // Subscribers attached to a fresh BotState must receive broadcasts;
        // this guards against regressions that would silently drop the sender
        // (e.g. if someone switched the channel capacity to 0 or forgot to
        // store the Arc).
        let state = BotState::default();
        let mut rx = state.chat_tx.subscribe();
        state.chat_tx.send("hello".to_string()).expect("send");
        let got = rx.try_recv().expect("receiver should have a message buffered");
        assert_eq!(got, "hello");
    }
}
