//! State management and persistence

use tracing::{debug, warn};

use crate::error::StoreError;
use crate::messages::ChestSyncReport;
use crate::types::{ItemId, Order, Pair, Trade, User};
use super::Store;

/// Merge a bot-reported [`ChestSyncReport`] into the store's authoritative
/// storage view.
///
/// Slot semantics: an entry of `-1` is the "bot did not inspect this slot"
/// sentinel and preserves the stored value; any value `>= 0` overwrites.
/// Reserved chests on node 0 (diamond and overflow) have their `item` field
/// force-reset to the protocol-required value regardless of what the bot
/// reported — a misbehaving bot cannot corrupt the reserved-chest assignment.
pub fn apply_chest_sync(store: &mut Store, report: ChestSyncReport) -> Result<(), StoreError> {
    for node in &mut store.storage.nodes {
        for chest in &mut node.chests {
            if chest.id == report.chest_id {
                if chest.id == crate::constants::DIAMOND_CHEST_ID {
                    if chest.item != "diamond" {
                        warn!("Attempted to change node 0 chest 0 item from diamond to {}, enforcing diamond", report.item);
                    }
                    chest.item = ItemId::new("diamond").expect("diamond is a valid item ID");
                } else if chest.id == crate::constants::OVERFLOW_CHEST_ID {
                    if chest.item != crate::constants::OVERFLOW_CHEST_ITEM {
                        warn!("Attempted to change node 0 chest 1 item from overflow to {}, enforcing overflow", report.item);
                    }
                    chest.item = ItemId::new(crate::constants::OVERFLOW_CHEST_ITEM).expect("OVERFLOW_CHEST_ITEM is a valid item ID");
                } else {
                    // Fall back to EMPTY so an invalid id from the bot marks the
                    // chest unassigned rather than poisoning it with a partial string.
                    chest.item = ItemId::new(&report.item).unwrap_or(ItemId::EMPTY);
                }

                // Bounds check guards against a report whose slot array is
                // longer than our configured chest layout.
                let mut updated = 0usize;
                for (i, &new_count) in report.amounts.iter().enumerate() {
                    if new_count >= 0 && i < chest.amounts.len() {
                        chest.amounts[i] = new_count;
                        updated += 1;
                    }
                }

                store.dirty = true;
                debug!(
                    chest_id = chest.id,
                    item = %chest.item,
                    slots_updated = updated,
                    "chest sync applied"
                );
                return Ok(());
            }
        }
    }
    warn!(chest_id = report.chest_id, "chest sync failed: chest id not found in any node");
    Err(StoreError::ChestOp(format!(
        "Chest {} not found in storage",
        report.chest_id
    )))
}

/// Persist every in-memory collection (pairs, users, orders, trades, storage)
/// to disk.
///
/// Trims `store.orders` and `store.trades` to their configured caps before
/// writing — handlers append on every transaction without checking the cap, so
/// the in-memory deques would otherwise grow unbounded between restarts.
/// Trimming at save time means a single autosave cadence bounds the working
/// set, and the on-disk files mirror what we kept in memory.
///
/// Orders are also truncated by `Order::save_all_with_limit` as a belt-and-
/// braces second cap; the rest are written in full. Returns the first I/O
/// error encountered; partial progress may have been committed to disk (each
/// type writes independently).
///
/// Users are written via `User::save_dirty` using the store's `dirty_users`
/// set so only users whose balance/operator changed since the last save get
/// rewritten + fsynced. The caller is responsible for clearing
/// `store.dirty_users` after this returns `Ok(())`.
pub fn save(store: &mut Store) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if store.orders.len() > store.config.max_orders {
        let drop = store.orders.len() - store.config.max_orders;
        store.orders.drain(..drop);
    }
    if store.trades.len() > store.config.max_trades_in_memory {
        let drop = store.trades.len() - store.config.max_trades_in_memory;
        store.trades.drain(..drop);
    }

    debug!("saving pairs={} users={} (dirty={}) orders={} trades={} nodes={}",
        store.pairs.len(), store.users.len(), store.dirty_users.len(),
        store.orders.len(), store.trades.len(), store.storage.nodes.len());

    Pair::save_all(&store.pairs)?;
    User::save_dirty(&store.users, &store.dirty_users)?;
    Order::save_all_with_limit(&store.orders, store.config.max_orders)?;
    Trade::save_all(&store.trades)?;
    store.storage.save().map_err(|e| Box::new(std::io::Error::other(e.to_string())) as Box<dyn std::error::Error + Send + Sync>)?;

    tracing::info!(
        pairs = store.pairs.len(),
        users = store.users.len(),
        orders = store.orders.len(),
        trades = store.trades.len(),
        nodes = store.storage.nodes.len(),
        "store saved"
    );
    Ok(())
}

