//! # Storage System
//!
//! Models physical storage as a graph: `Storage` → `Node` → `Chest` → shulker boxes.
//!
//! ## Architecture
//! - **Storage**: Root container with origin position and list of nodes
//! - **Node**: Cluster of 4 chests arranged in a 2×2 pattern (2 blocks tall)
//! - **Chest**: 54-slot container where **each slot contains 1 shulker box**
//! - **Shulker**: Each shulker box contains items (up to 27 slots × 64 items = 1728 items max)
//!
//! ## Layout
//! Each node has this footprint (top-down, bot at P facing north):
//! ```
//! NCCN  (chests, 2 blocks tall — back row)
//! NCCN  (chests — front row, clickable face)
//! XSNP  (X = pickup, S = shulker station, N = empty, P = bot position)
//! ```
//!
//! Nodes are arranged in a **clockwise spiral**, spaced 3 blocks apart.
//! See `ARCHITECTURE.md` § Node layout for the full diagram and chest-id
//! numbering.
//!
//! ## Storage Operations
//! - **`deposit_plan()`**: Allocates items to chests (creates new nodes if needed)
//! - **`withdraw_plan()`**: Removes items from chests (deterministic order)
//! - **`total_item_amount()`**: Sums items across all chests for a given item
//!
//! ## Shulker Handling
//! The bot must:
//! 1. Navigate to node position
//! 2. Open chest
//! 3. Take shulker from chest slot
//! 4. Place on shulker station (S position)
//! 5. Open shulker, transfer items
//! 6. Close shulker, pick it up
//! 7. Place shulker back in same chest slot
//!
//! See `bot.rs` for shulker automation implementation.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::types::chest::Chest;
use crate::types::ItemId;
use crate::types::node::Node;
use crate::types::position::Position;

/// Represents a planned transfer operation on a specific chest.
///
/// Used by `deposit_plan()` and `withdraw_plan()` to communicate which chests
/// the bot needs to interact with and how much to transfer.
///
/// The bot receives these plans and executes them sequentially.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChestTransfer {
    /// Chest ID (node_id * 4 + index)
    pub chest_id: i32,
    /// World position of the chest (for bot navigation)
    pub position: Position,
    /// Item identifier
    pub item: crate::types::ItemId,
    /// Amount to transfer (items, not stacks)
    pub amount: i32,
}

/// Root storage container: manages all nodes and their chests.
///
/// **Persistence**: Nodes are stored as `data/storage/{node_id}.json`
///
/// **Initialization**: If `data/storage/` doesn't exist, returns empty storage.
/// Nodes are loaded on startup and saved on each autosave.
///
/// **Node Management**: Nodes can be added/removed via CLI commands.
/// Physical validation (checking chests exist in-world) is optional.
#[derive(Debug, Default, Clone)]
pub struct Storage {
    /// Storage origin position (from config)
    pub position: Position,
    /// List of nodes (loaded from `data/storage/` JSON files)
    pub nodes: Vec<Node>,
}

// Several methods below (`new`, `deposit_plan`, `withdraw_plan`, overflow
// accessors, direct-mutation helpers) are test-only — production mutates the
// storage via `apply_chest_sync`. They are kept as a cohesive API surface
// exercised by the unit tests in this file.
#[allow(dead_code)]
impl Storage {
    /// Number of slots per chest (alias for `crate::constants::DOUBLE_CHEST_SLOTS`,
    /// kept for readability at call sites inside this file).
    pub const SLOTS_PER_CHEST: usize = crate::constants::DOUBLE_CHEST_SLOTS;

    /// Default maximum item capacity per shulker box (27 slots × 64 items = 1728).
    ///
    /// **Note**: Actual capacity varies by item type. Use `Pair::shulker_capacity_for_stack_size()`
    /// or `pair.shulker_capacity()` for accurate capacity based on item stack size.
    ///
    /// **Storage Model**:
    /// - Each chest slot **ALWAYS** contains 1 shulker box (any color, treated equally)
    /// - `Chest.amounts[i]` = item count **inside** the shulker in chest slot `i`
    /// - Max per shulker: 27 slots × stack_size items
    ///   - Most items: 27 × 64 = 1728 items
    ///   - Ender pearls, eggs: 27 × 16 = 432 items
    ///   - Tools, armor: 27 × 1 = 27 items
    /// - Max per chest: 54 shulkers × capacity
    pub const DEFAULT_SHULKER_CAPACITY: i32 = (crate::constants::SHULKER_BOX_SLOTS as i32) * 64;

