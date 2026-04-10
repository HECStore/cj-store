//! # Trading Pair Management
//!
//! Represents a trading pair (item <-> diamonds) with reserve-based pricing.
//! Each pair is stored as: `data/pairs/{item}.json`
//!
//! ## Pricing Model
//! Prices are **derived dynamically** from reserve ratios (not stored):
//! - **Buy price** = `(currency_stock / item_stock) * (1 + fee)`
//! - **Sell price** = `(currency_stock / item_stock) * (1 - fee)`
//!
//! This implements a simple constant product market maker (CPMM) model.
//! See `README.md` "Reserve-based pricing" for details.

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::fsutil::write_atomic;

/// Represents a trading pair: item <-> diamonds (currency).
///
/// **Persistence**: Saved to `data/pairs/{item}.json`
///
/// **Pricing**: Prices are **not stored** but derived from reserves:
/// - Buy: `(currency_stock / item_stock) * (1 + fee)` - player pays more
/// - Sell: `(currency_stock / item_stock) * (1 - fee)` - player receives less
///
/// **Reserves**:
/// - `item_stock`: Total items available in storage (sum of all chests for this item)
/// - `currency_stock`: Total diamonds in the pair's reserve
///
/// **Invariants** (enforced by Store):
/// - `item_stock >= 0`
/// - `currency_stock >= 0`
/// - When `item_stock == 0`, buy orders fail (no items to sell)
/// - When `currency_stock == 0`, sell orders fail (no diamonds to pay)
///
/// **Future Enhancements**:
/// - Track trading volumes, fees collected, number of trades
/// - Consider separate buy/sell fees
/// - Add statistics computed from Trade history
#[derive(Debug, Default, PartialEq, Serialize, Deserialize, Clone)]
pub struct Pair {
    /// Item identifier (e.g., "gunpowder", "cobblestone")
    /// Stored WITHOUT the "minecraft:" prefix for cleaner display and storage.
    pub item: String,
    /// Maximum stack size for this item (1, 16, or 64)
    /// - 64: Most items (cobblestone, diamonds, etc.)
    /// - 16: Ender pearls, eggs, snowballs, signs, banners, buckets
    /// - 1: Tools, weapons, armor, potions
    pub stack_size: i32,
    /// Total item count in storage (sum across all chests)
    pub item_stock: i32,
    /// Total diamonds in the pair's reserve
    pub currency_stock: f64,
}

impl Pair {
    // Directory where all individual pair files will be stored.
    // One file per pair keeps diffs small and avoids rewriting the whole catalog on every update.
    const PAIRS_DIR: &str = "data/pairs";

    /// Number of slots in a shulker box (27 = 3 rows × 9 columns).
    /// Used as the unit of storage capacity since the store organizes stock in shulker boxes.
    pub const SHULKER_BOX_SLOTS: i32 = 27;
    
    /// Calculate the maximum item capacity of a shulker box for this item.
    /// 
    /// A shulker box has 27 slots, each holding up to `stack_size` items.
    /// Returns: 27 × stack_size
    #[allow(dead_code)]
    pub fn shulker_capacity(&self) -> i32 {
        Self::SHULKER_BOX_SLOTS * self.stack_size
    }
    
    /// Calculate shulker capacity for a given stack size.
    /// Use this when you don't have a Pair instance but know the stack size.
    pub fn shulker_capacity_for_stack_size(stack_size: i32) -> i32 {
        Self::SHULKER_BOX_SLOTS * stack_size
    }

    /// Sanitizes an item name for use in filenames.
    /// Removes "minecraft:" prefix and replaces colons with underscores.
    /// This ensures Windows compatibility (colons are not allowed in filenames).
    fn sanitize_item_name_for_filename(item_name: &str) -> String {
        let mut sanitized = item_name.to_string();
        
        // Remove "minecraft:" prefix if present
        if sanitized.starts_with("minecraft:") {
            sanitized = sanitized["minecraft:".len()..].to_string();
        }
        
        // Replace any remaining colons with underscores (for safety)
        sanitized = sanitized.replace(':', "_");
        
        sanitized
    }

    /// Builds the on-disk path for a pair file, applying filename sanitization
    /// so the same item name always maps to the same path regardless of whether
    /// the caller passes "minecraft:gunpowder" or "gunpowder".
    fn get_pair_file_path(item_name: &str) -> PathBuf {
        let sanitized_name = Self::sanitize_item_name_for_filename(item_name);
        PathBuf::from(Self::PAIRS_DIR).join(format!("{}.json", sanitized_name))
    }

