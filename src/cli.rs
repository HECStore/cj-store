//! Interactive CLI menu for store operators.
//!
//! This module runs on a dedicated blocking thread (not a Tokio task) because
//! `dialoguer` performs synchronous terminal I/O. It communicates with the
//! async `Store` actor by sending `CliMessage`s over an `mpsc` channel and
//! awaiting replies via `oneshot` channels using `blocking_send` /
//! `blocking_recv`.

use crate::messages::{ChatCommand, CliMessage, StoreMessage};
use crate::types::TradeType;
use dialoguer::{Confirm, Input, Select};
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info, warn};

/// Retry a dialoguer prompt on transient I/O error rather than aborting.
///
/// A terminal read can fail for a variety of non-fatal reasons (EINTR during
/// terminal resize, lost/reattached stdin on some shells, etc.). Previously
/// every `.interact()` was wrapped in `.expect(..)`, which killed the entire
/// CLI task on the first hiccup. The loop re-prompts with a short backoff so
/// the operator sees the prompt again instead of the process exiting.
/// Compute buy/sell quotes from a pair's reserves and fee spread.
///
/// Returns `(None, None)` when either reserve is zero: the pair is untradeable
/// and the constant-product mid-price would be undefined or infinite.
fn quote_prices(item_stock: i32, currency_stock: f64, fee: f64) -> (Option<f64>, Option<f64>) {
    if item_stock > 0 && currency_stock > 0.0 {
        let base = currency_stock / (item_stock as f64);
        (Some(base * (1.0 + fee)), Some(base * (1.0 - fee)))
    } else {
        (None, None)
    }
}

fn with_retry<T, E: std::fmt::Display>(desc: &str, mut f: impl FnMut() -> Result<T, E>) -> T {
    loop {
        match f() {
            Ok(v) => return v,
            Err(e) => {
                warn!("[CLI] {desc}: {e} — retrying");
                std::thread::sleep(std::time::Duration::from_millis(crate::constants::DELAY_MEDIUM_MS));
            }
        }
    }
}

/// Runs the CLI task, providing an interactive menu to manage the store.
///
/// This function blocks the calling thread in a loop until the operator
/// selects "Exit". On exit it performs a coordinated shutdown: it sends a
/// `Shutdown` message, waits for the `Store` to confirm, then drops
/// `store_tx` so the `Store`'s receiver closes and its task can terminate.
///
/// `chat_tx` is `None` when chat is disabled; chat-related menu entries
/// are only shown when it is `Some`.
pub fn cli_task(
    store_tx: mpsc::Sender<StoreMessage>,
    chat_tx: Option<mpsc::Sender<ChatCommand>>,
) {
    let chat_enabled = chat_tx.is_some();
    loop {
        // Indices in the match below are positional — adding/removing an entry
        // shifts every case after it. Chat entries appear only when chat is
        // enabled.
        let mut options: Vec<&str> = vec![
            "Get user balances",
            "Get pairs",
            "Set operator status",
            "Add node (no validation)",
            "Add node (with bot validation)",
            "Discover storage (scan for existing nodes)",
            "Remove node",
            "Add pair",
            "Remove pair",
            "View storage",
            "View recent trades",
            "Audit state",
            "Repair state (recompute pair stock)",
            "Restart Bot",
            "Clear stuck order",
        ];
        if chat_enabled {
            // CHAT.md: full set of operator-facing chat actions. The label
            // strings are the dispatch keys in the match below — keep them
            // in sync. "Chat: show token spend today" reuses the same
            // status snapshot but renders a tokens-only view.
            options.push("Chat: status");
            options.push("Chat: pause");
            options.push("Chat: resume");
            options.push("Chat: toggle dry-run");
            options.push("Chat: clear moderation backoff");
            options.push("Chat: run retention sweep");
            options.push("Chat: run reflection now");
            options.push("Chat: show today's decision log (last N)");
            options.push("Chat: show token spend today");
            options.push("Chat: replay event <event_ts>");
            options.push("Chat: reset player memory <username>");
            options.push("Chat: dump player memory <username>");
            options.push("Chat: set operator trust <username>");
            options.push("Chat: clear operator trust <username>");
            options.push("Chat: regenerate persona");
            options.push("Chat: forget player <username>");
        }
        options.push("Exit");
        let selection = with_retry("Failed to read selection", || {
            Select::new()
                .with_prompt("Select an action")
                .items(&options)
                .default(0)
                .interact()
        });

        // Resolve the chosen menu label to its handler. Indexing the
        // dynamic `options` vec by selection lets us avoid hard-coding
        // chat-entry indices that would shift if menu items move.
        let label = options.get(selection).copied().unwrap_or("Exit");
        match label {
            "Get user balances" => get_balances(&store_tx),
            "Get pairs" => get_pairs(&store_tx),
            "Set operator status" => set_operator(&store_tx),
            "Add node (no validation)" => add_node(&store_tx),
            "Add node (with bot validation)" => add_node_with_validation(&store_tx),
            "Discover storage (scan for existing nodes)" => discover_storage(&store_tx),
            "Remove node" => remove_node(&store_tx),
            "Add pair" => add_pair(&store_tx),
            "Remove pair" => remove_pair(&store_tx),
            "View storage" => view_storage(&store_tx),
            "View recent trades" => view_trades(&store_tx),
            "Audit state" => audit_state(&store_tx, false),
            "Repair state (recompute pair stock)" => audit_state(&store_tx, true),
            "Restart Bot" => restart_bot(&store_tx),
            "Clear stuck order" => clear_stuck_order(&store_tx),
            "Chat: status" => chat_status(chat_tx.as_ref()),
            "Chat: pause" => chat_set_paused(chat_tx.as_ref(), true),
            "Chat: resume" => chat_set_paused(chat_tx.as_ref(), false),
            "Chat: toggle dry-run" => chat_toggle_dry_run(chat_tx.as_ref()),
            "Chat: clear moderation backoff" => chat_clear_moderation(chat_tx.as_ref()),
            "Chat: run retention sweep" => chat_run_sweep(chat_tx.as_ref()),
            "Chat: run reflection now" => chat_run_reflection(chat_tx.as_ref()),
            "Chat: show today's decision log (last N)" => chat_show_decision_log(chat_tx.as_ref()),
            "Chat: show token spend today" => chat_show_token_spend(chat_tx.as_ref()),
            "Chat: replay event <event_ts>" => chat_replay_event(chat_tx.as_ref()),
            "Chat: reset player memory <username>" => chat_reset_player_memory(chat_tx.as_ref()),
            "Chat: dump player memory <username>" => chat_dump_player_memory(chat_tx.as_ref()),
            "Chat: set operator trust <username>" => chat_set_operator_trust(chat_tx.as_ref(), true),
            "Chat: clear operator trust <username>" => chat_set_operator_trust(chat_tx.as_ref(), false),
            "Chat: regenerate persona" => chat_regenerate_persona(chat_tx.as_ref()),
            "Chat: forget player <username>" => chat_forget_player(chat_tx.as_ref()),
            "Exit" => {
                info!("[CLI] Initiating graceful shutdown");
                // Tell chat to drain in-flight work first; ignore failures
                // (chat may already be down).
                if let Some(ref ct) = chat_tx {
                    let (ack_tx, ack_rx) = oneshot::channel();
                    if ct.blocking_send(ChatCommand::Shutdown { ack: ack_tx }).is_ok() {
                        let _ = ack_rx.blocking_recv();
                    }
                }
                let (response_tx, response_rx) = oneshot::channel();
                let msg = StoreMessage::FromCli(CliMessage::Shutdown {
                    respond_to: response_tx,
                });

                if store_tx.blocking_send(msg).is_err() {
                    error!("[CLI] Shutdown send failed: Store channel closed");
                    return;
                }

                if response_rx.blocking_recv().is_err() {
                    error!("[CLI] Shutdown response channel closed without reply");
                    return;
                }

                // Drop senders so the Store's / Chat's receivers close and
                // their tasks can terminate.
                drop(chat_tx);
                drop(store_tx);
                info!("[CLI] Shutdown complete");
                break;
            }
            other => {
                warn!("[CLI] Unknown menu label: {other}");
            }
        }
    }
}

