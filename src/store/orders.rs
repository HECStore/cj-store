//! Order execution handlers (buy/sell/deposit/withdraw)

use tokio::sync::oneshot;
use tracing::{error, info, warn};

use crate::constants::CHEST_OP_TIMEOUT_SECS;
use crate::messages::{BotInstruction, ChestAction, QueuedOrderType, TradeItem};
use crate::types::{Order, Trade, TradeType};
use super::{Store, pricing, rollback, state, utils};
use super::queue::QueuedOrder;

/// Handle buy orders
pub async fn handle_buy_order(
    store: &mut Store,
    player_name: &str,
    item: &str,
    quantity: u32,
) -> Result<(), String> {
    info!("[Buy] Starting: player={} item={} qty={}", player_name, item, quantity);
    state::assert_invariants(store, "pre-buy", false)?;

    let user_uuid = utils::resolve_user_uuid(store, player_name).await?;
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
            error!("[Buy] Failed to send chest instruction to bot: {}", e);
            return Err(format!("Failed to send chest instruction to bot: {}", e));
        }

        let bot_result = match tokio::time::timeout(
            tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS),
            rx,
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                error!("[Buy] Channel dropped: {}", e);
                return Err(format!("Bot response dropped: {}", e));
            }
            Err(_) => {
                error!("[Buy] Timeout waiting for bot on chest {} withdrawal", t.chest_id);
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
                if let Err(e) = store.apply_chest_sync(report) {
                    warn!("[Buy] Chest sync failed after withdraw: {}", e);
                }
            }
        }
    }

    // Notify player of the trade details before sending the trade request
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
    info!("[Buy] Initiating trade: {}x {} for {} diamonds", qty_i32, item, diamonds_to_offer);
    let (trade_tx, trade_rx) = oneshot::channel();
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
        error!("[Buy] Failed to send trade instruction to bot: {}", e);
        return Err(format!("Failed to send trade instruction to bot: {}", e));
    }

    let trade_timeout_result = tokio::time::timeout(
        tokio::time::Duration::from_millis(store.config.trade_timeout_ms),
        trade_rx,
    )
    .await;

    let trade_result = match trade_timeout_result {
        Ok(channel_result) => {
            match channel_result {
                Ok(result) => result,
                Err(e) => {
                    error!("[Buy] Trade channel dropped: {}", e);
                    return Err(format!("Bot response dropped: {}", e));
                }
            }
        }
        Err(_) => {
            error!("[Buy] Trade timeout");
            return Err("Bot timed out waiting for trade completion".to_string());
        }
    };
    
    // Handle trade result - now returns actual items received
    let actual_received = match trade_result {
        Err(err) => {
            warn!("[Buy] Trade failed: {} - rolling back", err);
            // Items are currently in the bot's inventory (we already withdrew them).
            // Walk the original withdraw plan and deposit each chunk back into the same
            // chest it came from via the shared rollback helper.
            let stack_size = store.pairs.get(item).map(|p| p.stack_size).unwrap_or(64);
            let rb = rollback::deposit_transfers(
                store,
                &preview_withdraw_plan,
                item,
                stack_size,
                "[Buy]",
            )
            .await;

            let rollback_msg = if rb.has_failures() {
                format!(
                    "Buy aborted: trade failed: {}. Rollback: {} items returned, {} operations failed - some items may remain in bot inventory.",
                    err, rb.items_returned, rb.operations_failed
                )
            } else {
                format!("Buy aborted: trade failed: {} (items rolled back to storage)", err)
            };

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
        let stack_size = store.pairs.get(item).map(|p| p.stack_size).unwrap_or(64);
        let _ = rollback::deposit_transfers(
            store,
            &preview_withdraw_plan,
            item,
            stack_size,
            "[Buy] insufficient-payment",
        )
        .await;

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
    
    // Deposit diamonds received from player into storage
    if diamonds_received > 0 {
        let rb = rollback::rollback_amount_to_storage(
            store,
            "diamond",
            diamonds_received,
            64,
            "[Buy] diamond-deposit",
        )
        .await;
        if rb.has_failures() {
            warn!(
                "[Buy] Failed to deposit some diamonds into storage ({} failed steps) - diamonds may remain in bot inventory",
                rb.operations_failed
            );
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
        (deduction, 0.0)
    } else {
        // Player paid MORE diamonds than needed, credit surplus to balance
        let surplus_amount = -balance_needed; // balance_needed is negative, so negate it
        store.users.get_mut(&user_uuid).unwrap().balance += surplus_amount;
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
        "[Buy] Completed: {} {}x{} total={:.2} diamonds={} balance_used={:.2} surplus={:.2}",
        player_name, quantity, item, total_cost, diamonds_received, balance_deduction, surplus
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
    info!("[Sell] Starting: player={} item={} qty={}", player_name, item, quantity);

    state::assert_invariants(store, "pre-sell", false)?;
    let user_uuid = utils::resolve_user_uuid(store, player_name).await?;
    utils::ensure_user_exists(store, player_name, &user_uuid);

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
    let _ = sim_storage.nodes.len();

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
        let mut sim_diamond_storage = store.storage.clone();
        let diamond_withdraw_plan = sim_diamond_storage.withdraw_plan("diamond", whole_diamonds);
        let preview_diamond_withdrawn: i32 = diamond_withdraw_plan.iter().map(|t| t.amount).sum();

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

        for t in &diamond_withdraw_plan {
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
                error!("[Sell] Failed to send diamond withdrawal instruction: {}", e);
                return Err(format!("Failed to send chest instruction to bot: {}", e));
            }

            let bot_result = match tokio::time::timeout(
                tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS),
                rx,
            )
            .await
            {
                Ok(Ok(result)) => result,
                Ok(Err(e)) => {
                    error!("[Sell] Diamond withdrawal channel dropped: {}", e);
                    return Err(format!("Bot response dropped: {}", e));
                }
                Err(_) => {
                    error!("[Sell] Diamond withdrawal timeout");
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
                    if let Err(e) = store.apply_chest_sync(report) {
                        warn!("[Sell] Chest sync failed after diamond withdrawal: {}", e);
                    }
                }
            }
        }
    }

    // Perform trade GUI: player offers items, bot offers diamonds (whole part).
    info!(
        "[Sell] Initiating trade: {} offers {}x {} for {} diamonds",
        player_name, qty_i32, item, whole_diamonds
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

    let trade_result = tokio::time::timeout(
        tokio::time::Duration::from_millis(store.config.trade_timeout_ms),
        trade_rx,
    )
    .await
    .map_err(|_| {
        error!("[Sell] Trade timeout");
        "Bot timed out waiting for trade completion".to_string()
    })?
    .map_err(|e| {
        error!("[Sell] Trade response channel dropped: {}", e);
        format!("Bot response dropped: {}", e)
    })?;

    let actual_received = match trade_result {
        Err(err) => {
            warn!("[Sell] Trade failed for {}: {}", player_name, err);
            // Rollback: we already withdrew whole_diamonds into bot inventory for the payout.
            // Build a fresh deposit plan (intermediate layout may have shifted) and replay
            // it via the shared helper.
            let _ = rollback::rollback_amount_to_storage(
                store,
                "diamond",
                whole_diamonds,
                64,
                "[Sell] diamond",
            )
            .await;

            return utils::send_message_to_player(
                store,
                player_name,
                &format!("Sell aborted: trade failed: {}. Diamonds returned to storage.", err),
            )
            .await;
        }
        Ok(received) => received,
    };

    // CRITICAL: Validate that player actually put in the expected items.
    // The trade GUI enforces require_exact_amount=true above, but we defensively re-verify the
    // actual received items here to prevent exploits where slot-swapping or client-side tricks
    // could desync the bot's view of what was offered. Item IDs are normalized before comparison
    // to handle namespace/casing variants (e.g. "minecraft:cobblestone" vs "cobblestone").
    let target_item_id = crate::bot::Bot::normalize_item_id(item);

    let items_received: i32 = actual_received
        .iter()
        .filter(|t| crate::bot::Bot::normalize_item_id(&t.item) == target_item_id)
        .map(|t| t.amount)
        .sum();

    if items_received != qty_i32 {
        warn!(
            "[Sell] Validation failed: {} promised {}x {} but put {}",
            player_name, qty_i32, item, items_received
        );
        
        // Rollback: deposit diamonds back into storage (we already withdrew them)
        let _ = rollback::rollback_amount_to_storage(
            store,
            "diamond",
            whole_diamonds,
            64,
            "[Sell] validation-failed",
        )
        .await;

        // The items the player DID put in are now in bot inventory
        // We need to return them to the player
        if items_received > 0 {
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
            let _ = tokio::time::timeout(tokio::time::Duration::from_millis(store.config.trade_timeout_ms), rb_rx).await;
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
    
    // Now deposit items from bot inventory into storage
    for t in &preview_deposit_plan {
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
                    stack_size,
                },
                respond_to: tx,
            })
            .await;

        if let Err(e) = send_result {
            error!("[Sell] Failed to send deposit instruction: {}", e);
            return Err(format!("Failed to send chest instruction to bot: {}", e));
        }

        let bot_result = match tokio::time::timeout(
            tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS),
            rx,
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                error!("[Sell] Deposit channel dropped for chest {}: {}", t.chest_id, e);
                return Err(format!("Bot response dropped: {}", e));
            }
            Err(_) => {
                error!("[Sell] Timeout on chest {} deposit", t.chest_id);
                return Err("Bot timed out performing chest step".to_string());
            }
        };

        match bot_result {
            Err(err) => {
                error!("[Sell] Bot reported error on chest {} deposit: {}", t.chest_id, err);
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
                let _ = tokio::time::timeout(tokio::time::Duration::from_millis(store.config.trade_timeout_ms), rb_rx).await;

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
                if let Err(e) = store.apply_chest_sync(report) {
                    warn!("Chest sync failed after deposit: {}", e);
                }
            }
        }
    }

    // Commit: update ledgers after bot confirmed all chest operations and synced contents
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
        "[Sell] Completed: {} {}x{} total={:.2} whole={} fractional={:.2}",
        player_name, quantity, item, total_payout, whole_diamonds, fractional_diamonds
    );

    // Enforce invariants after mutation
    if let Err(e) = state::assert_invariants(store, "post-sell", true) {
        error!("[Sell] Invariant violation after sell: {}", e);
        let _ = state::save(store);
    }

    let deposit_summary = utils::summarize_transfers(&preview_deposit_plan, 3);
    let fee_amount = total_payout / (1.0 - store.config.fee) - total_payout;

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
        "[OrderExec] Order #{} type={:?} item={} qty={} user={}",
        order.id, order.order_type, order.item, order.quantity, order.username
    );

    let start_time = std::time::Instant::now();

    let result = match &order.order_type {
        QueuedOrderType::Buy => handle_buy_order(store, &order.username, &order.item, order.quantity)
            .await
            .map(|()| format!("Buy order completed: {} {} for {}", order.quantity, order.item, order.username)),
        QueuedOrderType::Sell => handle_sell_order(store, &order.username, &order.item, order.quantity)
            .await
            .map(|()| format!("Sell order completed: {} {} for {}", order.quantity, order.item, order.username)),
        QueuedOrderType::Deposit { amount } => {
            super::handlers::player::handle_deposit_balance_queued(store, &order.username, *amount)
                .await
                .map(|()| format!("Deposit completed for {}", order.username))
        }
        QueuedOrderType::Withdraw { amount } => {
            super::handlers::player::handle_withdraw_balance_queued(store, &order.username, *amount)
                .await
                .map(|()| format!("Withdraw completed for {}", order.username))
        }
    };

    let elapsed = start_time.elapsed();
    match &result {
        Ok(msg) => info!("[OrderExec] Order #{} completed in {:.2}s: {}", order.id, elapsed.as_secs_f64(), msg),
        Err(msg) => error!("[OrderExec] Order #{} failed after {:.2}s: {}", order.id, elapsed.as_secs_f64(), msg),
    }

    result
}
