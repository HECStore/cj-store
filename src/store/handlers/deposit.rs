//! `deposit` / `d` command: enqueue handler + queued-order processor.

use tracing::{debug, error, info, warn};

use super::super::{Store, state, utils};
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
        "Queueing deposit order"
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

/// Processes a dequeued deposit order: runs the trade, credits the user's
/// balance, and moves the received diamonds into storage.
///
/// If `amount` is `Some`, the player must offer exactly that many diamonds
/// (ceiled to whole diamonds). If `amount` is `None`, the deposit is flexible
/// and credits whatever the player puts in, up to 12 stacks (768 diamonds).
pub async fn handle_deposit_balance_queued(
    store: &mut Store,
    player_name: &str,
    amount: Option<f64>,
) -> Result<(), StoreError> {
    info!(
        player = player_name,
        amount = ?amount,
        "Deposit starting"
    );
    state::assert_invariants(store, "pre-deposit-balance", false)?;

    // Minecraft's vanilla trade UI exposes 12 offer slots (4x3 grid); each
    // slot holds at most one 64-stack of diamonds, so a single trade can move
    // at most 768 diamonds. We reject larger requests at the handler rather
    // than silently truncating so the player isn't surprised by a partial
    // transaction.
    const MAX_TRADE_DIAMONDS: i32 = 12 * 64; // 768

    let (diamonds_to_trade, is_flexible) = match amount {
        Some(amt) => {
            if !amt.is_finite() || amt <= 0.0 {
                debug!(
                    player = player_name,
                    amount = amt,
                    "Deposit rejected: non-positive or non-finite amount"
                );
                return utils::send_message_to_player(
                    store,
                    player_name,
                    "Amount must be positive",
                )
                .await;
            }
            let diamonds = amt.ceil() as i32;
            if diamonds > MAX_TRADE_DIAMONDS {
                debug!(
                    player = player_name,
                    amount = amt,
                    max = MAX_TRADE_DIAMONDS,
                    "Deposit rejected: amount exceeds single-trade capacity"
                );
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

    let user_uuid = crate::mojang::resolve_user_uuid(player_name)
        .await
        .map_err(StoreError::ValidationError)?;
    utils::ensure_user_exists(store, player_name, &user_uuid);

    let msg = if is_flexible {
        format!(
            "Deposit: Please offer diamonds in the trade (up to {} diamonds / 12 stacks). You'll be credited for the actual amount.",
            MAX_TRADE_DIAMONDS
        )
    } else {
        // unwrap is sound: `is_flexible` is false iff `amount` is `Some`.
        format!(
            "Deposit {:.2} diamonds: Please offer {} diamonds in the trade.",
            amount.unwrap(),
            diamonds_to_trade
        )
    };
    utils::send_message_to_player(store, player_name, &msg).await?;

    // Empty withdrawal plan: a deposit has nothing to pull from storage first,
    // but the trade state machine still requires the Queued -> Withdrawing ->
    // Trading progression.
    store.advance_trade(|s| s.begin_withdrawal(vec![]));
    store.advance_trade(|s| s.begin_trading());

    info!(
        player = player_name,
        uuid = %user_uuid,
        diamonds = diamonds_to_trade,
        flexible = is_flexible,
        "Deposit initiating trade"
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
        error!(
            player = player_name,
            uuid = %user_uuid,
            error = %e,
            "Deposit failed to send trade instruction"
        );
        return Err(StoreError::BotSendFailed(format!(
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
            error!(
                player = player_name,
                uuid = %user_uuid,
                error = %e,
                "Deposit trade channel dropped"
            );
            return Err(StoreError::BotResponseDropped(format!("Bot response dropped: {}", e)));
        }
        Err(_) => {
            error!(
                player = player_name,
                uuid = %user_uuid,
                timeout_ms = store.config.trade_timeout_ms,
                "Deposit trade timed out"
            );
            return Err(StoreError::TradeTimeout { after_ms: store.config.trade_timeout_ms });
        }
    };

    let actual_received = match trade_result {
        Err(err) => {
            warn!(
                player = player_name,
                uuid = %user_uuid,
                error = %err,
                "Deposit trade failed"
            );
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
        warn!(
            player = player_name,
            uuid = %user_uuid,
            "Deposit aborted: trade completed but zero diamonds received"
        );
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
        warn!(
            player = player_name,
            uuid = %user_uuid,
            failed_steps = rb.operations_failed,
            diamonds = diamonds_actually_received,
            "Deposit storage write partially failed; some diamonds may remain in bot inventory"
        );
    } else {
        info!(
            uuid = %user_uuid,
            item = "diamond",
            amount = diamonds_actually_received,
            "Deposit storage credited"
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
    store.dirty_users.insert(user_uuid.clone());

    info!(
        uuid = %user_uuid,
        item = "diamond",
        amount = actual_amount,
        new_balance = new_balance,
        "Deposit balance credited"
    );

    store.orders.push_back(crate::types::Order {
        order_type: crate::types::order::OrderType::DepositBalance,
        item: ItemId::from_normalized("diamond".to_string()),
        amount: diamonds_actually_received,
        currency_amount: 0.0,
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
        player = player_name,
        uuid = %user_uuid,
        amount = actual_amount,
        new_balance = new_balance,
        "Deposit completed"
    );

    if let Err(e) = state::assert_invariants(store, "post-deposit-balance", true) {
        error!(
            uuid = %user_uuid,
            error = %e,
            "Invariant violation after deposit balance"
        );
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
