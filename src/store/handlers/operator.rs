//! Operator command handlers

use tracing::{error, info, warn};

use crate::constants::{CHEST_OP_TIMEOUT_SECS, CHESTS_PER_NODE};
use crate::error::StoreError;
use crate::messages::TradeItem;
use crate::types::{ItemId, Order, Trade, TradeType};
use super::super::{Store, state, utils};

/// Operator command: move items from the operator's inventory into storage via
/// a one-sided trade, then deposit them across chests according to the planner.
/// The operator is made whole with a reverse trade if any deposit step fails.
pub async fn handle_additem_order(
    store: &mut Store,
    player_name: &str,
    item: &str,
    quantity: u32,
) -> Result<(), StoreError> {
    state::assert_invariants(store, "pre-additem", false)?;
    let user_uuid = utils::resolve_user_uuid(player_name).await?;
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
        .map_err(|_| StoreError::ValidationError("Quantity too large".to_string()))?;
    if qty_i32 <= 0 {
        return utils::send_message_to_player(store, player_name, "Quantity must be positive")
            .await;
    }

    let stock_before = store.pairs.get(item).map(|p| p.item_stock).unwrap_or(0);
    info!(
        "[Additem] start: operator={} uuid={} item={} qty={} stock_before={}",
        player_name, user_uuid, item, quantity, stock_before
    );

    // Plan deposit against a read-only view of storage so we don't pay the
    // cost of cloning the entire structure just to preview placement.
    let stack_size = store.expect_pair(item, "additem/preview")?.stack_size;
    let (preview_deposit_plan, _) =
        store.storage.simulate_deposit_plan(item, qty_i32, stack_size);

    utils::send_message_to_player(
        store,
        player_name,
        &format!("Additem {} {}: Please offer the items in the trade.", quantity, item),
    ).await?;

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
        error!("[Additem] failed to send trade instruction: operator={} item={} qty={} err={}",
            player_name, item, quantity, e);
        return Err(StoreError::BotError(format!("Failed to send trade instruction to bot: {}", e)));
    }

    let trade_result = tokio::time::timeout(tokio::time::Duration::from_millis(store.config.trade_timeout_ms), trade_rx)
        .await
        .map_err(|_| StoreError::TradeTimeout(store.config.trade_timeout_ms / 1000))?
        .map_err(|e| StoreError::BotError(format!("Bot response dropped: {}", e)))?;
    if let Err(err) = &trade_result {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Additem aborted: trade failed: {}", err),
        )
        .await;
    }
    // Trade accepted exact quantity; items now live in the bot's inventory. If any
    // chest step below fails we must track how much was deposited vs. still held so
    // the remainder can be returned via a reverse trade rather than silently lost.
    let mut items_deposited = 0i32;
    let mut deposit_failed = false;
    let mut failed_reason = String::new();
    
    for (step, t) in preview_deposit_plan.iter().enumerate() {
        let node_position = store.get_node_position(t.chest_id);
        let chest = crate::types::Chest {
            id: t.chest_id,
            node_id: t.chest_id / CHESTS_PER_NODE as i32,
            index: t.chest_id % CHESTS_PER_NODE as i32,
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
                    stack_size,
                },
                respond_to: tx,
            })
            .await;
        
        if let Err(e) = send_result {
            error!("[Additem] deposit step {} failed to send: operator={} item={} chunk_amount={} err={}",
                step + 1, player_name, item, t.amount, e);
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
                    warn!("[Additem] chest sync failed after deposit: item={} chest_id={} err={}",
                        item, t.chest_id, e);
                }
                items_deposited += t.amount;
            }
            Ok(Err(err)) => {
                error!("[Additem] deposit step {} bot error: item={} chest_id={} chunk_amount={} err={}",
                    step + 1, item, t.chest_id, t.amount, err);
                deposit_failed = true;
                failed_reason = format!("Bot failed chest deposit: {}", err);
                break;
            }
            Err(err) => {
                error!("[Additem] deposit step {} error: item={} chest_id={} chunk_amount={} err={}",
                    step + 1, item, t.chest_id, t.amount, err);
                deposit_failed = true;
                failed_reason = err;
                break;
            }
        }
    }
    
    // The operator already parted with their items in the trade above, so any
    // undeposited remainder is sitting in the bot's inventory. Return it via a reverse
    // trade so the operator is made whole. If that reverse trade also fails, the items
    // are genuinely stuck and need manual admin recovery (logged at error level).
    if deposit_failed {
        let items_in_bot_inventory = qty_i32 - items_deposited;
        if items_in_bot_inventory > 0 {
            warn!(
                "[Additem] deposit failed, returning to operator: operator={} item={} deposited={} stuck={} reason={}",
                player_name, item, items_deposited, items_in_bot_inventory, failed_reason
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
                    info!(
                        "[Additem] returned items to operator: operator={} item={} returned={} deposited={}",
                        player_name, item, items_in_bot_inventory, items_deposited
                    );
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
                    error!(
                        "[Additem] CRITICAL: return-to-operator trade failed, {} item(s) of '{}' stuck in bot inventory; operator={} deposited={}",
                        items_in_bot_inventory, item, player_name, items_deposited
                    );
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

    // Resync pair stock from authoritative storage totals (chest syncs above may have
    // moved the real amount by more than `items_deposited` if there was drift).
    let new_stock = store.storage.total_item_amount(item);
    let pair = store.expect_pair_mut(item, "additem/commit")?;
    pair.item_stock = new_stock;
    assert!(pair.item_stock >= 0,
        "[Additem] INVARIANT VIOLATED: item_stock went negative after add_stock \
        (item={}, stock={}). This indicates a storage accounting bug.",
        item, pair.item_stock);
    store.dirty = true;

    store.trades.push(Trade::new(
        TradeType::AddStock,
        ItemId::from_normalized(item.to_string()),
        qty_i32,
        0.0,
        user_uuid.clone(),
    ));

    store.orders.push_back(Order {
        order_type: crate::types::order::OrderType::AddItem,
        item: ItemId::from_normalized(item.to_string()),
        amount: qty_i32,
        currency_amount: 0.0,
        user_uuid: user_uuid.clone(),
    });

    info!(
        "[Additem] committed: operator={} uuid={} item={} qty={} stock_before={} stock_after={}",
        player_name, user_uuid, item, quantity, stock_before, new_stock
    );

    if let Err(e) = state::assert_invariants(store, "post-additem", true) {
        error!("[Additem] invariant violation after commit: operator={} item={} err={}",
            player_name, item, e);
        let _ = state::save(store);
    }

    utils::send_message_to_player(
        store,
        player_name,
        &format!("Added {} {} to storage. New stock: {}", quantity, item, new_stock),
    )
    .await
}

/// Operator command: withdraw items from storage chests into the bot's inventory,
/// then hand them to the operator via a one-sided trade. On any failure the
/// withdrawal is rolled back into its source chunks via the shared rollback helper.
pub async fn handle_removeitem_order(
    store: &mut Store,
    player_name: &str,
    item: &str,
    quantity: u32,
) -> Result<(), StoreError> {
    state::assert_invariants(store, "pre-removeitem", false)?;
    let user_uuid = utils::resolve_user_uuid(player_name).await?;
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
        .map_err(|_| StoreError::ValidationError("Quantity too large".to_string()))?;
    if qty_i32 <= 0 {
        return utils::send_message_to_player(store, player_name, "Quantity must be positive")
            .await;
    }

    let stock_before = store.pairs.get(item).map(|p| p.item_stock).unwrap_or(0);
    info!(
        "[Removeitem] start: operator={} uuid={} item={} qty={} stock_before={}",
        player_name, user_uuid, item, quantity, stock_before
    );

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

    // Plan withdrawal without cloning storage.
    let (preview_withdraw_plan, preview_withdrawn) =
        store.storage.simulate_withdraw_plan(item, qty_i32);
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

    utils::send_message_to_player(
        store,
        player_name,
        &format!("Removeitem {} {}: Withdrawing from storage, then trading to you.", quantity, item),
    ).await?;

    for t in &preview_withdraw_plan {
        let node_position = store.get_node_position(t.chest_id);
        let chest = crate::types::Chest {
            id: t.chest_id,
            node_id: t.chest_id / CHESTS_PER_NODE as i32,
            index: t.chest_id % CHESTS_PER_NODE as i32,
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
            .map_err(|e| StoreError::BotError(format!("Failed to send chest instruction to bot: {}", e)))?;

        let bot_result = tokio::time::timeout(tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS), rx)
            .await
            .map_err(|_| StoreError::TradeTimeout(CHEST_OP_TIMEOUT_SECS))?
            .map_err(|e| StoreError::BotError(format!("Bot response dropped: {}", e)))?;

        match bot_result {
            Err(err) => {
                error!("[Removeitem] chest withdraw step failed: operator={} item={} chest_id={} chunk_amount={} err={}",
                    player_name, item, t.chest_id, t.amount, err);
                return utils::send_message_to_player(
                    store,
                    player_name,
                    &format!("Removeitem aborted: bot failed chest withdrawal step: {}", err),
                )
                .await;
            }
            Ok(report) => {
                if let Err(e) = store.apply_chest_sync(report) {
                    warn!("[Removeitem] chest sync failed after withdraw: item={} chest_id={} err={}",
                        item, t.chest_id, e);
                }
            }
        }
    }

    let (trade_tx, trade_rx) = tokio::sync::oneshot::channel();
    let trade_send_result = store.bot_tx
        .send(crate::messages::BotInstruction::TradeWithPlayer {
            target_username: player_name.to_string(),
            bot_offers: vec![TradeItem {
                item: item.to_string(),
                amount: qty_i32,
            }],
            player_offers: vec![],
            // Operator offers nothing, so exact-amount enforcement is meaningless.
            require_exact_amount: false,
            flexible_validation: false,
            respond_to: trade_tx,
        })
        .await;

    if let Err(e) = trade_send_result {
        error!("[Removeitem] failed to send trade instruction: operator={} item={} qty={} err={}",
            player_name, item, quantity, e);
        // Rollback: withdrawal already moved items from chests into the bot.
        // Re-deposit each planned chunk back into its source chest via the shared helper.
        let stack_size = store.pairs.get(item).map(|p| p.stack_size).unwrap_or(64);
        let rb = super::super::rollback::deposit_transfers(
            store,
            &preview_withdraw_plan,
            item,
            stack_size,
            "[Removeitem] trade-send-failed",
        )
        .await;
        if rb.has_failures() {
            error!(
                "[Removeitem] CRITICAL: rollback failed after trade-send error — \
                {} item(s) of '{}' are stranded in the bot's inventory and require \
                manual operator recovery. player={} qty_requested={} rb_returned={} rb_failed_steps={}",
                qty_i32 - rb.items_returned,
                item,
                player_name,
                qty_i32,
                rb.items_returned,
                rb.operations_failed,
            );
        }
        return Err(StoreError::BotError(format!("Failed to send trade instruction to bot: {}", e)));
    }

    let trade_result = tokio::time::timeout(tokio::time::Duration::from_millis(store.config.trade_timeout_ms), trade_rx)
        .await
        .map_err(|_| StoreError::TradeTimeout(store.config.trade_timeout_ms / 1000))?
        .map_err(|e| StoreError::BotError(format!("Bot response dropped: {}", e)))?;

    if let Err(err) = &trade_result {
        warn!("[Removeitem] trade failed, rolling back to storage: operator={} item={} qty={} err={}",
            player_name, item, quantity, err);
        // Items are still in the bot's inventory; re-deposit them via the same plan.
        let stack_size = store.pairs.get(item).map(|p| p.stack_size).unwrap_or(64);
        let rb = super::super::rollback::deposit_transfers(
            store,
            &preview_withdraw_plan,
            item,
            stack_size,
            "[Removeitem] trade-failed",
        )
        .await;
        if rb.has_failures() {
            error!(
                "[Removeitem] CRITICAL: rollback failed after trade failure — \
                {} item(s) of '{}' are stranded in the bot's inventory and require \
                manual operator recovery. player={} qty_requested={} trade_error='{}' \
                rb_returned={} rb_failed_steps={}",
                qty_i32 - rb.items_returned,
                item,
                player_name,
                qty_i32,
                err,
                rb.items_returned,
                rb.operations_failed,
            );
            return utils::send_message_to_player(
                store,
                player_name,
                &format!(
                    "Removeitem CRITICAL ERROR: trade failed and rollback also partially failed. \
                    {} item(s) may be stuck in bot inventory. Contact administrator. trade_error='{}'",
                    qty_i32 - rb.items_returned, err
                ),
            )
            .await;
        }

        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Removeitem aborted: trade failed: {}. Items returned to storage.", err),
        )
        .await;
    }

    let new_stock = store.storage.total_item_amount(item);
    let pair = store.expect_pair_mut(item, "removeitem/commit")?;
    pair.item_stock = new_stock;
    assert!(pair.item_stock >= 0,
        "[Removeitem] INVARIANT VIOLATED: item_stock went negative after remove_stock \
        (item={}, stock={}). This indicates a storage accounting bug.",
        item, pair.item_stock);
    store.dirty = true;

    store.trades.push(Trade::new(
        TradeType::RemoveStock,
        ItemId::from_normalized(item.to_string()),
        qty_i32,
        0.0,
        user_uuid.clone(),
    ));

    store.orders.push_back(Order {
        order_type: crate::types::order::OrderType::RemoveItem,
        item: ItemId::from_normalized(item.to_string()),
        amount: qty_i32,
        currency_amount: 0.0,
        user_uuid: user_uuid.clone(),
    });

    info!(
        "[Removeitem] committed: operator={} uuid={} item={} qty={} stock_before={} stock_after={}",
        player_name, user_uuid, item, quantity, stock_before, new_stock
    );

    if let Err(e) = state::assert_invariants(store, "post-removeitem", true) {
        error!("[Removeitem] invariant violation after commit: operator={} item={} err={}",
            player_name, item, e);
        let _ = state::save(store);
    }

    utils::send_message_to_player(
        store,
        player_name,
        &format!("Removed {} {} from storage. Remaining stock: {}", quantity, item, new_stock),
    )
    .await
}

/// Operator command: credit a pair's currency reserve by `amount` diamonds. No
/// trade occurs — this is a pure bookkeeping adjustment, audited via Trade+Order.
pub async fn handle_add_currency(
    store: &mut Store,
    player_name: &str,
    item: &str,
    amount: f64,
) -> Result<(), StoreError> {
    state::assert_invariants(store, "pre-add-currency", false)?;
    let user_uuid = utils::resolve_user_uuid(player_name).await?;
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

    let reserve_before = store.pairs.get(item).map(|p| p.currency_stock).unwrap_or(0.0);
    info!(
        "[AddCurrency] start: operator={} uuid={} item={} amount={:.2} reserve_before={:.2}",
        player_name, user_uuid, item, amount, reserve_before
    );

    let pair = store.expect_pair_mut(item, "add-currency/commit")?;
    pair.currency_stock += amount;
    assert!(pair.currency_stock.is_finite() && pair.currency_stock >= 0.0,
        "[AddCurrency] INVARIANT VIOLATED: currency_stock invalid after add_currency \
        (item={}, stock={}). This indicates a currency accounting bug.",
        item, pair.currency_stock);
    let new_reserve = pair.currency_stock;
    store.dirty = true;

    store.trades.push(Trade::new(
        TradeType::AddCurrency,
        ItemId::from_normalized(item.to_string()),
        0,
        amount,
        user_uuid.clone(),
    ));

    store.orders.push_back(Order {
        order_type: crate::types::order::OrderType::AddCurrency,
        item: ItemId::from_normalized(item.to_string()),
        amount: 0,
        currency_amount: amount,
        user_uuid: user_uuid.clone(),
    });

    info!(
        "[AddCurrency] committed: operator={} uuid={} item={} amount={:.2} reserve_before={:.2} reserve_after={:.2}",
        player_name, user_uuid, item, amount, reserve_before, new_reserve
    );

    if let Err(e) = state::assert_invariants(store, "post-add-currency", true) {
        error!("[AddCurrency] invariant violation after commit: operator={} item={} err={}",
            player_name, item, e);
        let _ = state::save(store);
    }

    utils::send_message_to_player(
        store,
        player_name,
        &format!("Added {:.2} diamonds to {} reserve. New reserve: {:.2}", amount, item, new_reserve),
    )
    .await
}

/// Operator command: debit a pair's currency reserve by `amount` diamonds. No
/// trade occurs — this is a pure bookkeeping adjustment, audited via Trade+Order.
pub async fn handle_remove_currency(
    store: &mut Store,
    player_name: &str,
    item: &str,
    amount: f64,
) -> Result<(), StoreError> {
    state::assert_invariants(store, "pre-remove-currency", false)?;
    let user_uuid = utils::resolve_user_uuid(player_name).await?;
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

    let pair = store.expect_pair(item, "remove-currency/check")?;
    let reserve_before = pair.currency_stock;
    if reserve_before < amount {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Insufficient currency reserve. Available: {:.2}, requested: {:.2}",
                reserve_before, amount
            ),
        )
        .await;
    }

    info!(
        "[RemoveCurrency] start: operator={} uuid={} item={} amount={:.2} reserve_before={:.2}",
        player_name, user_uuid, item, amount, reserve_before
    );

    let pair = store.expect_pair_mut(item, "remove-currency/commit")?;
    pair.currency_stock -= amount;
    assert!(pair.currency_stock.is_finite() && pair.currency_stock >= 0.0,
        "[RemoveCurrency] INVARIANT VIOLATED: currency_stock invalid after remove_currency \
        (item={}, stock={}). This indicates a currency accounting bug.",
        item, pair.currency_stock);
    let new_reserve = pair.currency_stock;
    store.dirty = true;

    store.trades.push(Trade::new(
        TradeType::RemoveCurrency,
        ItemId::from_normalized(item.to_string()),
        0,
        amount,
        user_uuid.clone(),
    ));

    store.orders.push_back(Order {
        order_type: crate::types::order::OrderType::RemoveCurrency,
        item: ItemId::from_normalized(item.to_string()),
        amount: 0,
        currency_amount: amount,
        user_uuid: user_uuid.clone(),
    });

    info!(
        "[RemoveCurrency] committed: operator={} uuid={} item={} amount={:.2} reserve_before={:.2} reserve_after={:.2}",
        player_name, user_uuid, item, amount, reserve_before, new_reserve
    );

    if let Err(e) = state::assert_invariants(store, "post-remove-currency", true) {
        error!("[RemoveCurrency] invariant violation after commit: operator={} item={} err={}",
            player_name, item, e);
        let _ = state::save(store);
    }

    utils::send_message_to_player(
        store,
        player_name,
        &format!("Removed {:.2} diamonds from {} reserve. Remaining reserve: {:.2}", amount, item, new_reserve),
    )
    .await
}
