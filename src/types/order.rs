//! Order Management
//!
//! Orders represent the audit log of all buy/sell/deposit/withdraw operations.
//! The order queue is limited to prevent unbounded memory growth.
//!
//! The maximum number of orders can be configured in `data/config.json` via
//! the `max_orders` field. The default is 10,000.

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::fsutil::write_atomic;

/// The kind of transaction recorded in the audit log.
///
/// Variants are split between user-initiated trades (`Buy`/`Sell`),
/// operator inventory adjustments (`AddItem`/`RemoveItem`), user balance
/// movements (`DepositBalance`/`WithdrawBalance`), and operator balance
/// adjustments (`AddCurrency`/`RemoveCurrency`). Serialized variant names
/// are part of the on-disk format in `data/orders.json`, so renaming them
/// is a breaking change.
#[derive(Debug, PartialEq, Serialize, Deserialize, Default, Clone)]
pub enum OrderType {
    /// User purchased an item from the store.
    #[default]
    Buy,
    /// User sold an item to the store.
    Sell,
    /// Operator added items to storage (no currency movement).
    AddItem,
    /// Operator removed items from storage (no currency movement).
    RemoveItem,
    /// User deposited diamonds into their store balance.
    DepositBalance,
    /// User withdrew diamonds from their store balance.
    WithdrawBalance,
    /// Operator credited currency to a user's balance directly.
    AddCurrency,
    /// Operator debited currency from a user's balance directly.
    RemoveCurrency,
}

/// Represents a single order in the audit log.
/// 
/// Orders track all transactions for auditing and debugging purposes.
/// They are stored in a VecDeque with automatic pruning when MAX_ORDERS is exceeded.
#[derive(Debug, PartialEq, Serialize, Deserialize, Default, Clone)]
pub struct Order {
    /// Type of order (buy, sell, deposit, withdraw, etc.)
    pub order_type: OrderType,
    /// Item involved in the transaction
    pub item: crate::types::ItemId,
    /// Quantity of items
    pub amount: i32,
    /// UUID of the user who placed the order
    pub user_uuid: String,
}

/// Canonical filesystem path for the session-only orders file.
///
/// Exposed at module scope (not as an `impl Order` const) so unrelated callers
/// that need to reference the same path — e.g. `Store::new` deleting stale
/// orders on startup — can `use` it instead of duplicating the literal.
pub const ORDERS_FILE: &str = "data/orders.json";

impl Order {
    /// Saves a VecDeque of Orders to a single JSON file.
    /// Automatically prunes to the specified limit before saving.
    pub fn save_all_with_limit(orders: &VecDeque<Self>, max_orders: usize) -> io::Result<()> {
        let file_path = Path::new(ORDERS_FILE);

        // Ensure the parent directory exists
        if let Some(parent) = file_path.parent()
            && !parent.exists() {
                fs::create_dir_all(parent)?;
            }

        // Create a pruned copy if needed (don't mutate the original).
        // Skipping `len - max_orders` from the front keeps the most recent
        // `max_orders` entries, matching the pop_front pruning semantics
        // used elsewhere but without requiring a mutable borrow of the caller's queue.
        let orders_to_save: VecDeque<Self> = if orders.len() > max_orders {
            tracing::info!("Pruning {} orders to {} before saving", orders.len(), max_orders);
            orders.iter().skip(orders.len() - max_orders).cloned().collect()
        } else {
            orders.clone()
        };

        let json_str = serde_json::to_string_pretty(&orders_to_save)
            .map_err(io::Error::other)?;

        write_atomic(file_path, &json_str)?;
        Ok(())
    }
    
}
