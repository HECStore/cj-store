use crate::messages::{CliMessage, StoreMessage};
use crate::types::TradeType;
use dialoguer::{Input, Select};
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info};

/// Runs the CLI task, providing an interactive menu to manage the store.
pub fn cli_task(store_tx: mpsc::Sender<StoreMessage>) {
    loop {
        let options = vec![
            "Get user balances",
            "Get pairs",
            "Set operator status",
            "Add node (no validation)",
            "Add node (with bot validation)",
            "Discover storage (scan for existing nodes)",
            "Remove node",
            "Add pair",
            "Remove pair",
            "View storage",
            "View recent trades",
            "Audit state",
            "Repair state (recompute pair stock)",
            "Restart Bot",
            "Clear stuck order",
            "Exit",
        ];
        let selection = Select::new()
            .with_prompt("Select an action")
            .items(&options)
            .default(0)
            .interact()
            .expect("Failed to read selection");

        match selection {
            0 => get_balances(&store_tx),
            1 => get_pairs(&store_tx),
            2 => set_operator(&store_tx),
            3 => add_node(&store_tx),
            4 => add_node_with_validation(&store_tx),
            5 => discover_storage(&store_tx),
            6 => remove_node(&store_tx),
            7 => add_pair(&store_tx),
            8 => remove_pair(&store_tx),
            9 => view_storage(&store_tx),
            10 => view_trades(&store_tx),
            11 => audit_state(&store_tx, false),
            12 => audit_state(&store_tx, true),
            13 => restart_bot(&store_tx),
            14 => clear_stuck_order(&store_tx),
            15 => {
                info!("[CLI] User selected Exit - initiating graceful shutdown");
                let (response_tx, response_rx) = oneshot::channel();
                let msg = StoreMessage::FromCli(CliMessage::Shutdown {
                    respond_to: response_tx,
                });

                info!("[CLI] Sending shutdown message to Store");
                if store_tx.blocking_send(msg).is_err() {
                    error!("[CLI] Failed to send shutdown request to Store");
                    return;
                }
                info!("[CLI] Shutdown message sent to Store, waiting for confirmation");

                // Wait for shutdown confirmation
                if response_rx.blocking_recv().is_err() {
                    error!("[CLI] Failed to receive shutdown confirmation from Store");
                    return;
                }
                info!("[CLI] Received shutdown confirmation from Store");

                info!("[CLI] Dropping store_tx channel to signal Store shutdown");
                // Drop the store_tx channel to signal store shutdown
                drop(store_tx);
                info!("[CLI] store_tx dropped, CLI task exiting");
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

/// Sends a QueryPairs request and displays the results.
fn get_pairs(store_tx: &mpsc::Sender<StoreMessage>) {
    info!("Requesting pairs");

    // First, get the fee rate from config
    let (fee_tx, fee_rx) = oneshot::channel();
    let fee_msg = StoreMessage::FromCli(CliMessage::QueryFee {
        respond_to: fee_tx,
    });

    let fee = if store_tx.blocking_send(fee_msg).is_ok() {
        fee_rx.blocking_recv().unwrap_or(0.125) // Default to 12.5% if query fails
    } else {
        0.125 // Default to 12.5% if send fails
    };

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::QueryPairs {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("Failed to send QueryPairs request");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(pairs) => {
            if pairs.is_empty() {
                println!("No pairs found.");
                info!("No pairs found");
            } else {
                println!("\n=== Pairs ===");
                for pair in pairs {
                    let price_buy = if pair.item_stock > 0 && pair.currency_stock > 0.0 {
                        let base = pair.currency_stock / (pair.item_stock as f64);
                        Some(base * (1.0 + fee)) // Use actual fee from config
                    } else {
                        None
                    };
                    let price_sell = if pair.item_stock > 0 && pair.currency_stock > 0.0 {
                        let base = pair.currency_stock / (pair.item_stock as f64);
                        Some(base * (1.0 - fee)) // Use actual fee from config
                    } else {
                        None
                    };
                    println!(
                        "Item: {}, Stock: {}, Currency: {:.2}",
                        pair.item, pair.item_stock, pair.currency_stock
                    );
                    if let Some(pb) = price_buy {
                        println!("  Buy price: {:.2} diamonds/item", pb);
                    }
                    if let Some(ps) = price_sell {
                        println!("  Sell price: {:.2} diamonds/item", ps);
                    }
                }
                println!("====================\n");
            }
        }
        Err(_) => {
            println!("Failed to receive pairs.");
            error!("Failed to receive pairs");
        }
    }
}

/// Prompts for username/UUID and operator status, then sends a SetOperator request.
fn set_operator(store_tx: &mpsc::Sender<StoreMessage>) {
    let username_or_uuid: String = Input::new()
        .with_prompt("Enter username or UUID")
        .interact_text()
        .expect("Failed to read username/UUID");

    let is_operator: bool = Select::new()
        .with_prompt("Set operator status")
        .items(&["false", "true"])
        .default(0)
        .interact()
        .expect("Failed to read selection")
        == 1;

    info!("Setting operator status for {} to {}", username_or_uuid, is_operator);

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::SetOperator {
        username_or_uuid: username_or_uuid.clone(),
        is_operator,
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("Failed to send SetOperator request");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(())) => {
            println!("Operator status updated successfully.");
            info!("Operator status updated successfully");
        }
        Ok(Err(e)) => {
            println!("Failed to update operator status: {}", e);
            error!("Failed to update operator status: {}", e);
        }
        Err(_) => {
            println!("Failed to receive operator status update response.");
            error!("Failed to receive operator status update response");
        }
    }
}

/// Sends an AddNode request (without physical validation).
fn add_node(store_tx: &mpsc::Sender<StoreMessage>) {
    info!("Requesting to add node (no validation)");
    println!("Note: This adds the node WITHOUT verifying it exists in-world.");
    println!("Use 'Add node (with bot validation)' for physical verification.");

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::AddNode {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("Failed to send AddNode request");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(node_id)) => {
            println!("Node {} added successfully.", node_id);
            info!("Node {} added successfully", node_id);
        }
        Ok(Err(e)) => {
            println!("Failed to add node: {}", e);
            error!("Failed to add node: {}", e);
        }
        Err(_) => {
            println!("Failed to receive add node response.");
            error!("Failed to receive add node response");
        }
    }
}

/// Sends an AddNodeWithValidation request (with bot-based physical validation).
fn add_node_with_validation(store_tx: &mpsc::Sender<StoreMessage>) {
    info!("Requesting to add node with validation");
    println!("Bot will navigate to the calculated position and verify:");
    println!("  1. All 4 chests exist and can be opened");
    println!("  2. Each chest slot contains a shulker box");
    println!("This may take up to 2 minutes. Please wait...");

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::AddNodeWithValidation {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("Failed to send AddNodeWithValidation request");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(node_id)) => {
            println!("Node {} validated and added successfully!", node_id);
            info!("Node {} validated and added successfully", node_id);
        }
        Ok(Err(e)) => {
            println!("Failed to add node: {}", e);
            error!("Failed to add node: {}", e);
        }
        Err(_) => {
            println!("Failed to receive add node response.");
            error!("Failed to receive add node response");
        }
    }
}

/// Discovers existing storage nodes by having the bot physically scan positions.
fn discover_storage(store_tx: &mpsc::Sender<StoreMessage>) {
    info!("Requesting storage discovery");
    println!("Bot will scan for existing storage nodes starting from position 0.");
    println!("For each position, the bot will:");
    println!("  1. Navigate to the calculated node position");
    println!("  2. Check if all 4 chests exist and contain shulker boxes");
    println!("  3. Add valid nodes to storage");
    println!("Discovery stops when a position without valid chests is found.");
    println!("This may take several minutes. Please wait...");

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::DiscoverStorage {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("Failed to send DiscoverStorage request");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(count)) => {
            println!("Storage discovery complete! {} nodes discovered.", count);
            info!("Storage discovery complete: {} nodes discovered", count);
        }
        Ok(Err(e)) => {
            println!("Storage discovery failed: {}", e);
            error!("Storage discovery failed: {}", e);
        }
        Err(_) => {
            println!("Failed to receive discovery response.");
            error!("Failed to receive discovery response");
        }
    }
}

