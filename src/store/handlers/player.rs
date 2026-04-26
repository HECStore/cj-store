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

use tracing::{debug, warn};

use super::super::command::{parse_command, Command};
use super::super::{Store, utils};
use super::validation::validate_username;
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
    // Validate the username shape BEFORE any Mojang I/O: an attacker who
    // can make the bot see "X whispers: ..." chat lines would otherwise
    // trigger an uncached HTTPS request to api.mojang.com for every garbage
    // name they invent, and the URL interpolation is not percent-encoded.
    if let Err(reason) = validate_username(player_name) {
        debug!(
            player = player_name,
            command = command,
            reason = %reason,
            "Dropped whispered command from player with invalid username shape"
        );
        return Ok(());
    }

    // Cheap per-name rate-limit gate BEFORE the Mojang lookup. The real
    // per-user limiter below is keyed by the resolved UUID, so without this
    // gate a spammer could bypass the cooldown by whispering with many
    // distinct fake-but-valid usernames — each one forces an uncached
    // Mojang round-trip. Keyed by the lowercased raw name; since UUIDs are
    // 36-char dashed strings, they cannot collide with the 3-16 char
    // alphanumeric+underscore usernames that reach this point.
    let name_key = player_name.to_lowercase();
    if let Err(wait_duration) = store.rate_limiter.check(&name_key) {
        let wait_secs = wait_duration.as_secs_f64();
        let msg = if wait_secs < 1.0 {
            format!("Please wait {:.1}s before sending another message.", wait_secs)
        } else {
            format!("Please wait {:.0}s before sending another message.", wait_secs.ceil())
        };
        debug!(
            player = player_name,
            command = command,
            wait_ms = wait_duration.as_millis() as u64,
            "Pre-resolve rate-limited whispered command; whispering cooldown notice without Mojang lookup"
        );
        return utils::send_message_to_player(store, player_name, &msg).await;
    }

    let user_uuid = crate::mojang::resolve_user_uuid(player_name)
        .await
        .map_err(StoreError::ValidationError)?;
    utils::ensure_user_exists(store, player_name, &user_uuid);

    // Rate-limit check precedes parsing so malformed spam still counts
    // toward the per-user cooldown.
    if let Err(wait_duration) = store.rate_limiter.check(&user_uuid) {
        let wait_secs = wait_duration.as_secs_f64();
        let msg = if wait_secs < 1.0 {
            format!("Please wait {:.1}s before sending another message.", wait_secs)
        } else {
            format!("Please wait {:.0}s before sending another message.", wait_secs.ceil())
        };
        debug!(
            player = player_name,
            user_uuid = %user_uuid,
            command = command,
            wait_ms = wait_duration.as_millis() as u64,
            "Rate-limited player command; whispering cooldown notice"
        );
        return utils::send_message_to_player(store, player_name, &msg).await;
    }

    let parsed = match parse_command(command) {
        Ok(cmd) => cmd,
        Err(msg) => {
            debug!(
                player = player_name,
                user_uuid = %user_uuid,
                command = command,
                reason = %msg,
                "Rejected malformed player command"
            );
            return utils::send_message_to_player(store, player_name, &msg).await;
        }
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

        // Operator commands: authorization is enforced here (not in the
        // parser) so `parse_command` stays a pure function on the input
        // string and the "not authorized" whisper shares one code path.
        Command::AddItem { item, quantity } => {
            if !ensure_operator(store, player_name, &user_uuid, "additem").await? {
                return Ok(());
            }
            operator::handle_additem_order(store, player_name, &item, quantity).await
        }
        Command::RemoveItem { item, quantity } => {
            if !ensure_operator(store, player_name, &user_uuid, "removeitem").await? {
                return Ok(());
            }
            operator::handle_removeitem_order(store, player_name, &item, quantity).await
        }
        Command::AddCurrency { item, amount } => {
            if !ensure_operator(store, player_name, &user_uuid, "addcurrency").await? {
                return Ok(());
            }
            operator::handle_add_currency(store, player_name, &item, amount).await
        }
        Command::RemoveCurrency { item, amount } => {
            if !ensure_operator(store, player_name, &user_uuid, "removecurrency").await? {
                return Ok(());
            }
            operator::handle_remove_currency(store, player_name, &item, amount).await
        }
    }
}

/// Returns `Ok(true)` if the user is an operator; otherwise whispers the
/// standard rejection message, logs the denied attempt, and returns
/// `Ok(false)`. The `verb` is the command name (e.g. `"additem"`) used for
/// the log record so an operator investigating the audit trail can see what
/// privileged action the non-operator tried.
async fn ensure_operator(
    store: &Store,
    player_name: &str,
    user_uuid: &str,
    verb: &str,
) -> Result<bool, StoreError> {
    if utils::is_operator(store, user_uuid) {
        return Ok(true);
    }
    warn!(
        player = player_name,
        user_uuid = %user_uuid,
        command = verb,
        "Denied operator-only command to non-operator"
    );
    utils::send_message_to_player(
        store,
        player_name,
        "This command is only available to operators.",
    )
    .await?;
    Ok(false)
}

#[cfg(test)]
mod tests {
    //! Dispatcher-level tests: every whispered command path exercises
    //! `send_message_to_player`, so each test spawns a background task that
    //! auto-acks `BotInstruction::Whisper` and forwards the message text to
    //! a channel the test can assert on. All other dispatcher destinations
    //! (buy/sell/operator/etc.) are covered in their own modules.

    use super::*;
    use crate::config::Config;
    use crate::messages::BotInstruction;
    use crate::types::{Position, Storage, User};
    use std::collections::HashMap;
    use tokio::sync::mpsc;
    use tokio::time::{timeout, Duration};