// ---- Chat command helpers --------------------------------------------------

fn chat_status(chat_tx: Option<&mpsc::Sender<ChatCommand>>) {
    let Some(ct) = chat_tx else {
        println!("Chat is not enabled.");
        return;
    };
    let (resp_tx, resp_rx) = oneshot::channel();
    if ct.blocking_send(ChatCommand::Status { respond_to: resp_tx }).is_err() {
        println!("Chat task is not running.");
        return;
    }
    match resp_rx.blocking_recv() {
        Ok(s) => {
            println!("\n=== Chat status ===");
            println!("enabled:           {}", s.enabled);
            println!("paused:            {}", s.paused);
            println!("dry-run effective: {}", s.dry_run_effective);
            println!(
                "bot username:      {}",
                s.bot_username.as_deref().unwrap_or("<unknown>")
            );
            println!(
                "composer tokens:   in={} out={}",
                s.composer_input_today, s.composer_output_today
            );
            println!(
                "classifier tokens: in={} out={}",
                s.classifier_input_today, s.classifier_output_today
            );
            println!(
                "today USD spend:   ${:.4} / ${:.2} cap",
                s.estimated_usd_today, s.usd_cap
            );
            println!("history drops today: {}", s.history_drops_today);
            println!(
                "moderation backoff: {}",
                s.moderation_backoff_until.as_deref().unwrap_or("<none>")
            );
            println!(
                "model-404 backoff:  {}",
                s.model_404_backoff_until.as_deref().unwrap_or("<none>")
            );
            println!(
                "composer throttle backoff: {}",
                s.composer_throttle_backoff_until
                    .as_deref()
                    .unwrap_or("<none>")
            );
            println!(
                "persona regen cooldown: {}",
                s.persona_regen_cooldown_until.as_deref().unwrap_or("<none>")
            );
            println!(
                "last persona regen: {}",
                s.last_persona_regenerated_at.as_deref().unwrap_or("<never>")
            );
            println!("pending_adjustments: {}", s.pending_adjustments_count);
            println!("in_critical_section: {}", s.critical_section_active);
            println!(
                "last composer call: {} (${:.4})",
                s.last_composer_call_at.as_deref().unwrap_or("<never>"),
                s.last_composer_call_usd
            );
            println!("web_fetches today: {}", s.web_fetches_today);
            println!(
                "classifier active senders: {}",
                s.classifier_active_senders
            );
            println!(
                "proactive threading: enabled={} dispatch_wired={}",
                s.proactive_threading_enabled, s.proactive_dispatch_wired
            );
            println!("====================\n");
        }
        Err(_) => println!("Chat task did not respond."),
    }
}

