//! Order execution handlers (buy/sell/deposit/withdraw)

use tokio::sync::oneshot;
use tracing::{debug, error, info, warn};

use crate::constants::CHEST_OP_TIMEOUT_SECS;
use crate::messages::{BotInstruction, ChestAction, QueuedOrderType, TradeItem};
use crate::types::{Order, Trade, TradeType};
use super::{Store, pricing, state, utils};
use super::queue::QueuedOrder;

/// Handle buy orders
pub async fn handle_buy_order(
    store: &mut Store,
    player_name: &str,
    item: &str,
    quantity: u32,
) -> Result<(), String> {
    info!("[Buy] === STARTING BUY ORDER === player={} item={} qty={}", player_name, item, quantity);
    debug!("[Buy] Asserting pre-buy invariants...");
    state::assert_invariants(store, "pre-buy", false)?;
    debug!("[Buy] Pre-buy invariants passed");
    
    debug!("[Buy] Resolving user UUID for {}", player_name);
    let user_uuid = utils::resolve_user_uuid(store, player_name).await?;
    info!("[Buy] User {} resolved to UUID {}", player_name, user_uuid);
    utils::ensure_user_exists(store, player_name, &user_uuid);

    // Check if pair exists
    if !store.pairs.contains_key(item) {
        warn!(
            "Player {} attempted to buy unavailable item: {}",
            player_name, item
        );
        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Item '{}' is not available for trading", item),
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

    // Calculate total cost using constant product AMM formula
    let total_cost = match pricing::calculate_buy_cost(store, item, qty_i32) {
        Some(cost) => cost,
        None => {
            // Determine the reason for failure
            let pair = store.pairs.get(item).unwrap();
            if qty_i32 >= pair.item_stock {
                return utils::send_message_to_player(
                    store,
                    player_name,
                    &format!(
                        "Cannot buy {} {} - would exceed available stock ({}). Try a smaller amount.",
                        qty_i32, item, pair.item_stock
                    ),
                )
                .await;
            }
            return utils::send_message_to_player(
                store,
                player_name,
                &format!("Item '{}' is not available for trading (no stock or reserves).", item),
            )
            .await;
        }
    };

    if !total_cost.is_finite() || total_cost <= 0.0 {
        return utils::send_message_to_player(store, player_name, "Internal error: computed price is invalid.")
            .await;
    }

    // Physical stock check: ensure storage can actually fulfill.
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

    let pair = store.pairs.get(item).unwrap();
    if pair.item_stock < qty_i32 {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Not enough stock for '{}'. Available: {}, requested: {}",
                item, pair.item_stock, qty_i32
            ),
        )
        .await;
    }

    // Check if player has enough balance OR will offer enough diamonds in trade
    let user_balance = store.users.get(&user_uuid).map(|u| u.balance).unwrap_or(0.0);
    // Hybrid payment model: diamonds are whole-unit items in the trade GUI, but cost is a float.
    // Strategy: use the player's float balance first, then ceil the remainder into whole diamonds.
    // Any fractional overpayment (e.g. need 1.3 diamonds, player pays 2) is credited back to balance below.
    let balance_shortfall = total_cost - user_balance;
    let diamonds_to_offer = if balance_shortfall > 0.0 {
        let ceil_value = balance_shortfall.ceil();
        // Validate that the result fits in i32 (max 2,147,483,647 diamonds)
        if ceil_value > i32::MAX as f64 {
            return utils::send_message_to_player(
                store,
                player_name,
                "Transaction amount too large (exceeds maximum diamond limit)",
            )
            .await;
        }
        ceil_value as i32
    } else {
        0
    };
    
    // Validate: player must have enough balance + diamonds to offer
    if user_balance + (diamonds_to_offer as f64) < total_cost {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Insufficient funds. Required: {:.2}, Available balance: {:.2}, Need to offer in trade: {} diamonds",
                total_cost, user_balance, diamonds_to_offer
            ),
        )
        .await;
    }

    // **Transaction Planning Phase**: Clone storage and run withdraw_plan against the clone.
    // This produces a concrete list of chest operations (which chest, how much) without touching
    // real state. If planning fails we can bail cleanly. The actual storage is mutated later by
    // apply_chest_sync once the bot confirms each physical operation (plan-then-commit pattern).
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

    // **Execution Phase**: Bot performs physical operations.
    info!("[Buy] Starting withdrawal execution phase: {} chest operations", preview_withdraw_plan.len());
    let mut withdraw_step = 0;
    for t in &preview_withdraw_plan {
        withdraw_step += 1;
        info!("[Buy] Withdrawal step {}/{}: chest {} at ({},{},{}), {}x {}",
              withdraw_step, preview_withdraw_plan.len(),
              t.chest_id, t.position.x, t.position.y, t.position.z, t.amount, t.item);
        
        let node_position = store.get_node_position(t.chest_id);
        let chest = crate::types::Chest {
            id: t.chest_id,
            node_id: t.chest_id / 4,
            index: t.chest_id % 4,
            position: t.position,
            item: t.item.clone(),
            amounts: vec![0; crate::types::Storage::SLOTS_PER_CHEST],
        };

        let (tx, rx) = oneshot::channel();
        debug!("[Buy] Sending InteractWithChestAndSync instruction to bot for chest {}...", t.chest_id);
        let send_result = store.bot_tx
            .send(BotInstruction::InteractWithChestAndSync {
                target_chest: chest,
                node_position,
                action: ChestAction::Withdraw {
                    item: item.to_string(),
                    amount: t.amount,
                    to_player: None,
                    stack_size: store.pairs.get(item).map(|p| p.stack_size).unwrap_or(64),
                },
                respond_to: tx,
            })
            .await;
        
        if let Err(e) = send_result {
            error!("[Buy] FAILED to send chest instruction to bot: {}", e);
            return Err(format!("Failed to send chest instruction to bot: {}", e));
        }
        debug!("[Buy] Instruction sent, awaiting bot response (timeout {}s)...", CHEST_OP_TIMEOUT_SECS);
        
        let timeout_start = std::time::Instant::now();
        let timeout_result = tokio::time::timeout(tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS), rx).await;
        let timeout_elapsed = timeout_start.elapsed();
        
        let bot_result = match timeout_result {
            Ok(channel_result) => {
                debug!("[Buy] Received response from channel after {:.2}s", timeout_elapsed.as_secs_f64());
                match channel_result {
                    Ok(result) => result,
                    Err(e) => {
                        error!("[Buy] Channel DROPPED after {:.2}s: {}", timeout_elapsed.as_secs_f64(), e);
                        return Err(format!("Bot response dropped: {}", e));
                    }
                }
            }
            Err(_) => {
                error!("[Buy] TIMEOUT after {:.2}s waiting for bot response on chest {} withdrawal!", 
                       timeout_elapsed.as_secs_f64(), t.chest_id);
                return Err("Bot timed out performing chest step".to_string());
            }
        };

        match bot_result {
            Err(err) => {
                error!("[Buy] Bot reported error on chest {} withdrawal: {}", t.chest_id, err);
                return utils::send_message_to_player(
                    store,
                    player_name,
                    &format!("Buy aborted: bot failed chest withdrawal step: {}", err),
                )
                .await;
            }
            Ok(report) => {
                info!("[Buy] Chest {} withdrawal succeeded, syncing storage state", report.chest_id);
                if let Err(e) = store.apply_chest_sync(report) {
                    warn!("[Buy] Chest sync failed after withdraw: {}", e);
                }
            }
        }
    }
    info!("[Buy] All {} withdrawal operations completed successfully", preview_withdraw_plan.len());

    // Notify player of the trade details before sending the trade request
    info!("[Buy] Sending trade info message to player...");
    let trade_info_msg = if diamonds_to_offer > 0 {
        format!(
            "Buy {} {}: Total {:.2} diamonds. Please offer {} diamonds in the trade.",
            qty_i32, item, total_cost, diamonds_to_offer
        )
    } else {
        format!(
            "Buy {} {}: Total {:.2} diamonds (paid from balance). No diamonds needed in trade.",
            qty_i32, item, total_cost
        )
    };
    utils::send_message_to_player(store, player_name, &trade_info_msg).await?;

    // Perform trade GUI: bot offers items, player offers diamonds.
    info!("[Buy] Initiating trade: bot offers {}x {}, player should offer {} diamonds", qty_i32, item, diamonds_to_offer);
    let (trade_tx, trade_rx) = oneshot::channel();
    debug!("[Buy] Sending TradeWithPlayer instruction to bot...");
    let trade_send_result = store.bot_tx
        .send(BotInstruction::TradeWithPlayer {
            target_username: player_name.to_string(),
            bot_offers: vec![TradeItem {
                item: item.to_string(),
                amount: qty_i32,
            }],
            player_offers: if diamonds_to_offer > 0 {
                vec![TradeItem {
                    item: "diamond".to_string(),
                    amount: diamonds_to_offer,
                }]
            } else {
                vec![]
            },
            // Buy: accept if player offers at least the required diamonds (surplus OK)
            require_exact_amount: false,
            flexible_validation: false,
            respond_to: trade_tx,
        })
        .await;
    
    if let Err(e) = trade_send_result {
        error!("[Buy] FAILED to send trade instruction to bot: {}", e);
        return Err(format!("Failed to send trade instruction to bot: {}", e));
    }
    info!("[Buy] Trade instruction sent, awaiting trade result (timeout 45s)...");

    let trade_timeout_start = std::time::Instant::now();
    let trade_timeout_result = tokio::time::timeout(tokio::time::Duration::from_secs(45), trade_rx).await;
    let trade_timeout_elapsed = trade_timeout_start.elapsed();
    
    let trade_result = match trade_timeout_result {
        Ok(channel_result) => {
            debug!("[Buy] Trade response received after {:.2}s", trade_timeout_elapsed.as_secs_f64());
            match channel_result {
                Ok(result) => result,
                Err(e) => {
                    error!("[Buy] Trade channel DROPPED after {:.2}s: {}", trade_timeout_elapsed.as_secs_f64(), e);
                    return Err(format!("Bot response dropped: {}", e));
                }
            }
        }
        Err(_) => {
            error!("[Buy] Trade TIMEOUT after {:.2}s!", trade_timeout_elapsed.as_secs_f64());
            return Err("Bot timed out waiting for trade completion".to_string());
        }
    };
    
    // Handle trade result - now returns actual items received
    let actual_received = match trade_result {
        Err(err) => {
            error!("[Buy] Trade FAILED: {} - rolling back withdrawals", err);
            // Rollback: items are currently in the bot's inventory (we already withdrew them).
            // Walk the original withdraw plan in order and deposit each chunk back into the same
            // chest it came from. We track success/failure per-step but continue on failure —
            // items stuck in bot inventory after a rollback failure require operator intervention.
            info!("[Buy] Rolling back {} withdrawal operations to return {} items...", preview_withdraw_plan.len(), qty_i32);
            
            let mut rollback_success = 0;
            let mut rollback_failed = 0;
            let mut items_returned = 0i32;
            
            for (step, t) in preview_withdraw_plan.iter().enumerate() {
                info!("[Buy] Rollback step {}/{}: returning {}x {} to chest {}", 
                      step + 1, preview_withdraw_plan.len(), t.amount, t.item, t.chest_id);
                
                let node_position = store.get_node_position(t.chest_id);
                let chest = crate::types::Chest {
                    id: t.chest_id,
                    node_id: t.chest_id / 4,
                    index: t.chest_id % 4,
                    position: t.position,
                    item: t.item.clone(),
                    amounts: vec![0; crate::types::Storage::SLOTS_PER_CHEST],
                };
                let (tx, rx) = oneshot::channel();
                let send_result = store
                    .bot_tx
                    .send(BotInstruction::InteractWithChestAndSync {
                        target_chest: chest,
                        node_position,
                        action: ChestAction::Deposit {
                            item: item.to_string(),
                            amount: t.amount,
                            from_player: None,
                            stack_size: store.pairs.get(item).map(|p| p.stack_size).unwrap_or(64),
                        },
                        respond_to: tx,
                    })
                    .await;
                
                if let Err(e) = send_result {
                    error!("[Buy] Rollback step {} FAILED to send instruction: {}", step + 1, e);
                    rollback_failed += 1;
                    continue;
                }
                
                match tokio::time::timeout(tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS), rx).await {
                    Ok(Ok(Ok(report))) => {
                        info!("[Buy] Rollback step {} succeeded for chest {}", step + 1, report.chest_id);
                        if let Err(e) = store.apply_chest_sync(report) {
                            warn!("[Buy] Rollback step {} chest sync warning: {}", step + 1, e);
                        }
                        rollback_success += 1;
                        items_returned += t.amount;
                    }
                    Ok(Ok(Err(e))) => {
                        error!("[Buy] Rollback step {} bot returned error: {}", step + 1, e);
                        rollback_failed += 1;
                    }
                    Ok(Err(e)) => {
                        error!("[Buy] Rollback step {} channel dropped: {}", step + 1, e);
                        rollback_failed += 1;
                    }
                    Err(_) => {
                        error!("[Buy] Rollback step {} TIMEOUT after {}s", step + 1, CHEST_OP_TIMEOUT_SECS);
                        rollback_failed += 1;
                    }
                }
            }
            
            let rollback_msg = if rollback_failed > 0 {
                format!(
                    "Buy aborted: trade failed: {}. Rollback: {} items returned, {} operations failed - some items may remain in bot inventory.",
                    err, items_returned, rollback_failed
                )
            } else {
                format!("Buy aborted: trade failed: {} (items rolled back to storage)", err)
            };
            
            info!("[Buy] Rollback complete: {}/{} operations succeeded, {} items returned", 
                  rollback_success, preview_withdraw_plan.len(), items_returned);

            return utils::send_message_to_player(store, player_name, &rollback_msg).await;
        }
        Ok(received) => received,
    };
    
    // Calculate actual diamonds received from trade
    let diamonds_received: i32 = actual_received
        .iter()
        .filter(|t| t.item == "diamond")
        .map(|t| t.amount)
        .sum();
    
    // Re-read current balance (may have changed since we calculated diamonds_to_offer)
    let current_balance = store.users.get(&user_uuid).map(|u| u.balance).unwrap_or(0.0);

    // Final payment validation: balance + actual diamonds received from the trade must cover cost.
    // Because the buy trade uses require_exact_amount=false, the player could theoretically offer
    // fewer diamonds than requested. We recheck here and rollback if they shortchanged us.
    let total_available = (diamonds_received as f64) + current_balance;
    if total_available < total_cost {
        // Insufficient payment - need to rollback
        // This is a serious error since trade already completed: items left the bot's inventory
        // to the player's, and we also have some diamonds. Both need to be unwound.
        error!(
            "Insufficient payment after trade: received {} diamonds + {:.2} balance = {:.2}, need {:.2}",
            diamonds_received, current_balance, total_available, total_cost
        );
        
        // Rollback: deposit items back into storage
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
            let (tx, rx) = oneshot::channel();
            let _ = store
                .bot_tx
                .send(BotInstruction::InteractWithChestAndSync {
                    target_chest: chest,
                    node_position,
                    action: ChestAction::Deposit {
                        item: item.to_string(),
                        amount: t.amount,
                        from_player: None,
                        stack_size: store.pairs.get(item).map(|p| p.stack_size).unwrap_or(64),
                    },
                    respond_to: tx,
                })
                .await;
            if let Ok(Ok(Ok(report))) = tokio::time::timeout(tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS), rx).await {
                let _ = store.apply_chest_sync(report);
            }
        }
        
        // Try to return any diamonds received back to player
        if diamonds_received > 0 {
            warn!("Attempting to return {} diamonds to player after failed payment validation", diamonds_received);
            // The diamonds are in bot inventory - we'll deposit them to storage
            // and the player will need to withdraw them or get a refund manually
        }
        
        return utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Buy aborted: insufficient payment. You paid {} diamonds but need {:.2} total (your balance: {:.2}). Items rolled back.",
                diamonds_received, total_cost, current_balance
            ),
        )
        .await;
    }
    
    info!(
        "Trade payment validated: received {} diamonds + {:.2} balance = {:.2} available, need {:.2}",
        diamonds_received, current_balance, total_available, total_cost
    );

    // Deposit diamonds received from player into storage
    if diamonds_received > 0 {
        let mut sim_diamond_storage = store.storage.clone();
        let diamond_deposit_plan = sim_diamond_storage.deposit_plan("diamond", diamonds_received, 64);
        
        for t in &diamond_deposit_plan {
            let node_position = store.get_node_position(t.chest_id);
            let chest = crate::types::Chest {
                id: t.chest_id,
                node_id: t.chest_id / 4,
                index: t.chest_id % 4,
                position: t.position,
                item: t.item.clone(),
                amounts: vec![0; crate::types::Storage::SLOTS_PER_CHEST],
            };

            let (tx, rx) = oneshot::channel();
            store.bot_tx
                .send(BotInstruction::InteractWithChestAndSync {
                    target_chest: chest,
                    node_position,
                    action: ChestAction::Deposit {
                        item: "diamond".to_string(),
                        amount: t.amount,
                        from_player: None,
                        stack_size: 64, // Diamonds stack to 64
                    },
                    respond_to: tx,
                })
                .await
                .map_err(|e| format!("Failed to send chest instruction to bot: {}", e))?;

            let bot_result = tokio::time::timeout(tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS), rx)
                .await
                .map_err(|_| "Bot timed out depositing diamonds to storage".to_string())?
                .map_err(|e| format!("Bot response dropped: {}", e))?;

            match bot_result {
                Err(err) => {
                    // Diamond deposit failed - log warning but continue (diamonds in bot inventory)
                    warn!("Failed to deposit received diamonds into storage: {} - diamonds remain in bot inventory", err);
                }
                Ok(report) => {
                    if let Err(e) = store.apply_chest_sync(report) {
                        warn!("Chest sync failed after diamond deposit: {}", e);
                    }
                }
            }
        }
    }

    // Commit: update ledgers after bot confirmed all chest operations and synced contents
    let current_stock = store.storage.total_item_amount(item);
    let expected_stock = physical_stock - qty_i32;
    if current_stock != expected_stock {
        warn!(
            "Storage stock mismatch after buy: expected {}, got {} (difference: {})",
            expected_stock, current_stock, expected_stock - current_stock
        );
    }

    // Apply transfer: player paid with balance + diamonds from trade
    // Use the ACTUAL diamonds received, not the originally asked amount
    // This allows flexible payment: player can pay with more/less diamonds and balance covers the rest
    let diamonds_received_f64 = diamonds_received as f64;
    
    // Calculate how much needs to come from balance (negative means surplus)
    // balance_needed > 0: player underpaid with diamonds, deduct from balance
    // balance_needed < 0: player overpaid with diamonds, credit surplus to balance
    let balance_needed = total_cost - diamonds_received_f64;
    let (balance_deduction, surplus) = if balance_needed > 0.0 {
        // Player didn't pay enough diamonds, deduct from balance
        let deduction = balance_needed.min(current_balance);
        store.users.get_mut(&user_uuid).unwrap().balance -= deduction;
        info!(
            "Deducted {:.2} from {}'s balance (paid {} diamonds, needed {:.2} total)",
            deduction, player_name, diamonds_received, total_cost
        );
        (deduction, 0.0)
    } else {
        // Player paid MORE diamonds than needed, credit surplus to balance
        let surplus_amount = -balance_needed; // balance_needed is negative, so negate it
        store.users.get_mut(&user_uuid).unwrap().balance += surplus_amount;
        info!(
            "Credited {:.2} diamond surplus to {}'s balance (paid {} diamonds, needed {:.2})",
            surplus_amount, player_name, diamonds_received, total_cost
        );
        (0.0, surplus_amount)
    };
    
    store.users.get_mut(&user_uuid).unwrap().username = player_name.to_owned();
    store.dirty = true;

    let pair = store.pairs.get_mut(item).unwrap();
    pair.item_stock = store.storage.total_item_amount(item);
    pair.currency_stock += total_cost;
    store.dirty = true;

    // Record trade
    store.trades.push(Trade::new(
        TradeType::Buy,
        item.to_string(),
        qty_i32,
        total_cost,
        user_uuid.clone(),
    ));

    // Record order
    store.orders.push_back(Order {
        order_type: crate::types::order::OrderType::Buy,
        item: item.to_string(),
        amount: qty_i32,
        user_uuid: user_uuid.clone(),
    });

    info!(
        "Executed buy: user={} item={} qty={} total={:.2} (diamonds: {}, balance used: {:.2}, surplus: {:.2})",
        player_name, item, quantity, total_cost, diamonds_received, balance_deduction, surplus
    );

    // Enforce invariants after mutation
    if let Err(e) = state::assert_invariants(store, "post-buy", true) {
        error!("Invariant violation after buy: {}", e);
        let _ = state::save(store);
    }

    let pickup_summary = utils::summarize_transfers(&preview_withdraw_plan, 3);
    let fee_amount = total_cost - (total_cost / (1.0 + store.config.fee));
    
    // Build payment summary message
    let payment_msg = if surplus > 0.001 {
        format!(" {:.2} surplus credited to balance.", surplus)
    } else if balance_deduction > 0.001 {
        format!(" {:.2} deducted from balance.", balance_deduction)
    } else {
        String::new()
    };
    
    utils::send_message_to_player(
        store,
        player_name,
        &format!(
            "Bought {} {} for {:.2} diamonds (fee {:.2}).{} Trade complete. Storage: {}",
            quantity, item, total_cost, fee_amount, payment_msg, pickup_summary
        ),
    )
    .await
}

