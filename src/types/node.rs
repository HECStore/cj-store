//! # Node Management
//!
//! A node is a cluster of 4 chests arranged in a 2×2 pattern with a bot
//! access position. Nodes are laid out on a spiral around a single storage
//! origin (node 0) and persisted to `data/storage/{node_id}.json`.
//!
//! Footprint (top-down, P = bot position, C = chest, S = shulker station):
//! ```
//! NNNN
//! NCCN
//! NCCN
//! NSNP
//! ```
//!
//! Spiral order around the origin:
//! ```
//! . 6 7 8 9
//! . 5 0 1 .
//! . 4 3 2 .
//! ```
//! See [`Node::calc_position`] for the ring algorithm.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::constants::{CHESTS_PER_NODE, NODE_SPACING};
use crate::error::StoreError;
use crate::fsutil::write_atomic;
use crate::types::ItemId;
use crate::types::chest::Chest;
use crate::types::position::Position;

/// On-disk directory for per-node files. Single source of truth shared by
/// `Node::load`, `Node::save`, `Storage::load`, and the CLI removeNode path.
pub(crate) const STORAGE_DIR: &str = "data/storage";

/// On-disk path for node `id`. Mirrors the convention encoded by
/// [`STORAGE_DIR`] so callers don't hand-roll the format string.
pub(crate) fn node_file_path(id: i32) -> std::path::PathBuf {
    Path::new(STORAGE_DIR).join(format!("{id}.json"))
}

/// A storage node: 4 chests plus a bot access position, placed on the
/// storage spiral by [`Node::calc_position`].
///
/// `chests` always has exactly [`CHESTS_PER_NODE`] entries (indices 0..=3).
/// Node 0 reserves chest 0 for diamonds and chest 1 for overflow; these
/// assignments are re-enforced on every [`Node::load`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// Node ID, also the filename stem in `data/storage/{id}.json`.
    pub id: i32,
    /// World position the bot stands at to access this node.
    pub position: Position,
    /// Exactly [`CHESTS_PER_NODE`] chests, indices 0..=3.
    pub chests: Vec<Chest>,
}

impl Node {
    /// Creates a new node with 4 freshly-positioned chests.
    ///
    /// `storage_position` is the storage *origin* (node 0's world position),
    /// not this node's position — the node's own position is derived from
    /// the origin via [`Self::calc_position`].
    ///
    /// For node 0, chest 0 is force-assigned to `diamond` and chest 1 to
    /// the overflow item; see the module docs.
    pub fn new(node_id: i32, storage_position: &Position) -> Node {
        let node_position = Self::calc_position(node_id, storage_position);

        let mut chests = Vec::with_capacity(CHESTS_PER_NODE);

        for index in 0..CHESTS_PER_NODE as i32 {
            let mut chest = Chest::new(node_id, &node_position, index);

            // Node 0 reserves chests 0 and 1 for diamond and overflow.
            // These assignments are invariants, not defaults — see Self::load.
            if node_id == 0 {
                if index == 0 {
                    chest.item =
                        ItemId::from_normalized(crate::constants::BASE_CURRENCY_ITEM.to_string());
                } else if index == 1 {
                    chest.item =
                        ItemId::from_normalized(crate::constants::OVERFLOW_CHEST_ITEM.to_string());
                }
            }

            chests.push(chest);
        }

        Node {
            id: node_id,
            position: node_position,
            chests,
        }
    }

    /// Loads a node from `data/storage/{id}.json` and reconciles it with the
    /// current storage origin.
    ///
    /// Positions in the file are derivable state: the node position and all
    /// chest positions are recomputed from `storage_position` on every load,
    /// so moving the storage origin in config relocates existing nodes
    /// without a data migration. Only chest `item` assignments are
    /// authoritative on disk.
    ///
    /// For node 0, the reserved chest invariants (chest 0 = diamond,
    /// chest 1 = overflow) are re-enforced even on load in case the file was
    /// edited manually, and any correction is persisted back to disk.
    pub fn load(id: i32, storage_position: &Position) -> Result<Self, StoreError> {
        Self::load_from_dir(id, storage_position, Path::new(STORAGE_DIR))
    }

