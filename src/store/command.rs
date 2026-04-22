//! Typed player-command parser.
//!
//! `parse_command` converts a raw whispered string like `"buy cobblestone 64"`
//! into a structured [`Command`]. The dispatcher in `handlers::player` can
//! then match on the enum variant instead of peeking into a `Vec<&str>` by
//! index, which makes the control flow explicit and the argument types
//! obvious at the call site.
//!
//! Parsing is a pure function of the input string — it does not touch the
//! `Store` and therefore does not need async or borrowing. Authorization
//! (operator-only commands) stays in the dispatcher: `parse_command` accepts
//! any syntactically-valid operator command and lets the dispatcher reject
//! it for non-operators, so the error message can be consistent with the
//! rest of the permission system.

use super::handlers::validation::{validate_item_name, validate_quantity, validate_username};
use crate::types::ItemId;

/// A parsed player command.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    // Order commands (enqueued for the bot task to process)
    Buy { item: String, quantity: u32 },
    Sell { item: String, quantity: u32 },
    Deposit { amount: Option<f64> },
    Withdraw { amount: Option<f64> },
    // Quick commands (handled inline on the Store task)
    Price { item: String, quantity: Option<u32> },
    Balance { target: Option<String> },
    Pay { target: String, amount: f64 },
    Items { page: usize },
    Queue { page: usize },
    Cancel { order_id: u64 },
    Status,
    Help { topic: Option<String> },
    // Operator commands (permission checked by dispatcher)
    AddItem { item: String, quantity: u32 },
    RemoveItem { item: String, quantity: u32 },
    AddCurrency { item: String, amount: f64 },
    RemoveCurrency { item: String, amount: f64 },
}

/// Parse a raw command string into a [`Command`].
///
/// Returns a user-friendly error message on failure; callers should relay
/// the error verbatim to the player (via `send_message_to_player`).
pub fn parse_command(input: &str) -> Result<Command, String> {
    let parts: Vec<&str> = input.split_whitespace().collect();

    let verb = match parts.first() {
        Some(v) => *v,
        None => {
            return Err("Use 'help' to see available commands.".to_string());
        }
    };

    match verb {
        "buy" | "b" => parse_item_quantity(&parts, "buy").map(|(item, quantity)| Command::Buy { item, quantity }),
        "sell" | "s" => parse_item_quantity(&parts, "sell").map(|(item, quantity)| Command::Sell { item, quantity }),

        "deposit" | "d" => parse_optional_amount(&parts, "deposit").map(|amount| Command::Deposit { amount }),
        "withdraw" | "w" => parse_optional_amount(&parts, "withdraw").map(|amount| Command::Withdraw { amount }),

        "price" | "p" => parse_price(&parts),
        "balance" | "bal" => parse_balance(&parts),
        "pay" => parse_pay(&parts),
        "items" => Ok(Command::Items { page: parse_page(&parts) }),
        "queue" | "q" => Ok(Command::Queue { page: parse_page(&parts) }),
        "cancel" | "c" => parse_cancel(&parts),
        "status" => Ok(Command::Status),
        "help" | "h" => Ok(Command::Help {
            topic: parts.get(1).map(|s| s.to_string()),
        }),

        "additem" | "ai" => parse_item_quantity(&parts, "additem")
            .map(|(item, quantity)| Command::AddItem { item, quantity }),
        "removeitem" | "ri" => parse_item_quantity(&parts, "removeitem")
            .map(|(item, quantity)| Command::RemoveItem { item, quantity }),
        "addcurrency" | "ac" => parse_item_amount(&parts, "addcurrency")
            .map(|(item, amount)| Command::AddCurrency { item, amount }),
        "removecurrency" | "rc" => parse_item_amount(&parts, "removecurrency")
            .map(|(item, amount)| Command::RemoveCurrency { item, amount }),

        unknown => Err(format!(
            "Unknown command '{}'. Use 'help' to see available commands.",
            unknown
        )),
    }
}

fn parse_item_quantity(parts: &[&str], verb: &str) -> Result<(String, u32), String> {
    if parts.len() < 3 {
        return Err(format!("Usage: {} <item> <quantity>. Example: {} cobblestone 64", verb, verb));
    }
    validate_item_name(parts[1])?;
    let item = ItemId::new(parts[1]).map_err(|e| e.to_string())?.to_string();
    let quantity = validate_quantity(parts[2], verb)?;
    Ok((item, quantity))
}

fn parse_item_amount(parts: &[&str], verb: &str) -> Result<(String, f64), String> {
    if parts.len() < 3 {
        return Err(format!("Usage: {} <item> <amount>", verb));
    }
    validate_item_name(parts[1])?;
    let item = ItemId::new(parts[1]).map_err(|e| e.to_string())?.to_string();
    let amount: f64 = parts[2]
        .parse()
        .map_err(|_| format!("Invalid amount '{}'. Please enter a number.", parts[2]))?;
    if !amount.is_finite() {
        return Err("Amount must be a finite number.".to_string());
    }
    Ok((item, amount))
}

