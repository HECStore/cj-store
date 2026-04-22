//! # Order execution handlers (buy/sell/deposit/withdraw)
//!
//! High-level flow for any trade is always the same four phases:
//!   1. **Validate** — basic input/pair/balance/stock checks.
//!   2. **Plan** — compute a chest transfer plan against storage *without*
//!      mutating it (see `Storage::simulate_withdraw_plan` /
//!      `Storage::simulate_deposit_plan`).
//!   3. **Execute** — ask the bot to perform each chest operation and the
//!      trade GUI handoff with the player.
//!   4. **Commit** — update ledgers (pair stock, user balance, trade log)
//!      after the bot has confirmed the physical side.
//!
//! The helpers [`execute_chest_transfers`] and [`perform_trade`] encapsulate
//! the per-step bot plumbing (send instruction, await with timeout, apply
//! sync report) so the handlers read as a linear phase list instead of the
//! ~470-line monoliths we had before.

use tokio::sync::oneshot;
use tracing::{error, info, warn};

use crate::constants::CHEST_OP_TIMEOUT_SECS;
use crate::error::StoreError;
use crate::messages::{BotInstruction, ChestAction, QueuedOrderType, TradeItem};
use crate::types::{ItemId, Order};
use crate::types::storage::ChestTransfer;
use crate::types::{Trade, TradeType};
use super::{Store, pricing, rollback, state, utils};
use super::queue::QueuedOrder;

// ===========================================================================
// Shared helpers
// ===========================================================================

/// Execute a list of chest transfers via the bot (withdraw or deposit).
///
/// This is the single code path used by all handlers for "walk a plan and
/// make the bot do each step." Prior to the extraction each handler had its
/// own ~70-line copy of this loop with subtly different error handling.
///
/// On success every step's `ChestSyncReport` has already been applied to
/// store state. On failure the returned error identifies which step failed
/// and why; the caller is responsible for rollback of any earlier steps.
pub(crate) async fn execute_chest_transfers(
    store: &mut Store,
    transfers: &[ChestTransfer],
    item: &str,
    stack_size: i32,
    direction: ChestDirection,
    log_tag: &'static str,
) -> Result<(), StoreError> {
    for t in transfers {
        let node_position = store.get_node_position(t.chest_id);
        let chest = rollback::chest_from_transfer(t);
        let action = match direction {
            ChestDirection::Withdraw => ChestAction::Withdraw {
                item: item.to_string(),
                amount: t.amount,
                to_player: None,
                stack_size,
            },
            ChestDirection::Deposit => ChestAction::Deposit {
                item: item.to_string(),
                amount: t.amount,
                from_player: None,
                stack_size,
            },
        };

        let (tx, rx) = oneshot::channel();
        store
            .bot_tx
            .send(BotInstruction::InteractWithChestAndSync {
                target_chest: chest,
                node_position,
                action,
                respond_to: tx,
            })
            .await
            .map_err(|e| {
                error!(phase = log_tag, chest_id = t.chest_id, "Failed to send chest instruction: {}", e);
                StoreError::BotDisconnected
            })?;

        let bot_result = match tokio::time::timeout(
            tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS),
            rx,
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                error!(phase = log_tag, chest_id = t.chest_id, "Channel dropped: {}", e);
                return Err(StoreError::BotError(format!("Bot response dropped: {}", e)));
            }
            Err(_) => {
                error!(phase = log_tag, chest_id = t.chest_id, "Chest operation timeout");
                return Err(StoreError::TradeTimeout(CHEST_OP_TIMEOUT_SECS));
            }
        };

        match bot_result {
            Err(err) => {
                error!(phase = log_tag, chest_id = t.chest_id, "Bot reported error: {}", err);
                return Err(StoreError::ChestOp(err));
            }
            Ok(report) => {
                if let Err(e) = store.apply_chest_sync(report) {
                    warn!(phase = log_tag, chest_id = t.chest_id, "Chest sync warning: {}", e);
                }
            }
        }
    }
    Ok(())
}

/// Direction a chest transfer moves items.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ChestDirection {
    Withdraw,
    Deposit,
}

/// Fire a `TradeWithPlayer` instruction and await the bot's response.
///
/// Wraps the send + oneshot + timeout dance that used to be copy-pasted in
/// every handler. On success returns the items the bot recorded as actually
/// received from the player; on failure returns a descriptive error for the
/// caller to roll back and/or surface to the player.
pub(crate) async fn perform_trade(
    store: &Store,
    target_username: &str,
    bot_offers: Vec<TradeItem>,
    player_offers: Vec<TradeItem>,
    require_exact_amount: bool,
    flexible_validation: bool,
    log_tag: &'static str,
) -> Result<Vec<TradeItem>, StoreError> {
    let (trade_tx, trade_rx) = oneshot::channel();
    store
        .bot_tx
        .send(BotInstruction::TradeWithPlayer {
            target_username: target_username.to_string(),
            bot_offers,
            player_offers,
            require_exact_amount,
            flexible_validation,
            respond_to: trade_tx,
        })
        .await
        .map_err(|e| {
            error!(phase = log_tag, player = %target_username, "Failed to send trade instruction: {}", e);
            StoreError::BotDisconnected
        })?;

    let trade_result = tokio::time::timeout(
        tokio::time::Duration::from_millis(store.config.trade_timeout_ms),
        trade_rx,
    )
    .await
    .map_err(|_| {
        error!(phase = log_tag, player = %target_username, "Trade timeout");
        StoreError::TradeTimeout(store.config.trade_timeout_ms / 1000)
    })?
    .map_err(|e| {
        error!(phase = log_tag, player = %target_username, "Trade channel dropped: {}", e);
        StoreError::BotError(format!("Bot response dropped: {}", e))
    })?;

    trade_result.map_err(StoreError::TradeRejected)
}

