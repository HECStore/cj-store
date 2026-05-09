//! Operator command handlers

use tracing::{error, info, warn};

use crate::constants::{CHEST_OP_TIMEOUT_SECS, CHESTS_PER_NODE};
use crate::error::StoreError;
use crate::messages::TradeItem;
use crate::types::{ItemId, Order, Trade, TradeType};
use super::super::{Store, state, utils};

/// Resolve `player_name` to a Mojang UUID for an operator command and, on
/// failure, whisper a sanitized notice to the operator IN-PLACE before
/// surfacing the error.
///
/// The four operator handlers used to `?`-propagate the resolver error up to
/// `handle_bot_message`, which only `error!`-logs it — so a Mojang glitch
/// (timeout, upstream blip, malformed response) left the operator with no
/// feedback. Mirrors `player.rs`'s call site: log + whisper-via-
/// `whisper_error_to_player` + return `Err(...)`.
///
/// `Ok(Some(uuid))` on success, `Ok(None)` when the resolver error has
/// already been whispered (caller should `return Ok(())` from the handler),
/// `Err(...)` only if the whisper itself failed.
async fn resolve_operator_uuid(
    store: &mut Store,
    player_name: &str,
    verb: &str,
) -> Result<Option<String>, StoreError> {
    match crate::mojang::resolve_user_uuid(player_name).await {
        Ok(uuid) => Ok(Some(uuid)),
        Err(reason) => {
            warn!(
                player = player_name,
                command = verb,
                reason = %reason,
                "Mojang UUID lookup failed for operator command; whispering player-facing notice"
            );
            let err: StoreError = reason.into();
            utils::whisper_error_to_player(store, player_name, &err).await?;
            Ok(None)
        }
    }
}

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
    let user_uuid = match resolve_operator_uuid(store, player_name, "additem").await? {
        Some(uuid) => uuid,
        None => return Ok(()),
    };
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
    let (preview_deposit_plan, preview_planned) =
        store.storage.simulate_deposit_plan(item, qty_i32, stack_size);
    // The deposit planner is bounded by current storage capacity; if it can't
    // place every requested item, accepting the trade anyway would orphan the
    // overflow in the bot's inventory and falsify the audit row that records
    // `qty_i32` as added stock. Reject up front, mirroring the symmetric guard
    // in handle_removeitem_order.
    if preview_planned != qty_i32 {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Insufficient storage capacity for '{}': can only place {} of {} items. \
                 Operator should add a storage node.",
                item, preview_planned, qty_i32
            ),
        )
        .await;
    }

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
            return utils::whisper_action_aborted(store, player_name, "Additem", &err, None).await;
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
            // Sanitize for operator whisper: the raw mpsc SendError leaks
            // transport detail. The operator-facing whisper splices
            // `failed_reason` verbatim, so wrap through `user_message()`.
            failed_reason = StoreError::BotSendFailed(e.to_string()).user_message().into_owned();
            break;
        }

        let bot_result = tokio::time::timeout(tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS), rx)
            .await
            .map_err(|_| StoreError::ChestTimeout { after_ms: CHEST_OP_TIMEOUT_SECS * 1000 })
            .and_then(|r| r.map_err(|e| StoreError::BotResponseDropped(e.to_string())));

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
                // Sanitize for operator whisper: bot-reported error text may
                // include transport-layer detail. Wrap through `user_message()`.
                failed_reason = StoreError::BotReportedError(err).user_message().into_owned();
                break;
            }
            Err(err) => {
                error!("[Additem] deposit step {} error: item={} chest_id={} chunk_amount={} err={}",
                    step + 1, item, t.chest_id, t.amount, err);
                deposit_failed = true;
                failed_reason = err.user_message().into_owned();
                break;
            }
        }
    }
    
    // The operator already parted with their items in the trade above, so any
    // undeposited remainder is sitting in the bot's inventory. Return it via a reverse
    // trade so the operator is made whole. If that reverse trade also fails, the items
    // are genuinely stuck and need manual admin recovery (logged at error level).
    //
    // Every failure branch below funnels into `final_status` / `final_result` instead of
    // returning early — the tail block after this `if` MUST always run so that
    // `pair.item_stock` is resynced from physical storage and the post-additem invariant
    // check fires in repair mode. Returning early here would leave cached `pair.item_stock`
    // under-reporting physical inventory by `items_deposited`, poisoning the next handler's
    // `pre-*` checkpoint and the buy-handler stock gate.
    let mut final_status: Option<String> = None;
    let mut final_result: Result<(), StoreError> = Ok(());
    let mut record_audit = true;

    if deposit_failed {
        record_audit = false;
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
                final_result = Err(StoreError::BotSendFailed(format!(
                    "return-to-operator trade instruction after deposit failure ({} {} stuck in bot): {}",
                    items_in_bot_inventory, item, e
                )));
            } else {
                match tokio::time::timeout(tokio::time::Duration::from_millis(store.config.trade_timeout_ms), rb_rx).await {
                    Ok(Ok(Ok(_))) => {
                        info!(
                            "[Additem] returned items to operator: operator={} item={} returned={} deposited={}",
                            player_name, item, items_in_bot_inventory, items_deposited
                        );
                        final_status = Some(format!(
                            "Additem failed: {}. {} items were returned to you. {} items were stored successfully.",
                            failed_reason, items_in_bot_inventory, items_deposited
                        ));
                    }
                    Ok(Ok(Err(err))) => {
                        error!(
                            "[Additem] CRITICAL: return-to-operator trade rejected by bot (structured failure) — \
                            {} item(s) of '{}' stuck in bot inventory; operator={} deposited={} bot_err={}",
                            items_in_bot_inventory, item, player_name, items_deposited, err
                        );
                        final_status = Some(format!(
                            "Additem CRITICAL ERROR: {}. {} items stuck in bot inventory (bot rejected return trade: {}). Contact administrator. {} items were stored.",
                            failed_reason, items_in_bot_inventory, err, items_deposited
                        ));
                    }
                    Ok(Err(e)) => {
                        error!(
                            "[Additem] CRITICAL: return-to-operator oneshot dropped before reply (bot dropped response sender, likely crashed) — \
                            {} item(s) of '{}' stuck in bot inventory; operator={} deposited={} err={}",
                            items_in_bot_inventory, item, player_name, items_deposited, e
                        );
                        final_status = Some(format!(
                            "Additem CRITICAL ERROR: {}. {} items stuck in bot inventory (bot dropped response). Contact administrator. {} items were stored.",
                            failed_reason, items_in_bot_inventory, items_deposited
                        ));
                    }
                    Err(_) => {
                        error!(
                            "[Additem] CRITICAL: return-to-operator trade timed out after {}ms — \
                            {} item(s) of '{}' stuck in bot inventory; operator={} deposited={}",
                            store.config.trade_timeout_ms, items_in_bot_inventory, item, player_name, items_deposited
                        );
                        final_status = Some(format!(
                            "Additem CRITICAL ERROR: {}. {} items stuck in bot inventory (return trade timed out). Contact administrator. {} items were stored.",
                            failed_reason, items_in_bot_inventory, items_deposited
                        ));
                    }
                }
            }
        } else {
            final_status = Some(format!("Additem failed: {}", failed_reason));
        }
    }

    // Resync pair stock from authoritative storage totals (chest syncs above may have
    // moved the real amount by more than `items_deposited` if there was drift).
    //
    // This block runs unconditionally — even on partial-failure paths — so cached
    // `pair.item_stock` always reflects what physical storage actually holds after the
    // deposit loop. Skipping this on failure would under-report inventory by
    // `items_deposited` and trip the next handler's `pre-*` invariant checkpoint.
    let new_stock = store.storage.total_item_amount(item);
    let pair = store.expect_pair_mut(item, "additem/commit")?;
    pair.item_stock = new_stock;
    assert!(pair.item_stock >= 0,
        "[Additem] INVARIANT VIOLATED: item_stock went negative after add_stock \
        (item={}, stock={}). This indicates a storage accounting bug.",
        item, pair.item_stock);
    store.dirty = true;

    if record_audit {
        store.trades.push(Trade::new(
            TradeType::AddStock,
            ItemId::from_normalized(item.to_string()),
            qty_i32,
            0.0,
            user_uuid.clone(),
        ));

        store.orders.push_back(Order::add_item(
            ItemId::from_normalized(item.to_string()),
            qty_i32,
            user_uuid.clone(),
        ));

        info!(
            "[Additem] committed: operator={} uuid={} item={} qty={} stock_before={} stock_after={}",
            player_name, user_uuid, item, quantity, stock_before, new_stock
        );
    }

    // Repair-mode invariant check fires regardless of success/failure so any drift
    // detected post-deposit gets logged + saved rather than poisoning later handlers.
    if let Err(e) = state::assert_invariants(store, "post-additem", true) {
        error!("[Additem] invariant violation after commit: operator={} item={} err={}",
            player_name, item, e);
        let _ = state::save(store);
    }

    let whisper_text = final_status
        .unwrap_or_else(|| format!("Added {} {} to storage. New stock: {}", quantity, item, new_stock));
    let whisper_result = utils::send_message_to_player(store, player_name, &whisper_text).await;

    // Failure-path Err propagation (e.g. BotSendFailed) takes precedence over a successful
    // whisper; on the success path `final_result` is Ok and we return the whisper outcome.
    match final_result {
        Err(e) => Err(e),
        Ok(()) => whisper_result,
    }
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
    let user_uuid = match resolve_operator_uuid(store, player_name, "removeitem").await? {
        Some(uuid) => uuid,
        None => return Ok(()),
    };
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

    let stack_size = store.pairs.get(item).map(|p| p.stack_size).unwrap_or(64);
    if let Err(e) = super::super::orders::execute_chest_transfers(
        store,
        &preview_withdraw_plan,
        item,
        stack_size,
        super::super::orders::ChestDirection::Withdraw,
        "[Removeitem]",
    )
    .await
    {
        error!("[Removeitem] chest withdraw failed: operator={} item={} err={}",
            player_name, item, e);
        utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Removeitem aborted: bot failed chest withdrawal step: {}",
                e.user_message()
            ),
        )
        .await?;
        return Err(e);
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

    // Every failure branch below funnels into `final_status` / `final_result` instead of
    // returning early — the tail block after this `match` MUST always run so that
    // `pair.item_stock` is resynced from physical storage and the post-removeitem invariant
    // check fires in repair mode. Returning early on a partial-rollback path would leave
    // cached `pair.item_stock` at the pre-removeitem value while physical storage is mid-
    // rollback, poisoning the next handler's `pre-*` checkpoint and short-circuiting an
    // unrelated legitimate operation.
    let mut final_status: Option<String> = None;
    let mut final_result: Result<(), StoreError> = Ok(());
    let mut record_audit = true;

    match trade_outcome {
        Ok(_) => {}
        Err(StoreError::BotDisconnected) => {
            record_audit = false;
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
            final_result = Err(StoreError::BotDisconnected);
        }
        Err(StoreError::TradeRejected(err)) => {
            record_audit = false;
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
                final_status = Some(format!(
                    "Removeitem CRITICAL ERROR: trade failed and rollback also partially failed. \
                    {} item(s) may be stuck in bot inventory. Contact administrator. trade_error='{}'",
                    qty_i32 - rb.items_returned, err
                ));
            } else {
                final_status = Some(utils::format_action_aborted(
                    "Removeitem",
                    &err,
                    Some(" Items returned to storage."),
                ));
            }
        }
        Err(other) => return Err(other),
    }

    // Resync pair stock from authoritative storage totals (chest syncs above may have
    // moved the real amount by more than the planned withdrawal if there was drift, and
    // a partially-failed rollback leaves physical storage somewhere between pre- and
    // post-removeitem).
    //
    // This block runs unconditionally — even on partial-rollback paths — so cached
    // `pair.item_stock` always reflects what physical storage actually holds. Skipping
    // this on failure would leave the cache at the pre-removeitem value while storage
    // is mid-rollback, tripping the next handler's `pre-*` invariant checkpoint.
    let new_stock = store.storage.total_item_amount(item);
    let pair = store.expect_pair_mut(item, "removeitem/commit")?;
    pair.item_stock = new_stock;
    assert!(pair.item_stock >= 0,
        "[Removeitem] INVARIANT VIOLATED: item_stock went negative after remove_stock \
        (item={}, stock={}). This indicates a storage accounting bug.",
        item, pair.item_stock);
    store.dirty = true;

    if record_audit {
        store.trades.push(Trade::new(
            TradeType::RemoveStock,
            ItemId::from_normalized(item.to_string()),
            qty_i32,
            0.0,
            user_uuid.clone(),
        ));

        store.orders.push_back(Order::remove_item(
            ItemId::from_normalized(item.to_string()),
            qty_i32,
            user_uuid.clone(),
        ));

        info!(
            "[Removeitem] committed: operator={} uuid={} item={} qty={} stock_before={} stock_after={}",
            player_name, user_uuid, item, quantity, stock_before, new_stock
        );
    }

    // Repair-mode invariant check fires regardless of success/failure so any drift
    // detected post-rollback gets logged + saved rather than poisoning later handlers.
    if let Err(e) = state::assert_invariants(store, "post-removeitem", true) {
        error!("[Removeitem] invariant violation after commit: operator={} item={} err={}",
            player_name, item, e);
        let _ = state::save(store);
    }

    let whisper_text = final_status
        .unwrap_or_else(|| format!("Removed {} {} from storage. Remaining stock: {}", quantity, item, new_stock));
    let whisper_result = utils::send_message_to_player(store, player_name, &whisper_text).await;

    // Failure-path Err propagation (e.g. BotDisconnected) takes precedence over a successful
    // whisper; on the success path `final_result` is Ok and we return the whisper outcome.
    match final_result {
        Err(e) => Err(e),
        Ok(()) => whisper_result,
    }
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
    let user_uuid = match resolve_operator_uuid(store, player_name, "addcurrency").await? {
        Some(uuid) => uuid,
        None => return Ok(()),
    };

    // Validate inputs BEFORE recording the operator as a user. A rejected
    // request must leave the store untouched (dirty unchanged); creating
    // the user row first would dirty the store on every malformed
    // operator command.
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

    utils::ensure_user_exists(store, player_name, &user_uuid);

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

    store.orders.push_back(Order::add_currency(
        ItemId::from_normalized(item.to_string()),
        amount,
        user_uuid.clone(),
    ));

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
    let user_uuid = match resolve_operator_uuid(store, player_name, "removecurrency").await? {
        Some(uuid) => uuid,
        None => return Ok(()),
    };

    // Validate inputs BEFORE recording the operator as a user. A rejected
    // request must leave the store untouched (dirty unchanged); creating
    // the user row first would dirty the store on every malformed
    // operator command.
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

    utils::ensure_user_exists(store, player_name, &user_uuid);

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

    store.orders.push_back(Order::remove_currency(
        ItemId::from_normalized(item.to_string()),
        amount,
        user_uuid.clone(),
    ));

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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
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

    /// Build a `MockChestState` map matching what `make_storage(_, stock)`
    /// produces — chest id 2, slot 0 holds `stock`, all other slots are zero.
    /// Use with `spawn_mock_bot_with_state` so the mock returns truthful
    /// `prior + delta` results for tests that assert post-op `pair.item_stock`.
    fn mock_state_seeded_like_storage(stock: i32) -> MockChestState {
        let mut slots = vec![0; crate::constants::DOUBLE_CHEST_SLOTS];
        slots[0] = stock;
        let mut map = HashMap::new();
        map.insert(2, slots);
        Arc::new(std::sync::Mutex::new(map))
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
    fn spawn_mock_bot(rx: mpsc::Receiver<BotInstruction>) {
        spawn_mock_bot_with_fail_flag(rx, None);
    }

    /// Per-chest amounts the mock should track as authoritative prior state.
    ///
    /// Tests that exercise post-chest-op invariants (`pair.item_stock` resync,
    /// rollback accounting) need the mock to compute correct
    /// `prior + delta` results. The instruction's `target_chest.amounts` is
    /// always zero-filled by `chest_from_transfer` (see its doc comment — the
    /// real bot reads the world; the synthesized chest is purely a routing
    /// payload), so the mock can't derive prior state from there. A test can
    /// pre-seed this map with the chest's actual slot amounts and the mock
    /// will read/update it on every interaction.
    type MockChestState = Arc<std::sync::Mutex<HashMap<i32, Vec<i32>>>>;

    /// Apply a chest action to the optional shared chest state and return the
    /// `ChestSyncReport` payload. When `chest_state` is `Some`, the prior slot
    /// amounts come from the map and are updated in place. When `None`, the
    /// legacy behavior fires (prior derived from the always-zero
    /// `target_chest.amounts`, suitable for tests that don't assert
    /// post-state).
    fn mock_apply_chest_action(
        target_chest: &Chest,
        action: crate::messages::ChestAction,
        chest_state: Option<&MockChestState>,
    ) -> ChestSyncReport {
        let (item, delta) = match action {
            crate::messages::ChestAction::Withdraw { item, amount, .. } => (item, -amount),
            crate::messages::ChestAction::Deposit { item, amount, .. } => (item, amount),
        };
        let mut amounts = [-1i32; crate::constants::DOUBLE_CHEST_SLOTS];
        if let Some(state) = chest_state {
            let mut map = state.lock().expect("mock chest state mutex poisoned");
            let slots = map
                .entry(target_chest.id)
                .or_insert_with(|| vec![0; crate::constants::DOUBLE_CHEST_SLOTS]);
            slots[0] = (slots[0] + delta).max(0);
            // Mirror the full known state back so apply_chest_sync overwrites
            // every slot — sentinels in untouched slots would leave stale
            // values that diverge from the mock's tracked truth.
            for (i, v) in slots.iter().enumerate().take(amounts.len()) {
                amounts[i] = *v;
            }
        } else {
            let prior = target_chest.amounts.first().copied().unwrap_or(0);
            amounts[0] = (prior + delta).max(0);
        }
        ChestSyncReport {
            chest_id: target_chest.id,
            item,
            amounts,
        }
    }

    /// Variant of `spawn_mock_bot` with an optional one-shot fail-injection toggle:
    /// when `fail_next_chest_op` is `Some(flag)` and the flag is `true` at the moment
    /// a chest interaction arrives, the mock atomically clears the flag and replies
    /// with `Err`, simulating a bot-side failure mid-deposit. All other instructions
    /// (whisper, trade, subsequent chest ops) are answered with synthetic success so
    /// the handler can exercise its rollback path (e.g. the return-to-operator
    /// `TradeWithPlayer` after an additem deposit-step failure) without blocking on
    /// the channel.
    fn spawn_mock_bot_with_fail_flag(
        rx: mpsc::Receiver<BotInstruction>,
        fail_next_chest_op: Option<Arc<AtomicBool>>,
    ) {
        spawn_mock_bot_full(rx, None, fail_next_chest_op, None);
    }

    /// Variant that tracks per-chest state (so post-op invariants are checkable)
    /// and optionally arms the trade arm with a one-shot fail flag — used by
    /// the trade-rejection rollback test, which needs both behaviours.
    /// (Standalone trade-fail-flag spawner exists in git history if a future
    /// test wants the chest-stateless variant; trivial to re-add via
    /// `spawn_mock_bot_full(rx, None, None, Some(flag))`.)
    fn spawn_mock_bot_with_state(
        rx: mpsc::Receiver<BotInstruction>,
        chest_state: MockChestState,
        fail_next_trade: Option<Arc<AtomicBool>>,
    ) {
        spawn_mock_bot_full(rx, Some(chest_state), None, fail_next_trade);
    }

    /// All-knobs spawner. Public callers go through the convenience wrappers
    /// above; this is the single place the message-loop logic lives.
    fn spawn_mock_bot_full(
        mut rx: mpsc::Receiver<BotInstruction>,
        chest_state: Option<MockChestState>,
        fail_next_chest_op: Option<Arc<AtomicBool>>,
        fail_next_trade: Option<Arc<AtomicBool>>,
    ) {
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
                        // If the caller armed the fail flag, consume it and reply Err.
                        // `swap` ensures only the FIRST chest op after arming fails;
                        // subsequent ones (e.g. those issued during recovery) succeed.
                        if let Some(flag) = fail_next_chest_op.as_ref()
                            && flag.swap(false, Ordering::SeqCst)
                        {
                            let _ = respond_to
                                .send(Err("simulated bot chest failure".to_string()));
                            continue;
                        }
                        let report = mock_apply_chest_action(
                            &target_chest,
                            action,
                            chest_state.as_ref(),
                        );
                        let _ = respond_to.send(Ok(report));
                    }
                    BotInstruction::TradeWithPlayer {
                        bot_offers: _,
                        player_offers,
                        respond_to,
                        ..
                    } => {
                        if let Some(flag) = fail_next_trade.as_ref()
                            && flag.swap(false, Ordering::SeqCst)
                        {
                            let _ = respond_to
                                .send(Err("simulated trade rejection".to_string()));
                        } else {
                            let _ = respond_to.send(Ok(player_offers));
                        }
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

        let add_result = handle_add_currency(&mut store, "Ops", item, 250.0).await;
        assert!(add_result.is_ok(), "handle_add_currency failed: {:?}", add_result);

        let remove_result = handle_remove_currency(&mut store, "Ops", item, 250.0).await;
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

    // ======================================================================
    // handle_additem_order tests
    // ----------------------------------------------------------------------
    // `handle_additem_order` is a 300-line stateful chest-I/O handler with five
    // distinct failure branches and two load-bearing invariants:
    //
    //   1. "Audit-skip-on-failure" — `Trade { AddStock }` and `Order { AddItem }`
    //      rows are appended ONLY when the deposit loop completed end-to-end
    //      (i.e. `record_audit == true`). A regression that audited on partial
    //      failure would write phantom stock into the journal.
    //
    //   2. "Unconditional resync" — the tail block that writes
    //      `pair.item_stock = storage.total_item_amount(item)` runs on every
    //      path, including failure. A regression that returned early on deposit
    //      failure would leave cached stock under-reporting physical inventory
    //      and trip the next handler's `pre-*` invariant checkpoint.
    //
    // The three tests below pin happy path, pre-trade-rejection (the just-added
    // capacity guard), and the deposit-step-failure rollback path that exercises
    // both invariants together.
    // ======================================================================

    #[tokio::test]
    async fn additem_happy_path_deposits_and_appends_audit() {
        // Seed a single-node storage with 50 cobblestone in chest 2 (non-reserved)
        // and a matching pair stock so `pre-additem` invariants pass. Add 100 more;
        // shulker capacity per slot is 27*64=1728, so all 100 land in chest 2 slot 0
        // and the mock bot's deposit reply syncs that slot to 150.
        let item = "cobblestone";
        let (tx, rx) = mpsc::channel(64);
        let chest_state = mock_state_seeded_like_storage(50);
        spawn_mock_bot_with_state(rx, chest_state, None);

        let mut pairs = HashMap::new();
        let (k, p) = make_pair(item, 50, 0.0);
        pairs.insert(k, p);

        let storage = make_storage(item, 50);
        let mut store = Store::new_for_test(tx, test_config(), pairs, HashMap::new(), storage);

        let trades_before = store.trades.len();
        let orders_before = store.orders.len();

        let result = handle_additem_order(&mut store, "Alice", item, 100).await;
        assert!(result.is_ok(), "handle_additem_order failed: {:?}", result);

        // Pair stock matches physical storage post-deposit.
        let physical_after = store.storage.total_item_amount(item);
        let pair_after = store.pairs.get(item).expect("pair must exist").item_stock;
        assert_eq!(
            pair_after, physical_after,
            "pair.item_stock must equal storage.total_item_amount after deposit"
        );
        assert_eq!(
            pair_after, 150,
            "pair.item_stock expected 150 (50 + 100), got {}",
            pair_after
        );

        // Exactly one new Trade audit row of type AddStock.
        assert_eq!(
            store.trades.len(),
            trades_before + 1,
            "expected exactly one new trade audit row"
        );
        let trade: &Trade = &store.trades[trades_before];
        assert!(
            matches!(trade.trade_type, TradeType::AddStock),
            "expected AddStock trade type, got {:?}",
            trade.trade_type
        );
        assert_eq!(trade.amount, 100);
        assert!(
            trade.amount_currency.abs() < 1e-9,
            "trade.amount_currency expected 0.0, got {}",
            trade.amount_currency
        );
        assert_eq!(trade.item.as_str(), item);
        assert_eq!(trade.user_uuid, test_uuid("Alice"));

        // Exactly one new Order audit row of type AddItem.
        assert_eq!(
            store.orders.len(),
            orders_before + 1,
            "expected exactly one new order audit row"
        );
        let order: &Order = &store.orders[orders_before];
        assert!(
            matches!(order.order_type, OrderType::AddItem),
            "expected AddItem order type, got {:?}",
            order.order_type
        );
        assert_eq!(order.amount, 100);
        assert_eq!(order.item.as_str(), item);

        assert!(store.dirty, "store.dirty must be true after mutation");
    }

    #[tokio::test]
    async fn additem_insufficient_capacity_rejects_before_trade() {
        // Pin the just-added pre-trade guard mirroring `handle_removeitem_order`.
        // With an empty Storage (zero nodes) `simulate_deposit_plan` cannot place
        // any items, so `preview_planned == 0 != qty_i32 (10)` and the handler
        // must whisper the operator and abort BEFORE initiating a trade. No
        // audit rows must be written and `pair.item_stock` must be untouched.
        let item = "cobblestone";
        let (tx, rx) = mpsc::channel(64);
        spawn_mock_bot(rx);

        let mut pairs = HashMap::new();
        // Pair item_stock=0 to match empty physical storage (invariant gate).
        let (k, p) = make_pair(item, 0, 0.0);
        pairs.insert(k, p);

        // Empty Storage — no nodes at all, so deposit planner has nowhere to put
        // anything. This is the simplest way to trip the capacity guard without
        // needing to fully fill 16+ chests of node-0 with shulker-stacks.
        let storage = Storage::new(&Position { x: 0, y: 64, z: 0 });
        let mut store = Store::new_for_test(tx, test_config(), pairs, HashMap::new(), storage);

        let trades_before = store.trades.len();
        let orders_before = store.orders.len();
        let dirty_before = store.dirty;
        let stock_before = store.pairs.get(item).expect("pair must exist").item_stock;

        let result = handle_additem_order(&mut store, "Alice", item, 10).await;
        assert!(
            result.is_ok(),
            "handler should whisper, not propagate error: {:?}",
            result
        );

        // No trade was initiated, so no audit rows.
        assert_eq!(
            store.trades.len(),
            trades_before,
            "no trade audit on capacity rejection"
        );
        assert_eq!(
            store.orders.len(),
            orders_before,
            "no order audit on capacity rejection"
        );
        assert_eq!(
            store.dirty, dirty_before,
            "store.dirty must remain unchanged on pre-trade rejection"
        );
        assert_eq!(
            store.pairs.get(item).expect("pair must exist").item_stock,
            stock_before,
            "pair.item_stock must remain unchanged on capacity rejection"
        );
    }

    #[tokio::test]
    async fn additem_deposit_failure_skips_audit_but_resyncs_stock() {
        // Pins both load-bearing invariants of `handle_additem_order`:
        //
        //   * "Audit-skip-on-failure" — when the deposit loop bails out via
        //     `record_audit = false`, NO `Trade::AddStock` and NO `Order::AddItem`
        //     rows are appended.
        //
        //   * "Unconditional resync" — the tail block that rewrites
        //     `pair.item_stock = storage.total_item_amount(item)` MUST run on
        //     the failure path. The mock bot sends a "simulated chest failure"
        //     for the FIRST `InteractWithChestAndSync` arriving after the trade
        //     succeeds; `apply_chest_sync` therefore never runs for that step
        //     so physical storage is untouched and the resync should leave
        //     `pair.item_stock` at its pre-additem value (50). `store.dirty`
        //     must also be `true` because the tail-block resync sets it.
        let item = "cobblestone";
        let (tx, rx) = mpsc::channel(64);
        let fail_flag = Arc::new(AtomicBool::new(true));
        spawn_mock_bot_with_fail_flag(rx, Some(Arc::clone(&fail_flag)));

        let mut pairs = HashMap::new();
        let (k, p) = make_pair(item, 50, 0.0);
        pairs.insert(k, p);

        let storage = make_storage(item, 50);
        let mut store = Store::new_for_test(tx, test_config(), pairs, HashMap::new(), storage);

        let trades_before = store.trades.len();
        let orders_before = store.orders.len();
        let physical_before = store.storage.total_item_amount(item);

        let result = handle_additem_order(&mut store, "Alice", item, 100).await;
        // Handler returns Ok because the failure is reported via whisper, not Err.
        // (The only Err propagation paths are validation/StoreError::BotSendFailed;
        // a bot-reported chest error funnels into `final_status` whisper text.)
        assert!(
            result.is_ok(),
            "handler should whisper failure, not propagate error: {:?}",
            result
        );

        // Audit-skip invariant: `record_audit == false` ⇒ no Trade/Order rows.
        assert_eq!(
            store.trades.len(),
            trades_before,
            "no trade audit row on deposit-step failure (record_audit=false)"
        );
        assert_eq!(
            store.orders.len(),
            orders_before,
            "no order audit row on deposit-step failure (record_audit=false)"
        );

        // Unconditional-resync invariant: tail block runs even on failure.
        // The mock failed BEFORE applying the chest sync, so physical storage
        // is still 50 and the resynced pair stock must equal that.
        let physical_after = store.storage.total_item_amount(item);
        assert_eq!(
            physical_after, physical_before,
            "physical storage should be unchanged when chest op failed pre-sync"
        );
        let pair_after = store.pairs.get(item).expect("pair must exist").item_stock;
        assert_eq!(
            pair_after, physical_after,
            "pair.item_stock must equal storage.total_item_amount even on failure path"
        );
        // `store.dirty` is set by the unconditional tail block (and by the
        // return-to-operator trade's bot interactions); pin that the dirty flag
        // is true so the persistence layer would flush the (unchanged but
        // re-affirmed) state on the next tick.
        assert!(
            store.dirty,
            "store.dirty must be true after handler runs (unconditional resync sets it)"
        );

        // The fail flag must have been consumed (swap cleared it), proving the
        // mock injected the failure on the first chest op rather than later.
        assert!(
            !fail_flag.load(Ordering::SeqCst),
            "fail flag should have been consumed by the first chest op"
        );
    }

    // ======================================================================
    // handle_removeitem_order tests
    // ----------------------------------------------------------------------
    // `handle_removeitem_order` mirrors `handle_additem_order` in reverse — a
    // chest-withdraw loop followed by a return-to-operator trade — and shares
    // the same two load-bearing invariants:
    //
    //   1. "Audit-skip-on-failure" — `Trade { RemoveStock }` and
    //      `Order { RemoveItem }` rows are appended ONLY when the trade
    //      completed end-to-end (`record_audit == true`). On a TradeRejected
    //      branch the rollback re-deposits and we MUST NOT audit a non-trade.
    //
    //   2. "Unconditional resync" — the tail block that writes
    //      `pair.item_stock = storage.total_item_amount(item)` runs on every
    //      path, including the rollback path, so cached pair stock always
    //      reflects post-rollback physical storage.
    //
    // The two tests below pin the happy path and the TradeRejected rollback
    // path (which logs CRITICAL on partial-rollback failure — exactly the
    // silent-desync regression a unit test pins).
    // ======================================================================

    #[tokio::test]
    async fn removeitem_happy_path_withdraws_and_appends_audit() {
        // Seed a single-node storage with 50 cobblestone in chest 2 and a
        // matching pair stock so `pre-removeitem` invariants pass. Remove 20;
        // the planner emits one withdrawal from slot 0 (50 → 30), the mock
        // bot syncs the chest to 30, and `perform_trade` auto-accepts the
        // operator-targeted trade.
        let item = "cobblestone";
        let (tx, rx) = mpsc::channel(64);
        let chest_state = mock_state_seeded_like_storage(50);
        spawn_mock_bot_with_state(rx, chest_state, None);

        let mut pairs = HashMap::new();
        let (k, p) = make_pair(item, 50, 0.0);
        pairs.insert(k, p);

        let storage = make_storage(item, 50);
        let mut store = Store::new_for_test(tx, test_config(), pairs, HashMap::new(), storage);

        let trades_before = store.trades.len();
        let orders_before = store.orders.len();

        let result = handle_removeitem_order(&mut store, "Alice", item, 20).await;
        assert!(result.is_ok(), "handle_removeitem_order failed: {:?}", result);

        // Pair stock matches physical storage post-withdraw.
        let physical_after = store.storage.total_item_amount(item);
        let pair_after = store.pairs.get(item).expect("pair must exist").item_stock;
        assert_eq!(
            pair_after, physical_after,
            "pair.item_stock must equal storage.total_item_amount after withdraw"
        );
        assert_eq!(
            pair_after, 30,
            "pair.item_stock expected 30 (50 - 20), got {}",
            pair_after
        );

        // Exactly one new Trade audit row of type RemoveStock.
        assert_eq!(
            store.trades.len(),
            trades_before + 1,
            "expected exactly one new trade audit row"
        );
        let trade: &Trade = &store.trades[trades_before];
        assert!(
            matches!(trade.trade_type, TradeType::RemoveStock),
            "expected RemoveStock trade type, got {:?}",
            trade.trade_type
        );
        assert_eq!(trade.amount, 20);
        assert!(
            trade.amount_currency.abs() < 1e-9,
            "trade.amount_currency expected 0.0, got {}",
            trade.amount_currency
        );
        assert_eq!(trade.item.as_str(), item);
        assert_eq!(trade.user_uuid, test_uuid("Alice"));

        // Exactly one new Order audit row of type RemoveItem.
        assert_eq!(
            store.orders.len(),
            orders_before + 1,
            "expected exactly one new order audit row"
        );
        let order: &Order = &store.orders[orders_before];
        assert!(
            matches!(order.order_type, OrderType::RemoveItem),
            "expected RemoveItem order type, got {:?}",
            order.order_type
        );
        assert_eq!(order.amount, 20);
        assert_eq!(order.item.as_str(), item);

        assert!(store.dirty, "store.dirty must be true after mutation");
    }

    #[tokio::test]
    async fn removeitem_trade_rejected_rolls_back_and_skips_audit() {
        // Pins both load-bearing invariants of `handle_removeitem_order`:
        //
        //   * "Audit-skip-on-failure" — when `perform_trade` returns
        //     `StoreError::TradeRejected`, the handler sets `record_audit = false`
        //     and `rollback::deposit_transfers` re-deposits the withdrawn items;
        //     NO `Trade::RemoveStock` and NO `Order::RemoveItem` rows appear.
        //
        //   * "Unconditional resync" — the tail block that rewrites
        //     `pair.item_stock = storage.total_item_amount(item)` MUST run on
        //     the rollback path. After the withdraw chest sync (50 → 30) and
        //     the rollback deposit chest sync (30 → 50), physical storage is
        //     back to 50 and the resync should leave `pair.item_stock` at 50.
        //     `store.dirty` must also be `true` because the tail block sets it.
        let item = "cobblestone";
        let (tx, rx) = mpsc::channel(64);
        let trade_fail_flag = Arc::new(AtomicBool::new(true));
        let chest_state = mock_state_seeded_like_storage(50);
        spawn_mock_bot_with_state(rx, chest_state, Some(Arc::clone(&trade_fail_flag)));

        let mut pairs = HashMap::new();
        let (k, p) = make_pair(item, 50, 0.0);
        pairs.insert(k, p);

        let storage = make_storage(item, 50);
        let mut store = Store::new_for_test(tx, test_config(), pairs, HashMap::new(), storage);

        let trades_before = store.trades.len();
        let orders_before = store.orders.len();
        let stock_before = store.pairs.get(item).expect("pair must exist").item_stock;

        let result = handle_removeitem_order(&mut store, "Alice", item, 20).await;
        // Handler returns Ok because the failure is reported via whisper, not Err.
        // (TradeRejected funnels into `final_status`; only BotDisconnected propagates.)
        assert!(
            result.is_ok(),
            "handler should whisper failure, not propagate error: {:?}",
            result
        );

        // Audit-skip invariant: `record_audit == false` ⇒ no Trade/Order rows.
        assert_eq!(
            store.trades.len(),
            trades_before,
            "no trade audit row on TradeRejected (record_audit=false)"
        );
        assert_eq!(
            store.orders.len(),
            orders_before,
            "no order audit row on TradeRejected (record_audit=false)"
        );

        // Unconditional-resync invariant: tail block runs on the rollback path.
        // Withdraw moved 50 → 30; rollback deposit moved 30 → 50; resync rewrites
        // pair.item_stock to total_item_amount, which is back to the pre-removeitem 50.
        let physical_after = store.storage.total_item_amount(item);
        let pair_after = store.pairs.get(item).expect("pair must exist").item_stock;
        assert_eq!(
            pair_after, physical_after,
            "pair.item_stock must equal storage.total_item_amount even on rollback path"
        );
        assert_eq!(
            pair_after, stock_before,
            "pair.item_stock must be back to pre-removeitem value (50) after rollback"
        );

        // `store.dirty` is set by the unconditional tail block.
        assert!(
            store.dirty,
            "store.dirty must be true after handler runs (unconditional resync sets it)"
        );

        // The fail flag must have been consumed (swap cleared it), proving the
        // mock injected the rejection on the first trade rather than later.
        assert!(
            !trade_fail_flag.load(Ordering::SeqCst),
            "trade fail flag should have been consumed by the first trade"
        );
    }
}