/// Structured report from [`audit_state`].
///
/// `issues` is the plain list of problems found (safe-to-repair issues are
/// removed from this list when `repair=true` and the fix succeeded).
/// `repair_applied` is `true` iff `audit_state` was called with `repair=true`;
/// callers use it to decide whether to persist the store. Keeping the two
/// fields separate avoids the old fragile coupling where repair status was
/// smuggled as a "Repair applied..." string at position 0 of the vec.
#[derive(Debug, Clone, Default)]
pub struct AuditReport {
    pub issues: Vec<String>,
    pub repair_applied: bool,
}

impl AuditReport {
    /// Render the report as human-readable lines, suitable for pushing into a
    /// chat/CLI message. The "Repair applied..." marker (if any) is emitted
    /// first so the output is visually similar to the pre-refactor format.
    pub fn to_lines(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.issues.len() + usize::from(self.repair_applied));
        if self.repair_applied {
            out.push("Repair applied: recomputed Pair.item_stock from Storage".to_string());
        }
        out.extend(self.issues.iter().cloned());
        out
    }
}

/// Audit store state and optionally repair issues.
///
/// Walks users, storage chests and pairs looking for broken invariants and
/// returns a structured `AuditReport`. When `repair` is true, issues that
/// have a safe automatic fix (currently: `Pair.item_stock` drifting from the
/// physical chest total) are corrected in place and removed from the issues
/// list; `report.repair_applied` is set so callers can tell repairs ran even
/// when no other issues remain.
pub fn audit_state(store: &mut Store, repair: bool) -> AuditReport {
    let mut issues = Vec::new();
    let mut repairs: Vec<(String, i32, i32)> = Vec::new();

    // NaN/Inf would poison any later arithmetic, and negative balances would
    // let users spend money they never had -- both must be flagged.
    for user in store.users.values() {
        if !user.balance.is_finite() {
            issues.push(format!("User {} has non-finite balance", user.username));
        }
        if user.balance < 0.0 {
            issues.push(format!("User {} has negative balance: {}", user.username, user.balance));
        }
    }

    for node in &store.storage.nodes {
        for chest in &node.chests {
            if chest.amounts.len() != crate::types::Storage::SLOTS_PER_CHEST {
                issues.push(format!(
                    "Chest {} amounts len is {} (expected {})",
                    chest.id,
                    chest.amounts.len(),
                    crate::types::Storage::SLOTS_PER_CHEST
                ));
            }
            // Per-slot max is the item's shulker capacity (27 slots × stack size).
            // Unassigned chests fall back to the 64-stack default.
            let shulker_capacity = if chest.item.is_empty() {
                crate::types::Storage::DEFAULT_SHULKER_CAPACITY
            } else {
                store.pairs.get(chest.item.as_str())
                    .map(|p| crate::types::Pair::shulker_capacity_for_stack_size(p.stack_size))
                    .unwrap_or(crate::types::Storage::DEFAULT_SHULKER_CAPACITY)
            };

            for (i, a) in chest.amounts.iter().enumerate() {
                // -1 is the legal "unknown/unchecked" sentinel (see
                // apply_chest_sync); anything more negative is corruption.
                if *a < -1 {
                    issues.push(format!("Chest {} slot {} has invalid amount {}", chest.id, i, a));
                }
                if *a > shulker_capacity {
                    issues.push(format!(
                        "Chest {} (item: {}) slot {} exceeds max capacity ({}): {}",
                        chest.id,
                        if chest.item.is_empty() { "unassigned" } else { &chest.item },
                        i,
                        shulker_capacity,
                        a
                    ));
                }
            }
        }
    }

    // The cached Pair.item_stock must agree with the sum of physical chest
    // slots for that item; drift here typically indicates a missed sync or a
    // crash between a trade and a save. When repair is enabled we trust the
    // physical storage as the source of truth and rewrite the cached value.
    for pair in store.pairs.values_mut() {
        if pair.item_stock < 0 {
            issues.push(format!(
                "Pair {} has negative item_stock {}",
                pair.item, pair.item_stock
            ));
        }
        if pair.currency_stock < 0.0 {
            issues.push(format!(
                "Pair {} has negative currency_stock {}",
                pair.item, pair.currency_stock
            ));
        }
        let physical = store.storage.total_item_amount(&pair.item);
        if pair.item_stock != physical {
            if repair {
                // Safe auto-fix: rewrite the cached value from physical storage
                // and don't record the issue, so downstream `assert_invariants`
                // treats this checkpoint as clean.
                repairs.push((pair.item.to_string(), pair.item_stock, physical));
                pair.item_stock = physical;
            } else {
                issues.push(format!(
                    "Pair {} item_stock {} != physical {}",
                    pair.item, pair.item_stock, physical
                ));
            }
        }
    }

    if !issues.is_empty() {
        warn!(count = issues.len(), repair, "audit found invariant issues");
    }
    for (item, before, after) in &repairs {
        warn!(item = %item, before, after, "audit repaired pair item_stock drift");
    }

    AuditReport { issues, repair_applied: repair }
}

