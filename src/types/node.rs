//! # Node Management
//!
//! Represents a storage node: a cluster of 4 chests arranged in a 2×2 pattern.
//!
//! ## Layout
//! Each node has this footprint (top-down):
//! ```
//! NNNN
//! NCCN  (2 chests on top, 2 chests on bottom - 2 blocks tall total)
//! NCCN
//! NSNP  (N = nothing/empty, S = shulker station, P = bot position)
//! ```
//!
//! ## Spiral Pattern
//! Nodes are arranged in a spiral starting from the center (node 0):
//! ```
//! . 6 7 8 9
//! . 5 0 1 .
//! . 4 3 2 .
//! ```
//! Spaced **3 blocks** apart. See `calc_position()` for algorithm.
//!
//! ## Persistence
//! Nodes are stored as files: `data/storage/{node_id}.json`
//! Each node file contains the node's position and all 4 chests.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::fsutil::write_atomic;
use crate::types::chest::Chest;
use crate::types::ItemId;
use crate::types::position::Position;

/// Represents a storage node: a cluster of 4 chests with a bot access position.
///
/// **Layout**: 2×2 chest arrangement, 2 blocks tall, with bot position and shulker station.
///
/// **Chests**: Always exactly 4 chests (indices 0-3), arranged around the node position.
///
/// **Position**: Calculated from storage origin + node ID using spiral algorithm.
/// See `calc_position()` for details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// Node ID (used as filename: `data/storage/{id}.json`)
    pub id: i32,
    /// World position where bot stands to access this node
    pub position: Position,
    /// List of 4 chests (indices 0-3)
    pub chests: Vec<Chest>,
}

