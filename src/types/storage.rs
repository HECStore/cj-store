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
//! See `src/bot/` for shulker automation implementation.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::types::chest::Chest;
use crate::types::ItemId;
use crate::types::node::Node;
use crate::types::position::Position;

/// Planned transfer against one chest, produced by `deposit_plan` /
/// `withdraw_plan` (and their simulate counterparts) and executed by the bot.
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
    /// Actual capacity varies by item type — use
    /// `Pair::shulker_capacity_for_stack_size()` (or `pair.shulker_capacity()`)
    /// at call sites that know the item's stack size. The 64-stack constant is
    /// kept here only as a coarse upper bound used by generic helpers.
    pub const DEFAULT_SHULKER_CAPACITY: i32 = (crate::constants::SHULKER_BOX_SLOTS as i32) * 64;

    /// Persists every node to `data/storage/{node_id}.json`.
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        for node in &self.nodes {
            node.save()?;
        }
        tracing::debug!(nodes = self.nodes.len(), "[Storage] saved all nodes");
        Ok(())
    }

    /// Constructs an empty storage anchored at `storage_position` (no nodes).
    pub fn new(storage_position: &Position) -> Self {
        Storage {
            position: *storage_position,
            nodes: Vec::new(),
        }
    }

    /// Loads every `data/storage/*.json` as a `Node`, skipping files whose
    /// stem is not a valid `i32` or which fail to deserialise.
    ///
    /// Creates `data/storage/` on first run and returns an empty storage.
    pub fn load(storage_position: &Position) -> Result<Self, Box<dyn std::error::Error>> {
        let storage_path = "data/storage";

        if !Path::new(storage_path).exists() {
            fs::create_dir_all(storage_path)?;
            tracing::info!(path = storage_path, "[Storage] created empty storage directory");
            return Ok(Storage {
                position: *storage_position,
                nodes: Vec::new(),
            });
        }

        let mut nodes = Vec::new();
        let mut skipped = 0usize;

        for entry in fs::read_dir(storage_path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file()
                && let Some(extension) = path.extension()
                && extension == "json"
                && let Some(file_name) = path.file_stem()
                && let Some(file_str) = file_name.to_str()
                && let Ok(node_id) = file_str.parse::<i32>()
            {
                match Node::load(node_id, storage_position) {
                    Ok(node) => nodes.push(node),
                    Err(e) => {
                        skipped += 1;
                        tracing::warn!(
                            node_id,
                            error = %e,
                            "[Storage] failed to load node; skipping",
                        );
                    }
                }
            }
        }

        tracing::info!(
            loaded = nodes.len(),
            skipped,
            "[Storage] loaded nodes from disk",
        );

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
        let mut node_id = 0i32;
        while self.nodes.iter().any(|n| n.id == node_id) {
            node_id += 1;
        }
        let node = Node::new(node_id, &self.position);
        tracing::info!(node_id, total_nodes = self.nodes.len() + 1, "[Storage] added node");
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


    /// Returns a mutable reference to the chest with the given `chest_id`,
    /// or `None` if no node contains such a chest.
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
    /// Read-only counterpart to `withdraw_plan` — walks the same deterministic
    /// node/chest/slot order and produces the same plan, letting callers
    /// preview an operation without cloning the whole `Storage`.
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
        // Mirrors `find_empty_chest_index`, but may claim multiple empty chests
        // when `remaining` exceeds one chest's `SLOTS_PER_CHEST * shulker_capacity`.
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

        // Past this point we can't grow storage without mutation; the caller
        // inspects `qty - remaining` and decides whether to invoke the real
        // `deposit_plan` (which allocates a new node) or abort.
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
    /// **Note**: This is a planning function. The actual withdrawal happens
    /// when the bot executes the plan and syncs chest contents back to storage.
    pub fn withdraw_plan(&mut self, item: &str, mut qty: i32) -> Vec<ChestTransfer> {
        if qty <= 0 {
            return Vec::new();
        }
        let requested = qty;

        let mut plan: Vec<ChestTransfer> = Vec::new();

        for node_idx in 0..self.nodes.len() {
            for chest_idx in 0..self.nodes[node_idx].chests.len() {
                if qty <= 0 {
                    tracing::debug!(
                        item,
                        requested,
                        transfers = plan.len(),
                        "[Storage] withdraw_plan satisfied",
                    );
                    return plan;
                }

                let chest = &mut self.nodes[node_idx].chests[chest_idx];
                if chest.item != item {
                    continue;
                }

                Self::normalize_amounts_len(chest);
                let mut chest_taken = 0;

                for slot in 0..chest.amounts.len() {
                    if qty <= 0 {
                        break;
                    }

                    let available = chest.amounts[slot];
                    if available <= 0 {
                        continue;
                    }

                    let take = available.min(qty);
                    chest.amounts[slot] -= take;
                    qty -= take;
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

        tracing::debug!(
            item,
            requested,
            planned = requested - qty,
            short = qty,
            transfers = plan.len(),
            "[Storage] withdraw_plan under-supplied",
        );
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
    /// **Note**: This is a planning function. The actual deposit happens
    /// when the bot executes the plan and syncs chest contents back to storage.
    pub fn deposit_plan(&mut self, item: &str, mut qty: i32, stack_size: i32) -> Vec<ChestTransfer> {
        if qty <= 0 {
            return Vec::new();
        }
        let requested = qty;

        let mut plan: Vec<ChestTransfer> = Vec::new();

        // Phase 1: top up chests already assigned to this item.
        for node_idx in 0..self.nodes.len() {
            for chest_idx in 0..self.nodes[node_idx].chests.len() {
                if qty <= 0 {
                    tracing::debug!(
                        item,
                        requested,
                        transfers = plan.len(),
                        "[Storage] deposit_plan satisfied (phase 1)",
                    );
                    return plan;
                }
                // Reserved-chest policy (chest 0 → diamond, chest 1 → overflow)
                // lives in `is_reserved_chest_blocked_for`.
                if Self::is_reserved_chest_blocked_for(item, node_idx, chest_idx) {
                    continue;
                }
                let chest = &mut self.nodes[node_idx].chests[chest_idx];
                if chest.item != item {
                    continue;
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

        // Phase 2: claim empty chests; grow storage by one node if none exist.
        // A freshly-claimed chest may not have enough capacity for the whole
        // remaining `qty`, so this is a loop rather than a single call.
        while qty > 0 {
            let (node_idx, chest_idx) = match Self::find_empty_chest_index(&self.nodes, item) {
                Some(ix) => ix,
                None => {
                    // expect() is safe because `Node::new` constructs 4 empty chests.
                    self.add_node();
                    Self::find_empty_chest_index(&self.nodes, item).expect("new node must have chests")
                }
            };
            let chest = &mut self.nodes[node_idx].chests[chest_idx];
            chest.item = ItemId::from_normalized(item.to_string());
            tracing::info!(
                item,
                node_idx,
                chest_id = chest.id,
                "[Storage] assigned empty chest to item",
            );
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

        tracing::debug!(
            item,
            requested,
            transfers = plan.len(),
            "[Storage] deposit_plan completed",
        );
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
    /// touched — this minimises fragmentation across shulkers so the bot needs
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
        let shulker_capacity = crate::types::Pair::shulker_capacity_for_stack_size(stack_size);

        for slot in 0..chest.amounts.len() {
            if *qty <= 0 {
                return deposited;
            }

            let current = chest.amounts[slot];
            if current < 0 {
                continue; // reserved/missing slot sentinel
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
        if !nodes.is_empty() {
            let node_0 = &nodes[0];
            if item == "diamond" {
                let idx = crate::constants::DIAMOND_CHEST_ID as usize;
                if node_0.chests.get(idx).is_some_and(|c| c.item.is_empty()) {
                    return Some((0, idx));
                }
            }
            if item == crate::constants::OVERFLOW_CHEST_ITEM {
                let idx = crate::constants::OVERFLOW_CHEST_ID as usize;
                if node_0.chests.get(idx).is_some_and(|c| c.item.is_empty()) {
                    return Some((0, idx));
                }
            }
            // General-purpose chests on node 0 start at index 2 (0 and 1 are reserved).
            for ci in 2..node_0.chests.len() {
                if node_0.chests[ci].item.is_empty() {
                    return Some((0, ci));
                }
            }
        }

        for (ni, node) in nodes.iter().enumerate() {
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

    /// Returns node 0 / chest `OVERFLOW_CHEST_ID`, or `None` if node 0 doesn't
    /// exist yet.
    pub fn get_overflow_chest(&self) -> Option<&Chest> {
        self.nodes
            .first()?
            .chests
            .get(crate::constants::OVERFLOW_CHEST_ID as usize)
    }

    /// Mutable counterpart of [`get_overflow_chest`].
    pub fn get_overflow_chest_mut(&mut self) -> Option<&mut Chest> {
        self.nodes
            .first_mut()?
            .chests
            .get_mut(crate::constants::OVERFLOW_CHEST_ID as usize)
    }

    /// World position of the overflow chest, or `None` if node 0 doesn't exist.
    pub fn get_overflow_chest_position(&self) -> Option<Position> {
        self.get_overflow_chest().map(|c| c.position)
    }

    /// Returns `OVERFLOW_CHEST_ID` (node 0 / chest 1). Exposed as a function so
    /// callers don't need to reach into `crate::constants` directly.
    pub const fn overflow_chest_id() -> i32 {
        crate::constants::OVERFLOW_CHEST_ID
    }

    /// Whether `(node_idx, chest_idx)` is a reserved node-0 chest whose item
    /// doesn't match `item`. `true` means callers should skip this chest.
    ///
    /// Reserved slots (both in node 0):
    ///   * chest `DIAMOND_CHEST_ID` (0) — only "diamond"
    ///   * chest `OVERFLOW_CHEST_ID` (1) — only `OVERFLOW_CHEST_ITEM`
    ///
    /// Centralised here so `simulate_deposit_plan`, `deposit_plan`, and
    /// `find_empty_chest_index` can share one source of truth.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_storage() -> Storage {
        let origin = Position { x: 0, y: 64, z: 0 };
        let mut storage = Storage::new(&origin);
        storage.add_node();
        storage
    }

    #[test]
    fn new_creates_empty_storage_at_given_origin() {
        let origin = Position { x: 100, y: 64, z: -200 };
        let storage = Storage::new(&origin);

        assert_eq!(storage.position.x, 100);
        assert_eq!(storage.position.y, 64);
        assert_eq!(storage.position.z, -200);
        assert_eq!(storage.nodes.len(), 0);
    }

    #[test]
    fn add_node_appends_with_next_sequential_id() {
        let origin = Position { x: 0, y: 64, z: 0 };
        let mut storage = Storage::new(&origin);

        storage.add_node();
        storage.add_node();
        storage.add_node();

        let ids: Vec<i32> = storage.nodes.iter().map(|n| n.id).collect();
        assert_eq!(ids, vec![0, 1, 2]);
    }

    #[test]
    fn add_node_reuses_lowest_gap_id() {
        let origin = Position { x: 0, y: 64, z: 0 };
        let mut storage = Storage::new(&origin);
        storage.add_node(); // 0
        storage.add_node(); // 1
        storage.add_node(); // 2
        // Simulate a removed middle node: drop node_id=1 from the vec.
        storage.nodes.retain(|n| n.id != 1);

        storage.add_node();

        let mut ids: Vec<i32> = storage.nodes.iter().map(|n| n.id).collect();
        ids.sort();
        assert_eq!(ids, vec![0, 1, 2], "gap at id=1 should be reused");
    }

    #[test]
    fn deposit_plan_fills_assigned_chest() {
        let mut storage = test_storage();
        storage.nodes[0].chests[0].item =
            crate::types::ItemId::from_normalized("diamond".to_string());

        let plan = storage.deposit_plan("diamond", 100, 64);

        assert!(!plan.is_empty());
        let total: i32 = plan.iter().map(|t| t.amount).sum();
        assert_eq!(total, 100);
        assert_eq!(storage.total_item_amount("diamond"), 100);
    }

    #[test]
    fn withdraw_plan_from_empty_storage_returns_empty_plan() {
        let mut storage = test_storage();

        let plan = storage.withdraw_plan("diamond", 100);

        assert!(plan.is_empty());
    }

    #[test]
    fn withdraw_plan_with_nonpositive_qty_returns_empty() {
        let mut storage = test_storage();
        storage.deposit_plan("iron_ingot", 100, 64);

        assert!(storage.withdraw_plan("iron_ingot", 0).is_empty());
        assert!(storage.withdraw_plan("iron_ingot", -5).is_empty());
        // No side effects.
        assert_eq!(storage.total_item_amount("iron_ingot"), 100);
    }

    #[test]
    fn deposit_plan_with_nonpositive_qty_returns_empty() {
        let mut storage = test_storage();

        assert!(storage.deposit_plan("iron_ingot", 0, 64).is_empty());
        assert!(storage.deposit_plan("iron_ingot", -5, 64).is_empty());
    }

    #[test]
    fn deposit_then_withdraw_preserves_running_total() {
        let mut storage = test_storage();

        let deposit_plan = storage.deposit_plan("cobblestone", 500, 64);
        let deposited: i32 = deposit_plan.iter().map(|t| t.amount).sum();
        assert_eq!(deposited, 500);
        assert_eq!(storage.total_item_amount("cobblestone"), 500);

        let withdraw_plan = storage.withdraw_plan("cobblestone", 250);
        let withdrawn: i32 = withdraw_plan.iter().map(|t| t.amount).sum();
        assert_eq!(withdrawn, 250);
        assert_eq!(storage.total_item_amount("cobblestone"), 250);
    }

    #[test]
    fn total_item_amount_is_zero_with_no_nodes() {
        let storage = Storage::new(&Position { x: 0, y: 64, z: 0 });
        assert_eq!(storage.total_item_amount("iron_ingot"), 0);
    }

    #[test]
    fn total_item_amount_reflects_deposits() {
        let mut storage = test_storage();

        assert_eq!(storage.total_item_amount("iron_ingot"), 0);
        storage.deposit_plan("iron_ingot", 1000, 64);
        assert_eq!(storage.total_item_amount("iron_ingot"), 1000);
    }

    #[test]
    fn total_item_amount_only_counts_matching_item() {
        let mut storage = test_storage();
        storage.deposit_plan("iron_ingot", 500, 64);
        storage.deposit_plan("gold_ingot", 300, 64);

        assert_eq!(storage.total_item_amount("iron_ingot"), 500);
        assert_eq!(storage.total_item_amount("gold_ingot"), 300);
        assert_eq!(storage.total_item_amount("diamond"), 0);
    }

    #[test]
    fn total_item_amount_sums_across_multiple_nodes() {
        let mut storage = test_storage();
        // Enough cobble to overflow the first node's 16 general chests (minus
        // reserved) and spill into a second node.
        storage.deposit_plan("cobblestone", 2_000_000, 64);
        assert!(storage.nodes.len() >= 2, "expected spillover into a second node");
        assert_eq!(storage.total_item_amount("cobblestone"), 2_000_000);
    }

    #[test]
    fn deposit_plan_respects_shulker_capacity_per_slot() {
        let mut storage = test_storage();

        let plan = storage.deposit_plan("gold_ingot", 100, 64);
        let deposited: i32 = plan.iter().map(|t| t.amount).sum();
        assert_eq!(deposited, 100);

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
    fn deposit_plan_grows_storage_when_all_chests_full() {
        let mut storage = test_storage();
        let initial_node_count = storage.nodes.len();

        // Request more than one node can hold: 4 chests × 54 shulkers × 1728 = 373_248.
        let qty = 4 * 54 * 1728 + 1000;
        let plan = storage.deposit_plan("cobblestone", qty, 64);
        let deposited: i32 = plan.iter().map(|t| t.amount).sum();

        assert_eq!(deposited, qty);
        assert!(
            storage.nodes.len() > initial_node_count,
            "a new node should have been allocated to absorb overflow",
        );
    }

    #[test]
    fn withdraw_plan_returns_only_what_is_available() {
        let mut storage = test_storage();
        storage.deposit_plan("emerald", 100, 64);

        let plan = storage.withdraw_plan("emerald", 200);
        let withdrawn: i32 = plan.iter().map(|t| t.amount).sum();

        assert_eq!(withdrawn, 100);
        assert_eq!(storage.total_item_amount("emerald"), 0);
    }

    #[test]
    fn withdraw_plan_visits_chests_in_deterministic_order() {
        let mut storage = test_storage();
        storage.deposit_plan("cobblestone", 5000, 64);

        let plan = storage.withdraw_plan("cobblestone", 5000);
        let ids: Vec<i32> = plan.iter().map(|t| t.chest_id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "withdraw plan must visit chests in ascending id order");
    }

    // -----------------------------------------------------------------
    // simulate_withdraw_plan / simulate_deposit_plan (non-mutating)
    // -----------------------------------------------------------------

    #[test]
    fn simulate_withdraw_plan_does_not_mutate_storage() {
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
    fn simulate_deposit_plan_does_not_mutate_storage() {
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
    fn simulate_withdraw_matches_mutating_plan_shape() {
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
    fn simulate_withdraw_reports_short_when_undersupplied() {
        let mut storage = test_storage();
        storage.deposit_plan("gold_ingot", 50, 64);
        let (_plan, planned) = storage.simulate_withdraw_plan("gold_ingot", 200);
        assert_eq!(planned, 50, "should only plan what is actually in storage");
    }

    #[test]
    fn simulate_withdraw_with_nonpositive_qty_returns_empty() {
        let storage = test_storage();
        for qty in [0, -5] {
            let (plan, planned) = storage.simulate_withdraw_plan("gold_ingot", qty);
            assert!(plan.is_empty());
            assert_eq!(planned, 0);
        }
    }

    #[test]
    fn simulate_deposit_with_nonpositive_qty_returns_empty() {
        let storage = test_storage();
        for qty in [0, -5] {
            let (plan, planned) = storage.simulate_deposit_plan("gold_ingot", qty, 64);
            assert!(plan.is_empty());
            assert_eq!(planned, 0);
        }
    }

    #[test]
    fn simulate_deposit_fills_partial_shulkers_first() {
        let mut storage = test_storage();
        storage.deposit_plan("redstone", 100, 64);

        let (plan, planned) = storage.simulate_deposit_plan("redstone", 500, 64);
        assert_eq!(planned, 500);
        assert!(!plan.is_empty());
    }

    #[test]
    fn simulate_deposit_on_empty_storage_plans_from_empty_chest() {
        let origin = Position { x: 0, y: 64, z: 0 };
        let mut storage = Storage::new(&origin);
        storage.add_node();

        let (plan, planned) = storage.simulate_deposit_plan("iron_ingot", 100, 64);
        assert_eq!(planned, 100);
        assert!(!plan.is_empty(), "empty node 0 should be claimed for the item");
    }

    // -----------------------------------------------------------------
    // Reserved-chest policy & empty-chest allocation
    // -----------------------------------------------------------------

    #[test]
    fn is_reserved_blocks_non_diamond_on_diamond_chest() {
        assert!(Storage::is_reserved_chest_blocked_for(
            "iron_ingot", 0, crate::constants::DIAMOND_CHEST_ID as usize,
        ));
        assert!(!Storage::is_reserved_chest_blocked_for(
            "diamond", 0, crate::constants::DIAMOND_CHEST_ID as usize,
        ));
    }

    #[test]
    fn is_reserved_blocks_non_overflow_on_overflow_chest() {
        assert!(Storage::is_reserved_chest_blocked_for(
            "iron_ingot", 0, crate::constants::OVERFLOW_CHEST_ID as usize,
        ));
        assert!(!Storage::is_reserved_chest_blocked_for(
            crate::constants::OVERFLOW_CHEST_ITEM,
            0,
            crate::constants::OVERFLOW_CHEST_ID as usize,
        ));
    }

    #[test]
    fn is_reserved_does_not_block_general_chests_on_node_zero() {
        assert!(!Storage::is_reserved_chest_blocked_for("iron_ingot", 0, 2));
        assert!(!Storage::is_reserved_chest_blocked_for("iron_ingot", 0, 3));
    }

    #[test]
    fn is_reserved_never_blocks_on_nodes_above_zero() {
        // Reserved rules apply only to node 0.
        assert!(!Storage::is_reserved_chest_blocked_for("iron_ingot", 1, 0));
        assert!(!Storage::is_reserved_chest_blocked_for("iron_ingot", 1, 1));
    }

    #[test]
    fn deposit_plan_skips_reserved_diamond_chest_for_non_diamond() {
        let mut storage = test_storage();
        storage.deposit_plan("iron_ingot", 100, 64);

        let diamond_chest = &storage.nodes[0].chests[crate::constants::DIAMOND_CHEST_ID as usize];
        assert!(
            diamond_chest.item.is_empty() || diamond_chest.item == "diamond",
            "iron_ingot must not be placed into the diamond-reserved chest",
        );
    }

    #[test]
    fn deposit_plan_skips_reserved_overflow_chest_for_non_overflow() {
        let mut storage = test_storage();
        storage.deposit_plan("iron_ingot", 100, 64);

        let overflow_chest =
            &storage.nodes[0].chests[crate::constants::OVERFLOW_CHEST_ID as usize];
        assert!(
            overflow_chest.item.is_empty()
                || overflow_chest.item == crate::constants::OVERFLOW_CHEST_ITEM,
            "iron_ingot must not be placed into the overflow-reserved chest",
        );
    }

    #[test]
    fn deposit_plan_prefers_diamond_chest_for_diamond() {
        let mut storage = test_storage();
        storage.deposit_plan("diamond", 100, 64);

        let diamond_chest = &storage.nodes[0].chests[crate::constants::DIAMOND_CHEST_ID as usize];
        assert_eq!(diamond_chest.item, "diamond");
    }

    #[test]
    fn deposit_plan_prefers_overflow_chest_for_overflow_item() {
        let mut storage = test_storage();
        storage.deposit_plan(crate::constants::OVERFLOW_CHEST_ITEM, 100, 64);

        let overflow_chest =
            &storage.nodes[0].chests[crate::constants::OVERFLOW_CHEST_ID as usize];
        assert_eq!(overflow_chest.item, crate::constants::OVERFLOW_CHEST_ITEM);
    }

    // -----------------------------------------------------------------
    // Overflow-chest accessors
    // -----------------------------------------------------------------

    #[test]
    fn overflow_chest_id_matches_constant() {
        assert_eq!(Storage::overflow_chest_id(), crate::constants::OVERFLOW_CHEST_ID);
    }

    #[test]
    fn overflow_chest_accessors_are_none_without_node_zero() {
        let storage = Storage::new(&Position { x: 0, y: 64, z: 0 });
        assert!(storage.get_overflow_chest().is_none());
        assert!(storage.get_overflow_chest_position().is_none());
    }

    #[test]
    fn overflow_chest_accessors_return_node_zero_chest_one() {
        let storage = test_storage();
        let chest = storage.get_overflow_chest().expect("node 0 exists");
        assert_eq!(chest.id, crate::constants::OVERFLOW_CHEST_ID);
        assert_eq!(storage.get_overflow_chest_position(), Some(chest.position));
    }

    #[test]
    fn overflow_chest_mut_allows_edit() {
        let mut storage = test_storage();
        {
            let chest = storage.get_overflow_chest_mut().expect("node 0 exists");
            chest.amounts[0] = 123;
        }
        let chest = storage.get_overflow_chest().unwrap();
        assert_eq!(chest.amounts[0], 123);
    }

    // -----------------------------------------------------------------
    // Misc helpers
    // -----------------------------------------------------------------

    #[test]
    fn get_chest_mut_finds_existing_chest() {
        let mut storage = test_storage();
        let expected_id = storage.nodes[0].chests[0].id;

        let chest = storage.get_chest_mut(expected_id).expect("chest exists");
        assert_eq!(chest.id, expected_id);
    }

    #[test]
    fn get_chest_mut_returns_none_for_unknown_id() {
        let mut storage = test_storage();
        assert!(storage.get_chest_mut(9_999_999).is_none());
    }

    #[test]
    fn slots_per_chest_matches_double_chest_constant() {
        assert_eq!(Storage::SLOTS_PER_CHEST, crate::constants::DOUBLE_CHEST_SLOTS);
    }

    #[test]
    fn default_shulker_capacity_is_27_times_64() {
        assert_eq!(Storage::DEFAULT_SHULKER_CAPACITY, 27 * 64);
    }

    #[test]
    fn normalize_amounts_len_expands_short_vec() {
        let mut storage = test_storage();
        storage.nodes[0].chests[0].amounts = vec![1, 2, 3];

        Storage::normalize_amounts_len(&mut storage.nodes[0].chests[0]);

        assert_eq!(
            storage.nodes[0].chests[0].amounts.len(),
            Storage::SLOTS_PER_CHEST,
        );
        assert_eq!(&storage.nodes[0].chests[0].amounts[..3], &[1, 2, 3]);
    }

    #[test]
    fn normalize_amounts_len_truncates_long_vec() {
        let mut storage = test_storage();
        storage.nodes[0].chests[0].amounts = vec![7; Storage::SLOTS_PER_CHEST + 10];

        Storage::normalize_amounts_len(&mut storage.nodes[0].chests[0]);

        assert_eq!(
            storage.nodes[0].chests[0].amounts.len(),
            Storage::SLOTS_PER_CHEST,
        );
    }

    #[test]
    fn deposit_into_chest_skips_reserved_negative_slots() {
        let mut storage = test_storage();
        let chest = &mut storage.nodes[0].chests[2];
        chest.amounts = vec![-1; Storage::SLOTS_PER_CHEST];
        chest.amounts[0] = 0; // one usable slot

        let mut qty = 100;
        let deposited = Storage::deposit_into_chest(chest, &mut qty, 64);

        assert_eq!(deposited, 100);
        assert_eq!(chest.amounts[0], 100);
        for &v in &chest.amounts[1..] {
            assert_eq!(v, -1, "reserved slots must remain untouched");
        }
    }
}
