//! # cj-store - Minecraft Store Bot
//!
//! Main entry point for the store bot application. Spawns three cooperating tasks:
//! - **Store**: Authoritative state management and persistence
//! - **Bot**: Minecraft client I/O via Azalea
//! - **CLI**: Interactive operator interface
//!
//! See `README.md` for architecture overview and usage.

use crate::cli::cli_task;
use crate::messages::{BotInstruction, StoreMessage};
use crate::store::Store;
use tokio::sync::mpsc;
use tokio::task::LocalSet;
use tracing::{error, info};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

mod bot;
mod cli;
mod config;
mod constants;
mod error;
mod fsutil;
mod messages;
mod store;
mod types;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging: file-only output to `data/logs/store.log`
    // This avoids cluttering stdout/stderr and provides persistent logs.
    // See README.md "Logging" section for details.
    let file_appender = RollingFileAppender::new(Rotation::NEVER, "data/logs", "store.log");

    // Configure tracing with ONLY file output (no stdout/stderr)
    let file_layer = fmt::layer()
        .with_writer(file_appender)
        .with_ansi(false) // No color codes in file
        .with_target(false);

    // Idempotent initialization: ignore "already set" errors from dependencies
    // Default log levels:
    // - cj_store (this crate): info level
    // - Other crates: info level
    // Override with RUST_LOG env var: e.g., RUST_LOG=debug or RUST_LOG=cj_store=trace
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info").add_directive("cj_store=info".parse().unwrap()));

    tracing_subscriber::registry()
        .with(file_layer)
        .with(env_filter)
        .try_init()
        .ok(); // Some deps may initialize logging; ignore "already set"

    println!("🚀 Starting bot application...");
    println!("📋 To view logs in another terminal, run:");
    println!("   PowerShell: Get-Content data\\logs\\store.log -Wait -Tail 20");
    println!("   Or install WSL/Git Bash: tail -f data/logs/store.log");
    println!("📝 All application logs will be written to data/logs/store.log only");
    println!("📊 Log level: info (override with RUST_LOG=trace for more detail)");
    println!();

    // Azalea's bot runtime uses !Send tasks internally (LocalSet requirement).
    // We wrap everything in a LocalSet to allow spawning local tasks.
    // See: https://docs.rs/tokio/latest/tokio/task/struct.LocalSet.html
    let local = LocalSet::new();
    let result = local
        .run_until(async move {
            // Communication channels between tasks:
            // - Store <-> Bot: BotInstruction (Store -> Bot), BotMessage (Bot -> Store)
            // - Store <-> CLI: CliMessage (CLI -> Store), responses via oneshot channels
            //
            // Buffer size of 128 chosen as a pragmatic middle ground: large enough to
            // absorb bursts (e.g. many whispers during a raid/event) without blocking
            // senders, but small enough to apply backpressure if the Store falls behind.
            let (store_tx, store_rx) = mpsc::channel::<StoreMessage>(128);
            let (bot_tx, bot_rx) = mpsc::channel::<BotInstruction>(128);

            // Create Store instance: loads all persistent state (users, pairs, orders, trades, storage)
            // See `Store::new()` for initialization details.
            let store = Store::new(bot_tx.clone()).await?;

            // Spawn Store task (authoritative source of truth for all store data)
            let store_handle = tokio::spawn(store.run(store_rx, bot_tx.clone()));

            // Load config for bot creation
            let config = crate::config::Config::load()?;

            // Spawn Bot task (local due to Azalea's !Send requirements)
            let bot_handle = tokio::task::spawn_local(crate::bot::bot_task(
                store_tx.clone(),
                bot_rx,
                config.account_email,
                config.server_address,
                config.buffer_chest_position,
                config.trade_timeout_ms,
                config.pathfinding_timeout_ms,
            ));

            // Spawn CLI task (blocking I/O for interactive menu)
            let cli_handle = tokio::task::spawn_blocking(move || cli_task(store_tx));

            info!("[Main] All tasks spawned");
            let join_result = tokio::try_join!(store_handle, bot_handle, cli_handle);
            Ok::<_, Box<dyn std::error::Error>>(join_result)
        })
        .await;

    match result {
        Ok(Ok(_)) => {
            info!("[Main] All tasks completed");
            println!("✅ Application shutdown complete");
        }
        Ok(Err(e)) => {
            error!("[Main] Main loop error: {}", e);
            eprintln!("❌ Error during runtime: {}", e);
        }
        Err(e) => {
            error!("[Main] LocalSet join error: {}", e);
            eprintln!("❌ Error during runtime: {}", e);
        }
    }

    // Brief yield so the tracing file appender can flush final log lines
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    Ok(())
}
