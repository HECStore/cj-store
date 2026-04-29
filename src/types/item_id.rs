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

use serde::{Deserialize, Deserializer, Serialize};

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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
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
    /// `"minecraft:"` or `""`), or if it contains any character that is not
    /// ASCII alphanumeric or `_`. This rejects path traversal (`..`, `/`,
    /// `\`), control characters, and Unicode lookalikes such as Cyrillic `с`
    /// that visually resemble Latin letters.
    pub fn new(raw: &str) -> Result<Self, &'static str> {
        let normalized = raw.strip_prefix("minecraft:").unwrap_or(raw);
        if normalized.is_empty() {
            return Err("empty item ID");
        }
        if !normalized.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err("item id contains forbidden character");
        }
        Ok(Self(normalized.to_string()))
    }

    /// Build an `ItemId` from a string that is already known to be
    /// normalized (no `minecraft:` prefix). No prefix stripping is performed.
    ///
    /// # Panics (debug builds only)
    ///
    /// Panics if `s` is empty, because an empty `ItemId` is never valid outside
    /// the [`EMPTY`](Self::EMPTY) sentinel. Use [`new`](Self::new) for
    /// user/external input.
    pub fn from_normalized(s: String) -> Self {
        debug_assert!(!s.is_empty(), "ItemId::from_normalized called with empty string");
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

/// Custom `Deserialize` that routes through [`ItemId::new`] so the
/// normalization invariant (prefix-stripped, non-empty, ASCII-alphanumeric +
/// `_` only) holds for values loaded from JSON. The empty-string sentinel
/// [`ItemId::EMPTY`] is preserved as a special case so existing on-disk data
/// with empty `item` fields (unassigned chest slots) continues to load.
impl<'de> Deserialize<'de> for ItemId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        if raw.is_empty() {
            return Ok(ItemId::EMPTY);
        }
        ItemId::new(&raw).map_err(serde::de::Error::custom)
    }
}

/// `default()` returns [`EMPTY`](ItemId::EMPTY), the sentinel for unassigned
/// slots — it is NOT a valid item ID. Callers needing a real ID must use
/// [`ItemId::new`].
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

    #[test]
    fn borrow_str_enables_hashmap_lookup_with_itemid_ref() {
        // Borrow<str> + Hash/Eq consistency is what lets &ItemId look up a
        // String-keyed map. Regression guard: if Borrow<str> is removed or
        // the hash/eq contract diverges, this fails.
        use std::borrow::Borrow;
        let mut map: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        map.insert("iron_ingot".to_string(), 7);
        let id = ItemId::new("iron_ingot").unwrap();
        let key: &str = (&id).borrow();
        assert_eq!(map.get(key), Some(&7));
    }

    #[test]
    fn partial_eq_compares_normalized_form_only() {
        // ItemId("minecraft:diamond") is stored as "diamond", so comparing
        // against the prefixed literal is false. This documents the asymmetry
        // and guards against a future "fix" that would change the semantics.
        let id = ItemId::new("minecraft:diamond").unwrap();
        assert!(id == "diamond");
        assert!(id != "minecraft:diamond");
    }

    #[test]
    fn default_is_empty_sentinel() {
        let id = ItemId::default();
        assert!(id.is_empty());
        assert_eq!(id, ItemId::EMPTY);
    }

    #[test]
    fn rejects_path_traversal_dotdot() {
        assert!(ItemId::new("..").is_err());
        assert!(ItemId::new("../etc").is_err());
    }

    #[test]
    fn rejects_forward_slash() {
        assert!(ItemId::new("foo/bar").is_err());
        assert!(ItemId::new("/").is_err());
    }

    #[test]
    fn rejects_backslash() {
        assert!(ItemId::new("foo\\bar").is_err());
        assert!(ItemId::new("\\").is_err());
    }

    #[test]
    fn rejects_leading_dot() {
        assert!(ItemId::new(".hidden").is_err());
        assert!(ItemId::new(".").is_err());
    }

    #[test]
    fn rejects_control_char() {
        assert!(ItemId::new("\x00").is_err());
        assert!(ItemId::new("foo\x00bar").is_err());
    }

    #[test]
    fn rejects_cyrillic_lookalike() {
        // Cyrillic "с" (U+0441) looks like Latin "c" but is multi-byte UTF-8.
        // Must be rejected to prevent homograph confusion.
        assert!(ItemId::new("\u{0441}obblestone").is_err());
        assert!(ItemId::new("\u{0441}").is_err());
    }

    #[test]
    fn deserialize_rejects_path_traversal() {
        let result: Result<ItemId, _> = serde_json::from_str("\"../etc\"");
        assert!(result.is_err(), "deserializing \"../etc\" must be Err");
    }

    #[test]
    fn deserialize_routes_through_new() {
        // Various forbidden inputs must all fail at the deserialization layer.
        assert!(serde_json::from_str::<ItemId>("\"foo/bar\"").is_err());
        assert!(serde_json::from_str::<ItemId>("\"foo\\\\bar\"").is_err());
        assert!(serde_json::from_str::<ItemId>("\".hidden\"").is_err());
        assert!(serde_json::from_str::<ItemId>("\"\u{0441}obblestone\"").is_err());
    }

    #[test]
    fn deserialize_strips_minecraft_prefix() {
        // The custom impl must still go through new(), which strips the prefix.
        let id: ItemId = serde_json::from_str("\"minecraft:diamond\"").unwrap();
        assert_eq!(id.as_str(), "diamond");
    }

    #[test]
    fn deserialize_preserves_empty_sentinel() {
        // Existing on-disk data uses "" for unassigned chest slots.
        let id: ItemId = serde_json::from_str("\"\"").unwrap();
        assert_eq!(id, ItemId::EMPTY);
    }

    #[test]
    fn accepts_real_item_ids() {
        // Sample of items currently in on-disk data.
        for raw in [
            "cobblestone",
            "iron_ingot",
            "diamond",
            "gold_ingot",
            "gunpowder",
            "stone",
            "music_disc_11",
            "overflow",
        ] {
            assert!(ItemId::new(raw).is_ok(), "should accept real item id: {raw}");
        }
    }
}
