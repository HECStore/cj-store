//! State management and persistence

use tracing::warn;

use crate::messages::ChestSyncReport;
use crate::types::{Order, Pair, Trade, User};
use super::Store;

/// Apply chest sync report from bot (merges with existing storage)
/// 
/// Slots with count >= 0 are updated, slots with count == -1 are left unchanged.
/// This allows partial updates where only processed slots are reported.
pub fn apply_chest_sync(store: &mut Store, report: ChestSyncReport) -> Result<(), String> {
    // Find the chest and merge slot counts with the bot-reported truth.
    for node in &mut store.storage.nodes {
        for chest in &mut node.chests {
            if chest.id == report.chest_id {
                // Node 0 has reserved chests whose item type is fixed by protocol.
                // We warn (rather than erroring) so a misbehaving bot report is
                // logged but cannot corrupt the reserved-chest assignment.
                if chest.id == crate::constants::DIAMOND_CHEST_ID {
                    // Chest 0: dedicated for diamonds
                    if chest.item != "diamond" {
                        warn!("Attempted to change node 0 chest 0 item from diamond to {}, enforcing diamond", report.item);
                    }
                    chest.item = "diamond".to_string();
                } else if chest.id == crate::constants::OVERFLOW_CHEST_ID {
                    // Chest 1: dedicated for overflow (mixed items allowed, but keep the "overflow" item type)
                    if chest.item != crate::constants::OVERFLOW_CHEST_ITEM {
                        warn!("Attempted to change node 0 chest 1 item from overflow to {}, enforcing overflow", report.item);
                    }
                    chest.item = crate::constants::OVERFLOW_CHEST_ITEM.to_string();
                } else {
                    chest.item = report.item;
                }
                
                // Slot merge semantics:
                //   count >=  0 -> authoritative new value, overwrite in place
                //   count == -1 -> sentinel for "bot did not inspect this slot",
                //                  preserve the existing stored value
                // The bounds check guards against a report whose slot array is
                // longer than our configured chest layout.
                for (i, &new_count) in report.amounts.iter().enumerate() {
                    if new_count >= 0 && i < chest.amounts.len() {
                        chest.amounts[i] = new_count;
                    }
                }
                
                store.dirty = true;
                return Ok(());
            }
        }
    }
    Err(format!("Chest {} not found in storage", report.chest_id))
}

/// Save all store data to disk
/// 
/// Uses the config values for max_orders when pruning orders before save.
pub fn save(store: &Store) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing::info!("[Save] Starting save operation for all store data");
    tracing::info!("[Save] Saving {} pairs", store.pairs.len());
    Pair::save_all(&store.pairs)?;
    tracing::info!("[Save] Pairs saved successfully");
    
    tracing::info!("[Save] Saving {} users", store.users.len());
    User::save_all(&store.users)?;
    tracing::info!("[Save] Users saved successfully");
    
    tracing::info!("[Save] Saving {} orders (max: {})", store.orders.len(), store.config.max_orders);
    Order::save_all_with_limit(&store.orders, store.config.max_orders)?;
    tracing::info!("[Save] Orders saved successfully");
    
    tracing::info!("[Save] Saving {} trades", store.trades.len());
    Trade::save_all(&store.trades)?;
    tracing::info!("[Save] Trades saved successfully");
    
    tracing::info!("[Save] Saving storage ({} nodes)", store.storage.nodes.len());
    store.storage.save().map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())) as Box<dyn std::error::Error + Send + Sync>)?;
    tracing::info!("[Save] Storage saved successfully");
    
    tracing::info!("[Save] All store data saved successfully");
    Ok(())
}

/// Audit store state and optionally repair issues.
///
/// Walks users, storage chests and pairs looking for broken invariants and
/// returns a human-readable list of problems. When `repair` is true, issues
/// that have a safe automatic fix (currently: Pair.item_stock drifting from
/// the physical chest total) are corrected in place, and the returned vec is
/// prefixed with a "Repair applied" marker line so callers can tell repairs
/// ran even when no other issues remain.
pub fn audit_state(store: &mut Store, repair: bool) -> Vec<String> {
    let mut issues = Vec::new();

    // Users: NaN/Inf would poison any later arithmetic, and negative balances
    // would let users spend money they never had -- both must be flagged.
    for user in store.users.values() {
        if !user.balance.is_finite() {
            issues.push(format!("User {} has non-finite balance", user.username));
        }
        if user.balance < 0.0 {
            issues.push(format!("User {} has negative balance: {}", user.username, user.balance));
        }
    }

    // Storage
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
            // Get the item-specific shulker capacity from the pair's stack_size
            let shulker_capacity = if chest.item.is_empty() {
                crate::types::Storage::DEFAULT_SHULKER_CAPACITY
            } else {
                // Look up stack_size from pairs, default to 64 if not found
                store.pairs.get(&chest.item)
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

    // Pairs vs storage + numeric sanity.
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
            issues.push(format!(
                "Pair {} item_stock {} != physical {}",
                pair.item, pair.item_stock, physical
            ));
            if repair {
                pair.item_stock = physical;
            }
        }
    }

    if repair {
        issues.insert(0, "Repair applied: recomputed Pair.item_stock from Storage".to_string());
    }

    issues
}

/// Assert store invariants, optionally repairing issues.
///
/// Wraps [`audit_state`] and turns any remaining (unfixable) issues into an
/// `Err`. Intended for use at well-defined checkpoints (e.g. after loading,
/// before saving) where silently continuing on a broken state would be worse
/// than aborting the operation.
pub fn assert_invariants(store: &mut Store, context: &str, repair: bool) -> Result<(), String> {
    use crate::store::utils;
    let issues = audit_state(store, repair);
    // When repair is on, audit_state always prepends a "Repair applied..."
    // marker line; skip it so we only surface problems that repair couldn't
    // handle.
    let relevant = if repair {
        issues.into_iter().skip(1).collect::<Vec<_>>()
    } else {
        issues
    };
    if relevant.is_empty() {
        return Ok(());
    }
    Err(utils::fmt_issues(
        &format!("Invariant violation ({})", context),
        &relevant,
        8,
    ))
}
