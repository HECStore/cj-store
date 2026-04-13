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

pub mod connection;
pub mod navigation;
pub mod trade;
pub mod chest_io;
pub mod shulker;
pub mod inventory;

use azalea::prelude::*;
use azalea::{Client, Event};
use azalea::account::Account;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, mpsc, Mutex};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::messages::{BotInstruction, BotMessage, ChestAction, ChestSyncReport, StoreMessage};
use crate::types::Position;

#[derive(Clone, Component)]
pub struct BotState {
    pub connected: bool,
    pub store_tx: Option<mpsc::Sender<StoreMessage>>,
    pub client: Arc<RwLock<Option<Client>>>,
    pub chat_tx: Arc<broadcast::Sender<String>>,
    pub connecting: Arc<AtomicBool>,
}

impl Default for BotState {
    fn default() -> Self {
        let (chat_tx, _chat_rx) = broadcast::channel(256);
        Self {
            connected: false,
            store_tx: None,
            client: Arc::new(RwLock::new(None)),
            chat_tx: Arc::new(chat_tx),
            connecting: Arc::new(AtomicBool::new(false)),
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
}

impl Bot {
    pub async fn new(
        account_email: String,
        server_address: String,
        store_tx: mpsc::Sender<StoreMessage>,
        chat_tx: Arc<broadcast::Sender<String>>,
        buffer_chest_position: Option<Position>,
        trade_timeout_ms: u64,
        pathfinding_timeout_ms: u64,
        journal: crate::store::journal::SharedJournal,
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
        })
    }

    pub async fn send_chat_message(&self, message: &str) -> Result<(), String> {
        if let Some(client) = self.client.read().await.as_ref() {
            client.chat(message);
            debug!("Sent chat message: {}", message);
            Ok(())
        } else {
            Err("Bot not connected".to_string())
        }
    }

    pub async fn send_whisper(&self, target: &str, message: &str) -> Result<(), String> {
        if let Some(client) = self.client.read().await.as_ref() {
            client.chat(&format!("/msg {} {}", target, message));
            debug!("Sent whisper to {}: {}", target, message);
            Ok(())
        } else {
            Err("Bot not connected".to_string())
        }
    }

    /// Normalize item ID by stripping "minecraft:" prefix if present.
    /// 
    /// This is a wrapper around `crate::store::utils::normalize_item_id()` for
    /// convenience in bot code. Used for consistent item naming in storage.
    /// 
    /// # Examples
    /// - "minecraft:diamond" -> "diamond"
    /// - "diamond" -> "diamond"
    /// - "" -> "" (invalid, caller should validate)
    pub fn normalize_item_id(item: &str) -> String {
        crate::store::utils::normalize_item_id(item)
    }
    
    pub fn chat_subscribe(&self) -> broadcast::Receiver<String> {
        self.chat_tx.subscribe()
    }
}

