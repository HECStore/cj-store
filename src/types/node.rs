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
use crate::fsutil::write_atomic;
use crate::types::chest::Chest;
use crate::types::ItemId;
use crate::types::position::Position;

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
                    chest.item = ItemId::from_normalized("diamond".to_string());
                } else if index == 1 {
                    chest.item = ItemId::from_normalized(crate::constants::OVERFLOW_CHEST_ITEM.to_string());
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
    pub fn load(id: i32, storage_position: &Position) -> Result<Self, Box<dyn std::error::Error>> {
        let file_path = format!("data/storage/{}.json", id);

        if !Path::new(&file_path).exists() {
            return Err(format!("Node file not found: {}", file_path).into());
        }

        let json_data = fs::read_to_string(&file_path)?;
        let mut node: Node = serde_json::from_str(&json_data)?;

        if node.id != id {
            return Err(format!("Node ID mismatch: expected {}, got {}", id, node.id).into());
        }

        // Recompute positions from the current storage origin (see doc comment).
        node.position = Self::calc_position(id, storage_position);
        for chest in &mut node.chests {
            chest.position = Chest::calc_position(&node.position, chest.index);
        }

        if node.chests.len() != CHESTS_PER_NODE {
            return Err(format!(
                "Node {} has {} chests, expected {}",
                id,
                node.chests.len(),
                CHESTS_PER_NODE
            )
            .into());
        }

        node.chests.sort_by_key(|chest| chest.index);

        // Re-enforce node 0's reserved chest invariants in case the JSON was
        // hand-edited. Persist the correction so later loads see the fix.
        if id == 0 {
            let mut needs_save = false;

            if let Some(chest_0) = node.chests.get_mut(0)
                && chest_0.item != "diamond" {
                    chest_0.item = ItemId::from_normalized("diamond".to_string());
                    needs_save = true;
                }

            if let Some(chest_1) = node.chests.get_mut(1)
                && chest_1.item != crate::constants::OVERFLOW_CHEST_ITEM {
                    chest_1.item = ItemId::from_normalized(crate::constants::OVERFLOW_CHEST_ITEM.to_string());
                    needs_save = true;
                }

            if needs_save {
                tracing::warn!(
                    node_id = 0,
                    "node 0 reserved chest assignments were wrong on disk; rewriting"
                );
                if let Err(e) = node.save() {
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
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let file_path = format!("data/storage/{}.json", self.id);

        if let Some(parent_dir) = Path::new(&file_path).parent()
            && !parent_dir.exists() {
                fs::create_dir_all(parent_dir)?;
            }

        let json_data = serde_json::to_string_pretty(self)?;
        write_atomic(&file_path, &json_data)?;

        tracing::debug!(
            node_id = self.id,
            bytes = json_data.len(),
            "saved node"
        );
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
        let (dx, dz) = match pos_in_ring / side {
            0 => (ring, -ring + pos_in_ring),               // Right edge, walking +z
            1 => (ring - (pos_in_ring - side), ring),       // Bottom edge, walking -x
            2 => (-ring, ring - (pos_in_ring - 2 * side)),  // Left edge, walking -z
            _ => (-ring + (pos_in_ring - 3 * side), -ring), // Top edge, walking +x
        };

        // NODE_SPACING leaves room for the 2-wide chest footprint plus a
        // walking lane, so adjacent nodes' 2×2 chest clusters don't overlap.
        Position {
            x: storage_position.x + dx * NODE_SPACING,
            y: storage_position.y,
            z: storage_position.z + dz * NODE_SPACING,
        }
    }

    /// Thin wrapper around [`Chest::calc_position`] kept for path/naming
    /// stability with the bot validation call site.
    ///
    /// # Panics
    /// Panics if `chest_index` is not in `0..=3`.
    pub fn calc_chest_position(
        chest_index: i32,
        node_position: &Position,
    ) -> Position {
        Chest::calc_position(node_position, chest_index)
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
        let off = Position { x: 1000, y: 100, z: -500 };
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
            assert!(dx <= 2 * NODE_SPACING, "node {id} dx={dx} exceeds 2*spacing");
            assert!(dz <= 2 * NODE_SPACING, "node {id} dz={dz} exceeds 2*spacing");
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
                pos.x, pos.z
            );
        }
    }

    #[test]
    fn calc_position_translates_with_storage_origin() {
        // Moving the origin must translate every node by the same vector.
        let a = origin();
        let b = Position { x: 1000, y: 64, z: -500 };
        for id in 0..25 {
            let pa = Node::calc_position(id, &a);
            let pb = Node::calc_position(id, &b);
            assert_eq!(pb.x - pa.x, b.x - a.x, "node {id} x mistranslated");
            assert_eq!(pb.z - pa.z, b.z - a.z, "node {id} z mistranslated");
        }
    }

    #[test]
    fn calc_chest_position_matches_chest_calc_position() {
        // The wrapper must be an exact delegation for every valid index.
        let node_pos = Position { x: 100, y: 64, z: 200 };
        for idx in 0..CHESTS_PER_NODE as i32 {
            let via_node = Node::calc_chest_position(idx, &node_pos);
            let via_chest = Chest::calc_position(&node_pos, idx);
            assert_eq!(
                (via_node.x, via_node.y, via_node.z),
                (via_chest.x, via_chest.y, via_chest.z),
                "chest {idx} mismatch"
            );
        }
    }

    #[test]
    fn calc_chest_position_matches_documented_2x2_layout() {
        // Looking north (toward -z) from the bot position P:
        //   01  <- y+1 (top row)
        //   23  <- y (bottom row)
        // Left column: x-2, right column: x-1, all on the z-1 face.
        let p = Position { x: 100, y: 64, z: 200 };
        let expect = [
            (98, 65, 199),  // 0: left,  top
            (99, 65, 199),  // 1: right, top
            (98, 64, 199),  // 2: left,  bottom
            (99, 64, 199),  // 3: right, bottom
        ];
        for (idx, (ex, ey, ez)) in expect.iter().enumerate() {
            let c = Node::calc_chest_position(idx as i32, &p);
            assert_eq!((c.x, c.y, c.z), (*ex, *ey, *ez), "chest {idx} offset wrong");
        }
    }

    #[test]
    fn new_node_0_has_reserved_diamond_and_overflow_chests() {
        let node = Node::new(0, &Position { x: 50, y: 70, z: 100 });
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
            assert_eq!(c.item, "", "chest {i} on non-zero node should be unassigned");
        }
    }

    #[test]
    fn new_places_node_on_spiral_and_chests_relative_to_node() {
        // Node's own position must equal calc_position, and every chest
        // must sit at the documented offset from that position.
        let node = Node::new(7, &origin());
        let expected = Node::calc_position(7, &origin());
        assert_eq!((node.position.x, node.position.y, node.position.z),
                   (expected.x, expected.y, expected.z));
        for c in &node.chests {
            let expected_chest = Chest::calc_position(&node.position, c.index);
            assert_eq!((c.position.x, c.position.y, c.position.z),
                       (expected_chest.x, expected_chest.y, expected_chest.z),
                       "chest {} misplaced", c.index);
        }
    }
}