// ===========================================================================
// Buy order
// ===========================================================================

/// Outcome of buy-order validation: either an accepted plan or a rejection
/// message to forward to the player.
struct BuyPlan {
    user_uuid: String,
    qty_i32: i32,
    total_cost: f64,
    diamonds_to_offer: i32,
    user_balance_at_plan: f64,
    withdraw_plan: Vec<ChestTransfer>,
    physical_stock: i32,
    stack_size: i32,
}

async fn validate_and_plan_buy(
    store: &mut Store,
    player_name: &str,
    item: &str,
    quantity: u32,
) -> Result<Option<BuyPlan>, StoreError> {
    let user_uuid = utils::resolve_user_uuid(player_name).await?;
    utils::ensure_user_exists(store, player_name, &user_uuid);

    if !store.pairs.contains_key(item) {
        warn!(phase = "buy.validate", player = %player_name, item = %item, "Attempted to buy unavailable item");
        utils::send_message_to_player(
            store,
            player_name,
            &format!("Item '{}' is not available for trading", item),
        )
        .await?;
        return Ok(None);
    }

    let qty_i32: i32 = quantity
        .try_into()
        .map_err(|_| StoreError::ValidationError("Quantity too large".to_string()))?;
    if qty_i32 <= 0 {
        utils::send_message_to_player(store, player_name, "Quantity must be positive").await?;
        return Ok(None);
    }

    let total_cost = match pricing::calculate_buy_cost(store, item, qty_i32) {
        Some(cost) => cost,
        None => {
            let pair = store.expect_pair(item, "buy/price-fail")?;
            let msg = if qty_i32 >= pair.item_stock {
                format!(
                    "Cannot buy {} {} - would exceed available stock ({}). Try a smaller amount.",
                    qty_i32, item, pair.item_stock
                )
            } else {
                format!("Item '{}' is not available for trading (no stock or reserves).", item)
            };
            utils::send_message_to_player(store, player_name, &msg).await?;
            return Ok(None);
        }
    };

    if !total_cost.is_finite() || total_cost <= 0.0 {
        utils::send_message_to_player(store, player_name, "Internal error: computed price is invalid.").await?;
        return Ok(None);
    }

    let physical_stock = store.storage.total_item_amount(item);
    if physical_stock < qty_i32 {
        utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Out of physical stock for '{}'. Storage has {}, requested {}.",
                item, physical_stock, qty_i32
            ),
        )
        .await?;
        return Ok(None);
    }

    let pair = store.expect_pair(item, "buy/stock-check")?;
    if pair.item_stock < qty_i32 {
        utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Not enough stock for '{}'. Available: {}, requested: {}",
                item, pair.item_stock, qty_i32
            ),
        )
        .await?;
        return Ok(None);
    }
    let stack_size = pair.stack_size;

    let user_balance = store.users.get(&user_uuid).map(|u| u.balance).unwrap_or(0.0);
    let balance_shortfall = total_cost - user_balance;
    let diamonds_to_offer = if balance_shortfall > 0.0 {
        let ceil_value = balance_shortfall.ceil();
        if ceil_value > i32::MAX as f64 {
            utils::send_message_to_player(
                store,
                player_name,
                "Transaction amount too large (exceeds maximum diamond limit)",
            )
            .await?;
            return Ok(None);
        }
        ceil_value as i32
    } else {
        0
    };

    if user_balance + (diamonds_to_offer as f64) < total_cost {
        utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Insufficient funds. Required: {:.2}, Available balance: {:.2}, Need to offer in trade: {} diamonds",
                total_cost, user_balance, diamonds_to_offer
            ),
        )
        .await?;
        return Ok(None);
    }

    // Plan the withdrawal without cloning storage (see simulate_withdraw_plan).
    let (withdraw_plan, planned_total) = store.storage.simulate_withdraw_plan(item, qty_i32);
    if planned_total != qty_i32 {
        utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Failed to plan withdrawal for '{}' from storage. Planned {}, needed {}.",
                item, planned_total, qty_i32
            ),
        )
        .await?;
        return Ok(None);
    }

    Ok(Some(BuyPlan {
        user_uuid,
        qty_i32,
        total_cost,
        diamonds_to_offer,
        user_balance_at_plan: user_balance,
        withdraw_plan,
        physical_stock,
        stack_size,
    }))
}