fn parse_optional_amount(parts: &[&str], verb: &str) -> Result<Option<f64>, String> {
    if parts.len() < 2 {
        return Ok(None);
    }
    let amt: f64 = parts[1].parse().map_err(|_| {
        format!(
            "Invalid amount '{}'. Use a number. Example: {} 64",
            parts[1], verb
        )
    })?;
    if !amt.is_finite() || amt <= 0.0 {
        return Err("Amount must be positive".to_string());
    }
    Ok(Some(amt))
}

fn parse_price(parts: &[&str]) -> Result<Command, String> {
    if parts.len() < 2 {
        return Err("Usage: price <item> [quantity]. Example: price cobblestone 64".to_string());
    }
    validate_item_name(parts[1])?;
    let item = ItemId::new(parts[1]).map_err(|e| e.to_string())?.to_string();

    let quantity: Option<u32> = if parts.len() >= 3 {
        match parts[2].parse::<u32>() {
            Ok(q) if q > 0 => Some(q),
            _ => {
                return Err(format!(
                    "Invalid quantity '{}'. Use a positive number.",
                    parts[2]
                ));
            }
        }
    } else {
        None
    };

    Ok(Command::Price { item, quantity })
}

fn parse_balance(parts: &[&str]) -> Result<Command, String> {
    let target = if parts.len() >= 2 {
        validate_username(parts[1])?;
        Some(parts[1].to_string())
    } else {
        None
    };
    Ok(Command::Balance { target })
}

fn parse_pay(parts: &[&str]) -> Result<Command, String> {
    if parts.len() < 3 {
        return Err("Usage: pay <player> <amount>. Example: pay Steve 10.5".to_string());
    }
    validate_username(parts[1])?;
    let amount: f64 = parts[2].parse().map_err(|_| {
        format!(
            "Invalid amount '{}'. Please enter a number. Example: pay Steve 10.5",
            parts[2]
        )
    })?;
    if !amount.is_finite() || amount <= 0.0 {
        return Err("Amount must be positive. Example: pay Steve 10.5".to_string());
    }
    if amount > 1_000_000.0 {
        return Err("Amount too large. Maximum is 1,000,000 per payment.".to_string());
    }
    Ok(Command::Pay {
        target: parts[1].to_string(),
        amount,
    })
}

fn parse_page(parts: &[&str]) -> usize {
    if parts.len() >= 2 {
        parts[1].parse().unwrap_or(1).max(1)
    } else {
        1
    }
}

