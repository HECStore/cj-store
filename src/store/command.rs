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

    // ---- top-level dispatch ------------------------------------------------

    #[test]
    fn empty_input_prompts_help() {
        let err = parse_command("").unwrap_err();
        assert!(err.contains("help"));
    }

    #[test]
    fn whitespace_only_input_prompts_help() {
        // `split_whitespace` strips everything, so this hits the same arm as "".
        let err = parse_command("   ").unwrap_err();
        assert!(err.contains("help"));
    }

    #[test]
    fn unknown_verb_is_rejected_and_named() {
        // The error must name the offending verb so the player can see the typo.
        let err = parse_command("teleport").unwrap_err();
        assert!(err.contains("Unknown command"));
        assert!(err.contains("teleport"));
    }

    #[test]
    fn unknown_verb_ignores_trailing_args() {
        let err = parse_command("teleport home now").unwrap_err();
        assert!(err.contains("teleport"));
    }

    // ---- buy / sell --------------------------------------------------------

    #[test]
    fn buy_command_parses_item_and_quantity() {
        assert_eq!(
            parse_command("buy cobblestone 64").unwrap(),
            Command::Buy {
                item: "cobblestone".to_string(),
                quantity: 64
            }
        );
    }

    #[test]
    fn buy_alias_b_is_equivalent() {
        assert_eq!(
            parse_command("b diamond 1").unwrap(),
            Command::Buy {
                item: "diamond".to_string(),
                quantity: 1
            }
        );
    }

    #[test]
    fn buy_strips_minecraft_namespace_prefix() {
        assert_eq!(
            parse_command("buy minecraft:iron_ingot 32").unwrap(),
            Command::Buy {
                item: "iron_ingot".to_string(),
                quantity: 32
            }
        );
    }

    #[test]
    fn buy_without_args_reports_usage() {
        let err = parse_command("buy").unwrap_err();
        assert!(err.contains("Usage: buy"));
    }

    #[test]
    fn buy_without_quantity_reports_usage() {
        let err = parse_command("buy cobblestone").unwrap_err();
        assert!(err.contains("Usage: buy"));
    }

    #[test]
    fn buy_with_non_numeric_quantity_is_rejected() {
        let err = parse_command("buy cobblestone abc").unwrap_err();
        // Error flows through `validate_quantity`, which names the bad token
        // and the operation.
        assert!(err.contains("abc"));
        assert!(err.contains("buy"));
    }

    #[test]
    fn buy_with_zero_quantity_is_rejected() {
        let err = parse_command("buy cobblestone 0").unwrap_err();
        assert!(err.contains("at least 1"));
    }

    #[test]
    fn buy_with_invalid_item_name_is_rejected() {
        // Space characters can't appear in a single token, so use a hyphen to
        // exercise the validator.
        let err = parse_command("buy iron-ingot 1").unwrap_err();
        assert!(err.contains("invalid character"));
    }

    #[test]
    fn sell_command_parses_item_and_quantity() {
        assert_eq!(
            parse_command("sell iron_ingot 128").unwrap(),
            Command::Sell {
                item: "iron_ingot".to_string(),
                quantity: 128
            }
        );
    }

    #[test]
    fn sell_alias_s_is_equivalent() {
        assert_eq!(
            parse_command("s diamond 5").unwrap(),
            Command::Sell {
                item: "diamond".to_string(),
                quantity: 5
            }
        );
    }

    #[test]
    fn sell_with_bad_quantity_names_sell_in_error() {
        let err = parse_command("sell diamond abc").unwrap_err();
        assert!(err.contains("sell"));
    }

    // ---- deposit / withdraw ------------------------------------------------

    #[test]
    fn deposit_without_amount_leaves_amount_none() {
        assert_eq!(parse_command("deposit").unwrap(), Command::Deposit { amount: None });
        assert_eq!(parse_command("d").unwrap(), Command::Deposit { amount: None });
    }

    #[test]
    fn deposit_with_amount_parses_as_float() {
        assert_eq!(
            parse_command("deposit 64").unwrap(),
            Command::Deposit { amount: Some(64.0) }
        );
    }

    #[test]
    fn deposit_rejects_zero_amount() {
        let err = parse_command("deposit 0").unwrap_err();
        assert!(err.contains("positive"));
    }

    #[test]
    fn deposit_rejects_negative_amount() {
        let err = parse_command("deposit -1").unwrap_err();
        assert!(err.contains("positive"));
    }

    #[test]
    fn deposit_rejects_non_numeric_amount() {
        // Error must name the bad token and the verb.
        let err = parse_command("deposit abc").unwrap_err();
        assert!(err.contains("abc"));
        assert!(err.contains("deposit"));
    }

    #[test]
    fn deposit_rejects_non_finite_amount() {
        // f64 parses "inf" and "nan" successfully; the finite check catches them.
        assert!(parse_command("deposit inf").is_err());
        assert!(parse_command("deposit NaN").is_err());
    }

    #[test]
    fn withdraw_without_amount_leaves_amount_none() {
        assert_eq!(parse_command("withdraw").unwrap(), Command::Withdraw { amount: None });
        assert_eq!(parse_command("w").unwrap(), Command::Withdraw { amount: None });
    }

    #[test]
    fn withdraw_with_amount_parses_as_float() {
        assert_eq!(
            parse_command("withdraw 32").unwrap(),
            Command::Withdraw { amount: Some(32.0) }
        );
    }

    #[test]
    fn withdraw_rejects_non_finite_amount() {
        assert!(parse_command("withdraw inf").is_err());
    }

    // ---- price -------------------------------------------------------------

    #[test]
    fn price_without_quantity_leaves_quantity_none() {
        assert_eq!(
            parse_command("price cobblestone").unwrap(),
            Command::Price {
                item: "cobblestone".to_string(),
                quantity: None
            }
        );
    }

    #[test]
    fn price_alias_p_accepts_quantity() {
        assert_eq!(
            parse_command("p cobblestone 64").unwrap(),
            Command::Price {
                item: "cobblestone".to_string(),
                quantity: Some(64)
            }
        );
    }

    #[test]
    fn price_without_item_reports_usage() {
        let err = parse_command("price").unwrap_err();
        assert!(err.contains("Usage: price"));
    }

    #[test]
    fn price_with_zero_quantity_is_rejected() {
        let err = parse_command("price diamond 0").unwrap_err();
        assert!(err.contains("positive"));
    }

    #[test]
    fn price_with_negative_quantity_is_rejected() {
        // u32 parse rejects the leading `-`, so this hits the same error arm.
        let err = parse_command("price diamond -1").unwrap_err();
        assert!(err.contains("Invalid quantity"));
    }

    #[test]
    fn price_with_bad_item_name_is_rejected() {
        let err = parse_command("price iron-ingot").unwrap_err();
        assert!(err.contains("invalid character"));
    }

    // ---- balance -----------------------------------------------------------

    #[test]
    fn balance_without_target_leaves_target_none() {
        assert_eq!(parse_command("balance").unwrap(), Command::Balance { target: None });
        assert_eq!(parse_command("bal").unwrap(), Command::Balance { target: None });
    }

    #[test]
    fn balance_with_target_captures_username() {
        assert_eq!(
            parse_command("bal Steve").unwrap(),
            Command::Balance {
                target: Some("Steve".to_string())
            }
        );
    }

    #[test]
    fn balance_rejects_too_short_username() {
        let err = parse_command("bal ab").unwrap_err();
        assert!(err.contains("3-16 characters"));
    }

    #[test]
    fn balance_rejects_too_long_username() {
        let err = parse_command("bal thisnameistoolongforminecraft").unwrap_err();
        assert!(err.contains("3-16 characters"));
    }

    // ---- pay ---------------------------------------------------------------

    #[test]
    fn pay_parses_target_and_amount() {
        assert_eq!(
            parse_command("pay Steve 10.5").unwrap(),
            Command::Pay {
                target: "Steve".to_string(),
                amount: 10.5
            }
        );
    }

    #[test]
    fn pay_without_amount_reports_usage() {
        let err = parse_command("pay Steve").unwrap_err();
        assert!(err.contains("Usage: pay"));
    }

    #[test]
    fn pay_with_non_numeric_amount_is_rejected() {
        let err = parse_command("pay Steve abc").unwrap_err();
        assert!(err.contains("abc"));
    }

    #[test]
    fn pay_rejects_zero_amount() {
        let err = parse_command("pay Steve 0").unwrap_err();
        assert!(err.contains("positive"));
    }

    #[test]
    fn pay_rejects_negative_amount() {
        let err = parse_command("pay Steve -5").unwrap_err();
        assert!(err.contains("positive"));
    }

    #[test]
    fn pay_rejects_amount_above_cap() {
        // Cap is 1,000,000 per payment.
        let err = parse_command("pay Steve 2000000").unwrap_err();
        assert!(err.contains("Maximum"));
    }

    #[test]
    fn pay_accepts_amount_at_cap() {
        assert_eq!(
            parse_command("pay Steve 1000000").unwrap(),
            Command::Pay {
                target: "Steve".to_string(),
                amount: 1_000_000.0
            }
        );
    }

    #[test]
    fn pay_rejects_invalid_username() {
        let err = parse_command("pay hi 10").unwrap_err();
        assert!(err.contains("3-16 characters"));
    }

    #[test]
    fn pay_rejects_non_finite_amount() {
        assert!(parse_command("pay Steve inf").is_err());
        assert!(parse_command("pay Steve NaN").is_err());
    }

    // ---- items / queue (paged) --------------------------------------------

    #[test]
    fn items_without_page_defaults_to_one() {
        assert_eq!(parse_command("items").unwrap(), Command::Items { page: 1 });
    }

    #[test]
    fn items_accepts_explicit_page() {
        assert_eq!(parse_command("items 3").unwrap(), Command::Items { page: 3 });
    }

    #[test]
    fn items_non_numeric_page_falls_back_to_one() {
        // `parse_page` swallows malformed input rather than erroring — the list
        // view is low-risk and friendlier to mistype.
        assert_eq!(parse_command("items abc").unwrap(), Command::Items { page: 1 });
    }

    #[test]
    fn items_zero_page_is_clamped_to_one() {
        // `.max(1)` guards the 1-indexed pager.
        assert_eq!(parse_command("items 0").unwrap(), Command::Items { page: 1 });
    }

    #[test]
    fn queue_without_page_defaults_to_one() {
        assert_eq!(parse_command("queue").unwrap(), Command::Queue { page: 1 });
        assert_eq!(parse_command("q").unwrap(), Command::Queue { page: 1 });
    }

    #[test]
    fn queue_accepts_explicit_page() {
        assert_eq!(parse_command("queue 2").unwrap(), Command::Queue { page: 2 });
    }

    // ---- cancel ------------------------------------------------------------

    #[test]
    fn cancel_parses_bare_order_id() {
        assert_eq!(parse_command("cancel 5").unwrap(), Command::Cancel { order_id: 5 });
    }

    #[test]
    fn cancel_alias_c_is_equivalent() {
        assert_eq!(parse_command("c 5").unwrap(), Command::Cancel { order_id: 5 });
    }

    #[test]
    fn cancel_strips_leading_hash() {
        // `queue` shows order IDs as `#5`, so accept that form verbatim.
        assert_eq!(parse_command("c #5").unwrap(), Command::Cancel { order_id: 5 });
    }

    #[test]
    fn cancel_without_id_reports_usage() {
        let err = parse_command("cancel").unwrap_err();
        assert!(err.contains("Usage: cancel"));
    }

    #[test]
    fn cancel_rejects_non_numeric_id() {
        let err = parse_command("cancel abc").unwrap_err();
        assert!(err.contains("abc"));
    }

    #[test]
    fn cancel_rejects_hash_only() {
        // `#` strips to an empty string, which `u64::parse` rejects.
        assert!(parse_command("cancel #").is_err());
    }

    // ---- status / help -----------------------------------------------------

    #[test]
    fn status_command_parses_with_no_args() {
        assert_eq!(parse_command("status").unwrap(), Command::Status);
    }

    #[test]
    fn status_ignores_trailing_args() {
        // Extra tokens are silently dropped; `status` takes none.
        assert_eq!(parse_command("status now").unwrap(), Command::Status);
    }

    #[test]
    fn help_without_topic_leaves_topic_none() {
        assert_eq!(parse_command("help").unwrap(), Command::Help { topic: None });
        assert_eq!(parse_command("h").unwrap(), Command::Help { topic: None });
    }

    #[test]
    fn help_captures_topic_token() {
        assert_eq!(
            parse_command("help buy").unwrap(),
            Command::Help { topic: Some("buy".to_string()) }
        );
    }

    // ---- operator commands -------------------------------------------------

    #[test]
    fn additem_parses_item_and_quantity() {
        assert_eq!(
            parse_command("additem diamond 100").unwrap(),
            Command::AddItem {
                item: "diamond".to_string(),
                quantity: 100
            }
        );
    }

    #[test]
    fn additem_alias_ai_is_equivalent() {
        assert_eq!(
            parse_command("ai diamond 100").unwrap(),
            Command::AddItem {
                item: "diamond".to_string(),
                quantity: 100
            }
        );
    }

    #[test]
    fn additem_without_args_reports_usage_with_verb() {
        let err = parse_command("additem").unwrap_err();
        // Error must name the specific operator verb, not a generic "missing args".
        assert!(err.contains("additem"));
    }

    #[test]
    fn removeitem_parses_item_and_quantity() {
        assert_eq!(
            parse_command("removeitem coal 50").unwrap(),
            Command::RemoveItem {
                item: "coal".to_string(),
                quantity: 50
            }
        );
    }

    #[test]
    fn removeitem_alias_ri_is_equivalent() {
        assert_eq!(
            parse_command("ri coal 50").unwrap(),
            Command::RemoveItem {
                item: "coal".to_string(),
                quantity: 50
            }
        );
    }

    #[test]
    fn addcurrency_parses_item_and_amount() {
        assert_eq!(
            parse_command("addcurrency cobblestone 1000").unwrap(),
            Command::AddCurrency {
                item: "cobblestone".to_string(),
                amount: 1000.0
            }
        );
    }

    #[test]
    fn addcurrency_alias_ac_is_equivalent() {
        assert_eq!(
            parse_command("ac cobblestone 1000").unwrap(),
            Command::AddCurrency {
                item: "cobblestone".to_string(),
                amount: 1000.0
            }
        );
    }

    #[test]
    fn addcurrency_accepts_fractional_amount() {
        // `parse_item_amount` uses f64, unlike item quantities.
        assert_eq!(
            parse_command("ac diamond 12.5").unwrap(),
            Command::AddCurrency {
                item: "diamond".to_string(),
                amount: 12.5
            }
        );
    }

    #[test]
    fn addcurrency_rejects_non_numeric_amount() {
        let err = parse_command("addcurrency diamond xyz").unwrap_err();
        assert!(err.contains("xyz"));
    }

    #[test]
    fn addcurrency_rejects_non_finite_amount() {
        assert!(parse_command("addcurrency diamond inf").is_err());
        assert!(parse_command("addcurrency diamond NaN").is_err());
    }

    #[test]
    fn addcurrency_without_args_reports_usage_with_verb() {
        let err = parse_command("addcurrency").unwrap_err();
        assert!(err.contains("addcurrency"));
    }

    #[test]
    fn removecurrency_parses_item_and_amount() {
        assert_eq!(
            parse_command("removecurrency cobblestone 500").unwrap(),
            Command::RemoveCurrency {
                item: "cobblestone".to_string(),
                amount: 500.0
            }
        );
    }

    #[test]
    fn removecurrency_alias_rc_is_equivalent() {
        assert_eq!(
            parse_command("rc cobblestone 500").unwrap(),
            Command::RemoveCurrency {
                item: "cobblestone".to_string(),
                amount: 500.0
            }
        );
    }
}