    /// Absorb every `BotInstruction` emitted by the dispatcher and forward
    /// the text of any `Whisper` back to the test. Non-whisper instructions
    /// are acked permissively so a handler that issues (e.g.) a chest
    /// request does not hang the test.
    fn spawn_whisper_collector(
        mut rx: mpsc::Receiver<BotInstruction>,
    ) -> mpsc::UnboundedReceiver<(String, String)> {
        let (tx, out_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if let BotInstruction::Whisper { target, message, respond_to } = msg {
                    let _ = tx.send((target.clone(), message.clone()));
                    let _ = respond_to.send(Ok(()));
                }
            }
        });
        out_rx
    }

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

    fn make_store() -> (Store, mpsc::UnboundedReceiver<(String, String)>) {
        let (tx, rx) = mpsc::channel(16);
        let out_rx = spawn_whisper_collector(rx);
        let store = Store::new_for_test(
            tx,
            test_config(),
            HashMap::new(),
            HashMap::new(),
            Storage::default(),
        );
        (store, out_rx)
    }

    async fn recv_whisper(
        rx: &mut mpsc::UnboundedReceiver<(String, String)>,
    ) -> (String, String) {
        timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("whisper timeout")
            .expect("whisper channel closed")
    }

    fn expected_test_uuid(name: &str) -> String {
        let trimmed: String = name.chars().take(12).collect();
        let padded = format!("{:0>12}", trimmed);
        format!("00000000-0000-0000-0000-{}", padded)
    }

    #[tokio::test]
    async fn unknown_verb_whispers_parse_error_to_player() {
        let (mut store, mut whispers) = make_store();
        handle_player_command(&mut store, "Alice", "fizzbuzz 7").await.unwrap();
        let (target, message) = recv_whisper(&mut whispers).await;
        assert_eq!(target, "Alice");
        assert!(
            message.contains("Unknown command 'fizzbuzz'"),
            "expected parse error naming bad verb, got: {message}"
        );
    }

    #[tokio::test]
    async fn empty_command_whispers_help_hint() {
        let (mut store, mut whispers) = make_store();
        handle_player_command(&mut store, "Alice", "   ").await.unwrap();
        let (_, message) = recv_whisper(&mut whispers).await;
        assert!(
            message.contains("help"),
            "expected help hint for empty input, got: {message}"
        );
    }

    #[tokio::test]
    async fn non_operator_additem_is_rejected_with_operator_message() {
        let (mut store, mut whispers) = make_store();
        // Pre-seed user as non-operator so the rejection path runs without
        // ensure_user_exists creating them mid-dispatch.
        let uuid = expected_test_uuid("Alice");
        store.users.insert(uuid.clone(), User {
            uuid: uuid.clone(),
            username: "Alice".to_string(),
            balance: 0.0,
            operator: false,
        });

        handle_player_command(&mut store, "Alice", "additem cobblestone 64").await.unwrap();
        let (_, message) = recv_whisper(&mut whispers).await;
        assert_eq!(message, "This command is only available to operators.");
    }

    #[tokio::test]
    async fn non_operator_rejection_is_uniform_across_all_operator_commands() {
        // Guards against divergent rejection messages for the four operator
        // commands — they must share one code path / one whisper string.
        let cases = [
            "additem cobblestone 64",
            "removeitem cobblestone 64",
            "addcurrency diamond 10",
            "removecurrency diamond 10",
        ];
        for cmd in cases {
            let (mut store, mut whispers) = make_store();
            handle_player_command(&mut store, "Bob", cmd).await.unwrap();
            let (_, message) = recv_whisper(&mut whispers).await;
            assert_eq!(
                message, "This command is only available to operators.",
                "command {cmd} should be rejected with the shared operator message"
            );
        }
    }

    #[tokio::test]
    async fn rate_limit_violation_whispers_cooldown_notice() {
        let (mut store, mut whispers) = make_store();
        // First command passes the limiter; second one within the base
        // cooldown (2 s) trips it.
        handle_player_command(&mut store, "Alice", "status").await.unwrap();
        let _ = recv_whisper(&mut whispers).await; // consume status response

        handle_player_command(&mut store, "Alice", "status").await.unwrap();
        let (target, message) = recv_whisper(&mut whispers).await;
        assert_eq!(target, "Alice");
        assert!(
            message.starts_with("Please wait") && message.contains("before sending another message"),
            "expected cooldown notice, got: {message}"
        );
    }

    #[tokio::test]
    async fn rate_limiter_applies_to_malformed_commands() {
        // Spamming garbage must also consume the cooldown, otherwise a
        // spammer avoids rate limiting by sending junk.
        let (mut store, mut whispers) = make_store();
        handle_player_command(&mut store, "Alice", "???").await.unwrap();
        let _ = recv_whisper(&mut whispers).await; // parse error whisper

        handle_player_command(&mut store, "Alice", "???").await.unwrap();
        let (_, message) = recv_whisper(&mut whispers).await;
        assert!(
            message.starts_with("Please wait"),
            "second malformed command within cooldown should hit limiter, got: {message}"
        );
    }

    #[tokio::test]
    async fn dispatcher_creates_user_record_on_first_command() {
        let (mut store, mut whispers) = make_store();
        handle_player_command(&mut store, "Newbie", "status").await.unwrap();
        let _ = recv_whisper(&mut whispers).await;

        let uuid = expected_test_uuid("Newbie");
        let user = store.users.get(&uuid).expect("user auto-created");
        assert_eq!(user.username, "Newbie");
        assert!(!user.operator);
    }
}
