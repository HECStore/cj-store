//! `withdraw` / `w` command: enqueue handler + queued-order processor.

use tracing::{debug, info, warn};

use super::super::{Store, state, utils};
use crate::constants::{CHEST_OP_TIMEOUT_SECS, CHESTS_PER_NODE};
use crate::error::StoreError;
use crate::messages::QueuedOrderType;
use crate::types::ItemId;

// =========================================================================
// Dispatcher entry point (enqueue)
// =========================================================================

pub(super) async fn handle_enqueue(
    store: &mut Store,
    player_name: &str,
    user_uuid: &str,
    amount: Option<f64>,
) -> Result<(), StoreError> {
    debug!(
        "Queueing withdraw order: {} amount={:?}",
        player_name, amount
    );

    match store.order_queue.add(
        user_uuid.to_string(),
        player_name.to_string(),
        QueuedOrderType::Withdraw { amount },
        "diamond".to_string(),
        0,
    ) {
        Ok((order_id, position)) => {
            let queue_len = store.order_queue.len();
            let wait_estimate = store.order_queue.estimate_wait(position);
            let amount_str = match amount {
                Some(amt) => format!("{:.2} diamonds", amt),
                None => "full balance".to_string(),
            };
            let msg = format!(
                "Withdraw {} order #{} queued (position {}/{}). Est. wait: {}.",
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

/// Handle withdraw balance (user withdraws diamonds from their balance).
///
/// If `amount` is Some, withdraws that amount (floor'd to whole diamonds for the trade).
/// If `amount` is None, withdraws full balance as whole diamonds.
///
/// This is a public function called by the order queue processor.
pub async fn handle_withdraw_balance_queued(
    store: &mut Store,
    player_name: &str,
    amount: Option<f64>,
) -> Result<(), StoreError> {
    info!("[Withdraw] Starting: player={} amount={:?}", player_name, amount);
    state::assert_invariants(store, "pre-withdraw-balance", false)?;

    // Maximum diamonds the trade GUI can hold per transaction: 12 slots
    // times a 64-stack each = 768 diamonds. See handle_deposit_balance_queued
    // for the full rationale - the cap comes from the vanilla trade window
    // layout, not from any arbitrary policy.
    const MAX_TRADE_DIAMONDS: i32 = 12 * 64; // 768

    let user_uuid = utils::resolve_user_uuid(store, player_name).await?;
    utils::ensure_user_exists(store, player_name, &user_uuid);

    let user_balance = store.expect_user(&user_uuid, "withdraw-balance/pre-check")?.balance;

    let amount = match amount {
        Some(amt) => {
            if !amt.is_finite() || amt <= 0.0 {
                return utils::send_message_to_player(store, player_name, "Amount must be positive")
                    .await;
            }
            amt
        }
        None => {
            let whole_balance = user_balance.floor();
            if whole_balance <= 0.0 {
                return utils::send_message_to_player(
                    store,
                    player_name,
                    &format!("No whole diamonds to withdraw. Balance: {:.2} (need at least 1.00)", user_balance),
                ).await;
            }
            // If the user's balance exceeds the per-trade cap, tell them
            // explicitly so they know to issue another /withdraw for the rest
            // instead of assuming the full balance came out in one go.
            if whole_balance > MAX_TRADE_DIAMONDS as f64 {
                utils::send_message_to_player(
                    store,
                    player_name,
                    &format!(
                        "Balance {:.2} exceeds the per-trade cap of {} diamonds; withdrawing {} this transaction. Use /withdraw again for the rest.",
                        user_balance, MAX_TRADE_DIAMONDS, MAX_TRADE_DIAMONDS
                    ),
                ).await?;
            }
            whole_balance.min(MAX_TRADE_DIAMONDS as f64)
        }
    };

    if user_balance < amount {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Insufficient balance. Required: {:.2}, Available: {:.2}",
                amount, user_balance
            ),
        )
        .await;
    }

    let whole_diamonds = amount.floor() as i32;

    if whole_diamonds > MAX_TRADE_DIAMONDS {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Amount too large. Maximum withdrawal is {} diamonds (12 stacks) per transaction.", MAX_TRADE_DIAMONDS),
        )
        .await;
    }

    let withdraw_msg = if whole_diamonds > 0 {
        format!("Withdraw {:.2} diamonds: You'll receive {} diamonds in trade.", amount, whole_diamonds)
    } else {
        format!("Withdraw {:.2} diamonds: Amount too small for trade (must be at least 1 whole diamond).", amount)
    };
    utils::send_message_to_player(store, player_name, &withdraw_msg).await?;

    if whole_diamonds <= 0 {
        return utils::send_message_to_player(
            store,
            player_name,
            "Withdraw requires at least 1 whole diamond. Use a larger amount.",
        ).await;
    }

    // Advance: Queued -> Withdrawing (diamonds from storage)
    store.advance_trade(|s| s.begin_withdrawal(vec![]));

    // Withdraw diamonds from storage (diamond chest) before trading.
    if whole_diamonds > 0 {
        let (withdraw_plan, preview_withdrawn) =
            store.storage.simulate_withdraw_plan("diamond", whole_diamonds);

        if preview_withdrawn < whole_diamonds {
            tracing::error!(
                "[Withdraw] Insufficient physical diamonds: need {}, storage has {}",
                whole_diamonds, preview_withdrawn
            );
            return utils::send_message_to_player(
                store,
                player_name,
                &format!(
                    "Insufficient physical diamonds in storage. Storage has {}, need {}.",
                    preview_withdrawn, whole_diamonds
                ),
            )
            .await;
        }

        for t in &withdraw_plan {
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
                    action: crate::messages::ChestAction::Withdraw {
                        item: "diamond".to_string(),
                        amount: t.amount,
                        to_player: None,
                        stack_size: 64,
                    },
                    respond_to: tx,
                })
                .await;

            if let Err(e) = send_result {
                tracing::error!("[Withdraw] Failed to send chest instruction: {}", e);
                return Err(StoreError::BotError(format!(
                    "Failed to send chest instruction to bot: {}",
                    e
                )));
            }

            let bot_result = match tokio::time::timeout(
                tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS),
                rx,
            )
            .await
            {
                Ok(Ok(result)) => result,
                Ok(Err(e)) => {
                    tracing::error!("[Withdraw] Channel dropped: {}", e);
                    return Err(StoreError::BotError(format!("Bot response dropped: {}", e)));
                }
                Err(_) => {
                    tracing::error!("[Withdraw] Timeout waiting for bot");
                    return Err(StoreError::ChestOp(
                        "Bot timed out withdrawing diamonds from storage".to_string(),
                    ));
                }
            };

            match bot_result {
                Err(err) => {
                    tracing::error!("[Withdraw] Diamond withdrawal failed: {}", err);
                    return utils::send_message_to_player(
                        store,
                        player_name,
                        &format!("Withdraw aborted: failed to get diamonds from storage: {}", err),
                    )
                    .await;
                }
                Ok(report) => {
                    if let Err(e) = store.apply_chest_sync(report) {
                        tracing::warn!("[Withdraw] Chest sync failed after diamond withdrawal: {}", e);
                    }
                }
            }
        }
    }

    // NOTE: Do NOT deduct balance yet - wait until trade succeeds.
    // Ordering keeps physical diamonds and ledger balance in lockstep if the
    // trade fails: the rollback path below redeposits physical diamonds back
    // into storage, and because the balance was never touched, no balance
    // restoration is needed.

    store.advance_trade(|s| s.begin_trading());

    info!("[Withdraw] Initiating trade: {} diamonds to {}", whole_diamonds, player_name);
    let (trade_tx, trade_rx) = tokio::sync::oneshot::channel();
    let trade_send_result = store.bot_tx
        .send(crate::messages::BotInstruction::TradeWithPlayer {
            target_username: player_name.to_string(),
            bot_offers: if whole_diamonds > 0 {
                vec![crate::messages::TradeItem {
                    item: "diamond".to_string(),
                    amount: whole_diamonds,
                }]
            } else {
                vec![]
            },
            player_offers: vec![],
            require_exact_amount: false,
            flexible_validation: false,
            respond_to: trade_tx,
        })
        .await;

    if let Err(e) = trade_send_result {
        tracing::error!("[Withdraw] Failed to send trade instruction: {}", e);
        let _ = super::super::rollback::rollback_amount_to_storage(
            store,
            "diamond",
            whole_diamonds,
            64,
            "[Withdraw] trade-send-failed",
        )
        .await;
        return Err(StoreError::BotError(format!(
            "Failed to send trade instruction to bot: {}",
            e
        )));
    }

    let trade_result = match tokio::time::timeout(
        tokio::time::Duration::from_millis(store.config.trade_timeout_ms),
        trade_rx,
    )
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => {
            tracing::error!("[Withdraw] Trade channel dropped: {}", e);
            let _ = super::super::rollback::rollback_amount_to_storage(
                store,
                "diamond",
                whole_diamonds,
                64,
                "[Withdraw] channel-dropped",
            )
            .await;
            return utils::send_message_to_player(
                store,
                player_name,
                &format!("Withdraw aborted: bot response dropped: {}", e),
            )
            .await;
        }
        Err(_) => {
            tracing::error!("[Withdraw] Trade timeout");
            let _ = super::super::rollback::rollback_amount_to_storage(
                store,
                "diamond",
                whole_diamonds,
                64,
                "[Withdraw] timeout",
            )
            .await;
            return utils::send_message_to_player(
                store,
                player_name,
                "Withdraw aborted: bot timed out waiting for trade completion. Diamonds returned to storage.",
            )
            .await;
        }
    };

    if let Err(ref err) = trade_result {
        warn!("[Withdraw] Trade failed: {}", err);
        let _ = super::super::rollback::rollback_amount_to_storage(
            store,
            "diamond",
            whole_diamonds,
            64,
            "[Withdraw] trade-failed",
        )
        .await;

        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Withdraw aborted: trade failed: {}. Diamonds returned to storage.", err),
        )
        .await;
    }

    // Trade succeeded - NOW deduct from balance
    {
        let user = store.expect_user_mut(&user_uuid, "withdraw-balance/commit")?;
        user.balance -= amount;
        user.username = player_name.to_owned();
    }
    store.dirty = true;

    store.orders.push_back(crate::types::Order {
        order_type: crate::types::order::OrderType::WithdrawBalance,
        item: ItemId::from_normalized("diamond".to_string()),
        amount: whole_diamonds,
        user_uuid: user_uuid.clone(),
    });

    store.trades.push(crate::types::Trade::new(
        crate::types::TradeType::WithdrawBalance,
        ItemId::from_normalized("diamond".to_string()),
        whole_diamonds,
        amount,
        user_uuid.clone(),
    ));

    store.advance_trade(|s| s.commit("diamond".to_string(), whole_diamonds, amount));

    info!("[Withdraw] Completed: user={} amount={}", player_name, amount);

    if let Err(e) = state::assert_invariants(store, "post-withdraw-balance", true) {
        tracing::error!("Invariant violation after withdraw balance: {}", e);
        let _ = state::save(store);
    }

    let remaining_balance = store.expect_user(&user_uuid, "withdraw-balance/post-read")?.balance;
    utils::send_message_to_player(
        store,
        player_name,
        &format!("Withdrew {:.2} diamonds from your balance ({} whole diamonds in trade). Remaining balance: {:.2}", amount, whole_diamonds, remaining_balance),
    )
    .await
}