/// Tokens-only view of `ChatCommand::Status`. Same RPC under the hood
/// — a separate menu entry keeps the high-frequency operator question
/// ("am I burning the API budget?") one keystroke away from a noisy
/// full status dump.
fn chat_show_token_spend(chat_tx: Option<&mpsc::Sender<ChatCommand>>) {
    let Some(ct) = chat_tx else {
        println!("Chat is not enabled.");
        return;
    };
    let (resp_tx, resp_rx) = oneshot::channel();
    if ct.blocking_send(ChatCommand::Status { respond_to: resp_tx }).is_err() {
        println!("Chat task is not running.");
        return;
    }
    match resp_rx.blocking_recv() {
        Ok(s) => {
            println!("\n=== Chat token spend today ===");
            println!(
                "composer:   in={} out={}",
                s.composer_input_today, s.composer_output_today
            );
            println!(
                "classifier: in={} out={}",
                s.classifier_input_today, s.classifier_output_today
            );
            println!(
                "estimated:  ${:.4} / ${:.2} cap",
                s.estimated_usd_today, s.usd_cap
            );
            println!("==============================\n");
        }
        Err(_) => println!("Chat task did not respond."),
    }
}

/// Show the last N entries of today's decision log. Default N=50 mirrors
/// the typical "what just happened?" troubleshooting window.
fn chat_show_decision_log(chat_tx: Option<&mpsc::Sender<ChatCommand>>) {
    let Some(ct) = chat_tx else {
        println!("Chat is not enabled.");
        return;
    };
    let last_n: usize = with_retry("Failed to read N", || {
        Input::new()
            .with_prompt("How many decision-log entries to show")
            .default(50_usize)
            .interact_text()
    });
    let (resp_tx, resp_rx) = oneshot::channel();
    if ct
        .blocking_send(ChatCommand::ShowDecisionLog { last_n, respond_to: resp_tx })
        .is_err()
    {
        println!("Chat task is not running.");
        return;
    }
    match resp_rx.blocking_recv() {
        Ok(Ok(lines)) => {
            if lines.is_empty() {
                println!("No decision-log entries today.");
            } else {
                println!("\n=== Decision log (last {}) ===", lines.len());
                for line in lines {
                    println!("{}", line);
                }
                println!("====================\n");
            }
        }
        Ok(Err(e)) => println!("Failed to read decision log: {e}"),
        Err(_) => println!("Chat task did not respond."),
    }
}

/// Re-run a single past chat decision against the current persona/memory
/// for offline diagnosis. The `event_ts` is a freeform string the chat
/// module resolves to a row in today's history.
fn chat_replay_event(chat_tx: Option<&mpsc::Sender<ChatCommand>>) {
    let Some(ct) = chat_tx else {
        println!("Chat is not enabled.");
        return;
    };
    let event_ts: String = with_retry("Failed to read event_ts", || {
        Input::new()
            .with_prompt("Enter event_ts (timestamp string from history)")
            .interact_text()
    });
    let (resp_tx, resp_rx) = oneshot::channel();
    if ct
        .blocking_send(ChatCommand::ReplayEvent { event_ts: event_ts.clone(), respond_to: resp_tx })
        .is_err()
    {
        println!("Chat task is not running.");
        return;
    }
    match resp_rx.blocking_recv() {
        Ok(Ok(out)) => {
            println!("\n=== Replay of {event_ts} ===");
            println!("{out}");
            println!("====================\n");
        }
        Ok(Err(e)) => println!("Replay failed: {e}"),
        Err(_) => println!("Chat task did not respond."),
    }
}

/// Wipe a single player's memory file. Confirmed once — destructive but
/// scoped to one player, so a single confirm matches the prompt
/// discipline.
fn chat_reset_player_memory(chat_tx: Option<&mpsc::Sender<ChatCommand>>) {
    let Some(ct) = chat_tx else {
        println!("Chat is not enabled.");
        return;
    };
    let username: String = with_retry("Failed to read username", || {
        Input::new()
            .with_prompt("Enter player username to reset")
            .interact_text()
    });
    let confirmed = with_retry("Failed to read confirmation", || {
        Confirm::new()
            .with_prompt(format!("Really reset memory for '{}'? This wipes their per-player memory file.", username))
            .default(false)
            .interact()
    });
    if !confirmed {
        println!("Cancelled.");
        return;
    }
    let (resp_tx, resp_rx) = oneshot::channel();
    if ct
        .blocking_send(ChatCommand::ResetPlayerMemory { username: username.clone(), respond_to: resp_tx })
        .is_err()
    {
        println!("Chat task is not running.");
        return;
    }
    match resp_rx.blocking_recv() {
        Ok(Ok(())) => println!("Memory for '{username}' reset."),
        Ok(Err(e)) => println!("Reset failed: {e}"),
        Err(_) => println!("Chat task did not respond."),
    }
}

