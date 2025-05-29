use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::position::Position;

#[derive(Debug, Serialize, Deserialize)]
pub struct Chest {
    pub id: i32,
    pub node_id: i32,
    pub index: i32,         // 0 to 3 (4 chests per node)
    pub position: Position, // position in the world
    pub item: String,
    pub amounts: Vec<i32>, // amount of items in each of the 54 shulkers in a chest (might wanna make it a fixed array of size 54)
                           // might wanna have amount be -1 if shulker is missing (need to figure out how to handle replenishing of empty slots)
}

impl Chest {
    /// Creates a new Chest with the given node_id, node position, and index.
    /// The chest ID is calculated as node_id * 4 + index.
    /// Item is initialized as empty string and amounts as vector of 54 zeros.
    /// Position is calculated based on node position and chest index.
    pub fn new(node_id: i32, node_position: &Position, index: i32) -> Chest {
        let id = node_id * 4 + index;

        // Calculate chest position based on node position and index
        let position = match index {
            0 => Position {
                x: node_position.x,
                y: node_position.y + 1,
                z: node_position.z - 1,
            },
            1 => Position {
                x: node_position.x + 1,
                y: node_position.y + 1,
                z: node_position.z - 1,
            },
            2 => Position {
                x: node_position.x,
                y: node_position.y,
                z: node_position.z - 1,
            },
            3 => Position {
                x: node_position.x + 1,
                y: node_position.y,
                z: node_position.z - 1,
            },
            _ => panic!("Invalid chest index: {}", index),
        };

        Chest {
            id,
            node_id,
            index,
            position,
            item: String::new(),  // empty string
            amounts: vec![0; 54], // vector of 54 zeros
        }
    }

    pub fn node_position(&self) -> Position {
        match self.index {
            0 => Position {
                x: self.position.x,
                y: self.position.y - 1,
                z: self.position.z + 1,
            },
            1 => Position {
                x: self.position.x - 1,
                y: self.position.y - 1,
                z: self.position.z + 1,
            },
            2 => Position {
                x: self.position.x,
                y: self.position.y,
                z: self.position.z + 1,
            },
            3 => Position {
                x: self.position.x - 1,
                y: self.position.y,
                z: self.position.z + 1,
            },
            _ => panic!("Invalid chest index: {}", self.index),
        }
    }

    // position should make be recalculated on each load idk just in case the storage has moved or something idk
    pub fn load(node_id: i32, index: i32) -> Result<Self, Box<dyn std::error::Error>> {
        let file_path = format!("data/storage/{}/{}.json", node_id, index);

        // Check if file exists
        if !Path::new(&file_path).exists() {
            return Err(format!("Chest file not found: {}", file_path).into());
        }

        // Read file contents
        let json_data = fs::read_to_string(&file_path)?;

        // Deserialize JSON to Chest struct
        let chest: Chest = serde_json::from_str(&json_data)?;

        Ok(chest)
    }

    pub fn load_by_id(id: i32) -> Result<Self, Box<dyn std::error::Error>> {
        let node_id = id / 4;
        let index = id % 4;
        Ok(Self::load(node_id, index)?)
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let dir_path = format!("data/storage/{}", self.node_id);
        let file_path = format!("{}/{}.json", dir_path, self.index);

        // Create directories if they don't exist
        fs::create_dir_all(&dir_path)?;

        // Serialize struct to JSON
        let json_data = serde_json::to_string_pretty(self)?;

        // Write JSON to file
        fs::write(&file_path, json_data)?;

        Ok(())
    }
}
