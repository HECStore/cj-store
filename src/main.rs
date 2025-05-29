mod bot;
mod chest;
mod config;
mod node;
mod order;
mod pair;
mod position;
mod storage;
mod store;
mod user;

use crate::store::Store;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut store = Store::new().unwrap();

    // Initialize the bot
    store.init_bot().await?;

    // Example of sending a message
    store
        .send_trade_notification("Trade bot is now online!")
        .await?;

    // Keep the program running
    tokio::signal::ctrl_c().await?;

    // Cleanup
    println!("Saving store data...");
    store.save()?;
    println!("Disconnecting bot...");
    store.disconnect_bot().await?;
    println!("Cleanup complete!");

    Ok(())
}