/// Prompts for node ID, then sends a RemoveNode request.
fn remove_node(store_tx: &mpsc::Sender<StoreMessage>) {
    let node_id: i32 = Input::new()
        .with_prompt("Enter node ID to remove")
        .interact_text()
        .expect("Failed to read node ID");

    info!("Requesting to remove node {}", node_id);

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::RemoveNode {
        node_id,
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("Failed to send RemoveNode request");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(())) => {
            println!("Node {} removed successfully.", node_id);
            info!("Node {} removed successfully", node_id);
        }
        Ok(Err(e)) => {
            println!("Failed to remove node: {}", e);
            error!("Failed to remove node: {}", e);
        }
        Err(_) => {
            println!("Failed to receive remove node response.");
            error!("Failed to receive remove node response");
        }
    }
}

/// Prompts for item name and stack size, then sends an AddPair request.
fn add_pair(store_tx: &mpsc::Sender<StoreMessage>) {
    let item_name: String = Input::new()
        .with_prompt("Enter item name (without minecraft: prefix)")
        .interact_text()
        .expect("Failed to read item name");

    // Prompt for stack size with common options
    let stack_size_selection = Select::new()
        .with_prompt("Select stack size")
        .items(&["64 (most items)", "16 (ender pearls, eggs, signs, buckets)", "1 (tools, weapons, armor)"])
        .default(0)
        .interact()
        .expect("Failed to read stack size selection");
    
    let stack_size = match stack_size_selection {
        0 => 64,
        1 => 16,
        2 => 1,
        _ => 64,
    };

    info!("Requesting to add pair for {} with stack size {}", item_name, stack_size);

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::AddPair {
        item_name: item_name.clone(),
        stack_size,
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("Failed to send AddPair request");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(())) => {
            println!("Pair '{}' added successfully (stack size: {}, stocks set to zero).", item_name, stack_size);
            info!("Pair '{}' added successfully with stack size {}", item_name, stack_size);
        }
        Ok(Err(e)) => {
            println!("Failed to add pair: {}", e);
            error!("Failed to add pair: {}", e);
        }
        Err(_) => {
            println!("Failed to receive add pair response.");
            error!("Failed to receive add pair response");
        }
    }
}

