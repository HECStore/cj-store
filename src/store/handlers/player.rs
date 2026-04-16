//! Player command dispatcher.
//!
//! Commands are whispered to the bot and dispatched to sibling modules:
//! - Order commands (buy/sell/deposit/withdraw) → [`buy`], [`sell`], [`deposit`], [`withdraw`].
//!   Handlers here only validate and enqueue; actual chest I/O and trade
//!   GUI interaction happen later on the queue-processor task.
//! - Quick commands (balance/price/help/items/pay/queue/cancel/status) →
//!   [`info`]. These run inline because they need no bot movement.
//! - Operator admin commands (additem/removeitem/add/removecurrency) →
//!   [`operator`]. Gated here by [`utils::is_operator`].
//!
//! The queued-order processor entry points (`handle_deposit_balance_queued`,
//! `handle_withdraw_balance_queued`) and the in-process `pay_async` are
//! re-exported so external callers (orders.rs, integration tests) can keep
//! using `handlers::player::<fn>` paths.

use tracing::warn;

use super::super::{Store, utils};
use super::{buy, deposit, info, operator, sell, withdraw};

// Back-compat re-exports: orders.rs and tests reference these via
// `handlers::player::<fn>`. Keep them resolving through this module.
pub use deposit::handle_deposit_balance_queued;
#[cfg(test)]
pub use info::pay_async;
pub use withdraw::handle_withdraw_balance_queued;

pub async fn handle_player_command(
    store: &mut Store,
    player_name: &str,
    command: &str,
) -> Result<(), String> {
    // Resolve user UUID for rate limiting (creates user if needed).
    // UUID is the canonical identity key - rate limiting and ownership
    // must not rely on usernames since players can change them.
    let user_uuid = utils::resolve_user_uuid(store, player_name).await?;
    utils::ensure_user_exists(store, player_name, &user_uuid);

    // Rate limiting check - runs before command parsing so spamming any
    // command (including malformed ones) counts toward the per-user limit.
    if let Err(wait_duration) = store.rate_limiter.check(&user_uuid) {
        let wait_secs = wait_duration.as_secs_f64();
        let msg = if wait_secs < 1.0 {
            format!("Please wait {:.1}s before sending another message.", wait_secs)
        } else {
            format!("Please wait {:.0}s before sending another message.", wait_secs.ceil())
        };
        return utils::send_message_to_player(store, player_name, &msg).await;
    }

    let parts: Vec<&str> = command.split_whitespace().collect();

    match parts.first() {
        Some(&"buy") | Some(&"b") => {
            buy::handle(store, player_name, &user_uuid, &parts, command).await
        }
        Some(&"sell") | Some(&"s") => {
            sell::handle(store, player_name, &user_uuid, &parts, command).await
        }
        Some(&"deposit") | Some(&"d") => {
            deposit::handle_enqueue(store, player_name, &user_uuid, &parts).await
        }
        Some(&"withdraw") | Some(&"w") => {
            withdraw::handle_enqueue(store, player_name, &user_uuid, &parts).await
        }
        Some(&"bal") | Some(&"balance") => {
            info::handle_balance(store, player_name, &parts).await
        }
        Some(&"pay") => info::handle_pay(store, player_name, &parts, command).await,
        Some(&"price") | Some(&"p") => info::handle_price(store, player_name, &parts).await,
        Some(&"help") | Some(&"h") => info::handle_help(store, player_name, &parts).await,
        Some(&"items") => info::handle_items(store, player_name, &parts).await,
        Some(&"queue") | Some(&"q") => {
            info::handle_queue(store, player_name, &user_uuid, &parts).await
        }
        Some(&"cancel") | Some(&"c") => {
            info::handle_cancel(store, player_name, &user_uuid, &parts).await
        }
        Some(&"status") => info::handle_status(store, player_name).await,
        Some(&"additem") | Some(&"ai") => {
            if !utils::is_operator(store, &user_uuid) {
                return utils::send_message_to_player(
                    store,
                    player_name,
                    "This command is only available to operators.",
                )
                .await;
            }
            if parts.len() >= 3 {
                let item = utils::normalize_item_id(parts[1]);
                let quantity: u32 = parts[2]
                    .parse()
                    .map_err(|_| "Invalid quantity".to_string())?;
                operator::handle_additem_order(store, player_name, &item, quantity).await
            } else {
                utils::send_message_to_player(
                    store,
                    player_name,
                    "Usage: additem <item> <quantity>",
                )
                .await
            }
        }
        Some(&"removeitem") | Some(&"ri") => {
            if !utils::is_operator(store, &user_uuid) {
                return utils::send_message_to_player(
                    store,
                    player_name,
                    "This command is only available to operators.",
                )
                .await;
            }
            if parts.len() >= 3 {
                let item = utils::normalize_item_id(parts[1]);
                let quantity: u32 = parts[2]
                    .parse()
                    .map_err(|_| "Invalid quantity".to_string())?;
                operator::handle_removeitem_order(store, player_name, &item, quantity).await
            } else {
                utils::send_message_to_player(
                    store,
                    player_name,
                    "Usage: removeitem <item> <quantity>",
                )
                .await
            }
        }
        Some(&"addcurrency") | Some(&"ac") => {
            if !utils::is_operator(store, &user_uuid) {
                return utils::send_message_to_player(
                    store,
                    player_name,
                    "This command is only available to operators.",
                )
                .await;
            }
            if parts.len() >= 3 {
                let item = utils::normalize_item_id(parts[1]);
                let amount: f64 = parts[2]
                    .parse()
                    .map_err(|_| "Invalid amount".to_string())?;
                operator::handle_add_currency(store, player_name, &item, amount).await
            } else {
                utils::send_message_to_player(
                    store,
                    player_name,
                    "Usage: addcurrency <item> <amount>",
                )
                .await
            }
        }
        Some(&"removecurrency") | Some(&"rc") => {
            if !utils::is_operator(store, &user_uuid) {
                return utils::send_message_to_player(
                    store,
                    player_name,
                    "This command is only available to operators.",
                )
                .await;
            }
            if parts.len() >= 3 {
                let item = utils::normalize_item_id(parts[1]);
                let amount: f64 = parts[2]
                    .parse()
                    .map_err(|_| "Invalid amount".to_string())?;
                operator::handle_remove_currency(store, player_name, &item, amount).await
            } else {
                utils::send_message_to_player(
                    store,
                    player_name,
                    "Usage: removecurrency <item> <amount>",
                )
                .await
            }
        }
        Some(unknown_cmd) => {
            warn!("Unknown command '{}' from {}", unknown_cmd, player_name);
            utils::send_message_to_player(
                store,
                player_name,
                &format!(
                    "Unknown command '{}'. Use 'help' to see available commands.",
                    unknown_cmd
                ),
            )
            .await
        }
        None => {
            warn!("Empty command received from {}", player_name);
            utils::send_message_to_player(
                store,
                player_name,
                "Use 'help' to see available commands.",
            )
            .await
        }
    }
}
