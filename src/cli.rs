//! Interactive CLI menu for store operators.
//!
//! This module runs on a dedicated blocking thread (not a Tokio task) because
//! `dialoguer` performs synchronous terminal I/O. It communicates with the
//! async `Store` actor by sending `CliMessage`s over an `mpsc` channel and
//! awaiting replies via `oneshot` channels using `blocking_send` /
//! `blocking_recv`.

use crate::messages::{CliMessage, StoreMessage};
use crate::types::TradeType;
use dialoguer::{Input, Select};
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info, warn};

/// Retry a dialoguer prompt on transient I/O error rather than aborting.
///
/// A terminal read can fail for a variety of non-fatal reasons (EINTR during
/// terminal resize, lost/reattached stdin on some shells, etc.). Previously
/// every `.interact()` was wrapped in `.expect(..)`, which killed the entire
/// CLI task on the first hiccup. The loop re-prompts with a short backoff so
/// the operator sees the prompt again instead of the process exiting.
/// Compute buy/sell quotes from a pair's reserves and fee spread.
///
/// Returns `(None, None)` when either reserve is zero: the pair is untradeable
/// and the constant-product mid-price would be undefined or infinite.
fn quote_prices(item_stock: i32, currency_stock: f64, fee: f64) -> (Option<f64>, Option<f64>) {
    if item_stock > 0 && currency_stock > 0.0 {
        let base = currency_stock / (item_stock as f64);
        (Some(base * (1.0 + fee)), Some(base * (1.0 - fee)))
    } else {
        (None, None)
    }
}

fn with_retry<T, E: std::fmt::Display>(desc: &str, mut f: impl FnMut() -> Result<T, E>) -> T {
    loop {
        match f() {
            Ok(v) => return v,
            Err(e) => {
                warn!("[CLI] {desc}: {e} — retrying");
                std::thread::sleep(std::time::Duration::from_millis(crate::constants::DELAY_MEDIUM_MS));
            }
        }
    }
}

/// Runs the CLI task, providing an interactive menu to manage the store.
///
/// This function blocks the calling thread in a loop until the operator
/// selects "Exit". On exit it performs a coordinated shutdown: it sends a
/// `Shutdown` message, waits for the `Store` to confirm, then drops
/// `store_tx` so the `Store`'s receiver closes and its task can terminate.
pub fn cli_task(store_tx: mpsc::Sender<StoreMessage>) {
    loop {
        // Indices in the match below are positional — adding/removing an entry
        // shifts every case after it.
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
        let selection = with_retry("Failed to read selection", || {
            Select::new()
                .with_prompt("Select an action")
                .items(&options)
                .default(0)
                .interact()
        });

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
                info!("[CLI] Initiating graceful shutdown");
                let (response_tx, response_rx) = oneshot::channel();
                let msg = StoreMessage::FromCli(CliMessage::Shutdown {
                    respond_to: response_tx,
                });

                if store_tx.blocking_send(msg).is_err() {
                    error!("[CLI] Shutdown send failed: Store channel closed");
                    return;
                }

                if response_rx.blocking_recv().is_err() {
                    error!("[CLI] Shutdown response channel closed without reply");
                    return;
                }

                // Drop sender so the Store's receiver closes and its task can terminate.
                drop(store_tx);
                info!("[CLI] Shutdown complete");
                break;
            }
            _ => unreachable!(),
        }
    }
}

/// Sends a QueryBalances request and displays the results.
fn get_balances(store_tx: &mpsc::Sender<StoreMessage>) {
    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::QueryBalances {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] QueryBalances send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(balances) => {
            if balances.is_empty() {
                println!("No users found.");
            } else {
                println!("\n=== User Balances ===");
                for user in balances {
                    println!(
                        "User: {}, Balance: {} diamonds",
                        user.username, user.balance
                    );
                }
                println!("====================\n");
            }
        }
        Err(_) => {
            println!("Failed to receive balances.");
            error!("[CLI] QueryBalances response channel closed without reply");
        }
    }
}

