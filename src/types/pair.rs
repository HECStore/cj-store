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
use crate::types::ItemId;

use tracing::{info, warn};

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
    pub item: ItemId,
    /// Maximum stack size for this item — 64 (default), 16 (ender pearls, eggs,
    /// snowballs, signs, banners, buckets), or 1 (tools, weapons, armor, potions).
    /// Drives per-shulker capacity, so a wrong value under-reports storage.
    pub stack_size: i32,
    pub item_stock: i32,
    /// Reserve of the base currency (diamonds).
    pub currency_stock: f64,
}

impl Pair {
    // One file per pair keeps diffs small and avoids rewriting the catalog on every update.
    const PAIRS_DIR: &str = "data/pairs";

    pub fn shulker_capacity_for_stack_size(stack_size: i32) -> i32 {
        crate::constants::SHULKER_BOX_SLOTS as i32 * stack_size
    }

    /// Normalize an item name for use as a filename: strip the `minecraft:`
    /// prefix, then replace any remaining colons (reserved on NTFS) with `_`.
    fn sanitize_item_name_for_filename(item_name: &str) -> String {
        let mut sanitized = item_name.to_string();
        if sanitized.starts_with("minecraft:") {
            sanitized = sanitized["minecraft:".len()..].to_string();
        }
        sanitized.replace(':', "_")
    }

    /// Builds the on-disk path for a pair file, applying filename sanitization
    /// so the same item name always maps to the same path regardless of whether
    /// the caller passes "minecraft:gunpowder" or "gunpowder".
    pub(crate) fn get_pair_file_path(item_name: &str) -> PathBuf {
        let sanitized_name = Self::sanitize_item_name_for_filename(item_name);
        PathBuf::from(Self::PAIRS_DIR).join(format!("{sanitized_name}.json"))
    }

    /// Saves this single `Pair` instance to `data/pairs/{self.item}.json`.
    /// Creates the 'data/pairs' directory if it doesn't exist.
    /// Returns an error if the item name is empty/sentinel or `stack_size` is
    /// not one of Minecraft's three legal values (1, 16, 64).
    pub fn save(&self) -> io::Result<()> {
        // Guard against writing a pair with an unusable identifier: the
        // EMPTY sentinel sanitizes to ".json" and would silently collide or
        // corrupt storage. `ItemId::new` strips the "minecraft:" prefix and
        // rejects any colon, so a non-EMPTY ItemId-validated value cannot
        // smuggle in a bare "minecraft:" — the EMPTY check is the only
        // remaining failure mode.
        if self.item.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Cannot save pair with empty/sentinel item name",
            ));
        }
        // Minecraft only has three legal stack sizes (1, 16, 64). A pair
        // persisted with any other value (e.g. the `Default` of 0, or a
        // hand-edited 32) silently breaks `shulker_capacity_for_stack_size`
        // and downstream chest planning even though the file looks valid.
        if !matches!(self.stack_size, 1 | 16 | 64) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "Cannot save pair '{}' with invalid stack_size {} (must be 1, 16, or 64)",
                    self.item, self.stack_size
                ),
            ));
        }

        let path = Self::get_pair_file_path(&self.item);

        if let Some(parent_dir) = path.parent()
            && !parent_dir.exists() {
                fs::create_dir_all(parent_dir)?;
            }

        let json_str = serde_json::to_string_pretty(self)?;
        write_atomic(&path, &json_str)?;
        tracing::debug!(
            "[Pair] saved '{}' (stack={}, item_stock={}, currency_stock={})",
            self.item.as_str(), self.stack_size, self.item_stock, self.currency_stock,
        );
        Ok(())
    }

    /// Loads all `Pair`s by reading every JSON file in the `data/pairs/` directory.
    /// It uses the internal deserialization logic for each file.
    /// Files that cannot be deserialized are quarantined by renaming them to
    /// `*.json.corrupt` so the next `save_all` orphan-cleanup pass cannot
    /// silently delete them, and so subsequent `load_all` calls won't retry.
    /// If the directory does not exist, it returns an empty `HashMap<String, Pair>`.
    pub fn load_all() -> io::Result<HashMap<String, Self>> {
        let dir_path = Path::new(Self::PAIRS_DIR);
        let mut pairs = HashMap::new();

        if !dir_path.exists() {
            info!("[Pair] pairs directory not found at {}, starting empty", dir_path.display());
            return Ok(HashMap::new());
        }

        let mut quarantined = 0usize;
        for entry in fs::read_dir(dir_path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() && path.extension().is_some_and(|ext| ext == "json") {
                match fs::read_to_string(&path) {
                    Ok(json_str) => match serde_json::from_str::<Self>(&json_str) {
                        Ok(pair) => {
                            let item_name = pair.item.to_string();
                            // Two on-disk files mapping to the same `pair.item`
                            // would silently overwrite each other in memory and
                            // then `save_all`'s orphan-cleanup would delete the
                            // loser as a stale `.json`. Quarantine the second
                            // file (`*.json.dup`) so an operator can reconcile.
                            if pairs.contains_key(&item_name) {
                                quarantine_pair_file(
                                    &path,
                                    &format!("duplicate key '{item_name}' already loaded"),
                                )?;
                                quarantined += 1;
                            } else {
                                pairs.insert(item_name, pair);
                            }
                        }
                        Err(e) => {
                            quarantine_pair_file(&path, &format!("malformed: {e}"))?;
                            quarantined += 1;
                        }
                    },
                    Err(e) => {
                        quarantine_pair_file(&path, &format!("unreadable: {e}"))?;
                        quarantined += 1;
                    }
                }
            }
        }
        info!("[Pair] loaded {} pairs (quarantined {})", pairs.len(), quarantined);
        Ok(pairs)
    }

    /// Saves a HashMap of `Pair`s, where each `Pair` is saved to its own file
    /// in the `data/pairs/` directory using the `pair.save()` method.
    /// This method overwrites existing files and then removes any orphaned files.
    pub fn save_all(pairs: &HashMap<String, Self>) -> io::Result<()> {
        // Refuse to proceed with an empty map — the orphan-cleanup loop below
        // would otherwise wipe every pair file. The base-currency `diamond`
        // pair is invariantly retained, so an empty `pairs` is never legitimate.
        if pairs.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "save_all called with an empty pairs map; refusing to wipe the pairs directory",
            ));
        }

        let dir_path = Path::new(Self::PAIRS_DIR);

        if !dir_path.exists() {
            fs::create_dir_all(dir_path)?;
        }

        // Track which filenames are still "live" so we can garbage-collect
        // files for pairs removed from the in-memory map below.
        let mut expected_files = HashSet::new();

        for pair in pairs.values() {
            pair.save()?;
            let sanitized_name = Self::sanitize_item_name_for_filename(&pair.item);
            let filename = format!("{sanitized_name}.json");
            expected_files.insert(filename);
        }

        let mut removed = 0usize;
        if dir_path.exists() {
            for entry in fs::read_dir(dir_path)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_file() && path.extension().is_some_and(|ext| ext == "json")
                    && let Some(filename) = path.file_name().and_then(|n| n.to_str())
                        && !expected_files.contains(filename) {
                            fs::remove_file(&path)?;
                            removed += 1;
                        }
            }
        }

        info!("[Pair] save_all: wrote {} pairs, cleaned {} orphan files", pairs.len(), removed);
        Ok(())
    }
}

