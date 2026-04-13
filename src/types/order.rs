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

/// Default maximum number of orders to retain in memory and on disk.
/// This value is used when loading config or when config is not available.
/// Can be overridden in config.json via the `max_orders` field.
#[allow(dead_code)] // fallback constant for config loading
pub const DEFAULT_MAX_ORDERS: usize = 10_000;

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

#[allow(dead_code)] // persistence API — load/save/prune used by state management (keep as cohesive surface)
impl Order {
    const ORDERS_FILE: &'static str = "data/orders.json";

    /// Loads all orders from a single JSON file into a VecDeque.
    /// If the file has more than DEFAULT_MAX_ORDERS, only the most recent are kept.
    /// Use `load_all_with_limit` if you need a custom limit.
    pub fn load_all() -> io::Result<VecDeque<Self>> {
        Self::load_all_with_limit(DEFAULT_MAX_ORDERS)
    }
    
    /// Loads all orders from a single JSON file into a VecDeque.
    /// If the file has more than `max_orders`, only the most recent are kept.
    pub fn load_all_with_limit(max_orders: usize) -> io::Result<VecDeque<Self>> {
        let file_path = Path::new(Self::ORDERS_FILE);

        if !file_path.exists() {
            tracing::info!(
                "Orders file not found at {}. Starting with empty order queue.",
                file_path.display()
            );
            return Ok(VecDeque::new());
        }

        match fs::read_to_string(file_path) {
            Ok(json_str) => match serde_json::from_str::<VecDeque<Self>>(&json_str) {
                Ok(mut orders) => {
                    // Prune if necessary
                    let original_len = orders.len();
                    Self::prune_to_limit(&mut orders, max_orders);
                    if orders.len() < original_len {
                        tracing::info!(
                            "Pruned {} old orders (kept {} of {})",
                            original_len - orders.len(),
                            orders.len(),
                            original_len
                        );
                    }
                    Ok(orders)
                }
                Err(e) => {
                    tracing::warn!(
                        "Could not deserialize orders from {}: {}. Starting fresh.",
                        file_path.display(),
                        e
                    );
                    Ok(VecDeque::new())
                }
            },
            Err(e) => {
                tracing::warn!(
                    "Could not read orders file {}: {}. Starting fresh.",
                    file_path.display(),
                    e
                );
                Ok(VecDeque::new())
            }
        }
    }

    /// Prune the order queue to DEFAULT_MAX_ORDERS, removing the oldest orders.
    /// This should be called periodically to prevent unbounded growth.
    pub fn prune(orders: &mut VecDeque<Self>) {
        Self::prune_to_limit(orders, DEFAULT_MAX_ORDERS);
    }
    
    /// Prune the order queue to a custom limit, removing the oldest orders.
    ///
    /// Orders are appended to the back of the `VecDeque`, so the front holds
    /// the oldest entries. Popping from the front preserves chronological
    /// order while discarding the least recent history first.
    pub fn prune_to_limit(orders: &mut VecDeque<Self>, max_orders: usize) {
        while orders.len() > max_orders {
            orders.pop_front(); // Remove oldest
        }
    }

    /// Saves a VecDeque of Orders to a single JSON file.
    /// Automatically prunes to DEFAULT_MAX_ORDERS before saving.
    /// Use `save_all_with_limit` for a custom limit.
    pub fn save_all(orders: &VecDeque<Self>) -> io::Result<()> {
        Self::save_all_with_limit(orders, DEFAULT_MAX_ORDERS)
    }
    
    /// Saves a VecDeque of Orders to a single JSON file.
    /// Automatically prunes to the specified limit before saving.
    pub fn save_all_with_limit(orders: &VecDeque<Self>, max_orders: usize) -> io::Result<()> {
        let file_path = Path::new(Self::ORDERS_FILE);

        // Ensure the parent directory exists
        if let Some(parent) = file_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)?;
            }
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
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        write_atomic(file_path, &json_str)?;
        Ok(())
    }
    
    /// Get the number of orders currently stored.
    pub fn count(orders: &VecDeque<Self>) -> usize {
        orders.len()
    }
}