/// Sends a QueryPairs request and displays the results, including
/// AMM-style buy/sell prices derived from each pair's current reserves.
fn get_pairs(store_tx: &mpsc::Sender<StoreMessage>) {
    // Fall back to the default configured fee on query failure so the operator
    // still sees a price estimate. A non-default fee would make the displayed
    // prices materially wrong, so the fallback path must warn.
    const DEFAULT_FEE_FALLBACK: f64 = 0.125;
    let (fee_tx, fee_rx) = oneshot::channel();
    let fee_msg = StoreMessage::FromCli(CliMessage::QueryFee {
        respond_to: fee_tx,
    });

    let fee = if store_tx.blocking_send(fee_msg).is_ok() {
        match fee_rx.blocking_recv() {
            Ok(f) => f,
            Err(_) => {
                warn!("[CLI] QueryFee response failed, displaying prices with fallback fee {}", DEFAULT_FEE_FALLBACK);
                DEFAULT_FEE_FALLBACK
            }
        }
    } else {
        warn!("[CLI] QueryFee send failed, displaying prices with fallback fee {}", DEFAULT_FEE_FALLBACK);
        DEFAULT_FEE_FALLBACK
    };

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::QueryPairs {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] QueryPairs send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(pairs) => {
            if pairs.is_empty() {
                println!("No pairs found.");
            } else {
                println!("\n=== Pairs ===");
                for pair in pairs {
                    let (price_buy, price_sell) = quote_prices(pair.item_stock, pair.currency_stock, fee);
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
            error!("[CLI] QueryPairs response channel closed without reply");
        }
    }
}

/// Prompts for username/UUID and operator status, then sends a SetOperator request.
fn set_operator(store_tx: &mpsc::Sender<StoreMessage>) {
    let username_or_uuid: String = with_retry("Failed to read username/UUID", || {
        Input::new()
            .with_prompt("Enter username or UUID")
            .interact_text()
    });

    // Default to "false" (index 0) so accidentally pressing Enter never
    // grants operator privileges by mistake.
    let is_operator: bool = with_retry("Failed to read selection", || {
        Select::new()
            .with_prompt("Set operator status")
            .items(["false", "true"])
            .default(0)
            .interact()
    }) == 1;

    info!("[CLI] Setting operator status for {} to {}", username_or_uuid, is_operator);

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::SetOperator {
        username_or_uuid: username_or_uuid.clone(),
        is_operator,
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] SetOperator send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(())) => println!("Operator status updated successfully."),
        Ok(Err(e)) => {
            println!("Failed to update operator status: {}", e);
            error!("[CLI] SetOperator for {username_or_uuid} failed: {e}");
        }
        Err(_) => error!("[CLI] SetOperator response channel closed without reply"),
    }
}

/// Sends an AddNode request (without physical validation).
fn add_node(store_tx: &mpsc::Sender<StoreMessage>) {
    println!("Note: This adds the node WITHOUT verifying it exists in-world.");
    println!("Use 'Add node (with bot validation)' for physical verification.");

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::AddNode {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] AddNode send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(node_id)) => println!("Node {} added successfully.", node_id),
        Ok(Err(e)) => {
            println!("Failed to add node: {}", e);
            error!("[CLI] AddNode failed: {e}");
        }
        Err(_) => error!("[CLI] AddNode response channel closed without reply"),
    }
}

/// Sends an AddNodeWithValidation request (with bot-based physical validation).
fn add_node_with_validation(store_tx: &mpsc::Sender<StoreMessage>) {
    println!("Bot will navigate to the calculated position and verify:");
    println!("  1. All 4 chests exist and can be opened");
    println!("  2. Each chest slot contains a shulker box");
    println!("This may take up to 2 minutes. Please wait...");

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::AddNodeWithValidation {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] AddNodeWithValidation send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(node_id)) => println!("Node {} validated and added successfully!", node_id),
        Ok(Err(e)) => {
            println!("Failed to add node: {}", e);
            error!("[CLI] AddNodeWithValidation failed: {e}");
        }
        Err(_) => error!("[CLI] AddNodeWithValidation response channel closed without reply"),
    }
}

