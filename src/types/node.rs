use std::fs;
use std::path::Path;

use crate::types::chest::Chest;
use crate::types::position::Position;

#[derive(Debug)]
pub struct Node {
    pub id: i32,
    pub position: Position, // position in the world
    pub chests: Vec<Chest>, // might wanna make this a fixed array of size 4
}

impl Node {
    /// Creates a new Node with the given ID and storage position.
    /// Automatically creates 4 chests positioned around the node.
    pub fn new(node_id: i32, storage_position: &Position) -> Node {
        let node_position = Self::calc_position(node_id, storage_position);

        // Create 4 chests with their respective positions
        let mut chests = Vec::with_capacity(4);

        for index in 0..4 {
            let chest = Chest::new(node_id, &node_position, index);
            chests.push(chest);
        }

        Node {
            id: node_id,
            position: node_position,
            chests,
        }
    }

    pub fn load(id: i32, storage_position: &Position) -> Result<Self, Box<dyn std::error::Error>> {
        let dir_path = format!("data/storage/{}", id);

        // Check if directory exists
        if !Path::new(&dir_path).exists() {
            return Err(format!("Node directory not found: {}", dir_path).into());
        }

        // Read all chest files in the directory
        let mut chests = Vec::new();
        let entries = fs::read_dir(&dir_path)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            // Only process .json files
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                // Extract index from filename (e.g., "0.json" -> 0)
                if let Some(filename) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(index) = filename.parse::<i32>() {
                        match Chest::load(id, index) {
                            Ok(chest) => chests.push(chest),
                            Err(e) => {
                                eprintln!("Warning: Failed to load chest {}/{}: {}", id, index, e)
                            }
                        }
                    }
                }
            }
        }

        // Sort chests by index for consistent ordering
        chests.sort_by_key(|chest| chest.index);

        // Create node
        let node = Node {
            id,
            position: Self::calc_position(id, storage_position), // calc position from id and storage_position
            chests,
        };

        Ok(node)
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        // Save each chest individually
        for chest in &self.chests {
            chest.save()?;
        }

        Ok(())
    }

    /// Calculates the world position of a node based on its ID and storage origin position.
    /// Nodes are arranged in a spiral pattern starting from the center (node 0).
    /// Each node is spaced 3 units apart from adjacent nodes.
    fn calc_position(id: i32, storage_position: &Position) -> Position {
        let (dx, dz) = if id == 0 {
            (-2, 0)
        } else {
            // Find ring: solve n*(n+1)*4 < id+1 <= (n+1)*(n+2)*4
            let mut ring = 1;
            while ring * (ring + 1) * 4 < id + 1 {
                ring += 1;
            }

            let pos_in_ring = id - (ring - 1) * ring * 4;
            let side = 2 * ring;

            match pos_in_ring / side {
                0 => (-2 + ring, -ring + pos_in_ring),              // Right
                1 => (-2 + ring - (pos_in_ring - side), ring),      // Bottom
                2 => (-2 - ring, ring - (pos_in_ring - 2 * side)),  // Left
                _ => (-2 - ring + (pos_in_ring - 3 * side), -ring), // Top
            }
        };

        Position {
            x: storage_position.x + dx * 3,
            y: storage_position.y,
            z: storage_position.z + dz * 3,
        }
    }
}