/// Prompts for item name, then sends a RemovePair request.
fn remove_pair(store_tx: &mpsc::Sender<StoreMessage>) {
    let item_name: String = Input::new()
        .with_prompt("Enter item name to remove")
        .interact_text()
        .expect("Failed to read item name");

    info!("Requesting to remove pair for {}", item_name);

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::RemovePair {
        item_name: item_name.clone(),
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("Failed to send RemovePair request");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(())) => {
            println!("Pair '{}' removed successfully.", item_name);
            info!("Pair '{}' removed successfully", item_name);
        }
        Ok(Err(e)) => {
            println!("Failed to remove pair: {}", e);
            error!("Failed to remove pair: {}", e);
        }
        Err(_) => {
            println!("Failed to receive remove pair response.");
            error!("Failed to receive remove pair response");
        }
    }
}

/// Sends a QueryStorage request and displays the storage state.
fn view_storage(store_tx: &mpsc::Sender<StoreMessage>) {
    info!("Requesting storage state");

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::QueryStorage {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("Failed to send QueryStorage request");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(storage) => {
            println!("\n=== Storage State ===");
            println!("Origin: ({}, {}, {})", storage.position.x, storage.position.y, storage.position.z);
            println!("Total nodes: {}", storage.nodes.len());
            println!();
            
            if storage.nodes.is_empty() {
                println!("No nodes configured.");
            } else {
                for node in &storage.nodes {
                    println!("--- Node {} ---", node.id);
                    println!("  Position: ({}, {}, {})", node.position.x, node.position.y, node.position.z);
                    println!("  Chests:");
                    for chest in &node.chests {
                        let total_items: i32 = chest.amounts.iter().sum();
                        let item_display = if chest.item.is_empty() { "(empty)" } else { &chest.item };
                        println!("    Chest {}: {} - {} items total", chest.id, item_display, total_items);
                    }
                    println!();
                }
            }
            println!("====================\n");
        }
        Err(_) => {
            println!("Failed to receive storage state.");
            error!("Failed to receive storage state");
        }
    }
}