/// Discovers existing storage nodes by having the bot physically scan positions.
fn discover_storage(store_tx: &mpsc::Sender<StoreMessage>) {
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
        error!("[CLI] DiscoverStorage send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(count)) => println!("Storage discovery complete! {} nodes discovered.", count),
        Ok(Err(e)) => {
            println!("Storage discovery failed: {}", e);
            error!("[CLI] DiscoverStorage failed: {e}");
        }
        Err(_) => error!("[CLI] DiscoverStorage response channel closed without reply"),
    }
}

/// Prompts for node ID, then sends a RemoveNode request.
fn remove_node(store_tx: &mpsc::Sender<StoreMessage>) {
    let node_id: i32 = with_retry("Failed to read node ID", || {
        Input::new()
            .with_prompt("Enter node ID to remove")
            .interact_text()
    });

    info!("[CLI] Requesting to remove node {}", node_id);

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::RemoveNode {
        node_id,
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] RemoveNode send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(())) => println!("Node {} removed successfully.", node_id),
        Ok(Err(e)) => {
            println!("Failed to remove node: {}", e);
            error!("[CLI] RemoveNode {node_id} failed: {e}");
        }
        Err(_) => error!("[CLI] RemoveNode response channel closed without reply"),
    }
}

/// Prompts for item name and stack size, then sends an AddPair request.
fn add_pair(store_tx: &mpsc::Sender<StoreMessage>) {
    let item_name: String = with_retry("Failed to read item name", || {
        Input::new()
            .with_prompt("Enter item name (without minecraft: prefix)")
            .interact_text()
    });

    // Stack size must match Minecraft's hard-coded per-item limit, otherwise
    // the bot's storage math (shulker box layouts, chest capacity) will be
    // off. We expose the three valid values rather than a free-form number
    // so operators can't enter an illegal stack size like 32.
    let stack_size_selection = with_retry("Failed to read stack size selection", || {
        Select::new()
            .with_prompt("Select stack size")
            .items(["64 (most items)", "16 (ender pearls, eggs, signs, buckets)", "1 (tools, weapons, armor)"])
            .default(0)
            .interact()
    });

    let stack_size = match stack_size_selection {
        0 => 64,
        1 => 16,
        2 => 1,
        _ => unreachable!("Select bounded by items() above"),
    };

    info!("[CLI] Requesting to add pair for {} with stack size {}", item_name, stack_size);

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::AddPair {
        item_name: item_name.clone(),
        stack_size,
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] AddPair send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(())) => println!("Pair '{}' added successfully (stack size: {}, stocks set to zero).", item_name, stack_size),
        Ok(Err(e)) => {
            println!("Failed to add pair: {}", e);
            error!("[CLI] AddPair for {item_name} (stack {stack_size}) failed: {e}");
        }
        Err(_) => error!("[CLI] AddPair response channel closed without reply"),
    }
}

/// Prompts for item name, then sends a RemovePair request.
fn remove_pair(store_tx: &mpsc::Sender<StoreMessage>) {
    let item_name: String = with_retry("Failed to read item name", || {
        Input::new()
            .with_prompt("Enter item name to remove")
            .interact_text()
    });

    info!("[CLI] Requesting to remove pair for {}", item_name);

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::RemovePair {
        item_name: item_name.clone(),
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] RemovePair send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(())) => println!("Pair '{}' removed successfully.", item_name),
        Ok(Err(e)) => {
            println!("Failed to remove pair: {}", e);
            error!("[CLI] RemovePair for {item_name} failed: {e}");
        }
        Err(_) => error!("[CLI] RemovePair response channel closed without reply"),
    }
}

