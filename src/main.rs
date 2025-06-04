use crate::cli::cli_task;
use crate::logging::init_logger;
use crate::messages::{StoreMessage, StoreToBot};
use crate::store::Store;
use tokio::sync::mpsc;

mod bot;
mod cli;
mod config;
mod logging;
mod messages;
mod store;
mod types;

#[tokio::main]
async fn main() {
    // Initialize logger
    let logger = init_logger();

    // Channels for communication
    let (store_tx, store_rx) = mpsc::channel::<StoreMessage>(100);
    let (bot_tx, bot_rx) = mpsc::channel::<StoreToBot>(100);

    // Spawn Store task
    let store = Store::new(logger.clone(), bot_tx.clone()).await;
    let store_handle = tokio::spawn(store.run(store_rx, bot_tx.clone()));

    // Spawn Bot task
    let bot_handle = tokio::spawn(bot_task(store_tx.clone(), bot_rx, logger.clone()));

    // Spawn CLI task
    let cli_handle = tokio::task::spawn_blocking(move || cli_task(store_tx, logger));

    // Wait for tasks to complete
    let _ = tokio::try_join!(store_handle, bot_handle, cli_handle);
}