    /// Same as [`Self::load`], but reads from `base/{id}.json` instead of the
    /// hard-coded [`STORAGE_DIR`]. Exists so unit tests can exercise the JSON
    /// invariant checks against a temp dir without polluting the real data dir.
    fn load_from_dir(
        id: i32,
        storage_position: &Position,
        base: &Path,
    ) -> Result<Self, StoreError> {
        let file_path = base.join(format!("{id}.json"));

        if !file_path.exists() {
            return Err(StoreError::InvariantViolation(format!(
                "Node file not found: {}",
                file_path.display()
            )));
        }

        let json_data = fs::read_to_string(&file_path)?;
        let mut node: Node = serde_json::from_str(&json_data).map_err(|e| {
            StoreError::InvariantViolation(format!("Failed to parse node {}: {}", id, e))
        })?;

        if node.id != id {
            return Err(StoreError::InvariantViolation(format!(
                "Node ID mismatch: expected {}, got {}",
                id, node.id
            )));
        }

        // Reject wrong chest counts up-front so the per-chest scan below isn't
        // wasting cycles on a node that was always going to be rejected, and
        // operators get a clearer error than whichever per-chest check would
        // happen to fail first on a 5-chest hand-edited file.
        if node.chests.len() != CHESTS_PER_NODE {
            return Err(StoreError::InvariantViolation(format!(
                "Node {} has {} chests, expected {}",
                id,
                node.chests.len(),
                CHESTS_PER_NODE
            )));
        }

        // Recompute positions from the current storage origin (see doc comment).
        // Validate `chest.index` BEFORE computing positions: `Chest::calc_position`
        // panics on out-of-range indices, and panics propagate past the
        // skip-on-Err handler in `Storage::load`, taking down the bot at
        // startup. A hand-edited or corrupted file with `index: 7` is just a
        // bad node — we want to skip it, not crash.
        for chest in &node.chests {
            if !(0..CHESTS_PER_NODE as i32).contains(&chest.index) {
                return Err(StoreError::InvariantViolation(format!(
                    "Node {} has chest with invalid index {} (must be 0..{})",
                    id, chest.index, CHESTS_PER_NODE
                )));
            }
            // The Chest doc-comment claims amounts.len() == DOUBLE_CHEST_SLOTS
            // is an invariant. Enforce it here so storage.rs's raw iterations
            // (total_item_amount, simulate_*_plan) cannot silently feed wrong
            // totals into pricing on a hand-edited or partially-migrated file.
            if chest.amounts.len() != crate::constants::DOUBLE_CHEST_SLOTS {
                return Err(StoreError::InvariantViolation(format!(
                    "Node {} chest {} has amounts.len()={}, expected {}",
                    id,
                    chest.index,
                    chest.amounts.len(),
                    crate::constants::DOUBLE_CHEST_SLOTS
                )));
            }
        }

        node.position = Self::calc_position(id, storage_position);
        for chest in &mut node.chests {
            chest.position = Chest::calc_position(&node.position, chest.index);
        }

        node.chests.sort_by_key(|chest| chest.index);

        // After sort, indices must be exactly [0, 1, 2, 3] with no duplicates,
        // and the redundant id fields (`chest.id`, `chest.node_id`, `chest.index`)
        // must all agree — otherwise `Storage::get_chest_mut` and
        // `apply_chest_sync` (which look up by id) silently return the wrong
        // chest, and a chest with id=0 sitting in a non-zero node would be
        // force-relabeled to "diamond" by the bot's first sync.
        for (expected_idx, chest) in node.chests.iter().enumerate() {
            chest.check_invariants(id, expected_idx as i32)?;
        }

        // Re-enforce node 0's reserved chest invariants in case the JSON was
        // hand-edited. Persist the correction so later loads see the fix.
        //
        // Refuse the relabel if the existing chest holds a non-empty stockpile
        // of the wrong item: silently relabeling would mint currency. The
        // operator must reconcile manually. -1 is the wire-protocol unchecked
        // sentinel (ChestSyncReport) and counts as "no stockpile".
        if id == 0 {
            let mut needs_save = false;

            if let Some(chest_0) = node.chests.get_mut(0)
                && chest_0.item != crate::constants::BASE_CURRENCY_ITEM
            {
                let stockpile: i32 = chest_0.amounts.iter().filter(|&&a| a > 0).sum();
                if stockpile > 0 {
                    return Err(StoreError::InvariantViolation(format!(
                        "Node 0 chest 0 reserved for `{}` but on-disk item is `{}` with stockpile of {} units; refusing to relabel (would mint currency). Reconcile manually.",
                        crate::constants::BASE_CURRENCY_ITEM,
                        chest_0.item,
                        stockpile
                    )));
                }
                chest_0.item =
                    ItemId::from_normalized(crate::constants::BASE_CURRENCY_ITEM.to_string());
                needs_save = true;
            }

            if let Some(chest_1) = node.chests.get_mut(1)
                && chest_1.item != crate::constants::OVERFLOW_CHEST_ITEM
            {
                let stockpile: i32 = chest_1.amounts.iter().filter(|&&a| a > 0).sum();
                if stockpile > 0 {
                    return Err(StoreError::InvariantViolation(format!(
                        "Node 0 chest 1 reserved for `{}` but on-disk item is `{}` with stockpile of {} units; refusing to relabel. Reconcile manually.",
                        crate::constants::OVERFLOW_CHEST_ITEM,
                        chest_1.item,
                        stockpile
                    )));
                }
                chest_1.item =
                    ItemId::from_normalized(crate::constants::OVERFLOW_CHEST_ITEM.to_string());
                needs_save = true;
            }

            if needs_save {
                tracing::warn!(
                    node_id = 0,
                    "node 0 reserved chest assignments were wrong on disk; rewriting"
                );
                // Save back to the SAME base we loaded from — otherwise a
                // test loading from a temp dir would silently rewrite the
                // real `data/storage` dir on a node-0 fixup.
                if let Err(e) = node.save_to_dir(base) {
                    tracing::error!(
                        node_id = 0,
                        error = %e,
                        "failed to persist node 0 reserved chest correction"
                    );
                }
            }
        }

        tracing::debug!(node_id = id, chests = node.chests.len(), "loaded node");
        Ok(node)
    }

