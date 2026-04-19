//! # cj-store - Minecraft Store Bot
//!
//! Main entry point for the store bot application. Spawns three cooperating tasks:
//! - **Store**: Authoritative state management and persistence
//! - **Bot**: Minecraft client I/O via Azalea
//! - **CLI**: Interactive operator interface
//!
//! See `README.md` for architecture overview and usage.

// Rustdoc hygiene: fail the build on broken intra-doc links or malformed HTML.
// Enforced in main.rs so CI (`cargo doc --no-deps`) catches any regression.
#![deny(rustdoc::broken_intra_doc_links)]
#![deny(rustdoc::invalid_html_tags)]

use crate::cli::cli_task;
use crate::messages::{BotInstruction, StoreMessage};
use crate::store::Store;
use tokio::sync::mpsc;
use tokio::task::LocalSet;
use tracing::{error, info, warn};
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
    // CLI flag parsing — kept tiny on purpose (no clap dependency).
    // Supported:
    //   --validate-only / --dry-run : load + validate config, then exit.
    //   --help / -h                 : usage and exit.
    let args: Vec<String> = std::env::args().collect();
    for a in args.iter().skip(1) {
        match a.as_str() {
            "--validate-only" | "--dry-run" => return run_validate_only(),
            "--help" | "-h" => {
                print_usage();
                return Ok(());
            }
            other => {
                eprintln!("Unknown argument: {other}");
                print_usage();
                return Err(format!("unknown argument: {other}").into());
            }
        }
    }

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

            // Snapshot the config fields needed by bot_task before `store` is
            // moved into `run` — avoids a redundant second disk read of
            // data/config.json here.
            let account_email = store.config.account_email.clone();
            let server_address = store.config.server_address.clone();
            let buffer_chest_position = store.config.buffer_chest_position;
            let trade_timeout_ms = store.config.trade_timeout_ms;
            let pathfinding_timeout_ms = store.config.pathfinding_timeout_ms;

            // Spawn Store task (authoritative source of truth for all store data)
            let store_handle = tokio::spawn(store.run(store_rx, bot_tx.clone()));

            // Spawn Bot task (local due to Azalea's !Send requirements)
            let bot_handle = tokio::task::spawn_local(crate::bot::bot_task(
                store_tx.clone(),
                bot_rx,
                account_email,
                server_address,
                buffer_chest_position,
                trade_timeout_ms,
                pathfinding_timeout_ms,
            ));

            // Spawn config file watcher (hot-reload of `fee` and `autosave_interval_secs`).
            // Other config fields are cached at startup and logged as warnings
            // if edited — see `Store::reload_config`.
            spawn_config_watcher(store_tx.clone());

            // Spawn CLI task (blocking I/O for interactive menu)
            let cli_handle = tokio::task::spawn_blocking(move || cli_task(store_tx));

            info!("[Main] All tasks spawned");
            let join_result = tokio::try_join!(store_handle, bot_handle, cli_handle);
            Ok::<_, Box<dyn std::error::Error>>(join_result)
        })
        .await;

    // Track whether any task failed so we can exit non-zero after flushing
    // logs. Without this, systemd/CI see exit code 0 even when the bot
    // crashed.
    let had_error = match result {
        Ok(Ok(_)) => {
            info!("[Main] All tasks completed");
            println!("✅ Application shutdown complete");
            false
        }
        Ok(Err(e)) => {
            error!("[Main] Main loop error: {}", e);
            eprintln!("❌ Error during runtime: {}", e);
            true
        }
        Err(e) => {
            error!("[Main] LocalSet join error: {}", e);
            eprintln!("❌ Error during runtime: {}", e);
            true
        }
    };

    // Brief yield so the tracing file appender can flush final log lines
    tokio::time::sleep(tokio::time::Duration::from_millis(crate::constants::DELAY_SHORT_MS)).await;

    if had_error {
        std::process::exit(1);
    }
    Ok(())
}