/// Print a player's current memory contents (read-only).
fn chat_dump_player_memory(chat_tx: Option<&mpsc::Sender<ChatCommand>>) {
    let Some(ct) = chat_tx else {
        println!("Chat is not enabled.");
        return;
    };
    let username: String = with_retry("Failed to read username", || {
        Input::new()
            .with_prompt("Enter player username to dump")
            .interact_text()
    });
    let (resp_tx, resp_rx) = oneshot::channel();
    if ct
        .blocking_send(ChatCommand::DumpPlayerMemory { username: username.clone(), respond_to: resp_tx })
        .is_err()
    {
        println!("Chat task is not running.");
        return;
    }
    match resp_rx.blocking_recv() {
        Ok(Ok(contents)) => {
            println!("\n=== Memory for {username} ===");
            println!("{contents}");
            println!("====================\n");
        }
        Ok(Err(e)) => println!("Dump failed: {e}"),
        Err(_) => println!("Chat task did not respond."),
    }
}

/// Set or clear the operator-trust bit for a player. The single helper
/// powers both menu entries via the `set` argument so the prompt flow
/// stays identical (only the verb in the confirmation changes).
fn chat_set_operator_trust(chat_tx: Option<&mpsc::Sender<ChatCommand>>, set: bool) {
    let Some(ct) = chat_tx else {
        println!("Chat is not enabled.");
        return;
    };
    let username: String = with_retry("Failed to read username", || {
        Input::new()
            .with_prompt("Enter player username")
            .interact_text()
    });
    let reason: String = with_retry("Failed to read reason", || {
        Input::new()
            .with_prompt("Reason (free text, recorded in audit log)")
            .interact_text()
    });
    let verb = if set { "GRANT" } else { "REVOKE" };
    let confirmed = with_retry("Failed to read confirmation", || {
        Confirm::new()
            .with_prompt(format!("{verb} operator trust for '{}'?", username))
            .default(false)
            .interact()
    });
    if !confirmed {
        println!("Cancelled.");
        return;
    }
    let (resp_tx, resp_rx) = oneshot::channel();
    if ct
        .blocking_send(ChatCommand::SetOperatorTrust {
            username: username.clone(),
            set,
            reason,
            respond_to: resp_tx,
        })
        .is_err()
    {
        println!("Chat task is not running.");
        return;
    }
    match resp_rx.blocking_recv() {
        Ok(Ok(())) => println!(
            "Operator trust for '{username}' is now {}.",
            if set { "GRANTED" } else { "REVOKED" }
        ),
        Ok(Err(e)) => println!("Failed: {e}"),
        Err(_) => println!("Chat task did not respond."),
    }
}

/// Regenerate the bot's persona file. Confirmed once because it
/// overwrites the active persona — recoverable from the persona archive
/// but still a behavior-changing action.
fn chat_regenerate_persona(chat_tx: Option<&mpsc::Sender<ChatCommand>>) {
    let Some(ct) = chat_tx else {
        println!("Chat is not enabled.");
        return;
    };
    let confirmed = with_retry("Failed to read confirmation", || {
        Confirm::new()
            .with_prompt("Really regenerate the persona? Active persona will be archived and replaced.")
            .default(false)
            .interact()
    });
    if !confirmed {
        println!("Cancelled.");
        return;
    }
    let (resp_tx, resp_rx) = oneshot::channel();
    if ct
        .blocking_send(ChatCommand::RegeneratePersona { respond_to: resp_tx })
        .is_err()
    {
        println!("Chat task is not running.");
        return;
    }
    match resp_rx.blocking_recv() {
        Ok(Ok(())) => println!("Persona regenerated."),
        Ok(Err(e)) => println!("Regeneration failed: {e}"),
        Err(_) => println!("Chat task did not respond."),
    }
}

/// Forget a player entirely (memory + audit traces). Double-confirmed —
/// this is the most destructive chat action exposed to the CLI, so the
/// second prompt forces the operator to retype the username.
fn chat_forget_player(chat_tx: Option<&mpsc::Sender<ChatCommand>>) {
    let Some(ct) = chat_tx else {
        println!("Chat is not enabled.");
        return;
    };
    let username: String = with_retry("Failed to read username", || {
        Input::new()
            .with_prompt("Enter player username to FORGET (irreversible)")
            .interact_text()
    });
    let confirmed1 = with_retry("Failed to read confirmation", || {
        Confirm::new()
            .with_prompt(format!("Really forget '{}'? Removes all chat-side traces of this player.", username))
            .default(false)
            .interact()
    });
    if !confirmed1 {
        println!("Cancelled.");
        return;
    }
    // Second confirmation: the operator must retype the username, not
    // just press Enter. Defeats the muscle-memory "yes, yes, yes" that a
    // single boolean confirm couldn't catch.
    let retyped: String = with_retry("Failed to read username", || {
        Input::new()
            .with_prompt(format!("Type '{}' again to confirm", username))
            .interact_text()
    });
    if retyped.trim() != username.trim() {
        println!("Username mismatch — cancelled.");
        return;
    }
    let (resp_tx, resp_rx) = oneshot::channel();
    if ct
        .blocking_send(ChatCommand::ForgetPlayer { username: username.clone(), respond_to: resp_tx })
        .is_err()
    {
        println!("Chat task is not running.");
        return;
    }
    match resp_rx.blocking_recv() {
        Ok(Ok(())) => println!("Player '{username}' forgotten."),
        Ok(Err(e)) => println!("Forget failed: {e}"),
        Err(_) => println!("Chat task did not respond."),
    }
}