    /// Serializes this node to `data/storage/{id}.json`.
    ///
    /// Uses [`write_atomic`] (write-to-temp + rename) so a crash mid-write
    /// cannot leave a partially-written node file on disk.
    pub fn save(&self) -> Result<(), StoreError> {
        self.save_to_dir(Path::new(STORAGE_DIR))
    }

    /// Same as [`Self::save`], but writes to `base/{id}.json` instead of the
    /// hard-coded [`STORAGE_DIR`]. Exists so unit tests can round-trip a node
    /// through a temp dir without touching the real data dir.
    fn save_to_dir(&self, base: &Path) -> Result<(), StoreError> {
        // Validate in-memory invariants before persisting so a mutation bug
        // (accidental Vec::push of a 5th chest, swapped indices, etc.) fails
        // at the save boundary rather than corrupting disk and surfacing only
        // on the next load — far from the offending mutation site.
        if self.chests.len() != CHESTS_PER_NODE {
            return Err(StoreError::InvariantViolation(format!(
                "Node {} has {} chests, expected {}",
                self.id,
                self.chests.len(),
                CHESTS_PER_NODE
            )));
        }
        for (expected_idx, chest) in self.chests.iter().enumerate() {
            chest.check_invariants(self.id, expected_idx as i32)?;
        }

        let file_path = base.join(format!("{}.json", self.id));

        if let Some(parent_dir) = file_path.parent()
            && !parent_dir.is_dir()
        {
            // Use is_dir, not exists: if `data/storage` already exists as a
            // regular file (operator mistake), `create_dir_all` returns a
            // clear "Not a directory" error; `exists()` would short-circuit
            // and let the downstream tempfile rename fail with a confusing
            // path-error far from the cause.
            fs::create_dir_all(parent_dir)?;
        }

        let json_data = serde_json::to_string_pretty(self).map_err(|e| {
            StoreError::InvariantViolation(format!("Failed to serialize node {}: {}", self.id, e))
        })?;
        write_atomic(&file_path, &json_data)?;

        tracing::debug!(node_id = self.id, bytes = json_data.len(), "saved node");
        Ok(())
    }

