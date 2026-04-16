//! `deposit` / `d` command: enqueue handler + queued-order processor.

use tracing::{debug, info, warn};

use super::super::{Store, state, utils};
use crate::messages::QueuedOrderType;
use crate::types::ItemId;

// =========================================================================
// Dispatcher entry point (enqueue)
// =========================================================================

pub(super) async fn handle_enqueue(
    store: &mut Store,
    player_name: &str,
    user_uuid: &str,
    parts: &[&str],
) -> Result<(), String> {
    let amount: Option<f64> = if parts.len() >= 2 {
        match parts[1].parse() {
            Ok(amt) => {
                if amt <= 0.0 {
                    return utils::send_message_to_player(
                        store,
                        player_name,
                        "Amount must be positive",
                    )
                    .await;
                }
                Some(amt)
            }
            Err(_) => {
                return utils::send_message_to_player(
                    store,
                    player_name,
                    &format!(
                        "Invalid amount '{}'. Use a number. Example: deposit 64",
                        parts[1]
                    ),
                )
                .await;
            }
        }
    } else {
        None
    };

    debug!(
        "Queueing deposit order: {} amount={:?}",
        player_name, amount
    );

    match store.order_queue.add(
        user_uuid.to_string(),
        player_name.to_string(),
        QueuedOrderType::Deposit { amount },
        "diamond".to_string(),
        0,
    ) {
        Ok((order_id, position)) => {
            let queue_len = store.order_queue.len();
            let wait_estimate = store.order_queue.estimate_wait(position);
            let amount_str = match amount {
                Some(amt) => format!("{:.2} diamonds", amt),
                None => "diamonds (flexible)".to_string(),
            };
            let msg = format!(
                "Deposit {} order #{} queued (position {}/{}). Est. wait: {}.",
                amount_str, order_id, position, queue_len, wait_estimate
            );
            utils::send_message_to_player(store, player_name, &msg).await
        }
        Err(e) => utils::send_message_to_player(store, player_name, &e).await,
    }
}

// =========================================================================
// Queued-order processor (called from orders.rs)
// =========================================================================

