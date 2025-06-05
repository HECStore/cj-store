use crate::bot::bot_task;
use crate::cli::cli_task;
use crate::messages::{BotInstruction, StoreMessage};
use crate::store::Store;
use tokio::sync::mpsc;
use tracing::{error, info};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

mod bot;
mod cli;
mod config;
mod messages;
mod store;
mod types;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a rolling file appender that creates a new log file daily
    let file_appender = RollingFileAppender::new(Rotation::NEVER, "data/logs", "store.log");

    // Configure tracing with ONLY file output (no stdout/stderr)
    let file_layer = fmt::layer()
        .with_writer(file_appender)
        .with_ansi(false) // No color codes in file
        .with_target(false);

    tracing_subscriber::registry()
        .with(file_layer)
        .with(EnvFilter::from_default_env().add_directive("info".parse().unwrap()))
        .init();

    println!("🚀 Starting bot application...");
    println!("📋 To view logs in another terminal, run:");
    println!("   PowerShell: Get-Content store.log -Wait -Tail 20");
    println!("   Or install WSL/Git Bash: tail -f store.log");
    println!("📝 All application logs will be written to store.log only");
    println!();

    // Channels for communication
    let (store_tx, store_rx) = mpsc::channel::<StoreMessage>(128);
    let (bot_tx, bot_rx) = mpsc::channel::<BotInstruction>(128);

    // Create Store instance (no logger needed with direct tracing)
    let store = Store::new(bot_tx.clone()).await?;

    // Spawn Store task
    let store_handle = tokio::spawn(store.run(store_rx, bot_tx.clone()));

    // Load config for bot creation
    let config = crate::config::Config::load()?;

    // Spawn Bot task (no logger needed with direct tracing)
    let bot_handle = tokio::spawn(bot_task(
        store_tx.clone(),
        bot_rx,
        config.account_email,
        config.server_address,
    ));

    // Spawn CLI task (no logger needed with direct tracing)
    let cli_handle = tokio::task::spawn_blocking(move || cli_task(store_tx));

    // Wait for tasks to complete
    let result = tokio::try_join!(store_handle, bot_handle, cli_handle);

    match result {
        Ok(_) => {
            info!("All tasks completed successfully");
            println!("✅ Application shutdown complete");
        }
        Err(e) => {
            error!("Task error during shutdown: {}", e);
            eprintln!("❌ Error during shutdown: {}", e);
        }
    }

    // Give a moment for any final logging to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    Ok(())
}