impl Node {
    /// Creates a new Node with the given ID and storage position.
    /// Automatically creates 4 chests positioned around the node.
    ///
    /// Note: `storage_position` is the storage *origin* (node 0's location),
    /// not the node's own position. The node's world position is derived from
    /// the origin via `calc_position`.
    pub fn new(node_id: i32, storage_position: &Position) -> Node {
        let node_position = Self::calc_position(node_id, storage_position);

        // Create 4 chests with their respective positions
        let mut chests = Vec::with_capacity(4);

        for index in 0..4 {
            let mut chest = Chest::new(node_id, &node_position, index);

            // Node 0 has special reserved chests (forced, cannot change)
            if node_id == 0 {
                if index == 0 {
                    // Chest 0: dedicated for diamonds
                    chest.item = ItemId::from_normalized("diamond".to_string());
                } else if index == 1 {
                    // Chest 1: overflow/failsafe for unknown/leftover items
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
    /// Positions in the file are treated as derivable state: we always
    /// recompute the node position and chest positions from `storage_position`
    /// so that moving the storage origin in config correctly relocates existing
    /// nodes without requiring a data migration. Only the chest `item`
    /// assignments are authoritative on disk.
    ///
    /// For node 0, the reserved chest invariants (chest 0 = diamond,
    /// chest 1 = overflow) are re-enforced on load in case the file was edited
    /// manually, and any correction is persisted back to disk.
    pub fn load(id: i32, storage_position: &Position) -> Result<Self, Box<dyn std::error::Error>> {
        let file_path = format!("data/storage/{}.json", id);

        // Check if file exists
        if !Path::new(&file_path).exists() {
            return Err(format!("Node file not found: {}", file_path).into());
        }

        // Read file contents
        let json_data = fs::read_to_string(&file_path)?;

        // Deserialize JSON to Node struct
        let mut node: Node = serde_json::from_str(&json_data)?;

        // Ensure node ID matches (safety check)
        if node.id != id {
            return Err(format!("Node ID mismatch: expected {}, got {}", id, node.id).into());
        }

        // Calculate expected node position from storage origin
        let expected_position = Self::calc_position(id, storage_position);

        // Update position if storage origin has moved (recalculate for consistency)
        node.position = expected_position;

        // Recalculate chest positions to ensure consistency (in case storage origin moved)
        for chest in &mut node.chests {
            chest.position = Chest::new(id, &node.position, chest.index).position;
        }

        // Ensure we have exactly 4 chests
        if node.chests.len() != 4 {
            return Err(format!("Node {} has {} chests, expected 4", id, node.chests.len()).into());
        }

        // Sort chests by index for consistent ordering
        node.chests.sort_by_key(|chest| chest.index);

        // Node 0 has special reserved chests (forced, cannot change)
        // Enforce this even when loading from disk
        if id == 0 {
            let mut needs_save = false;

            // Chest 0: dedicated for diamonds
            if let Some(chest_0) = node.chests.get_mut(0) {
                if chest_0.item != "diamond" {
                    chest_0.item = ItemId::from_normalized("diamond".to_string());
                    needs_save = true;
                }
            }

            // Chest 1: overflow/failsafe for unknown/leftover items
            if let Some(chest_1) = node.chests.get_mut(1) {
                if chest_1.item != crate::constants::OVERFLOW_CHEST_ITEM {
                    chest_1.item = ItemId::from_normalized(crate::constants::OVERFLOW_CHEST_ITEM.to_string());
                    needs_save = true;
                }
            }

            if needs_save {
                if let Err(e) = node.save() {
                    eprintln!(
                        "Warning: Failed to save node 0 reserved chest assignments: {}",
                        e
                    );
                }
            }
        }

        Ok(node)
    }

    /// Serializes this node to `data/storage/{id}.json`.
    ///
    /// Uses `write_atomic` (write-to-temp + rename) so a crash mid-write
    /// cannot leave a partially-written node file on disk.
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let file_path = format!("data/storage/{}.json", self.id);
        tracing::debug!("[Node] Saving node {} to {:?}", self.id, file_path);

        // Ensure the directory exists
        if let Some(parent_dir) = Path::new(&file_path).parent() {
            if !parent_dir.exists() {
                tracing::debug!("[Node] Creating parent directory for node {}", self.id);
                fs::create_dir_all(parent_dir)?;
            }
        }

        // Serialize node to JSON
        tracing::debug!("[Node] Serializing node {} to JSON", self.id);
        let json_data = serde_json::to_string_pretty(self)?;
        tracing::debug!(
            "[Node] Node {} serialized ({} bytes)",
            self.id,
            json_data.len()
        );

        // Write JSON to file (atomically)
        tracing::debug!("[Node] Writing node {} to file", self.id);
        write_atomic(&file_path, &json_data)?;
        tracing::debug!("[Node] Node {} saved successfully", self.id);

        Ok(())
    }

    /// Calculates the world position of a node based on its ID and storage origin.
    ///
    /// **Algorithm**: Spiral pattern starting from center (node 0).
    ///
    /// **Spacing**: Each node is **3 blocks** apart from adjacent nodes.
    ///
    /// **Pattern**:
    /// ```
    /// . 6 7 8 9
    /// . 5 0 1 .
    /// . 4 3 2 .
    /// ```
    ///
    /// **Ring Calculation**:
    /// - Ring 0: Node 0 only (center)
    /// - Ring 1: Nodes 1-8 (8 nodes)
    /// - Ring 2: Nodes 9-24 (16 nodes)
    /// - Ring n: Nodes (n-1)*n*4+1 to n*(n+1)*4 (8*n nodes)
    ///
    /// **Position Formula**:
    /// - Node 0: `storage_origin` (P aligns with storage origin)
    /// - Other nodes: calculated by ring and side (Right/Bottom/Left/Top)
    ///
    /// See `README.md` "Persistence layout - Node" for examples.
    pub fn calc_position(id: i32, storage_position: &Position) -> Position {
        if id == 0 {
            // Center node: P aligns with storage origin
            Position {
                x: storage_position.x,
                y: storage_position.y,
                z: storage_position.z,
            }
        } else {
            // Find which ring this node belongs to.
            // Ring n is a square "shell" of 8*n nodes surrounding ring n-1,
            // so ring n contains IDs (n-1)*n*4+1 ..= n*(n+1)*4.
            // We walk outward from ring 1 until the ring's last ID is >= our id.
            // (Closed-form via sqrt would work but this loop is O(sqrt(id))
            // and avoids any floating-point rounding concerns.)
            let mut ring = 1;
            while ring * (ring + 1) * 4 < id {
                ring += 1;
            }

            // Offset of this id within its ring, starting at 0.
            let pos_in_ring = id - (ring - 1) * ring * 4;
            // Each of the 4 sides of a ring holds `2*ring` nodes; together
            // they cover the 8*ring nodes of the ring.
            let side = 2 * ring;

            // Walk the ring clockwise in (dx, dz) grid coordinates (pre-scaling).
            // The ring is the square of radius `ring` centered on node 0, and
            // we traverse: Right edge (top->bottom), Bottom edge (right->left),
            // Left edge (bottom->top), Top edge (left->right). This produces
            // the spiral shown in the module docs.
            //
            // Note: -z is "up" / north in Minecraft, so "top side" uses dz=-ring.
            let (dx, dz) = match pos_in_ring / side {
                0 => (ring, -ring + pos_in_ring),               // Right side (walking +z)
                1 => (ring - (pos_in_ring - side), ring),       // Bottom side (walking -x)
                2 => (-ring, ring - (pos_in_ring - 2 * side)),  // Left side (walking -z)
                _ => (-ring + (pos_in_ring - 3 * side), -ring), // Top side (walking +x)
            };

            // Scale grid coordinates by 3-block spacing. The 3-block gap
            // leaves room for the 2-wide chest footprint plus a walking lane
            // between adjacent nodes, preventing their 2x2 chest clusters
            // from overlapping.
            Position {
                x: storage_position.x + dx * 3,
                y: storage_position.y,
                z: storage_position.z + dz * 3,
            }
        }
    }

    /// Returns the shulker station position for this node.
    /// Layout (top down, P is southeast corner):
    /// ```
    /// NCCN  <- z-2
    /// NCCN  <- z-1
    /// XSNP  <- z (S at x-2, P at x)
    /// ```
    /// Shulker station is 2 blocks west of P, at the same Y and Z level.
    /// Reserved for future use (currently calculated inline in bot.rs).
    #[allow(dead_code)] // reserved; bot currently computes inline
    pub fn shulker_station_position(&self) -> Position {
        Position {
            x: self.position.x - 2,
            y: self.position.y,
            z: self.position.z,
        }
    }

    /// Calculate chest position from node ID, chest index, and node position.
    ///
    /// This is a static method that can calculate a chest's world position
    /// without creating a full Chest object. Useful for node validation
    /// and for recomputing positions in `load()` when the storage origin
    /// has shifted.
    ///
    /// The returned position is the block the bot interacts with (the
    /// south-facing front block of the double chest), not the chest block
    /// itself — that is why every branch uses `z - 1`.
    ///
    /// # Arguments
    /// * `_node_id` - Node ID (unused, for future use)
    /// * `chest_index` - Chest index (0-3)
    /// * `node_position` - World position of the node
    ///
    /// # Layout (looking north from P)
    /// All chests accessed from z-1 (south face of double chests).
    /// ```
    /// 01  <- y+1 (top row)
    /// 23  <- y (bottom row)
    /// ```
    /// Chest 0,2: x-2 (left)    Chest 1,3: x-1 (right)
    ///
    /// # Panics
    /// Panics if chest_index is not in range 0-3.
    pub fn calc_chest_position(
        _node_id: i32,
        chest_index: i32,
        node_position: &Position,
    ) -> Position {
        match chest_index {
            0 => Position {
                x: node_position.x - 2, // Left column
                y: node_position.y + 1, // Top chest
                z: node_position.z - 1, // South face (where we click)
            },
            1 => Position {
                x: node_position.x - 1, // Right column
                y: node_position.y + 1, // Top chest
                z: node_position.z - 1, // South face (where we click)
            },
            2 => Position {
                x: node_position.x - 2, // Left column
                y: node_position.y,     // Bottom chest
                z: node_position.z - 1, // South face (where we click)
            },
            3 => Position {
                x: node_position.x - 1, // Right column
                y: node_position.y,     // Bottom chest
                z: node_position.z - 1, // South face (where we click)
            },
            _ => panic!("Invalid chest index: {} (must be 0-3)", chest_index),
        }
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn origin() -> Position {
        Position { x: 0, y: 64, z: 0 }
    }

    #[test]
    fn test_calc_position_node_0() {
        // Node 0 should be at the origin
        let pos = Node::calc_position(0, &origin());
        assert_eq!(pos.x, 0);
        assert_eq!(pos.y, 64);
        assert_eq!(pos.z, 0);
    }

    #[test]
    fn test_calc_position_ring_1() {
        // Ring 1 nodes (1-8) should be 3 blocks away from center
        let spacing = 3;

        // Node 1: Right side start (ring 1, pos 0)
        let pos1 = Node::calc_position(1, &origin());
        assert_eq!(pos1.x, spacing); // dx = ring = 1, scaled by 3

        // Node 3: Should be on bottom right
        let _pos3 = Node::calc_position(3, &origin());
        // Positions vary based on spiral algorithm

        // All ring 1 nodes should be at distance sqrt(9) = 3 or sqrt(18) ~= 4.24 from origin
        for id in 1..=8 {
            let pos = Node::calc_position(id, &origin());
            let dx = (pos.x - origin().x).abs();
            let dz = (pos.z - origin().z).abs();
            // Max distance should be ring * spacing = 1 * 3 = 3
            assert!(dx <= spacing, "Node {} has dx={} > {}", id, dx, spacing);
            assert!(dz <= spacing, "Node {} has dz={} > {}", id, dz, spacing);
        }
    }

    #[test]
    fn test_calc_position_deterministic() {
        // Same ID should always give same position
        let pos1 = Node::calc_position(5, &origin());
        let pos2 = Node::calc_position(5, &origin());

        assert_eq!(pos1.x, pos2.x);
        assert_eq!(pos1.y, pos2.y);
        assert_eq!(pos1.z, pos2.z);
    }

    #[test]
    fn test_calc_position_unique() {
        // Each node should have a unique position
        let mut positions = std::collections::HashSet::new();

        for id in 0..25 {
            let pos = Node::calc_position(id, &origin());
            let key = (pos.x, pos.z);
            assert!(positions.insert(key), "Node {} has duplicate position", id);
        }
    }

    #[test]
    fn test_calc_position_with_offset_origin() {
        // Test with non-zero origin
        let offset_origin = Position {
            x: 1000,
            y: 100,
            z: -500,
        };

        let pos0 = Node::calc_position(0, &offset_origin);
        assert_eq!(pos0.x, 1000);
        assert_eq!(pos0.y, 100);
        assert_eq!(pos0.z, -500);

        let pos1 = Node::calc_position(1, &offset_origin);
        // Should be offset from origin
        assert_ne!(pos1.x, offset_origin.x);
    }

    #[test]
    fn test_calc_chest_position() {
        let node_pos = Position {
            x: 100,
            y: 64,
            z: 200,
        };

        // All chests accessed from z-1 (south face of double chests).
        // Looking north from P:
        //   01  <- y+1 (top row)
        //   23  <- y (bottom row)
        // Left column: x-2, Right column: x-1

        // Chest 0: left, top (x-2, y+1, z-1)
        let chest0 = Node::calc_chest_position(0, 0, &node_pos);
        assert_eq!(chest0.x, 98); // x-2
        assert_eq!(chest0.y, 65); // y+1
        assert_eq!(chest0.z, 199); // z-1

        // Chest 1: right, top (x-1, y+1, z-1)
        let chest1 = Node::calc_chest_position(0, 1, &node_pos);
        assert_eq!(chest1.x, 99); // x-1
        assert_eq!(chest1.y, 65); // y+1
        assert_eq!(chest1.z, 199); // z-1

        // Chest 2: left, bottom (x-2, y, z-1)
        let chest2 = Node::calc_chest_position(0, 2, &node_pos);
        assert_eq!(chest2.x, 98); // x-2
        assert_eq!(chest2.y, 64); // same y
        assert_eq!(chest2.z, 199); // z-1

        // Chest 3: right, bottom (x-1, y, z-1)
        let chest3 = Node::calc_chest_position(0, 3, &node_pos);
        assert_eq!(chest3.x, 99); // x-1
        assert_eq!(chest3.y, 64); // same y
        assert_eq!(chest3.z, 199); // z-1
    }

    #[test]
    fn test_node_new() {
        let origin = Position {
            x: 50,
            y: 70,
            z: 100,
        };
        let node = Node::new(0, &origin);

        assert_eq!(node.id, 0);
        assert_eq!(node.chests.len(), 4);

        // Node 0, chest 0 should be assigned to diamond
        assert_eq!(node.chests[0].item, "diamond");

        // Node 0, chest 1 should be assigned to overflow
        assert_eq!(node.chests[1].item, crate::constants::OVERFLOW_CHEST_ITEM);

        // Node 0, chests 2-3 should be empty (unassigned)
        assert_eq!(node.chests[2].item, "");
        assert_eq!(node.chests[3].item, "");
    }

    #[test]
    fn test_shulker_station_position() {
        let origin = Position { x: 0, y: 64, z: 0 };
        let node = Node::new(0, &origin);

        let station = node.shulker_station_position();
        // Station should be at (x-2, y, z) relative to node position
        assert_eq!(station.x, node.position.x - 2);
        assert_eq!(station.y, node.position.y);
        assert_eq!(station.z, node.position.z);
    }
}
