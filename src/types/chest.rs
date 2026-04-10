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

use crate::types::position::Position;

/// Represents a single chest in the storage system.
///
/// **Model**: 54-slot double chest where each slot contains 1 shulker box.
///
/// **Storage**:
/// - `item`: Item type stored in this chest (empty string = unassigned)
/// - `amounts[i]`: Item count inside the shulker box in slot `i`
///
/// **Invariants**:
/// - `amounts.len() == 54` (enforced by `Storage::normalize_amounts_len()`)
/// - `amounts[i] >= 0` (negative values reserved for future use)
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
    /// Item type stored in this chest (empty = unassigned)
    pub item: String,
    /// Item count per slot (54 slots, each contains 1 shulker box)
    /// `amounts[i]` = items inside the shulker in slot `i`
    pub amounts: Vec<i32>,
}

impl Chest {
    /// Utility helper (reserved for future bot navigation logic).
    /// 
    /// # Returns
    /// * `Some(Position)` - The node position for valid chest indices (0-3)
    /// * `None` - If chest index is invalid
    /// 
    /// **Note**: In normal operation, chest index should always be 0-3.
    /// Invalid indices indicate a data corruption or programming error.
    /// 
    /// # Layout (looking north from P)
    /// ```
    /// 01  <- y+1 (top row)
    /// 23  <- y (bottom row)
    /// ```
    /// All chests at z-1 (south face), x-2 (left) or x-1 (right).
    #[allow(dead_code)]
    pub fn node_position(&self) -> Option<Position> {
        match self.index {
            0 => Some(Position {
                x: self.position.x + 2,  // Chest at x-2, so P at x
                y: self.position.y - 1,  // Chest at y+1, so P at y
                z: self.position.z + 1,  // Chest at z-1, so P at z
            }),
            1 => Some(Position {
                x: self.position.x + 1,  // Chest at x-1, so P at x
                y: self.position.y - 1,  // Chest at y+1, so P at y
                z: self.position.z + 1,  // Chest at z-1, so P at z
            }),
            2 => Some(Position {
                x: self.position.x + 2,  // Chest at x-2, so P at x
                y: self.position.y,      // Chest at y, so P at y
                z: self.position.z + 1,  // Chest at z-1, so P at z
            }),
            3 => Some(Position {
                x: self.position.x + 1,  // Chest at x-1, so P at x
                y: self.position.y,      // Chest at y, so P at y
                z: self.position.z + 1,  // Chest at z-1, so P at z
            }),
            _ => {
                // Unlike Chest::new() which panics on invalid index, this getter
                // logs and returns None because it may be called on deserialized
                // data where a corrupted JSON file should not crash the bot.
                tracing::error!("Invalid chest index: {} (expected 0-3)", self.index);
                None
            }
        }
    }

    /// Creates a new Chest with the given node_id, node position, and index.
    /// The chest ID is calculated as node_id * 4 + index.
    /// Item is initialized as empty string and amounts as vector of 54 zeros.
    /// Position is calculated based on node position and chest index.
    /// 
    /// # Arguments
    /// * `node_id` - The parent node's ID
    /// * `node_position` - The world position of the parent node
    /// * `index` - The chest index within the node (must be 0-3)
    /// 
    /// # Panics
    /// Panics if `index` is not in range 0-3. This is a programming error
    /// that should never occur in normal operation.
    /// 
    /// **Design note**: We use panic here because an invalid index indicates
    /// a bug in the calling code, not a runtime error that can be recovered from.
    /// All callers (Node::new, Node::load) control the index parameter directly.
    pub fn new(node_id: i32, node_position: &Position, index: i32) -> Chest {
        // Validate index at creation time
        assert!(
            (0..=3).contains(&index),
            "Invalid chest index: {} (must be 0-3). This is a programming error.",
            index
        );
        
        let id = node_id * 4 + index;

        // Calculate chest position based on node position and index
        // Layout (top down, P is southeast corner at x, z):
        // ```
        // NCCN  <- z-2 (back of double chests, not accessed)
        // NCCN  <- z-1 (front of double chests, where we click)
        // XSNP  <- z (working row)
        // ```
        // All 4 chests are accessed from z-1 (south face).
        // When standing at P looking north, chest IDs are:
        //   01  <- y+1 (top row)
        //   23  <- y (bottom row)
        // Chests are at x-2 (left) and x-1 (right), west of P.
        let position = match index {
            0 => Position {
                x: node_position.x - 2,  // Left column
                y: node_position.y + 1,  // Top chest
                z: node_position.z - 1,  // South face (where we click)
            },
            1 => Position {
                x: node_position.x - 1,  // Right column
                y: node_position.y + 1,  // Top chest
                z: node_position.z - 1,  // South face (where we click)
            },
            2 => Position {
                x: node_position.x - 2,  // Left column
                y: node_position.y,      // Bottom chest
                z: node_position.z - 1,  // South face (where we click)
            },
            3 => Position {
                x: node_position.x - 1,  // Right column
                y: node_position.y,      // Bottom chest
                z: node_position.z - 1,  // South face (where we click)
            },
            // This branch is unreachable due to the assert above,
            // but we include it for completeness
            _ => unreachable!("Index validated above"),
        };

        Chest {
            id,
            node_id,
            index,
            position,
            item: String::new(),  // empty = unassigned chest
            amounts: vec![0; 54], // one entry per slot in a double chest
        }
    }

}
