use crate::messages::{CliMessage, StoreMessage};
use dialoguer::{Input, Select};
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info, warn};

/// Runs the CLI task, providing an interactive menu to manage the store.
pub fn cli_task(store_tx: mpsc::Sender<StoreMessage>) {
    loop {
        let options = vec!["Get user balances", "Set item price", "Restart Bot", "Exit"];
        let selection = Select::new()
            .with_prompt("Select an action")
            .items(&options)
            .default(0)
            .interact()
            .expect("Failed to read selection");

        match selection {
            0 => get_balances(&store_tx),
            1 => set_price(&store_tx),
            2 => restart_bot(&store_tx),
            3 => {
                info!("Initiating graceful shutdown");
                let (response_tx, response_rx) = oneshot::channel();
                let msg = StoreMessage::FromCli(CliMessage::Shutdown {
                    respond_to: response_tx,
                });

                if store_tx.blocking_send(msg).is_err() {
                    error!("Failed to send shutdown request");
                    return;
                }

                // Wait for shutdown confirmation
                if response_rx.blocking_recv().is_err() {
                    error!("Failed to receive shutdown confirmation");
                    return;
                }

                info!("Shutdown complete");
                // Drop the store_tx channel to signal store shutdown
                drop(store_tx);
                break;
            }
            _ => unreachable!(),
        }
    }
}

/// Sends a QueryBalances request and displays the results.
fn get_balances(store_tx: &mpsc::Sender<StoreMessage>) {
    info!("Requesting user balances");

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::QueryBalances {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("Failed to send QueryBalances request");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(balances) => {
            if balances.is_empty() {
                println!("No users found.");
                info!("No users found");
            } else {
                println!("\n=== User Balances ===");
                for user in balances {
                    println!(
                        "User: {}, Balance: {} diamonds",
                        user.username, user.balance
                    );
                    info!(
                        "User: {}, Balance: {} diamonds",
                        user.username, user.balance
                    );
                }
                println!("====================\n");
            }
        }
        Err(_) => {
            println!("Failed to receive balances.");
            error!("Failed to receive balances");
        }
    }
}

/// Prompts for item and price, then sends an UpdatePrice request.
fn set_price(store_tx: &mpsc::Sender<StoreMessage>) {
    let item_name: String = Input::new()
        .with_prompt("Enter item name")
        .interact_text()
        .expect("Failed to read item name");

    let new_price: f64 = Input::new()
        .with_prompt("Enter new price")
        .interact_text()
        .expect("Failed to read price");

    if new_price < 0.0 {
        println!("Price cannot be negative.");
        warn!("Attempted to set negative price");
        return;
    }

    info!("Setting price for {} to {} diamonds", item_name, new_price);

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::UpdatePrice {
        item_name,
        new_price,
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("Failed to send UpdatePrice request");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(())) => {
            println!("Price updated successfully.");
            info!("Price updated successfully");
        }
        Ok(Err(e)) => {
            println!("Failed to update price: {}", e);
            error!("Failed to update price: {}", e);
        }
        Err(_) => {
            println!("Failed to receive price update response.");
            error!("Failed to receive price update response");
        }
    }
}

/// Sends a RestartBot request.
fn restart_bot(store_tx: &mpsc::Sender<StoreMessage>) {
    info!("Requesting Bot restart");

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::RestartBot {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("Failed to send RestartBot request");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(())) => {
            println!("Bot restart initiated successfully.");
            info!("Bot restart initiated successfully");
        }
        Ok(Err(e)) => {
            println!("Failed to restart Bot: {}", e);
            error!("Failed to restart Bot: {}", e);
        }
        Err(_) => {
            println!("Failed to receive restart response.");
            error!("Failed to receive restart response");
        }
    }
}
