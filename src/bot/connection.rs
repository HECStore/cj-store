//! Connection management for the bot

use super::{Bot, BotState, handle_event_fn};
use azalea::account::Account;
use std::sync::atomic::Ordering;
use std::time::Instant;
use tracing::{debug, info, warn};

/// Target Minecraft protocol version translated to by the ViaVersion plugin.
///
/// Azalea itself follows Mojang's latest release; this string tells ViaProxy
/// which older protocol to translate down to so the bot can connect to a
/// server that hasn't been bumped yet. Update this when the target server is
/// upgraded.
///
/// Visible to `Bot::new` (sibling module) so the one-shot `ViaVersionPlugin::start`
/// at construction time uses the same target version this module wires into
/// every reconnect's `ClientBuilder`.
pub(super) const VIA_TARGET_VERSION: &str = "1.21.10";

/// Connect the bot to the server
pub async fn connect(
    bot: &Bot,
    account: Account,
    server_address: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let account_name = account.username().to_string();

    if bot.shutdown.load(Ordering::SeqCst) {
        debug!(
            "[Connection] Connect skipped (shutdown flag set): account={} server={}",
            account_name, server_address
        );
        return Err("Shutdown requested, cannot connect".into());
    }

    // Prevent spawning multiple concurrent client start tasks.
    if bot.connecting.swap(true, Ordering::SeqCst) {
        debug!(
            "[Connection] Connect skipped (already connecting): account={} server={}",
            account_name, server_address
        );
        return Ok(());
    }

    info!(
        "[Connection] Connecting: account={} server={}",
        account_name, server_address
    );

    if let Some(old_task) = bot.client_task.lock().await.take() {
        debug!(
            "[Connection] Aborting previous client task before reconnect: account={}",
            account_name
        );
        old_task.abort();
        // abort() is asynchronous: the task continues running until its next
        // .await checks for cancellation. Wait (bounded) for it to fully
        // complete so its Azalea/Bevy ECS world drops before we spawn a new
        // ClientBuilder. Without this, fast reconnects can leave multiple
        // client instances alive simultaneously and inflate RSS by tens of MB
        // per cycle. The disconnect path achieves the same effect via a
        // post-abort sleep; the reconnect path previously did neither.
        let _ = tokio::time::timeout(
            tokio::time::Duration::from_millis(crate::constants::DELAY_DISCONNECT_MS),
            old_task,
        )
        .await;
    }

    // Create initial state with our communication channels
    let initial_state = BotState {
        connected: false,
        store_tx: Some(bot.store_tx.clone()),
        client: bot.client.clone(),
        chat_tx: bot.chat_tx.clone(),
        connecting: bot.connecting.clone(),
        chat_events_tx: bot.chat_events_tx.clone(),
        history_tx: bot.history_tx.clone(),
        bot_username: bot.bot_username.clone(),
        chat_config: bot.chat_config.clone(),
        history_drops: bot.history_drops.clone(),
    };

    let shutdown = bot.shutdown.clone();
    let task_account = account_name.clone();
    let task_server = server_address.clone();

    // Reuse the cached ViaProxy plugin instead of calling
    // `ViaVersionPlugin::start` again — that would spawn a fresh `java -jar
    // ViaProxy.jar` subprocess on every reconnect with no cleanup hook (see
    // the `via_plugin` field doc on `Bot`). Cloning is cheap and every clone
    // routes through the same ViaProxy instance via its shared `bind_addr`.
    let via_plugin = bot.via_plugin.clone();

    // Azalea's ClientBuilder::start returns !Send, so it must run on a LocalSet
    // (see main.rs). tokio::spawn would fail to compile.
    let handle = tokio::task::spawn_local(async move {
        if shutdown.load(Ordering::SeqCst) {
            return;
        }

        // ClientBuilder::new() tries to install bevy's LogPlugin, which clashes with our
        // tracing subscriber and emits a benign "Could not set global logger" error.
        // Switching to new_without_plugins() + custom plugin list would fix it but drags
        // in a direct bevy dep. start() returns AppExit; any real connection error is
        // reported separately via Event::Disconnect.
        let exit = azalea::ClientBuilder::new()
            .add_plugins(via_plugin)
            .set_handler(handle_event_fn)
            .set_state(initial_state)
            .start(account, task_server.clone())
            .await;
        debug!(
            "[Connection] ClientBuilder::start returned: account={} server={} exit={:?}",
            task_account, task_server, exit
        );
    });

    *bot.client_task.lock().await = Some(handle);

    debug!(
        "[Connection] Connect task spawned: account={} server={}",
        account_name, server_address
    );
    Ok(())
}

