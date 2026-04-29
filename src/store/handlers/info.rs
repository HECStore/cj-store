//! Read-only / quick informational commands:
//! `price`, `balance`, `pay`, `items`, `queue`, `cancel`, `status`, `help`.
//!
//! These run inline on the Store task (no bot trade round-trip) and therefore
//! live outside the queued-order path.

use tracing::{info, warn};

use super::super::{Store, state, utils};
use super::super::pricing;
use crate::error::StoreError;

pub(super) async fn handle_price(
    store: &mut Store,
    player_name: &str,
    item: &str,
    quantity: Option<u32>,
) -> Result<(), StoreError> {
    handle_price_command(store, player_name, item, quantity).await
}

pub(super) async fn handle_balance(
    store: &mut Store,
    player_name: &str,
    target: Option<&str>,
) -> Result<(), StoreError> {
    let target_name = target.unwrap_or(player_name);

    match get_user_balance_async(store, target_name).await {
        Ok(balance) => {
            let message = format!("{}'s balance: {:.2} diamonds", target_name, balance);
            utils::send_message_to_player(store, player_name, &message).await
        }
        Err(e) => {
            if e.contains("not found") || e.contains("No user") {
                utils::send_message_to_player(
                    store,
                    player_name,
                    &format!("{} has no account yet (balance: 0 diamonds)", target_name),
                )
                .await
            } else {
                utils::send_message_to_player(store, player_name, &e).await
            }
        }
    }
}

pub(super) async fn handle_pay(
    store: &mut Store,
    player_name: &str,
    recipient: &str,
    amount: f64,
) -> Result<(), StoreError> {
    match pay_async(store, player_name, recipient, amount).await {
        Ok(()) => {
            info!(
                payer = player_name,
                payee = recipient,
                amount,
                "Payment completed"
            );

            let payee_message = format!(
                "You received {:.2} diamonds from {}",
                amount, player_name
            );
            let _ = utils::send_message_to_player(store, recipient, &payee_message).await;

            let payer_message = format!("Paid {:.2} diamonds to {}", amount, recipient);
            utils::send_message_to_player(store, player_name, &payer_message).await
        }
        Err(e) => {
            warn!(
                payer = player_name,
                payee = recipient,
                amount,
                error = %e,
                "Payment failed"
            );
            utils::send_message_to_player(store, player_name, &e.user_message()).await
        }
    }
}

pub(super) async fn handle_items(
    store: &mut Store,
    player_name: &str,
    page: usize,
) -> Result<(), StoreError> {
    handle_items_command(store, player_name, page).await
}

pub(super) async fn handle_queue(
    store: &mut Store,
    player_name: &str,
    user_uuid: &str,
    page: usize,
) -> Result<(), StoreError> {
    let user_orders = store.order_queue.get_user_orders(user_uuid);

    if user_orders.is_empty() {
        let total_queue = store.order_queue.len();
        let msg = if total_queue > 0 {
            format!(
                "You have no orders queued. ({} orders in queue from other players)",
                total_queue
            )
        } else {
            "You have no orders queued. Queue is empty.".to_string()
        };
        return utils::send_message_to_player(store, player_name, &msg).await;
    }

    const ORDERS_PER_PAGE: usize = 4;
    let total_user_orders = user_orders.len();
    let total_pages = total_user_orders.div_ceil(ORDERS_PER_PAGE);

    if page > total_pages {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Invalid page. You have {} order(s), use 'queue 1' to 'queue {}'.",
                total_user_orders, total_pages
            ),
        )
        .await;
    }

    let start_idx = (page - 1) * ORDERS_PER_PAGE;
    let end_idx = (start_idx + ORDERS_PER_PAGE).min(total_user_orders);
    let page_orders = &user_orders[start_idx..end_idx];

    let orders_str: Vec<String> = page_orders
        .iter()
        .map(|(order, pos)| format!("#{} {} (pos {})", order.id, order.description(), pos))
        .collect();

    let total_queue = store.order_queue.len();
    let msg = if total_pages == 1 {
        format!(
            "Your queue ({}/{}): {}",
            total_user_orders,
            total_queue,
            orders_str.join(", ")
        )
    } else {
        format!(
            "Your queue (page {}/{}, {}/{}): {}",
            page,
            total_pages,
            total_user_orders,
            total_queue,
            orders_str.join(", ")
        )
    };
    utils::send_message_to_player(store, player_name, &msg).await
}

