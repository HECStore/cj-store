use std::fs;
use std::path::Path;

use crate::chest::Chest;
use crate::node::Node;
use crate::position::Position;

#[derive(Debug, Default)]
pub struct Storage {
    pub position: Position, // position in the world (origin)
    pub nodes: Vec<Node>,
}

impl Storage {
    pub fn new(position: Position) -> Self {
        Storage {
            position,
            nodes: Vec::new(),
        }
    }

    /// Save the storage by calling save() on all nodes
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        for node in &self.nodes {
            node.save()?;
        }
        Ok(())
    }

    /// Load storage nodes by reading all folders in data/storage and loading corresponding nodes
    pub fn load(storage_position: &Position) -> Result<Self, Box<dyn std::error::Error>> {
        let storage_path = "data/storage";

        // Create directory if it doesn't exist
        if !Path::new(storage_path).exists() {
            fs::create_dir_all(storage_path)?;
            return Ok(Storage {
                position: *storage_position,
                nodes: Vec::new(),
            });
        }

        let mut nodes = Vec::new();

        // Read all entries in the storage directory
        for entry in fs::read_dir(storage_path)? {
            let entry = entry?;
            let path = entry.path();

            // Only process directories
            if path.is_dir() {
                if let Some(folder_name) = path.file_name() {
                    if let Some(folder_str) = folder_name.to_str() {
                        // Parse folder name as node ID
                        if let Ok(node_id) = folder_str.parse::<i32>() {
                            // Load the node using its ID and the storage position
                            match Node::load(node_id, &storage_position) {
                                Ok(node) => nodes.push(node),
                                Err(e) => {
                                    // Log error but continue loading other nodes
                                    eprintln!("Failed to load node {}: {}", node_id, e);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(Storage {
            position: *storage_position,
            nodes,
        })
    }

    // Return mutable references to chests
    pub fn chests_with_item_mut(&mut self, item: &str) -> Vec<&mut Chest> {
        let mut chests: Vec<&mut Chest> = self
            .nodes
            .iter_mut()
            .flat_map(|node| &mut node.chests)
            .filter(|chest| chest.item == item)
            .collect();

        chests.sort_by_key(|chest| chest.id);
        chests
    }

    // Return just IDs (useful for storage/serialization)
    pub fn chest_ids_with_item(&self, item: &str) -> Vec<i32> {
        let mut chest_ids: Vec<i32> = self
            .nodes
            .iter()
            .flat_map(|node| &node.chests)
            .filter(|chest| chest.item == item)
            .map(|chest| chest.id)
            .collect();

        chest_ids.sort();
        chest_ids
    }

    pub fn get_chest_mut(&mut self, chest_id: i32) -> Option<&mut Chest> {
        let node_id = chest_id / 4;
        let index = chest_id % 4;

        // Find the node with the matching id
        let node = self.nodes.iter_mut().find(|node| node.id == node_id)?;

        // Get the chest at the specified index
        node.chests.get_mut(index as usize)
    }

    pub fn add_node(&mut self) -> &Node {
        let node_id = self.nodes.len() as i32;
        let node = Node::new(node_id, &self.position);
        self.nodes.push(node);
        self.nodes.last().unwrap()
    }

    // might need a method for initializing new storage via bot (it visiting each node one by one until it cant find any more)
}