/// Handle buy orders.
pub async fn handle_buy_order(
    store: &mut Store,
    player_name: &str,
    item: &str,
    quantity: u32,
) -> Result<(), StoreError> {
    info!(phase = "buy.start", player = %player_name, item = %item, qty = quantity, "Buy order starting");
    state::assert_invariants(store, "pre-buy", false)?;

    let plan = match validate_and_plan_buy(store, player_name, item, quantity).await? {
        Some(p) => p,
        None => return Ok(()), // player-facing rejection already sent
    };

    // Advance: Queued -> Withdrawing
    store.advance_trade(|s| s.begin_withdrawal(plan.withdraw_plan.clone()));

    // Execute withdrawal: bot walks the plan and pulls items into its inventory.
    if let Err(e) = execute_chest_transfers(
        store,
        &plan.withdraw_plan,
        item,
        plan.stack_size,
        ChestDirection::Withdraw,
        "[Buy]",
    )
    .await
    {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Buy aborted: bot failed chest withdrawal step: {}", e),
        )
        .await;
    }

    // Notify player of the trade terms before opening the trade GUI.
    let trade_info_msg = if plan.diamonds_to_offer > 0 {
        format!(
            "Buy {} {}: Total {:.2} diamonds. Please offer {} diamonds in the trade.",
            plan.qty_i32, item, plan.total_cost, plan.diamonds_to_offer
        )
    } else {
        format!(
            "Buy {} {}: Total {:.2} diamonds (paid from balance). No diamonds needed in trade.",
            plan.qty_i32, item, plan.total_cost
        )
    };
    utils::send_message_to_player(store, player_name, &trade_info_msg).await?;

    // Advance: Withdrawing -> Trading
    store.advance_trade(|s| s.begin_trading());

    info!(
        phase = "buy.trade",
        player = %player_name,
        item = %item,
        qty = plan.qty_i32,
        diamonds_offered = plan.diamonds_to_offer,
        "Initiating buy trade"
    );
    let player_offers = if plan.diamonds_to_offer > 0 {
        vec![TradeItem {
            item: "diamond".to_string(),
            amount: plan.diamonds_to_offer,
        }]
    } else {
        vec![]
    };
    let trade_result = perform_trade(
        store,
        player_name,
        vec![TradeItem {
            item: item.to_string(),
            amount: plan.qty_i32,
        }],
        player_offers,
        false, // buy: accept if player offers at least the required diamonds (surplus OK)
        false,
        "[Buy]",
    )
    .await;

    let actual_received = match trade_result {
        Err(err) => {
            warn!(phase = "buy.rollback", player = %player_name, item = %item, "Trade failed, rolling back: {}", err);
            let rb = rollback::deposit_transfers(
                store,
                &plan.withdraw_plan,
                item,
                plan.stack_size,
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
        Ok(r) => r,
    };

    let diamonds_received: i32 = actual_received
        .iter()
        .filter(|t| t.item == "diamond")
        .map(|t| t.amount)
        .sum();

    let current_balance = store.users.get(&plan.user_uuid).map(|u| u.balance).unwrap_or(0.0);

    // Recheck payment: with require_exact_amount=false the player could have offered
    // fewer diamonds than requested. If (diamonds + balance) doesn't cover cost, bail.
    let total_available = (diamonds_received as f64) + current_balance;
    if total_available < plan.total_cost {
        error!(
            phase = "buy.payment_check",
            player = %player_name,
            diamonds_received,
            balance = format_args!("{:.2}", current_balance),
            total_available = format_args!("{:.2}", total_available),
            total_cost = format_args!("{:.2}", plan.total_cost),
            "Insufficient payment after trade"
        );
        let _ = rollback::deposit_transfers(
            store,
            &plan.withdraw_plan,
            item,
            plan.stack_size,
            "[Buy] insufficient-payment",
        )
        .await;
        if diamonds_received > 0 {
            warn!(phase = "buy.payment_check", player = %player_name, diamonds_received, "Attempting to return diamonds after failed payment validation");
        }
        return utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Buy aborted: insufficient payment. You paid {} diamonds but need {:.2} total (your balance: {:.2}). Items rolled back.",
                diamonds_received, plan.total_cost, current_balance
            ),
        )
        .await;
    }

    // Deposit received diamonds into storage.
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
            warn!(phase = "buy.diamond_deposit", player = %player_name, operations_failed = rb.operations_failed, "Failed to deposit some diamonds into storage");
        }
    }

    // Commit ledgers.
    let current_stock = store.storage.total_item_amount(item);
    let expected_stock = plan.physical_stock - plan.qty_i32;
    if current_stock != expected_stock {
        warn!(
            phase = "buy.commit",
            player = %player_name,
            item = %item,
            expected_stock,
            current_stock,
            difference = expected_stock - current_stock,
            "Storage stock mismatch after buy"
        );
    }

    let diamonds_received_f64 = diamonds_received as f64;
    let balance_needed = plan.total_cost - diamonds_received_f64;
    let (balance_deduction, surplus) = if balance_needed > 0.0 {
        let deduction = balance_needed.min(current_balance);
        store.expect_user_mut(&plan.user_uuid, "buy/commit-deduct")?.balance -= deduction;
        (deduction, 0.0)
    } else {
        let surplus_amount = -balance_needed;
        store.expect_user_mut(&plan.user_uuid, "buy/commit-surplus")?.balance += surplus_amount;
        (0.0, surplus_amount)
    };
    store.expect_user_mut(&plan.user_uuid, "buy/commit-username")?.username = player_name.to_owned();
    store.dirty = true;

    let new_item_stock = store.storage.total_item_amount(item);
    let pair = store.expect_pair_mut(item, "buy/commit-pair")?;
    pair.item_stock = new_item_stock;
    pair.currency_stock += plan.total_cost;
    debug_assert!(pair.item_stock >= 0, "item_stock went negative after buy");
    debug_assert!(pair.currency_stock.is_finite() && pair.currency_stock >= 0.0,
        "currency_stock invalid after buy: {}", pair.currency_stock);
    store.dirty = true;

    store.trades.push(Trade::new(
        TradeType::Buy,
        ItemId::from_normalized(item.to_string()),
        plan.qty_i32,
        plan.total_cost,
        plan.user_uuid.clone(),
    ));
    store.orders.push_back(Order {
        order_type: crate::types::order::OrderType::Buy,
        item: ItemId::from_normalized(item.to_string()),
        amount: plan.qty_i32,
        user_uuid: plan.user_uuid.clone(),
    });

    // Advance: Trading -> Committed
    store.advance_trade(|s| s.commit(item.to_string(), plan.qty_i32, plan.total_cost));

    info!(
        phase = "buy.done",
        player = %player_name,
        item = %item,
        qty = quantity,
        total_cost = format_args!("{:.2}", plan.total_cost),
        diamonds_received,
        balance_used = format_args!("{:.2}", balance_deduction),
        surplus = format_args!("{:.2}", surplus),
        "Buy order completed"
    );
    let _ = plan.user_balance_at_plan; // kept for audit log context

    if let Err(e) = state::assert_invariants(store, "post-buy", true) {
        error!(phase = "buy.invariant", player = %player_name, item = %item, "Invariant violation after buy: {}", e);
        let _ = state::save(store);
    }

    let pickup_summary = utils::summarize_transfers(&plan.withdraw_plan, 3);
    let fee_amount = plan.total_cost - (plan.total_cost / (1.0 + store.config.fee));
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
            quantity, item, plan.total_cost, fee_amount, payment_msg, pickup_summary
        ),
    )
    .await
}

