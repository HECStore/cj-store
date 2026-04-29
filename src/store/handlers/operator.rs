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
    let user_uuid = crate::mojang::resolve_user_uuid(player_name)
        .await
        .map_err(StoreError::ValidationError)?;
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

    // Exact-amount enforcement is critical here: the deposit plan was sized for
    // `qty_i32` and our stock accounting assumes that's what entered bot inventory.
    // Accepting a different amount would desync the plan from reality and could
    // leave orphaned items in the bot or under-fill chests.
    match super::super::orders::perform_trade(
        store,
        player_name,
        vec![],
        vec![TradeItem {
            item: item.to_string(),
            amount: qty_i32,
        }],
        true,  // require_exact_amount
        false, // flexible_validation
        "[Additem]",
    )
    .await
    {
        Ok(_) => {}
        Err(StoreError::TradeRejected(err)) => {
            return utils::send_message_to_player(
                store,
                player_name,
                &format!("Additem aborted: trade failed: {}", err),
            )
            .await;
        }
        Err(other) => return Err(other),
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
            let rb_send_result = store.bot_tx
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

            if let Err(e) = rb_send_result {
                error!(
                    "[Additem] CRITICAL: return-to-operator bot_tx send failure (mpsc receiver gone) — \
                    {} item(s) of '{}' stuck in bot inventory; operator={} deposited={} err={}",
                    items_in_bot_inventory, item, player_name, items_deposited, e
                );
                return Err(StoreError::BotSendFailed(format!(
                    "return-to-operator trade instruction after deposit failure ({} {} stuck in bot): {}",
                    items_in_bot_inventory, item, e
                )));
            }

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
                Ok(Ok(Err(err))) => {
                    error!(
                        "[Additem] CRITICAL: return-to-operator trade rejected by bot (structured failure) — \
                        {} item(s) of '{}' stuck in bot inventory; operator={} deposited={} bot_err={}",
                        items_in_bot_inventory, item, player_name, items_deposited, err
                    );
                    return utils::send_message_to_player(
                        store,
                        player_name,
                        &format!(
                            "Additem CRITICAL ERROR: {}. {} items stuck in bot inventory (bot rejected return trade: {}). Contact administrator. {} items were stored.",
                            failed_reason, items_in_bot_inventory, err, items_deposited
                        ),
                    ).await;
                }
                Ok(Err(e)) => {
                    error!(
                        "[Additem] CRITICAL: return-to-operator oneshot dropped before reply (bot dropped response sender, likely crashed) — \
                        {} item(s) of '{}' stuck in bot inventory; operator={} deposited={} err={}",
                        items_in_bot_inventory, item, player_name, items_deposited, e
                    );
                    return utils::send_message_to_player(
                        store,
                        player_name,
                        &format!(
                            "Additem CRITICAL ERROR: {}. {} items stuck in bot inventory (bot dropped response). Contact administrator. {} items were stored.",
                            failed_reason, items_in_bot_inventory, items_deposited
                        ),
                    ).await;
                }
                Err(_) => {
                    error!(
                        "[Additem] CRITICAL: return-to-operator trade timed out after {}ms — \
                        {} item(s) of '{}' stuck in bot inventory; operator={} deposited={}",
                        store.config.trade_timeout_ms, items_in_bot_inventory, item, player_name, items_deposited
                    );
                    return utils::send_message_to_player(
                        store,
                        player_name,
                        &format!(
                            "Additem CRITICAL ERROR: {}. {} items stuck in bot inventory (return trade timed out). Contact administrator. {} items were stored.",
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
    let user_uuid = crate::mojang::resolve_user_uuid(player_name)
        .await
        .map_err(StoreError::ValidationError)?;
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
            .map_err(|e| StoreError::BotSendFailed(e.to_string()))?;

        let bot_result = tokio::time::timeout(tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS), rx)
            .await
            .map_err(|_| StoreError::ChestTimeout { after_ms: CHEST_OP_TIMEOUT_SECS.saturating_mul(1000) })?
            .map_err(|e| StoreError::BotResponseDropped(e.to_string()))?;

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

    // Operator offers nothing, so exact-amount enforcement is meaningless.
    let trade_outcome = super::super::orders::perform_trade(
        store,
        player_name,
        vec![TradeItem {
            item: item.to_string(),
            amount: qty_i32,
        }],
        vec![],
        false, // require_exact_amount
        false, // flexible_validation
        "[Removeitem]",
    )
    .await;

    match trade_outcome {
        Ok(_) => {}
        Err(StoreError::BotDisconnected) => {
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
            return Err(StoreError::BotDisconnected);
        }
        Err(StoreError::TradeRejected(err)) => {
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
        Err(other) => return Err(other),
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
    let user_uuid = crate::mojang::resolve_user_uuid(player_name)
        .await
        .map_err(StoreError::ValidationError)?;
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
    let user_uuid = crate::mojang::resolve_user_uuid(player_name)
        .await
        .map_err(StoreError::ValidationError)?;
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

#[cfg(test)]
mod tests {
    //! Tests for operator-only currency/stock adjustment handlers.
    //!
    //! Helpers here are duplicated (intentionally) from `src/store/orders.rs`'s
    //! private `mod tests` — that module is not importable, so we inline the
    //! minimum set needed to spin up a `Store::new_for_test` and a mock bot.
    use super::*;
    use crate::config::Config;
    use crate::messages::{BotInstruction, ChestSyncReport};
    use crate::types::{Chest, Node, Pair, Position, Storage, User};
    use std::collections::HashMap;
    use tokio::sync::mpsc;
    use crate::types::order::OrderType;
    use crate::types::trade::TradeType;
    use crate::types::ItemId;
    use crate::types::{Order, Trade};
    use super::super::super::Store;

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
            chat: crate::config::ChatConfig::default(),
        }
    }

    fn test_uuid(username: &str) -> String {
        let trimmed: String = username.chars().take(12).collect();
        let padded = format!("{:0>12}", trimmed);
        format!("00000000-0000-0000-0000-{}", padded)
    }

    /// Kept (allowed-dead) so future fixers adding rejection / round-trip
    /// tests can reuse it without re-deriving `test_uuid` glue.
    #[allow(dead_code)]
    fn make_user(username: &str, balance: f64) -> (String, User) {
        let uuid = test_uuid(username);
        (
            uuid.clone(),
            User {
                uuid,
                username: username.to_string(),
                balance,
                operator: false,
            },
        )
    }

    /// Build a minimal single-node storage pre-seeded with `stock` items of
    /// `item` in chest index 2 (a non-reserved chest of node 0).
    fn make_storage(item: &str, stock: i32) -> Storage {
        let origin = Position { x: 0, y: 64, z: 0 };
        let mut storage = Storage::new(&origin);
        let node = Node::new(0, &origin);
        storage.nodes.push(node);
        let chest: &mut Chest = &mut storage.nodes[0].chests[2];
        chest.item = ItemId::from_normalized(item.to_string());
        chest.amounts = vec![0; crate::constants::DOUBLE_CHEST_SLOTS];
        chest.amounts[0] = stock;
        storage
    }

    fn make_pair(item: &str, item_stock: i32, currency_stock: f64) -> (String, Pair) {
        (
            item.to_string(),
            Pair {
                item: ItemId::from_normalized(item.to_string()),
                stack_size: 64,
                item_stock,
                currency_stock,
            },
        )
    }

    /// Spawn a mock bot task that auto-responds to every `BotInstruction`. Only
    /// `Whisper` is exercised by `handle_add_currency`; we cover the other
    /// variants too so the channel never blocks a future test addition.
    fn spawn_mock_bot(mut rx: mpsc::Receiver<BotInstruction>) {
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                match msg {
                    BotInstruction::Whisper { respond_to, .. } => {
                        let _ = respond_to.send(Ok(()));
                    }
                    BotInstruction::InteractWithChestAndSync {
                        target_chest,
                        action,
                        respond_to,
                        ..
                    } => {
                        let (item, delta) = match action {
                            crate::messages::ChestAction::Withdraw {
                                item, amount, ..
                            } => (item, -amount),
                            crate::messages::ChestAction::Deposit {
                                item, amount, ..
                            } => (item, amount),
                        };
                        let mut amounts = [-1i32; crate::constants::DOUBLE_CHEST_SLOTS];
                        let prior = target_chest.amounts.first().copied().unwrap_or(0);
                        amounts[0] = (prior + delta).max(0);
                        let _ = respond_to.send(Ok(ChestSyncReport {
                            chest_id: target_chest.id,
                            item,
                            amounts,
                        }));
                    }
                    BotInstruction::TradeWithPlayer {
                        bot_offers: _,
                        player_offers,
                        respond_to,
                        ..
                    } => {
                        let _ = respond_to.send(Ok(player_offers));
                    }
                    _ => {}
                }
            }
        });
    }

    #[tokio::test]
    async fn add_currency_happy_path_increments_stock_and_appends_audit() {
        let (tx, rx) = mpsc::channel(64);
        spawn_mock_bot(rx);

        // Pair item_stock must match physical Storage for `assert_invariants`.
        // We seed 100 cobblestone in storage and 500.0 currency reserve.
        let item = "cobblestone";
        let mut pairs = HashMap::new();
        let (k, p) = make_pair(item, 100, 500.0);
        pairs.insert(k, p);

        let storage = make_storage(item, 100);
        let mut store = Store::new_for_test(
            tx,
            test_config(),
            pairs,
            HashMap::new(),
            storage,
        );

        let result = handle_add_currency(&mut store, "Alice", item, 250.0).await;
        assert!(result.is_ok(), "handle_add_currency failed: {:?}", result);

        // Currency reserve incremented exactly.
        let after = store.pairs.get(item).expect("pair must exist").currency_stock;
        assert!(
            (after - 750.0).abs() < 1e-9,
            "currency_stock expected 750.0 (500 + 250), got {}",
            after
        );

        // Trade audit record matches contract.
        let trade: &Trade = store.trades.last().expect("trade must be appended");
        assert!(matches!(trade.trade_type, TradeType::AddCurrency));
        assert!(
            (trade.amount_currency - 250.0).abs() < 1e-9,
            "trade.amount_currency expected 250.0, got {}",
            trade.amount_currency
        );
        assert_eq!(trade.amount, 0);
        assert_eq!(trade.item.as_str(), item);
        assert_eq!(trade.user_uuid, test_uuid("Alice"));

        // Order audit record matches contract.
        let order: &Order = store.orders.back().expect("order must be appended");
        assert!(matches!(order.order_type, OrderType::AddCurrency));
        assert_eq!(order.amount, 0);
        assert!(
            (order.currency_amount - 250.0).abs() < 1e-9,
            "order.currency_amount expected 250.0, got {}",
            order.currency_amount
        );
        assert_eq!(order.item.as_str(), item);
        assert_eq!(order.user_uuid, test_uuid("Alice"));

        // Mutation flag set so persistence layer would flush on next tick.
        assert!(store.dirty, "store.dirty must be true after mutation");
    }

    /// Build a Store seeded with one pair (`item`, item_stock=100, currency_stock=500.0)
    /// and matching physical storage so `assert_invariants` passes. Returns the store
    /// plus the seeded item name. Mock bot is spawned and channel kept alive.
    fn rejection_test_store(item: &str) -> Store {
        let (tx, rx) = mpsc::channel(64);
        spawn_mock_bot(rx);

        let mut pairs = HashMap::new();
        let (k, p) = make_pair(item, 100, 500.0);
        pairs.insert(k, p);

        let storage = make_storage(item, 100);
        Store::new_for_test(tx, test_config(), pairs, HashMap::new(), storage)
    }

    #[tokio::test]
    async fn add_currency_unknown_item_does_not_mutate() {
        let item = "cobblestone";
        let mut store = rejection_test_store(item);

        // Snapshot the pair map BEFORE the call so we can prove it's unchanged.
        let pairs_before = store.pairs.clone();

        let result = handle_add_currency(&mut store, "Alice", "diamond_block", 50.0).await;
        assert!(result.is_ok(), "handle_add_currency should whisper, not error: {:?}", result);

        // Pair map untouched (no new entry inserted, no existing one mutated).
        assert_eq!(
            store.pairs.len(),
            pairs_before.len(),
            "pair map size changed after unknown-item rejection"
        );
        assert!(
            !store.pairs.contains_key("diamond_block"),
            "unknown item must not be inserted into pairs"
        );
        let seeded = store.pairs.get(item).expect("seeded pair must survive");
        assert!(
            (seeded.currency_stock - 500.0).abs() < 1e-9,
            "seeded currency_stock must remain 500.0, got {}",
            seeded.currency_stock
        );

        assert!(store.trades.is_empty(), "no trade audit on unknown-item rejection");
        assert!(store.orders.is_empty(), "no order audit on unknown-item rejection");
        assert!(!store.dirty, "store.dirty must remain false on rejection");
    }

    #[tokio::test]
    async fn add_currency_zero_amount_rejected() {
        let item = "cobblestone";
        let mut store = rejection_test_store(item);

        let result = handle_add_currency(&mut store, "Alice", item, 0.0).await;
        assert!(result.is_ok(), "handle_add_currency should whisper, not error: {:?}", result);

        let after = store.pairs.get(item).expect("pair must exist").currency_stock;
        assert!(
            (after - 500.0).abs() < 1e-9,
            "currency_stock must remain 500.0 on zero-amount reject, got {}",
            after
        );
        assert!(store.trades.is_empty(), "no trade audit on zero-amount rejection");
        assert!(store.orders.is_empty(), "no order audit on zero-amount rejection");
        assert!(!store.dirty, "store.dirty must remain false on rejection");
    }

    #[tokio::test]
    async fn add_currency_negative_amount_rejected() {
        let item = "cobblestone";
        let mut store = rejection_test_store(item);

        let result = handle_add_currency(&mut store, "Alice", item, -1.0).await;
        assert!(result.is_ok(), "handle_add_currency should whisper, not error: {:?}", result);

        let after = store.pairs.get(item).expect("pair must exist").currency_stock;
        assert!(
            (after - 500.0).abs() < 1e-9,
            "currency_stock must remain 500.0 on negative-amount reject, got {}",
            after
        );
        assert!(store.trades.is_empty(), "no trade audit on negative-amount rejection");
        assert!(store.orders.is_empty(), "no order audit on negative-amount rejection");
        assert!(!store.dirty, "store.dirty must remain false on rejection");
    }

    #[tokio::test]
    async fn add_currency_nan_amount_rejected() {
        let item = "cobblestone";
        let mut store = rejection_test_store(item);

        let result = handle_add_currency(&mut store, "Alice", item, f64::NAN).await;
        assert!(result.is_ok(), "handle_add_currency should whisper, not error: {:?}", result);

        let after = store.pairs.get(item).expect("pair must exist").currency_stock;
        assert!(
            (after - 500.0).abs() < 1e-9,
            "currency_stock must remain 500.0 on NaN-amount reject, got {}",
            after
        );
        assert!(store.trades.is_empty(), "no trade audit on NaN-amount rejection");
        assert!(store.orders.is_empty(), "no order audit on NaN-amount rejection");
        assert!(!store.dirty, "store.dirty must remain false on rejection");
    }

    #[tokio::test]
    async fn add_currency_infinite_amount_rejected() {
        let item = "cobblestone";
        let mut store = rejection_test_store(item);

        let result = handle_add_currency(&mut store, "Alice", item, f64::INFINITY).await;
        assert!(result.is_ok(), "handle_add_currency should whisper, not error: {:?}", result);

        let after = store.pairs.get(item).expect("pair must exist").currency_stock;
        assert!(
            (after - 500.0).abs() < 1e-9,
            "currency_stock must remain 500.0 on infinite-amount reject, got {}",
            after
        );
        assert!(store.trades.is_empty(), "no trade audit on infinite-amount rejection");
        assert!(store.orders.is_empty(), "no order audit on infinite-amount rejection");
        assert!(!store.dirty, "store.dirty must remain false on rejection");
    }

    // ======================================================================
    // handle_remove_currency tests
    // ----------------------------------------------------------------------
    // `handle_remove_currency` is the only path that drains a pair's currency
    // reserve outside of sell trades, so the exact-reserve boundary
    // (reserve == amount) is the most likely off-by-one site. We seed
    // currency_stock=1000.0 / 500.0 / 100.0 depending on the scenario and
    // exercise: happy path, exact-reserve drain, over-reserve rejection,
    // unknown item, and the four amount-validation rejections.
    // ======================================================================

    /// Build a Store seeded with one pair and configurable initial currency reserve.
    /// Mirrors `rejection_test_store` but lets each remove-currency test pick its own
    /// reserve so we can probe the exact-reserve and over-reserve boundaries.
    fn remove_currency_test_store(item: &str, reserve: f64) -> Store {
        let (tx, rx) = mpsc::channel(64);
        spawn_mock_bot(rx);

        let mut pairs = HashMap::new();
        let (k, p) = make_pair(item, 100, reserve);
        pairs.insert(k, p);

        let storage = make_storage(item, 100);
        Store::new_for_test(tx, test_config(), pairs, HashMap::new(), storage)
    }

    #[tokio::test]
    async fn remove_currency_happy_path_decrements_stock_and_appends_audit() {
        let item = "cobblestone";
        let mut store = remove_currency_test_store(item, 1000.0);

        let result = handle_remove_currency(&mut store, "Alice", item, 250.0).await;
        assert!(result.is_ok(), "handle_remove_currency failed: {:?}", result);

        // Currency reserve decremented exactly: 1000 - 250 = 750.
        let after = store.pairs.get(item).expect("pair must exist").currency_stock;
        assert!(
            (after - 750.0).abs() < 1e-9,
            "currency_stock expected 750.0 (1000 - 250), got {}",
            after
        );

        // Exactly one Trade audit row, of type RemoveCurrency, with the
        // currency_amount in `amount_currency` and zero in `amount`.
        assert_eq!(store.trades.len(), 1, "expected exactly one trade audit row");
        let trade: &Trade = store.trades.last().expect("trade must be appended");
        assert!(matches!(trade.trade_type, TradeType::RemoveCurrency));
        assert!(
            (trade.amount_currency - 250.0).abs() < 1e-9,
            "trade.amount_currency expected 250.0, got {}",
            trade.amount_currency
        );
        assert_eq!(trade.amount, 0);
        assert_eq!(trade.item.as_str(), item);
        assert_eq!(trade.user_uuid, test_uuid("Alice"));

        // Exactly one Order audit row of type RemoveCurrency.
        assert_eq!(store.orders.len(), 1, "expected exactly one order audit row");
        let order: &Order = store.orders.back().expect("order must be appended");
        assert!(matches!(order.order_type, OrderType::RemoveCurrency));
        assert_eq!(order.amount, 0);
        assert!(
            (order.currency_amount - 250.0).abs() < 1e-9,
            "order.currency_amount expected 250.0, got {}",
            order.currency_amount
        );
        assert_eq!(order.item.as_str(), item);
        assert_eq!(order.user_uuid, test_uuid("Alice"));

        // Mutation flag set so persistence layer would flush on next tick.
        assert!(store.dirty, "store.dirty must be true after mutation");
    }

    #[tokio::test]
    async fn remove_currency_exact_reserve_drains_to_zero() {
        // The boundary case: requested amount == available reserve. The handler
        // must succeed (reserve_before >= amount holds with equality) and the
        // post-debit invariant (`stock >= 0.0`) must not panic. Because the
        // arithmetic is `500.0 - 500.0`, IEEE 754 guarantees exact zero, so
        // `assert_eq!` is meaningful here.
        let item = "cobblestone";
        let mut store = remove_currency_test_store(item, 500.0);

        let result = handle_remove_currency(&mut store, "Alice", item, 500.0).await;
        assert!(result.is_ok(), "handle_remove_currency failed: {:?}", result);

        // Exact zero — no epsilon. The assert in the handler would have
        // panicked if the value were negative or non-finite.
        let after = store.pairs.get(item).expect("pair must exist").currency_stock;
        assert_eq!(after, 0.0, "currency_stock must be EXACTLY 0.0 after exact-reserve drain");

        assert_eq!(store.trades.len(), 1, "expected exactly one trade audit row");
        let trade: &Trade = store.trades.last().expect("trade must be appended");
        assert!(matches!(trade.trade_type, TradeType::RemoveCurrency));
        assert!(
            (trade.amount_currency - 500.0).abs() < 1e-9,
            "trade.amount_currency expected 500.0, got {}",
            trade.amount_currency
        );

        assert_eq!(store.orders.len(), 1, "expected exactly one order audit row");
        let order: &Order = store.orders.back().expect("order must be appended");
        assert!(matches!(order.order_type, OrderType::RemoveCurrency));

        assert!(store.dirty, "store.dirty must be true after mutation");
    }

    #[tokio::test]
    async fn remove_currency_over_reserve_rejected() {
        // The just-over boundary: reserve=100.0, requested=100.01. The handler
        // must reject (whisper, no mutation) — this is the off-by-one twin of
        // `exact_reserve_drains_to_zero`.
        let item = "cobblestone";
        let mut store = remove_currency_test_store(item, 100.0);

        let result = handle_remove_currency(&mut store, "Alice", item, 100.01).await;
        assert!(result.is_ok(), "handle_remove_currency should whisper, not error: {:?}", result);

        let after = store.pairs.get(item).expect("pair must exist").currency_stock;
        assert!(
            (after - 100.0).abs() < 1e-9,
            "currency_stock must remain 100.0 on over-reserve reject, got {}",
            after
        );
        assert!(store.trades.is_empty(), "no trade audit on over-reserve rejection");
        assert!(store.orders.is_empty(), "no order audit on over-reserve rejection");
        assert!(!store.dirty, "store.dirty must remain false on rejection");
    }

    #[tokio::test]
    async fn remove_currency_unknown_item_does_not_mutate() {
        let item = "cobblestone";
        let mut store = rejection_test_store(item);

        // Snapshot the pair map BEFORE the call so we can prove it's unchanged.
        let pairs_before = store.pairs.clone();

        let result = handle_remove_currency(&mut store, "Alice", "diamond_block", 50.0).await;
        assert!(result.is_ok(), "handle_remove_currency should whisper, not error: {:?}", result);

        // Pair map untouched (no new entry inserted, no existing one mutated).
        assert_eq!(
            store.pairs.len(),
            pairs_before.len(),
            "pair map size changed after unknown-item rejection"
        );
        assert!(
            !store.pairs.contains_key("diamond_block"),
            "unknown item must not be inserted into pairs"
        );
        let seeded = store.pairs.get(item).expect("seeded pair must survive");
        assert!(
            (seeded.currency_stock - 500.0).abs() < 1e-9,
            "seeded currency_stock must remain 500.0, got {}",
            seeded.currency_stock
        );

        assert!(store.trades.is_empty(), "no trade audit on unknown-item rejection");
        assert!(store.orders.is_empty(), "no order audit on unknown-item rejection");
        assert!(!store.dirty, "store.dirty must remain false on rejection");
    }

    #[tokio::test]
    async fn remove_currency_zero_amount_rejected() {
        let item = "cobblestone";
        let mut store = rejection_test_store(item);

        let result = handle_remove_currency(&mut store, "Alice", item, 0.0).await;
        assert!(result.is_ok(), "handle_remove_currency should whisper, not error: {:?}", result);

        let after = store.pairs.get(item).expect("pair must exist").currency_stock;
        assert!(
            (after - 500.0).abs() < 1e-9,
            "currency_stock must remain 500.0 on zero-amount reject, got {}",
            after
        );
        assert!(store.trades.is_empty(), "no trade audit on zero-amount rejection");
        assert!(store.orders.is_empty(), "no order audit on zero-amount rejection");
        assert!(!store.dirty, "store.dirty must remain false on rejection");
    }

    #[tokio::test]
    async fn remove_currency_negative_amount_rejected() {
        let item = "cobblestone";
        let mut store = rejection_test_store(item);

        let result = handle_remove_currency(&mut store, "Alice", item, -1.0).await;
        assert!(result.is_ok(), "handle_remove_currency should whisper, not error: {:?}", result);

        let after = store.pairs.get(item).expect("pair must exist").currency_stock;
        assert!(
            (after - 500.0).abs() < 1e-9,
            "currency_stock must remain 500.0 on negative-amount reject, got {}",
            after
        );
        assert!(store.trades.is_empty(), "no trade audit on negative-amount rejection");
        assert!(store.orders.is_empty(), "no order audit on negative-amount rejection");
        assert!(!store.dirty, "store.dirty must remain false on rejection");
    }

    #[tokio::test]
    async fn remove_currency_nan_amount_rejected() {
        let item = "cobblestone";
        let mut store = rejection_test_store(item);

        let result = handle_remove_currency(&mut store, "Alice", item, f64::NAN).await;
        assert!(result.is_ok(), "handle_remove_currency should whisper, not error: {:?}", result);

        let after = store.pairs.get(item).expect("pair must exist").currency_stock;
        assert!(
            (after - 500.0).abs() < 1e-9,
            "currency_stock must remain 500.0 on NaN-amount reject, got {}",
            after
        );
        assert!(store.trades.is_empty(), "no trade audit on NaN-amount rejection");
        assert!(store.orders.is_empty(), "no order audit on NaN-amount rejection");
        assert!(!store.dirty, "store.dirty must remain false on rejection");
    }

    #[tokio::test]
    async fn remove_currency_infinite_amount_rejected() {
        let item = "cobblestone";
        let mut store = rejection_test_store(item);

        let result = handle_remove_currency(&mut store, "Alice", item, f64::INFINITY).await;
        assert!(result.is_ok(), "handle_remove_currency should whisper, not error: {:?}", result);

        let after = store.pairs.get(item).expect("pair must exist").currency_stock;
        assert!(
            (after - 500.0).abs() < 1e-9,
            "currency_stock must remain 500.0 on infinite-amount reject, got {}",
            after
        );
        assert!(store.trades.is_empty(), "no trade audit on infinite-amount rejection");
        assert!(store.orders.is_empty(), "no order audit on infinite-amount rejection");
        assert!(!store.dirty, "store.dirty must remain false on rejection");
    }

    // ======================================================================
    // Symmetry / round-trip probe
    // ----------------------------------------------------------------------
    // `handle_add_currency` and `handle_remove_currency` are mirror twins.
    // An isolated test for either side cannot catch the kind of drift that
    // breaks symmetry: a sign inversion in one handler, a swapped
    // Trade/Order append order in only one of the two paths, an audit row
    // appended on the wrong side, etc. The round-trip below seeds an
    // integer-valued reserve, applies +250 then -250, and asserts the pair
    // is bit-exactly restored AND that exactly two audit rows landed in
    // the (AddCurrency, RemoveCurrency) order on both the Trade log and
    // the Order queue.
    // ======================================================================
    #[tokio::test]
    async fn add_then_remove_currency_round_trip_restores_reserve() {
        let item = "cobblestone";
        // Seed reserve at 1000.0 — an integer-valued f64, so 1000 + 250 - 250
        // is bit-exact under IEEE 754. Test uses `assert_eq!` accordingly.
        let mut store = remove_currency_test_store(item, 1000.0);

        let trades_before = store.trades.len();
        let orders_before = store.orders.len();

        let add_result = handle_add_currency(&mut store, "Op", item, 250.0).await;
        assert!(add_result.is_ok(), "handle_add_currency failed: {:?}", add_result);

        let remove_result = handle_remove_currency(&mut store, "Op", item, 250.0).await;
        assert!(remove_result.is_ok(), "handle_remove_currency failed: {:?}", remove_result);

        // Bit-exact: integer-typed f64 round trip must restore the reserve.
        let after = store.pairs.get(item).expect("pair must exist").currency_stock;
        assert_eq!(
            after, 1000.0,
            "currency_stock must be EXACTLY 1000.0 after add+remove round trip"
        );

        // Exactly two new Trade rows appended, in (AddCurrency, RemoveCurrency) order.
        assert_eq!(
            store.trades.len(),
            trades_before + 2,
            "expected exactly two trades appended (one Add, one Remove)"
        );
        let add_trade = &store.trades[trades_before];
        let remove_trade = &store.trades[trades_before + 1];
        assert!(
            matches!(add_trade.trade_type, TradeType::AddCurrency),
            "first appended trade must be AddCurrency, got {:?}",
            add_trade.trade_type
        );
        assert!(
            matches!(remove_trade.trade_type, TradeType::RemoveCurrency),
            "second appended trade must be RemoveCurrency, got {:?}",
            remove_trade.trade_type
        );

        // Exactly two new Order rows appended, in (AddCurrency, RemoveCurrency) order.
        assert_eq!(
            store.orders.len(),
            orders_before + 2,
            "expected exactly two orders appended (one Add, one Remove)"
        );
        let add_order = &store.orders[orders_before];
        let remove_order = &store.orders[orders_before + 1];
        assert!(
            matches!(add_order.order_type, OrderType::AddCurrency),
            "first appended order must be AddCurrency, got {:?}",
            add_order.order_type
        );
        assert!(
            matches!(remove_order.order_type, OrderType::RemoveCurrency),
            "second appended order must be RemoveCurrency, got {:?}",
            remove_order.order_type
        );

        assert!(store.dirty, "store.dirty must be true after mutation");
    }
}
