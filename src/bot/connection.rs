//! Connection management for the bot

use azalea::account::Account;
use tracing::{debug, info};
use std::sync::atomic::Ordering;
use std::time::Instant;
use super::{Bot, BotState, handle_event_fn};

/// Connect the bot to the server
pub async fn connect(
    bot: &Bot,
    account: Account,
    server_address: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Check if shutdown was requested
    if bot.shutdown.load(Ordering::SeqCst) {
        return Err("Shutdown requested, cannot connect".into());
    }

    // Prevent spawning multiple concurrent client start tasks.
    if bot.connecting.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    info!("Connecting to server: {}", server_address);

    // Abort any existing client task
    if let Some(old_task) = bot.client_task.lock().await.take() {
        old_task.abort();
    }

    // Create initial state with our communication channels
    let initial_state = BotState {
        connected: false,
        store_tx: Some(bot.store_tx.clone()),
        client: bot.client.clone(),
        chat_tx: bot.chat_tx.clone(),
        connecting: bot.connecting.clone(),
    };

    let account = account.clone();
    let server_address = server_address.clone();
    let shutdown = bot.shutdown.clone();

    // In azalea 0.15+, ClientBuilder::start runs on a LocalSet (not Send).
    // Spawn it locally so we can continue processing BotInstruction messages.
    // NOTE: This requires the caller to be running inside a tokio LocalSet
    // (see main.rs). Using tokio::spawn instead would fail to compile because
    // the future returned by start() is !Send.
    let handle = tokio::task::spawn_local(async move {
        // Check shutdown flag before starting
        if shutdown.load(Ordering::SeqCst) {
            return;
        }
        
        // Use ClientBuilder::new() - it will try to set up LogPlugin which conflicts with our tracing setup
        // This causes a harmless error message: "Could not set global logger and tracing subscriber as they are already set"
        // The error is safe to ignore - our logging setup takes precedence and works correctly
        // To properly fix this, we would need to add bevy as a dependency and use:
        //   ClientBuilder::new_without_plugins()
        //       .add_plugins(DefaultPlugins.build().disable::<LogPlugin>())
        //       .add_plugins(DefaultBotPlugins)
        // But that adds significant dependencies, so we accept the harmless error instead.
        // The `let _ =` intentionally discards the Result: the only failure path here is
        // bevy's LogPlugin double-initialization error described above, which is benign
        // and must not abort the connect task.
        let _ = azalea::ClientBuilder::new()
            .set_handler(handle_event_fn)
            .set_state(initial_state)
            .start(account, server_address)
            .await;
    });

    // Store the task handle
    *bot.client_task.lock().await = Some(handle);

    info!("Bot connect task spawned");
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
/// **Timing**: The function waits approximately 4 seconds total (2s for disconnect packet + 2s after abort)
/// to ensure the server sees the disconnect immediately and the bot doesn't linger on the server.
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
pub async fn disconnect(bot: &Bot, shutdown: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
    
    // Give the disconnect packet time to be sent and processed.
    // This is the FIRST of two ~2s waits. It happens BEFORE aborting the task so
    // that Azalea's event loop is still alive to actually flush the disconnect
    // packet out of its send buffer; aborting too early would drop the packet
    // and the server would only notice us via a keep-alive timeout.
    // IMPORTANT: We need to wait long enough for:
    // 1. The disconnect packet to be sent over the network
    // 2. The server to receive and process it
    // 3. The TCP connection to be closed by both sides
    // 4. The OS to release the socket
    // Wait for the disconnect packet to be sent before aborting the task.
    // Azalea's event loop must still be alive to flush the packet; aborting too
    // early would drop it and the server would only notice via keep-alive timeout.
    if had_client {
        let mut elapsed = tokio::time::Duration::from_millis(0);
        let check_interval = tokio::time::Duration::from_millis(100);
        let max_wait = tokio::time::Duration::from_millis(2000);

        while elapsed < max_wait {
            tokio::time::sleep(check_interval).await;
            elapsed += check_interval;
            if bot.client.read().await.is_none() {
                debug!("[Connection] Client cleared after {:?}", elapsed);
                break;
            }
        }
    }
    
    // Abort the Azalea client task, then wait for OS-level TCP teardown.
    // task.abort() is async — the Drop chain (which closes the socket) runs after
    // the task's current await point, so a fast reconnect could race the old socket.
    let task_aborted = {
        let mut task_guard = bot.client_task.lock().await;
        if let Some(task) = task_guard.take() {
            task.abort();
            tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
            true
        } else {
            false
        }
    };

    // Clear the client reference
    bot.client.write().await.take();

    info!("[Connection] Disconnect complete in {:?} (had_client={}, task_aborted={})",
          disconnect_start.elapsed(), had_client, task_aborted);
    Ok(())
}
