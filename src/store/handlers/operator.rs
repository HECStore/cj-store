//! Operator command handlers

use tracing::{error, info, warn};

use crate::constants::CHEST_OP_TIMEOUT_SECS;
use crate::messages::TradeItem;
use crate::types::{Order, Trade, TradeType};
use super::super::{Store, state, utils};

/// Handle additem orders (operator-only)
pub async fn handle_additem_order(
    store: &mut Store,
    player_name: &str,
    item: &str,
    quantity: u32,
) -> Result<(), String> {
    info!("[Additem] === STARTING ADDITEM ORDER === player={} item={} qty={}", player_name, item, quantity);
    state::assert_invariants(store, "pre-additem", false)?;
    let user_uuid = utils::resolve_user_uuid(store, player_name).await?;
    utils::ensure_user_exists(store, player_name, &user_uuid);

    if !store.pairs.contains_key(item) {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Item '{}' is not available. Use CLI to add it first.", item),
        )
        .await;
    }

    let qty_i32: i32 = quantity
        .try_into()
        .map_err(|_| "Quantity too large".to_string())?;
    if qty_i32 <= 0 {
        return utils::send_message_to_player(store, player_name, "Quantity must be positive")
            .await;
    }

    // Plan deposit
    let stack_size = store.pairs.get(item).unwrap().stack_size;
    let mut sim_storage = store.storage.clone();
    let preview_deposit_plan = sim_storage.deposit_plan(item, qty_i32, stack_size);

    // Notify operator before trade
    utils::send_message_to_player(
        store,
        player_name,
        &format!("Additem {} {}: Please offer the items in the trade.", quantity, item),
    ).await?;

    // Perform trade: player offers items, bot offers nothing
    let (trade_tx, trade_rx) = tokio::sync::oneshot::channel();
    let trade_send_result = store.bot_tx
        .send(crate::messages::BotInstruction::TradeWithPlayer {
            target_username: player_name.to_string(),
            bot_offers: vec![],
            player_offers: vec![TradeItem {
                item: item.to_string(),
                amount: qty_i32,
            }],
            // Exact-amount enforcement is critical here: the deposit plan was sized for
            // `qty_i32` and our stock accounting assumes that's what entered bot inventory.
            // Accepting a different amount would desync the plan from reality and could
            // leave orphaned items in the bot or under-fill chests.
            require_exact_amount: true,
            flexible_validation: false,
            respond_to: trade_tx,
        })
        .await;
    
    if let Err(e) = trade_send_result {
        error!("[Additem] FAILED to send trade instruction: {}", e);
        return Err(format!("Failed to send trade instruction to bot: {}", e));
    }

    let trade_result = tokio::time::timeout(tokio::time::Duration::from_millis(store.config.trade_timeout_ms), trade_rx)
        .await
        .map_err(|_| "Bot timed out waiting for trade completion".to_string())?
        .map_err(|e| format!("Bot response dropped: {}", e))?;
    if let Err(err) = &trade_result {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Additem aborted: trade failed: {}", err),
        )
        .await;
    }
    // Trade succeeded - for operator additem we trust the exact amount was given
    info!("[Additem] Trade succeeded, depositing items into storage...");

    // Deposit items into storage.
    // At this point the trade has already succeeded, so the items physically live in the
    // bot's inventory. If any subsequent chest step fails we must account for exactly how
    // much made it into storage vs. how much is still held by the bot, so we can hand the
    // remainder back to the operator rather than silently losing it.
    let mut items_deposited = 0i32;
    let mut deposit_failed = false;
    let mut failed_reason = String::new();
    
    for (step, t) in preview_deposit_plan.iter().enumerate() {
        let node_position = store.get_node_position(t.chest_id);
        let chest = crate::types::Chest {
            id: t.chest_id,
            node_id: t.chest_id / 4,
            index: t.chest_id % 4,
            position: t.position,
            item: t.item.clone(),
            amounts: vec![0; crate::types::Storage::SLOTS_PER_CHEST],
        };

        let (tx, rx) = tokio::sync::oneshot::channel();
        let send_result = store.bot_tx
            .send(crate::messages::BotInstruction::InteractWithChestAndSync {
                target_chest: chest,
                node_position,
                action: crate::messages::ChestAction::Deposit {
                    item: item.to_string(),
                    amount: t.amount,
                    from_player: None,
                    stack_size: stack_size,
                },
                respond_to: tx,
            })
            .await;
        
        if let Err(e) = send_result {
            error!("[Additem] Deposit step {} FAILED to send: {}", step + 1, e);
            deposit_failed = true;
            failed_reason = format!("Failed to send deposit instruction: {}", e);
            break;
        }

        let bot_result = tokio::time::timeout(tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS), rx)
            .await
            .map_err(|_| "Bot timed out performing chest step".to_string())
            .and_then(|r| r.map_err(|e| format!("Bot response dropped: {}", e)));

        match bot_result {
            Ok(Ok(report)) => {
                if let Err(e) = store.apply_chest_sync(report) {
                    warn!("[Additem] Chest sync failed after deposit: {}", e);
                }
                items_deposited += t.amount;
                info!("[Additem] Deposit step {} succeeded, {} items deposited so far", step + 1, items_deposited);
            }
            Ok(Err(err)) => {
                error!("[Additem] Deposit step {} bot error: {}", step + 1, err);
                deposit_failed = true;
                failed_reason = format!("Bot failed chest deposit: {}", err);
                break;
            }
            Err(err) => {
                error!("[Additem] Deposit step {} error: {}", step + 1, err);
                deposit_failed = true;
                failed_reason = err;
                break;
            }
        }
    }
    
    // Failsafe: the operator already parted with their items in the trade above, so any
    // undeposited remainder is sitting in the bot's inventory. Return it via a reverse
    // trade so the operator is made whole. If that reverse trade also fails, the items
    // are genuinely stuck and need manual admin recovery - we log a CRITICAL message.
    if deposit_failed {
        let items_in_bot_inventory = qty_i32 - items_deposited;
        if items_in_bot_inventory > 0 {
            warn!(
                "[Additem] Storage deposit failed. {} items in bot inventory, attempting to return to operator...",
                items_in_bot_inventory
            );
            
            let (rb_tx, rb_rx) = tokio::sync::oneshot::channel();
            let _ = store.bot_tx
                .send(crate::messages::BotInstruction::TradeWithPlayer {
                    target_username: player_name.to_string(),
                    bot_offers: vec![TradeItem {
                        item: item.to_string(),
                        amount: items_in_bot_inventory,
                    }],
                    player_offers: vec![],
                    // This is a return-to-sender trade; the operator offers nothing, so
                    // exact-amount enforcement would be meaningless here.
                    require_exact_amount: false,
                    flexible_validation: false,
                    respond_to: rb_tx,
                })
                .await;
            
            match tokio::time::timeout(tokio::time::Duration::from_millis(store.config.trade_timeout_ms), rb_rx).await {
                Ok(Ok(Ok(_))) => {
                    info!("[Additem] Successfully returned {} items to operator", items_in_bot_inventory);
                    return utils::send_message_to_player(
                        store,
                        player_name,
                        &format!(
                            "Additem failed: {}. {} items were returned to you. {} items were stored successfully.",
                            failed_reason, items_in_bot_inventory, items_deposited
                        ),
                    ).await;
                }
                _ => {
                    error!("[Additem] Failed to return items to operator - items stuck in bot inventory");
                    return utils::send_message_to_player(
                        store,
                        player_name,
                        &format!(
                            "Additem CRITICAL ERROR: {}. {} items stuck in bot inventory! Contact administrator. {} items were stored.",
                            failed_reason, items_in_bot_inventory, items_deposited
                        ),
                    ).await;
                }
            }
        } else {
            return utils::send_message_to_player(
                store,
                player_name,
                &format!("Additem failed: {}", failed_reason),
            ).await;
        }
    }

    // Commit: update pair stock from actual storage (bot has already synced chest contents)
    let pair = store.pairs.get_mut(item).unwrap();
    pair.item_stock = store.storage.total_item_amount(item);
    store.dirty = true;

    store.trades.push(Trade::new(
        TradeType::AddStock,
        item.to_string(),
        qty_i32,
        0.0,
        user_uuid.clone(),
    ));

    store.orders.push_back(Order {
        order_type: crate::types::order::OrderType::AddItem,
        item: item.to_string(),
        amount: qty_i32,
        user_uuid: user_uuid.clone(),
    });

    info!("Executed additem: user={} item={} qty={}", player_name, item, quantity);

    if let Err(e) = state::assert_invariants(store, "post-additem", true) {
        error!("Invariant violation after additem: {}", e);
        let _ = state::save(store);
    }

    let new_stock = store.pairs.get(item).map(|p| p.item_stock).unwrap_or(0);
    utils::send_message_to_player(
        store,
        player_name,
        &format!("Added {} {} to storage. New stock: {}", quantity, item, new_stock),
    )
    .await
}

