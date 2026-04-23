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

/// A single entry in the audit log.
#[derive(Debug, PartialEq, Serialize, Deserialize, Default, Clone)]
pub struct Order {
    pub order_type: OrderType,
    pub item: crate::types::ItemId,
    pub amount: i32,
    pub user_uuid: String,
}

/// Canonical filesystem path for the session-only orders file.
///
/// Exposed at module scope (not as an `impl Order` const) so unrelated callers
/// that need to reference the same path — e.g. `Store::new` deleting stale
/// orders on startup — can `use` it instead of duplicating the literal.
pub const ORDERS_FILE: &str = "data/orders.json";

impl Order {
    /// Saves a VecDeque of Orders to `ORDERS_FILE`, keeping only the most
    /// recent `max_orders` entries.
    pub fn save_all_with_limit(orders: &VecDeque<Self>, max_orders: usize) -> io::Result<()> {
        let file_path = Path::new(ORDERS_FILE);

        if let Some(parent) = file_path.parent()
            && !parent.exists() {
                fs::create_dir_all(parent)?;
            }

        // Skipping `len - max_orders` from the front keeps the most recent
        // entries, matching the pop_front pruning used at the in-memory layer
        // but without requiring a mutable borrow of the caller's queue.
        let orders_to_save: VecDeque<Self> = if orders.len() > max_orders {
            tracing::info!("[Order] pruning {} -> {} before save", orders.len(), max_orders);
            orders.iter().skip(orders.len() - max_orders).cloned().collect()
        } else {
            orders.clone()
        };

        let json_str = serde_json::to_string_pretty(&orders_to_save)
            .map_err(io::Error::other)?;

        write_atomic(file_path, &json_str)?;
        tracing::debug!("[Order] wrote {} orders to {}", orders_to_save.len(), ORDERS_FILE);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(i: u32) -> Order {
        Order {
            order_type: OrderType::Buy,
            item: crate::types::ItemId::new("diamond").unwrap(),
            amount: i as i32,
            user_uuid: format!("u-{i}"),
        }
    }

    #[test]
    fn order_type_default_is_buy() {
        assert_eq!(OrderType::default(), OrderType::Buy);
    }

    #[test]
    fn order_type_serde_round_trip_preserves_all_variants() {
        for v in [
            OrderType::Buy,
            OrderType::Sell,
            OrderType::AddItem,
            OrderType::RemoveItem,
            OrderType::DepositBalance,
            OrderType::WithdrawBalance,
            OrderType::AddCurrency,
            OrderType::RemoveCurrency,
        ] {
            let j = serde_json::to_string(&v).unwrap();
            let back: OrderType = serde_json::from_str(&j).unwrap();
            assert_eq!(back, v);
        }
    }

    #[test]
    fn pruning_keeps_most_recent_and_preserves_order() {
        // Simulate what save_all_with_limit does to the in-memory queue.
        let mut q = VecDeque::new();
        for i in 0..5u32 { q.push_back(make(i)); }
        let max = 3;
        let kept: VecDeque<Order> = q.iter().skip(q.len() - max).cloned().collect();
        assert_eq!(kept.len(), 3);
        assert_eq!(kept.front().unwrap().amount, 2);
        assert_eq!(kept.back().unwrap().amount, 4);
    }

    #[test]
    fn no_pruning_when_under_limit() {
        let mut q = VecDeque::new();
        for i in 0..3u32 { q.push_back(make(i)); }
        assert!(q.len() <= 10);
        assert_eq!(q.len(), 3);
    }
}