pub(super) async fn handle_cancel(
    store: &mut Store,
    player_name: &str,
    user_uuid: &str,
    order_id: u64,
) -> Result<(), StoreError> {
    if let Some(ref trade) = store.current_trade
        && trade.order().id == order_id {
            return utils::send_message_to_player(
                store,
                player_name,
                &format!(
                    "Order #{} is currently being processed ({}) and cannot be cancelled.",
                    order_id,
                    trade.phase()
                ),
            )
            .await;
        }

    match store.order_queue.cancel(user_uuid, order_id) {
        Ok(()) => {
            let msg = format!("Order #{} cancelled.", order_id);
            utils::send_message_to_player(store, player_name, &msg).await
        }
        Err(e) => utils::send_message_to_player(store, player_name, &e).await,
    }
}

pub(super) async fn handle_status(
    store: &mut Store,
    player_name: &str,
) -> Result<(), StoreError> {
    handle_status_command(store, player_name).await
}

pub(super) async fn handle_help(
    store: &mut Store,
    player_name: &str,
    topic: Option<&str>,
) -> Result<(), StoreError> {
    handle_help_command(store, player_name, topic).await
}

/// Reports buy and sell quotes for `quantity` of `item` (default one stack).
///
/// Quotes come from the constant-product AMM (`x * y = k`) and so include
/// slippage — the per-unit price depends on trade size. Both a total and an
/// average per-item price are shown.
async fn handle_price_command(
    store: &mut Store,
    player_name: &str,
    item: &str,
    quantity: Option<u32>,
) -> Result<(), StoreError> {
    let pair = match store.pairs.get(item) {
        Some(p) => p,
        None => {
            return utils::send_message_to_player(
                store,
                player_name,
                &format!("Item '{}' is not available for trading.", item),
            )
            .await;
        }
    };

    let qty = quantity.unwrap_or(pair.stack_size as u32);
    let qty_i32 = qty as i32;

    let buy_total = pricing::calculate_buy_cost(store, item, qty_i32);
    let sell_total = pricing::calculate_sell_payout(store, item, qty_i32);

    // Re-fetch the pair below: the original `pair` borrow had to be dropped
    // before the `pricing::*` calls (which need `&store`). The pair cannot
    // disappear in between — `pricing::*` only reads reserves, never mutates
    // the pairs map, and we already returned early above when it was missing.
    match (buy_total, sell_total) {
        (Some(buy_cost), Some(sell_payout)) => {
            let buy_per = buy_cost / (qty as f64);
            let sell_per = sell_payout / (qty as f64);
            let pair = store.pairs.get(item).expect("pair existed above");
            let message = format!(
                "{} x{}: Buy for {:.2} diamonds ({:.4}/ea), Sell for {:.2} diamonds ({:.4}/ea). Stock: {}",
                item, qty, buy_cost, buy_per, sell_payout, sell_per, pair.item_stock
            );
            utils::send_message_to_player(store, player_name, &message).await
        }
        (None, Some(sell_payout)) => {
            let sell_per = sell_payout / (qty as f64);
            let pair = store.pairs.get(item).expect("pair existed above");
            let message = format!(
                "{} x{}: Buy unavailable (exceeds stock {}), Sell for {:.2} diamonds ({:.4}/ea)",
                item, qty, pair.item_stock, sell_payout, sell_per
            );
            utils::send_message_to_player(store, player_name, &message).await
        }
        _ => {
            let pair = store.pairs.get(item).expect("pair existed above");
            let message = if pair.item_stock == 0 {
                format!("{}: No stock available (item_stock: 0)", item)
            } else if pair.currency_stock <= 0.0 {
                format!("{}: No currency reserve (currency_stock: 0)", item)
            } else {
                format!("{}: Price unavailable (insufficient reserves)", item)
            };
            utils::send_message_to_player(store, player_name, &message).await
        }
    }
}