fn chat_set_paused(chat_tx: Option<&mpsc::Sender<ChatCommand>>, paused: bool) {
    let Some(ct) = chat_tx else {
        println!("Chat is not enabled.");
        return;
    };
    let (resp_tx, resp_rx) = oneshot::channel();
    if ct
        .blocking_send(ChatCommand::SetPaused {
            paused,
            respond_to: resp_tx,
        })
        .is_err()
    {
        println!("Chat task is not running.");
        return;
    }
    let _ = resp_rx.blocking_recv();
    println!("Chat {}.", if paused { "paused" } else { "resumed" });
}

fn chat_toggle_dry_run(chat_tx: Option<&mpsc::Sender<ChatCommand>>) {
    let Some(ct) = chat_tx else {
        println!("Chat is not enabled.");
        return;
    };
    // Without snapshotting current state we'd toggle blindly; query
    // status first.
    let (q_tx, q_rx) = oneshot::channel();
    if ct.blocking_send(ChatCommand::Status { respond_to: q_tx }).is_err() {
        println!("Chat task is not running.");
        return;
    }
    let now = match q_rx.blocking_recv() {
        Ok(s) => s.dry_run_effective,
        Err(_) => {
            println!("Chat task did not respond.");
            return;
        }
    };
    let want = !now;
    let (resp_tx, resp_rx) = oneshot::channel();
    if ct
        .blocking_send(ChatCommand::SetDryRun {
            dry_run: want,
            respond_to: resp_tx,
        })
        .is_err()
    {
        println!("Chat task is not running.");
        return;
    }
    let _ = resp_rx.blocking_recv();
    println!("Chat dry-run is now {}.", if want { "ON" } else { "OFF" });
}

fn chat_clear_moderation(chat_tx: Option<&mpsc::Sender<ChatCommand>>) {
    let Some(ct) = chat_tx else {
        println!("Chat is not enabled.");
        return;
    };
    let (resp_tx, resp_rx) = oneshot::channel();
    if ct
        .blocking_send(ChatCommand::ClearModerationBackoff { respond_to: resp_tx })
        .is_err()
    {
        println!("Chat task is not running.");
        return;
    }
    let _ = resp_rx.blocking_recv();
    println!("Moderation backoff cleared.");
}

fn chat_run_reflection(chat_tx: Option<&mpsc::Sender<ChatCommand>>) {
    let Some(ct) = chat_tx else {
        println!("Chat is not enabled.");
        return;
    };
    let (resp_tx, resp_rx) = oneshot::channel();
    if ct
        .blocking_send(ChatCommand::RunReflection { respond_to: resp_tx })
        .is_err()
    {
        println!("Chat task is not running.");
        return;
    }
    match resp_rx.blocking_recv() {
        Ok(Ok(o)) => println!(
            "Reflection: admitted {} lessons; rejected (substring={} triggers={} senders={} trust={}); haiku tokens in={} out={}",
            o.admitted.len(),
            o.rejected_substring,
            o.rejected_distinct_triggers,
            o.rejected_distinct_senders,
            o.rejected_low_trust,
            o.haiku_input_tokens,
            o.haiku_output_tokens,
        ),
        Ok(Err(e)) => println!("Reflection failed: {e}"),
        Err(_) => println!("Chat task did not respond."),
    }
}

fn chat_run_sweep(chat_tx: Option<&mpsc::Sender<ChatCommand>>) {
    let Some(ct) = chat_tx else {
        println!("Chat is not enabled.");
        return;
    };
    let (resp_tx, resp_rx) = oneshot::channel();
    if ct
        .blocking_send(ChatCommand::RunRetentionSweep { respond_to: resp_tx })
        .is_err()
    {
        println!("Chat task is not running.");
        return;
    }
    match resp_rx.blocking_recv() {
        Ok(r) => println!(
            "Retention sweep deleted {} files (history={} decisions={} overlays={} pending_adj={} pending_self={} persona_archives={} markdown_archives={}).",
            r.total(),
            r.history_deleted,
            r.decisions_deleted,
            r.overlays_deleted,
            r.pending_adjustments_deleted,
            r.pending_self_memory_deleted,
            r.persona_archives_deleted,
            r.markdown_archives_deleted,
        ),
        Err(_) => println!("Chat task did not respond."),
    }
}

