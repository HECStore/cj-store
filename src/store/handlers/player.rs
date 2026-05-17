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

use super::super::command::{Command, parse_command};
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

/// Compose the `n:`-prefixed rate-limit key for a raw player name.
///
/// Centralized so the lowercase normalization is a single load-bearing site
/// — without it, ALICE/Alice/alice would each consume a distinct slot in
/// the limiter and a distinct whisper-gate, doubling (or worse) the spam
/// amplification a single attacker can extract by case-cycling. The
/// `to_lowercase()` allocation is skipped on already-lowercase input
/// (the common case) since `validate_username` permits any ASCII case.
fn name_rate_limit_key(player_name: &str) -> String {
    if player_name.bytes().any(|b| b.is_ascii_uppercase()) {
        format!("n:{}", player_name.to_lowercase())
    } else {
        format!("n:{player_name}")
    }
}

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
    //
    // The `n:` and `u:` gates each consume their own slot in the limiter:
    // a single legitimate command therefore reserves two slots, halving the
    // effective `MAX_RATE_LIMIT_ENTRIES` cap relative to user count. The
    // `n:` gate is the primary anti-DoS surface (it fires before any Mojang
    // I/O); the `u:` gate is a defense-in-depth check after a successful
    // resolve. Don't remove either — together they cap both fake-name spam
    // and resolved-user spam.
    //
    let name_key = name_rate_limit_key(player_name);
    if let Err(throttled) = store.rate_limiter.check(&name_key) {
        if !throttled.should_whisper {
            // Suppress the whisper to cap outbound chat amplification on
            // sustained spam (attacker cycling distinct usernames). The
            // rejection still lands; only the player-facing notice is
            // throttled — see `RateLimiter::check` returning `Throttled`.
            return Ok(());
        }
        return whisper_rate_limit_notice(
            store,
            player_name,
            command,
            throttled.wait,
            throttled.reason,
            None,
            "pre-resolve",
        )
        .await;
    }

    let user_uuid = match crate::mojang::resolve_user_uuid(player_name).await {
        Ok(uuid) => uuid,
        Err(reason) => {
            // Don't propagate the error past this point: doing so reaches
            // `mod.rs::handle_bot_message` which only `error!`-logs and never
            // tells the player anything. The player has already consumed
            // their `n:` rate-limit slot above, so silence here looks like
            // "the bot ignored me". Whisper a sanitized notice and stop.
            //
            // The typed `MojangResolveError` is converted to a sanitized
            // `StoreError` via the central `From` impl in `error.rs`:
            // `NotFound` → `UserNotFound` (passes the username through),
            // `InvalidShape` → `ValidationError`, everything else →
            // `MojangNetwork` (collapses to the generic whisper). Operator
            // visibility is preserved by the `warn!` above with `%reason`.
            tracing::warn!(
                player = player_name,
                command = command,
                reason = %reason,
                "Mojang UUID lookup failed; whispering player-facing notice"
            );
            let err: StoreError = reason.into();
            return utils::whisper_error_to_player(store, player_name, &err).await;
        }
    };
    utils::ensure_user_exists(store, player_name, &user_uuid);

    // Rate-limit check precedes parsing so malformed spam still counts
    // toward the per-user cooldown.
    let uuid_key = format!("u:{}", user_uuid);
    if let Err(throttled) = store.rate_limiter.check(&uuid_key) {
        if !throttled.should_whisper {
            return Ok(());
        }
        return whisper_rate_limit_notice(
            store,
            player_name,
            command,
            throttled.wait,
            throttled.reason,
            Some(&user_uuid),
            "post-resolve",
        )
        .await;
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
            info::handle_balance(store, player_name, &user_uuid, target.as_deref()).await
        }
        Command::Pay { target, amount } => {
            info::handle_pay(store, player_name, &user_uuid, &target, amount).await
        }
        Command::Items { page } => info::handle_items(store, player_name, page).await,
        Command::Queue { page } => info::handle_queue(store, player_name, &user_uuid, page).await,
        Command::Cancel { order_id } => {
            info::handle_cancel(store, player_name, &user_uuid, order_id).await
        }
        Command::Status => info::handle_status(store, player_name).await,
        Command::Help { topic } => {
            info::handle_help(store, player_name, &user_uuid, topic.as_deref()).await
        }

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