async fn handle_status_command(
    store: &mut Store,
    player_name: &str,
) -> Result<(), StoreError> {
    let queue_len = store.order_queue.len();

    let status_msg = if store.processing_order {
        if let Some(ref trade) = store.current_trade {
            let activity = format!("{} [{}]", trade, trade.phase());

            if queue_len > 0 {
                format!(
                    "Status: {}. {} order(s) waiting in queue.",
                    activity, queue_len
                )
            } else {
                format!("Status: {}.", activity)
            }
        } else if queue_len > 0 {
            format!(
                "Status: Processing order. {} order(s) waiting in queue.",
                queue_len
            )
        } else {
            "Status: Processing order.".to_string()
        }
    } else if queue_len > 0 {
        format!(
            "Status: Ready. {} order(s) in queue, processing will start shortly.",
            queue_len
        )
    } else {
        "Status: Idle. No orders being processed. Queue is empty.".to_string()
    };

    utils::send_message_to_player(store, player_name, &status_msg).await
}

async fn handle_items_command(
    store: &mut Store,
    player_name: &str,
    page: usize,
) -> Result<(), StoreError> {
    let items: Vec<String> = store.pairs.keys().cloned().collect();

    if items.is_empty() {
        return utils::send_message_to_player(
            store,
            player_name,
            "No items available for trading.",
        )
        .await;
    }

    let mut sorted_items = items;
    sorted_items.sort();

    const ITEMS_PER_PAGE: usize = 4;
    let chunks: Vec<Vec<String>> = sorted_items
        .chunks(ITEMS_PER_PAGE)
        .map(|chunk| chunk.to_vec())
        .collect();

    let total_pages = chunks.len();

    if page > total_pages {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Invalid page. Use 'items 1' to 'items {}'.", total_pages),
        )
        .await;
    }

    let page_items = &chunks[page - 1];
    let items_list = page_items.join(", ");

    let message = if total_pages == 1 {
        format!("Items: {}", items_list)
    } else {
        format!("Items (page {}/{}): {}", page, total_pages, items_list)
    };

    utils::send_message_to_player(store, player_name, &message).await
}

