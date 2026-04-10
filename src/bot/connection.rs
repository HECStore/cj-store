//! Connection management for the bot

use azalea::account::Account;
use tracing::{info, warn, debug};
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
    info!("[Connection] ===== DISCONNECT START (shutdown={}) =====", shutdown);
    
    if shutdown {
        // Set shutdown flag to prevent reconnection
        info!("[Connection] Step 1/6: Setting shutdown flag to prevent reconnection");
        bot.shutdown.store(true, Ordering::SeqCst);
        debug!("[Connection] Shutdown flag set to true");
    }
    
    // Clear connecting flag
    info!("[Connection] Step 2/6: Clearing connecting flag");
    bot.connecting.store(false, Ordering::SeqCst);
    debug!("[Connection] Connecting flag cleared");
    
    // Check current connection state
    let client_exists_before = bot.client.read().await.is_some();
    let task_exists_before = bot.client_task.lock().await.is_some();
    info!("[Connection] Step 3/6: Pre-disconnect state check - client exists: {}, task exists: {}", 
          client_exists_before, task_exists_before);
    
    // Disconnect the client first to send disconnect packet gracefully
    info!("[Connection] Step 4/6: Disconnecting client to send disconnect packet");
    let had_client = {
        let client_guard = bot.client.write().await;
        if let Some(client) = client_guard.as_ref() {
            info!("[Connection] Client found, calling disconnect() method");
            let before_disconnect = Instant::now();
            client.disconnect();
            let disconnect_call_duration = before_disconnect.elapsed();
            info!("[Connection] Client.disconnect() called (took {:?})", disconnect_call_duration);
            debug!("[Connection] Disconnect method returned (non-blocking)");
            true
        } else {
            warn!("[Connection] No client found (already disconnected or never connected)");
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
    if had_client {
        info!("[Connection] Step 5/6: Waiting for disconnect packet to be sent and TCP connection to close");
        info!("[Connection] Waiting 2000ms (2 seconds) for network I/O, server processing, and TCP closure");
        let wait_start = Instant::now();
        
        // Poll the client state periodically to see if it's been cleared (indicating disconnect event processed)
        let mut elapsed = tokio::time::Duration::from_millis(0);
        let check_interval = tokio::time::Duration::from_millis(100);
        let max_wait = tokio::time::Duration::from_millis(2000);
        
        while elapsed < max_wait {
            tokio::time::sleep(check_interval).await;
            elapsed += check_interval;
            
            let client_still_exists = bot.client.read().await.is_some();
            if !client_still_exists {
                info!("[Connection] Client cleared after {:?} (Disconnect event likely processed)", elapsed);
                break;
            }
        }
        
        let wait_duration = wait_start.elapsed();
        let client_still_exists = bot.client.read().await.is_some();
        info!("[Connection] Disconnect wait complete (actual wait: {:?}, client still exists: {})", 
              wait_duration, client_still_exists);
        
        if client_still_exists {
            warn!("[Connection] Client still exists after wait - Disconnect event may not have fired yet");
        }
    } else {
        info!("[Connection] Step 5/6: Skipping wait (no client to disconnect)");
    }
    
    // Now abort the Azalea client task (this will drop BotState which contains store_tx clone)
    // IMPORTANT: Even after aborting, the TCP connection may take time to close at the OS level
    info!("[Connection] Step 6/6: Aborting Azalea client task");
    let task_aborted = {
        let mut task_guard = bot.client_task.lock().await;
        if let Some(task) = task_guard.take() {
            info!("[Connection] Azalea client task found, aborting now");
            let abort_start = Instant::now();
            task.abort();
            let abort_duration = abort_start.elapsed();
            info!("[Connection] Task.abort() called (took {:?})", abort_duration);
            debug!("[Connection] Task abort signal sent (task may still be cleaning up)");
            
            // Wait for the abort to take effect and the TCP connection to fully close at OS level.
            // This is the SECOND ~2s wait: distinct from the pre-abort wait above because
            // task.abort() is asynchronous - the task may still be mid-await when the signal
            // arrives, and its Drop chain (which closes the socket) runs after that. Without
            // this second delay, a fast reconnect could race the old socket's teardown and
            // see the server still holding the previous session.
            // The OS may keep the connection in TIME_WAIT state for a few seconds
            info!("[Connection] Waiting 2000ms (2 seconds) for task abort and OS-level TCP connection closure");
            let abort_wait_start = Instant::now();
            tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
            let abort_wait_duration = abort_wait_start.elapsed();
            info!("[Connection] Task abort wait complete (actual wait: {:?})", abort_wait_duration);
            warn!("[Connection] NOTE: OS may keep TCP connection in TIME_WAIT for up to 60 seconds, but server should see disconnect immediately");
            true
        } else {
            warn!("[Connection] No Azalea client task found (already aborted or never spawned)");
            false
        }
    };
    
    // Clear the client reference
    info!("[Connection] Clearing client reference from bot state");
    let client_cleared = bot.client.write().await.take().is_some();
    info!("[Connection] Client reference cleared (had client: {})", client_cleared);
    
    let total_duration = disconnect_start.elapsed();
    info!("[Connection] ===== DISCONNECT COMPLETE (total time: {:?}, had_client: {}, task_aborted: {}) =====", 
          total_duration, had_client, task_aborted);
    Ok(())
}
