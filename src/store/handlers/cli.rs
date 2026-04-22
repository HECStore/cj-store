//! CLI message handlers

use tokio::sync::oneshot;
use tracing::{debug, error, info, warn};

use crate::error::StoreError;
use crate::messages::{BotInstruction, CliMessage};
use crate::types::ItemId;
use crate::types::User;
use super::super::{Store, state, utils};

/// Handle messages from the CLI
pub async fn handle_cli_message(store: &mut Store, message: CliMessage) -> Result<(), StoreError> {
    match message {
        CliMessage::QueryBalances { respond_to } => {
            // Read-only snapshot: clones the user map so the CLI receives an
            // owned Vec without holding a reference into the live store.
            debug!("Querying user balances");
            let users: Vec<User> = store.users.values().cloned().collect();
            let _ = respond_to.send(users);
            Ok(())
        }
        CliMessage::QueryPairs { respond_to } => {
            debug!("Querying pairs");
            let pairs: Vec<crate::types::Pair> = store.pairs.values().cloned().collect();
            let _ = respond_to.send(pairs);
            Ok(())
        }
        CliMessage::QueryFee { respond_to } => {
            debug!("Querying fee rate");
            let _ = respond_to.send(store.config.fee);
            Ok(())
        }
        CliMessage::SetOperator {
            username_or_uuid,
            is_operator,
            respond_to,
        } => {
            // Heuristic: hyphen presence is used to distinguish a raw UUID
            // (e.g. "xxxxxxxx-xxxx-...") from a Minecraft username, which
            // cannot contain hyphens. Usernames require an async Mojang
            // lookup; UUIDs are used as-is.
            let uuid = if username_or_uuid.contains('-') {
                // Assume it's a UUID
                username_or_uuid.clone()
            } else {
                // Assume it's a username, resolve UUID
                utils::resolve_user_uuid(&username_or_uuid).await?
            };
            // Auto-create the user record if missing so operators can be
            // granted to players who have never interacted with the store.
            utils::ensure_user_exists(store, &username_or_uuid, &uuid);
            if let Some(user) = store.users.get_mut(&uuid) {
                user.operator = is_operator;
                // Mark dirty so the periodic save picks up the flag change.
                store.dirty = true;
                let _ = respond_to.send(Ok(()));
            } else {
                // Shouldn't normally happen after ensure_user_exists, but
                // guard against a failed insert rather than panicking.
                let _ = respond_to.send(Err("User not found".to_string()));
            }
            Ok(())
        }
        CliMessage::AddNode { respond_to } => {
            // IMPORTANT: Physical node validation is the OPERATOR's responsibility.
            // Before adding a node, ensure the following in-world setup exists:
            // 1. Four double chests arranged in a 2x2 pattern (see README for exact layout)
            // 2. Each chest slot (54 total per chest) must contain one shulker box
            // 3. A shulker station block must be placed at the correct position
            // 4. The bot must have clear pathfinding access to the node position
            //
            // For bot-based validation, use AddNodeWithValidation instead.
            //
            // For now, we trust the operator has built the node correctly.
            // If the node doesn't exist, the bot will fail when trying to access it.
            info!("[CLI] Adding new node (no validation) - operator must ensure physical node exists at the calculated position");
            
            let node = store.storage.add_node();
            let node_id = node.id;
            info!("[CLI] Node {} created at position ({}, {}, {})", 
                  node_id, node.position.x, node.position.y, node.position.z);
            
            // Node 0 has reserved chests (forced, cannot change)
            if node_id == 0 {
                if let Some(chest_0) = node.chests.get_mut(0) {
                    chest_0.item = ItemId::new(crate::constants::BASE_CURRENCY_ITEM).expect("BASE_CURRENCY_ITEM is a valid item ID");
                    info!("Node 0 chest 0 set to diamond (forced, cannot change)");
                }
                if let Some(chest_1) = node.chests.get_mut(1) {
                    chest_1.item = ItemId::new(crate::constants::OVERFLOW_CHEST_ITEM).expect("OVERFLOW_CHEST_ITEM is a valid item ID");
                    info!("Node 0 chest 1 set to overflow (forced, cannot change)");
                }
            }
            
            // Save the node to persist it (including reserved assignments for node 0).
            // Node persistence is per-file so we save immediately rather than
            // waiting for the periodic store-level save.
            if let Err(e) = node.save() {
                warn!("Failed to save node {}: {}", node_id, e);
            }

            // Dirty flag still set so the aggregate Storage state (which
            // tracks the node list) is persisted on the next save cycle.
            store.dirty = true;
            let _ = respond_to.send(Ok(node_id));
            Ok(())
        }
        CliMessage::AddNodeWithValidation { respond_to } => {
            // Add node with bot-based physical validation.
            // Bot will navigate to the calculated position and verify:
            // 1. All 4 chests exist and can be opened
            // 2. Each chest slot contains a shulker box
            // Only adds the node if all checks pass.
            info!("[CLI] Adding new node with physical validation");
            
            // Calculate the would-be node ID and position.
            // We compute this BEFORE calling add_node so the bot can be sent
            // to the exact position it will occupy, and we can reject the
            // node (without rollback) if validation fails.
            let mut next_node_id = 0i32;
            while store.storage.nodes.iter().any(|n| n.id == next_node_id) {
                next_node_id += 1;
            }
            let node_position = crate::types::Node::calc_position(next_node_id, &store.storage.position);
            
            info!("[CLI] Validating node {} at position ({}, {}, {})",
                  next_node_id, node_position.x, node_position.y, node_position.z);
            
            // Send validation request to bot
            let (validation_tx, validation_rx) = oneshot::channel();
            if let Err(e) = store.bot_tx.send(BotInstruction::ValidateNode {
                node_id: next_node_id,
                node_position,
                respond_to: validation_tx,
            }).await {
                let _ = respond_to.send(Err(format!("Failed to send validation request to bot: {}", e)));
                return Ok(());
            }
            
            // Wait for validation result (with timeout)
            let validation_result = tokio::time::timeout(
                tokio::time::Duration::from_secs(120), // 2 minute timeout for validation
                validation_rx
            ).await;
            
            // Nested result layering:
            //   outer Result = timeout (Err = elapsed)
            //   middle Result = oneshot recv (Err = bot dropped sender)
            //   inner Result = actual validation outcome from the bot
            match validation_result {
                Ok(Ok(Ok(()))) => {
                    // Validation passed - add the node
                    info!("[CLI] Node {} validation passed, adding to storage", next_node_id);
                    let node = store.storage.add_node();
                    let node_id = node.id;
                    
                    // Node 0 has reserved chests
                    if node_id == 0 {
                        if let Some(chest_0) = node.chests.get_mut(0) {
                            chest_0.item = ItemId::new(crate::constants::BASE_CURRENCY_ITEM).expect("BASE_CURRENCY_ITEM is a valid item ID");
                            info!("Node 0 chest 0 set to diamond (forced)");
                        }
                        if let Some(chest_1) = node.chests.get_mut(1) {
                            chest_1.item = ItemId::new(crate::constants::OVERFLOW_CHEST_ITEM).expect("OVERFLOW_CHEST_ITEM is a valid item ID");
                            info!("Node 0 chest 1 set to overflow (forced)");
                        }
                    }
                    
                    if let Err(e) = node.save() {
                        warn!("Failed to save node {}: {}", node_id, e);
                    }
                    
                    store.dirty = true;
                    let _ = respond_to.send(Ok(node_id));
                }
                Ok(Ok(Err(validation_error))) => {
                    // Validation failed - don't add the node
                    warn!("[CLI] Node {} validation failed: {}", next_node_id, validation_error);
                    let _ = respond_to.send(Err(validation_error));
                }
                Ok(Err(_)) => {
                    // Channel dropped
                    let _ = respond_to.send(Err("Bot validation response dropped".to_string()));
                }
                Err(_) => {
                    // Timeout
                    let _ = respond_to.send(Err("Node validation timed out after 120 seconds".to_string()));
                }
            }
            Ok(())
        }
        CliMessage::RemoveNode { node_id, respond_to } => {
            // IMPORTANT: Before removing a node, ensure:
            // 1. All items have been withdrawn from the node's chests
            // 2. No pending orders reference chests in this node
            // 3. The node is not currently being accessed by the bot
            //
            // This operation removes the node from the storage model.
            // The physical chests remain in-world and should be manually cleared.
            //
            // Full validation could check if any chests have non-zero amounts,
            // but for now we trust the operator has cleared the node.
            
            // Check if node has any items stored (basic validation)
            if let Some(node) = store.storage.nodes.iter().find(|n| n.id == node_id) {
                let total_items: i32 = node.chests.iter()
                    .flat_map(|c| c.amounts.iter())
                    .sum();
                if total_items > 0 {
                    warn!("[CLI] Removing node {} which may still contain {} items", node_id, total_items);
                }
            }
            
            let idx = store.storage.nodes.iter().position(|n| n.id == node_id);
            if let Some(idx) = idx {
                store.storage.nodes.remove(idx);
                // Remove the file data/storage/{node_id}.json so a stale
                // entry doesn't get reloaded on next startup.
                let file_path = format!("data/storage/{}.json", node_id);
                if let Err(e) = std::fs::remove_file(&file_path) {
                    warn!("Failed to remove node file {}: {}", file_path, e);
                    // Continue anyway - the node is removed from memory
                }
                // Persist the updated Storage (node list) on the next save.
                store.dirty = true;
                let _ = respond_to.send(Ok(()));
            } else {
                let _ = respond_to.send(Err(format!("Node {} not found", node_id)));
            }
            Ok(())
        }
        CliMessage::AddPair { item_name, stack_size, respond_to } => {
            // Validate item name is not empty
            if item_name.trim().is_empty() {
                let _ = respond_to.send(Err("Item name cannot be empty".to_string()));
                return Ok(());
            }
            // Stack size must be a valid Minecraft stack size: 1 (unstackable
            // items like tools), 16 (ender pearls, signs, snowballs), or
            // 64 (most items). Any other value indicates a typo.
            if stack_size != 1 && stack_size != 16 && stack_size != 64 {
                let _ = respond_to.send(Err(format!("Invalid stack size: {}. Must be 1, 16, or 64", stack_size)));
                return Ok(());
            }
            // Normalize to the canonical item id (strip minecraft: prefix) so
            // the pair key is consistent with how trades reference it.
            let item_id = match ItemId::new(&item_name) {
                Ok(id) => id,
                Err(_) => {
                    let _ = respond_to.send(Err("Invalid item name".to_string()));
                    return Ok(());
                }
            };
            let normalized_item = item_id.to_string();
            if store.pairs.contains_key(&normalized_item) {
                let _ = respond_to.send(Err(format!("Pair '{}' already exists", normalized_item)));
            } else {
                store.pairs.insert(
                    normalized_item.clone(),
                    crate::types::Pair {
                        item: item_id,
                        stack_size,
                        item_stock: 0,
                        currency_stock: 0.0,
                    },
                );
                store.dirty = true;
                let _ = respond_to.send(Ok(()));
            }
            Ok(())
        }
        CliMessage::RemovePair { item_name, respond_to } => {
            // Validate item name is not empty
            if item_name.trim().is_empty() {
                let _ = respond_to.send(Err("Item name cannot be empty".to_string()));
                return Ok(());
            }
            let normalized_item = match ItemId::new(&item_name) {
                Ok(id) => id.to_string(),
                Err(_) => {
                    let _ = respond_to.send(Err("Invalid item name".to_string()));
                    return Ok(());
                }
            };

            // The base currency pair cannot be removed; it underpins every
            // existing pair's pricing and user balance accounting.
            if normalized_item == crate::constants::BASE_CURRENCY_ITEM {
                let _ = respond_to.send(Err("Cannot remove diamond pair (used as currency)".to_string()));
                return Ok(());
            }
            
            if store.pairs.contains_key(&normalized_item) {
                // Check if pair has any stock (warn but still allow removal)
                if let Some(pair) = store.pairs.get(&normalized_item)
                    && (pair.item_stock > 0 || pair.currency_stock > 0.0) {
                        warn!("[CLI] Removing pair '{}' which has stock: {} items, {:.2} currency", 
                              normalized_item, pair.item_stock, pair.currency_stock);
                    }
                
                store.pairs.remove(&normalized_item);
                
                // Remove the pair file from disk
                let file_path = format!("data/pairs/{}.json", normalized_item);
                if let Err(e) = std::fs::remove_file(&file_path) {
                    warn!("Failed to remove pair file {}: {}", file_path, e);
                    // Continue anyway - the pair is removed from memory
                }
                
                store.dirty = true;
                let _ = respond_to.send(Ok(()));
            } else {
                let _ = respond_to.send(Err(format!("Pair '{}' not found", normalized_item)));
            }
            Ok(())
        }
        CliMessage::QueryStorage { respond_to } => {
            debug!("Querying storage state");
            let _ = respond_to.send(store.storage.clone());
            Ok(())
        }
        CliMessage::QueryTrades { limit, respond_to } => {
            debug!("Querying recent trades (limit: {})", limit);
            // Trades are appended chronologically, so rev() + take(limit)
            // yields the N most recent trades in newest-first order. Using
            // .take() on the reversed iterator avoids allocating the full
            // history when only a small window is requested.
            let recent_trades: Vec<crate::types::Trade> = store.trades
                .iter()
                .rev() // Most recent first
                .take(limit)
                .cloned()
                .collect();
            let _ = respond_to.send(recent_trades);
            Ok(())
        }
        CliMessage::RestartBot { respond_to } => {
            info!("Initiating bot restart");
            // A failed bot_tx send means the bot channel is closed, which is
            // fatal for this handler: we propagate the error upward (not just
            // via respond_to) so Store::run can surface it.
            if let Err(e) = store.bot_tx.send(BotInstruction::Restart).await {
                let error_msg = format!("Failed to send restart instruction: {}", e);
                error!("{}", error_msg);
                let _ = respond_to.send(Err(error_msg));
                return Err(StoreError::BotDisconnected);
            }
            let _ = respond_to.send(Ok(()));
            Ok(())
        }
        CliMessage::AuditState { repair, respond_to } => {
            let report = state::audit_state(store, repair);
            // Persist if we ran repair and actually fixed something - either a
            // resolved issue (removed from `issues`) or a remaining issue
            // (surfaced for the operator). The absence of state divergence is
            // rare enough that over-persisting costs nothing, while under-
            // persisting would lose the fix on next crash.
            if repair && report.repair_applied {
                store.dirty = true;
            }
            let _ = respond_to.send(report.to_lines());
            Ok(())
        }
        CliMessage::DiscoverStorage { respond_to } => {
            // Discover storage nodes by having the bot physically visit positions.
            // Bot iterates through node positions (0, 1, 2, ...) until it finds
            // a position without valid chests.
            info!("[CLI] Starting storage discovery");
            
            let mut discovered_count = 0usize;

            // Snapshot existing IDs up-front so the skip check inside the
            // loop is O(1) and isn't affected as add_node() mutates the
            // nodes vec during discovery.
            let existing_ids: std::collections::HashSet<i32> = store.storage.nodes.iter()
                .map(|n| n.id)
                .collect();
            
            let mut next_node_id = 0i32;
            
            // Try to discover nodes sequentially
            loop {
                // Skip node IDs that already exist
                while existing_ids.contains(&next_node_id) {
                    next_node_id += 1;
                }
                
                let node_position = crate::types::Node::calc_position(next_node_id, &store.storage.position);
                info!("[CLI] Checking node {} at position ({}, {}, {})",
                      next_node_id, node_position.x, node_position.y, node_position.z);
                
                // Send validation request to bot
                let (validation_tx, validation_rx) = oneshot::channel();
                if let Err(e) = store.bot_tx.send(BotInstruction::ValidateNode {
                    node_id: next_node_id,
                    node_position,
                    respond_to: validation_tx,
                }).await {
                    let _ = respond_to.send(Err(format!("Failed to send validation request: {}", e)));
                    return Ok(());
                }
                
                // Wait for validation result (with timeout)
                let validation_result = tokio::time::timeout(
                    tokio::time::Duration::from_secs(120), // 2 minute timeout per node
                    validation_rx
                ).await;
                
                match validation_result {
                    Ok(Ok(Ok(()))) => {
                        // Node found and valid - add it
                        info!("[CLI] Discovered valid node at position {}", next_node_id);
                        let node = store.storage.add_node();
                        let node_id = node.id;
                        
                        // Node 0 has reserved chests
                        if node_id == 0 {
                            if let Some(chest_0) = node.chests.get_mut(0) {
                                chest_0.item = ItemId::new(crate::constants::BASE_CURRENCY_ITEM).expect("BASE_CURRENCY_ITEM is a valid item ID");
                            }
                            if let Some(chest_1) = node.chests.get_mut(1) {
                                chest_1.item = ItemId::new(crate::constants::OVERFLOW_CHEST_ITEM).expect("OVERFLOW_CHEST_ITEM is a valid item ID");
                            }
                        }
                        
                        if let Err(e) = node.save() {
                            warn!("Failed to save discovered node {}: {}", node_id, e);
                        }
                        
                        discovered_count += 1;
                        store.dirty = true;
                        next_node_id += 1;
                    }
                    Ok(Ok(Err(validation_error))) => {
                        // A missing/invalid position is the termination
                        // signal: discovery assumes nodes are laid out
                        // contiguously, so the first gap ends the scan.
                        info!("[CLI] Node {} not found or invalid: {} - stopping discovery",
                              next_node_id, validation_error);
                        break;
                    }
                    Ok(Err(_)) => {
                        // Channel dropped
                        let _ = respond_to.send(Err("Bot validation response dropped".to_string()));
                        return Ok(());
                    }
                    Err(_) => {
                        // Timeout - stop discovery
                        warn!("[CLI] Node validation timed out - stopping discovery");
                        break;
                    }
                }
            }
            
            info!("[CLI] Storage discovery complete: {} nodes discovered", discovered_count);
            let _ = respond_to.send(Ok(discovered_count));
            Ok(())
        }
        CliMessage::ClearStuckOrder { respond_to } => {
            // Manual escape hatch: if an order never reaches a terminal
            // state (bot crashed mid-trade, chest stuck, etc.) the queue
            // refuses to advance because processing_order stays true.
            // This command forcibly clears that flag so the next order can
            // be picked up on the following tick.
            info!("[CLI] Clearing stuck order processing state");
            
            let stuck_order_desc = if store.processing_order {
                if let Some(ref trade) = store.current_trade {
                    let desc = format!("Order #{} [{}]: {}", trade.order().id, trade.phase(), trade);
                    warn!("[CLI] Clearing stuck order: {}", desc);
                    Some(desc)
                } else {
                    warn!("[CLI] processing_order was true but current_trade was None (inconsistent state)");
                    Some("Unknown order (inconsistent state)".to_string())
                }
            } else {
                info!("[CLI] No stuck order detected (processing_order was already false)");
                None
            };

            // Reset the processing state
            store.processing_order = false;
            store.current_trade = None;
            store.dirty = true;

            // Persist the queue immediately (in addition to the dirty flag)
            // so a crash between now and the next periodic save doesn't
            // re-strand the queue on the same phantom order.
            if let Err(e) = store.order_queue.save() {
                warn!("[CLI] Failed to save queue after clearing stuck order: {}", e);
            }
            
            let _ = respond_to.send(stuck_order_desc);
            Ok(())
        }
        CliMessage::Shutdown { respond_to } => {
            // Graceful shutdown sequence:
            // 1. Signal Bot to shutdown and wait for confirmation (Bot disconnects from server)
            // 2. Save all store data to disk
            // 3. Send confirmation to CLI
            // After this handler returns, Store::run() will break from its loop and exit
            // See README.md "Graceful Shutdown" section for complete sequence
            info!("[Store] Shutdown handler: Initiating graceful shutdown");
            info!("[Store] Shutdown handler: Step 1/4 - Sending shutdown instruction to Bot");

            // Signal bot to shutdown
            let (bot_response_tx, bot_response_rx) = oneshot::channel();
            if let Err(e) = store
                .bot_tx
                .send(BotInstruction::Shutdown {
                    respond_to: bot_response_tx,
                })
                .await
            {
                error!("[Store] Shutdown handler: Failed to send shutdown instruction to bot: {}", e);
            } else {
                info!("[Store] Shutdown handler: Shutdown instruction sent to Bot, waiting for confirmation");
            }

            // Wait for bot shutdown confirmation
            info!("[Store] Shutdown handler: Step 2/4 - Waiting for Bot shutdown confirmation");
            if let Err(e) = bot_response_rx.await {
                error!("[Store] Shutdown handler: Failed to receive bot shutdown confirmation: {}", e);
            } else {
                info!("[Store] Shutdown handler: Bot shutdown confirmed");
            }

            // Save all data before shutdown
            info!("[Store] Shutdown handler: Step 3/4 - Saving all store data to disk");
            if let Err(e) = state::save(store) {
                error!("[Store] Shutdown handler: Failed to save store data during shutdown: {}", e);
            } else {
                info!("[Store] Shutdown handler: Store data saved successfully");
            }

            // Signal shutdown complete
            info!("[Store] Shutdown handler: Step 4/4 - Sending shutdown complete signal to CLI");
            let _ = respond_to.send(());
            info!("[Store] Shutdown handler: Shutdown complete, handler returning");
            Ok(())
        }
    }
}
