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
use crate::types::ItemId;

/// The kind of transaction recorded in the audit log.
///
/// Variants are split between user-initiated trades (`Buy`/`Sell`),
/// operator inventory adjustments (`AddItem`/`RemoveItem`), user balance
/// movements (`DepositBalance`/`WithdrawBalance`), and operator balance
/// adjustments (`AddCurrency`/`RemoveCurrency`). Serialized variant names
/// are part of the on-disk format in `data/orders.json`, so renaming them
/// is a breaking change.
#[derive(Debug, PartialEq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(Default))]
pub enum OrderType {
    /// User purchased an item from the store.
    #[cfg_attr(test, default)]
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
///
/// `currency_amount` is the diamond magnitude for every value-bearing variant
/// (Buy/Sell/Deposit/Withdraw/AddCurrency/RemoveCurrency); 0.0 only for
/// AddItem/RemoveItem which move items without a currency leg.
#[derive(Debug, PartialEq, Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(Default))]
pub struct Order {
    pub order_type: OrderType,
    pub item: crate::types::ItemId,
    pub amount: i32,
    #[serde(default)]
    pub currency_amount: f64,
    pub user_uuid: String,
}

/// Canonical filesystem path for the session-only orders file.
///
/// Exposed at module scope (not as an `impl Order` const) so unrelated callers
/// that need to reference the same path — e.g. `Store::new` deleting stale
/// orders on startup — can `use` it instead of duplicating the literal.
pub const ORDERS_FILE: &str = "data/orders.json";

impl Order {
    /// User purchased `qty` of `item` for `price` total diamonds.
    pub fn buy(item: ItemId, qty: i32, price: f64, uuid: String) -> Self {
        Self {
            order_type: OrderType::Buy,
            item,
            amount: qty,
            currency_amount: price,
            user_uuid: uuid,
        }
    }

    /// User sold `qty` of `item` for `payout` total diamonds.
    pub fn sell(item: ItemId, qty: i32, payout: f64, uuid: String) -> Self {
        Self {
            order_type: OrderType::Sell,
            item,
            amount: qty,
            currency_amount: payout,
            user_uuid: uuid,
        }
    }

    /// User deposited `amount` diamonds into their store balance. `amount`
    /// is the diamond magnitude credited; `Order::amount` carries the whole
    /// diamond count.
    pub fn deposit_balance(amount: f64, uuid: String) -> Self {
        Self {
            order_type: OrderType::DepositBalance,
            item: ItemId::from_normalized("diamond".to_string()),
            amount: amount as i32,
            currency_amount: amount,
            user_uuid: uuid,
        }
    }

    /// User withdrew `amount` diamonds from their store balance.
    pub fn withdraw_balance(amount: f64, uuid: String) -> Self {
        Self {
            order_type: OrderType::WithdrawBalance,
            item: ItemId::from_normalized("diamond".to_string()),
            amount: amount as i32,
            currency_amount: amount,
            user_uuid: uuid,
        }
    }

    /// Operator credited `amount` of currency to the reserve for `item`.
    pub fn add_currency(item: ItemId, amount: f64, uuid: String) -> Self {
        Self {
            order_type: OrderType::AddCurrency,
            item,
            amount: 0,
            currency_amount: amount,
            user_uuid: uuid,
        }
    }

    /// Operator debited `amount` of currency from the reserve for `item`.
    pub fn remove_currency(item: ItemId, amount: f64, uuid: String) -> Self {
        Self {
            order_type: OrderType::RemoveCurrency,
            item,
            amount: 0,
            currency_amount: amount,
            user_uuid: uuid,
        }
    }

    /// Operator added `qty` of `item` to storage (no currency leg).
    pub fn add_item(item: ItemId, qty: i32, uuid: String) -> Self {
        Self {
            order_type: OrderType::AddItem,
            item,
            amount: qty,
            currency_amount: 0.0,
            user_uuid: uuid,
        }
    }

    /// Operator removed `qty` of `item` from storage (no currency leg).
    pub fn remove_item(item: ItemId, qty: i32, uuid: String) -> Self {
        Self {
            order_type: OrderType::RemoveItem,
            item,
            amount: qty,
            currency_amount: 0.0,
            user_uuid: uuid,
        }
    }