    /// Computes the world position of node `id` relative to `storage_position`
    /// (the position of node 0) using the storage spiral.
    ///
    /// Node 0 sits at `storage_position`. Every other node lives on ring
    /// `n ≥ 1`, a square shell of `8*n` nodes surrounding ring `n-1`, with
    /// IDs in `(n-1)*n*4 + 1 ..= n*(n+1)*4`. Within a ring we walk clockwise:
    /// Right edge (top→bottom), Bottom edge (right→left), Left edge
    /// (bottom→top), Top edge (left→right). Adjacent nodes are spaced
    /// [`NODE_SPACING`] blocks apart.
    ///
    /// Pattern (ids shown on the grid):
    /// ```
    /// . 6 7 8 9
    /// . 5 0 1 .
    /// . 4 3 2 .
    /// ```
    pub fn calc_position(id: i32, storage_position: &Position) -> Position {
        // Negative IDs would silently produce garbage coordinates and can
        // collide with a real node (e.g. id=-5 → (0,-1) which is node 7).
        // Filenames are validated at the `Storage::load` boundary, so this
        // should be unreachable; promote to a release-effective `assert!`
        // because silent garbage in release is worse than no assert at all.
        assert!(id >= 0, "Node::calc_position requires id >= 0, got {id}",);

        if id == 0 {
            return Position {
                x: storage_position.x,
                y: storage_position.y,
                z: storage_position.z,
            };
        }

        // Walk outward from ring 1 until the ring's last id covers `id`.
        // O(sqrt(id)); a closed-form via sqrt would work but this avoids
        // any floating-point rounding concerns.
        let mut ring = 1;
        while ring * (ring + 1) * 4 < id {
            ring += 1;
        }

        // 1-indexed offset within the ring: the ring's first id maps to 1
        // (so id=1 → 1 for ring 1, id=9 → 1 for ring 2), the last id maps
        // to 8*ring. The side-selection below compensates so every ring
        // walks its 8*ring nodes in a single clockwise pass.
        let pos_in_ring = id - (ring - 1) * ring * 4;
        // Each of the 4 sides of a ring holds `2*ring` nodes.
        let side = 2 * ring;

        // Note: -z is "up" / north in Minecraft, so "top side" uses dz=-ring.
        // Section 4 happens exactly once per ring at pos_in_ring == 8*ring
        // (the last id of every ring); the top-edge formula evaluates to
        // (ring, -ring), the top-right corner that closes the ring. This
        // is a consistent consequence of the parameterisation, not a
        // coincidence — a future refactor of the top edge must preserve
        // the corner case explicitly.
        let (dx, dz) = match pos_in_ring / side {
            0 => (ring, -ring + pos_in_ring),         // Right edge, walking +z
            1 => (ring - (pos_in_ring - side), ring), // Bottom edge, walking -x
            2 => (-ring, ring - (pos_in_ring - 2 * side)), // Left edge, walking -z
            3 | 4 => (-ring + (pos_in_ring - 3 * side), -ring), // Top edge + ring corner
            other => unreachable!("pos_in_ring/side must be 0..=4 by construction, got {other}"),
        };

        // NODE_SPACING leaves room for the 2-wide chest footprint plus a
        // walking lane, so adjacent nodes' 2×2 chest clusters don't overlap.
        Position {
            x: storage_position.x + dx * NODE_SPACING,
            y: storage_position.y,
            z: storage_position.z + dz * NODE_SPACING,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin() -> Position {
        Position { x: 0, y: 64, z: 0 }
    }

    #[test]
    fn calc_position_places_node_0_at_storage_origin() {
        let pos = Node::calc_position(0, &origin());
        assert_eq!((pos.x, pos.y, pos.z), (0, 64, 0));
    }

    #[test]
    fn calc_position_places_node_0_at_offset_origin() {
        let off = Position {
            x: 1000,
            y: 100,
            z: -500,
        };
        let pos = Node::calc_position(0, &off);
        assert_eq!((pos.x, pos.y, pos.z), (1000, 100, -500));
    }

    #[test]
    fn calc_position_keeps_y_equal_to_storage_origin() {
        // Every node in the spiral shares the origin's y — the spiral is 2D.
        for id in 0..50 {
            let pos = Node::calc_position(id, &origin());
            assert_eq!(pos.y, origin().y, "node {id} drifted in y");
        }
    }

    #[test]
    fn calc_position_ring_1_starts_on_right_edge_at_node_1() {
        // Node 1 is the first node of ring 1, on the right edge (dx = +ring).
        let pos = Node::calc_position(1, &origin());
        assert_eq!(pos.x, NODE_SPACING);
        assert_eq!(pos.z, 0);
    }

    #[test]
    fn calc_position_ring_1_fits_within_one_spacing_of_origin() {
        // Every ring-1 node sits within Chebyshev distance `ring * spacing`.
        for id in 1..=8 {
            let pos = Node::calc_position(id, &origin());
            let dx = (pos.x - origin().x).abs();
            let dz = (pos.z - origin().z).abs();
            assert!(dx <= NODE_SPACING, "node {id} dx={dx} exceeds spacing");
            assert!(dz <= NODE_SPACING, "node {id} dz={dz} exceeds spacing");
        }
    }

    #[test]
    fn calc_position_ring_2_fits_within_two_spacings_of_origin() {
        for id in 9..=24 {
            let pos = Node::calc_position(id, &origin());
            let dx = (pos.x - origin().x).abs();
            let dz = (pos.z - origin().z).abs();
            assert!(
                dx <= 2 * NODE_SPACING,
                "node {id} dx={dx} exceeds 2*spacing"
            );
            assert!(
                dz <= 2 * NODE_SPACING,
                "node {id} dz={dz} exceeds 2*spacing"
            );
        }
    }

    #[test]
    fn calc_position_is_deterministic_for_same_id() {
        let a = Node::calc_position(5, &origin());
        let b = Node::calc_position(5, &origin());
        assert_eq!((a.x, a.y, a.z), (b.x, b.y, b.z));
    }

    #[test]
    fn calc_position_assigns_unique_xz_to_every_node() {
        let mut seen = std::collections::HashSet::new();
        for id in 0..100 {
            let pos = Node::calc_position(id, &origin());
            assert!(
                seen.insert((pos.x, pos.z)),
                "node {id} collides with a previously placed node at ({}, {})",
                pos.x,
                pos.z
            );
        }
    }

    #[test]
    fn calc_position_translates_with_storage_origin() {
        // Moving the origin must translate every node by the same vector.
        let a = origin();
        let b = Position {
            x: 1000,
            y: 64,
            z: -500,
        };
        for id in 0..25 {
            let pa = Node::calc_position(id, &a);
            let pb = Node::calc_position(id, &b);
            assert_eq!(pb.x - pa.x, b.x - a.x, "node {id} x mistranslated");
            assert_eq!(pb.z - pa.z, b.z - a.z, "node {id} z mistranslated");
        }
    }

    #[test]
    fn new_node_0_has_reserved_diamond_and_overflow_chests() {
        let node = Node::new(
            0,
            &Position {
                x: 50,
                y: 70,
                z: 100,
            },
        );
        assert_eq!(node.id, 0);
        assert_eq!(node.chests.len(), CHESTS_PER_NODE);
        assert_eq!(node.chests[0].item, "diamond");
        assert_eq!(node.chests[1].item, crate::constants::OVERFLOW_CHEST_ITEM);
        // Remaining chests start unassigned.
        assert_eq!(node.chests[2].item, "");
        assert_eq!(node.chests[3].item, "");
    }

    #[test]
    fn new_non_zero_node_leaves_all_chests_unassigned() {
        // The reserved-chest invariant applies to node 0 only.
        let node = Node::new(5, &origin());
        assert_eq!(node.id, 5);
        assert_eq!(node.chests.len(), CHESTS_PER_NODE);
        for (i, c) in node.chests.iter().enumerate() {
            assert_eq!(
                c.item, "",
                "chest {i} on non-zero node should be unassigned"
            );
        }
    }

    #[test]
    fn new_places_node_on_spiral_and_chests_relative_to_node() {
        // Node's own position must equal calc_position, and every chest
        // must sit at the documented offset from that position.
        let node = Node::new(7, &origin());
        let expected = Node::calc_position(7, &origin());
        assert_eq!(
            (node.position.x, node.position.y, node.position.z),
            (expected.x, expected.y, expected.z)
        );
        for c in &node.chests {
            let expected_chest = Chest::calc_position(&node.position, c.index);
            assert_eq!(
                (c.position.x, c.position.y, c.position.z),
                (expected_chest.x, expected_chest.y, expected_chest.z),
                "chest {} misplaced",
                c.index
            );
        }
    }

    // -----------------------------------------------------------------
    // save_to_dir / load_from_dir round-trip + on-disk invariant tests.
    //
    // These pin every JSON-shaped invariant in `Node::load`: chest count,
    // duplicate / out-of-range indices, redundant-id agreement,
    // amounts.len, and the node-0 reserved-chest fixup including the
    // "refuse to relabel a non-empty reserved chest" guard. Without
    // these, a regression that loosened any check would slip past CI.
    // We use `tempfile::tempdir()` so the real `data/storage` is
    // never touched.
    // -----------------------------------------------------------------

    fn write_node_json(dir: &Path, id: i32, json: &str) {
        fs::write(dir.join(format!("{id}.json")), json).unwrap();
    }

    /// Build a minimal valid node-0 JSON with `chest_0_item` and
    /// `chest_0_amounts` controllable for the reserved-chest tests.
    fn node_0_json(chest_0_item: &str, chest_0_amounts: Vec<i32>) -> String {
        // amounts arrays for the other chests are always 54 zeros.
        let zeros: Vec<i32> = vec![0; crate::constants::DOUBLE_CHEST_SLOTS];
        let zeros_json = serde_json::to_string(&zeros).unwrap();
        let chest_0_amounts_json = serde_json::to_string(&chest_0_amounts).unwrap();
        format!(
            r#"{{
              "id": 0,
              "position": {{"x": 0, "y": 64, "z": 0}},
              "chests": [
                {{"id": 0, "node_id": 0, "index": 0, "position": {{"x":0,"y":0,"z":0}}, "item": "{chest_0_item}", "amounts": {chest_0_amounts_json}}},
                {{"id": 1, "node_id": 0, "index": 1, "position": {{"x":0,"y":0,"z":0}}, "item": "overflow", "amounts": {zeros_json}}},
                {{"id": 2, "node_id": 0, "index": 2, "position": {{"x":0,"y":0,"z":0}}, "item": "", "amounts": {zeros_json}}},
                {{"id": 3, "node_id": 0, "index": 3, "position": {{"x":0,"y":0,"z":0}}, "item": "", "amounts": {zeros_json}}}
              ]
            }}"#
        )
    }