/// Sends a QueryTrades request and displays recent trades.
fn view_trades(store_tx: &mpsc::Sender<StoreMessage>) {
    let limit: usize = Input::new()
        .with_prompt("How many recent trades to show")
        .default(20)
        .interact_text()
        .expect("Failed to read limit");

    info!("Requesting recent trades (limit: {})", limit);

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::QueryTrades {
        limit,
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("Failed to send QueryTrades request");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(trades) => {
            if trades.is_empty() {
                println!("No trades found.");
                info!("No trades found");
            } else {
                println!("\n=== Recent Trades ({} shown) ===", trades.len());
                for trade in trades {
                    let trade_type = match trade.trade_type {
                        TradeType::Buy => "BUY",
                        TradeType::Sell => "SELL",
                        TradeType::AddStock => "ADD_STOCK",
                        TradeType::RemoveStock => "REMOVE_STOCK",
                        TradeType::DepositBalance => "DEPOSIT",
                        TradeType::WithdrawBalance => "WITHDRAW",
                        TradeType::AddCurrency => "ADD_CURRENCY",
                        TradeType::RemoveCurrency => "REMOVE_CURRENCY",
                    };
                    println!(
                        "[{}] {} - {}x {} for {:.2} diamonds (user: {})",
                        trade.timestamp.format("%Y-%m-%d %H:%M:%S"),
                        trade_type,
                        trade.amount,
                        trade.item,
                        trade.amount_currency,
                        trade.user_uuid
                    );
                }
                println!("====================\n");
            }
        }
        Err(_) => {
            println!("Failed to receive trades.");
            error!("Failed to receive trades");
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

/// Clears stuck order processing state, allowing the queue to continue.
fn clear_stuck_order(store_tx: &mpsc::Sender<StoreMessage>) {
    info!("Requesting to clear stuck order");
    println!("This will clear any stuck order processing state.");
    println!("Use this if an order got stuck and the queue isn't advancing.");

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::ClearStuckOrder {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("Failed to send ClearStuckOrder request");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Some(stuck_order)) => {
            println!("Cleared stuck order: {}", stuck_order);
            println!("Queue will now continue processing remaining orders.");
            info!("Cleared stuck order: {}", stuck_order);
        }
        Ok(None) => {
            println!("No stuck order was detected (processing was not blocked).");
            info!("No stuck order detected");
        }
        Err(_) => {
            println!("Failed to receive clear stuck order response.");
            error!("Failed to receive clear stuck order response");
        }
    }
}

/// Sends an AuditState request and displays any invariant violations found.
/// If `repair` is true, also applies safe automatic repairs (e.g. recomputing pair stock).
fn audit_state(store_tx: &mpsc::Sender<StoreMessage>, repair: bool) {
    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::AuditState { repair, respond_to: response_tx });

    if store_tx.blocking_send(msg).is_err() {
        error!("Failed to send AuditState request");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(lines) => {
            if lines.is_empty() {
                println!("Audit OK (no issues found).");
            } else {
                println!("\n=== Audit Report ===");
                for line in lines {
                    println!("- {}", line);
                }
                println!("====================\n");
            }
        }
        Err(_) => {
            println!("Failed to receive audit response.");
            error!("Failed to receive audit response");
        }
    }
}