    /// Saves a VecDeque of Orders to `ORDERS_FILE`, keeping only the most
    /// recent `max_orders` entries.
    pub fn save_all_with_limit(orders: &VecDeque<Self>, max_orders: usize) -> io::Result<()> {
        Self::save_all_with_limit_at(orders, max_orders, Path::new(ORDERS_FILE))
    }

    /// Path-parameterized form of `save_all_with_limit`. Tests drive this
    /// directly against a `tempfile::TempDir` so the on-disk write path is
    /// covered without touching `data/orders.json`; the public
    /// `save_all_with_limit` is a thin one-liner over this helper.
    fn save_all_with_limit_at(
        orders: &VecDeque<Self>,
        max_orders: usize,
        file_path: &Path,
    ) -> io::Result<()> {
        if let Some(parent) = file_path.parent()
            && !parent.exists() {
                fs::create_dir_all(parent)?;
            }

        // Skipping `len - max_orders` from the front keeps the most recent
        // entries. Note: there is no in-memory pruning of `store.orders` —
        // `state::save` (src/store/state.rs) drains the front before calling
        // this function as the primary cap; this branch is a defense-in-depth
        // second cap that fires only if the caller passes an unbounded queue.
        let orders_to_save: VecDeque<Self> = if orders.len() > max_orders {
            tracing::info!("[Order] pruning {} -> {} before save", orders.len(), max_orders);
            orders.iter().skip(orders.len() - max_orders).cloned().collect()
        } else {
            orders.clone()
        };

        let json_str = serde_json::to_string_pretty(&orders_to_save)
            .map_err(io::Error::other)?;

        write_atomic(file_path, &json_str)?;
        tracing::debug!(
            "[Order] wrote {} orders to {}",
            orders_to_save.len(),
            file_path.display()
        );
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
            currency_amount: 0.0,
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
    fn save_all_with_limit_at_prunes_oldest_and_round_trips() {
        // Drives the real `save_all_with_limit_at` against a tempdir so the
        // prune branch and the on-disk JSON round-trip are both covered;
        // replaces the previous self-referential test that re-implemented the
        // skip expression inline rather than calling the function.
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("orders.json");

        let mut q = VecDeque::new();
        for i in 0..5u32 { q.push_back(make(i)); }

        Order::save_all_with_limit_at(&q, 3, &file_path).unwrap();

        let json = std::fs::read_to_string(&file_path).unwrap();
        let on_disk: VecDeque<Order> = serde_json::from_str(&json).unwrap();
        assert_eq!(on_disk.len(), 3);
        assert_eq!(on_disk.front().unwrap().amount, 2);
        assert_eq!(on_disk.back().unwrap().amount, 4);
    }

    #[test]
    fn save_all_with_limit_at_under_limit_writes_full_queue() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("orders.json");

        let mut q = VecDeque::new();
        for i in 0..3u32 { q.push_back(make(i)); }

        Order::save_all_with_limit_at(&q, 10, &file_path).unwrap();

        let json = std::fs::read_to_string(&file_path).unwrap();
        let on_disk: VecDeque<Order> = serde_json::from_str(&json).unwrap();
        assert_eq!(on_disk.len(), 3);
        assert_eq!(on_disk.front().unwrap().amount, 0);
        assert_eq!(on_disk.back().unwrap().amount, 2);
    }

    #[test]
    fn save_all_with_limit_at_boundary_len_equals_max_is_no_prune() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("orders.json");

        let mut q = VecDeque::new();
        for i in 0..3u32 { q.push_back(make(i)); }

        Order::save_all_with_limit_at(&q, 3, &file_path).unwrap();

        let json = std::fs::read_to_string(&file_path).unwrap();
        let on_disk: VecDeque<Order> = serde_json::from_str(&json).unwrap();
        assert_eq!(on_disk.len(), 3);
        assert_eq!(on_disk.front().unwrap().amount, 0);
    }

    #[test]
    fn currency_order_round_trip_preserves_currency_amount() {
        // AddCurrency / RemoveCurrency carry the real magnitude in `currency_amount`
        // while `amount` stays 0. Round-tripping through JSON must preserve both.
        for (ot, mag) in [
            (OrderType::AddCurrency, 1.0),
            (OrderType::RemoveCurrency, 10_000.5),
        ] {
            let o = Order {
                order_type: ot.clone(),
                item: crate::types::ItemId::new("diamond").unwrap(),
                amount: 0,
                currency_amount: mag,
                user_uuid: "op-1".to_string(),
            };
            let j = serde_json::to_string(&o).unwrap();
            let back: Order = serde_json::from_str(&j).unwrap();
            assert_eq!(back, o);
            assert_eq!(back.currency_amount, mag);
        }
    }

