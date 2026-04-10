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
            let (store_tx, store_rx) = mpsc::channel::<StoreMessage>(128);
            let (bot_tx, bot_rx) = mpsc::channel::<BotInstruction>(128);

            // Create Store instance: loads all persistent state (users, pairs, orders, trades, storage)
            // See `Store::new()` for initialization details.
            let store = Store::new(bot_tx.clone()).await?;

            // Spawn Store task: handles all state mutations and persistence
            // This is the authoritative source of truth for all store data.
            info!("[Main] Spawning Store task");
            let store_handle = tokio::spawn(store.run(store_rx, bot_tx.clone()));
            info!("[Main] Store task spawned");

            // Load config for bot creation (account email, server address, storage position)
            info!("[Main] Loading config for bot");
            let config = crate::config::Config::load()?;
            info!("[Main] Config loaded");

            // Spawn Bot task (must be local due to Azalea's !Send requirements)
            // Handles Minecraft client connection, whisper parsing, trade automation, chest I/O
            info!("[Main] Spawning Bot task (local)");
            let bot_handle = tokio::task::spawn_local(crate::bot::bot_task(
                store_tx.clone(),
                bot_rx,
                config.account_email,
                config.server_address,
                config.buffer_chest_position,
            ));
            info!("[Main] Bot task spawned");

            // Spawn CLI task (blocking I/O for interactive menu)
            // Provides operator interface for managing store state
            info!("[Main] Spawning CLI task (blocking)");
            let cli_handle = tokio::task::spawn_blocking(move || cli_task(store_tx));
            info!("[Main] CLI task spawned");

            // Wait for tasks to complete
            info!("[Main] All tasks spawned, waiting for completion");
            let join_result = tokio::try_join!(store_handle, bot_handle, cli_handle);
            info!("[Main] All tasks completed, join result received");
            Ok::<_, Box<dyn std::error::Error>>(join_result)
        })
        .await;

    info!("[Main] Processing task join results");
    match result {
        Ok(Ok((store_result, bot_result, cli_result))) => {
            info!("[Main] All tasks completed successfully");
            info!("[Main] Store task result: {:?}", store_result);
            info!("[Main] Bot task result: {:?}", bot_result);
            info!("[Main] CLI task result: {:?}", cli_result);
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

    // Give a moment for any final logging to complete
    info!("[Main] Waiting 100ms for final logging to complete");
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    info!("[Main] Final wait complete, main() returning");

    Ok(())
}
