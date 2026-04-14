//! Player command handlers
//!
//! Handles player commands received via whisper from the bot.
//! Provides user-friendly error messages and input validation.
//!
//! ## Order Queue System
//!
//! Commands are categorized into two types:
//!
//! **Quick commands** (execute immediately):
//! - balance, price, help, items, pay, status
//! - queue, cancel
//! - Operator commands (additem, removeitem, etc.)
//!
//! **Order commands** (queued for sequential processing):
//! - buy, sell, deposit, withdraw
//! - These are validated and added to the queue
//! - Player gets immediate feedback with queue position

use tracing::{debug, warn, info};

use crate::constants::CHEST_OP_TIMEOUT_SECS;
use crate::types::ItemId;

use super::super::{Store, state, utils};
use super::operator;
use crate::constants::MAX_TRANSACTION_QUANTITY;
use crate::messages::QueuedOrderType;

/// Validate item name format.
/// Item names should be alphanumeric with optional underscores and colons.
/// 
/// # Returns
/// * `Ok(())` if valid
/// * `Err(message)` with user-friendly error message if invalid
fn validate_item_name(item: &str) -> Result<(), String> {
    if item.is_empty() {
        return Err("Item name cannot be empty. Example: buy cobblestone 64".to_string());
    }
    
    // Check for invalid characters
    let normalized = utils::normalize_item_id(item);
    if normalized.is_empty() {
        return Err("Invalid item name. Example items: cobblestone, iron_ingot, diamond".to_string());
    }
    
    // Basic format validation: alphanumeric, underscores, colons
    for c in item.chars() {
        if !c.is_alphanumeric() && c != '_' && c != ':' {
            return Err(format!(
                "Item name contains invalid character '{}'. Use only letters, numbers, and underscores.",
                c
            ));
        }
    }
    
    Ok(())
}

/// Validate quantity for transactions.
/// 
/// # Returns
/// * `Ok(quantity)` if valid
/// * `Err(message)` with user-friendly error message if invalid
fn validate_quantity(quantity_str: &str, operation: &str) -> Result<u32, String> {
    let quantity: u32 = quantity_str.parse().map_err(|_| {
        format!(
            "Invalid quantity '{}'. Please enter a whole number. Example: {} cobblestone 64",
            quantity_str, operation
        )
    })?;
    
    if quantity == 0 {
        return Err(format!(
            "Quantity must be at least 1. Example: {} cobblestone 64",
            operation
        ));
    }
    
    if quantity > MAX_TRANSACTION_QUANTITY as u32 {
        return Err(format!(
            "Quantity {} is too large. Maximum is {} items per transaction.",
            quantity, MAX_TRANSACTION_QUANTITY
        ));
    }
    
    Ok(quantity)
}

/// Validate username format.
/// Minecraft usernames are 3-16 characters, alphanumeric with underscores.
fn validate_username(username: &str) -> Result<(), String> {
    if username.len() < 3 || username.len() > 16 {
        return Err(format!(
            "Invalid username '{}'. Minecraft usernames are 3-16 characters.",
            username
        ));
    }
    
    for c in username.chars() {
        if !c.is_alphanumeric() && c != '_' {
            return Err(format!(
                "Invalid username '{}'. Usernames contain only letters, numbers, and underscores.",
                username
            ));
        }
    }
    
    Ok(())
}