fn parse_cancel(parts: &[&str]) -> Result<Command, String> {
    if parts.len() < 2 {
        return Err("Usage: cancel <order_id>. Use 'queue' to see your orders.".to_string());
    }
    let order_id: u64 = parts[1]
        .trim_start_matches('#')
        .parse()
        .map_err(|_| format!("Invalid order ID '{}'. Use: cancel <order_id>", parts[1]))?;
    Ok(Command::Cancel { order_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty() {
        let err = parse_command("").unwrap_err();
        assert!(err.contains("help"));
        let err = parse_command("   ").unwrap_err();
        assert!(err.contains("help"));
    }

    #[test]
    fn test_parse_unknown() {
        let err = parse_command("teleport").unwrap_err();
        assert!(err.contains("Unknown command"));
    }

    #[test]
    fn test_parse_buy() {
        assert_eq!(
            parse_command("buy cobblestone 64").unwrap(),
            Command::Buy {
                item: "cobblestone".to_string(),
                quantity: 64
            }
        );
        // Alias
        assert_eq!(
            parse_command("b diamond 1").unwrap(),
            Command::Buy {
                item: "diamond".to_string(),
                quantity: 1
            }
        );
        // minecraft: prefix is stripped
        assert_eq!(
            parse_command("buy minecraft:iron_ingot 32").unwrap(),
            Command::Buy {
                item: "iron_ingot".to_string(),
                quantity: 32
            }
        );
        // Missing args
        assert!(parse_command("buy").is_err());
        assert!(parse_command("buy cobblestone").is_err());
        // Bad quantity
        assert!(parse_command("buy cobblestone abc").is_err());
        assert!(parse_command("buy cobblestone 0").is_err());
    }

    #[test]
    fn test_parse_sell() {
        assert_eq!(
            parse_command("sell iron_ingot 128").unwrap(),
            Command::Sell {
                item: "iron_ingot".to_string(),
                quantity: 128
            }
        );
        assert_eq!(
            parse_command("s diamond 5").unwrap(),
            Command::Sell {
                item: "diamond".to_string(),
                quantity: 5
            }
        );
    }

    #[test]
    fn test_parse_deposit() {
        assert_eq!(parse_command("deposit").unwrap(), Command::Deposit { amount: None });
        assert_eq!(parse_command("d").unwrap(), Command::Deposit { amount: None });
        assert_eq!(
            parse_command("deposit 64").unwrap(),
            Command::Deposit { amount: Some(64.0) }
        );
        // Negative / zero rejected
        assert!(parse_command("deposit 0").is_err());
        assert!(parse_command("deposit -1").is_err());
        // Non-numeric
        assert!(parse_command("deposit abc").is_err());
    }

    #[test]
    fn test_parse_withdraw() {
        assert_eq!(parse_command("withdraw").unwrap(), Command::Withdraw { amount: None });
        assert_eq!(parse_command("w").unwrap(), Command::Withdraw { amount: None });
        assert_eq!(
            parse_command("withdraw 32").unwrap(),
            Command::Withdraw { amount: Some(32.0) }
        );
    }

    #[test]
    fn test_parse_price() {
        assert_eq!(
            parse_command("price cobblestone").unwrap(),
            Command::Price {
                item: "cobblestone".to_string(),
                quantity: None
            }
        );
        assert_eq!(
            parse_command("p cobblestone 64").unwrap(),
            Command::Price {
                item: "cobblestone".to_string(),
                quantity: Some(64)
            }
        );
        // No item
        assert!(parse_command("price").is_err());
        // Zero quantity
        assert!(parse_command("price x 0").is_err());
    }

    #[test]
    fn test_parse_balance() {
        assert_eq!(
            parse_command("balance").unwrap(),
            Command::Balance { target: None }
        );
        assert_eq!(
            parse_command("bal").unwrap(),
            Command::Balance { target: None }
        );
        assert_eq!(
            parse_command("bal Steve").unwrap(),
            Command::Balance {
                target: Some("Steve".to_string())
            }
        );
        // Invalid username
        assert!(parse_command("bal ab").is_err());
        assert!(parse_command("bal thisnameistoolongforminecraft").is_err());
    }

    #[test]
    fn test_parse_pay() {
        assert_eq!(
            parse_command("pay Steve 10.5").unwrap(),
            Command::Pay {
                target: "Steve".to_string(),
                amount: 10.5
            }
        );
        // Missing amount
        assert!(parse_command("pay Steve").is_err());
        // Bad amount
        assert!(parse_command("pay Steve abc").is_err());
        // Zero / negative
        assert!(parse_command("pay Steve 0").is_err());
        assert!(parse_command("pay Steve -5").is_err());
        // Too large
        assert!(parse_command("pay Steve 2000000").is_err());
    }

    #[test]
    fn test_parse_items() {
        assert_eq!(parse_command("items").unwrap(), Command::Items { page: 1 });
        assert_eq!(parse_command("items 3").unwrap(), Command::Items { page: 3 });
        // Bad page defaults to 1
        assert_eq!(parse_command("items abc").unwrap(), Command::Items { page: 1 });
    }

    #[test]
    fn test_parse_queue() {
        assert_eq!(parse_command("queue").unwrap(), Command::Queue { page: 1 });
        assert_eq!(parse_command("q").unwrap(), Command::Queue { page: 1 });
        assert_eq!(parse_command("queue 2").unwrap(), Command::Queue { page: 2 });
    }

    #[test]
    fn test_parse_cancel() {
        assert_eq!(parse_command("cancel 5").unwrap(), Command::Cancel { order_id: 5 });
        assert_eq!(parse_command("c 5").unwrap(), Command::Cancel { order_id: 5 });
        assert_eq!(parse_command("c #5").unwrap(), Command::Cancel { order_id: 5 });
        // Missing
        assert!(parse_command("cancel").is_err());
        // Non-numeric
        assert!(parse_command("cancel abc").is_err());
    }

    #[test]
    fn test_parse_status_and_help() {
        assert_eq!(parse_command("status").unwrap(), Command::Status);
        assert_eq!(parse_command("help").unwrap(), Command::Help { topic: None });
        assert_eq!(parse_command("h").unwrap(), Command::Help { topic: None });
        assert_eq!(
            parse_command("help buy").unwrap(),
            Command::Help { topic: Some("buy".to_string()) }
        );
    }

    #[test]
    fn test_parse_operator_commands() {
        assert_eq!(
            parse_command("additem diamond 100").unwrap(),
            Command::AddItem {
                item: "diamond".to_string(),
                quantity: 100
            }
        );
        assert_eq!(
            parse_command("ai diamond 100").unwrap(),
            Command::AddItem {
                item: "diamond".to_string(),
                quantity: 100
            }
        );
        assert_eq!(
            parse_command("removeitem coal 50").unwrap(),
            Command::RemoveItem {
                item: "coal".to_string(),
                quantity: 50
            }
        );
        assert_eq!(
            parse_command("ri coal 50").unwrap(),
            Command::RemoveItem {
                item: "coal".to_string(),
                quantity: 50
            }
        );
        assert_eq!(
            parse_command("addcurrency cobblestone 1000").unwrap(),
            Command::AddCurrency {
                item: "cobblestone".to_string(),
                amount: 1000.0
            }
        );
        assert_eq!(
            parse_command("ac cobblestone 1000").unwrap(),
            Command::AddCurrency {
                item: "cobblestone".to_string(),
                amount: 1000.0
            }
        );
        assert_eq!(
            parse_command("removecurrency cobblestone 500").unwrap(),
            Command::RemoveCurrency {
                item: "cobblestone".to_string(),
                amount: 500.0
            }
        );
        assert_eq!(
            parse_command("rc cobblestone 500").unwrap(),
            Command::RemoveCurrency {
                item: "cobblestone".to_string(),
                amount: 500.0
            }
        );
    }
}