/// Handle sell orders
pub async fn handle_sell_order(
    store: &mut Store,
    player_name: &str,
    item: &str,
    quantity: u32,
) -> Result<(), String> {
    info!("[Sell] === Starting sell order: {} selling {}x {} ===", player_name, quantity, item);
    
    state::assert_invariants(store, "pre-sell", false)?;
    let user_uuid = utils::resolve_user_uuid(store, player_name).await?;
    utils::ensure_user_exists(store, player_name, &user_uuid);
    info!("[Sell] User {} resolved to UUID {}", player_name, user_uuid);

    // Check if pair exists
    if !store.pairs.contains_key(item) {
        warn!(
            "Player {} attempted to sell unavailable item: {}",
            player_name, item
        );
        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Item '{}' is not available for trading", item),
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

    // Calculate total payout using constant product AMM formula
    let total_payout = match pricing::calculate_sell_payout(store, item, qty_i32) {
        Some(payout) => payout,
        None => {
            return utils::send_message_to_player(
                store,
                player_name,
                &format!("Item '{}' is not available for trading (no stock or reserves).", item),
            )
            .await;
        }
    };

    if !total_payout.is_finite() || total_payout <= 0.0 {
        return utils::send_message_to_player(store, player_name, "Internal error: computed payout is invalid.")
            .await;
    }

    let pair = store.pairs.get(item).unwrap();
    if pair.currency_stock < total_payout {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Store has insufficient diamonds to buy that. Available reserve: {:.2}, needed: {:.2}",
                pair.currency_stock, total_payout
            ),
        )
        .await;
    }

    // Plan deposit - this also validates that storage has physical space
    // The deposit_plan function allocates items to existing chests, empty chests,
    // or creates new nodes if needed. We simulate on a clone (plan-then-commit) so we can
    // reject the order cleanly if planning fails without leaving the real storage in a bad state.
    let stack_size = pair.stack_size;
    let mut sim_storage = store.storage.clone();
    let preview_deposit_plan = sim_storage.deposit_plan(item, qty_i32, stack_size);
    let preview_deposited: i32 = preview_deposit_plan.iter().map(|t| t.amount).sum();
    
    // Validate that the deposit plan can accommodate all items
    if preview_deposited != qty_i32 {
        warn!(
            "Deposit preview mismatch for {}: planned {}, expected {} - storage may not have sufficient space",
            item, preview_deposited, qty_i32
        );
        // If we couldn't plan the full deposit, reject the sell order
        if preview_deposited < qty_i32 {
            return utils::send_message_to_player(
                store,
                player_name,
                &format!(
                    "Storage space validation failed for '{}': can only store {} items, but {} requested. Please contact an operator to add more storage nodes.",
                    item, preview_deposited, qty_i32
                ),
            )
            .await;
        }
    }
    
    // Additional validation: check if we need to create new nodes
    let nodes_before = store.storage.nodes.len();
    let nodes_after = sim_storage.nodes.len();
    if nodes_after > nodes_before {
        info!(
            "Sell order for {} x{} will require {} new node(s) to be created",
            item, qty_i32, nodes_after - nodes_before
        );
    }

    // Split the float payout into two channels: whole diamonds are handed over in the trade GUI
    // (since Minecraft items are integer units), and the leftover fraction (< 1 diamond) is
    // credited to the player's store balance during the commit phase below. This lets AMM pricing
    // produce non-integer payouts without losing precision.
    let floor_value = total_payout.floor();
    // Validate that the result fits in i32
    if floor_value > i32::MAX as f64 {
        return utils::send_message_to_player(
            store,
            player_name,
            "Payout amount too large (exceeds maximum diamond limit)",
        )
        .await;
    }
    let whole_diamonds = floor_value as i32;
    let fractional_diamonds = total_payout - (whole_diamonds as f64);

    // Notify player of the trade details before sending the trade request
    let trade_info_msg = if whole_diamonds > 0 && fractional_diamonds > 0.001 {
        format!(
            "Sell {} {}: You'll receive {} diamonds in trade + {:.2} to balance (total {:.2}).",
            qty_i32, item, whole_diamonds, fractional_diamonds, total_payout
        )
    } else if whole_diamonds > 0 {
        format!(
            "Sell {} {}: You'll receive {} diamonds in trade.",
            qty_i32, item, whole_diamonds
        )
    } else {
        format!(
            "Sell {} {}: You'll receive {:.2} diamonds to balance (amount too small for trade).",
            qty_i32, item, total_payout
        )
    };
    utils::send_message_to_player(store, player_name, &trade_info_msg).await?;

    // Withdraw diamonds from storage before trading them to the player
    if whole_diamonds > 0 {
        info!("[Sell] Withdrawing {} diamonds from storage for payout", whole_diamonds);
        let mut sim_diamond_storage = store.storage.clone();
        let diamond_withdraw_plan = sim_diamond_storage.withdraw_plan("diamond", whole_diamonds);
        let preview_diamond_withdrawn: i32 = diamond_withdraw_plan.iter().map(|t| t.amount).sum();
        debug!("[Sell] Diamond withdrawal plan: {} operations, {} total diamonds", 
               diamond_withdraw_plan.len(), preview_diamond_withdrawn);
        
        if preview_diamond_withdrawn < whole_diamonds {
            error!("[Sell] Insufficient physical diamonds: need {}, storage has {}", 
                   whole_diamonds, preview_diamond_withdrawn);
            return utils::send_message_to_player(
                store,
                player_name,
                &format!(
                    "Store has insufficient physical diamonds. Storage has {}, need {}.",
                    preview_diamond_withdrawn, whole_diamonds
                ),
            )
            .await;
        }

        let mut diamond_step = 0;
        for t in &diamond_withdraw_plan {
            diamond_step += 1;
            info!("[Sell] Diamond withdrawal step {}/{}: chest {} at ({},{},{}), {} diamonds",
                  diamond_step, diamond_withdraw_plan.len(),
                  t.chest_id, t.position.x, t.position.y, t.position.z, t.amount);
            
            let node_position = store.get_node_position(t.chest_id);
            let chest = crate::types::Chest {
                id: t.chest_id,
                node_id: t.chest_id / 4,
                index: t.chest_id % 4,
                position: t.position,
                item: t.item.clone(),
                amounts: vec![0; crate::types::Storage::SLOTS_PER_CHEST],
            };

            let (tx, rx) = oneshot::channel();
            debug!("[Sell] Sending diamond withdrawal instruction to bot...");
            let send_result = store.bot_tx
                .send(BotInstruction::InteractWithChestAndSync {
                    target_chest: chest,
                    node_position,
                    action: ChestAction::Withdraw {
                        item: "diamond".to_string(),
                        amount: t.amount,
                        to_player: None,
                        stack_size: 64, // Diamonds stack to 64
                    },
                    respond_to: tx,
                })
                .await;
            
            if let Err(e) = send_result {
                error!("[Sell] FAILED to send diamond withdrawal instruction: {}", e);
                return Err(format!("Failed to send chest instruction to bot: {}", e));
            }

            debug!("[Sell] Awaiting diamond withdrawal response (timeout {}s)...", CHEST_OP_TIMEOUT_SECS);
            let await_start = std::time::Instant::now();
            let timeout_result = tokio::time::timeout(tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS), rx).await;
            let await_elapsed = await_start.elapsed();
            
            let bot_result = match timeout_result {
                Ok(channel_result) => {
                    debug!("[Sell] Diamond withdrawal response received after {:.2}s", await_elapsed.as_secs_f64());
                    match channel_result {
                        Ok(result) => result,
                        Err(e) => {
                            error!("[Sell] Diamond withdrawal channel DROPPED after {:.2}s: {}", await_elapsed.as_secs_f64(), e);
                            return Err(format!("Bot response dropped: {}", e));
                        }
                    }
                }
                Err(_) => {
                    error!("[Sell] Diamond withdrawal TIMEOUT after {:.2}s!", await_elapsed.as_secs_f64());
                    return Err("Bot timed out withdrawing diamonds from storage".to_string());
                }
            };

            match bot_result {
                Err(err) => {
                    error!("[Sell] Diamond withdrawal failed: {}", err);
                    return utils::send_message_to_player(
                        store,
                        player_name,
                        &format!("Sell aborted: failed to get diamonds from storage: {}", err),
                    )
                    .await;
                }
                Ok(report) => {
                    info!("[Sell] Diamond withdrawal from chest {} succeeded", report.chest_id);
                    if let Err(e) = store.apply_chest_sync(report) {
                        warn!("[Sell] Chest sync failed after diamond withdrawal: {}", e);
                    }
                }
            }
        }
        info!("[Sell] All {} diamond withdrawal operations completed", diamond_withdraw_plan.len());
    } else {
        debug!("[Sell] No diamonds to withdraw (whole_diamonds = 0)");
    }

    // Perform trade GUI: player offers items, bot offers diamonds (whole part).
    info!(
        "[Sell] Initiating trade: bot offers {} diamonds, player {} offers {}x {}",
        whole_diamonds, player_name, qty_i32, item
    );
    
    let (trade_tx, trade_rx) = oneshot::channel();
    store.bot_tx
        .send(BotInstruction::TradeWithPlayer {
            target_username: player_name.to_string(),
            bot_offers: if whole_diamonds > 0 {
                vec![TradeItem {
                    item: "diamond".to_string(),
                    amount: whole_diamonds,
                }]
            } else {
                vec![]
            },
            player_offers: vec![TradeItem {
                item: item.to_string(),
                amount: qty_i32,
            }],
            // Sell: require EXACT amount - reject if player offers more or less
            require_exact_amount: true,
            flexible_validation: false,
            respond_to: trade_tx,
        })
        .await
        .map_err(|e| {
            error!("[Sell] Failed to send trade instruction to bot: {}", e);
            format!("Failed to send trade instruction to bot: {}", e)
        })?;

    info!("[Sell] Trade instruction sent, awaiting trade result (timeout 45s)...");
    let trade_result = tokio::time::timeout(tokio::time::Duration::from_secs(45), trade_rx)
        .await
        .map_err(|_| {
            error!("[Sell] Trade timed out after 45 seconds!");
            "Bot timed out waiting for trade completion".to_string()
        })?
        .map_err(|e| {
            error!("[Sell] Trade response channel dropped: {}", e);
            format!("Bot response dropped: {}", e)
        })?;
    
    info!("[Sell] Trade result received, processing...");
    let actual_received = match trade_result {
        Err(err) => {
            error!("[Sell] Trade failed for {}: {}", player_name, err);
            // Rollback: we already withdrew whole_diamonds into bot inventory for the payout.
            // Since the trade never completed, we still hold those diamonds and must return them
            // to storage. Build a fresh deposit plan (not the symmetric of the withdraw plan,
            // since intermediate layout may have shifted) and replay it best-effort.
            if whole_diamonds > 0 {
                info!("[Sell] Rolling back {} diamonds to storage", whole_diamonds);
                let mut sim_diamond_storage = store.storage.clone();
                let diamond_deposit_plan = sim_diamond_storage.deposit_plan("diamond", whole_diamonds, 64);
                
                let mut rollback_success = 0;
                let mut rollback_failed = 0;
                
                for (step, t) in diamond_deposit_plan.iter().enumerate() {
                    let node_position = store.get_node_position(t.chest_id);
                    let chest = crate::types::Chest {
                        id: t.chest_id,
                        node_id: t.chest_id / 4,
                        index: t.chest_id % 4,
                        position: t.position,
                        item: t.item.clone(),
                        amounts: vec![0; crate::types::Storage::SLOTS_PER_CHEST],
                    };

                    let (tx, rx) = oneshot::channel();
                    let send_result = store.bot_tx
                        .send(BotInstruction::InteractWithChestAndSync {
                            target_chest: chest,
                            node_position,
                            action: ChestAction::Deposit {
                                item: "diamond".to_string(),
                                amount: t.amount,
                                from_player: None,
                                stack_size: 64, // Diamonds stack to 64
                            },
                            respond_to: tx,
                        })
                        .await;
                    
                    if let Err(e) = send_result {
                        error!("[Sell] Diamond rollback step {} FAILED to send: {}", step + 1, e);
                        rollback_failed += 1;
                        continue;
                    }
                    
                    match tokio::time::timeout(tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS), rx).await {
                        Ok(Ok(Ok(report))) => {
                            if let Err(e) = store.apply_chest_sync(report) {
                                warn!("[Sell] Diamond rollback step {} sync warning: {}", step + 1, e);
                            }
                            rollback_success += 1;
                        }
                        Ok(Ok(Err(e))) => {
                            error!("[Sell] Diamond rollback step {} bot error: {}", step + 1, e);
                            rollback_failed += 1;
                        }
                        Ok(Err(e)) => {
                            error!("[Sell] Diamond rollback step {} channel dropped: {}", step + 1, e);
                            rollback_failed += 1;
                        }
                        Err(_) => {
                            error!("[Sell] Diamond rollback step {} TIMEOUT", step + 1);
                            rollback_failed += 1;
                        }
                    }
                }
                
                info!("[Sell] Diamond rollback: {}/{} operations succeeded ({} failed)", 
                      rollback_success, diamond_deposit_plan.len(), rollback_failed);
            }
            
            return utils::send_message_to_player(
                store,
                player_name,
                &format!("Sell aborted: trade failed: {}. Diamonds returned to storage.", err),
            )
            .await;
        }
        Ok(received) => {
            info!(
                "[Sell] Trade succeeded for {}: received {} item types",
                player_name, received.len()
            );
            for ti in &received {
                info!("[Sell]   - {}x {}", ti.amount, ti.item);
            }
            received
        }
    };
    
    // CRITICAL: Validate that player actually put in the expected items.
    // The trade GUI enforces require_exact_amount=true above, but we defensively re-verify the
    // actual received items here to prevent exploits where slot-swapping or client-side tricks
    // could desync the bot's view of what was offered. Item IDs are normalized before comparison
    // to handle namespace/casing variants (e.g. "minecraft:cobblestone" vs "cobblestone").
    info!("[Sell] Validating items received from trade...");
    let target_item_id = crate::bot::Bot::normalize_item_id(item);
    info!("[Sell] Target item ID (normalized): '{}' (original: '{}')", target_item_id, item);
    
    let items_received: i32 = actual_received
        .iter()
        .filter(|t| {
            let normalized = crate::bot::Bot::normalize_item_id(&t.item);
            let matches = normalized == target_item_id;
            if !matches {
                info!("[Sell] Item '{}' (normalized: '{}') does not match target '{}'", 
                    t.item, normalized, target_item_id);
            }
            matches
        })
        .map(|t| t.amount)
        .sum();
    
    info!("[Sell] Items received: {}, expected: {}", items_received, qty_i32);
    
    if items_received != qty_i32 {
        error!(
            "SELL VALIDATION FAILED: Player {} promised {} {} but only put {} in trade!",
            player_name, qty_i32, item, items_received
        );
        
        // Rollback: deposit diamonds back into storage (we already withdrew them)
        if whole_diamonds > 0 {
            let mut sim_diamond_storage = store.storage.clone();
            let diamond_deposit_plan = sim_diamond_storage.deposit_plan("diamond", whole_diamonds, 64);
            
            for t in &diamond_deposit_plan {
                let node_position = store.get_node_position(t.chest_id);
                let chest = crate::types::Chest {
                    id: t.chest_id,
                    node_id: t.chest_id / 4,
                    index: t.chest_id % 4,
                    position: t.position,
                    item: t.item.clone(),
                    amounts: vec![0; crate::types::Storage::SLOTS_PER_CHEST],
                };

                let (tx, rx) = oneshot::channel();
                let _ = store.bot_tx
                    .send(BotInstruction::InteractWithChestAndSync {
                        target_chest: chest,
                        node_position,
                        action: ChestAction::Deposit {
                            item: "diamond".to_string(),
                            amount: t.amount,
                            from_player: None,
                            stack_size: 64, // Diamonds stack to 64
                        },
                        respond_to: tx,
                    })
                    .await;
                if let Ok(Ok(Ok(report))) = tokio::time::timeout(tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS), rx).await {
                    let _ = store.apply_chest_sync(report);
                }
            }
        }
        
        // The items the player DID put in are now in bot inventory
        // We need to return them to the player
        if items_received > 0 {
            warn!(
                "Attempting to return {} {} to player after failed sell validation",
                items_received, item
            );
            let (rb_tx, rb_rx) = oneshot::channel();
            let _ = store
                .bot_tx
                .send(BotInstruction::TradeWithPlayer {
                    target_username: player_name.to_string(),
                    bot_offers: vec![TradeItem {
                        item: item.to_string(),
                        amount: items_received,
                    }],
                    player_offers: vec![],
                    // Return items: player offers nothing
                    require_exact_amount: false,
                    flexible_validation: false,
                    respond_to: rb_tx,
                })
                .await;
            let _ = tokio::time::timeout(tokio::time::Duration::from_secs(45), rb_rx).await;
        }
        
        return utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Sell REJECTED: You only put {} {} in the trade but promised {}. Trade cancelled, items returned.",
                items_received, item, qty_i32
            ),
        )
        .await;
    }
    
    info!(
        "[Sell] Validation passed: received {} {} from player (expected {})",
        items_received, item, qty_i32
    );

    // Now deposit items from bot inventory into storage
    info!(
        "[Sell] Starting deposit loop: {} chest operations planned for {}x {}",
        preview_deposit_plan.len(), qty_i32, item
    );
    
    let mut deposit_step = 0;
    for t in &preview_deposit_plan {
        deposit_step += 1;
        info!(
            "[Sell] Deposit step {}/{}: chest {} at ({},{},{}), {}x {}",
            deposit_step, preview_deposit_plan.len(),
            t.chest_id, t.position.x, t.position.y, t.position.z,
            t.amount, t.item
        );
        let node_position = store.get_node_position(t.chest_id);
        let chest = crate::types::Chest {
            id: t.chest_id,
            node_id: t.chest_id / 4,
            index: t.chest_id % 4,
            position: t.position,
            item: t.item.clone(),
            amounts: vec![0; crate::types::Storage::SLOTS_PER_CHEST],
        };

        let (tx, rx) = oneshot::channel();
        debug!("[Sell] Created oneshot channel for chest {} deposit response", t.chest_id);
        info!("[Sell] Sending deposit instruction to bot for chest {}...", t.chest_id);
        
        let send_start = std::time::Instant::now();
        let send_result = store.bot_tx
            .send(BotInstruction::InteractWithChestAndSync {
                target_chest: chest,
                node_position,
                action: ChestAction::Deposit {
                    item: item.to_string(),
                    amount: t.amount,
                    from_player: None,
                    stack_size: stack_size,
                },
                respond_to: tx,
            })
            .await;
        let send_elapsed = send_start.elapsed();
        
        if let Err(e) = send_result {
            error!("[Sell] FAILED to send deposit instruction to bot after {:.3}s: {}", send_elapsed.as_secs_f64(), e);
            return Err(format!("Failed to send chest instruction to bot: {}", e));
        }
        debug!("[Sell] Instruction sent successfully in {:.3}s", send_elapsed.as_secs_f64());

        info!("[Sell] Awaiting bot response for chest {} deposit (timeout {}s)...", t.chest_id, CHEST_OP_TIMEOUT_SECS);
        let await_start = std::time::Instant::now();
        let timeout_result = tokio::time::timeout(tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS), rx).await;
        let await_elapsed = await_start.elapsed();
        
        debug!("[Sell] Timeout/channel await returned after {:.2}s", await_elapsed.as_secs_f64());
        
        let bot_result = match timeout_result {
            Ok(channel_result) => {
                debug!("[Sell] Channel returned result after {:.2}s (within timeout)", await_elapsed.as_secs_f64());
                match channel_result {
                    Ok(result) => {
                        info!("[Sell] Bot response received for chest {} after {:.2}s", t.chest_id, await_elapsed.as_secs_f64());
                        result
                    }
                    Err(e) => {
                        error!("[Sell] Bot response channel DROPPED for chest {} after {:.2}s: {}", 
                               t.chest_id, await_elapsed.as_secs_f64(), e);
                        return Err(format!("Bot response dropped: {}", e));
                    }
                }
            }
            Err(_) => {
                error!("[Sell] TIMEOUT on chest {} deposit after {:.2}s (limit was {}s)!", 
                       t.chest_id, await_elapsed.as_secs_f64(), CHEST_OP_TIMEOUT_SECS);
                error!("[Sell] This indicates the bot operation took longer than expected or got stuck!");
                return Err("Bot timed out performing chest step".to_string());
            }
        };

        match bot_result {
            Err(err) => {
                error!("[Sell] Bot reported error on chest {} deposit after {:.2}s: {}",
                       t.chest_id, await_elapsed.as_secs_f64(), err);
                // Partial-deposit failure: some earlier chests in the plan may have already
                // accepted items, but this one failed. We cannot cleanly unwind the earlier
                // deposits (items are already committed to storage and player's items are mixed
                // in), so we do NOT credit the player and attempt a best-effort return via trade
                // of the ORIGINAL qty. The apply_chest_sync calls above have kept storage state
                // consistent with what actually happened physically.
                let (rb_tx, rb_rx) = oneshot::channel();
                let _ = store
                    .bot_tx
                    .send(BotInstruction::TradeWithPlayer {
                        target_username: player_name.to_string(),
                        bot_offers: vec![TradeItem {
                            item: item.to_string(),
                            amount: qty_i32,
                        }],
                        player_offers: vec![],
                        // Return items: player offers nothing
                        require_exact_amount: false,
                        flexible_validation: false,
                        respond_to: rb_tx,
                    })
                    .await;
                let _ = tokio::time::timeout(tokio::time::Duration::from_secs(45), rb_rx).await;

                return utils::send_message_to_player(
                    store,
                    player_name,
                    &format!(
                        "Sell aborted: failed to deposit into storage: {}. You were NOT paid. I attempted to return items via trade; if you did not receive them, contact an operator.",
                        err
                    ),
                )
                .await;
            }
            Ok(report) => {
                info!("[Sell] Chest {} deposit succeeded, syncing storage state", report.chest_id);
                if let Err(e) = store.apply_chest_sync(report) {
                    warn!("Chest sync failed after deposit: {}", e);
                }
            }
        }
    }
    
    info!("[Sell] All {} deposit operations completed successfully", preview_deposit_plan.len());

    // Commit: update ledgers after bot confirmed all chest operations and synced contents
    info!("[Sell] Committing ledger updates for {} sell of {}x {}", player_name, qty_i32, item);
    let pair = store.pairs.get_mut(item).unwrap();

    // Credit the leftover sub-diamond fraction to the player's balance. The whole_diamonds
    // portion was already handed over physically during the trade GUI step, so it is not
    // touched here — only the fractional remainder needs to be recorded in the ledger.
    store.users.get_mut(&user_uuid).unwrap().balance += fractional_diamonds;
    store.users.get_mut(&user_uuid).unwrap().username = player_name.to_owned();
    store.dirty = true;

    pair.item_stock = store.storage.total_item_amount(item);
    pair.currency_stock -= total_payout;
    store.dirty = true;

    // Record trade
    store.trades.push(Trade::new(
        TradeType::Sell,
        item.to_string(),
        qty_i32,
        total_payout,
        user_uuid.clone(),
    ));

    // Record order
    store.orders.push_back(Order {
        order_type: crate::types::order::OrderType::Sell,
        item: item.to_string(),
        amount: qty_i32,
        user_uuid: user_uuid.clone(),
    });

    info!(
        "[Sell] Executed sell: user={} item={} qty={} total={:.2} (whole: {}, fractional: {:.2})",
        player_name, item, quantity, total_payout, whole_diamonds, fractional_diamonds
    );

    // Enforce invariants after mutation
    if let Err(e) = state::assert_invariants(store, "post-sell", true) {
        error!("[Sell] Invariant violation after sell: {}", e);
        let _ = state::save(store);
    }

    let deposit_summary = utils::summarize_transfers(&preview_deposit_plan, 3);
    let fee_amount = total_payout / (1.0 - store.config.fee) - total_payout;
    
    info!("[Sell] === Sell order complete: {} sold {}x {} for {:.2} diamonds ===", 
        player_name, quantity, item, total_payout);
    
    utils::send_message_to_player(
        store,
        player_name,
        &format!(
            "Sold {} {} for {:.2} diamonds (fee {:.2}). Trade complete. Storage: {}",
            quantity, item, total_payout, fee_amount, deposit_summary
        ),
    )
    .await
}