/// Handle player commands from the bot
///
/// This function first checks rate limiting, then dispatches to the appropriate handler.
/// Quick commands (balance, price, help, etc.) execute immediately.
/// Order commands (buy, sell, deposit, withdraw) are queued for sequential processing.
pub async fn handle_player_command(
    store: &mut Store,
    player_name: &str,
    command: &str,
) -> Result<(), String> {
    // Resolve user UUID for rate limiting (creates user if needed).
    // UUID is the canonical identity key - rate limiting and ownership
    // must not rely on usernames since players can change them.
    let user_uuid = utils::resolve_user_uuid(store, player_name).await?;
    utils::ensure_user_exists(store, player_name, &user_uuid);

    // Rate limiting check - runs before command parsing so spamming any
    // command (including malformed ones) counts toward the per-user limit.
    // Violations are silently absorbed with a "please wait" reply: no
    // dispatch happens, so the queue and downstream state stay untouched.
    if let Err(wait_duration) = store.rate_limiter.check(&user_uuid) {
        let wait_secs = wait_duration.as_secs_f64();
        let msg = if wait_secs < 1.0 {
            format!("Please wait {:.1}s before sending another message.", wait_secs)
        } else {
            format!("Please wait {:.0}s before sending another message.", wait_secs.ceil())
        };
        return utils::send_message_to_player(store, player_name, &msg).await;
    }

    let parts: Vec<&str> = command.split_whitespace().collect();

    // Command dispatch:
    // - "Order commands" (buy/sell/deposit/withdraw) only validate input
    //   here and push an entry onto store.order_queue. Actual chest I/O and
    //   trade GUI interaction happen later on the queue processor task.
    // - "Quick commands" (balance/price/help/items/pay/queue/cancel/status
    //   and operator admin commands) run inline because they don't need
    //   the bot to physically move or interact with chests/players.
    match parts.get(0) {
        Some(&"buy") | Some(&"b") => {
            if parts.len() >= 3 {
                // Validate item name
                if let Err(e) = validate_item_name(parts[1]) {
                    return utils::send_message_to_player(store, player_name, &e).await;
                }
                let item = utils::normalize_item_id(parts[1]);
                
                // Validate quantity
                let quantity = match validate_quantity(parts[2], "buy") {
                    Ok(q) => q,
                    Err(e) => {
                        warn!("Invalid quantity provided by {}: {}", player_name, parts[2]);
                        return utils::send_message_to_player(store, player_name, &e).await;
                    }
                };

                // Check if pair exists
                if !store.pairs.contains_key(&item) {
                    return utils::send_message_to_player(
                        store,
                        player_name,
                        &format!("Item '{}' is not available for trading", item),
                    ).await;
                }

                debug!(
                    "Queueing buy order: {} wants {} of {}",
                    player_name, quantity, item
                );

                // Enqueue for sequential processing. Orders run one at a
                // time because the bot has a single body - it must walk to
                // chests, open the trade GUI, etc. Player receives the
                // assigned order id and position so they can track/cancel.
                match store.order_queue.add(
                    user_uuid.clone(),
                    player_name.to_string(),
                    QueuedOrderType::Buy,
                    item.clone(),
                    quantity,
                ) {
                    Ok((order_id, position)) => {
                        let queue_len = store.order_queue.len();
                        let wait_estimate = store.order_queue.estimate_wait(position);
                        let msg = format!(
                            "Order #{} queued (position {}/{}). Est. wait: {}. You have {} order(s) pending.",
                            order_id, position, queue_len, wait_estimate,
                            store.order_queue.user_order_count(&user_uuid)
                        );
                        utils::send_message_to_player(store, player_name, &msg).await
                    }
                    Err(e) => utils::send_message_to_player(store, player_name, &e).await,
                }
            } else {
                warn!(
                    "Invalid buy command format from {}: {}",
                    player_name, command
                );
                // Send usage message back to bot
                utils::send_message_to_player(store, player_name, "Usage: buy <item> <quantity>")
                    .await
            }
        }
        Some(&"sell") | Some(&"s") => {
            if parts.len() >= 3 {
                // Validate item name
                if let Err(e) = validate_item_name(parts[1]) {
                    return utils::send_message_to_player(store, player_name, &e).await;
                }
                let item = utils::normalize_item_id(parts[1]);
                
                // Validate quantity
                let quantity = match validate_quantity(parts[2], "sell") {
                    Ok(q) => q,
                    Err(e) => {
                        warn!("Invalid quantity provided by {}: {}", player_name, parts[2]);
                        return utils::send_message_to_player(store, player_name, &e).await;
                    }
                };

                // Check if pair exists
                if !store.pairs.contains_key(&item) {
                    return utils::send_message_to_player(
                        store,
                        player_name,
                        &format!("Item '{}' is not available for trading", item),
                    ).await;
                }
                
                debug!(
                    "Queueing sell order: {} wants to sell {} of {}",
                    player_name, quantity, item
                );

                // Add to order queue
                match store.order_queue.add(
                    user_uuid.clone(),
                    player_name.to_string(),
                    QueuedOrderType::Sell,
                    item.clone(),
                    quantity,
                ) {
                    Ok((order_id, position)) => {
                        let queue_len = store.order_queue.len();
                        let wait_estimate = store.order_queue.estimate_wait(position);
                        let msg = format!(
                            "Order #{} queued (position {}/{}). Est. wait: {}. You have {} order(s) pending.",
                            order_id, position, queue_len, wait_estimate,
                            store.order_queue.user_order_count(&user_uuid)
                        );
                        utils::send_message_to_player(store, player_name, &msg).await
                    }
                    Err(e) => utils::send_message_to_player(store, player_name, &e).await,
                }
            } else {
                warn!(
                    "Invalid sell command format from {}: {}",
                    player_name, command
                );
                utils::send_message_to_player(store, player_name, "Usage: sell <item> <quantity>. Example: sell cobblestone 64")
                    .await
            }
        }
        Some(&"bal") | Some(&"balance") => {
            // Check if a username was provided to look up someone else's balance
            let target_name = if parts.len() >= 2 {
                // Validate the username
                if let Err(e) = validate_username(parts[1]) {
                    return utils::send_message_to_player(store, player_name, &e).await;
                }
                parts[1]
            } else {
                player_name
            };
            
            debug!("Balance check requested by {} for {}", player_name, target_name);
            match get_user_balance_async(store, target_name).await {
                Ok(balance) => {
                    let message = format!("{}'s balance: {:.2} diamonds", target_name, balance);
                    utils::send_message_to_player(store, player_name, &message).await
                }
                Err(e) => {
                    // User might not exist yet
                    if e.contains("not found") || e.contains("No user") {
                        utils::send_message_to_player(
                            store,
                            player_name,
                            &format!("{} has no account yet (balance: 0 diamonds)", target_name),
                        ).await
                    } else {
                        utils::send_message_to_player(store, player_name, &e).await
                    }
                }
            }
        }
        Some(&"pay") => {
            if parts.len() >= 3 {
                let recipient = parts[1];
                
                // Validate recipient username
                if let Err(e) = validate_username(recipient) {
                    return utils::send_message_to_player(store, player_name, &e).await;
                }
                
                let amount: f64 = parts[2].parse().map_err(|_| {
                    format!(
                        "Invalid amount '{}'. Please enter a number. Example: pay Steve 10.5",
                        parts[2]
                    )
                })?;
                
                if amount <= 0.0 {
                    return utils::send_message_to_player(
                        store,
                        player_name,
                        "Amount must be positive. Example: pay Steve 10.5",
                    )
                    .await;
                }
                
                if amount > 1_000_000.0 {
                    return utils::send_message_to_player(
                        store,
                        player_name,
                        "Amount too large. Maximum is 1,000,000 per payment.",
                    )
                    .await;
                }
                
                info!(
                    "Processing payment: {} -> {} ({})",
                    player_name, recipient, amount
                );
                match pay_async(store, player_name, recipient, amount).await {
                    Ok(()) => {
                        info!(
                            "Payment successful: {} paid {} to {}",
                            player_name, amount, recipient
                        );
                        
                        // Notify the recipient (will only reach them if they're online)
                        let payee_message = format!(
                            "You received {:.2} diamonds from {}",
                            amount, player_name
                        );
                        let _ = utils::send_message_to_player(store, recipient, &payee_message).await;
                        
                        // Notify the payer of success
                        let payer_message = format!("Paid {:.2} diamonds to {}", amount, recipient);
                        utils::send_message_to_player(store, player_name, &payer_message).await
                    }
                    Err(e) => {
                        warn!("Payment failed: {} -> {}: {}", player_name, recipient, e);
                        utils::send_message_to_player(store, player_name, &e).await
                    }
                }
            } else {
                warn!(
                    "Invalid pay command format from {}: {}",
                    player_name, command
                );
                utils::send_message_to_player(store, player_name, "Usage: pay <player> <amount>. Example: pay Steve 10.5")
                    .await
            }
        }
        Some(&"additem") | Some(&"ai") => {
            let user_uuid = utils::resolve_user_uuid(store, player_name).await?;
            if !utils::is_operator(store, &user_uuid) {
                return utils::send_message_to_player(store, player_name, "This command is only available to operators.")
                    .await;
            }
            if parts.len() >= 3 {
                let item = utils::normalize_item_id(parts[1]);
                let quantity: u32 = parts[2].parse().map_err(|_| "Invalid quantity".to_string())?;
                operator::handle_additem_order(store, player_name, &item, quantity).await
            } else {
                utils::send_message_to_player(store, player_name, "Usage: additem <item> <quantity>").await
            }
        }
        Some(&"removeitem") | Some(&"ri") => {
            let user_uuid = utils::resolve_user_uuid(store, player_name).await?;
            if !utils::is_operator(store, &user_uuid) {
                return utils::send_message_to_player(store, player_name, "This command is only available to operators.")
                    .await;
            }
            if parts.len() >= 3 {
                let item = utils::normalize_item_id(parts[1]);
                let quantity: u32 = parts[2].parse().map_err(|_| "Invalid quantity".to_string())?;
                operator::handle_removeitem_order(store, player_name, &item, quantity).await
            } else {
                utils::send_message_to_player(store, player_name, "Usage: removeitem <item> <quantity>").await
            }
        }
        Some(&"deposit") | Some(&"d") => {
            let amount: Option<f64> = if parts.len() >= 2 {
                match parts[1].parse() {
                    Ok(amt) => {
                        if amt <= 0.0 {
                            return utils::send_message_to_player(store, player_name, "Amount must be positive").await;
                        }
                        Some(amt)
                    }
                    Err(_) => {
                        return utils::send_message_to_player(
                            store,
                            player_name,
                            &format!("Invalid amount '{}'. Use a number. Example: deposit 64", parts[1]),
                        ).await;
                    }
                }
            } else {
                None // Flexible deposit - credit whatever player puts in
            };

            debug!(
                "Queueing deposit order: {} amount={:?}",
                player_name, amount
            );

            // Add to order queue
            match store.order_queue.add(
                user_uuid.clone(),
                player_name.to_string(),
                QueuedOrderType::Deposit { amount },
                "diamond".to_string(),
                0, // quantity not used for deposit
            ) {
                Ok((order_id, position)) => {
                    let queue_len = store.order_queue.len();
                    let wait_estimate = store.order_queue.estimate_wait(position);
                    let amount_str = match amount {
                        Some(amt) => format!("{:.2} diamonds", amt),
                        None => "diamonds (flexible)".to_string(),
                    };
                    let msg = format!(
                        "Deposit {} order #{} queued (position {}/{}). Est. wait: {}.",
                        amount_str, order_id, position, queue_len, wait_estimate
                    );
                    utils::send_message_to_player(store, player_name, &msg).await
                }
                Err(e) => utils::send_message_to_player(store, player_name, &e).await,
            }
        }
        Some(&"withdraw") | Some(&"w") => {
            let amount: Option<f64> = if parts.len() >= 2 {
                match parts[1].parse() {
                    Ok(amt) => {
                        if amt <= 0.0 {
                            return utils::send_message_to_player(store, player_name, "Amount must be positive").await;
                        }
                        Some(amt)
                    }
                    Err(_) => {
                        return utils::send_message_to_player(
                            store,
                            player_name,
                            &format!("Invalid amount '{}'. Use a number. Example: withdraw 64", parts[1]),
                        ).await;
                    }
                }
            } else {
                None // Withdraw full balance (whole diamonds only)
            };

            debug!(
                "Queueing withdraw order: {} amount={:?}",
                player_name, amount
            );

            // Add to order queue
            match store.order_queue.add(
                user_uuid.clone(),
                player_name.to_string(),
                QueuedOrderType::Withdraw { amount },
                "diamond".to_string(),
                0, // quantity not used for withdraw
            ) {
                Ok((order_id, position)) => {
                    let queue_len = store.order_queue.len();
                    let wait_estimate = store.order_queue.estimate_wait(position);
                    let amount_str = match amount {
                        Some(amt) => format!("{:.2} diamonds", amt),
                        None => "full balance".to_string(),
                    };
                    let msg = format!(
                        "Withdraw {} order #{} queued (position {}/{}). Est. wait: {}.",
                        amount_str, order_id, position, queue_len, wait_estimate
                    );
                    utils::send_message_to_player(store, player_name, &msg).await
                }
                Err(e) => utils::send_message_to_player(store, player_name, &e).await,
            }
        }
        Some(&"addcurrency") | Some(&"ac") => {
            let user_uuid = utils::resolve_user_uuid(store, player_name).await?;
            if !utils::is_operator(store, &user_uuid) {
                return utils::send_message_to_player(store, player_name, "This command is only available to operators.")
                    .await;
            }
            if parts.len() >= 3 {
                let item = utils::normalize_item_id(parts[1]);
                let amount: f64 = parts[2].parse().map_err(|_| "Invalid amount".to_string())?;
                operator::handle_add_currency(store, player_name, &item, amount).await
            } else {
                utils::send_message_to_player(store, player_name, "Usage: addcurrency <item> <amount>").await
            }
        }
        Some(&"removecurrency") | Some(&"rc") => {
            let user_uuid = utils::resolve_user_uuid(store, player_name).await?;
            if !utils::is_operator(store, &user_uuid) {
                return utils::send_message_to_player(store, player_name, "This command is only available to operators.")
                    .await;
            }
            if parts.len() >= 3 {
                let item = utils::normalize_item_id(parts[1]);
                let amount: f64 = parts[2].parse().map_err(|_| "Invalid amount".to_string())?;
                operator::handle_remove_currency(store, player_name, &item, amount).await
            } else {
                utils::send_message_to_player(store, player_name, "Usage: removecurrency <item> <amount>").await
            }
        }
        Some(&"price") | Some(&"p") => {
            if parts.len() >= 2 {
                // Validate item name
                if let Err(e) = validate_item_name(parts[1]) {
                    return utils::send_message_to_player(store, player_name, &e).await;
                }
                let item = utils::normalize_item_id(parts[1]);
                
                // Get optional quantity (default to stack size from pair, or 64 if not found)
                let quantity: Option<u32> = if parts.len() >= 3 {
                    match parts[2].parse() {
                        Ok(q) if q > 0 => Some(q),
                        _ => {
                            return utils::send_message_to_player(
                                store,
                                player_name,
                                &format!("Invalid quantity '{}'. Use a positive number.", parts[2]),
                            ).await;
                        }
                    }
                } else {
                    None // Will use stack size
                };
                
                handle_price_command(store, player_name, &item, quantity).await
            } else {
                utils::send_message_to_player(
                    store,
                    player_name,
                    "Usage: price <item> [quantity]. Example: price cobblestone 64",
                ).await
            }
        }
        Some(&"help") | Some(&"h") => {
            // Check if help is requested for a specific command
            if parts.len() >= 2 {
                handle_help_command(store, player_name, Some(parts[1])).await
            } else {
                handle_help_command(store, player_name, None).await
            }
        }
        Some(&"items") => {
            // Get optional page number (default to 1)
            let page: usize = if parts.len() >= 2 {
                parts[1].parse().unwrap_or(1).max(1)
            } else {
                1
            };
            handle_items_command(store, player_name, page).await
        }
        Some(&"queue") | Some(&"q") => {
            // Get optional page number (default to 1)
            let page: usize = if parts.len() >= 2 {
                parts[1].parse().unwrap_or(1).max(1)
            } else {
                1
            };
            
            // Show user's queued orders with pagination
            let user_orders = store.order_queue.get_user_orders(&user_uuid);
            
            if user_orders.is_empty() {
                let total_queue = store.order_queue.len();
                let msg = if total_queue > 0 {
                    format!("You have no orders queued. ({} orders in queue from other players)", total_queue)
                } else {
                    "You have no orders queued. Queue is empty.".to_string()
                };
                utils::send_message_to_player(store, player_name, &msg).await
            } else {
                // Paginate: max 4 orders per page
                const ORDERS_PER_PAGE: usize = 4;
                let total_user_orders = user_orders.len();
                let total_pages = (total_user_orders + ORDERS_PER_PAGE - 1) / ORDERS_PER_PAGE;
                
                // Validate page number
                if page > total_pages {
                    return utils::send_message_to_player(
                        store,
                        player_name,
                        &format!("Invalid page. You have {} order(s), use 'queue 1' to 'queue {}'.", total_user_orders, total_pages),
                    ).await;
                }
                
                // Get the slice for this page
                let start_idx = (page - 1) * ORDERS_PER_PAGE;
                let end_idx = (start_idx + ORDERS_PER_PAGE).min(total_user_orders);
                let page_orders = &user_orders[start_idx..end_idx];
                
                // Format orders for this page
                let orders_str: Vec<String> = page_orders
                    .iter()
                    .map(|(order, pos)| format!("#{} {} (pos {})", order.id, order.description(), pos))
                    .collect();
                
                let total_queue = store.order_queue.len();
                let msg = if total_pages == 1 {
                    format!(
                        "Your queue ({}/{}): {}",
                        total_user_orders,
                        total_queue,
                        orders_str.join(", ")
                    )
                } else {
                    format!(
                        "Your queue (page {}/{}, {}/{}): {}",
                        page, total_pages,
                        total_user_orders,
                        total_queue,
                        orders_str.join(", ")
                    )
                };
                utils::send_message_to_player(store, player_name, &msg).await
            }
        }
        Some(&"cancel") | Some(&"c") => {
            if parts.len() >= 2 {
                // Parse order ID
                let order_id: u64 = match parts[1].trim_start_matches('#').parse() {
                    Ok(id) => id,
                    Err(_) => {
                        return utils::send_message_to_player(
                            store,
                            player_name,
                            &format!("Invalid order ID '{}'. Use: cancel <order_id>", parts[1]),
                        ).await;
                    }
                };

                // Check if this order is currently being processed
                if let Some(ref trade) = store.current_trade {
                    if trade.order().id == order_id {
                        return utils::send_message_to_player(
                            store,
                            player_name,
                            &format!("Order #{} is currently being processed ({}) and cannot be cancelled.", order_id, trade.phase()),
                        ).await;
                    }
                }

                match store.order_queue.cancel(&user_uuid, order_id) {
                    Ok(()) => {
                        let msg = format!("Order #{} cancelled.", order_id);
                        utils::send_message_to_player(store, player_name, &msg).await
                    }
                    Err(e) => utils::send_message_to_player(store, player_name, &e).await,
                }
            } else {
                utils::send_message_to_player(
                    store,
                    player_name,
                    "Usage: cancel <order_id>. Use 'queue' to see your orders.",
                ).await
            }
        }
        Some(&"status") => {
            handle_status_command(store, player_name).await
        }
        Some(unknown_cmd) => {
            warn!("Unknown command '{}' from {}", unknown_cmd, player_name);
            utils::send_message_to_player(
                store,
                player_name,
                &format!("Unknown command '{}'. Use 'help' to see available commands.", unknown_cmd),
            ).await
        }
        None => {
            warn!("Empty command received from {}", player_name);
            utils::send_message_to_player(
                store,
                player_name,
                "Use 'help' to see available commands.",
            ).await
        }
    }
}