    #[test]
    fn buy_constructor_shape() {
        let item = ItemId::new("iron_ingot").unwrap();
        let o = Order::buy(item.clone(), 7, 14.5, "user-1".to_string());
        assert_eq!(o.order_type, OrderType::Buy);
        assert_eq!(o.item, item);
        assert_eq!(o.amount, 7);
        assert_eq!(o.currency_amount, 14.5);
        assert_eq!(o.user_uuid, "user-1");
    }

    #[test]
    fn sell_constructor_shape() {
        let item = ItemId::new("gold_ingot").unwrap();
        let o = Order::sell(item.clone(), 3, 6.0, "user-2".to_string());
        assert_eq!(o.order_type, OrderType::Sell);
        assert_eq!(o.item, item);
        assert_eq!(o.amount, 3);
        assert_eq!(o.currency_amount, 6.0);
        assert_eq!(o.user_uuid, "user-2");
    }

    #[test]
    fn deposit_balance_constructor_shape() {
        let o = Order::deposit_balance(42.0, "user-3".to_string());
        assert_eq!(o.order_type, OrderType::DepositBalance);
        assert_eq!(o.item, ItemId::new("diamond").unwrap());
        assert_eq!(o.amount, 42);
        assert_eq!(o.currency_amount, 42.0);
        assert_eq!(o.user_uuid, "user-3");
    }

    #[test]
    fn withdraw_balance_constructor_shape() {
        let o = Order::withdraw_balance(17.0, "user-4".to_string());
        assert_eq!(o.order_type, OrderType::WithdrawBalance);
        assert_eq!(o.item, ItemId::new("diamond").unwrap());
        assert_eq!(o.amount, 17);
        assert_eq!(o.currency_amount, 17.0);
        assert_eq!(o.user_uuid, "user-4");
    }

    #[test]
    fn add_currency_constructor_shape() {
        let item = ItemId::new("emerald").unwrap();
        let o = Order::add_currency(item.clone(), 25.5, "op-1".to_string());
        assert_eq!(o.order_type, OrderType::AddCurrency);
        assert_eq!(o.item, item);
        assert_eq!(o.amount, 0);
        assert_eq!(o.currency_amount, 25.5);
        assert_eq!(o.user_uuid, "op-1");
    }

    #[test]
    fn remove_currency_constructor_shape() {
        let item = ItemId::new("emerald").unwrap();
        let o = Order::remove_currency(item.clone(), 9.25, "op-2".to_string());
        assert_eq!(o.order_type, OrderType::RemoveCurrency);
        assert_eq!(o.item, item);
        assert_eq!(o.amount, 0);
        assert_eq!(o.currency_amount, 9.25);
        assert_eq!(o.user_uuid, "op-2");
    }

    #[test]
    fn add_item_constructor_shape() {
        let item = ItemId::new("oak_log").unwrap();
        let o = Order::add_item(item.clone(), 64, "op-3".to_string());
        assert_eq!(o.order_type, OrderType::AddItem);
        assert_eq!(o.item, item);
        assert_eq!(o.amount, 64);
        assert_eq!(o.currency_amount, 0.0);
        assert_eq!(o.user_uuid, "op-3");
    }

    #[test]
    fn remove_item_constructor_shape() {
        let item = ItemId::new("cobblestone").unwrap();
        let o = Order::remove_item(item.clone(), 32, "op-4".to_string());
        assert_eq!(o.order_type, OrderType::RemoveItem);
        assert_eq!(o.item, item);
        assert_eq!(o.amount, 32);
        assert_eq!(o.currency_amount, 0.0);
        assert_eq!(o.user_uuid, "op-4");
    }

    #[test]
    fn legacy_order_without_currency_amount_deserializes_to_zero() {
        // Existing data/orders.json files predate the field; serde(default) must
        // let them load with currency_amount = 0.0 rather than failing.
        let legacy = r#"{
            "order_type": "Buy",
            "item": "diamond",
            "amount": 5,
            "user_uuid": "u-1"
        }"#;
        let o: Order = serde_json::from_str(legacy).unwrap();
        assert_eq!(o.currency_amount, 0.0);
        assert_eq!(o.amount, 5);
    }
}