    #[test]
    fn save_load_round_trip_preserves_node() {
        let dir = tempfile::tempdir().unwrap();
        let mut node = Node::new(5, &origin());
        // Plant some recognisable state we can assert survives the
        // round-trip so the test doesn't accidentally pass on a default-
        // valued node where every field is zero/empty.
        node.chests[2].item = ItemId::from_normalized("cobblestone".to_string());
        node.chests[2].amounts[0] = 17;
        node.chests[2].amounts[53] = 99;
        node.chests[3].item = ItemId::from_normalized("iron_ingot".to_string());
        node.chests[3].amounts[10] = 42;

        node.save_to_dir(dir.path()).unwrap();

        let loaded = Node::load_from_dir(5, &origin(), dir.path()).unwrap();
        assert_eq!(loaded.id, node.id);
        assert_eq!(loaded.position, node.position);
        assert_eq!(loaded.chests.len(), node.chests.len());
        for (a, b) in loaded.chests.iter().zip(node.chests.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.node_id, b.node_id);
            assert_eq!(a.index, b.index);
            assert_eq!(a.position, b.position);
            assert_eq!(a.item, b.item);
            assert_eq!(a.amounts, b.amounts);
        }
    }

    #[test]
    fn load_rejects_chest_count_other_than_4() {
        let dir = tempfile::tempdir().unwrap();
        let zeros: Vec<i32> = vec![0; crate::constants::DOUBLE_CHEST_SLOTS];
        let zeros_json = serde_json::to_string(&zeros).unwrap();
        // Three chests instead of four.
        let json = format!(
            r#"{{
              "id": 1,
              "position": {{"x":0,"y":64,"z":0}},
              "chests": [
                {{"id": 4, "node_id": 1, "index": 0, "position": {{"x":0,"y":0,"z":0}}, "item": "", "amounts": {zeros_json}}},
                {{"id": 5, "node_id": 1, "index": 1, "position": {{"x":0,"y":0,"z":0}}, "item": "", "amounts": {zeros_json}}},
                {{"id": 6, "node_id": 1, "index": 2, "position": {{"x":0,"y":0,"z":0}}, "item": "", "amounts": {zeros_json}}}
              ]
            }}"#
        );
        write_node_json(dir.path(), 1, &json);
        let err = Node::load_from_dir(1, &origin(), dir.path()).unwrap_err();
        assert!(
            matches!(err, StoreError::InvariantViolation(_)),
            "expected InvariantViolation, got {err:?}"
        );
    }

    #[test]
    fn load_rejects_duplicate_chest_indices() {
        let dir = tempfile::tempdir().unwrap();
        let zeros: Vec<i32> = vec![0; crate::constants::DOUBLE_CHEST_SLOTS];
        let zeros_json = serde_json::to_string(&zeros).unwrap();
        // Two chests with index=0 (and no index=3) — must be rejected.
        let json = format!(
            r#"{{
              "id": 1,
              "position": {{"x":0,"y":64,"z":0}},
              "chests": [
                {{"id": 4, "node_id": 1, "index": 0, "position": {{"x":0,"y":0,"z":0}}, "item": "", "amounts": {zeros_json}}},
                {{"id": 4, "node_id": 1, "index": 0, "position": {{"x":0,"y":0,"z":0}}, "item": "", "amounts": {zeros_json}}},
                {{"id": 5, "node_id": 1, "index": 1, "position": {{"x":0,"y":0,"z":0}}, "item": "", "amounts": {zeros_json}}},
                {{"id": 6, "node_id": 1, "index": 2, "position": {{"x":0,"y":0,"z":0}}, "item": "", "amounts": {zeros_json}}}
              ]
            }}"#
        );
        write_node_json(dir.path(), 1, &json);
        let err = Node::load_from_dir(1, &origin(), dir.path()).unwrap_err();
        assert!(
            matches!(err, StoreError::InvariantViolation(_)),
            "expected InvariantViolation, got {err:?}"
        );
    }

    #[test]
    fn load_rejects_chest_id_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let zeros: Vec<i32> = vec![0; crate::constants::DOUBLE_CHEST_SLOTS];
        let zeros_json = serde_json::to_string(&zeros).unwrap();
        // Node 1 chest at index 2 should have id = 1*4+2 = 6, but we lie
        // and write id=99 — `check_invariants` must catch it.
        let json = format!(
            r#"{{
              "id": 1,
              "position": {{"x":0,"y":64,"z":0}},
              "chests": [
                {{"id": 4, "node_id": 1, "index": 0, "position": {{"x":0,"y":0,"z":0}}, "item": "", "amounts": {zeros_json}}},
                {{"id": 5, "node_id": 1, "index": 1, "position": {{"x":0,"y":0,"z":0}}, "item": "", "amounts": {zeros_json}}},
                {{"id": 99, "node_id": 1, "index": 2, "position": {{"x":0,"y":0,"z":0}}, "item": "", "amounts": {zeros_json}}},
                {{"id": 7, "node_id": 1, "index": 3, "position": {{"x":0,"y":0,"z":0}}, "item": "", "amounts": {zeros_json}}}
              ]
            }}"#
        );
        write_node_json(dir.path(), 1, &json);
        let err = Node::load_from_dir(1, &origin(), dir.path()).unwrap_err();
        assert!(
            matches!(err, StoreError::InvariantViolation(_)),
            "expected InvariantViolation, got {err:?}"
        );
    }

    #[test]
    fn load_rejects_amounts_len_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let zeros_full: Vec<i32> = vec![0; crate::constants::DOUBLE_CHEST_SLOTS];
        let zeros_full_json = serde_json::to_string(&zeros_full).unwrap();
        // 53 entries — one short of DOUBLE_CHEST_SLOTS.
        let zeros_short: Vec<i32> = vec![0; crate::constants::DOUBLE_CHEST_SLOTS - 1];
        let zeros_short_json = serde_json::to_string(&zeros_short).unwrap();
        let json = format!(
            r#"{{
              "id": 1,
              "position": {{"x":0,"y":64,"z":0}},
              "chests": [
                {{"id": 4, "node_id": 1, "index": 0, "position": {{"x":0,"y":0,"z":0}}, "item": "", "amounts": {zeros_full_json}}},
                {{"id": 5, "node_id": 1, "index": 1, "position": {{"x":0,"y":0,"z":0}}, "item": "", "amounts": {zeros_short_json}}},
                {{"id": 6, "node_id": 1, "index": 2, "position": {{"x":0,"y":0,"z":0}}, "item": "", "amounts": {zeros_full_json}}},
                {{"id": 7, "node_id": 1, "index": 3, "position": {{"x":0,"y":0,"z":0}}, "item": "", "amounts": {zeros_full_json}}}
              ]
            }}"#
        );
        write_node_json(dir.path(), 1, &json);
        let err = Node::load_from_dir(1, &origin(), dir.path()).unwrap_err();
        assert!(
            matches!(err, StoreError::InvariantViolation(_)),
            "expected InvariantViolation, got {err:?}"
        );
    }

    #[test]
    fn load_refuses_relabel_on_node_0_chest_with_nonempty_stockpile() {
        let dir = tempfile::tempdir().unwrap();
        // Chest 0 mis-labelled as iron_ingot but holds 100 units of it —
        // silently relabelling to diamond would mint currency, so load must
        // refuse.
        let mut amounts = vec![0; crate::constants::DOUBLE_CHEST_SLOTS];
        amounts[0] = 100;
        let json = node_0_json("iron_ingot", amounts);
        write_node_json(dir.path(), 0, &json);
        let err = Node::load_from_dir(0, &origin(), dir.path()).unwrap_err();
        match err {
            StoreError::InvariantViolation(msg) => {
                assert!(
                    msg.contains("refusing to relabel"),
                    "expected 'refusing to relabel' in error, got: {msg}"
                );
            }
            other => panic!("expected InvariantViolation, got {other:?}"),
        }
    }

    #[test]
    fn load_relabels_node_0_empty_reserved_chest() {
        let dir = tempfile::tempdir().unwrap();
        // Wrong item but empty stockpile — load should silently relabel
        // back to BASE_CURRENCY_ITEM (diamond) and persist the fix.
        let amounts = vec![0; crate::constants::DOUBLE_CHEST_SLOTS];
        let json = node_0_json("wrong_item", amounts);
        write_node_json(dir.path(), 0, &json);
        let loaded = Node::load_from_dir(0, &origin(), dir.path()).unwrap();
        assert_eq!(
            loaded.chests[0].item,
            crate::constants::BASE_CURRENCY_ITEM,
            "empty reserved chest should be relabeled to base currency"
        );
    }
}
