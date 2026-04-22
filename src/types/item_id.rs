//! # Type-safe item identifiers
//!
//! `ItemId` wraps a `String` that has been normalized (no `minecraft:` prefix,
//! non-empty). All item-referencing fields (`Pair::item`, `Chest::item`,
//! `Order::item`, `Trade::item`, etc.) use `ItemId` instead of raw `String`,
//! so normalization bugs become compile-time errors.
//!
//! ## Serialization
//!
//! `#[serde(transparent)]` means the JSON representation is a bare string —
//! no wrapper object — so existing `data/*.json` files are fully compatible.

use std::borrow::Borrow;
use std::fmt;

use serde::{Deserialize, Serialize};

/// A normalized, non-empty item identifier.
///
/// Constructed via [`ItemId::new`] which strips any `minecraft:` prefix and
/// rejects empty strings. The inner value is prefix-free (e.g. `"cobblestone"`,
/// not `"minecraft:cobblestone"`). Case is preserved as given — Minecraft item
/// IDs are lowercase by convention but this type does not enforce casing.
///
/// Implements `Deref<Target = str>` so it can be passed to any function
/// expecting `&str` via deref coercion, and `Borrow<str>` so it works as a
/// `HashMap<String, _>` lookup key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ItemId(String);

/// Sentinel value for an unassigned chest slot.
///
/// Pre-ItemId code used `""` (empty string) to mean "no item assigned".
/// `ItemId::EMPTY` preserves this convention in a discoverable constant.
/// It intentionally bypasses the `new()` non-empty check because it is
/// a domain-level sentinel, not a user-supplied value.
impl ItemId {
    pub const EMPTY: ItemId = ItemId(String::new());

    /// Create a new `ItemId`, normalizing the `minecraft:` prefix.
    ///
    /// Returns `Err` if the resulting identifier is empty (e.g. bare
    /// `"minecraft:"` or `""`).
    pub fn new(raw: &str) -> Result<Self, &'static str> {
        let normalized = raw.strip_prefix("minecraft:").unwrap_or(raw);
        if normalized.is_empty() {
            return Err("empty item ID");
        }
        Ok(Self(normalized.to_string()))
    }

    /// Build an `ItemId` from a string that is already known to be
    /// normalized. No prefix stripping or validation is performed.
    ///
    /// Prefer [`new`](Self::new) for user/external input.
    pub fn from_normalized(s: String) -> Self {
        Self(s)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Return the Minecraft-namespaced form (e.g. `"minecraft:cobblestone"`).
    pub fn with_minecraft_prefix(&self) -> String {
        format!("minecraft:{}", self.0)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Trait impls for ergonomic usage
// ---------------------------------------------------------------------------

impl std::ops::Deref for ItemId {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for ItemId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ItemId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl PartialEq<str> for ItemId {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for ItemId {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<String> for ItemId {
    fn eq(&self, other: &String) -> bool {
        self.0 == *other
    }
}

impl From<ItemId> for String {
    fn from(id: ItemId) -> Self {
        id.0
    }
}

impl Default for ItemId {
    fn default() -> Self {
        Self::EMPTY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_minecraft_prefix() {
        let id = ItemId::new("minecraft:diamond").unwrap();
        assert_eq!(id.as_str(), "diamond");
    }

    #[test]
    fn no_prefix_passthrough() {
        let id = ItemId::new("cobblestone").unwrap();
        assert_eq!(id.as_str(), "cobblestone");
    }

    #[test]
    fn rejects_empty() {
        assert!(ItemId::new("").is_err());
        assert!(ItemId::new("minecraft:").is_err());
    }

    #[test]
    fn with_prefix() {
        let id = ItemId::new("gunpowder").unwrap();
        assert_eq!(id.with_minecraft_prefix(), "minecraft:gunpowder");
    }

    #[test]
    fn serde_transparent_roundtrip() {
        let id = ItemId::new("iron_ingot").unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"iron_ingot\"");
        let back: ItemId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn empty_sentinel() {
        assert!(ItemId::EMPTY.is_empty());
        let json = serde_json::to_string(&ItemId::EMPTY).unwrap();
        assert_eq!(json, "\"\"");
    }

    #[test]
    fn partial_eq_str() {
        let id = ItemId::new("diamond").unwrap();
        assert!(id == "diamond");
        assert!(id == *"diamond");
    }

    #[test]
    fn deref_coercion() {
        let id = ItemId::new("emerald").unwrap();
        fn takes_str(_: &str) {}
        takes_str(&id);
    }

    #[test]
    fn hashmap_lookup() {
        let mut map = std::collections::HashMap::new();
        map.insert("gold_ingot".to_string(), 42);
        let id = ItemId::new("gold_ingot").unwrap();
        assert_eq!(map.get(id.as_str()), Some(&42));
    }
}