/// Assert store invariants, optionally repairing issues.
///
/// Wraps [`audit_state`] and turns any remaining (unfixable) issues into an
/// `Err`. Intended for use at well-defined checkpoints (e.g. after loading,
/// before saving) where silently continuing on a broken state would be worse
/// than aborting the operation.
pub fn assert_invariants(store: &mut Store, context: &str, repair: bool) -> Result<(), StoreError> {
    use crate::store::utils;
    let report = audit_state(store, repair);
    if report.issues.is_empty() {
        return Ok(());
    }
    tracing::error!(
        context = context,
        count = report.issues.len(),
        repair,
        "invariant violation detected"
    );
    Err(StoreError::InvariantViolation(utils::fmt_issues(
        &format!("({})", context),
        &report.issues,
        8,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::messages::BotInstruction;
    use crate::types::{Node, Pair, Position, Storage};
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    fn test_config() -> Config {
        Config {
            position: Position { x: 0, y: 64, z: 0 },
            fee: 0.125,
            account_email: String::new(),
            server_address: "test".to_string(),
            buffer_chest_position: None,
            trade_timeout_ms: 5_000,
            pathfinding_timeout_ms: 5_000,
            max_orders: 1000,
            max_trades_in_memory: 1000,
            autosave_interval_secs: 10,
            chat: crate::config::ChatConfig::default(),
        }
    }

    fn test_storage() -> Storage {
        let origin = Position { x: 0, y: 64, z: 0 };
        let mut storage = Storage::new(&origin);
        storage.nodes.push(Node::new(0, &origin));
        storage
    }

    fn build_store(
        pairs: HashMap<String, Pair>,
        users: HashMap<String, User>,
        storage: Storage,
    ) -> Store {
        let (tx, _rx) = mpsc::channel::<BotInstruction>(16);
        Store::new_for_test(tx, test_config(), pairs, users, storage)
    }

    // ---------- apply_chest_sync ----------

    #[test]
    fn apply_chest_sync_overwrites_nonnegative_slots_and_preserves_sentinel() {
        let mut store = build_store(HashMap::new(), HashMap::new(), test_storage());
        // Pre-seed chest 2 (non-reserved) with existing values.
        {
            let chest = &mut store.storage.nodes[0].chests[2];
            chest.amounts[0] = 100;
            chest.amounts[1] = 200;
            chest.amounts[2] = 300;
        }

        let mut amounts = [-1i32; crate::constants::DOUBLE_CHEST_SLOTS];
        amounts[0] = 50; // overwrite
        // amounts[1] stays -1 (preserve)
        amounts[2] = 0; // overwrite with zero (still >= 0, so authoritative)
        let report = ChestSyncReport {
            chest_id: store.storage.nodes[0].chests[2].id,
            item: "iron_ingot".to_string(),
            amounts,
        };

        apply_chest_sync(&mut store, report).expect("sync should succeed");
        let chest = &store.storage.nodes[0].chests[2];
        assert_eq!(chest.amounts[0], 50, "slot 0 should be overwritten");
        assert_eq!(chest.amounts[1], 200, "slot 1 sentinel (-1) should preserve prior value");
        assert_eq!(chest.amounts[2], 0, "slot 2 zero should overwrite");
        assert_eq!(chest.item.as_str(), "iron_ingot");
        assert!(store.dirty);
    }

    #[test]
    fn apply_chest_sync_forces_diamond_item_on_reserved_chest() {
        let mut store = build_store(HashMap::new(), HashMap::new(), test_storage());
        let report = ChestSyncReport {
            chest_id: crate::constants::DIAMOND_CHEST_ID,
            // Bot reports the wrong item; reserved-slot protection must override.
            item: "iron_ingot".to_string(),
            amounts: [-1i32; crate::constants::DOUBLE_CHEST_SLOTS],
        };
        apply_chest_sync(&mut store, report).expect("sync should succeed");
        assert_eq!(
            store.storage.nodes[0].chests[crate::constants::DIAMOND_CHEST_ID as usize]
                .item
                .as_str(),
            "diamond"
        );
    }

    #[test]
    fn apply_chest_sync_forces_overflow_item_on_reserved_chest() {
        let mut store = build_store(HashMap::new(), HashMap::new(), test_storage());
        let report = ChestSyncReport {
            chest_id: crate::constants::OVERFLOW_CHEST_ID,
            item: "diamond".to_string(),
            amounts: [-1i32; crate::constants::DOUBLE_CHEST_SLOTS],
        };
        apply_chest_sync(&mut store, report).expect("sync should succeed");
        assert_eq!(
            store.storage.nodes[0].chests[crate::constants::OVERFLOW_CHEST_ID as usize]
                .item
                .as_str(),
            crate::constants::OVERFLOW_CHEST_ITEM
        );
    }

    #[test]
    fn apply_chest_sync_invalid_item_id_becomes_empty() {
        let mut store = build_store(HashMap::new(), HashMap::new(), test_storage());
        let chest_id = store.storage.nodes[0].chests[2].id;
        let report = ChestSyncReport {
            chest_id,
            // Bare "minecraft:" normalizes to empty and fails ItemId::new;
            // apply_chest_sync should substitute EMPTY rather than panicking.
            item: "minecraft:".to_string(),
            amounts: [-1i32; crate::constants::DOUBLE_CHEST_SLOTS],
        };
        apply_chest_sync(&mut store, report).expect("sync should succeed");
        assert!(store.storage.nodes[0].chests[2].item.is_empty());
    }

    #[test]
    fn apply_chest_sync_unknown_chest_id_returns_error_with_id() {
        let mut store = build_store(HashMap::new(), HashMap::new(), test_storage());
        let report = ChestSyncReport {
            chest_id: 9999,
            item: "iron_ingot".to_string(),
            amounts: [-1i32; crate::constants::DOUBLE_CHEST_SLOTS],
        };
        let err = apply_chest_sync(&mut store, report).unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("9999"), "error should include the missing chest id: got {msg}");
        assert!(!store.dirty, "failed sync must not mark store dirty");
    }

    // ---------- audit_state ----------

    #[test]
    fn audit_state_clean_store_reports_no_issues() {
        let store_storage = test_storage();
        let mut store = build_store(HashMap::new(), HashMap::new(), store_storage);
        let report = audit_state(&mut store, false);
        assert!(report.issues.is_empty(), "clean store should have no issues: {:?}", report.issues);
        assert!(!report.repair_applied);
    }

    #[test]
    fn audit_state_flags_nonfinite_and_negative_balances() {
        let mut users = HashMap::new();
        users.insert(
            "u1".to_string(),
            User { uuid: "u1".to_string(), username: "alice".to_string(), balance: f64::NAN, operator: false },
        );
        users.insert(
            "u2".to_string(),
            User { uuid: "u2".to_string(), username: "bob".to_string(), balance: -5.0, operator: false },
        );
        let mut store = build_store(HashMap::new(), users, test_storage());
        let report = audit_state(&mut store, false);
        assert!(report.issues.iter().any(|i| i.contains("alice") && i.contains("non-finite")));
        assert!(report.issues.iter().any(|i| i.contains("bob") && i.contains("negative balance")));
    }

    #[test]
    fn audit_state_flags_slot_below_sentinel_and_above_capacity() {
        let mut store = build_store(HashMap::new(), HashMap::new(), test_storage());
        let chest_id = store.storage.nodes[0].chests[2].id;
        {
            let chest = &mut store.storage.nodes[0].chests[2];
            chest.item = ItemId::new("iron_ingot").unwrap();
            chest.amounts[0] = -5;      // below -1 sentinel: corruption
            chest.amounts[1] = 10_000;  // exceeds default shulker capacity (1728)
        }
        let report = audit_state(&mut store, false);
        let has_invalid = report.issues.iter().any(|i| i.contains(&format!("Chest {}", chest_id)) && i.contains("invalid amount"));
        let has_capacity = report.issues.iter().any(|i| i.contains(&format!("Chest {}", chest_id)) && i.contains("exceeds max capacity"));
        assert!(has_invalid, "expected invalid-amount issue, got {:?}", report.issues);
        assert!(has_capacity, "expected capacity issue, got {:?}", report.issues);
    }

    #[test]
    fn audit_state_repair_fixes_pair_stock_drift_and_sets_flag() {
        // Physical storage has 100 iron; pair says 42.
        let mut storage = test_storage();
        let origin = Position { x: 0, y: 64, z: 0 };
        storage.nodes[0] = Node::new(0, &origin);
        seed_iron(&mut storage, 100);
        let mut pairs = HashMap::new();
        pairs.insert(
            "iron_ingot".to_string(),
            Pair {
                item: ItemId::new("iron_ingot").unwrap(),
                stack_size: 64,
                item_stock: 42,
                currency_stock: 0.0,
            },
        );
        let mut store = build_store(pairs, HashMap::new(), storage);

        // repair = false: drift is reported, not fixed.
        let report = audit_state(&mut store, false);
        assert!(report.issues.iter().any(|i| i.contains("iron_ingot") && i.contains("42") && i.contains("100")));
        assert_eq!(store.pairs["iron_ingot"].item_stock, 42);
        assert!(!report.repair_applied);

        // repair = true: drift is fixed, the repair flag is set, and the
        // now-fixed issue is NOT re-reported in issues (so assert_invariants
        // treats a repaired checkpoint as clean).
        let report = audit_state(&mut store, true);
        assert_eq!(store.pairs["iron_ingot"].item_stock, 100, "repair should rewrite from physical");
        assert!(report.repair_applied);
        assert!(
            !report.issues.iter().any(|i| i.contains("iron_ingot") && i.contains("!=")),
            "repaired drift should not be re-listed as an issue: {:?}", report.issues
        );
    }

    fn seed_iron(storage: &mut Storage, count: i32) {
        let chest = &mut storage.nodes[0].chests[2];
        chest.item = ItemId::new("iron_ingot").unwrap();
        chest.amounts[0] = count;
    }

    #[test]
    fn audit_state_flags_negative_pair_stocks() {
        let mut pairs = HashMap::new();
        pairs.insert(
            "iron_ingot".to_string(),
            Pair {
                item: ItemId::new("iron_ingot").unwrap(),
                stack_size: 64,
                item_stock: -1,
                currency_stock: -2.0,
            },
        );
        let mut store = build_store(pairs, HashMap::new(), test_storage());
        let report = audit_state(&mut store, false);
        assert!(report.issues.iter().any(|i| i.contains("negative item_stock")));
        assert!(report.issues.iter().any(|i| i.contains("negative currency_stock")));
    }

    // ---------- assert_invariants ----------

    #[test]
    fn assert_invariants_returns_ok_when_clean() {
        let mut store = build_store(HashMap::new(), HashMap::new(), test_storage());
        assert!(assert_invariants(&mut store, "test-clean", false).is_ok());
    }

    #[test]
    fn assert_invariants_error_includes_context() {
        let mut users = HashMap::new();
        users.insert(
            "u".to_string(),
            User { uuid: "u".to_string(), username: "eve".to_string(), balance: -1.0, operator: false },
        );
        let mut store = build_store(HashMap::new(), users, test_storage());
        let err = assert_invariants(&mut store, "pre-checkpoint", false).unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("pre-checkpoint"), "error message should include context: {msg}");
    }

    #[test]
    fn assert_invariants_repair_clears_fixable_drift() {
        let mut storage = test_storage();
        seed_iron(&mut storage, 50);
        let mut pairs = HashMap::new();
        pairs.insert(
            "iron_ingot".to_string(),
            Pair {
                item: ItemId::new("iron_ingot").unwrap(),
                stack_size: 64,
                item_stock: 10, // drift; only issue
                currency_stock: 0.0,
            },
        );
        let mut store = build_store(pairs, HashMap::new(), storage);
        // With repair enabled, the only issue (drift) is fixed and removed,
        // so assert_invariants should return Ok.
        assert!(assert_invariants(&mut store, "post-op", true).is_ok());
        assert_eq!(store.pairs["iron_ingot"].item_stock, 50);
    }

    // ---------- AuditReport::to_lines ----------

    #[test]
    fn audit_report_to_lines_prepends_repair_marker() {
        let r = AuditReport {
            issues: vec!["issue A".to_string()],
            repair_applied: true,
        };
        let lines = r.to_lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("Repair applied"));
        assert_eq!(lines[1], "issue A");
    }

    #[test]
    fn audit_report_to_lines_omits_marker_when_no_repair() {
        let r = AuditReport {
            issues: vec!["x".to_string(), "y".to_string()],
            repair_applied: false,
        };
        assert_eq!(r.to_lines(), vec!["x".to_string(), "y".to_string()]);
    }
}