/// Main bot task that handles instructions from the Store
pub async fn bot_task(
    store_tx: mpsc::Sender<StoreMessage>,
    mut bot_rx: mpsc::Receiver<BotInstruction>,
    account_email: String,
    server_address: String,
    buffer_chest_position: Option<Position>,
    trade_timeout_ms: u64,
    pathfinding_timeout_ms: u64,
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
            let shared = std::sync::Arc::new(std::sync::Mutex::new(journal));
            if let Err(e) = shared.lock().unwrap().clear_leftover() {
                warn!("[Bot] Failed to clear journal after startup warning: {}", e);
            }
            shared
        }
        Err(e) => {
            warn!("[Bot] Failed to load operation journal: {} — starting with empty journal", e);
            std::sync::Arc::new(std::sync::Mutex::new(crate::store::journal::Journal::default()))
        }
    };

    // Create bot instance using config values
    let bot = match Bot::new(
        account_email,
        server_address,
        store_tx.clone(),
        Arc::new(chat_tx),
        buffer_chest_position,
        trade_timeout_ms,
        pathfinding_timeout_ms,
        journal,
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
                        // Poll up to 20s for Event::Init to populate bot.client. connect() returns
                        // as soon as the task is spawned, but the client handle is only set once
                        // the server completes the login/configuration handshake.
                        let mut ok = false;
                        let start = tokio::time::Instant::now();
                        while start.elapsed() < tokio::time::Duration::from_secs(20) {
                            if bot.client.read().await.is_some() {
                                ok = true;
                                break;
                            }
                            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
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
                let _ = respond_to.send(result);
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
                                    let known_counts = if target_chest.amounts.len() == 54
                                        && target_chest.amounts.iter().any(|&x| x > 0) {
                                        Some(&target_chest.amounts)
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
                                        known_counts,
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
                                    let known_counts = if target_chest.amounts.len() == 54
                                        && target_chest.amounts.iter().any(|&x| x > 0) {
                                        Some(&target_chest.amounts)
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
                                        known_counts,
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
                info!("Validating node {} at position ({}, {}, {})", 
                      node_id, node_position.x, node_position.y, node_position.z);
                let result = validate_node_physically(&bot, node_id, &node_position).await;
                let _ = respond_to.send(result);
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

/// Physically validate a node by checking that all 4 chests exist and contain shulker boxes.
///
/// # Validation Steps
/// 1. Navigate to the node position
/// 2. For each of the 4 chests:
///    a. Calculate chest position from node position and chest index
///    b. Attempt to open the chest
///    c. Verify all 54 slots contain shulker boxes
/// 3. Return Ok if all checks pass, Err with detailed failure info otherwise
///
/// # Arguments
/// * `bot` - Bot instance
/// * `node_id` - Node ID being validated (for error messages)
/// * `node_position` - Calculated position where bot should stand for this node
async fn validate_node_physically(
    bot: &Bot,
    node_id: i32,
    node_position: &Position,
) -> Result<(), String> {
    use crate::types::Node;
    
    info!("Validating node {} at ({}, {}, {})",
          node_id, node_position.x, node_position.y, node_position.z);
    
    // Step 1: Navigate to node position
    navigation::go_to_node(bot, node_position).await.map_err(|e| {
        format!("Node {} validation failed: could not navigate to position ({}, {}, {}): {}",
                node_id, node_position.x, node_position.y, node_position.z, e)
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
                crate::constants::DELAY_VALIDATION_BETWEEN_CHESTS_MS
            )).await;
        }
        
        let chest_pos = Node::calc_chest_position(node_id, chest_index, node_position);
        let block_pos = azalea::BlockPos::new(chest_pos.x, chest_pos.y, chest_pos.z);
        
        debug!("Validating chest {} at ({}, {}, {})",
               chest_index, chest_pos.x, chest_pos.y, chest_pos.z);
        
        // Try to open the chest using fast validation (no retries, short timeout)
        // If there's no chest at this position, we fail fast instead of waiting 45+ seconds
        match chest_io::open_chest_container_for_validation(bot, block_pos).await {
            Ok(container) => {
                // Verify contents are all shulker boxes
                match container.contents() {
                    Some(contents) => {
                        // A valid storage chest is a double chest with exactly 54 slots.
                        // Single chests (27) or any other size indicate the block at this
                        // position isn't the expected double chest.
                        if contents.len() != 54 {
                            validation_errors.push(format!(
                                "Chest {} has {} slots (expected 54)",
                                chest_index, contents.len()
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
                                        non_shulker_slots.push(format!("slot {} has {} (not shulker)", slot_idx, item_id));
                                    }
                                }
                            }

                            if !non_shulker_slots.is_empty() {
                                // Cap the per-chest error detail at 5 slots to keep error
                                // messages readable when a whole chest is misconfigured.
                                let issues = if non_shulker_slots.len() > 5 {
                                    format!("{} slots missing shulkers (first 5: {})", 
                                            non_shulker_slots.len(),
                                            non_shulker_slots.iter().take(5).cloned().collect::<Vec<_>>().join(", "))
                                } else {
                                    non_shulker_slots.join(", ")
                                };
                                validation_errors.push(format!("Chest {}: {}", chest_index, issues));
                            }
                        }
                    }
                    None => {
                        validation_errors.push(format!("Chest {} opened but contents unavailable", chest_index));
                    }
                }
                container.close();
                // Small delay after closing to ensure server processes it
                tokio::time::sleep(tokio::time::Duration::from_millis(
                    crate::constants::DELAY_MEDIUM_MS
                )).await;
            }
            Err(e) => {
                validation_errors.push(format!("Chest {} at ({}, {}, {}): {}",
                    chest_index, chest_pos.x, chest_pos.y, chest_pos.z, e));
            }
        }
    }
    
    if validation_errors.is_empty() {
        info!("Node {} validation passed: all 4 chests exist with 54 shulker boxes each", node_id);
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

// Function pointer that matches the expected signature
pub(crate) fn handle_event_fn(
    client: Client,
    event: Event,
    mut state: BotState,
) -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
    async move { handle_event(client, event, &mut state).await }
}

// Your event handler that works with the state
async fn handle_event(client: Client, event: Event, state: &mut BotState) -> anyhow::Result<()> {
    match event {
        Event::Init => {
            info!("Bot connected and initialized!");
            state.connected = true;
            *state.client.write().await = Some(client.clone());
            state.connecting.store(false, Ordering::SeqCst);
        }
        Event::Chat(m) => {
            let message_text = m.message().to_string();
            tracing::debug!("Chat message received: {}", message_text);

            // Log ALL whispers at info level, regardless of store connection
            if message_text.contains("whispers:") {
                let sender = m.sender().unwrap_or_else(|| "Unknown".to_string());
                let content = if let Some(pos) = message_text.find("whispers:") {
                    message_text[pos + 9..].trim()
                } else {
                    ""
                };
                info!("Received whisper from {}: {}", sender, content);
            }

            // Broadcast chat to the bot task for trade failure detection.
            let _ = state.chat_tx.send(message_text.clone());

            if let Some(store_tx) = &state.store_tx {
                if let Err(e) = handle_chat_message(client, m, store_tx).await {
                    error!("Error handling chat message: {}", e);
                }
            }
        }
        Event::Disconnect(reason) => {
            warn!("[Event] Bot disconnected from server - reason: {:?}", reason);
            let disconnect_time = std::time::Instant::now();
            state.connected = false;
            *state.client.write().await = None;
            state.connecting.store(false, Ordering::SeqCst);
            info!("[Event] Disconnect event processed - client cleared, flags updated");
            debug!("[Event] Disconnect processing took: {:?}", disconnect_time.elapsed());
        }
        _ => {}
    }
    Ok(())
}

async fn handle_chat_message(
    _client: Client,
    message: azalea::chat::ChatPacket,
    store_tx: &mpsc::Sender<StoreMessage>,
) -> anyhow::Result<()> {
    let msg = message.message().to_string();
    let sender = message.sender().unwrap_or_else(|| "Unknown".to_string());

    // Check if this is a whisper to our bot
    if msg.contains("whispers:") {
        // Extract the actual message content (already logged at event handler level)
        let content = if let Some(pos) = msg.find("whispers:") {
            msg[pos + 9..].trim()
        } else {
            return Ok(());
        };

        // Send the command to the store for processing
        let bot_message = BotMessage::PlayerCommand {
            player_name: sender.clone(),
            command: content.to_string(),
        };

        let store_message = StoreMessage::FromBot(bot_message);

        if let Err(e) = store_tx.send(store_message).await {
            error!("Failed to send message to store: {}", e);
        }
    } else {
        // Log other chat messages for debugging
        tracing::trace!("Public chat - {}: {}", sender, msg);
    }

    Ok(())
}