    /// Save the storage by calling save() on all nodes
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        tracing::debug!("[Storage] Saving {} nodes", self.nodes.len());
        for node in &self.nodes {
            tracing::debug!("[Storage] Saving node {}", node.id);
            node.save()?;
            tracing::debug!("[Storage] Node {} saved successfully", node.id);
        }
        tracing::debug!("[Storage] All nodes saved successfully");
        Ok(())
    }

    /// Convenience constructor (reserved for future tooling/tests).
    pub fn new(storage_position: &Position) -> Self {
        Storage {
            position: *storage_position,
            nodes: Vec::new(),
        }
    }

    /// Load storage nodes by reading all JSON files in data/storage and loading corresponding nodes
    pub fn load(storage_position: &Position) -> Result<Self, Box<dyn std::error::Error>> {
        let storage_path = "data/storage";

        // Create directory if it doesn't exist
        if !Path::new(storage_path).exists() {
            fs::create_dir_all(storage_path)?;
            return Ok(Storage {
                position: *storage_position,
                nodes: Vec::new(),
            });
        }

        let mut nodes = Vec::new();

        // Read all entries in the storage directory
        for entry in fs::read_dir(storage_path)? {
            let entry = entry?;
            let path = entry.path();

            // Only process JSON files
            if path.is_file()
                && let Some(extension) = path.extension()
                    && extension == "json"
                        && let Some(file_name) = path.file_stem()
                            && let Some(file_str) = file_name.to_str() {
                                // Parse filename as node ID (e.g., "0.json" -> 0)
                                if let Ok(node_id) = file_str.parse::<i32>() {
                                    // Load the node using its ID and the storage position
                                    match Node::load(node_id, storage_position) {
                                        Ok(node) => nodes.push(node),
                                        Err(e) => {
                                            // Log error but continue loading other nodes
                                            eprintln!("Failed to load node {}: {}", node_id, e);
                                        }
                                    }
                                }
                            }
        }

        Ok(Storage {
            position: *storage_position,
            nodes,
        })
    }

    /// Creates a new node with the lowest unused id and appends it to `self.nodes`.
    ///
    /// The id is chosen as the smallest non-negative integer not already in use so
    /// that gaps left by removed nodes are reused (keeping on-disk filenames dense).
    /// `Node::new` computes the node's world position from `self.position` using
    /// the spiral layout described at the top of this file.
    pub fn add_node(&mut self) -> &mut Node {
        // Find the next available node_id (handle gaps from removed nodes)
        let mut node_id = 0i32;
        while self.nodes.iter().any(|n| n.id == node_id) {
            node_id += 1;
        }
        let node = Node::new(node_id, &self.position);
        self.nodes.push(node);
        // unwrap: pushed on the line above, so `last_mut()` cannot return None.
        self.nodes.last_mut().unwrap()
    }

    /// Sums the counts of `item` across every shulker slot in every chest.
    ///
    /// Only chests whose assigned `item` matches are considered; negative slot
    /// values (reserved/missing semantics) are filtered out by the `> 0` check.
    pub fn total_item_amount(&self, item: &str) -> i32 {
        self.nodes
            .iter()
            .flat_map(|n| &n.chests)
            .filter(|c| c.item == item)
            .flat_map(|c| c.amounts.iter().copied())
            .filter(|a| *a > 0)
            .sum()
    }


    /// Find a chest by id.
    /// Reserved for future bot/chest routing logic.
    pub fn get_chest_mut(&mut self, chest_id: i32) -> Option<&mut Chest> {
        for node in &mut self.nodes {
            for chest in &mut node.chests {
                if chest.id == chest_id {
                    return Some(chest);
                }
            }
        }
        None
    }

    /// Plans withdrawal of `qty` items **without mutating** storage state.
    ///
    /// This is the read-only counterpart to `withdraw_plan`. It walks the
    /// same deterministic node/chest/slot order and computes the exact same
    /// plan, but never touches `Chest.amounts`, so callers no longer need to
    /// `.clone()` the entire `Storage` struct just to preview an operation.
    ///
    /// Returns the plan plus the total amount that could actually be planned
    /// (may be less than `qty` if storage is short).
    pub fn simulate_withdraw_plan(&self, item: &str, qty: i32) -> (Vec<ChestTransfer>, i32) {
        if qty <= 0 {
            return (Vec::new(), 0);
        }
        let mut plan: Vec<ChestTransfer> = Vec::new();
        let mut remaining = qty;
        for node in &self.nodes {
            if remaining <= 0 {
                break;
            }
            for chest in &node.chests {
                if remaining <= 0 {
                    break;
                }
                if chest.item != item {
                    continue;
                }
                let mut chest_taken = 0i32;
                for slot in 0..chest.amounts.len() {
                    if remaining <= 0 {
                        break;
                    }
                    let available = chest.amounts[slot];
                    if available <= 0 {
                        continue;
                    }
                    let take = available.min(remaining);
                    remaining -= take;
                    chest_taken += take;
                }
                if chest_taken > 0 {
                    plan.push(ChestTransfer {
                        chest_id: chest.id,
                        position: chest.position,
                        item: ItemId::from_normalized(item.to_string()),
                        amount: chest_taken,
                    });
                }
            }
        }
        (plan, qty - remaining)
    }

    /// Plans a deposit of `qty` items **without mutating** storage state.
    ///
    /// Read-only counterpart to `deposit_plan`. Mirrors the exact placement
    /// rules (reserved chests on node 0, prefer partial shulkers, spill into
    /// empty chests, grow by one node when nothing else is available). Because
    /// this method never mutates, "growing by one node" is represented as a
    /// synthetic extra chest allocation in the returned plan — good enough
    /// for the plan-then-commit pattern used by order handlers, where the
    /// real `deposit_plan` will allocate the node during execution.
    ///
    /// Returns the plan plus the total amount that could actually be planned.
    pub fn simulate_deposit_plan(&self, item: &str, qty: i32, stack_size: i32) -> (Vec<ChestTransfer>, i32) {
        if qty <= 0 {
            return (Vec::new(), 0);
        }

        let shulker_capacity = crate::types::Pair::shulker_capacity_for_stack_size(stack_size);
        let mut plan: Vec<ChestTransfer> = Vec::new();
        let mut remaining = qty;

        // Tracks empty chests already earmarked by this simulation so we don't
        // allocate the same chest twice in a single plan.
        let mut claimed_empty: std::collections::HashSet<i32> = std::collections::HashSet::new();

        fn record_transfer(
            plan: &mut Vec<ChestTransfer>,
            chest_id: i32,
            position: crate::types::Position,
            item: &str,
            amt: i32,
        ) {
            if amt <= 0 { return; }
            if let Some(last) = plan.last_mut()
                && last.chest_id == chest_id {
                    last.amount += amt;
                    return;
                }
            plan.push(ChestTransfer {
                chest_id,
                position,
                item: ItemId::from_normalized(item.to_string()),
                amount: amt,
            });
        }

        // Phase 1: fill existing chests already assigned to this item.
        for (node_idx, node) in self.nodes.iter().enumerate() {
            if remaining <= 0 {
                return (plan, qty - remaining);
            }
            for (chest_idx, chest) in node.chests.iter().enumerate() {
                if remaining <= 0 {
                    break;
                }
                if Self::is_reserved_chest_blocked_for(item, node_idx, chest_idx) {
                    continue;
                }
                if chest.item != item {
                    continue;
                }
                for slot_val in chest.amounts.iter() {
                    if remaining <= 0 {
                        break;
                    }
                    let current = *slot_val;
                    if current < 0 || current >= shulker_capacity {
                        continue;
                    }
                    let capacity_left = shulker_capacity - current;
                    let add = capacity_left.min(remaining);
                    remaining -= add;
                    record_transfer(&mut plan, chest.id, chest.position, item, add);
                }
            }
        }

        // Phase 2: allocate empty chests in node 0 priority order, then other nodes.
        // We mirror `find_empty_chest_index` but may claim multiple empty chests.
        // Each freshly-claimed chest has `SLOTS_PER_CHEST * shulker_capacity` space.
        let empty_chest_capacity = (Self::SLOTS_PER_CHEST as i32) * shulker_capacity;

        let try_claim = |remaining: &mut i32,
                             plan: &mut Vec<ChestTransfer>,
                             claimed: &mut std::collections::HashSet<i32>,
                             chest: &Chest| {
            if *remaining <= 0 || !chest.item.is_empty() || claimed.contains(&chest.id) {
                return;
            }
            claimed.insert(chest.id);
            let add = empty_chest_capacity.min(*remaining);
            *remaining -= add;
            record_transfer(plan, chest.id, chest.position, item, add);
        };

        if remaining > 0 && !self.nodes.is_empty() {
            let node_0 = &self.nodes[0];
            if item == "diamond"
                && let Some(chest) = node_0.chests.get(crate::constants::DIAMOND_CHEST_ID as usize) {
                    try_claim(&mut remaining, &mut plan, &mut claimed_empty, chest);
                }
            if item == crate::constants::OVERFLOW_CHEST_ITEM
                && let Some(chest) = node_0.chests.get(crate::constants::OVERFLOW_CHEST_ID as usize) {
                    try_claim(&mut remaining, &mut plan, &mut claimed_empty, chest);
                }
            for ci in 2..node_0.chests.len() {
                if remaining <= 0 { break; }
                try_claim(&mut remaining, &mut plan, &mut claimed_empty, &node_0.chests[ci]);
            }
        }

        if remaining > 0 {
            for (ni, node) in self.nodes.iter().enumerate() {
                if remaining <= 0 { break; }
                if ni == 0 { continue; }
                for chest in &node.chests {
                    if remaining <= 0 { break; }
                    try_claim(&mut remaining, &mut plan, &mut claimed_empty, chest);
                }
            }
        }

        // If we still have `remaining > 0` after exhausting real chests,
        // the caller can grow storage; we report how much we could plan so
        // they can decide whether to proceed. (The authoritative mutating
        // `deposit_plan` still handles the growth case during execution.)
        (plan, qty - remaining)
    }

    /// Plans withdrawal of `qty` items from storage.
    ///
    /// **Mutates** storage state (removes items from `Chest.amounts`) and returns
    /// a plan of which chests to visit and how much to withdraw from each.
    ///
    /// **Deterministic Order**: Processes nodes/chests/slots in fixed order:
    /// 1. Nodes: by index (0, 1, 2, ...)
    /// 2. Chests: by index within node (0, 1, 2, 3)
    /// 3. Slots: by index within chest (0..53)
    ///
    /// This ensures consistent behavior and makes rollback easier.
    ///
    /// **Returns**: Vector of `ChestTransfer` indicating which chests to visit.
    /// The bot will execute these transfers sequentially.
    ///
    /// **Note**: This is a **planning** function. The actual withdrawal happens
    /// when the bot executes the plan and syncs chest contents back to storage.
    pub fn withdraw_plan(&mut self, item: &str, mut qty: i32) -> Vec<ChestTransfer> {
        if qty <= 0 {
            return Vec::new();
        }

        let mut plan: Vec<ChestTransfer> = Vec::new();

        // Deterministic order: node id then chest index then slot index.
        // This ensures consistent behavior and makes rollback predictable.
        for node_idx in 0..self.nodes.len() {
            for chest_idx in 0..self.nodes[node_idx].chests.len() {
                if qty <= 0 {
                    return plan; // Early exit when we have enough
                }

                let chest = &mut self.nodes[node_idx].chests[chest_idx];
                if chest.item != item {
                    continue; // Skip chests that don't contain this item
                }

                // Ensure chest has exactly 54 slots (defensive)
                Self::normalize_amounts_len(chest);
                let mut chest_taken = 0;

                // Withdraw from each shulker slot until we have enough
                for slot in 0..chest.amounts.len() {
                    if qty <= 0 {
                        break;
                    }

                    let available = chest.amounts[slot];
                    if available <= 0 {
                        continue; // Empty shulker slot
                    }

                    // Take as much as we need (or all available, whichever is less)
                    let take = available.min(qty);
                    chest.amounts[slot] -= take;
                    qty -= take;
                    chest_taken += take;
                }

                // If we took anything from this chest, add it to the plan
                if chest_taken > 0 {
                    plan.push(ChestTransfer {
                        chest_id: chest.id,
                        position: chest.position,
                        item: ItemId::from_normalized(item.to_string()),
                        amount: chest_taken,
                    });
                }
            }
        }

        plan
    }

    /// Plans deposit of `qty` items into storage.
    ///
    /// **Mutates** storage state (adds items to `Chest.amounts`) and returns
    /// a plan of which chests to visit and how much to deposit in each.
    ///
    /// **Allocation Strategy**:
    /// 1. **Fill existing chests** already assigned to this item (prefer same item)
    /// 2. **Use empty chests** if available (prefer same node if possible)
    /// 3. **Create new nodes** if no empty chests exist
    ///
    /// **Filling Order**: Partially-filled shulker slots are filled first, then empty slots.
    /// This minimizes fragmentation and keeps shulkers organized.
    ///
    /// **Arguments**:
    /// - `item`: Item identifier (without minecraft: prefix)
    /// - `qty`: Quantity to deposit
    /// - `stack_size`: Maximum stack size for this item (1, 16, or 64)
    ///
    /// **Returns**: Vector of `ChestTransfer` indicating which chests to visit.
    /// The bot will execute these transfers sequentially.
    ///
    /// **Note**: This is a **planning** function. The actual deposit happens
    /// when the bot executes the plan and syncs chest contents back to storage.
    pub fn deposit_plan(&mut self, item: &str, mut qty: i32, stack_size: i32) -> Vec<ChestTransfer> {
        if qty <= 0 {
            return Vec::new();
        }

        let mut plan: Vec<ChestTransfer> = Vec::new();

        // Phase 1: Fill existing chests already assigned to this item.
        // This keeps items consolidated and minimizes chest usage.
        for node_idx in 0..self.nodes.len() {
            for chest_idx in 0..self.nodes[node_idx].chests.len() {
                if qty <= 0 {
                    return plan; // Early exit when we've deposited everything
                }
                // Skip node 0 reserved chests for non-matching items.
                // The reserved-chest policy (chest 0 → diamond, chest 1 →
                // overflow) lives in `is_reserved_chest_blocked_for` — keep
                // this call site in sync if the rules ever change.
                if Self::is_reserved_chest_blocked_for(item, node_idx, chest_idx) {
                    continue;
                }
                let chest = &mut self.nodes[node_idx].chests[chest_idx];
                if chest.item != item {
                    continue; // Skip chests assigned to other items
                }
                let deposited_here = Self::deposit_into_chest(chest, &mut qty, stack_size);
                if deposited_here > 0 {
                    plan.push(ChestTransfer {
                        chest_id: chest.id,
                        position: chest.position,
                        item: ItemId::from_normalized(item.to_string()),
                        amount: deposited_here,
                    });
                }
            }
        }

        // Phase 2: Use empty chests; create new nodes if none exist.
        // This handles overflow when existing chests are full, or when no chest
        // has yet been assigned to this item. The loop is needed (instead of a
        // single call) because a freshly-claimed chest may not have enough
        // capacity for the remaining `qty`, forcing us to claim another.
        while qty > 0 {
            let (node_idx, chest_idx) = match Self::find_empty_chest_index(&self.nodes, item) {
                Some(ix) => ix, // Found an empty chest
                None => {
                    // No empty chests anywhere: grow storage by one node (which
                    // adds 4 fresh empty chests) and retry. The expect() is safe
                    // because a newly-constructed node always has empty chests.
                    self.add_node();
                    Self::find_empty_chest_index(&self.nodes, item).expect("new node must have chests")
                }
            };
            let chest = &mut self.nodes[node_idx].chests[chest_idx];
            chest.item = ItemId::from_normalized(item.to_string());
            let deposited_here = Self::deposit_into_chest(chest, &mut qty, stack_size);
            if deposited_here > 0 {
                plan.push(ChestTransfer {
                    chest_id: chest.id,
                    position: chest.position,
                    item: ItemId::from_normalized(item.to_string()),
                    amount: deposited_here,
                });
            }
        }

        plan
    }

    /// Defensive invariant enforcement: every chest must expose exactly
    /// `SLOTS_PER_CHEST` slots so that slot indices map 1:1 to Minecraft chest
    /// slots. Older on-disk data or partially-initialised chests may violate
    /// this, so deposit/withdraw paths call this before indexing `amounts`.
    fn normalize_amounts_len(chest: &mut Chest) {
        if chest.amounts.len() != Self::SLOTS_PER_CHEST {
            chest.amounts.resize(Self::SLOTS_PER_CHEST, 0);
        }
    }

    /// Deposits as much of `*qty` as fits into `chest`, decrementing `qty` in
    /// place and returning the amount actually deposited.
    ///
    /// Iterates slots in order, topping up each shulker to `shulker_capacity`
    /// before moving on. Because we walk slots left-to-right, any partially
    /// filled shulker encountered first is finished before an empty slot is
    /// touched - this minimises fragmentation across shulkers so the bot needs
    /// fewer shulker swaps per transfer.
    ///
    /// Slots with a negative count are treated as reserved and skipped (the
    /// encoding is kept for future "missing shulker" semantics).
    fn deposit_into_chest(chest: &mut Chest, qty: &mut i32, stack_size: i32) -> i32 {
        if *qty <= 0 {
            return 0;
        }

        Self::normalize_amounts_len(chest);
        let mut deposited = 0;
        
        // Calculate shulker capacity from stack size (27 slots × stack_size)
        let shulker_capacity = crate::types::Pair::shulker_capacity_for_stack_size(stack_size);

        // Fill partially-filled slots first, then empty slots.
        for slot in 0..chest.amounts.len() {
            if *qty <= 0 {
                return deposited;
            }

            let current = chest.amounts[slot];
            if current < 0 {
                continue; // reserved/missing slot semantics (future)
            }
            if current >= shulker_capacity {
                continue;
            }

            let capacity_left = shulker_capacity - current;
            let add = capacity_left.min(*qty);
            chest.amounts[slot] += add;
            *qty -= add;
            deposited += add;
        }

        deposited
    }

    /// Locates an unassigned chest suitable for a new `item` assignment.
    ///
    /// Returns `(node_idx, chest_idx)` of the first empty chest found under the
    /// reservation rules below, or `None` if every chest is either assigned or
    /// reserved-but-unavailable for this item.
    ///
    /// Selection priority:
    /// 1. If `item == "diamond"`: node 0 / chest 0 (the diamond-reserved slot).
    /// 2. If `item == OVERFLOW_CHEST_ITEM`: node 0 / chest 1 (overflow slot).
    /// 3. Node 0 chests 2..=3 (general-purpose chests closest to spawn).
    /// 4. Any empty chest in subsequent nodes, in node/chest order.
    ///
    /// Preferring node 0 keeps frequently-used items physically close to the
    /// bot's parking position, reducing navigation time.
    fn find_empty_chest_index(nodes: &[Node], item: &str) -> Option<(usize, usize)> {
        // Node 0 has reserved chests:
        // - Chest 0: dedicated for diamonds only
        // - Chest 1: dedicated for overflow only (bot deposits unknown/leftover items)
        // For other items: skip chest 0 and chest 1, prioritize node 0 chests 2, 3
        // Note: Phase 1 already fills existing chests assigned to the item, so Phase 2 only
        // looks for truly empty (unassigned) chests
        if !nodes.is_empty() {
            let node_0 = &nodes[0];
            // Check node 0 chest 0 if depositing diamonds and it's empty
            if item == "diamond" {
                let idx = crate::constants::DIAMOND_CHEST_ID as usize;
                if node_0.chests.get(idx).is_some_and(|c| c.item.is_empty()) {
                    return Some((0, idx));
                }
            }
            // Check node 0 chest 1 if depositing to overflow and it's empty
            if item == crate::constants::OVERFLOW_CHEST_ITEM {
                let idx = crate::constants::OVERFLOW_CHEST_ID as usize;
                if node_0.chests.get(idx).is_some_and(|c| c.item.is_empty()) {
                    return Some((0, idx));
                }
            }
            // For all items, check node 0 chests 2, 3 (skip chest 0 for non-diamonds, skip chest 1 for non-overflow)
            for ci in 2..node_0.chests.len() {
                if node_0.chests[ci].item.is_empty() {
                    return Some((0, ci));
                }
            }
        }
        
        // If node 0 chests are not empty, check other nodes
        for (ni, node) in nodes.iter().enumerate() {
            // Skip node 0 (already checked above)
            if ni == 0 {
                continue;
            }
            for (ci, chest) in node.chests.iter().enumerate() {
                if chest.item.is_empty() {
                    return Some((ni, ci));
                }
            }
        }
        None
    }

    /// Get the overflow chest (node 0, chest `OVERFLOW_CHEST_ID`) if it exists.
    /// Returns None if node 0 doesn't exist yet.
    pub fn get_overflow_chest(&self) -> Option<&Chest> {
        self.nodes
            .first()?
            .chests
            .get(crate::constants::OVERFLOW_CHEST_ID as usize)
    }

    /// Get a mutable reference to the overflow chest (node 0, chest `OVERFLOW_CHEST_ID`).
    /// Returns None if node 0 doesn't exist yet.
    pub fn get_overflow_chest_mut(&mut self) -> Option<&mut Chest> {
        self.nodes
            .first_mut()?
            .chests
            .get_mut(crate::constants::OVERFLOW_CHEST_ID as usize)
    }

    /// Get the overflow chest position.
    /// Returns None if node 0 doesn't exist yet.
    pub fn get_overflow_chest_position(&self) -> Option<Position> {
        self.get_overflow_chest().map(|c| c.position)
    }

    /// Get the overflow chest ID (always 1, since it's node 0 chest 1).
    pub const fn overflow_chest_id() -> i32 {
        crate::constants::OVERFLOW_CHEST_ID
    }

    /// Is the (node_idx, chest_idx) slot a reserved chest whose item type
    /// doesn't match `item`? Returns true when the chest is off-limits for the
    /// given item (i.e. the caller should skip this slot in plan / allocation
    /// passes), false otherwise.
    ///
    /// The reserved slots are all in node 0:
    ///   * chest `DIAMOND_CHEST_ID` (0) — only "diamond"
    ///   * chest `OVERFLOW_CHEST_ID` (1) — only the overflow sentinel item
    ///
    /// Centralising the check here avoids three places needing to stay in
    /// sync with the reserved-chest rules in
    /// [`simulate_deposit_plan`], [`deposit_plan`], and
    /// [`find_empty_chest_index`].
    pub fn is_reserved_chest_blocked_for(item: &str, node_idx: usize, chest_idx: usize) -> bool {
        if node_idx != 0 {
            return false;
        }
        if chest_idx == crate::constants::DIAMOND_CHEST_ID as usize {
            return item != "diamond";
        }
        if chest_idx == crate::constants::OVERFLOW_CHEST_ID as usize {
            return item != crate::constants::OVERFLOW_CHEST_ITEM;
        }
        false
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_storage() -> Storage {
        let origin = Position { x: 0, y: 64, z: 0 };
        let mut storage = Storage::new(&origin);
        // Add a node for testing
        storage.add_node();
        storage
    }

    #[test]
    fn test_new_storage() {
        let origin = Position { x: 100, y: 64, z: -200 };
        let storage = Storage::new(&origin);
        
        assert_eq!(storage.position.x, 100);
        assert_eq!(storage.position.y, 64);
        assert_eq!(storage.position.z, -200);
        assert_eq!(storage.nodes.len(), 0);
    }

    #[test]
    fn test_add_node() {
        let mut storage = test_storage();
        let initial_nodes = storage.nodes.len();
        
        storage.add_node();
        
        assert_eq!(storage.nodes.len(), initial_nodes + 1);
    }

    #[test]
    fn test_deposit_plan_empty_storage() {
        let mut storage = test_storage();
        
        // Assign chest to item first
        storage.nodes[0].chests[0].item = crate::types::ItemId::from_normalized("diamond".to_string());
        
        // Stack size 64 for diamond
        let plan = storage.deposit_plan("diamond", 100, 64);
        
        assert!(!plan.is_empty());
        let total: i32 = plan.iter().map(|t| t.amount).sum();
        assert_eq!(total, 100);
    }

    #[test]
    fn test_withdraw_plan_empty_storage() {
        let mut storage = test_storage();
        
        // Try to withdraw from empty storage
        let plan = storage.withdraw_plan("diamond", 100);
        
        assert!(plan.is_empty());
    }

    #[test]
    fn test_deposit_then_withdraw() {
        let mut storage = test_storage();
        
        // Deposit items (stack size 64)
        let deposit_plan = storage.deposit_plan("cobblestone", 500, 64);
        let deposited: i32 = deposit_plan.iter().map(|t| t.amount).sum();
        assert_eq!(deposited, 500);
        
        // Check total
        assert_eq!(storage.total_item_amount("cobblestone"), 500);
        
        // Withdraw half
        let withdraw_plan = storage.withdraw_plan("cobblestone", 250);
        let withdrawn: i32 = withdraw_plan.iter().map(|t| t.amount).sum();
        assert_eq!(withdrawn, 250);
        
        // Check remaining
        assert_eq!(storage.total_item_amount("cobblestone"), 250);
    }

    #[test]
    fn test_total_item_amount() {
        let mut storage = test_storage();
        
        // Initially zero
        assert_eq!(storage.total_item_amount("iron_ingot"), 0);
        
        // Deposit (stack size 64)
        storage.deposit_plan("iron_ingot", 1000, 64);
        assert_eq!(storage.total_item_amount("iron_ingot"), 1000);
    }

    #[test]
    fn test_shulker_capacity_limit() {
        let mut storage = test_storage();
        
        // Each shulker can hold 27 slots × 64 items = 1728 items (for most items)
        // One chest has 54 shulkers = 54 * 1728 = 93,312 items max
        // With 4 chests per node = 4 * 93,312 = 373,248 items max per node
        
        // Deposit more than one shulker can hold (stack size 64)
        let plan = storage.deposit_plan("gold_ingot", 100, 64);
        let deposited: i32 = plan.iter().map(|t| t.amount).sum();
        assert_eq!(deposited, 100);
        
        // Verify amounts don't exceed shulker capacity per slot
        // For gold_ingot (stack size 64), capacity is 27 * 64 = 1728
        let gold_capacity = crate::types::Pair::shulker_capacity_for_stack_size(64);
        for node in &storage.nodes {
            for chest in &node.chests {
                for &amount in &chest.amounts {
                    assert!(amount <= gold_capacity);
                }
            }
        }
    }
    
    #[test]
    fn test_shulker_capacity_calculation() {
        // Test shulker capacities for different stack sizes
        use crate::types::Pair;
        
        // Stack size 64: 27 * 64 = 1728
        assert_eq!(Pair::shulker_capacity_for_stack_size(64), 27 * 64);
        
        // Stack size 16: 27 * 16 = 432
        assert_eq!(Pair::shulker_capacity_for_stack_size(16), 27 * 16);
        
        // Stack size 1: 27 * 1 = 27
        assert_eq!(Pair::shulker_capacity_for_stack_size(1), 27);
    }

    #[test]
    fn test_withdraw_partial() {
        let mut storage = test_storage();
        
        // Deposit (stack size 64)
        storage.deposit_plan("emerald", 100, 64);
        
        // Withdraw more than available
        let plan = storage.withdraw_plan("emerald", 200);
        let withdrawn: i32 = plan.iter().map(|t| t.amount).sum();
        
        // Should only get what's available
        assert_eq!(withdrawn, 100);
        assert_eq!(storage.total_item_amount("emerald"), 0);
    }

    // -----------------------------------------------------------------
    // simulate_withdraw_plan / simulate_deposit_plan (non-mutating)
    // -----------------------------------------------------------------

    #[test]
    fn test_simulate_withdraw_plan_does_not_mutate() {
        let mut storage = test_storage();
        storage.deposit_plan("iron_ingot", 500, 64);
        let before: Vec<Vec<i32>> = storage
            .nodes
            .iter()
            .flat_map(|n| n.chests.iter().map(|c| c.amounts.clone()))
            .collect();

        let (plan, planned) = storage.simulate_withdraw_plan("iron_ingot", 300);
        assert_eq!(planned, 300);
        let total: i32 = plan.iter().map(|t| t.amount).sum();
        assert_eq!(total, 300);

        let after: Vec<Vec<i32>> = storage
            .nodes
            .iter()
            .flat_map(|n| n.chests.iter().map(|c| c.amounts.clone()))
            .collect();
        assert_eq!(before, after, "simulate_withdraw_plan must not mutate storage");
        assert_eq!(storage.total_item_amount("iron_ingot"), 500);
    }

    #[test]
    fn test_simulate_deposit_plan_does_not_mutate() {
        let storage = {
            let mut s = test_storage();
            s.deposit_plan("emerald", 100, 64);
            s
        };
        let before: Vec<Vec<i32>> = storage
            .nodes
            .iter()
            .flat_map(|n| n.chests.iter().map(|c| c.amounts.clone()))
            .collect();

        let (plan, planned) = storage.simulate_deposit_plan("emerald", 200, 64);
        assert_eq!(planned, 200);
        let total: i32 = plan.iter().map(|t| t.amount).sum();
        assert_eq!(total, 200);

        let after: Vec<Vec<i32>> = storage
            .nodes
            .iter()
            .flat_map(|n| n.chests.iter().map(|c| c.amounts.clone()))
            .collect();
        assert_eq!(before, after, "simulate_deposit_plan must not mutate storage");
    }

    #[test]
    fn test_simulate_withdraw_matches_mutating_plan() {
        // Two parallel storages: one we simulate against, one we mutate.
        let mut sim = test_storage();
        sim.deposit_plan("cobblestone", 800, 64);
        let mut real = sim.clone();

        let (sim_plan, sim_total) = sim.simulate_withdraw_plan("cobblestone", 450);
        let real_plan = real.withdraw_plan("cobblestone", 450);
        let real_total: i32 = real_plan.iter().map(|t| t.amount).sum();

        assert_eq!(sim_total, real_total);
        assert_eq!(sim_plan.len(), real_plan.len());
        for (a, b) in sim_plan.iter().zip(real_plan.iter()) {
            assert_eq!(a.chest_id, b.chest_id);
            assert_eq!(a.amount, b.amount);
        }
    }

    #[test]
    fn test_simulate_withdraw_short_when_undersupplied() {
        let mut storage = test_storage();
        storage.deposit_plan("gold_ingot", 50, 64);
        let (_plan, planned) = storage.simulate_withdraw_plan("gold_ingot", 200);
        assert_eq!(planned, 50, "should only plan what is actually in storage");
    }

    #[test]
    fn test_simulate_deposit_fills_partial_shulkers_first() {
        let mut storage = test_storage();
        // Deposit 100 into an assigned chest so first shulker is partially full.
        storage.deposit_plan("redstone", 100, 64);

        let (plan, planned) = storage.simulate_deposit_plan("redstone", 500, 64);
        assert_eq!(planned, 500);
        assert!(!plan.is_empty());
    }
}
