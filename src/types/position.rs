//! # 3D Position Type
//!
//! Simple Minecraft world coordinates used for chest, node, and storage positions.

use serde::{Deserialize, Serialize};

/// A 3D position in Minecraft world coordinates.
///
/// Uses integer coordinates (block positions).
/// - `x`: East (+) / West (-) axis
/// - `y`: Height (0-320 typical, -64 minimum in modern Minecraft)
/// - `z`: South (+) / North (-) axis
///
/// **Default**: Origin (0, 0, 0)
///
/// **Usage**: Represents positions of nodes, chests, and the storage origin.
#[derive(Debug, PartialEq, Serialize, Deserialize, Default, Clone, Copy)]
pub struct Position {
    /// X coordinate (East-West axis)
    pub x: i32,
    /// Y coordinate (Height)
    pub y: i32,
    /// Z coordinate (North-South axis)
    pub z: i32,
}