/// Sends a QueryBalances request and displays the results.
fn get_balances(store_tx: &mpsc::Sender<StoreMessage>) {
    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::QueryBalances {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] QueryBalances send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(balances) => {
            if balances.is_empty() {
                println!("No users found.");
            } else {
                println!("\n=== User Balances ===");
                for user in balances {
                    println!(
                        "User: {}, Balance: {} diamonds",
                        user.username, user.balance
                    );
                }
                println!("====================\n");
            }
        }
        Err(_) => {
            println!("Failed to receive balances.");
            error!("[CLI] QueryBalances response channel closed without reply");
        }
    }
}

/// Sends a QueryPairs request and displays the results, including
/// AMM-style buy/sell prices derived from each pair's current reserves.
fn get_pairs(store_tx: &mpsc::Sender<StoreMessage>) {
    // Fall back to the default configured fee on query failure so the operator
    // still sees a price estimate. A non-default fee would make the displayed
    // prices materially wrong, so the fallback path must warn.
    const DEFAULT_FEE_FALLBACK: f64 = 0.125;
    let (fee_tx, fee_rx) = oneshot::channel();
    let fee_msg = StoreMessage::FromCli(CliMessage::QueryFee {
        respond_to: fee_tx,
    });

    let fee = if store_tx.blocking_send(fee_msg).is_ok() {
        match fee_rx.blocking_recv() {
            Ok(f) => f,
            Err(_) => {
                warn!("[CLI] QueryFee response failed, displaying prices with fallback fee {}", DEFAULT_FEE_FALLBACK);
                DEFAULT_FEE_FALLBACK
            }
        }
    } else {
        warn!("[CLI] QueryFee send failed, displaying prices with fallback fee {}", DEFAULT_FEE_FALLBACK);
        DEFAULT_FEE_FALLBACK
    };

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::QueryPairs {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] QueryPairs send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(pairs) => {
            if pairs.is_empty() {
                println!("No pairs found.");
            } else {
                println!("\n=== Pairs ===");
                for pair in pairs {
                    let (price_buy, price_sell) = quote_prices(pair.item_stock, pair.currency_stock, fee);
                    println!(
                        "Item: {}, Stock: {}, Currency: {:.2}",
                        pair.item, pair.item_stock, pair.currency_stock
                    );
                    if let Some(pb) = price_buy {
                        println!("  Buy price: {:.2} diamonds/item", pb);
                    }
                    if let Some(ps) = price_sell {
                        println!("  Sell price: {:.2} diamonds/item", ps);
                    }
                }
                println!("====================\n");
            }
        }
        Err(_) => {
            println!("Failed to receive pairs.");
            error!("[CLI] QueryPairs response channel closed without reply");
        }
    }
}

/// Prompts for username/UUID and operator status, then sends a SetOperator request.
fn set_operator(store_tx: &mpsc::Sender<StoreMessage>) {
    let username_or_uuid: String = with_retry("Failed to read username/UUID", || {
        Input::new()
            .with_prompt("Enter username or UUID")
            .interact_text()
    });

    // Default to "false" (index 0) so accidentally pressing Enter never
    // grants operator privileges by mistake.
    let is_operator: bool = with_retry("Failed to read selection", || {
        Select::new()
            .with_prompt("Set operator status")
            .items(["false", "true"])
            .default(0)
            .interact()
    }) == 1;

    info!("[CLI] Setting operator status for {} to {}", username_or_uuid, is_operator);

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::SetOperator {
        username_or_uuid: username_or_uuid.clone(),
        is_operator,
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] SetOperator send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(())) => println!("Operator status updated successfully."),
        Ok(Err(e)) => {
            println!("Failed to update operator status: {}", e);
            error!("[CLI] SetOperator for {username_or_uuid} failed: {e}");
        }
        Err(_) => error!("[CLI] SetOperator response channel closed without reply"),
    }
}

/// Sends an AddNode request (without physical validation).
fn add_node(store_tx: &mpsc::Sender<StoreMessage>) {
    println!("Note: This adds the node WITHOUT verifying it exists in-world.");
    println!("Use 'Add node (with bot validation)' for physical verification.");

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::AddNode {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] AddNode send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(node_id)) => println!("Node {} added successfully.", node_id),
        Ok(Err(e)) => {
            println!("Failed to add node: {}", e);
            error!("[CLI] AddNode failed: {e}");
        }
        Err(_) => error!("[CLI] AddNode response channel closed without reply"),
    }
}

/// Sends an AddNodeWithValidation request (with bot-based physical validation).
fn add_node_with_validation(store_tx: &mpsc::Sender<StoreMessage>) {
    println!("Bot will navigate to the calculated position and verify:");
    println!("  1. All 4 chests exist and can be opened");
    println!("  2. Each chest slot contains a shulker box");
    println!("This may take up to 2 minutes. Please wait...");

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::AddNodeWithValidation {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] AddNodeWithValidation send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(node_id)) => println!("Node {} validated and added successfully!", node_id),
        Ok(Err(e)) => {
            println!("Failed to add node: {}", e);
            error!("[CLI] AddNodeWithValidation failed: {e}");
        }
        Err(_) => error!("[CLI] AddNodeWithValidation response channel closed without reply"),
    }
}