/// Print CLI usage to stdout.
fn print_usage() {
    println!("cj-store — Minecraft store bot");
    println!();
    println!("USAGE:");
    println!("    cj-store [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("    --validate-only, --dry-run   Load and validate data/config.json, then exit");
    println!("                                 without connecting to the server");
    println!("    -h, --help                   Show this help");
}

/// Load config, run validation, print result, and exit without connecting.
///
/// Useful for CI checks or for operators to sanity-check a config edit before
/// restarting the bot. Exit code is 0 on success, 1 on validation error.
fn run_validate_only() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔍 Validating data/config.json ...");
    match crate::config::Config::load() {
        Ok(cfg) => {
            println!("✅ Config OK");
            println!("   position:            ({}, {}, {})", cfg.position.x, cfg.position.y, cfg.position.z);
            println!("   fee:                 {}", cfg.fee);
            println!("   server_address:      {}", cfg.server_address);
            println!(
                "   account_email:       {}",
                if cfg.account_email.is_empty() { "<empty>" } else { cfg.account_email.as_str() }
            );
            match cfg.buffer_chest_position {
                Some(p) => println!("   buffer_chest_position: ({}, {}, {})", p.x, p.y, p.z),
                None => println!("   buffer_chest_position: <none>"),
            }
            println!("   trade_timeout_ms:    {}", cfg.trade_timeout_ms);
            println!("   pathfinding_timeout_ms: {}", cfg.pathfinding_timeout_ms);
            println!("   max_orders:          {}", cfg.max_orders);
            println!("   max_trades_in_memory: {}", cfg.max_trades_in_memory);
            println!("   autosave_interval_secs: {}", cfg.autosave_interval_secs);
            Ok(())
        }
        Err(e) => {
            eprintln!("❌ Config invalid: {e}");
            Err(e.into())
        }
    }
}

/// Watch `data/config.json` and send `StoreMessage::ReloadConfig` to the
/// Store whenever it changes on disk. Events are debounced (~500 ms) because
/// editors typically produce a burst of writes on save (rename-over-old,
/// metadata touch, final write), and we only want one reload per user edit.
///
/// Validation failures keep the running config — a malformed edit is logged
/// but never crashes the bot.
fn spawn_config_watcher(store_tx: mpsc::Sender<StoreMessage>) {
    use notify::{EventKind, RecursiveMode, Watcher};
    use std::path::Path;
    use std::time::Duration;

    // Bridge the sync notify callback into tokio.
    let (event_tx, mut event_rx) = mpsc::channel::<notify::Result<notify::Event>>(16);

    tokio::spawn(async move {
        let mut watcher = match notify::recommended_watcher(move |res| {
            // blocking_send is fine: the callback runs on notify's own thread,
            // not inside the tokio runtime.
            let _ = event_tx.blocking_send(res);
        }) {
            Ok(w) => w,
            Err(e) => {
                warn!("[ConfigWatcher] Failed to create watcher, hot-reload disabled: {}", e);
                return;
            }
        };
        if let Err(e) = watcher.watch(Path::new("data/config.json"), RecursiveMode::NonRecursive) {
            warn!("[ConfigWatcher] Failed to watch data/config.json, hot-reload disabled: {}", e);
            return;
        }
        info!("[ConfigWatcher] Watching data/config.json for changes");

        while let Some(res) = event_rx.recv().await {
            match res {
                Ok(ev) if matches!(ev.kind, EventKind::Modify(_) | EventKind::Create(_)) => {
                    // Debounce: drain any further events that arrive within the window.
                    tokio::time::sleep(Duration::from_millis(crate::constants::DELAY_CONFIG_DEBOUNCE_MS)).await;
                    while event_rx.try_recv().is_ok() {}

                    // `Config::load` writes a default config if the file is
                    // missing. Skip the reload in that case so a transient
                    // deletion (e.g. atomic rename) never silently replaces
                    // the operator's config with defaults.
                    if !Path::new("data/config.json").exists() {
                        warn!("[ConfigWatcher] data/config.json missing, skipping reload");
                        continue;
                    }

                    match crate::config::Config::load() {
                        Ok(cfg) => {
                            if store_tx.send(StoreMessage::ReloadConfig(cfg)).await.is_err() {
                                info!("[ConfigWatcher] Store channel closed, exiting");
                                return;
                            }
                        }
                        Err(e) => warn!("[ConfigWatcher] Reload failed, keeping old config: {}", e),
                    }
                }
                Ok(_) => {}
                Err(e) => warn!("[ConfigWatcher] Watch error: {}", e),
            }
        }
    });
}
