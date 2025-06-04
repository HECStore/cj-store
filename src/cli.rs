// src/cli.rs

use crate::logging::LogMessage;
use crate::messages::{CliToStore, StoreMessage};
use crate::types::User;
use dialoguer::{Input, Select};
use tokio::sync::{mpsc, oneshot};
use tracing::Level;

/// Runs the CLI task, providing an interactive menu to manage the store.
pub fn cli_task(store_tx: mpsc::Sender<StoreMessage>, logger: mpsc::Sender<LogMessage>) {
    loop {
        let options = vec!["Get user balances", "Set item price", "Reboot Bot", "Exit"];
        let selection = Select::new()
            .with_prompt("Select an action")
            .items(&options)
            .default(0)
            .interact()
            .expect("Failed to read selection");

        match selection {
            0 => get_balances(&store_tx, &logger),
            1 => set_price(&store_tx, &logger),
            2 => reboot_bot(&store_tx, &logger),
            3 => {
                log(&logger, Level::INFO, "Exiting CLI".to_string());
                break;
            }
            _ => unreachable!(),
        }
    }
}

/// Sends a GetBalances request and displays the results.
fn get_balances(store_tx: &mpsc::Sender<StoreMessage>, logger: &mpsc::Sender<LogMessage>) {
    log(logger, Level::INFO, "Requesting user balances".to_string());

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliToStore::GetBalances {
        response_channel: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        log(
            logger,
            Level::ERROR,
            "Failed to send GetBalances request".to_string(),
        );
        return;
    }

    match response_rx.blocking_recv() {
        Ok(balances) => {
            if balances.is_empty() {
                println!("No users found.");
                log(logger, Level::INFO, "No users found".to_string());
            } else {
                for user in balances {
                    println!("User: {}, Balance: {}", user.username, user.balance);
                    log(
                        logger,
                        Level::INFO,
                        format!("User: {}, Balance: {}", user.username, user.balance),
                    );
                }
            }
        }
        Err(_) => {
            println!("Failed to receive balances.");
            log(
                logger,
                Level::ERROR,
                "Failed to receive balances".to_string(),
            );
        }
    }
}

/// Prompts for item and price, then sends a SetPrice request.
fn set_price(store_tx: &mpsc::Sender<StoreMessage>, logger: &mpsc::Sender<LogMessage>) {
    let item: String = Input::new()
        .with_prompt("Enter item name")
        .interact_text()
        .expect("Failed to read item name");
    let price: f64 = Input::new()
        .with_prompt("Enter price")
        .interact_text()
        .expect("Failed to read price");

    log(
        logger,
        Level::INFO,
        format!("Setting price for {} to {}", item, price),
    );

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliToStore::SetPrice {
        item,
        price,
        response_channel: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        log(
            logger,
            Level::ERROR,
            "Failed to send SetPrice request".to_string(),
        );
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(())) => {
            println!("Price set successfully.");
            log(logger, Level::INFO, "Price set successfully".to_string());
        }
        Ok(Err(e)) => {
            println!("Failed to set price: {}", e);
            log(logger, Level::ERROR, format!("Failed to set price: {}", e));
        }
        Err(_) => {
            println!("Failed to receive price response.");
            log(
                logger,
                Level::ERROR,
                "Failed to receive price response".to_string(),
            );
        }
    }
}

/// Sends a RebootBot request.
fn reboot_bot(store_tx: &mpsc::Sender<StoreMessage>, logger: &mpsc::Sender<LogMessage>) {
    log(logger, Level::INFO, "Requesting Bot reboot".to_string());

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliToStore::RebootBot {
        response_channel: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        log(
            logger,
            Level::ERROR,
            "Failed to send RebootBot request".to_string(),
        );
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(())) => {
            println!("Bot reboot initiated.");
            log(logger, Level::INFO, "Bot reboot initiated".to_string());
        }
        Ok(Err(e)) => {
            println!("Failed to reboot Bot: {}", e);
            log(logger, Level::ERROR, format!("Failed to reboot Bot: {}", e));
        }
        Err(_) => {
            println!("Failed to receive reboot response.");
            log(
                logger,
                Level::ERROR,
                "Failed to receive reboot response".to_string(),
            );
        }
    }
}

/// Helper to send a log message.
fn log(logger: &mpsc::Sender<LogMessage>, level: Level, message: String) {
    let _ = logger.blocking_send(LogMessage { level, message });
}
