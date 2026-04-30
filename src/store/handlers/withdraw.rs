//! `withdraw` / `w` command: enqueue handler + queued-order processor.

use tracing::{debug, error, info, warn};

use super::super::{Store, state, utils};
use crate::constants::{CHEST_OP_TIMEOUT_SECS, CHESTS_PER_NODE, MAX_TRADE_DIAMONDS};
use crate::error::StoreError;
use crate::messages::QueuedOrderType;
use crate::types::ItemId;

pub(super) async fn handle_enqueue(
    store: &mut Store,
    player_name: &str,
    user_uuid: &str,
    amount: Option<f64>,
) -> Result<(), StoreError> {
    debug!(
        player = player_name,
        uuid = user_uuid,
        amount = ?amount,
        "Queueing withdraw order"
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

/// Withdraw diamonds from the user's balance and hand them over in a trade.
///
/// `amount = Some(x)`: withdraws `x` (floored to whole diamonds for the trade).
/// `amount = None`: withdraws the full balance, capped at `MAX_TRADE_DIAMONDS`
/// (the user is told to re-issue the command for any remainder).
///
/// Called by the order queue processor.
pub async fn handle_withdraw_balance_queued(
    store: &mut Store,
    player_name: &str,
    amount: Option<f64>,
) -> Result<(), StoreError> {
    info!(
        player = player_name,
        amount = ?amount,
        "Withdraw starting"
    );
    state::assert_invariants(store, "pre-withdraw-balance", false)?;

    let user_uuid = crate::mojang::resolve_user_uuid(player_name)
        .await
        .map_err(StoreError::ValidationError)?;
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

    if whole_diamonds <= 0 {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Withdraw {:.2}: amount must be at least 1 whole diamond (got {}).",
                amount, whole_diamonds
            ),
        )
        .await;
    }

    let withdraw_msg = format!(
        "Withdraw {:.2} diamonds: You'll receive {} diamonds in trade.",
        amount, whole_diamonds
    );
    utils::send_message_to_player(store, player_name, &withdraw_msg).await?;

    store.advance_trade(|s| s.begin_withdrawal(vec![]));

    // Pull diamonds from storage before the trade. Balance is NOT decremented
    // here: if the trade fails we roll diamonds back into storage, and because
    // the ledger was never touched there is nothing to restore on the balance
    // side.
    {
        let (withdraw_plan, preview_withdrawn) =
            store.storage.simulate_withdraw_plan("diamond", whole_diamonds);

        if preview_withdrawn < whole_diamonds {
            error!(
                uuid = %user_uuid,
                player = player_name,
                item = "diamond",
                need = whole_diamonds,
                available = preview_withdrawn,
                "Withdraw blocked: insufficient physical diamonds in storage"
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

        info!(
            uuid = %user_uuid,
            player = player_name,
            item = "diamond",
            amount = whole_diamonds,
            chests = withdraw_plan.len(),
            "Withdraw: decrementing storage"
        );

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
                error!(
                    uuid = %user_uuid,
                    player = player_name,
                    chest_id = t.chest_id,
                    "Withdraw: failed to send chest instruction: {}", e
                );
                return Err(StoreError::BotSendFailed(e.to_string()));
            }

            let bot_result = match tokio::time::timeout(
                tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS),
                rx,
            )
            .await
            {
                Ok(Ok(result)) => result,
                Ok(Err(e)) => {
                    error!(
                        uuid = %user_uuid,
                        player = player_name,
                        chest_id = t.chest_id,
                        "Withdraw: chest response channel dropped: {}", e
                    );
                    return Err(StoreError::BotResponseDropped(e.to_string()));
                }
                Err(_) => {
                    error!(
                        uuid = %user_uuid,
                        player = player_name,
                        chest_id = t.chest_id,
                        timeout_secs = CHEST_OP_TIMEOUT_SECS,
                        "Withdraw: timed out waiting for bot chest operation"
                    );
                    return Err(StoreError::ChestTimeout { after_ms: CHEST_OP_TIMEOUT_SECS.saturating_mul(1000) });
                }
            };

            match bot_result {
                Err(err) => {
                    error!(
                        uuid = %user_uuid,
                        player = player_name,
                        chest_id = t.chest_id,
                        "Withdraw: bot failed to withdraw diamonds from chest: {}", err
                    );
                    return utils::send_message_to_player(
                        store,
                        player_name,
                        &format!("Withdraw aborted: failed to get diamonds from storage: {}", err),
                    )
                    .await;
                }
                Ok(report) => {
                    if let Err(e) = store.apply_chest_sync(report) {
                        warn!(
                            uuid = %user_uuid,
                            player = player_name,
                            chest_id = t.chest_id,
                            "Withdraw: chest sync failed after diamond withdrawal: {}", e
                        );
                    }
                }
            }
        }
    }

    store.advance_trade(|s| s.begin_trading());

    info!(
        uuid = %user_uuid,
        player = player_name,
        amount = whole_diamonds,
        "Withdraw: initiating trade"
    );
    let (trade_tx, trade_rx) = tokio::sync::oneshot::channel();
    let trade_send_result = store.bot_tx
        .send(crate::messages::BotInstruction::TradeWithPlayer {
            target_username: player_name.to_string(),
            bot_offers: vec![crate::messages::TradeItem {
                item: "diamond".to_string(),
                amount: whole_diamonds,
            }],
            player_offers: vec![],
            require_exact_amount: false,
            flexible_validation: false,
            respond_to: trade_tx,
        })
        .await;

    if let Err(e) = trade_send_result {
        error!(
            uuid = %user_uuid,
            player = player_name,
            amount = whole_diamonds,
            "Withdraw: failed to send trade instruction, rolling diamonds back to storage: {}", e
        );
        let rb = super::super::rollback::rollback_amount_to_storage(
            store,
            "diamond",
            whole_diamonds,
            64,
            "[Withdraw] trade-send-failed",
        )
        .await;
        if rb.has_failures() {
            warn!(
                uuid = %user_uuid,
                player = player_name,
                operations_failed = rb.operations_failed,
                items_unplanned = rb.items_unplanned,
                items_returned = rb.items_returned,
                "Withdraw: rollback after trade-send failure was partial — items may remain on bot"
            );
        }
        return Err(StoreError::BotSendFailed(e.to_string()));
    }

    let trade_result = match tokio::time::timeout(
        tokio::time::Duration::from_millis(store.config.trade_timeout_ms),
        trade_rx,
    )
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => {
            warn!(
                uuid = %user_uuid,
                player = player_name,
                amount = whole_diamonds,
                "Withdraw: trade response channel dropped, rolling diamonds back to storage: {}", e
            );
            let rb = super::super::rollback::rollback_amount_to_storage(
                store,
                "diamond",
                whole_diamonds,
                64,
                "[Withdraw] channel-dropped",
            )
            .await;
            let suffix = match rb.partial_message() {
                Some(detail) => format!(" Rollback partial: {}.", detail),
                None => String::new(),
            };
            return utils::send_message_to_player(
                store,
                player_name,
                &format!("Withdraw aborted: bot response dropped: {}.{}", e, suffix),
            )
            .await;
        }
        Err(_) => {
            warn!(
                uuid = %user_uuid,
                player = player_name,
                amount = whole_diamonds,
                timeout_ms = store.config.trade_timeout_ms,
                "Withdraw: trade timed out, rolling diamonds back to storage"
            );
            let rb = super::super::rollback::rollback_amount_to_storage(
                store,
                "diamond",
                whole_diamonds,
                64,
                "[Withdraw] timeout",
            )
            .await;
            let msg = match rb.partial_message() {
                Some(detail) => format!(
                    "Withdraw aborted: bot timed out waiting for trade completion. Rollback partial: {}.",
                    detail
                ),
                None => {
                    "Withdraw aborted: bot timed out waiting for trade completion. Diamonds returned to storage."
                        .to_string()
                }
            };
            return utils::send_message_to_player(store, player_name, &msg).await;
        }
    };

    if let Err(ref err) = trade_result {
        warn!(
            uuid = %user_uuid,
            player = player_name,
            amount = whole_diamonds,
            "Withdraw: trade failed, rolling diamonds back to storage: {}", err
        );
        let rb = super::super::rollback::rollback_amount_to_storage(
            store,
            "diamond",
            whole_diamonds,
            64,
            "[Withdraw] trade-failed",
        )
        .await;
        let msg = match rb.partial_message() {
            Some(detail) => format!(
                "Withdraw aborted: trade failed: {}. Rollback partial: {}.",
                err, detail
            ),
            None => format!(
                "Withdraw aborted: trade failed: {}. Diamonds returned to storage.",
                err
            ),
        };
        return utils::send_message_to_player(store, player_name, &msg).await;
    }

    // Trade succeeded: decrement the ledger balance now that the diamonds are
    // in the player's hands.
    {
        let user = store.expect_user_mut(&user_uuid, "withdraw-balance/commit")?;
        user.balance -= amount;
        user.username = player_name.to_owned();
    }
    store.dirty = true;
    store.dirty_users.insert(user_uuid.clone());
    info!(
        uuid = %user_uuid,
        player = player_name,
        item = "diamond",
        amount = amount,
        "Withdraw: decremented user balance"
    );

    store.orders.push_back(crate::types::Order {
        order_type: crate::types::order::OrderType::WithdrawBalance,
        item: ItemId::from_normalized("diamond".to_string()),
        amount: whole_diamonds,
        currency_amount: 0.0,
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

    info!(
        uuid = %user_uuid,
        player = player_name,
        amount = amount,
        whole_diamonds = whole_diamonds,
        "Withdraw completed"
    );

    // With `repair=true`, `assert_invariants` returns Ok once any fixable
    // drift has been repaired; an Err here means the audit found something it
    // could not reconcile and operator attention is required.
    if let Err(e) = state::assert_invariants(store, "post-withdraw-balance", true) {
        error!(
            uuid = %user_uuid,
            player = player_name,
            "Unrecoverable invariant violation after withdraw balance: {}", e
        );
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