/// Rename a malformed/unreadable pair file to `*.json.corrupt.<millis>` so the
/// next `save_all` orphan-cleanup cannot delete it (extension is no longer
/// `.json`) and subsequent `load_all` calls do not retry deserializing it.
/// The millisecond timestamp suffix avoids collisions if quarantine fires
/// repeatedly for the same path.
fn quarantine_pair_file(path: &Path, reason: &str) -> io::Result<()> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let target = path.with_extension(format!("json.corrupt.{ts}"));
    warn!(
        "[Pair] quarantining {} ({}): renaming to {}",
        path.display(),
        reason,
        target.display(),
    );
    fs::rename(path, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shulker_capacity_scales_linearly_with_stack_size() {
        let s = crate::constants::SHULKER_BOX_SLOTS as i32;
        assert_eq!(Pair::shulker_capacity_for_stack_size(64), s * 64);
        assert_eq!(Pair::shulker_capacity_for_stack_size(16), s * 16);
        assert_eq!(Pair::shulker_capacity_for_stack_size(1), s);
        assert_eq!(Pair::shulker_capacity_for_stack_size(0), 0);
    }

    #[test]
    fn sanitize_strips_minecraft_prefix() {
        assert_eq!(Pair::sanitize_item_name_for_filename("minecraft:diamond"), "diamond");
    }

    #[test]
    fn sanitize_replaces_remaining_colons_after_prefix_strip() {
        // "minecraft:something:odd" -> strip "minecraft:" -> "something:odd" -> "something_odd"
        assert_eq!(Pair::sanitize_item_name_for_filename("minecraft:something:odd"), "something_odd");
    }

    #[test]
    fn sanitize_bare_minecraft_prefix_produces_empty_name() {
        // This is the edge case save() guards against at the Pair boundary.
        assert_eq!(Pair::sanitize_item_name_for_filename("minecraft:"), "");
    }

    #[test]
    fn sanitize_plain_name_passes_through() {
        assert_eq!(Pair::sanitize_item_name_for_filename("cobblestone"), "cobblestone");
    }

    #[test]
    fn get_pair_file_path_is_stable_under_prefix() {
        let with = Pair::get_pair_file_path("minecraft:diamond");
        let without = Pair::get_pair_file_path("diamond");
        assert_eq!(with, without);
    }

    #[test]
    fn save_rejects_empty_and_bare_prefix_item_names() {
        // An empty or bare-prefix name would sanitize to ".json", which would
        // silently collide across pairs.
        let mut p = Pair::default();
        p.item = ItemId::EMPTY;
        assert_eq!(p.save().unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn save_all_refuses_empty_map_to_prevent_accidental_wipe() {
        let err = Pair::save_all(&HashMap::new()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("empty pairs map"));
    }
}