/// Handle the price command - shows buy/sell prices for an item
/// 
/// Prices are calculated using constant product AMM formula (x * y = k).
/// The price depends on trade size (slippage), so we show the total cost/payout
/// and average price per item for the requested quantity.
async fn handle_price_command(
    store: &mut Store,
    player_name: &str,
    item: &str,
    quantity: Option<u32>,
) -> Result<(), String> {
    use super::super::pricing;
    
    // Check if pair exists
    let pair = match store.pairs.get(item) {
        Some(p) => p,
        None => {
            return utils::send_message_to_player(
                store,
                player_name,
                &format!("Item '{}' is not available for trading.", item),
            ).await;
        }
    };
    
    // Determine quantity (default to stack size)
    let qty = quantity.unwrap_or(pair.stack_size as u32);
    let qty_i32 = qty as i32;
    
    // Get prices using constant product formula (price depends on trade size)
    let buy_total = pricing::calculate_buy_cost(store, item, qty_i32);
    let sell_total = pricing::calculate_sell_payout(store, item, qty_i32);
    
    match (buy_total, sell_total) {
        (Some(buy_cost), Some(sell_payout)) => {
            let buy_per = buy_cost / (qty as f64);
            let sell_per = sell_payout / (qty as f64);
            let message = format!(
                "{} x{}: Buy for {:.2} diamonds ({:.4}/ea), Sell for {:.2} diamonds ({:.4}/ea). Stock: {}",
                item, qty, buy_cost, buy_per, sell_payout, sell_per, pair.item_stock
            );
            utils::send_message_to_player(store, player_name, &message).await
        }
        (None, Some(sell_payout)) => {
            // Can sell but can't buy (probably trying to buy too much)
            let sell_per = sell_payout / (qty as f64);
            let message = format!(
                "{} x{}: Buy unavailable (exceeds stock {}), Sell for {:.2} diamonds ({:.4}/ea)",
                item, qty, pair.item_stock, sell_payout, sell_per
            );
            utils::send_message_to_player(store, player_name, &message).await
        }
        _ => {
            // Reserves insufficient for price calculation
            let message = if pair.item_stock == 0 {
                format!("{}: No stock available (item_stock: 0)", item)
            } else if pair.currency_stock <= 0.0 {
                format!("{}: No currency reserve (currency_stock: 0)", item)
            } else {
                format!("{}: Price unavailable (insufficient reserves)", item)
            };
            utils::send_message_to_player(store, player_name, &message).await
        }
    }
}