/// Format and whisper a rate-limit cooldown notice. Single helper used by
/// both the pre-resolve (`n:` gate) and post-resolve (`u:` gate) sites so
/// one fix lands in both places — earlier the two sites duplicated 14 lines
/// each and the sub-second formatting branch printed `"Please wait 0.0s"`
/// at the saturation cap, telling a still-throttled user to wait zero
/// seconds. The fixed formatting drops to milliseconds for sub-second
/// remainders with a 1ms floor so the displayed wait is never zero.
async fn whisper_rate_limit_notice(
    store: &Store,
    player_name: &str,
    command: &str,
    wait_duration: std::time::Duration,
    reason: crate::store::rate_limit::ThrottleReason,
    user_uuid: Option<&str>,
    stage: &str,
) -> Result<(), StoreError> {
    use crate::store::rate_limit::ThrottleReason;
    let wait_ms = wait_duration.as_millis() as u64;
    let secs_ceil = wait_duration.as_secs_f64().ceil().max(1.0) as u64;
    let msg = match reason {
        ThrottleReason::GlobalCap => {
            format!("Server is busy with too many active users; please try again in {secs_ceil}s.")
        }
        ThrottleReason::PerUser => {
            if wait_ms < 1_000 {
                format!(
                    "Please wait {} ms before sending another message.",
                    wait_ms.max(1)
                )
            } else {
                format!("Please wait {secs_ceil}s before sending another message.")
            }
        }
    };
    debug!(
        player = player_name,
        user_uuid = user_uuid.unwrap_or(""),
        command = command,
        wait_ms = wait_ms,
        reason = ?reason,
        stage = stage,
        "Rate-limited player command; whispering cooldown notice"
    );
    utils::send_message_to_player(store, player_name, &msg).await
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
    use tokio::time::{Duration, timeout};

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
                if let BotInstruction::Whisper {
                    target,
                    message,
                    respond_to,
                } = msg
                {
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

    #[test]
    fn name_rate_limit_key_collapses_case_variants() {
        // Pin the lowercase normalization: every case-variant of the same
        // player name must produce the SAME `n:`-key so a single attacker
        // cannot get one whisper-budget per case-variant by alternating
        // ALICE/alice/Alice. If a future refactor of `name_rate_limit_key`
        // drops or moves the lowercase step, this test must catch it.
        for variant in &["alice", "Alice", "ALICE", "aLiCe", "AlIcE"] {
            assert_eq!(
                name_rate_limit_key(variant),
                "n:alice",
                "case-variant `{variant}` must collapse to the same n:-key as `alice`"
            );
        }
    }

    #[test]
    fn name_rate_limit_key_skips_lowercase_path_for_already_lowercase() {
        // The skip exists so the warm path (already-lowercase names — the
        // common case for Minecraft players) does not pay a `to_lowercase()`
        // allocation. Verify the output matches the slow path so the skip is
        // observably equivalent.
        for n in &["bob", "alice42", "snake_case_user"] {
            assert_eq!(name_rate_limit_key(n), format!("n:{}", n.to_lowercase()));
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

    async fn recv_whisper(rx: &mut mpsc::UnboundedReceiver<(String, String)>) -> (String, String) {
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
        handle_player_command(&mut store, "Alice", "fizzbuzz 7")
            .await
            .unwrap();
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
        handle_player_command(&mut store, "Alice", "   ")
            .await
            .unwrap();
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
        store.users.insert(
            uuid.clone(),
            User {
                uuid: uuid.clone(),
                username: "Alice".to_string(),
                balance: 0.0,
                operator: false,
            },
        );

        handle_player_command(&mut store, "Alice", "additem cobblestone 64")
            .await
            .unwrap();
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
        handle_player_command(&mut store, "Alice", "status")
            .await
            .unwrap();
        let _ = recv_whisper(&mut whispers).await; // consume status response

        handle_player_command(&mut store, "Alice", "status")
            .await
            .unwrap();
        let (target, message) = recv_whisper(&mut whispers).await;
        assert_eq!(target, "Alice");
        assert!(
            message.starts_with("Please wait")
                && message.contains("before sending another message"),
            "expected cooldown notice, got: {message}"
        );
    }

    #[tokio::test]
    async fn rate_limiter_applies_to_malformed_commands() {
        // Spamming garbage must also consume the cooldown, otherwise a
        // spammer avoids rate limiting by sending junk.
        let (mut store, mut whispers) = make_store();
        handle_player_command(&mut store, "Alice", "???")
            .await
            .unwrap();
        let _ = recv_whisper(&mut whispers).await; // parse error whisper

        handle_player_command(&mut store, "Alice", "???")
            .await
            .unwrap();
        let (_, message) = recv_whisper(&mut whispers).await;
        assert!(
            message.starts_with("Please wait"),
            "second malformed command within cooldown should hit limiter, got: {message}"
        );
    }

    #[tokio::test]
    async fn dispatcher_creates_user_record_on_first_command() {
        // The mojang `cfg(test)` fixture embeds the username's literal
        // characters in the trailing UUID segment, and `ensure_user_exists`
        // now rejects non-canonical UUIDs (lowercase hex / hyphens only).
        // Pick a username whose first 12 chars are all valid hex digits so
        // the synthetic UUID passes the shape gate and the auto-created
        // user lands in `store.users`.
        let username = "abcdef";
        let (mut store, mut whispers) = make_store();
        handle_player_command(&mut store, username, "status")
            .await
            .unwrap();
        let _ = recv_whisper(&mut whispers).await;

        let uuid = expected_test_uuid(username);
        let user = store.users.get(&uuid).expect("user auto-created");
        assert_eq!(user.username, username);
        assert!(!user.operator);
    }
}