/// Handle removeitem orders (operator-only)
pub async fn handle_removeitem_order(
    store: &mut Store,
    player_name: &str,
    item: &str,
    quantity: u32,
) -> Result<(), String> {
    state::assert_invariants(store, "pre-removeitem", false)?;
    let user_uuid = utils::resolve_user_uuid(store, player_name).await?;
    utils::ensure_user_exists(store, player_name, &user_uuid);

    if !store.pairs.contains_key(item) {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Item '{}' is not available.", item),
        )
        .await;
    }

    let qty_i32: i32 = quantity
        .try_into()
        .map_err(|_| "Quantity too large".to_string())?;
    if qty_i32 <= 0 {
        return utils::send_message_to_player(store, player_name, "Quantity must be positive")
            .await;
    }

    let physical_stock = store.storage.total_item_amount(item);
    if physical_stock < qty_i32 {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Out of physical stock for '{}'. Storage has {}, requested {}.",
                item, physical_stock, qty_i32
            ),
        )
        .await;
    }

    // Plan withdrawal
    let mut sim_storage = store.storage.clone();
    let preview_withdraw_plan = sim_storage.withdraw_plan(item, qty_i32);
    let preview_withdrawn: i32 = preview_withdraw_plan.iter().map(|t| t.amount).sum();
    if preview_withdrawn != qty_i32 {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Failed to plan withdrawal for '{}' from storage. Planned {}, needed {}.",
                item, preview_withdrawn, qty_i32
            ),
        )
        .await;
    }

    // Notify operator before withdrawal
    utils::send_message_to_player(
        store,
        player_name,
        &format!("Removeitem {} {}: Withdrawing from storage, then trading to you.", quantity, item),
    ).await?;

    // Withdraw items from storage
    for t in &preview_withdraw_plan {
        let node_position = store.get_node_position(t.chest_id);
        let chest = crate::types::Chest {
            id: t.chest_id,
            node_id: t.chest_id / 4,
            index: t.chest_id % 4,
            position: t.position,
            item: t.item.clone(),
            amounts: vec![0; crate::types::Storage::SLOTS_PER_CHEST],
        };

        let (tx, rx) = tokio::sync::oneshot::channel();
        store.bot_tx
            .send(crate::messages::BotInstruction::InteractWithChestAndSync {
                target_chest: chest,
                node_position,
                action: crate::messages::ChestAction::Withdraw {
                    item: item.to_string(),
                    amount: t.amount,
                    to_player: None,
                    stack_size: store.pairs.get(item).map(|p| p.stack_size).unwrap_or(64),
                },
                respond_to: tx,
            })
            .await
            .map_err(|e| format!("Failed to send chest instruction to bot: {}", e))?;

        let bot_result = tokio::time::timeout(tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS), rx)
            .await
            .map_err(|_| "Bot timed out performing chest step".to_string())?
            .map_err(|e| format!("Bot response dropped: {}", e))?;

        match bot_result {
            Err(err) => {
                return utils::send_message_to_player(
                    store,
                    player_name,
                    &format!("Removeitem aborted: bot failed chest withdrawal step: {}", err),
                )
                .await;
            }
            Ok(report) => {
                if let Err(e) = store.apply_chest_sync(report) {
                    warn!("Chest sync failed after withdraw: {}", e);
                }
            }
        }
    }

    // Perform trade: bot offers items, player offers nothing
    let (trade_tx, trade_rx) = tokio::sync::oneshot::channel();
    let trade_send_result = store.bot_tx
        .send(crate::messages::BotInstruction::TradeWithPlayer {
            target_username: player_name.to_string(),
            bot_offers: vec![TradeItem {
                item: item.to_string(),
                amount: qty_i32,
            }],
            player_offers: vec![],
            // Removeitem: player offers nothing
            require_exact_amount: false,
            flexible_validation: false,
            respond_to: trade_tx,
        })
        .await;
    
    if let Err(e) = trade_send_result {
        error!("[Removeitem] FAILED to send trade instruction: {}", e);
        // Rollback: withdrawal already moved items from chests into the bot.
        // Re-deposit each planned chunk back into its source chest via the shared helper.
        let stack_size = store.pairs.get(item).map(|p| p.stack_size).unwrap_or(64);
        let _ = super::super::rollback::deposit_transfers(
            store,
            &preview_withdraw_plan,
            item,
            stack_size,
            "[Removeitem] trade-send-failed",
        )
        .await;
        return Err(format!("Failed to send trade instruction to bot: {}", e));
    }

    let trade_result = tokio::time::timeout(tokio::time::Duration::from_millis(store.config.trade_timeout_ms), trade_rx)
        .await
        .map_err(|_| "Bot timed out waiting for trade completion".to_string())?
        .map_err(|e| format!("Bot response dropped: {}", e))?;
    
    if let Err(err) = &trade_result {
        error!("[Removeitem] Trade FAILED: {} - rolling back items to storage", err);
        // Rollback: items are still in the bot's inventory. Deposit them back using
        // the same plan we withdrew with via the shared helper.
        let stack_size = store.pairs.get(item).map(|p| p.stack_size).unwrap_or(64);
        let _ = super::super::rollback::deposit_transfers(
            store,
            &preview_withdraw_plan,
            item,
            stack_size,
            "[Removeitem] trade-failed",
        )
        .await;

        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Removeitem aborted: trade failed: {}. Items returned to storage.", err),
        )
        .await;
    }
    // Trade succeeded - bot gave items to operator

    // Commit: update pair stock from actual storage
    let pair = store.pairs.get_mut(item).unwrap();
    pair.item_stock = store.storage.total_item_amount(item);
    store.dirty = true;

    store.trades.push(Trade::new(
        TradeType::RemoveStock,
        item.to_string(),
        qty_i32,
        0.0,
        user_uuid.clone(),
    ));

    store.orders.push_back(Order {
        order_type: crate::types::order::OrderType::RemoveItem,
        item: item.to_string(),
        amount: qty_i32,
        user_uuid: user_uuid.clone(),
    });

    info!("Executed removeitem: user={} item={} qty={}", player_name, item, quantity);

    if let Err(e) = state::assert_invariants(store, "post-removeitem", true) {
        error!("Invariant violation after removeitem: {}", e);
        let _ = state::save(store);
    }

    let remaining_stock = store.pairs.get(item).map(|p| p.item_stock).unwrap_or(0);
    utils::send_message_to_player(
        store,
        player_name,
        &format!("Removed {} {} from storage. Remaining stock: {}", quantity, item, remaining_stock),
    )
    .await
}