/// Handle the status command - shows what the bot is currently doing
async fn handle_status_command(
    store: &mut Store,
    player_name: &str,
) -> Result<(), String> {
    let queue_len = store.order_queue.len();
    
    // Determine current activity
    let status_msg = if store.processing_order {
        // Bot is actively processing an order
        if let Some(ref trade) = store.current_trade {
            let activity = format!("{} [{}]", trade, trade.phase());

            if queue_len > 0 {
                format!("Status: {}. {} order(s) waiting in queue.", activity, queue_len)
            } else {
                format!("Status: {}.", activity)
            }
        } else {
            // Shouldn't happen, but handle gracefully
            if queue_len > 0 {
                format!("Status: Processing order. {} order(s) waiting in queue.", queue_len)
            } else {
                "Status: Processing order.".to_string()
            }
        }
    } else if queue_len > 0 {
        // Not processing but queue has items (shouldn't normally happen, might be starting up)
        format!("Status: Ready. {} order(s) in queue, processing will start shortly.", queue_len)
    } else {
        // Idle - no processing, empty queue
        "Status: Idle. No orders being processed. Queue is empty.".to_string()
    };
    
    utils::send_message_to_player(store, player_name, &status_msg).await
}

/// Handle the items command - lists available trading pairs with pagination
async fn handle_items_command(
    store: &mut Store,
    player_name: &str,
    page: usize,
) -> Result<(), String> {
    // Get all available pairs
    let items: Vec<String> = store.pairs.keys().cloned().collect();
    
    if items.is_empty() {
        return utils::send_message_to_player(
            store,
            player_name,
            "No items available for trading.",
        ).await;
    }
    
    // Sort items alphabetically for consistent display
    let mut sorted_items = items;
    sorted_items.sort();
    
    // Split into pages of 4 items each
    const ITEMS_PER_PAGE: usize = 4;
    let chunks: Vec<Vec<String>> = sorted_items
        .chunks(ITEMS_PER_PAGE)
        .map(|chunk| chunk.to_vec())
        .collect();
    
    let total_pages = chunks.len();
    
    // Validate page number
    if page > total_pages {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Invalid page. Use 'items 1' to 'items {}'.", total_pages),
        ).await;
    }
    
    // Get the requested page (1-indexed)
    let page_items = &chunks[page - 1];
    let items_list = page_items.join(", ");
    
    let message = if total_pages == 1 {
        format!("Items: {}", items_list)
    } else {
        format!("Items (page {}/{}): {}", page, total_pages, items_list)
    };
    
    utils::send_message_to_player(store, player_name, &message).await
}

