//! # 3D Position Type
//!
//! Simple Minecraft world coordinates used for chest, node, and storage positions.

use serde::{Deserialize, Serialize};

/// A 3D position in Minecraft world coordinates.
///
/// Uses `i32` because these refer to discrete block positions, not entity
/// positions (which would be floating-point). This matches the coordinate
/// type used by Minecraft's block API and avoids rounding ambiguity when
/// addressing chests or other fixed-grid structures.
/// - `x`: East (+) / West (-) axis
/// - `y`: Height (0-320 typical, -64 minimum in modern Minecraft)
/// - `z`: South (+) / North (-) axis
///
/// **Default**: Origin (0, 0, 0)
///
/// **Usage**: Represents positions of nodes, chests, and the storage origin.
#[derive(Debug, PartialEq, Serialize, Deserialize, Default, Clone, Copy)]
pub struct Position {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_origin() {
        assert_eq!(Position::default(), Position { x: 0, y: 0, z: 0 });
    }

    #[test]
    fn serde_round_trip_preserves_negative_and_extreme_coords() {
        let p = Position { x: -1_234, y: -64, z: i32::MAX };
        let json = serde_json::to_string(&p).unwrap();
        let back: Position = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }
}