/// Handle add currency (operator-only)
pub async fn handle_add_currency(
    store: &mut Store,
    player_name: &str,
    item: &str,
    amount: f64,
) -> Result<(), String> {
    state::assert_invariants(store, "pre-add-currency", false)?;
    let user_uuid = utils::resolve_user_uuid(store, player_name).await?;
    utils::ensure_user_exists(store, player_name, &user_uuid);

    if !store.pairs.contains_key(item) {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Item '{}' is not available. Use CLI to add it first.", item),
        )
        .await;
    }

    if !amount.is_finite() || amount <= 0.0 {
        return utils::send_message_to_player(store, player_name, "Amount must be positive")
            .await;
    }

    let pair = store.pairs.get_mut(item).unwrap();
    pair.currency_stock += amount;
    store.dirty = true;

    store.trades.push(Trade::new(
        TradeType::AddCurrency,
        item.to_string(),
        0,
        amount,
        user_uuid.clone(),
    ));

    store.orders.push_back(Order {
        order_type: crate::types::order::OrderType::AddCurrency,
        item: item.to_string(),
        amount: 0,
        user_uuid: user_uuid.clone(),
    });

    info!("Executed add currency: user={} item={} amount={}", player_name, item, amount);

    if let Err(e) = state::assert_invariants(store, "post-add-currency", true) {
        error!("Invariant violation after add currency: {}", e);
        let _ = state::save(store);
    }

    let new_reserve = store.pairs.get(item).map(|p| p.currency_stock).unwrap_or(0.0);
    utils::send_message_to_player(
        store,
        player_name,
        &format!("Added {:.2} diamonds to {} reserve. New reserve: {:.2}", amount, item, new_reserve),
    )
    .await
}