/// Disconnect the bot from the server gracefully.
///
/// This function ensures the bot disconnects cleanly from the Minecraft server by:
/// 1. Sending a disconnect packet via `client.disconnect()`
/// 2. Waiting for the disconnect packet to be sent and the Disconnect event to be processed
/// 3. Aborting the Azalea client task
/// 4. Waiting for OS-level TCP connection closure
///
/// **Timing**: Up to ~4 seconds total (up to `DELAY_DISCONNECT_MS` for the disconnect
/// packet to flush — exits early when the client clears — plus `DELAY_DISCONNECT_MS`
/// after abort for TCP teardown).
///
/// **Why the long waits?**
/// - Network I/O: The disconnect packet must be sent over the network
/// - Server processing: The server must receive and process the disconnect
/// - TCP closure: The TCP connection must be closed by both sides
/// - OS cleanup: The OS may keep the connection in TIME_WAIT state briefly
///
/// If `shutdown` is true, sets the shutdown flag to prevent automatic reconnection attempts.
///
/// See README.md "Graceful Shutdown" section for the complete shutdown sequence.
pub async fn disconnect(
    bot: &Bot,
    shutdown: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let disconnect_start = Instant::now();
    info!("[Connection] Disconnect starting (shutdown={})", shutdown);

    if shutdown {
        bot.shutdown.store(true, Ordering::SeqCst);
    }

    bot.connecting.store(false, Ordering::SeqCst);

    // Disconnect the client first to send disconnect packet gracefully
    let had_client = {
        let client_guard = bot.client.write().await;
        if let Some(client) = client_guard.as_ref() {
            client.disconnect();
            true
        } else {
            false
        }
    };

    // Wait for the disconnect packet to flush before aborting the task.
    // Azalea's event loop must still be alive to flush the packet out of its
    // send buffer; aborting too early would drop it and the server would only
    // notice us via a keep-alive timeout. Exits early once the client clears.
    if had_client {
        let mut elapsed = tokio::time::Duration::from_millis(0);
        let check_interval = tokio::time::Duration::from_millis(crate::constants::DELAY_SHORT_MS);
        let max_wait = tokio::time::Duration::from_millis(crate::constants::DELAY_DISCONNECT_MS);
        let mut cleared = false;

        while elapsed < max_wait {
            tokio::time::sleep(check_interval).await;
            elapsed += check_interval;
            if bot.client.read().await.is_none() {
                debug!("[Connection] Client cleared after {:?}", elapsed);
                cleared = true;
                break;
            }
        }
        if !cleared {
            warn!(
                "[Connection] Disconnect packet did not flush within {}ms; aborting task anyway",
                crate::constants::DELAY_DISCONNECT_MS
            );
        }
    }

    // Abort the Azalea client task, then wait for OS-level TCP teardown.
    // task.abort() is async — the Drop chain (which closes the socket) runs after
    // the task's current await point, so a fast reconnect could race the old socket.
    let task_aborted = {
        let mut task_guard = bot.client_task.lock().await;
        if let Some(task) = task_guard.take() {
            task.abort();
            tokio::time::sleep(tokio::time::Duration::from_millis(
                crate::constants::DELAY_DISCONNECT_MS,
            ))
            .await;
            true
        } else {
            false
        }
    };

    // Clear the client reference
    bot.client.write().await.take();

    info!(
        "[Connection] Disconnect complete in {:?} (had_client={}, task_aborted={})",
        disconnect_start.elapsed(),
        had_client,
        task_aborted
    );
    Ok(())
}
