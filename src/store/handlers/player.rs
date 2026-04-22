//! Player command dispatcher.
//!
//! Commands are whispered to the bot, parsed by [`super::super::command::parse_command`]
//! into a typed [`Command`], and then dispatched to sibling handler modules:
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

use super::super::command::{parse_command, Command};
use super::super::{Store, utils};
use super::{buy, deposit, info, operator, sell, withdraw};
use crate::error::StoreError;

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
) -> Result<(), StoreError> {
    // Resolve user UUID for rate limiting (creates user if needed).
    // UUID is the canonical identity key - rate limiting and ownership
    // must not rely on usernames since players can change them.
    let user_uuid = utils::resolve_user_uuid(player_name).await?;
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

    let parsed = match parse_command(command) {
        Ok(cmd) => cmd,
        Err(msg) => return utils::send_message_to_player(store, player_name, &msg).await,
    };

    match parsed {
        Command::Buy { item, quantity } => {
            buy::handle(store, player_name, &user_uuid, &item, quantity).await
        }
        Command::Sell { item, quantity } => {
            sell::handle(store, player_name, &user_uuid, &item, quantity).await
        }
        Command::Deposit { amount } => {
            deposit::handle_enqueue(store, player_name, &user_uuid, amount).await
        }
        Command::Withdraw { amount } => {
            withdraw::handle_enqueue(store, player_name, &user_uuid, amount).await
        }
        Command::Price { item, quantity } => {
            info::handle_price(store, player_name, &item, quantity).await
        }
        Command::Balance { target } => {
            info::handle_balance(store, player_name, target.as_deref()).await
        }
        Command::Pay { target, amount } => {
            info::handle_pay(store, player_name, &target, amount).await
        }
        Command::Items { page } => info::handle_items(store, player_name, page).await,
        Command::Queue { page } => info::handle_queue(store, player_name, &user_uuid, page).await,
        Command::Cancel { order_id } => {
            info::handle_cancel(store, player_name, &user_uuid, order_id).await
        }
        Command::Status => info::handle_status(store, player_name).await,
        Command::Help { topic } => info::handle_help(store, player_name, topic.as_deref()).await,

        // Operator commands: check permission before delegating.
        // Keeping authorization in the dispatcher (not the parser) means
        // `parse_command` can stay a pure function on the input string.
        Command::AddItem { item, quantity } => {
            if !utils::is_operator(store, &user_uuid) {
                return utils::send_message_to_player(
                    store,
                    player_name,
                    "This command is only available to operators.",
                )
                .await;
            }
            operator::handle_additem_order(store, player_name, &item, quantity).await
        }
        Command::RemoveItem { item, quantity } => {
            if !utils::is_operator(store, &user_uuid) {
                return utils::send_message_to_player(
                    store,
                    player_name,
                    "This command is only available to operators.",
                )
                .await;
            }
            operator::handle_removeitem_order(store, player_name, &item, quantity).await
        }
        Command::AddCurrency { item, amount } => {
            if !utils::is_operator(store, &user_uuid) {
                return utils::send_message_to_player(
                    store,
                    player_name,
                    "This command is only available to operators.",
                )
                .await;
            }
            operator::handle_add_currency(store, player_name, &item, amount).await
        }
        Command::RemoveCurrency { item, amount } => {
            if !utils::is_operator(store, &user_uuid) {
                return utils::send_message_to_player(
                    store,
                    player_name,
                    "This command is only available to operators.",
                )
                .await;
            }
            operator::handle_remove_currency(store, player_name, &item, amount).await
        }
    }
}