/// Handle the help command - shows available commands and their usage
async fn handle_help_command(
    store: &mut Store,
    player_name: &str,
    command: Option<&str>,
) -> Result<(), String> {
    let user_uuid = utils::resolve_user_uuid(store, player_name).await.ok();
    let is_op = user_uuid.as_ref().map(|u| utils::is_operator(store, u)).unwrap_or(false);
    
    match command {
        Some("buy") | Some("b") => {
            utils::send_message_to_player(
                store,
                player_name,
                "buy <item> <quantity> - Buy items from the store. Example: buy cobblestone 64",
            ).await
        }
        Some("sell") | Some("s") => {
            utils::send_message_to_player(
                store,
                player_name,
                "sell <item> <quantity> - Sell items to the store. Example: sell iron_ingot 128",
            ).await
        }
        Some("price") | Some("p") => {
            utils::send_message_to_player(
                store,
                player_name,
                "price <item> [quantity] - Check buy/sell prices. Defaults to one stack. Example: price cobblestone 64",
            ).await
        }
        Some("balance") | Some("bal") => {
            utils::send_message_to_player(
                store,
                player_name,
                "balance [player] (or bal) - Check your balance, or another player's. Example: bal Steve",
            ).await
        }
        Some("pay") => {
            utils::send_message_to_player(
                store,
                player_name,
                "pay <player> <amount> - Pay diamonds to another player. Example: pay Steve 10.5",
            ).await
        }
        Some("deposit") | Some("d") => {
            utils::send_message_to_player(
                store,
                player_name,
                "deposit [amount] - Deposit physical diamonds into your balance. If no amount specified, credits whatever you put in the trade (max 768 / 12 stacks). Example: deposit, deposit 64",
            ).await
        }
        Some("withdraw") | Some("w") => {
            utils::send_message_to_player(
                store,
                player_name,
                "withdraw [amount] - Withdraw diamonds from your balance. If no amount specified, withdraws your full balance (whole diamonds only, max 768 / 12 stacks). Example: withdraw, withdraw 32",
            ).await
        }
        Some("items") => {
            utils::send_message_to_player(
                store,
                player_name,
                "items [page] - List available items for trading. Shows 4 items per page. Example: items 2",
            ).await
        }
        Some("queue") | Some("q") => {
            utils::send_message_to_player(
                store,
                player_name,
                "queue [page] (or q) - Show your pending orders (4 per page). Example: queue, queue 2",
            ).await
        }
        Some("cancel") | Some("c") => {
            utils::send_message_to_player(
                store,
                player_name,
                "cancel <order_id> (or c) - Cancel a pending order. Use 'queue' to see your order IDs. Example: c 5",
            ).await
        }
        Some("status") => {
            utils::send_message_to_player(
                store,
                player_name,
                "status - Check what the bot is currently doing (idle, buying, selling, etc.) and queue status.",
            ).await
        }
        Some("additem") | Some("ai") if is_op => {
            utils::send_message_to_player(
                store,
                player_name,
                "additem <item> <quantity> - (Operator) Add items to store stock. Example: additem diamond 100",
            ).await
        }
        Some("removeitem") | Some("ri") if is_op => {
            utils::send_message_to_player(
                store,
                player_name,
                "removeitem <item> <quantity> - (Operator) Remove items from store stock. Example: removeitem coal 50",
            ).await
        }
        Some("addcurrency") | Some("ac") if is_op => {
            utils::send_message_to_player(
                store,
                player_name,
                "addcurrency <item> <amount> - (Operator) Add diamonds to item's reserve. Example: addcurrency cobblestone 1000",
            ).await
        }
        Some("removecurrency") | Some("rc") if is_op => {
            utils::send_message_to_player(
                store,
                player_name,
                "removecurrency <item> <amount> - (Operator) Remove diamonds from item's reserve. Example: removecurrency cobblestone 500",
            ).await
        }
        Some(cmd) => {
            // Unknown command for help, or operator command requested by non-operator
            utils::send_message_to_player(
                store,
                player_name,
                &format!("Unknown command '{}'. Use 'help' to see available commands.", cmd),
            ).await
        }
        None => {
            // Show list of available commands
            let base_commands = "Commands: buy (b), sell (s), price (p), items, balance (bal), pay, deposit (d), withdraw (w), queue (q), cancel (c), status, help (h). Use 'help <command>' for details.";
            if is_op {
                utils::send_message_to_player(
                    store,
                    player_name,
                    &format!("{} Operator: additem (ai), removeitem (ri), addcurrency (ac), removecurrency (rc)", base_commands),
                ).await
            } else {
                utils::send_message_to_player(store, player_name, base_commands).await
            }
        }
    }
}

/// Get user balance asynchronously
async fn get_user_balance_async(store: &mut Store, username: &str) -> Result<f64, String> {
    state::assert_invariants(store, "pre-balance", false)?;
    let uuid = utils::resolve_user_uuid(store, username).await?;
    utils::ensure_user_exists(store, username, &uuid);
    let bal = store.users.get(&uuid).map(|u| u.balance).unwrap_or(0.0);
    if !bal.is_finite() || bal < 0.0 {
        return Err("Internal error: invalid stored balance".to_string());
    }
    Ok(bal)
}

