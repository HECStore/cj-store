//! # Chest Management
//!
//! Represents a single chest in the storage system.
//!
//! ## Model
//! - **54 slots** (standard double chest)
//! - **Each slot contains 1 shulker box** (any color, treated equally)
//! - **`amounts[i]`** = item count **inside** the shulker in slot `i`
//!
//! ## Persistence
//! Chests are stored as part of their parent node in `data/storage/{node_id}.json`.
//! Individual chest files are no longer used - nodes contain all their chests.
//!
//! ## Position Calculation
//! Chest positions are derived from node position and chest index.
//! See `Chest::new()` for offset calculations.

use serde::{Deserialize, Serialize};

use crate::constants::{CHESTS_PER_NODE, DOUBLE_CHEST_SLOTS};
use crate::types::item_id::ItemId;
use crate::types::position::Position;

/// Represents a single chest in the storage system.
///
/// **Model**: 54-slot double chest where each slot contains 1 shulker box.
///
/// **Storage**:
/// - `item`: Item type stored in this chest (empty = unassigned via `ItemId::EMPTY`)
/// - `amounts[i]`: Item count inside the shulker box in slot `i`
///
/// **Invariants**:
/// - `amounts.len() == 54` (enforced by `Storage::normalize_amounts_len()`)
/// - `amounts[i] >= 0`
/// - `amounts[i] <= pair.shulker_capacity()` (varies by item stack size)
///   - Most items: 27 × 64 = 1728 max per shulker
///   - Stack-16 items (ender pearls, etc.): 27 × 16 = 432 max
///   - Non-stackable items (tools, etc.): 27 × 1 = 27 max
/// - If `item.is_empty()`, all `amounts` should be 0 (empty chest)
///
/// **ID Calculation**: `id = node_id * 4 + index` (4 chests per node, indices 0-3)
///
/// **Position**: Calculated from node position + index offset (see `Chest::new()`)
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Chest {
    /// Unique chest ID: `node_id * 4 + index`
    pub id: i32,
    /// Parent node ID
    pub node_id: i32,
    /// Index within node (0-3, 4 chests per node)
    pub index: i32,
    /// World position of the chest (for bot navigation)
    pub position: Position,
    /// Item type stored in this chest (`ItemId::EMPTY` = unassigned)
    pub item: ItemId,
    /// Item count per slot (54 slots, each contains 1 shulker box)
    /// `amounts[i]` = items inside the shulker in slot `i`
    pub amounts: Vec<i32>,
}

impl Chest {
    /// Creates a new Chest with ID `node_id * 4 + index`.
    ///
    /// # Panics
    /// Panics if `index` is not in `0..4`. Invalid indices indicate a bug in
    /// the caller (Node::new, Node::load) — these callers control the index
    /// directly, so an out-of-range value is unrecoverable, not a runtime error.
    pub fn new(node_id: i32, node_position: &Position, index: i32) -> Chest {
        let id = node_id * CHESTS_PER_NODE as i32 + index;
        let position = Self::calc_position(node_position, index);

        Chest {
            id,
            node_id,
            index,
            position,
            item: ItemId::EMPTY,
            amounts: vec![0; DOUBLE_CHEST_SLOTS],
        }
    }

    /// Calculate the world position of a chest from its parent node's position and its index.
    ///
    /// Returned position is the block the bot interacts with (the south-facing
    /// front block of the double chest), not the chest block itself — that is
    /// why every branch uses `z - 1`.
    ///
    /// Layout (top down, P is southeast corner at x, z):
    /// ```text
    /// NCCN  <- z-2 (back of double chests, not accessed)
    /// NCCN  <- z-1 (front of double chests, where we click)
    /// NSNP  <- z (working row; N = empty, S = shulker station, P = bot position)
    /// ```
    /// When standing at P looking north, chest indices are:
    /// ```text
    /// 01  <- y+1 (top row)
    /// 23  <- y (bottom row)
    /// ```
    /// Left column: x-2. Right column: x-1.
    ///
    /// # Panics
    /// Panics if `index` is not in range 0-3. This is a programming error;
    /// all callers (Node::new, Node::load, Node::calc_chest_position) control
    /// the index parameter directly.
    pub fn calc_position(node_position: &Position, index: i32) -> Position {
        match index {
            0 => Position {
                x: node_position.x - 2,
                y: node_position.y + 1,
                z: node_position.z - 1,
            },
            1 => Position {
                x: node_position.x - 1,
                y: node_position.y + 1,
                z: node_position.z - 1,
            },
            2 => Position {
                x: node_position.x - 2,
                y: node_position.y,
                z: node_position.z - 1,
            },
            3 => Position {
                x: node_position.x - 1,
                y: node_position.y,
                z: node_position.z - 1,
            },
            _ => panic!("Invalid chest index: {index} (must be 0-3)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node_origin() -> Position { Position { x: 100, y: 70, z: -50 } }

    #[test]
    fn new_computes_id_from_node_and_index() {
        for index in 0..4 {
            let c = Chest::new(3, &node_origin(), index);
            assert_eq!(c.id, 3 * CHESTS_PER_NODE as i32 + index);
            assert_eq!(c.node_id, 3);
            assert_eq!(c.index, index);
            assert_eq!(c.amounts.len(), DOUBLE_CHEST_SLOTS);
            assert!(c.item.is_empty());
            assert!(c.amounts.iter().all(|&a| a == 0));
        }
    }

    #[test]
    fn calc_position_matches_layout_for_all_indices() {
        let n = node_origin();
        // All chests use z-1 (front of double chest); columns x-2 / x-1; rows y+1 / y.
        assert_eq!(Chest::calc_position(&n, 0), Position { x: n.x - 2, y: n.y + 1, z: n.z - 1 });
        assert_eq!(Chest::calc_position(&n, 1), Position { x: n.x - 1, y: n.y + 1, z: n.z - 1 });
        assert_eq!(Chest::calc_position(&n, 2), Position { x: n.x - 2, y: n.y,     z: n.z - 1 });
        assert_eq!(Chest::calc_position(&n, 3), Position { x: n.x - 1, y: n.y,     z: n.z - 1 });
    }

    #[test]
    #[should_panic(expected = "Invalid chest index: 4")]
    fn calc_position_panics_on_out_of_range_index() {
        Chest::calc_position(&node_origin(), 4);
    }

    #[test]
    #[should_panic(expected = "Invalid chest index: -1")]
    fn calc_position_panics_on_negative_index() {
        Chest::calc_position(&node_origin(), -1);
    }
}