/// Discovers existing storage nodes by having the bot physically scan positions.
fn discover_storage(store_tx: &mpsc::Sender<StoreMessage>) {
    println!("Bot will scan for existing storage nodes starting from position 0.");
    println!("For each position, the bot will:");
    println!("  1. Navigate to the calculated node position");
    println!("  2. Check if all 4 chests exist and contain shulker boxes");
    println!("  3. Add valid nodes to storage");
    println!("Discovery stops when a position without valid chests is found.");
    println!("This may take several minutes. Please wait...");

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::DiscoverStorage {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] DiscoverStorage send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(count)) => println!("Storage discovery complete! {} nodes discovered.", count),
        Ok(Err(e)) => {
            println!("Storage discovery failed: {}", e);
            error!("[CLI] DiscoverStorage failed: {e}");
        }
        Err(_) => error!("[CLI] DiscoverStorage response channel closed without reply"),
    }
}

/// Prompts for node ID, then sends a RemoveNode request.
fn remove_node(store_tx: &mpsc::Sender<StoreMessage>) {
    let node_id: i32 = with_retry("Failed to read node ID", || {
        Input::new()
            .with_prompt("Enter node ID to remove")
            .interact_text()
    });

    let confirmed = with_retry("Failed to read confirmation", || {
        Confirm::new()
            .with_prompt(format!("Really remove node {}? This deletes data/storage/{}.json from disk.", node_id, node_id))
            .default(false)
            .interact()
    });
    if !confirmed {
        println!("Cancelled.");
        return;
    }

    info!("[CLI] Requesting to remove node {}", node_id);

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::RemoveNode {
        node_id,
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] RemoveNode send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(())) => println!("Node {} removed successfully.", node_id),
        Ok(Err(e)) => {
            println!("Failed to remove node: {}", e);
            error!("[CLI] RemoveNode {node_id} failed: {e}");
        }
        Err(_) => error!("[CLI] RemoveNode response channel closed without reply"),
    }
}

/// Prompts for item name and stack size, then sends an AddPair request.
fn add_pair(store_tx: &mpsc::Sender<StoreMessage>) {
    let item_name: String = with_retry("Failed to read item name", || {
        Input::new()
            .with_prompt("Enter item name (without minecraft: prefix)")
            .interact_text()
    });

    // Stack size must match Minecraft's hard-coded per-item limit, otherwise
    // the bot's storage math (shulker box layouts, chest capacity) will be
    // off. We expose the three valid values rather than a free-form number
    // so operators can't enter an illegal stack size like 32.
    let stack_size_selection = with_retry("Failed to read stack size selection", || {
        Select::new()
            .with_prompt("Select stack size")
            .items(["64 (most items)", "16 (ender pearls, eggs, signs, buckets)", "1 (tools, weapons, armor)"])
            .default(0)
            .interact()
    });

    let stack_size = match stack_size_selection {
        0 => 64,
        1 => 16,
        2 => 1,
        _ => unreachable!("Select bounded by items() above"),
    };

    info!("[CLI] Requesting to add pair for {} with stack size {}", item_name, stack_size);

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::AddPair {
        item_name: item_name.clone(),
        stack_size,
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] AddPair send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(())) => println!("Pair '{}' added successfully (stack size: {}, stocks set to zero).", item_name, stack_size),
        Ok(Err(e)) => {
            println!("Failed to add pair: {}", e);
            error!("[CLI] AddPair for {item_name} (stack {stack_size}) failed: {e}");
        }
        Err(_) => error!("[CLI] AddPair response channel closed without reply"),
    }
}

/// Prompts for item name, then sends a RemovePair request.
fn remove_pair(store_tx: &mpsc::Sender<StoreMessage>) {
    let item_name: String = with_retry("Failed to read item name", || {
        Input::new()
            .with_prompt("Enter item name to remove")
            .interact_text()
    });

    let confirmed = with_retry("Failed to read confirmation", || {
        Confirm::new()
            .with_prompt(format!("Really remove pair '{}'?", item_name))
            .default(false)
            .interact()
    });
    if !confirmed {
        println!("Cancelled.");
        return;
    }

    info!("[CLI] Requesting to remove pair for {}", item_name);

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::RemovePair {
        item_name: item_name.clone(),
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] RemovePair send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(())) => println!("Pair '{}' removed successfully.", item_name),
        Ok(Err(e)) => {
            println!("Failed to remove pair: {}", e);
            error!("[CLI] RemovePair for {item_name} failed: {e}");
        }
        Err(_) => error!("[CLI] RemovePair response channel closed without reply"),
    }
}

/// Sends a QueryStorage request and displays the storage state.
fn view_storage(store_tx: &mpsc::Sender<StoreMessage>) {
    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::QueryStorage {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] QueryStorage send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(storage) => {
            println!("\n=== Storage State ===");
            println!("Origin: ({}, {}, {})", storage.position.x, storage.position.y, storage.position.z);
            println!("Total nodes: {}", storage.nodes.len());
            println!();
            
            if storage.nodes.is_empty() {
                println!("No nodes configured.");
            } else {
                for node in &storage.nodes {
                    println!("--- Node {} ---", node.id);
                    println!("  Position: ({}, {}, {})", node.position.x, node.position.y, node.position.z);
                    println!("  Chests:");
                    for chest in &node.chests {
                        let total_items: i32 = chest.amounts.iter().sum();
                        let item_display = if chest.item.is_empty() { "(empty)" } else { &chest.item };
                        println!("    Chest {}: {} - {} items total", chest.id, item_display, total_items);
                    }
                    println!();
                }
            }
            println!("====================\n");
        }
        Err(_) => error!("[CLI] QueryStorage response channel closed without reply"),
    }
}