/// Handle payment between players
pub async fn pay_async(
    store: &mut Store,
    payer_username: &str,
    payee_username: &str,
    amount: f64,
) -> Result<(), String> {
    state::assert_invariants(store, "pre-pay", false)?;
    if !amount.is_finite() || amount <= 0.0 {
        warn!("Invalid payment amount attempted: {}", amount);
        return Err("Amount must be positive".to_string());
    }

    let payer_uuid = utils::resolve_user_uuid(store, payer_username).await?;
    let payee_uuid = utils::resolve_user_uuid(store, payee_username).await?;

    // Ensure payer exists
    if !store.users.contains_key(&payer_uuid) {
        return Err(format!("Payer '{}' not found in store records", payer_username));
    }

    // Ensure payee exists
    utils::ensure_user_exists(store, payee_username, &payee_uuid);

    let payer_balance = store.expect_user(&payer_uuid, "pay/payer-balance")?.balance;
    if payer_balance < amount {
        warn!(
            "Insufficient balance for payment: {} has {}, needs {}",
            payer_username, payer_balance, amount
        );
        return Err(format!(
            "Insufficient balance. Required: {}, Available: {}",
            amount, payer_balance
        ));
    }

    // Transfer
    {
        let payer = store.expect_user_mut(&payer_uuid, "pay/payer-debit")?;
        payer.balance -= amount;
        payer.username = payer_username.to_owned();
    }
    {
        let payee = store.expect_user_mut(&payee_uuid, "pay/payee-credit")?;
        payee.balance += amount;
        payee.username = payee_username.to_owned();
    }
    store.dirty = true;

    state::assert_invariants(store, "post-pay", true)?;
    Ok(())
}

/// Handle deposit balance (user deposits diamonds to their balance)
/// 
/// If `amount` is Some, expects exactly that many diamonds (ceil'd to whole diamonds).
/// If `amount` is None, flexible deposit - credits whatever the player puts in (up to 12 stacks = 768 diamonds).
///
/// This is a public function called by the order queue processor.
pub async fn handle_deposit_balance_queued(
    store: &mut Store,
    player_name: &str,
    amount: Option<f64>,
) -> Result<(), String> {
    info!("[Deposit] Starting: player={} amount={:?}", player_name, amount);
    state::assert_invariants(store, "pre-deposit-balance", false)?;

    // Maximum diamonds the trade GUI can hold (12 stacks of 64 = 768).
    // Rationale: Minecraft's vanilla trade/offer UI exposes 12 offer slots
    // (4x3 grid on each side). Each slot holds at most one 64-stack of
    // diamonds, so a single trade round-trip can move at most 768 diamonds.
    // Requests larger than this cannot fit into one trade window and must
    // be split - we reject them at the handler rather than silently
    // truncating so the player isn't surprised by a partial transaction.
    const MAX_TRADE_DIAMONDS: i32 = 12 * 64; // 768
    
    // Determine if this is a fixed-amount or flexible deposit
    let (diamonds_to_trade, is_flexible) = match amount {
        Some(amt) => {
            if !amt.is_finite() || amt <= 0.0 {
                return utils::send_message_to_player(store, player_name, "Amount must be positive")
                    .await;
            }
            let diamonds = amt.ceil() as i32;
            if diamonds > MAX_TRADE_DIAMONDS {
                return utils::send_message_to_player(
                    store,
                    player_name,
                    &format!("Amount too large. Maximum deposit is {} diamonds (12 stacks).", MAX_TRADE_DIAMONDS),
                )
                .await;
            }
            (diamonds, false)
        }
        None => {
            // Flexible deposit - expect up to max, credit whatever they actually give
            (MAX_TRADE_DIAMONDS, true)
        }
    };

    let user_uuid = utils::resolve_user_uuid(store, player_name).await?;
    utils::ensure_user_exists(store, player_name, &user_uuid);

    // Notify player before trade
    let msg = if is_flexible {
        format!("Deposit: Please offer diamonds in the trade (up to {} diamonds / 12 stacks). You'll be credited for the actual amount.", MAX_TRADE_DIAMONDS)
    } else {
        format!("Deposit {:.2} diamonds: Please offer {} diamonds in the trade.", amount.unwrap(), diamonds_to_trade)
    };
    utils::send_message_to_player(store, player_name, &msg).await?;

    // Advance: Queued -> Withdrawing (empty plan) -> Trading
    store.advance_trade(|s| s.begin_withdrawal(vec![]));
    store.advance_trade(|s| s.begin_trading());

    // Perform trade: player offers diamonds, bot offers nothing
    info!(
        "[Deposit] Initiating trade: {} offers up to {} diamonds (flexible={})",
        player_name, diamonds_to_trade, is_flexible
    );
    let (trade_tx, trade_rx) = tokio::sync::oneshot::channel();
    let send_result = store.bot_tx
        .send(crate::messages::BotInstruction::TradeWithPlayer {
            target_username: player_name.to_string(),
            bot_offers: vec![],
            // For flexible deposits: expect at least 1 diamond (any amount OK)
            // For fixed deposits: expect exact amount
            player_offers: vec![crate::messages::TradeItem {
                item: "diamond".to_string(),
                // Flexible: minimum 1, Fixed: exact amount requested
                amount: if is_flexible { 1 } else { diamonds_to_trade },
            }],
            // Deposit: accept if player offers at least the required amount (surplus OK)
            require_exact_amount: false,
            // Flexible deposit: accept any amount >= 1 diamond
            flexible_validation: is_flexible,
            respond_to: trade_tx,
        })
        .await;
    
    if let Err(e) = send_result {
        tracing::error!("[Deposit] Failed to send trade instruction: {}", e);
        return Err(format!("Failed to send trade instruction to bot: {}", e));
    }

    let trade_result = match tokio::time::timeout(
        tokio::time::Duration::from_millis(store.config.trade_timeout_ms),
        trade_rx,
    )
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => {
            tracing::error!("[Deposit] Trade channel dropped: {}", e);
            return Err(format!("Bot response dropped: {}", e));
        }
        Err(_) => {
            tracing::error!("[Deposit] Trade timeout");
            return Err("Bot timed out waiting for trade completion".to_string());
        }
    };

    let actual_received = match trade_result {
        Err(err) => {
            warn!("[Deposit] Trade failed: {}", err);
            return utils::send_message_to_player(
                store,
                player_name,
                &format!("Deposit aborted: trade failed: {}", err),
            )
            .await;
        }
        Ok(received) => received,
    };
    
    // Calculate actual diamonds received
    let diamonds_actually_received: i32 = actual_received
        .iter()
        .filter(|t| t.item == "diamond")
        .map(|t| t.amount)
        .sum();
    
    // If player deposited 0 diamonds, abort
    if diamonds_actually_received <= 0 {
        return utils::send_message_to_player(
            store,
            player_name,
            "Deposit aborted: no diamonds received in trade",
        )
        .await;
    }

    // Deposit the received diamonds into storage (diamond chest).
    // Uses the actual amount received, not the originally requested amount.
    // Per-step errors are tolerated here: the user is still credited because
    // the trade completed, and any stuck diamonds will be logged and need
    // operator attention via the overflow chest.
    let rb = super::super::rollback::rollback_amount_to_storage(
        store,
        "diamond",
        diamonds_actually_received,
        64,
        "[Deposit]",
    )
    .await;
    if rb.has_failures() {
        tracing::warn!(
            "[Deposit] Failed to deposit {} diamond step(s) into storage - some diamonds may remain in bot inventory",
            rb.operations_failed
        );
    }

    // Add to balance - credit the ACTUAL diamonds received, not the requested amount
    let actual_amount = diamonds_actually_received as f64;
    let new_balance = {
        let user = store.expect_user_mut(&user_uuid, "deposit-balance/credit")?;
        user.balance += actual_amount;
        user.username = player_name.to_owned();
        user.balance
    };
    store.dirty = true;

    // Record order
    store.orders.push_back(crate::types::Order {
        order_type: crate::types::order::OrderType::DepositBalance,
        item: ItemId::from_normalized("diamond".to_string()),
        amount: diamonds_actually_received,
        user_uuid: user_uuid.clone(),
    });

    // Record trade
    store.trades.push(crate::types::Trade::new(
        crate::types::TradeType::DepositBalance,
        ItemId::from_normalized("diamond".to_string()),
        diamonds_actually_received,
        actual_amount,
        user_uuid.clone(),
    ));

    // Advance: Trading -> Committed
    store.advance_trade(|s| s.commit("diamond".to_string(), diamonds_actually_received, actual_amount));

    info!("[Deposit] Completed: user={} amount={}", player_name, actual_amount);

    if let Err(e) = state::assert_invariants(store, "post-deposit-balance", true) {
        tracing::error!("Invariant violation after deposit balance: {}", e);
        let _ = state::save(store);
    }

    utils::send_message_to_player(
        store,
        player_name,
        &format!("Deposited {:.2} diamonds to your balance. New balance: {:.2}", actual_amount, new_balance),
    )
    .await
}