/// Sends usage text for a single command, or (when `command` is `None`) the
/// full command list. Operator-only commands are included only for operators.
async fn handle_help_command(
    store: &mut Store,
    player_name: &str,
    command: Option<&str>,
) -> Result<(), StoreError> {
    let user_uuid = crate::mojang::resolve_user_uuid(player_name).await.ok();
    let is_op = user_uuid
        .as_ref()
        .map(|u| utils::is_operator(store, u))
        .unwrap_or(false);

    match command {
        Some("buy") | Some("b") => {
            utils::send_message_to_player(
                store,
                player_name,
                "buy <item> <quantity> - Buy items from the store. Example: buy cobblestone 64",
            )
            .await
        }
        Some("sell") | Some("s") => {
            utils::send_message_to_player(
                store,
                player_name,
                "sell <item> <quantity> - Sell items to the store. Example: sell iron_ingot 128",
            )
            .await
        }
        Some("price") | Some("p") => {
            utils::send_message_to_player(
                store,
                player_name,
                "price <item> [quantity] - Check buy/sell prices. Defaults to one stack. Example: price cobblestone 64",
            )
            .await
        }
        Some("balance") | Some("bal") => {
            utils::send_message_to_player(
                store,
                player_name,
                "balance [player] (or bal) - Check your balance, or another player's. Example: bal Steve",
            )
            .await
        }
        Some("pay") => {
            utils::send_message_to_player(
                store,
                player_name,
                "pay <player> <amount> - Pay diamonds to another player. Example: pay Steve 10.5",
            )
            .await
        }
        Some("deposit") | Some("d") => {
            utils::send_message_to_player(
                store,
                player_name,
                "deposit [amount] - Deposit physical diamonds into your balance. If no amount specified, credits whatever you put in the trade (max 768 / 12 stacks). Example: deposit, deposit 64",
            )
            .await
        }
        Some("withdraw") | Some("w") => {
            utils::send_message_to_player(
                store,
                player_name,
                "withdraw [amount] - Withdraw diamonds from your balance. If no amount specified, withdraws your full balance (whole diamonds only, max 768 / 12 stacks). Example: withdraw, withdraw 32",
            )
            .await
        }
        Some("items") => {
            utils::send_message_to_player(
                store,
                player_name,
                "items [page] - List available items for trading. Shows 4 items per page. Example: items 2",
            )
            .await
        }
        Some("queue") | Some("q") => {
            utils::send_message_to_player(
                store,
                player_name,
                "queue [page] (or q) - Show your pending orders (4 per page). Example: queue, queue 2",
            )
            .await
        }
        Some("cancel") | Some("c") => {
            utils::send_message_to_player(
                store,
                player_name,
                "cancel <order_id> (or c) - Cancel a pending order. Use 'queue' to see your order IDs. Example: c 5",
            )
            .await
        }
        Some("status") => {
            utils::send_message_to_player(
                store,
                player_name,
                "status - Check what the bot is currently doing (idle, buying, selling, etc.) and queue status.",
            )
            .await
        }
        Some("additem") | Some("ai") if is_op => {
            utils::send_message_to_player(
                store,
                player_name,
                "additem <item> <quantity> - (Operator) Add items to store stock. Example: additem diamond 100",
            )
            .await
        }
        Some("removeitem") | Some("ri") if is_op => {
            utils::send_message_to_player(
                store,
                player_name,
                "removeitem <item> <quantity> - (Operator) Remove items from store stock. Example: removeitem coal 50",
            )
            .await
        }
        Some("addcurrency") | Some("ac") if is_op => {
            utils::send_message_to_player(
                store,
                player_name,
                "addcurrency <item> <amount> - (Operator) Add diamonds to item's reserve. Example: addcurrency cobblestone 1000",
            )
            .await
        }
        Some("removecurrency") | Some("rc") if is_op => {
            utils::send_message_to_player(
                store,
                player_name,
                "removecurrency <item> <amount> - (Operator) Remove diamonds from item's reserve. Example: removecurrency cobblestone 500",
            )
            .await
        }
        Some(cmd) => utils::send_message_to_player(
            store,
            player_name,
            &format!("Unknown command '{}'. Use 'help' to see available commands.", cmd),
        )
        .await,
        None => {
            let base_commands = "Commands: buy (b), sell (s), price (p), items, balance (bal), pay, deposit (d), withdraw (w), queue (q), cancel (c), status, help (h). Use 'help <command>' for details.";
            if is_op {
                utils::send_message_to_player(
                    store,
                    player_name,
                    &format!(
                        "{} Operator: additem (ai), removeitem (ri), addcurrency (ac), removecurrency (rc)",
                        base_commands
                    ),
                )
                .await
            } else {
                utils::send_message_to_player(store, player_name, base_commands).await
            }
        }
    }
}

async fn get_user_balance_async(store: &mut Store, username: &str) -> Result<f64, String> {
    state::assert_invariants(store, "pre-balance", false)?;
    let uuid = crate::mojang::resolve_user_uuid(username).await?;
    utils::ensure_user_exists(store, username, &uuid);
    let bal = store.users.get(&uuid).map(|u| u.balance).unwrap_or(0.0);
    if !bal.is_finite() || bal < 0.0 {
        return Err("Internal error: invalid stored balance".to_string());
    }
    Ok(bal)
}