/// Handle deposit balance (user deposits diamonds to their balance)
///
/// If `amount` is Some, expects exactly that many diamonds (ceil'd to whole diamonds).
/// If `amount` is None, flexible deposit - credits whatever the player puts in (up to 12 stacks = 768 diamonds).
///
/// This is a public function called by the order queue processor.
pub async fn handle_deposit_balance_queued(
    store: &mut Store,
    player_name: &str,
    amount: Option<f64>,
) -> Result<(), String> {
    info!("[Deposit] Starting: player={} amount={:?}", player_name, amount);
    state::assert_invariants(store, "pre-deposit-balance", false)?;

    // Maximum diamonds the trade GUI can hold (12 stacks of 64 = 768).
    // Rationale: Minecraft's vanilla trade/offer UI exposes 12 offer slots
    // (4x3 grid on each side). Each slot holds at most one 64-stack of
    // diamonds, so a single trade round-trip can move at most 768 diamonds.
    // Requests larger than this cannot fit into one trade window and must
    // be split - we reject them at the handler rather than silently
    // truncating so the player isn't surprised by a partial transaction.
    const MAX_TRADE_DIAMONDS: i32 = 12 * 64; // 768

    let (diamonds_to_trade, is_flexible) = match amount {
        Some(amt) => {
            if !amt.is_finite() || amt <= 0.0 {
                return utils::send_message_to_player(
                    store,
                    player_name,
                    "Amount must be positive",
                )
                .await;
            }
            let diamonds = amt.ceil() as i32;
            if diamonds > MAX_TRADE_DIAMONDS {
                return utils::send_message_to_player(
                    store,
                    player_name,
                    &format!(
                        "Amount too large. Maximum deposit is {} diamonds (12 stacks).",
                        MAX_TRADE_DIAMONDS
                    ),
                )
                .await;
            }
            (diamonds, false)
        }
        None => (MAX_TRADE_DIAMONDS, true),
    };

    let user_uuid = utils::resolve_user_uuid(store, player_name).await?;
    utils::ensure_user_exists(store, player_name, &user_uuid);

    let msg = if is_flexible {
        format!(
            "Deposit: Please offer diamonds in the trade (up to {} diamonds / 12 stacks). You'll be credited for the actual amount.",
            MAX_TRADE_DIAMONDS
        )
    } else {
        format!(
            "Deposit {:.2} diamonds: Please offer {} diamonds in the trade.",
            amount.unwrap(),
            diamonds_to_trade
        )
    };
    utils::send_message_to_player(store, player_name, &msg).await?;

    // Advance: Queued -> Withdrawing (empty plan) -> Trading
    store.advance_trade(|s| s.begin_withdrawal(vec![]));
    store.advance_trade(|s| s.begin_trading());

    info!(
        "[Deposit] Initiating trade: {} offers up to {} diamonds (flexible={})",
        player_name, diamonds_to_trade, is_flexible
    );
    let (trade_tx, trade_rx) = tokio::sync::oneshot::channel();
    let send_result = store
        .bot_tx
        .send(crate::messages::BotInstruction::TradeWithPlayer {
            target_username: player_name.to_string(),
            bot_offers: vec![],
            player_offers: vec![crate::messages::TradeItem {
                item: "diamond".to_string(),
                amount: if is_flexible { 1 } else { diamonds_to_trade },
            }],
            require_exact_amount: false,
            flexible_validation: is_flexible,
            respond_to: trade_tx,
        })
        .await;

    if let Err(e) = send_result {
        tracing::error!("[Deposit] Failed to send trade instruction: {}", e);
        return Err(format!("Failed to send trade instruction to bot: {}", e));
    }

    let trade_result = match tokio::time::timeout(
        tokio::time::Duration::from_millis(store.config.trade_timeout_ms),
        trade_rx,
    )
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => {
            tracing::error!("[Deposit] Trade channel dropped: {}", e);
            return Err(format!("Bot response dropped: {}", e));
        }
        Err(_) => {
            tracing::error!("[Deposit] Trade timeout");
            return Err("Bot timed out waiting for trade completion".to_string());
        }
    };

    let actual_received = match trade_result {
        Err(err) => {
            warn!("[Deposit] Trade failed: {}", err);
            return utils::send_message_to_player(
                store,
                player_name,
                &format!("Deposit aborted: trade failed: {}", err),
            )
            .await;
        }
        Ok(received) => received,
    };

    let diamonds_actually_received: i32 = actual_received
        .iter()
        .filter(|t| t.item == "diamond")
        .map(|t| t.amount)
        .sum();

    if diamonds_actually_received <= 0 {
        return utils::send_message_to_player(
            store,
            player_name,
            "Deposit aborted: no diamonds received in trade",
        )
        .await;
    }

    let rb = super::super::rollback::rollback_amount_to_storage(
        store,
        "diamond",
        diamonds_actually_received,
        64,
        "[Deposit]",
    )
    .await;
    if rb.has_failures() {
        tracing::warn!(
            "[Deposit] Failed to deposit {} diamond step(s) into storage - some diamonds may remain in bot inventory",
            rb.operations_failed
        );
    }

    let actual_amount = diamonds_actually_received as f64;
    let new_balance = {
        let user = store.expect_user_mut(&user_uuid, "deposit-balance/credit")?;
        user.balance += actual_amount;
        user.username = player_name.to_owned();
        user.balance
    };
    store.dirty = true;

    store.orders.push_back(crate::types::Order {
        order_type: crate::types::order::OrderType::DepositBalance,
        item: ItemId::from_normalized("diamond".to_string()),
        amount: diamonds_actually_received,
        user_uuid: user_uuid.clone(),
    });

    store.trades.push(crate::types::Trade::new(
        crate::types::TradeType::DepositBalance,
        ItemId::from_normalized("diamond".to_string()),
        diamonds_actually_received,
        actual_amount,
        user_uuid.clone(),
    ));

    store.advance_trade(|s| {
        s.commit("diamond".to_string(), diamonds_actually_received, actual_amount)
    });

    info!(
        "[Deposit] Completed: user={} amount={}",
        player_name, actual_amount
    );

    if let Err(e) = state::assert_invariants(store, "post-deposit-balance", true) {
        tracing::error!("Invariant violation after deposit balance: {}", e);
        let _ = state::save(store);
    }

    utils::send_message_to_player(
        store,
        player_name,
        &format!(
            "Deposited {:.2} diamonds to your balance. New balance: {:.2}",
            actual_amount, new_balance
        ),
    )
    .await
}