// ===========================================================================
// Sell order
// ===========================================================================

struct SellPlan {
    user_uuid: String,
    qty_i32: i32,
    total_payout: f64,
    whole_diamonds: i32,
    fractional_diamonds: f64,
    deposit_plan: Vec<ChestTransfer>,
    stack_size: i32,
}

async fn validate_and_plan_sell(
    store: &mut Store,
    player_name: &str,
    item: &str,
    quantity: u32,
) -> Result<Option<SellPlan>, StoreError> {
    let user_uuid = utils::resolve_user_uuid(player_name).await?;
    utils::ensure_user_exists(store, player_name, &user_uuid);

    if !store.pairs.contains_key(item) {
        warn!(phase = "sell.validate", player = %player_name, item = %item, "Attempted to sell unavailable item");
        utils::send_message_to_player(
            store,
            player_name,
            &format!("Item '{}' is not available for trading", item),
        )
        .await?;
        return Ok(None);
    }

    let qty_i32: i32 = quantity
        .try_into()
        .map_err(|_| StoreError::ValidationError("Quantity too large".to_string()))?;
    if qty_i32 <= 0 {
        utils::send_message_to_player(store, player_name, "Quantity must be positive").await?;
        return Ok(None);
    }

    let total_payout = match pricing::calculate_sell_payout(store, item, qty_i32) {
        Some(p) => p,
        None => {
            utils::send_message_to_player(
                store,
                player_name,
                &format!("Item '{}' is not available for trading (no stock or reserves).", item),
            )
            .await?;
            return Ok(None);
        }
    };

    if !total_payout.is_finite() || total_payout <= 0.0 {
        utils::send_message_to_player(store, player_name, "Internal error: computed payout is invalid.").await?;
        return Ok(None);
    }

    let pair = store.expect_pair(item, "sell/reserve-check")?;
    if pair.currency_stock < total_payout {
        utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Store has insufficient diamonds to buy that. Available reserve: {:.2}, needed: {:.2}",
                pair.currency_stock, total_payout
            ),
        )
        .await?;
        return Ok(None);
    }

    let stack_size = pair.stack_size;
    let (deposit_plan, planned_deposited) = store.storage.simulate_deposit_plan(item, qty_i32, stack_size);
    if planned_deposited < qty_i32 {
        utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Storage space validation failed for '{}': can only store {} items, but {} requested. Please contact an operator to add more storage nodes.",
                item, planned_deposited, qty_i32
            ),
        )
        .await?;
        return Ok(None);
    }

    let floor_value = total_payout.floor();
    if floor_value > i32::MAX as f64 {
        utils::send_message_to_player(
            store,
            player_name,
            "Payout amount too large (exceeds maximum diamond limit)",
        )
        .await?;
        return Ok(None);
    }
    let whole_diamonds = floor_value as i32;
    let fractional_diamonds = total_payout - (whole_diamonds as f64);

    Ok(Some(SellPlan {
        user_uuid,
        qty_i32,
        total_payout,
        whole_diamonds,
        fractional_diamonds,
        deposit_plan,
        stack_size,
    }))
}