/// Sends a QueryTrades request and displays recent trades.
fn view_trades(store_tx: &mpsc::Sender<StoreMessage>) {
    let limit: usize = with_retry("Failed to read limit", || {
        Input::new()
            .with_prompt("How many recent trades to show")
            .default(20)
            .interact_text()
    });

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::QueryTrades {
        limit,
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] QueryTrades send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(trades) => {
            if trades.is_empty() {
                println!("No trades found.");
            } else {
                println!("\n=== Recent Trades ({} shown) ===", trades.len());
                for trade in trades {
                    let trade_type = match trade.trade_type {
                        TradeType::Buy => "BUY",
                        TradeType::Sell => "SELL",
                        TradeType::AddStock => "ADD_STOCK",
                        TradeType::RemoveStock => "REMOVE_STOCK",
                        TradeType::DepositBalance => "DEPOSIT",
                        TradeType::WithdrawBalance => "WITHDRAW",
                        TradeType::AddCurrency => "ADD_CURRENCY",
                        TradeType::RemoveCurrency => "REMOVE_CURRENCY",
                    };
                    println!(
                        "[{}] {} - {}x {} for {:.2} diamonds (user: {})",
                        trade.timestamp.format("%Y-%m-%d %H:%M:%S"),
                        trade_type,
                        trade.amount,
                        trade.item,
                        trade.amount_currency,
                        trade.user_uuid
                    );
                }
                println!("====================\n");
            }
        }
        Err(_) => error!("[CLI] QueryTrades response channel closed without reply"),
    }
}

/// Sends a RestartBot request.
fn restart_bot(store_tx: &mpsc::Sender<StoreMessage>) {
    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::RestartBot {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] RestartBot send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(())) => println!("Bot restart initiated successfully."),
        Ok(Err(e)) => {
            println!("Failed to restart Bot: {}", e);
            error!("[CLI] RestartBot failed: {e}");
        }
        Err(_) => error!("[CLI] RestartBot response channel closed without reply"),
    }
}

/// Clears stuck order processing state, allowing the queue to continue.
fn clear_stuck_order(store_tx: &mpsc::Sender<StoreMessage>) {
    println!("This will clear any stuck order processing state.");
    println!("Use this if an order got stuck and the queue isn't advancing.");

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::ClearStuckOrder {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] ClearStuckOrder send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Some(stuck_order)) => {
            println!("Cleared stuck order: {}", stuck_order);
            println!("Queue will now continue processing remaining orders.");
        }
        Ok(None) => println!("No stuck order was detected (processing was not blocked)."),
        Err(_) => error!("[CLI] ClearStuckOrder response channel closed without reply"),
    }
}

/// Sends an AuditState request and displays any invariant violations found.
/// If `repair` is true, also applies safe automatic repairs (e.g. recomputing pair stock).
fn audit_state(store_tx: &mpsc::Sender<StoreMessage>, repair: bool) {
    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::AuditState { repair, respond_to: response_tx });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] AuditState send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(lines) => {
            if lines.is_empty() {
                println!("Audit OK (no issues found).");
            } else {
                println!("\n=== Audit Report ===");
                for line in lines {
                    println!("- {}", line);
                }
                println!("====================\n");
            }
        }
        Err(_) => {
            println!("Failed to receive audit response.");
            error!("[CLI] AuditState response channel closed without reply");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_prices_returns_none_when_item_stock_is_zero() {
        assert_eq!(quote_prices(0, 100.0, 0.125), (None, None));
    }

    #[test]
    fn quote_prices_returns_none_when_currency_stock_is_zero() {
        assert_eq!(quote_prices(10, 0.0, 0.125), (None, None));
    }

    #[test]
    fn quote_prices_returns_none_when_currency_stock_is_negative() {
        // Defensive: constant-product is undefined for non-positive reserves.
        assert_eq!(quote_prices(10, -1.0, 0.125), (None, None));
    }

    #[test]
    fn quote_prices_applies_fee_symmetrically_around_mid() {
        // item_stock=10, currency_stock=100 -> mid = 10.0, fee = 0.125
        // buy  = 10.0 * 1.125 = 11.25
        // sell = 10.0 * 0.875 =  8.75
        let (buy, sell) = quote_prices(10, 100.0, 0.125);
        assert!((buy.unwrap() - 11.25).abs() < 1e-9);
        assert!((sell.unwrap() - 8.75).abs() < 1e-9);
    }

    #[test]
    fn quote_prices_with_zero_fee_collapses_buy_and_sell_to_mid() {
        let (buy, sell) = quote_prices(4, 8.0, 0.0);
        assert_eq!(buy, sell);
        assert!((buy.unwrap() - 2.0).abs() < 1e-9);
    }
}