/// Handle remove currency (operator-only)
pub async fn handle_remove_currency(
    store: &mut Store,
    player_name: &str,
    item: &str,
    amount: f64,
) -> Result<(), String> {
    state::assert_invariants(store, "pre-remove-currency", false)?;
    let user_uuid = utils::resolve_user_uuid(store, player_name).await?;
    utils::ensure_user_exists(store, player_name, &user_uuid);

    if !store.pairs.contains_key(item) {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Item '{}' is not available.", item),
        )
        .await;
    }

    if !amount.is_finite() || amount <= 0.0 {
        return utils::send_message_to_player(store, player_name, "Amount must be positive")
            .await;
    }

    let pair = store.pairs.get(item).unwrap();
    if pair.currency_stock < amount {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Insufficient currency reserve. Available: {:.2}, requested: {:.2}",
                pair.currency_stock, amount
            ),
        )
        .await;
    }

    let pair = store.pairs.get_mut(item).unwrap();
    pair.currency_stock -= amount;
    store.dirty = true;

    store.trades.push(Trade::new(
        TradeType::RemoveCurrency,
        item.to_string(),
        0,
        amount,
        user_uuid.clone(),
    ));

    store.orders.push_back(Order {
        order_type: crate::types::order::OrderType::RemoveCurrency,
        item: item.to_string(),
        amount: 0,
        user_uuid: user_uuid.clone(),
    });

    info!("Executed remove currency: user={} item={} amount={}", player_name, item, amount);

    if let Err(e) = state::assert_invariants(store, "post-remove-currency", true) {
        error!("Invariant violation after remove currency: {}", e);
        let _ = state::save(store);
    }

    let remaining_reserve = store.pairs.get(item).map(|p| p.currency_stock).unwrap_or(0.0);
    utils::send_message_to_player(
        store,
        player_name,
        &format!("Removed {:.2} diamonds from {} reserve. Remaining reserve: {:.2}", amount, item, remaining_reserve),
    )
    .await
}