/// Transfers `amount` diamonds from `payer_username` to `payee_username`.
///
/// The payer must already exist in `store.users`; the payee is auto-created
/// if missing. Both usernames are refreshed from their UUIDs on each call so
/// a rename propagates into the user record.
pub async fn pay_async(
    store: &mut Store,
    payer_username: &str,
    payee_username: &str,
    amount: f64,
) -> Result<(), StoreError> {
    state::assert_invariants(store, "pre-pay", false)?;
    if !amount.is_finite() || amount <= 0.0 {
        warn!(
            payer = payer_username,
            payee = payee_username,
            amount,
            "Rejected payment: amount must be finite and positive"
        );
        return Err(StoreError::ValidationError("Amount must be positive".to_string()));
    }

    let payer_uuid = crate::mojang::resolve_user_uuid(payer_username)
        .await
        .map_err(StoreError::ValidationError)?;
    let payee_uuid = crate::mojang::resolve_user_uuid(payee_username)
        .await
        .map_err(StoreError::ValidationError)?;

    if !store.users.contains_key(&payer_uuid) {
        return Err(StoreError::ValidationError(format!(
            "Payer '{}' not found in store records",
            payer_username
        )));
    }

    utils::ensure_user_exists(store, payee_username, &payee_uuid);

    let payer_balance = store.expect_user(&payer_uuid, "pay/payer-balance")?.balance;
    if payer_balance < amount {
        warn!(
            payer = payer_username,
            payee = payee_username,
            balance = payer_balance,
            amount,
            "Rejected payment: insufficient payer balance"
        );
        return Err(StoreError::ValidationError(format!(
            "Insufficient balance. Required: {}, Available: {}",
            amount, payer_balance
        )));
    }

    {
        let payer = store.expect_user_mut(&payer_uuid, "pay/payer-debit")?;
        payer.balance -= amount;
        payer.username = payer_username.to_owned();
    }
    {
        let payee = store.expect_user_mut(&payee_uuid, "pay/payee-credit")?;
        payee.balance += amount;
        payee.username = payee_username.to_owned();
    }
    store.dirty = true;
    store.dirty_users.insert(payer_uuid.clone());
    store.dirty_users.insert(payee_uuid.clone());

    state::assert_invariants(store, "post-pay", true)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Unit tests for branches that do not require a mock bot (i.e. they
    //! return before any `send_message_to_player` call). The happy-path and
    //! insufficient-balance branches of `pay_async` are covered in
    //! `store::orders::tests`, where a mock bot is already wired up.

    use super::*;
    use crate::config::Config;
    use crate::types::{Position, Storage};
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    fn empty_store() -> Store {
        let (tx, _rx) = mpsc::channel(1);
        let config = Config {
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
        };
        Store::new_for_test(tx, config, HashMap::new(), HashMap::new(), Storage::default())
    }

    #[tokio::test]
    async fn pay_async_rejects_zero_amount() {
        let mut store = empty_store();
        let err = pay_async(&mut store, "Alice", "Bob", 0.0).await.unwrap_err();
        assert!(
            matches!(err, StoreError::ValidationError(ref m) if m.contains("positive")),
            "expected ValidationError(positive), got {err:?}"
        );
    }

    #[tokio::test]
    async fn pay_async_rejects_negative_amount() {
        let mut store = empty_store();
        let err = pay_async(&mut store, "Alice", "Bob", -1.0).await.unwrap_err();
        assert!(
            matches!(err, StoreError::ValidationError(ref m) if m.contains("positive")),
            "expected ValidationError(positive), got {err:?}"
        );
    }

    #[tokio::test]
    async fn pay_async_rejects_nan_amount() {
        let mut store = empty_store();
        let err = pay_async(&mut store, "Alice", "Bob", f64::NAN)
            .await
            .unwrap_err();
        assert!(
            matches!(err, StoreError::ValidationError(_)),
            "expected ValidationError for NaN, got {err:?}"
        );
    }

    #[tokio::test]
    async fn pay_async_rejects_infinite_amount() {
        let mut store = empty_store();
        let err = pay_async(&mut store, "Alice", "Bob", f64::INFINITY)
            .await
            .unwrap_err();
        assert!(
            matches!(err, StoreError::ValidationError(_)),
            "expected ValidationError for infinity, got {err:?}"
        );
    }

    #[tokio::test]
    async fn pay_async_rejects_unknown_payer() {
        // No users in the store: the payer-not-found branch fires after the
        // amount guard passes.
        let mut store = empty_store();
        let err = pay_async(&mut store, "Ghost", "Bob", 5.0).await.unwrap_err();
        assert!(
            matches!(err, StoreError::ValidationError(ref m) if m.contains("not found")),
            "expected ValidationError(not found), got {err:?}"
        );
    }
}