/// Handle withdraw balance (user withdraws diamonds from their balance)
/// 
/// If `amount` is Some, withdraws exactly that amount (floor'd to whole diamonds for trade).
/// If `amount` is None, withdraws the user's full balance (whole diamonds only), capped at 12 stacks = 768 diamonds.
///
/// This is a public function called by the order queue processor.
pub async fn handle_withdraw_balance_queued(
    store: &mut Store,
    player_name: &str,
    amount: Option<f64>,
) -> Result<(), String> {
    info!("[Withdraw] Starting: player={} amount={:?}", player_name, amount);
    state::assert_invariants(store, "pre-withdraw-balance", false)?;

    // Maximum diamonds the trade GUI can hold per transaction: 12 slots
    // times a 64-stack each = 768 diamonds. See handle_deposit_balance_queued
    // for the full rationale - the cap comes from the vanilla trade window
    // layout, not from any arbitrary policy.
    const MAX_TRADE_DIAMONDS: i32 = 12 * 64; // 768

    let user_uuid = utils::resolve_user_uuid(store, player_name).await?;
    utils::ensure_user_exists(store, player_name, &user_uuid);

    let user_balance = store.expect_user(&user_uuid, "withdraw-balance/pre-check")?.balance;
    
    // Determine actual withdrawal amount
    let amount = match amount {
        Some(amt) => {
            if !amt.is_finite() || amt <= 0.0 {
                return utils::send_message_to_player(store, player_name, "Amount must be positive")
                    .await;
            }
            amt
        }
        None => {
            // Full balance withdrawal - only whole diamonds, capped at max trade capacity
            let whole_balance = user_balance.floor();
            if whole_balance <= 0.0 {
                return utils::send_message_to_player(
                    store,
                    player_name,
                    &format!("No whole diamonds to withdraw. Balance: {:.2} (need at least 1.00)", user_balance),
                ).await;
            }
            // Cap at maximum trade capacity
            whole_balance.min(MAX_TRADE_DIAMONDS as f64)
        }
    };

    if user_balance < amount {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!(
                "Insufficient balance. Required: {:.2}, Available: {:.2}",
                amount, user_balance
            ),
        )
        .await;
    }

    // Calculate whole diamonds to trade (round down)
    let whole_diamonds = amount.floor() as i32;
    
    // Cap at maximum trade capacity
    if whole_diamonds > MAX_TRADE_DIAMONDS {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Amount too large. Maximum withdrawal is {} diamonds (12 stacks) per transaction.", MAX_TRADE_DIAMONDS),
        )
        .await;
    }

    // Notify player before trade
    let withdraw_msg = if whole_diamonds > 0 {
        format!("Withdraw {:.2} diamonds: You'll receive {} diamonds in trade.", amount, whole_diamonds)
    } else {
        format!("Withdraw {:.2} diamonds: Amount too small for trade (must be at least 1 whole diamond).", amount)
    };
    utils::send_message_to_player(store, player_name, &withdraw_msg).await?;

    // If no whole diamonds to trade, just return error
    if whole_diamonds <= 0 {
        return utils::send_message_to_player(
            store,
            player_name,
            "Withdraw requires at least 1 whole diamond. Use a larger amount.",
        ).await;
    }

    // Advance: Queued -> Withdrawing (diamonds from storage)
    store.advance_trade(|s| s.begin_withdrawal(vec![]));

    // Withdraw diamonds from storage (diamond chest) before trading.
    // Uses the non-mutating `simulate_withdraw_plan` so we can validate
    // feasibility without cloning all of storage.
    if whole_diamonds > 0 {
        let (withdraw_plan, preview_withdrawn) =
            store.storage.simulate_withdraw_plan("diamond", whole_diamonds);

        if preview_withdrawn < whole_diamonds {
            tracing::error!(
                "[Withdraw] Insufficient physical diamonds: need {}, storage has {}",
                whole_diamonds, preview_withdrawn
            );
            return utils::send_message_to_player(
                store,
                player_name,
                &format!(
                    "Insufficient physical diamonds in storage. Storage has {}, need {}.",
                    preview_withdrawn, whole_diamonds
                ),
            )
            .await;
        }

        for t in &withdraw_plan {
            let node_position = store.get_node_position(t.chest_id);
            let chest = crate::types::Chest {
                id: t.chest_id,
                node_id: t.chest_id / 4,
                index: t.chest_id % 4,
                position: t.position,
                item: t.item.clone(),
                amounts: vec![0; crate::types::Storage::SLOTS_PER_CHEST],
            };

            let (tx, rx) = tokio::sync::oneshot::channel();
            let send_result = store.bot_tx
                .send(crate::messages::BotInstruction::InteractWithChestAndSync {
                    target_chest: chest,
                    node_position,
                    action: crate::messages::ChestAction::Withdraw {
                        item: "diamond".to_string(),
                        amount: t.amount,
                        to_player: None,
                        stack_size: 64, // Diamonds stack to 64
                    },
                    respond_to: tx,
                })
                .await;
            
            if let Err(e) = send_result {
                tracing::error!("[Withdraw] Failed to send chest instruction: {}", e);
                return Err(format!("Failed to send chest instruction to bot: {}", e));
            }

            let bot_result = match tokio::time::timeout(
                tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS),
                rx,
            )
            .await
            {
                Ok(Ok(result)) => result,
                Ok(Err(e)) => {
                    tracing::error!("[Withdraw] Channel dropped: {}", e);
                    return Err(format!("Bot response dropped: {}", e));
                }
                Err(_) => {
                    tracing::error!("[Withdraw] Timeout waiting for bot");
                    return Err("Bot timed out withdrawing diamonds from storage".to_string());
                }
            };

            match bot_result {
                Err(err) => {
                    tracing::error!("[Withdraw] Diamond withdrawal failed: {}", err);
                    return utils::send_message_to_player(
                        store,
                        player_name,
                        &format!("Withdraw aborted: failed to get diamonds from storage: {}", err),
                    )
                    .await;
                }
                Ok(report) => {
                    if let Err(e) = store.apply_chest_sync(report) {
                        tracing::warn!("[Withdraw] Chest sync failed after diamond withdrawal: {}", e);
                    }
                }
            }
        }
    }

    // NOTE: Do NOT deduct balance yet - wait until trade succeeds.
    // This prevents the bug where balance is deducted even if trade is rejected.
    //
    // Ordering of withdraw steps (important for rollback correctness):
    //   1. Pull diamonds out of the storage chests into bot inventory (done above).
    //   2. Attempt the trade handoff to the player.
    //   3. ONLY on successful trade, subtract from the user's balance.
    //
    // If step 2 fails for ANY reason (send error, channel drop, timeout,
    // trade rejected by player), the rollback path below redeposits the
    // physical diamonds back into storage and returns early - and because
    // the balance was never touched, no balance restoration is needed.
    // This keeps physical diamonds (chests) and ledger balance in lockstep
    // so a user cannot lose diamonds to a failed/cancelled trade.

    // Advance: Withdrawing -> Trading
    store.advance_trade(|s| s.begin_trading());

    // Perform trade: bot offers whole diamonds, player offers nothing
    info!("[Withdraw] Initiating trade: {} diamonds to {}", whole_diamonds, player_name);
    let (trade_tx, trade_rx) = tokio::sync::oneshot::channel();
    let trade_send_result = store.bot_tx
        .send(crate::messages::BotInstruction::TradeWithPlayer {
            target_username: player_name.to_string(),
            bot_offers: if whole_diamonds > 0 {
                vec![crate::messages::TradeItem {
                    item: "diamond".to_string(),
                    amount: whole_diamonds,
                }]
            } else {
                vec![]
            },
            player_offers: vec![],
            // Withdraw: player offers nothing
            require_exact_amount: false,
            flexible_validation: false,
            respond_to: trade_tx,
        })
        .await;
    
    if let Err(e) = trade_send_result {
        tracing::error!("[Withdraw] Failed to send trade instruction: {}", e);
        // Rollback: deposit diamonds back into storage (balance was NOT deducted)
        let _ = super::super::rollback::rollback_amount_to_storage(
            store,
            "diamond",
            whole_diamonds,
            64,
            "[Withdraw] trade-send-failed",
        )
        .await;
        return Err(format!("Failed to send trade instruction to bot: {}", e));
    }

    let trade_result = match tokio::time::timeout(
        tokio::time::Duration::from_millis(store.config.trade_timeout_ms),
        trade_rx,
    )
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => {
            tracing::error!("[Withdraw] Trade channel dropped: {}", e);
            let _ = super::super::rollback::rollback_amount_to_storage(
                store,
                "diamond",
                whole_diamonds,
                64,
                "[Withdraw] channel-dropped",
            )
            .await;
            return utils::send_message_to_player(
                store,
                player_name,
                &format!("Withdraw aborted: bot response dropped: {}", e),
            )
            .await;
        }
        Err(_) => {
            tracing::error!("[Withdraw] Trade timeout");
            let _ = super::super::rollback::rollback_amount_to_storage(
                store,
                "diamond",
                whole_diamonds,
                64,
                "[Withdraw] timeout",
            )
            .await;
            return utils::send_message_to_player(
                store,
                player_name,
                "Withdraw aborted: bot timed out waiting for trade completion. Diamonds returned to storage.",
            )
            .await;
        }
    };

    if let Err(ref err) = trade_result {
        warn!("[Withdraw] Trade failed: {}", err);
        // Balance was NOT deducted yet, so no need to restore it; only need to put
        // the physical diamonds back.
        let _ = super::super::rollback::rollback_amount_to_storage(
            store,
            "diamond",
            whole_diamonds,
            64,
            "[Withdraw] trade-failed",
        )
        .await;

        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Withdraw aborted: trade failed: {}. Diamonds returned to storage.", err),
        )
        .await;
    }

    // Trade succeeded - NOW deduct from balance
    {
        let user = store.expect_user_mut(&user_uuid, "withdraw-balance/commit")?;
        user.balance -= amount;
        user.username = player_name.to_owned();
    }
    store.dirty = true;

    // Record order
    store.orders.push_back(crate::types::Order {
        order_type: crate::types::order::OrderType::WithdrawBalance,
        item: ItemId::from_normalized("diamond".to_string()),
        amount: whole_diamonds,
        user_uuid: user_uuid.clone(),
    });

    // Record trade
    store.trades.push(crate::types::Trade::new(
        crate::types::TradeType::WithdrawBalance,
        ItemId::from_normalized("diamond".to_string()),
        whole_diamonds,
        amount,
        user_uuid.clone(),
    ));

    // Advance: Trading -> Committed
    store.advance_trade(|s| s.commit("diamond".to_string(), whole_diamonds, amount));

    info!("[Withdraw] Completed: user={} amount={}", player_name, amount);

    if let Err(e) = state::assert_invariants(store, "post-withdraw-balance", true) {
        tracing::error!("Invariant violation after withdraw balance: {}", e);
        let _ = state::save(store);
    }

    let remaining_balance = store.expect_user(&user_uuid, "withdraw-balance/post-read")?.balance;
    utils::send_message_to_player(
        store,
        player_name,
        &format!("Withdrew {:.2} diamonds from your balance ({} whole diamonds in trade). Remaining balance: {:.2}", amount, whole_diamonds, remaining_balance),
    )
    .await
}