/// Handle sell orders.
pub async fn handle_sell_order(
    store: &mut Store,
    player_name: &str,
    item: &str,
    quantity: u32,
) -> Result<(), StoreError> {
    info!(phase = "sell.start", player = %player_name, item = %item, qty = quantity, "Sell order starting");
    state::assert_invariants(store, "pre-sell", false)?;

    let plan = match validate_and_plan_sell(store, player_name, item, quantity).await? {
        Some(p) => p,
        None => return Ok(()),
    };

    // Advance: Queued -> Withdrawing (diamonds for payout)
    store.advance_trade(|s| s.begin_withdrawal(plan.deposit_plan.clone()));

    let trade_info_msg = if plan.whole_diamonds > 0 && plan.fractional_diamonds > 0.001 {
        format!(
            "Sell {} {}: You'll receive {} diamonds in trade + {:.2} to balance (total {:.2}).",
            plan.qty_i32, item, plan.whole_diamonds, plan.fractional_diamonds, plan.total_payout
        )
    } else if plan.whole_diamonds > 0 {
        format!(
            "Sell {} {}: You'll receive {} diamonds in trade.",
            plan.qty_i32, item, plan.whole_diamonds
        )
    } else {
        format!(
            "Sell {} {}: You'll receive {:.2} diamonds to balance (amount too small for trade).",
            plan.qty_i32, item, plan.total_payout
        )
    };
    utils::send_message_to_player(store, player_name, &trade_info_msg).await?;

    // Withdraw the whole-diamond portion from storage so the bot has the physical
    // coins to hand to the player during the trade GUI handoff.
    if plan.whole_diamonds > 0 {
        let (diamond_plan, planned_total) =
            store.storage.simulate_withdraw_plan("diamond", plan.whole_diamonds);
        if planned_total < plan.whole_diamonds {
            error!(
                phase = "sell.withdraw",
                player = %player_name,
                needed = plan.whole_diamonds,
                available = planned_total,
                "Insufficient physical diamonds for sell payout"
            );
            return utils::send_message_to_player(
                store,
                player_name,
                &format!(
                    "Store has insufficient physical diamonds. Storage has {}, need {}.",
                    planned_total, plan.whole_diamonds
                ),
            )
            .await;
        }

        if let Err(e) = execute_chest_transfers(
            store,
            &diamond_plan,
            "diamond",
            64,
            ChestDirection::Withdraw,
            "[Sell]",
        )
        .await
        {
            return utils::send_message_to_player(
                store,
                player_name,
                &format!("Sell aborted: failed to get diamonds from storage: {}", e),
            )
            .await;
        }
    }

    // Advance: Withdrawing -> Trading
    store.advance_trade(|s| s.begin_trading());

    info!(
        phase = "sell.trade",
        player = %player_name,
        item = %item,
        qty = plan.qty_i32,
        diamonds_offered = plan.whole_diamonds,
        "Initiating sell trade"
    );
    let bot_offers = if plan.whole_diamonds > 0 {
        vec![TradeItem {
            item: "diamond".to_string(),
            amount: plan.whole_diamonds,
        }]
    } else {
        vec![]
    };
    let trade_result = perform_trade(
        store,
        player_name,
        bot_offers,
        vec![TradeItem {
            item: item.to_string(),
            amount: plan.qty_i32,
        }],
        true, // sell: require EXACT amount
        false,
        "[Sell]",
    )
    .await;

    let actual_received = match trade_result {
        Err(err) => {
            warn!(phase = "sell.rollback", player = %player_name, item = %item, "Sell trade failed: {}", err);
            let _ = rollback::rollback_amount_to_storage(
                store,
                "diamond",
                plan.whole_diamonds,
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
        Ok(r) => r,
    };

    // Defensive recheck: even though require_exact_amount is on, we compare
    // the normalized item counts the bot actually saw.
    let target_item_id = crate::bot::Bot::normalize_item_id(item);
    let items_received: i32 = actual_received
        .iter()
        .filter(|t| crate::bot::Bot::normalize_item_id(&t.item) == target_item_id)
        .map(|t| t.amount)
        .sum();

    if items_received != plan.qty_i32 {
        warn!(
            phase = "sell.validation",
            player = %player_name,
            item = %item,
            expected = plan.qty_i32,
            received = items_received,
            "Sell validation failed: item count mismatch"
        );
        let _ = rollback::rollback_amount_to_storage(
            store,
            "diamond",
            plan.whole_diamonds,
            64,
            "[Sell] validation-failed",
        )
        .await;
        if items_received > 0 {
            let _ = perform_trade(
                store,
                player_name,
                vec![TradeItem {
                    item: item.to_string(),
                    amount: items_received,
                }],
                vec![],
                false,
                false,
                "[Sell] return-items",
            )
            .await;
        }
        return utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Sell REJECTED: You only put {} {} in the trade but promised {}. Trade cancelled, items returned.",
                items_received, item, plan.qty_i32
            ),
        )
        .await;
    }

    // Advance: Trading -> Depositing
    store.advance_trade(|s| s.begin_depositing(
        super::trade_state::TradeResult { items_received: actual_received.clone() },
        plan.deposit_plan.clone(),
    ));

    // Deposit items from bot inventory into storage.
    if let Err(err) = execute_chest_transfers(
        store,
        &plan.deposit_plan,
        item,
        plan.stack_size,
        ChestDirection::Deposit,
        "[Sell]",
    )
    .await
    {
        // Best-effort return of the ORIGINAL qty via trade - partial deposits can't be
        // cleanly unwound because the bot has already committed earlier steps.
        let _ = perform_trade(
            store,
            player_name,
            vec![TradeItem {
                item: item.to_string(),
                amount: plan.qty_i32,
            }],
            vec![],
            false,
            false,
            "[Sell] deposit-failed",
        )
        .await;
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

    // Commit ledgers.
    {
        let user = store.expect_user_mut(&plan.user_uuid, "sell/commit-user")?;
        user.balance += plan.fractional_diamonds;
        user.username = player_name.to_owned();
    }
    store.dirty = true;
    let new_item_stock = store.storage.total_item_amount(item);
    let pair = store.expect_pair_mut(item, "sell/commit-pair")?;
    pair.item_stock = new_item_stock;
    pair.currency_stock -= plan.total_payout;
    debug_assert!(pair.item_stock >= 0, "item_stock went negative after sell");
    debug_assert!(pair.currency_stock.is_finite() && pair.currency_stock >= 0.0,
        "currency_stock invalid after sell: {}", pair.currency_stock);
    store.dirty = true;

    store.trades.push(Trade::new(
        TradeType::Sell,
        ItemId::from_normalized(item.to_string()),
        plan.qty_i32,
        plan.total_payout,
        plan.user_uuid.clone(),
    ));
    store.orders.push_back(Order {
        order_type: crate::types::order::OrderType::Sell,
        item: ItemId::from_normalized(item.to_string()),
        amount: plan.qty_i32,
        user_uuid: plan.user_uuid.clone(),
    });

    // Advance: Depositing -> Committed
    store.advance_trade(|s| s.commit(item.to_string(), plan.qty_i32, plan.total_payout));

    info!(
        phase = "sell.done",
        player = %player_name,
        item = %item,
        qty = quantity,
        total_payout = format_args!("{:.2}", plan.total_payout),
        whole_diamonds = plan.whole_diamonds,
        fractional = format_args!("{:.2}", plan.fractional_diamonds),
        "Sell order completed"
    );

    if let Err(e) = state::assert_invariants(store, "post-sell", true) {
        error!(phase = "sell.invariant", player = %player_name, item = %item, "Invariant violation after sell: {}", e);
        let _ = state::save(store);
    }

    let deposit_summary = utils::summarize_transfers(&plan.deposit_plan, 3);
    let fee_amount = plan.total_payout / (1.0 - store.config.fee) - plan.total_payout;
    utils::send_message_to_player(
        store,
        player_name,
        &format!(
            "Sold {} {} for {:.2} diamonds (fee {:.2}). Trade complete. Storage: {}",
            quantity, item, plan.total_payout, fee_amount, deposit_summary
        ),
    )
    .await
}

// ===========================================================================
// Queue dispatcher
// ===========================================================================

/// Execute a queued order.
///
/// Dispatches to the appropriate handler based on order type. Returns a
/// success message on completion or an error message on failure. The
/// handlers themselves send messages to the player during execution.
pub async fn execute_queued_order(
    store: &mut Store,
    order: &QueuedOrder,
) -> Result<String, StoreError> {
    info!(
        order_id = order.id,
        phase = "order.start",
        player = %order.username,
        order_type = ?order.order_type,
        item = %order.item,
        qty = order.quantity,
        "Executing queued order"
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
        Ok(msg) => info!(
            order_id = order.id,
            phase = "order.done",
            player = %order.username,
            duration_ms = elapsed.as_millis() as u64,
            "Order completed: {}", msg
        ),
        Err(msg) => error!(
            order_id = order.id,
            phase = "order.failed",
            player = %order.username,
            duration_ms = elapsed.as_millis() as u64,
            "Order failed: {}", msg
        ),
    }

    result
}

#[cfg(test)]
mod tests {
    //! Integration tests for order handlers.
    //!
    //! These tests construct a `Store` entirely in-memory via
    //! `Store::new_for_test` and spawn a mock bot task that auto-responds to
    //! every `BotInstruction`. Username→UUID resolution is stubbed in
    //! `utils::resolve_user_uuid` under `#[cfg(test)]` so no Mojang API calls
    //! happen.

    use super::*;
    use crate::config::Config;
    use crate::messages::{BotInstruction, ChestSyncReport};
    use crate::store::handlers::player;
    use crate::types::{Chest, Node, Pair, Position, Storage, User};
    use std::collections::HashMap;
    use tokio::sync::mpsc;

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

    fn test_uuid(username: &str) -> String {
        let trimmed: String = username.chars().take(12).collect();
        let padded = format!("{:0>12}", trimmed);
        format!("00000000-0000-0000-0000-{}", padded)
    }

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
    /// `item` spread across chest 2 (arbitrary non-reserved chest of node 0).
    fn make_storage(item: &str, stock: i32) -> Storage {
        let origin = Position { x: 0, y: 64, z: 0 };
        let mut storage = Storage::new(&origin);
        let node = Node::new(0, &origin);
        storage.nodes.push(node);
        // Fill chest index 2 (not reserved diamond/overflow) with the item.
        // Pack into a few shulker slots; the planner splits as needed.
        let chest: &mut Chest = &mut storage.nodes[0].chests[2];
        chest.item = ItemId::from_normalized(item.to_string());
        // Put all stock in slot 0 for simplicity (within default shulker capacity).
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

    /// Spawn a mock bot task that auto-responds to every `BotInstruction` with
    /// a synthetic success. For `InteractWithChestAndSync`, the response
    /// adjusts the reported per-slot counts to match what a real bot would
    /// have done (subtracting on withdraw, adding on deposit) for a single
    /// slot at index 0 of the target chest.
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
                        // Mirror the action into a slot-count report. The
                        // real bot returns -1 for "unchanged" on untouched
                        // slots; we emit a single non-negative value for
                        // slot 0 so `apply_chest_sync` has something to
                        // merge and the rest stay as-is.
                        let (item, delta) = match action {
                            crate::messages::ChestAction::Withdraw {
                                item, amount, ..
                            } => (item, -amount),
                            crate::messages::ChestAction::Deposit {
                                item, amount, ..
                            } => (item, amount),
                        };
                        let mut amounts = [-1i32; crate::constants::DOUBLE_CHEST_SLOTS];
                        // Compute new value for slot 0 based on the prior state
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
                        player_offers: items_player_must_give,
                        respond_to,
                        ..
                    } => {
                        // Simulate a successful trade: the bot reports that
                        // the player delivered exactly what was requested.
                        // `perform_trade` returns the items the bot received
                        // from the player, which is always `player_offers`
                        // from the instruction (diamonds on a buy, the traded
                        // item on a sell). Callers use this list for
                        // post-trade accounting.
                        let _ = respond_to.send(Ok(items_player_must_give));
                    }
                    _ => {
                        // Other variants aren't exercised by these tests.
                    }
                }
            }
        });
    }

    #[tokio::test]
    async fn test_buy_out_of_stock_rejected() {
        let (tx, rx) = mpsc::channel(64);
        spawn_mock_bot(rx);

        let mut users = HashMap::new();
        let (uuid, user) = make_user("Alice", 1000.0);
        users.insert(uuid.clone(), user);

        let mut pairs = HashMap::new();
        // Pair stock matches physical storage (invariant) but is below qty.
        let (k, p) = make_pair("cobblestone", 50, 500.0);
        pairs.insert(k, p);

        let storage = make_storage("cobblestone", 50);
        let mut store = Store::new_for_test(tx, test_config(), pairs, users, storage);

        // Request more than physical storage holds — handler rejects during
        // validation, before any bot instruction is sent.
        let result = handle_buy_order(&mut store, "Alice", "cobblestone", 500).await;

        assert!(result.is_ok(), "handler should not propagate error: {:?}", result);
        assert_eq!(store.users.get(&uuid).unwrap().balance, 1000.0);
        assert_eq!(store.pairs.get("cobblestone").unwrap().item_stock, 50);
    }

    #[tokio::test]
    async fn test_buy_unknown_item_rejected() {
        let (tx, rx) = mpsc::channel(64);
        spawn_mock_bot(rx);

        let mut users = HashMap::new();
        let (uuid, user) = make_user("Bob", 100.0);
        users.insert(uuid, user);

        let storage = make_storage("cobblestone", 0);
        let mut store = Store::new_for_test(
            tx,
            test_config(),
            HashMap::new(),
            users,
            storage,
        );

        let result = handle_buy_order(&mut store, "Bob", "gunpowder", 10).await;
        assert!(result.is_ok());
        // No pair created, no user balance change.
        assert!(!store.pairs.contains_key("gunpowder"));
    }

    #[tokio::test]
    async fn test_pay_transfer_updates_both_balances() {
        let (tx, rx) = mpsc::channel(64);
        spawn_mock_bot(rx);

        let mut users = HashMap::new();
        let (payer_uuid, payer) = make_user("Payer", 50.0);
        let (payee_uuid, payee) = make_user("Payee", 10.0);
        users.insert(payer_uuid.clone(), payer);
        users.insert(payee_uuid.clone(), payee);

        let storage = Storage::new(&Position { x: 0, y: 64, z: 0 });
        let mut store = Store::new_for_test(
            tx,
            test_config(),
            HashMap::new(),
            users,
            storage,
        );

        let result = player::pay_async(&mut store, "Payer", "Payee", 20.0).await;
        assert!(result.is_ok(), "pay failed: {:?}", result);
        assert_eq!(store.users.get(&payer_uuid).unwrap().balance, 30.0);
        assert_eq!(store.users.get(&payee_uuid).unwrap().balance, 30.0);
    }

    #[tokio::test]
    async fn test_pay_insufficient_balance_rejected() {
        let (tx, rx) = mpsc::channel(64);
        spawn_mock_bot(rx);

        let mut users = HashMap::new();
        let (payer_uuid, payer) = make_user("Poor", 5.0);
        let (payee_uuid, payee) = make_user("Rich", 100.0);
        users.insert(payer_uuid.clone(), payer);
        users.insert(payee_uuid.clone(), payee);

        let storage = Storage::new(&Position { x: 0, y: 64, z: 0 });
        let mut store = Store::new_for_test(
            tx,
            test_config(),
            HashMap::new(),
            users,
            storage,
        );

        let result = player::pay_async(&mut store, "Poor", "Rich", 50.0).await;
        assert!(result.is_err());
        // Balances unchanged.
        assert_eq!(store.users.get(&payer_uuid).unwrap().balance, 5.0);
        assert_eq!(store.users.get(&payee_uuid).unwrap().balance, 100.0);
    }

    #[tokio::test]
    async fn test_sell_unknown_item_rejected() {
        let (tx, rx) = mpsc::channel(64);
        spawn_mock_bot(rx);

        let mut users = HashMap::new();
        let (uuid, user) = make_user("Seller", 0.0);
        users.insert(uuid.clone(), user);

        let storage = make_storage("cobblestone", 0);
        let mut store = Store::new_for_test(
            tx,
            test_config(),
            HashMap::new(),
            users,
            storage,
        );

        let result = handle_sell_order(&mut store, "Seller", "gunpowder", 10).await;
        assert!(result.is_ok());
        assert_eq!(store.users.get(&uuid).unwrap().balance, 0.0);
        assert!(!store.pairs.contains_key("gunpowder"));
    }

    #[tokio::test]
    async fn test_sell_zero_quantity_rejected() {
        let (tx, rx) = mpsc::channel(64);
        spawn_mock_bot(rx);

        let mut users = HashMap::new();
        let (uuid, user) = make_user("Zed", 0.0);
        users.insert(uuid.clone(), user);

        let mut pairs = HashMap::new();
        let (k, p) = make_pair("cobblestone", 100, 500.0);
        pairs.insert(k, p);

        let storage = make_storage("cobblestone", 100);
        let mut store = Store::new_for_test(tx, test_config(), pairs, users, storage);

        let result = handle_sell_order(&mut store, "Zed", "cobblestone", 0).await;
        assert!(result.is_ok());
        // Reserves unchanged.
        assert_eq!(store.pairs.get("cobblestone").unwrap().item_stock, 100);
        assert_eq!(store.pairs.get("cobblestone").unwrap().currency_stock, 500.0);
    }

    #[tokio::test]
    async fn test_deposit_non_positive_amount_rejected() {
        let (tx, rx) = mpsc::channel(64);
        spawn_mock_bot(rx);

        let mut users = HashMap::new();
        let (uuid, user) = make_user("Depositor", 10.0);
        users.insert(uuid.clone(), user);

        let storage = Storage::new(&Position { x: 0, y: 64, z: 0 });
        let mut store = Store::new_for_test(
            tx,
            test_config(),
            HashMap::new(),
            users,
            storage,
        );

        // Zero amount
        let result = player::handle_deposit_balance_queued(&mut store, "Depositor", Some(0.0)).await;
        assert!(result.is_ok(), "handler should reject gracefully: {:?}", result);
        assert_eq!(store.users.get(&uuid).unwrap().balance, 10.0);

        // Negative amount
        let result = player::handle_deposit_balance_queued(&mut store, "Depositor", Some(-5.0)).await;
        assert!(result.is_ok(), "handler should reject gracefully: {:?}", result);
        assert_eq!(store.users.get(&uuid).unwrap().balance, 10.0);
    }

    #[tokio::test]
    async fn test_deposit_over_max_rejected() {
        let (tx, rx) = mpsc::channel(64);
        spawn_mock_bot(rx);

        let mut users = HashMap::new();
        let (uuid, user) = make_user("BigSpender", 0.0);
        users.insert(uuid.clone(), user);

        let storage = Storage::new(&Position { x: 0, y: 64, z: 0 });
        let mut store = Store::new_for_test(
            tx,
            test_config(),
            HashMap::new(),
            users,
            storage,
        );

        // 12 stacks * 64 = 768 is the cap; 1000 exceeds it.
        let result = player::handle_deposit_balance_queued(&mut store, "BigSpender", Some(1000.0)).await;
        assert!(result.is_ok());
        assert_eq!(store.users.get(&uuid).unwrap().balance, 0.0);
    }

    #[tokio::test]
    async fn test_withdraw_insufficient_balance_rejected() {
        let (tx, rx) = mpsc::channel(64);
        spawn_mock_bot(rx);

        let mut users = HashMap::new();
        let (uuid, user) = make_user("Broke", 3.0);
        users.insert(uuid.clone(), user);

        let storage = Storage::new(&Position { x: 0, y: 64, z: 0 });
        let mut store = Store::new_for_test(
            tx,
            test_config(),
            HashMap::new(),
            users,
            storage,
        );

        let result = player::handle_withdraw_balance_queued(&mut store, "Broke", Some(50.0)).await;
        assert!(result.is_ok(), "handler should reject gracefully: {:?}", result);
        assert_eq!(store.users.get(&uuid).unwrap().balance, 3.0);
    }

    #[tokio::test]
    async fn test_withdraw_non_positive_amount_rejected() {
        let (tx, rx) = mpsc::channel(64);
        spawn_mock_bot(rx);

        let mut users = HashMap::new();
        let (uuid, user) = make_user("Neg", 100.0);
        users.insert(uuid.clone(), user);

        let storage = Storage::new(&Position { x: 0, y: 64, z: 0 });
        let mut store = Store::new_for_test(
            tx,
            test_config(),
            HashMap::new(),
            users,
            storage,
        );

        let result = player::handle_withdraw_balance_queued(&mut store, "Neg", Some(-1.0)).await;
        assert!(result.is_ok());
        assert_eq!(store.users.get(&uuid).unwrap().balance, 100.0);

        let result = player::handle_withdraw_balance_queued(&mut store, "Neg", Some(0.0)).await;
        assert!(result.is_ok());
        assert_eq!(store.users.get(&uuid).unwrap().balance, 100.0);
    }

    #[tokio::test]
    async fn test_withdraw_full_balance_zero_rejected() {
        let (tx, rx) = mpsc::channel(64);
        spawn_mock_bot(rx);

        let mut users = HashMap::new();
        let (uuid, user) = make_user("Empty", 0.5);
        users.insert(uuid.clone(), user);

        let storage = Storage::new(&Position { x: 0, y: 64, z: 0 });
        let mut store = Store::new_for_test(
            tx,
            test_config(),
            HashMap::new(),
            users,
            storage,
        );

        // Full-balance withdraw (amount=None) on <1 diamond balance: rejected.
        let result = player::handle_withdraw_balance_queued(&mut store, "Empty", None).await;
        assert!(result.is_ok());
        assert_eq!(store.users.get(&uuid).unwrap().balance, 0.5);
    }
}