    /// Loads a single `Pair` from `data/pairs/{item_name}.json`.
    /// Returns an `io::Error` with `ErrorKind::NotFound` if the file does not exist.
    #[allow(dead_code)]
    pub fn load(item_name: &str) -> io::Result<Self> {
        let path = Self::get_pair_file_path(item_name);

        if path.exists() {
            let json_str = fs::read_to_string(&path)?;
            let pair: Self = serde_json::from_str(&json_str)?;
            Ok(pair)
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Pair file not found: {}", path.display()),
            ))
        }
    }

    /// Saves this single `Pair` instance to `data/pairs/{self.item}.json`.
    /// Creates the 'data/pairs' directory if it doesn't exist.
    /// Returns an error if the item name is empty or invalid (e.g., "minecraft:").
    pub fn save(&self) -> io::Result<()> {
        // Guard against writing a pair with an unusable identifier: an empty name
        // would produce ".json", and a bare "minecraft:" prefix would sanitize to
        // an empty name, both of which would silently collide or corrupt storage.
        if self.item.trim().is_empty() || self.item == "minecraft:" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Cannot save pair with invalid item name: '{}'", self.item),
            ));
        }

        let path = Self::get_pair_file_path(&self.item);

        // Ensure the directory exists
        if let Some(parent_dir) = path.parent() {
            if !parent_dir.exists() {
                fs::create_dir_all(parent_dir)?;
            }
        }

        let json_str = serde_json::to_string_pretty(self)?; // Serialize the single Pair
        write_atomic(&path, &json_str)?;
        Ok(())
    }

    /// Loads all `Pair`s by reading every JSON file in the `data/pairs/` directory.
    /// It uses the internal deserialization logic for each file.
    /// Files that cannot be deserialized are skipped with a warning.
    /// If the directory does not exist, it returns an empty `HashMap<String, Pair>`.
    pub fn load_all() -> io::Result<HashMap<String, Self>> {
        let dir_path = Path::new(Self::PAIRS_DIR);
        let mut pairs = HashMap::new();

        if !dir_path.exists() {
            println!(
                "Pairs directory not found at {}. Returning an empty HashMap.",
                dir_path.display()
            );
            return Ok(HashMap::new());
        }

        for entry in fs::read_dir(dir_path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() && path.extension().is_some_and(|ext| ext == "json") {
                // Here, we can't directly call Pair::load because Pair::load expects an item_name
                // and attempts to read a file based on that. Instead, we read the file
                // and then deserialize it, which is the core logic of Pair::load.
                match fs::read_to_string(&path) {
                    Ok(json_str) => match serde_json::from_str::<Self>(&json_str) {
                        Ok(pair) => {
                            let item_name = pair.item.clone();
                            pairs.insert(item_name, pair);
                        }
                        Err(e) => eprintln!(
                            "Warning: Could not deserialize pair from {}: {}",
                            path.display(),
                            e
                        ),
                    },
                    Err(e) => eprintln!("Warning: Could not read file {}: {}", path.display(), e),
                }
            }
        }
        Ok(pairs)
    }

    /// Saves a HashMap of `Pair`s, where each `Pair` is saved to its own file
    /// in the `data/pairs/` directory using the `pair.save()` method.
    /// This method overwrites existing files and then removes any orphaned files.
    pub fn save_all(pairs: &HashMap<String, Self>) -> io::Result<()> {
        let dir_path = Path::new(Self::PAIRS_DIR);

        // Ensure the directory exists
        if !dir_path.exists() {
            fs::create_dir_all(dir_path)?;
        }

        // Track which filenames are still "live" so we can garbage-collect
        // any orphaned files below (pairs that existed on disk but were
        // removed from the in-memory map).
        let mut expected_files = HashSet::new();

        // Save each pair individually using the individual pair.save() method
        for pair in pairs.values() {
            pair.save()?;
            // Use sanitized filename for tracking expected files
            let sanitized_name = Self::sanitize_item_name_for_filename(&pair.item);
            let filename = format!("{}.json", sanitized_name);
            expected_files.insert(filename);
        }

        // Remove any files that shouldn't exist anymore
        if dir_path.exists() {
            for entry in fs::read_dir(dir_path)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_file() && path.extension().is_some_and(|ext| ext == "json") {
                    if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                        if !expected_files.contains(filename) {
                            fs::remove_file(&path)?;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