/// Sends a QueryStorage request and displays the storage state.
fn view_storage(store_tx: &mpsc::Sender<StoreMessage>) {
    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::QueryStorage {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] QueryStorage send failed: Store channel closed");
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
        Err(_) => error!("[CLI] QueryStorage response channel closed without reply"),
    }
}

/// Sends a QueryTrades request and displays recent trades.
fn view_trades(store_tx: &mpsc::Sender<StoreMessage>) {
    let limit: usize = with_retry("Failed to read limit", || {
        Input::new()
            .with_prompt("How many recent trades to show")
            .default(20)
            .interact_text()
    });

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::QueryTrades {
        limit,
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] QueryTrades send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(trades) => {
            if trades.is_empty() {
                println!("No trades found.");
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
        Err(_) => error!("[CLI] QueryTrades response channel closed without reply"),
    }
}

/// Sends a RestartBot request.
fn restart_bot(store_tx: &mpsc::Sender<StoreMessage>) {
    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::RestartBot {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] RestartBot send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Ok(())) => println!("Bot restart initiated successfully."),
        Ok(Err(e)) => {
            println!("Failed to restart Bot: {}", e);
            error!("[CLI] RestartBot failed: {e}");
        }
        Err(_) => error!("[CLI] RestartBot response channel closed without reply"),
    }
}

/// Clears stuck order processing state, allowing the queue to continue.
fn clear_stuck_order(store_tx: &mpsc::Sender<StoreMessage>) {
    println!("This will clear any stuck order processing state.");
    println!("Use this if an order got stuck and the queue isn't advancing.");

    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::ClearStuckOrder {
        respond_to: response_tx,
    });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] ClearStuckOrder send failed: Store channel closed");
        return;
    }

    match response_rx.blocking_recv() {
        Ok(Some(stuck_order)) => {
            println!("Cleared stuck order: {}", stuck_order);
            println!("Queue will now continue processing remaining orders.");
        }
        Ok(None) => println!("No stuck order was detected (processing was not blocked)."),
        Err(_) => error!("[CLI] ClearStuckOrder response channel closed without reply"),
    }
}

/// Sends an AuditState request and displays any invariant violations found.
/// If `repair` is true, also applies safe automatic repairs (e.g. recomputing pair stock).
fn audit_state(store_tx: &mpsc::Sender<StoreMessage>, repair: bool) {
    let (response_tx, response_rx) = oneshot::channel();
    let msg = StoreMessage::FromCli(CliMessage::AuditState { repair, respond_to: response_tx });

    if store_tx.blocking_send(msg).is_err() {
        error!("[CLI] AuditState send failed: Store channel closed");
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
            error!("[CLI] AuditState response channel closed without reply");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_prices_returns_none_when_item_stock_is_zero() {
        assert_eq!(quote_prices(0, 100.0, 0.125), (None, None));
    }

    #[test]
    fn quote_prices_returns_none_when_currency_stock_is_zero() {
        assert_eq!(quote_prices(10, 0.0, 0.125), (None, None));
    }

    #[test]
    fn quote_prices_returns_none_when_currency_stock_is_negative() {
        // Defensive: constant-product is undefined for non-positive reserves.
        assert_eq!(quote_prices(10, -1.0, 0.125), (None, None));
    }

    #[test]
    fn quote_prices_applies_fee_symmetrically_around_mid() {
        // item_stock=10, currency_stock=100 -> mid = 10.0, fee = 0.125
        // buy  = 10.0 * 1.125 = 11.25
        // sell = 10.0 * 0.875 =  8.75
        let (buy, sell) = quote_prices(10, 100.0, 0.125);
        assert!((buy.unwrap() - 11.25).abs() < 1e-9);
        assert!((sell.unwrap() - 8.75).abs() < 1e-9);
    }

    #[test]
    fn quote_prices_with_zero_fee_collapses_buy_and_sell_to_mid() {
        let (buy, sell) = quote_prices(4, 8.0, 0.0);
        assert_eq!(buy, sell);
        assert!((buy.unwrap() - 2.0).abs() < 1e-9);
    }
}
