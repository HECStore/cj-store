//! CLI message handlers.
//!
//! All logs in this module are tagged `[CLI-Store]` so operator-originated
//! actions (issued from the CLI menu) are distinguishable from player chat
//! commands handled in `operator.rs` / `player.rs`.

use tokio::sync::oneshot;
use tracing::{debug, error, info, warn};

use crate::error::StoreError;
use crate::messages::{BotInstruction, CliMessage};
use crate::types::ItemId;
use crate::types::User;
use super::super::{Store, state, trade_state, utils};

/// Handle messages from the CLI
pub async fn handle_cli_message(store: &mut Store, message: CliMessage) -> Result<(), StoreError> {
    match message {
        CliMessage::QueryBalances { respond_to } => {
            debug!("[CLI-Store] Querying user balances");
            // Clone the user map so the CLI receives an owned Vec rather
            // than a borrow into live store state.
            let users: Vec<User> = store.users.values().cloned().collect();
            let _ = respond_to.send(users);
            Ok(())
        }
        CliMessage::QueryPairs { respond_to } => {
            debug!("[CLI-Store] Querying pairs");
            let pairs: Vec<crate::types::Pair> = store.pairs.values().cloned().collect();
            let _ = respond_to.send(pairs);
            Ok(())
        }
        CliMessage::QueryFee { respond_to } => {
            debug!("[CLI-Store] Querying fee rate");
            let _ = respond_to.send(store.config.fee);
            Ok(())
        }
        CliMessage::SetOperator {
            username_or_uuid,
            is_operator,
            respond_to,
        } => {
            // Shape-gate the input BEFORE any state mutation. The previous
            // hyphen heuristic accepted arbitrary strings like "steve-the-op"
            // as literal UUIDs; ensure_user_exists then inserted a phantom
            // operator=true record that the persistence layer silently
            // quarantines on next save while emitting a misleading success
            // log. Loud rejection at the input boundary fixes this.
            let trimmed = username_or_uuid.trim();
            let input_was_uuid = crate::types::user::is_valid_uuid_shape(trimmed);
            let uuid = if input_was_uuid {
                // Accepts both 36-char canonical hyphenated and 32-char bare
                // hex, matching what the persistence layer accepts.
                trimmed.to_string()
            } else if let Err(e) = crate::store::handlers::validation::validate_username(trimmed) {
                warn!("[CLI-Store] SetOperator: rejecting input {:?}: {}", username_or_uuid, e);
                let _ = respond_to.send(Err(
                    "input is neither a valid Minecraft username nor a canonical/bare-hex UUID".into()
                ));
                return Ok(());
            } else {
                // Username shape OK; resolve via Mojang. The typed
                // `MojangResolveError` is converted to a sanitized
                // `StoreError` via the central `From` impl in `error.rs`.
                crate::mojang::resolve_user_uuid(trimmed).await?
            };
            // When the operator typed a UUID, we must NOT pass `trimmed` as a
            // username to `ensure_user_exists` — `trimmed == uuid` in that
            // branch, which would corrupt an existing user's `username` field
            // with the UUID string (the drift branch overwrites unconditionally).
            // For an unknown UUID, refuse rather than fabricate a phantom record.
            // For a known UUID, look up the existing username so the drift
            // branch is a no-op.
            if input_was_uuid {
                let Some(existing) = store.users.get(&uuid) else {
                    warn!("[CLI-Store] SetOperator: unknown UUID {}", uuid);
                    let _ = respond_to.send(Err(
                        "unknown UUID — grant operator by username instead, or pre-onboard the user first".into()
                    ));
                    return Ok(());
                };
                let existing_username = existing.username.clone();
                utils::ensure_user_exists(store, &existing_username, &uuid);
            } else {
                // Auto-create the user record so operators can be granted to
                // players who have never interacted with the store.
                utils::ensure_user_exists(store, trimmed, &uuid);
            }
            if let Some(user) = store.users.get_mut(&uuid) {
                user.operator = is_operator;
                store.dirty = true;
                store.dirty_users.insert(uuid.clone());
                info!("[CLI-Store] Set operator={} for user {} ({})", is_operator, trimmed, uuid);
                let _ = respond_to.send(Ok(()));
            } else {
                // Guard against a failed insert rather than panicking;
                // should not happen after ensure_user_exists.
                error!("[CLI-Store] SetOperator: user {} ({}) missing after ensure_user_exists", trimmed, uuid);
                let _ = respond_to.send(Err("User not found".to_string()));
            }
            Ok(())
        }
        CliMessage::AddNode { respond_to } => {
            // Physical node validation is the OPERATOR's responsibility here:
            // the in-world 2x2 chest layout, shulker contents, shulker
            // station block, and bot pathing are all assumed correct. Use
            // AddNodeWithValidation to have the bot verify before insert.
            info!("[CLI-Store] Adding new node (no validation) - operator must ensure physical node exists at the calculated position");

            let node = store.storage.add_node();
            let node_id = node.id;
            info!("[CLI-Store] Node {} created at position ({}, {}, {})",
                  node_id, node.position.x, node.position.y, node.position.z);

            // Node 0's first two chests are forced to base currency and
            // overflow; every other pair is looked up by item id and needs
            // node 0 present to settle payments.
            if node_id == 0 {
                if let Some(chest_0) = node.chests.get_mut(0) {
                    chest_0.item = ItemId::new(crate::constants::BASE_CURRENCY_ITEM).expect("BASE_CURRENCY_ITEM is a valid item ID");
                    info!("[CLI-Store] Node 0 chest 0 set to base currency (forced)");
                }
                if let Some(chest_1) = node.chests.get_mut(1) {
                    chest_1.item = ItemId::new(crate::constants::OVERFLOW_CHEST_ITEM).expect("OVERFLOW_CHEST_ITEM is a valid item ID");
                    info!("[CLI-Store] Node 0 chest 1 set to overflow (forced)");
                }
            }

            // Node files are per-node, so save immediately; the aggregate
            // Storage state (node list) is flushed via store.dirty on the
            // next periodic save.
            if let Err(e) = node.save() {
                warn!("[CLI-Store] Failed to save node {}: {}", node_id, e);
            }

            store.dirty = true;
            let _ = respond_to.send(Ok(node_id));
            Ok(())
        }
        CliMessage::AddNodeWithValidation { respond_to } => {
            info!("[CLI-Store] Adding new node with physical validation");

            // Compute the next id/position BEFORE add_node so the bot can be
            // sent to the exact slot it would occupy, and we can abort
            // without rollback if validation fails.
            let mut next_node_id = 0i32;
            while store.storage.nodes.iter().any(|n| n.id == next_node_id) {
                next_node_id += 1;
            }
            let node_position = crate::types::Node::calc_position(next_node_id, &store.storage.position);

            info!("[CLI-Store] Validating node {} at position ({}, {}, {})",
                  next_node_id, node_position.x, node_position.y, node_position.z);

            let (validation_tx, validation_rx) = oneshot::channel();
            if let Err(e) = store.bot_tx.send(BotInstruction::ValidateNode {
                node_id: next_node_id,
                node_position,
                respond_to: validation_tx,
            }).await {
                error!("[CLI-Store] AddNodeWithValidation: bot channel send failed for node {}: {}", next_node_id, e);
                let _ = respond_to.send(Err(format!("Failed to send validation request to bot: {}", e)));
                return Ok(());
            }

            let validation_result = tokio::time::timeout(
                tokio::time::Duration::from_secs(120),
                validation_rx
            ).await;

            // Three nested Results:
            //   outer  = timeout elapsed
            //   middle = oneshot recv (bot dropped sender)
            //   inner  = validation outcome reported by the bot
            match validation_result {
                Ok(Ok(Ok(()))) => {
                    info!("[CLI-Store] Node {} validation passed, adding to storage", next_node_id);
                    let node = store.storage.add_node();
                    let node_id = node.id;

                    if node_id == 0 {
                        if let Some(chest_0) = node.chests.get_mut(0) {
                            chest_0.item = ItemId::new(crate::constants::BASE_CURRENCY_ITEM).expect("BASE_CURRENCY_ITEM is a valid item ID");
                            info!("[CLI-Store] Node 0 chest 0 set to base currency (forced)");
                        }
                        if let Some(chest_1) = node.chests.get_mut(1) {
                            chest_1.item = ItemId::new(crate::constants::OVERFLOW_CHEST_ITEM).expect("OVERFLOW_CHEST_ITEM is a valid item ID");
                            info!("[CLI-Store] Node 0 chest 1 set to overflow (forced)");
                        }
                    }

                    if let Err(e) = node.save() {
                        warn!("[CLI-Store] Failed to save node {}: {}", node_id, e);
                    }

                    store.dirty = true;
                    let _ = respond_to.send(Ok(node_id));
                }
                Ok(Ok(Err(validation_error))) => {
                    warn!("[CLI-Store] Node {} validation failed: {}", next_node_id, validation_error);
                    let _ = respond_to.send(Err(validation_error));
                }
                Ok(Err(_)) => {
                    error!("[CLI-Store] AddNodeWithValidation: bot validation response channel dropped (node {})", next_node_id);
                    let _ = respond_to.send(Err("Bot validation response dropped".to_string()));
                }
                Err(_) => {
                    warn!("[CLI-Store] AddNodeWithValidation: node {} timed out after 120s", next_node_id);
                    let _ = respond_to.send(Err("Node validation timed out after 120 seconds".to_string()));
                }
            }
            Ok(())
        }
        CliMessage::RemoveNode { node_id, respond_to } => {
            // Operator is expected to have withdrawn all items, confirmed no
            // pending orders reference this node, and stopped bot access
            // before calling this. We only warn on non-zero stored totals;
            // the physical chests remain in-world and must be cleared by hand.
            if let Some(node) = store.storage.nodes.iter().find(|n| n.id == node_id) {
                let total_items: i32 = node.chests.iter()
                    .flat_map(|c| c.amounts.iter())
                    .sum();
                if total_items > 0 {
                    warn!("[CLI-Store] Removing node {} which still contains {} items", node_id, total_items);
                }
            }

            let idx = store.storage.nodes.iter().position(|n| n.id == node_id);
            if let Some(idx) = idx {
                store.storage.nodes.remove(idx);
                // Delete data/storage/{node_id}.json so a stale file isn't
                // reloaded on next startup.
                let file_path = crate::types::node::node_file_path(node_id);
                if let Err(e) = std::fs::remove_file(&file_path) {
                    warn!("[CLI-Store] Failed to remove node file {}: {} (node removed from memory anyway)", file_path.display(), e);
                }
                store.dirty = true;
                info!("[CLI-Store] Removed node {}", node_id);
                let _ = respond_to.send(Ok(()));
            } else {
                warn!("[CLI-Store] RemoveNode: node {} not found", node_id);
                let _ = respond_to.send(Err(format!("Node {} not found", node_id)));
            }
            Ok(())
        }
        CliMessage::AddPair { item_name, stack_size, respond_to } => {
            if item_name.trim().is_empty() {
                let _ = respond_to.send(Err("Item name cannot be empty".to_string()));
                return Ok(());
            }
            // Stack size must match a real Minecraft stack: 1 (unstackable
            // tools), 16 (ender pearls, signs, snowballs), or 64 (most
            // items). Anything else is a typo.
            if stack_size != 1 && stack_size != 16 && stack_size != 64 {
                let _ = respond_to.send(Err(format!("Invalid stack size: {}. Must be 1, 16, or 64", stack_size)));
                return Ok(());
            }
            // Normalize to the canonical item id (strip minecraft: prefix)
            // so the pair key matches how trades reference the item.
            let item_id = match ItemId::new(&item_name) {
                Ok(id) => id,
                Err(_) => {
                    let _ = respond_to.send(Err("Invalid item name".to_string()));
                    return Ok(());
                }
            };
            let normalized_item = item_id.to_string();
            // `OVERFLOW_CHEST_ITEM` is the literal `item` field on node 0's
            // chest 1 (used as a sentinel for misplaced shulkers). Adding it
            // as a tradeable pair would let pair-stock recompute sum the
            // sentinel chest into the new pair's stock and corrupt AMM
            // pricing. `BASE_CURRENCY_ITEM` is the diamond pair the rest of
            // the code expects to be the canonical currency surface; a
            // duplicate created here would invalidate node 0's
            // forced-diamond invariant on every later add_node. Reject both
            // after `ItemId::new` normalization so case/prefix variants
            // (`Overflow`, `OVERFLOW`, `minecraft:overflow`, …) all hit the
            // same gate. RemovePair already rejects BASE_CURRENCY_ITEM at
            // line 283; this is the symmetric AddPair gate.
            if normalized_item == crate::constants::OVERFLOW_CHEST_ITEM
                || normalized_item == crate::constants::BASE_CURRENCY_ITEM
            {
                warn!(
                    "[CLI-Store] AddPair: '{}' is a reserved chest sentinel",
                    normalized_item
                );
                let _ = respond_to.send(Err(format!(
                    "'{}' is a reserved chest sentinel and cannot be a tradeable pair",
                    normalized_item
                )));
            } else if store.pairs.contains_key(&normalized_item) {
                warn!("[CLI-Store] AddPair: pair '{}' already exists", normalized_item);
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
                info!("[CLI-Store] Added pair '{}' (stack_size={})", normalized_item, stack_size);
                let _ = respond_to.send(Ok(()));
            }
            Ok(())
        }
        CliMessage::RemovePair { item_name, respond_to } => {
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

            // The base currency pair underpins every other pair's pricing
            // and user balance accounting; removing it would corrupt the
            // store, so reject unconditionally.
            if normalized_item == crate::constants::BASE_CURRENCY_ITEM {
                warn!("[CLI-Store] RemovePair: refused to remove base currency pair");
                let _ = respond_to.send(Err("Cannot remove diamond pair (used as currency)".to_string()));
                return Ok(());
            }

            if store.pairs.contains_key(&normalized_item) {
                if let Some(pair) = store.pairs.get(&normalized_item)
                    && (pair.item_stock > 0 || pair.currency_stock > 0.0) {
                        warn!("[CLI-Store] Removing pair '{}' which has stock: {} items, {:.2} currency",
                              normalized_item, pair.item_stock, pair.currency_stock);
                    }

                store.pairs.remove(&normalized_item);

                let file_path = crate::types::Pair::get_pair_file_path(&normalized_item);
                if let Err(e) = std::fs::remove_file(&file_path) {
                    warn!("[CLI-Store] Failed to remove pair file {}: {} (pair removed from memory anyway)", file_path.display(), e);
                }

                store.dirty = true;
                info!("[CLI-Store] Removed pair '{}'", normalized_item);
                let _ = respond_to.send(Ok(()));
            } else {
                warn!("[CLI-Store] RemovePair: pair '{}' not found", normalized_item);
                let _ = respond_to.send(Err(format!("Pair '{}' not found", normalized_item)));
            }
            Ok(())
        }
        CliMessage::QueryStorage { respond_to } => {
            debug!("[CLI-Store] Querying storage state");
            let _ = respond_to.send(store.storage.clone());
            Ok(())
        }
        CliMessage::QueryTrades { limit, respond_to } => {
            debug!("[CLI-Store] Querying recent trades (limit: {})", limit);
            // Trades are appended chronologically, so rev() + take(limit)
            // yields the N most recent in newest-first order without
            // allocating the full history when only a small window is asked for.
            let recent_trades: Vec<crate::types::Trade> = store.trades
                .iter()
                .rev()
                .take(limit)
                .cloned()
                .collect();
            let _ = respond_to.send(recent_trades);
            Ok(())
        }
        CliMessage::RestartBot { respond_to } => {
            info!("[CLI-Store] Initiating bot restart");
            // A bot_tx send failure means the bot channel is closed, which
            // is fatal for this handler: propagate the error upward (not
            // just via respond_to) so Store::run can surface it.
            if let Err(e) = store.bot_tx.send(BotInstruction::Restart).await {
                let error_msg = format!("Failed to send restart instruction: {}", e);
                error!("[CLI-Store] RestartBot: {}", error_msg);
                let _ = respond_to.send(Err(error_msg));
                return Err(StoreError::BotDisconnected);
            }
            let _ = respond_to.send(Ok(()));
            Ok(())
        }
        CliMessage::AuditState { repair, respond_to } => {
            info!("[CLI-Store] AuditState (repair={})", repair);
            let report = state::audit_state(store, repair);
            // Persist when repair actually changed something (either a
            // resolved issue or a remaining one surfaced for the operator).
            // State divergence is rare enough that over-persisting costs
            // nothing, while under-persisting would lose the fix on crash.
            if report.repair_applied {
                store.dirty = true;
            }
            let _ = respond_to.send(report.to_lines());
            Ok(())
        }
        CliMessage::DiscoverStorage { respond_to } => {
            info!("[CLI-Store] Starting storage discovery");

            let mut discovered_count = 0usize;

            // Snapshot existing IDs up-front so the skip check inside the
            // loop is O(1) and isn't invalidated as add_node() mutates the
            // nodes vec during discovery.
            let existing_ids: std::collections::HashSet<i32> = store.storage.nodes.iter()
                .map(|n| n.id)
                .collect();

            let mut next_node_id = 0i32;

            loop {
                while existing_ids.contains(&next_node_id) {
                    next_node_id += 1;
                }

                let node_position = crate::types::Node::calc_position(next_node_id, &store.storage.position);
                info!("[CLI-Store] Checking node {} at position ({}, {}, {})",
                      next_node_id, node_position.x, node_position.y, node_position.z);

                let (validation_tx, validation_rx) = oneshot::channel();
                if let Err(e) = store.bot_tx.send(BotInstruction::ValidateNode {
                    node_id: next_node_id,
                    node_position,
                    respond_to: validation_tx,
                }).await {
                    error!("[CLI-Store] DiscoverStorage: bot channel send failed for node {}: {}", next_node_id, e);
                    let _ = respond_to.send(Err(format!("Failed to send validation request: {}", e)));
                    return Ok(());
                }

                let validation_result = tokio::time::timeout(
                    tokio::time::Duration::from_secs(120),
                    validation_rx
                ).await;

                match validation_result {
                    Ok(Ok(Ok(()))) => {
                        info!("[CLI-Store] Discovered valid node at position {}", next_node_id);
                        let node = store.storage.add_node();
                        let node_id = node.id;

                        if node_id == 0 {
                            if let Some(chest_0) = node.chests.get_mut(0) {
                                chest_0.item = ItemId::new(crate::constants::BASE_CURRENCY_ITEM).expect("BASE_CURRENCY_ITEM is a valid item ID");
                            }
                            if let Some(chest_1) = node.chests.get_mut(1) {
                                chest_1.item = ItemId::new(crate::constants::OVERFLOW_CHEST_ITEM).expect("OVERFLOW_CHEST_ITEM is a valid item ID");
                            }
                        }

                        if let Err(e) = node.save() {
                            warn!("[CLI-Store] Failed to save discovered node {}: {}", node_id, e);
                        }

                        discovered_count += 1;
                        store.dirty = true;
                        next_node_id += 1;
                    }
                    Ok(Ok(Err(validation_error))) => {
                        // Discovery assumes nodes are laid out contiguously,
                        // so the first gap ends the scan.
                        info!("[CLI-Store] Node {} not found or invalid: {} - stopping discovery",
                              next_node_id, validation_error);
                        break;
                    }
                    Ok(Err(_)) => {
                        error!("[CLI-Store] DiscoverStorage: bot validation response channel dropped at node {}", next_node_id);
                        let _ = respond_to.send(Err("Bot validation response dropped".to_string()));
                        return Ok(());
                    }
                    Err(_) => {
                        warn!("[CLI-Store] Node {} validation timed out after 120s - stopping discovery", next_node_id);
                        break;
                    }
                }
            }

            info!("[CLI-Store] Storage discovery complete: {} nodes discovered", discovered_count);
            let _ = respond_to.send(Ok(discovered_count));
            Ok(())
        }
        CliMessage::ClearStuckOrder { respond_to } => {
            // Escape hatch: if an order never reaches a terminal state (bot
            // crashed mid-trade, chest stuck, etc.) the queue refuses to
            // advance because processing_order stays true. Forcibly clear
            // the flag so the next order is picked up on the following tick.
            info!("[CLI-Store] Clearing stuck order processing state");

            let stuck_order_desc = if store.processing_order {
                if let Some(ref trade) = store.current_trade {
                    let desc = format!("Order #{} [{}]: {}", trade.order().id, trade.phase(), trade);
                    warn!("[CLI-Store] Clearing stuck order: {}", desc);
                    Some(desc)
                } else {
                    warn!("[CLI-Store] processing_order=true but current_trade=None (inconsistent state)");
                    Some("Unknown order (inconsistent state)".to_string())
                }
            } else {
                info!("[CLI-Store] No stuck order detected (processing_order was already false)");
                None
            };

            store.processing_order = false;
            store.current_trade = None;
            store.dirty = true;

            // The on-disk trade-state mirror was the previous run's evidence
            // of an in-flight trade. The operator's explicit "clear" decision
            // means that evidence should not survive a subsequent crash —
            // otherwise next startup mis-detects the cleared trade as a fresh
            // interrupted-trade and emits a misleading alert.
            if let Err(e) = trade_state::clear_persisted() {
                warn!("[CLI-Store] Failed to clear persisted trade state after clearing stuck order: {}", e);
            }

            // Persist the queue immediately (in addition to the dirty flag)
            // so a crash before the next periodic save doesn't re-strand the
            // queue on the same phantom order.
            if let Err(e) = store.order_queue.save() {
                warn!("[CLI-Store] Failed to save queue after clearing stuck order: {}", e);
            }

            let _ = respond_to.send(stuck_order_desc);
            Ok(())
        }
        CliMessage::Shutdown { respond_to } => {
            // Graceful shutdown sequence (also documented in README):
            //   1. Signal Bot to shut down and wait for confirmation
            //      (Bot disconnects from the server).
            //   2. Save all store data to disk.
            //   3. Send confirmation to the CLI.
            // After this handler returns, Store::run breaks its loop and exits.
            info!("[CLI-Store] Shutdown: signalling Bot to disconnect");

            let (bot_response_tx, bot_response_rx) = oneshot::channel();
            if let Err(e) = store
                .bot_tx
                .send(BotInstruction::Shutdown {
                    respond_to: bot_response_tx,
                })
                .await
            {
                error!("[CLI-Store] Shutdown: failed to send shutdown instruction to Bot: {}", e);
            }

            if let Err(e) = bot_response_rx.await {
                error!("[CLI-Store] Shutdown: failed to receive Bot shutdown confirmation: {}", e);
            } else {
                info!("[CLI-Store] Shutdown: Bot shutdown confirmed");
            }

            info!("[CLI-Store] Shutdown: saving store data to disk");
            if let Err(e) = state::save(store) {
                error!("[CLI-Store] Shutdown: failed to save store data: {}", e);
            }

            let _ = respond_to.send(());
            info!("[CLI-Store] Shutdown complete");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    //! Regression tests for `handle_cli_message`. The Mojang lookup path
    //! requires network and is exercised separately; these tests focus on the
    //! UUID-input branch which must never round-trip the UUID string into
    //! `User.username`.

    use super::*;
    use std::collections::HashMap;
    use tokio::sync::{mpsc, oneshot};

    fn test_config() -> crate::config::Config {
        crate::config::Config {
            position: crate::types::Position { x: 0, y: 64, z: 0 },
            fee: 0.125,
            account_email: String::new(),
            server_address: "test".to_string(),
            buffer_chest_position: None,
            trade_timeout_ms: 5_000,
            pathfinding_timeout_ms: 5_000,
            max_orders: 1000,
            max_trades_in_memory: 1000,
            autosave_interval_secs: 10,
            chat: crate::config::ChatConfig::default(),
        }
    }

    /// Canonical 36-char hyphenated UUID for "Alice". Must satisfy
    /// `is_valid_uuid_shape` so the SetOperator handler takes the UUID branch.
    const ALICE_UUID: &str = "00000000-0000-0000-0000-000000000001";

    #[tokio::test]
    async fn set_operator_by_uuid_does_not_overwrite_username() {
        // Pre-populate a User with username "Alice" and the canonical UUID.
        let mut users: HashMap<String, User> = HashMap::new();
        users.insert(
            ALICE_UUID.to_string(),
            User {
                uuid: ALICE_UUID.to_string(),
                username: "Alice".to_string(),
                balance: 0.0,
                operator: false,
            },
        );
        let (bot_tx, _bot_rx) = mpsc::channel::<BotInstruction>(16);
        let mut store = Store::new_for_test(
            bot_tx,
            test_config(),
            HashMap::new(),
            users,
            crate::types::Storage::default(),
        );

        let (resp_tx, resp_rx) = oneshot::channel::<Result<(), String>>();
        let msg = CliMessage::SetOperator {
            username_or_uuid: ALICE_UUID.to_string(),
            is_operator: true,
            respond_to: resp_tx,
        };

        handle_cli_message(&mut store, msg).await.expect("handler ok");
        let result = resp_rx.await.expect("response received");
        assert!(result.is_ok(), "SetOperator by known UUID should succeed: {:?}", result);

        let user = store.users.get(ALICE_UUID).expect("user still present");
        assert_eq!(
            user.username, "Alice",
            "username must NOT be overwritten with the UUID string"
        );
        assert!(user.operator, "operator flag should be set to true");
    }

    #[tokio::test]
    async fn set_operator_by_unknown_uuid_is_rejected() {
        // No pre-existing user. Typing an unknown UUID must be a hard error
        // rather than fabricating a phantom record.
        let (bot_tx, _bot_rx) = mpsc::channel::<BotInstruction>(16);
        let mut store = Store::new_for_test(
            bot_tx,
            test_config(),
            HashMap::new(),
            HashMap::new(),
            crate::types::Storage::default(),
        );

        let (resp_tx, resp_rx) = oneshot::channel::<Result<(), String>>();
        let msg = CliMessage::SetOperator {
            username_or_uuid: ALICE_UUID.to_string(),
            is_operator: true,
            respond_to: resp_tx,
        };

        handle_cli_message(&mut store, msg).await.expect("handler ok");
        let result = resp_rx.await.expect("response received");
        assert!(result.is_err(), "unknown UUID must be rejected");
        assert!(
            !store.users.contains_key(ALICE_UUID),
            "no phantom record should be created for an unknown UUID"
        );
    }
}