/// Execute a queued order
///
/// This function dispatches to the appropriate handler based on order type.
/// It returns a success message on completion or an error message on failure.
/// The handlers themselves send messages to the player during execution.
pub async fn execute_queued_order(
    store: &mut Store,
    order: &QueuedOrder,
) -> Result<String, String> {
    info!(
        "[OrderExec] === EXECUTING ORDER #{} === type={:?} item={} qty={} user={}",
        order.id, order.order_type, order.item, order.quantity, order.username
    );
    debug!("[OrderExec] Order queued at: {}, executing now", order.queued_at);
    
    let start_time = std::time::Instant::now();

    let result = match &order.order_type {
        QueuedOrderType::Buy => {
            info!("[OrderExec] Dispatching to handle_buy_order...");
            match handle_buy_order(store, &order.username, &order.item, order.quantity).await {
                Ok(()) => {
                    info!("[OrderExec] handle_buy_order returned Ok");
                    Ok(format!(
                        "Buy order completed: {} {} for {}",
                        order.quantity, order.item, order.username
                    ))
                }
                Err(e) => {
                    error!("[OrderExec] handle_buy_order returned Err: {}", e);
                    Err(e)
                }
            }
        }
        QueuedOrderType::Sell => {
            info!("[OrderExec] Dispatching to handle_sell_order...");
            match handle_sell_order(store, &order.username, &order.item, order.quantity).await {
                Ok(()) => {
                    info!("[OrderExec] handle_sell_order returned Ok");
                    Ok(format!(
                        "Sell order completed: {} {} for {}",
                        order.quantity, order.item, order.username
                    ))
                }
                Err(e) => {
                    error!("[OrderExec] handle_sell_order returned Err: {}", e);
                    Err(e)
                }
            }
        }
        QueuedOrderType::Deposit { amount } => {
            info!("[OrderExec] Dispatching to handle_deposit_balance_queued (amount={:?})...", amount);
            match super::handlers::player::handle_deposit_balance_queued(store, &order.username, *amount).await {
                Ok(()) => {
                    info!("[OrderExec] handle_deposit_balance_queued returned Ok");
                    Ok(format!(
                        "Deposit completed for {}",
                        order.username
                    ))
                }
                Err(e) => {
                    error!("[OrderExec] handle_deposit_balance_queued returned Err: {}", e);
                    Err(e)
                }
            }
        }
        QueuedOrderType::Withdraw { amount } => {
            info!("[OrderExec] Dispatching to handle_withdraw_balance_queued (amount={:?})...", amount);
            match super::handlers::player::handle_withdraw_balance_queued(store, &order.username, *amount).await {
                Ok(()) => {
                    info!("[OrderExec] handle_withdraw_balance_queued returned Ok");
                    Ok(format!(
                        "Withdraw completed for {}",
                        order.username
                    ))
                }
                Err(e) => {
                    error!("[OrderExec] handle_withdraw_balance_queued returned Err: {}", e);
                    Err(e)
                }
            }
        }
    };
    
    let elapsed = start_time.elapsed();
    match &result {
        Ok(msg) => info!("[OrderExec] === ORDER #{} COMPLETED in {:.2}s === {}", order.id, elapsed.as_secs_f64(), msg),
        Err(msg) => error!("[OrderExec] === ORDER #{} FAILED after {:.2}s === {}", order.id, elapsed.as_secs_f64(), msg),
    }
    
    result
}
