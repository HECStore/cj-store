//! Shared input validators for player commands.
//!
//! Pulled out of `player.rs` so the per-command handler modules can share
//! them without cycles.

use super::super::utils;
use crate::constants::MAX_TRANSACTION_QUANTITY;

/// Validate item name format.
/// Item names should be alphanumeric with optional underscores and colons.
///
/// # Returns
/// * `Ok(())` if valid
/// * `Err(message)` with user-friendly error message if invalid
pub(super) fn validate_item_name(item: &str) -> Result<(), String> {
    if item.is_empty() {
        return Err("Item name cannot be empty. Example: buy cobblestone 64".to_string());
    }

    let normalized = utils::normalize_item_id(item);
    if normalized.is_empty() {
        return Err("Invalid item name. Example items: cobblestone, iron_ingot, diamond".to_string());
    }

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
pub(super) fn validate_quantity(quantity_str: &str, operation: &str) -> Result<u32, String> {
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
pub(super) fn validate_username(username: &str) -> Result<(), String> {
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
