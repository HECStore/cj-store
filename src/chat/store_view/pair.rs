//! Read-only view over `data/pairs/*.json`.
//!
//! Path safety: `get` has a single-file fast path that constructs
//! `data/pairs/{normalized}.json` directly using the same
//! name-to-filename mapping the trade bot uses (lowercase, strip
//! `minecraft:`, replace any remaining `:` with `_`). Before any
//! filesystem access the mapped stem is gated through a shape check
//! that is strictly lowercase ASCII `[a-z0-9_]` and non-empty,
//! intentionally stricter than [`crate::types::ItemId`] (which accepts
//! uppercase) because on-disk pair filenames are canonical lowercase.
//! Path-traversal bytes (`/`, `\`, `..`, NUL) are explicitly rejected.
//! On any shape rejection — or on `NotFound` / parse-error after the
//! rename-window retry — `get` falls back to scanning the whole
//! catalog and looking up by the in-memory `item` field, which is
//! itself normalized through [`crate::types::ItemId`] at write time.
//! A path-traversal item string therefore never reaches the filesystem
//! layer.

use std::collections::HashMap;

use serde::Deserialize;

/// Pairs catalog directory. Mirrors `crate::types::Pair::PAIRS_DIR`
/// but owned by chat.
pub const PAIRS_DIR: &str = "data/pairs";

/// Minimal deserializer for one pair JSON file. Skips orphan/internal
/// fields not needed by chat (`stack_size` is kept because it lets the
/// model answer "how many shulkers worth" follow-up questions).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PairView {
    pub item: String,
    pub stack_size: i32,
    pub item_stock: i32,
    pub currency_stock: f64,
}

/// Load every pair file in `data/pairs/`. Files that fail to
/// deserialize are skipped. Returns `item` -> `PairView`.
///
/// Wrapped in `spawn_blocking` because the chat task runs on a small
/// tokio worker pool; a 200-pair catalog scan is fast but still sync
/// I/O.
#[allow(dead_code)] // Public helper; current callers reach the same
// data through `get` / `get_in_dir`, but kept available for future
// chat features that want the whole map.
pub async fn load_all() -> Result<HashMap<String, PairView>, String> {
    tokio::task::spawn_blocking(|| load_all_in_dir(std::path::Path::new(PAIRS_DIR)))
        .await
        .map_err(|e| format!("load_all join: {e}"))?
}

/// Look up one pair by item name. Strips `minecraft:` prefix and
/// lowercases the rest, so any case form (`"diamond"`, `"Diamond"`,
/// `"minecraft:Diamond"`) hits the same entry.
///
/// Fast path: when the normalized (lowercased) name passes the shape
/// gate (strictly lowercase ASCII `[a-z0-9_]`, no `/`, `\`, `..`, or
/// NUL — see [`is_safe_pair_stem`]), reads `data/pairs/{stem}.json`
/// directly with a retry-on-rename loop, where `{stem}` mirrors the
/// trade bot's filename mapping (`:` -> `_`). On shape rejection, or
/// on `NotFound` / parse-error after both attempts, falls back to a
/// full catalog scan + map lookup so any future filename-sanitization
/// drift in the writer side still resolves correctly.
///
/// Returns `None` when the catalog doesn't contain the item OR the
/// underlying load fails.
pub async fn get(item: &str) -> Option<PairView> {
    get_in_dir(item, std::path::Path::new(PAIRS_DIR)).await
}

/// Inner async helper for [`get`], exposed at module scope so tests can
/// point at a temp dir. `dir` is used both for the single-file fast
/// path and for the full-catalog fallback scan.
pub(crate) async fn get_in_dir(item: &str, dir: &std::path::Path) -> Option<PairView> {
    // Lowercase up front: on-disk filenames are canonical lowercase
    // (the writer normalizes through `ItemId`), and the in-memory
    // `item` field used by the fallback `map.get(...)` is keyed by the
    // same lowercase form. A caller passing "Diamond" or
    // "minecraft:Diamond" must hit the same entry as "diamond".
    let normalized = item
        .strip_prefix("minecraft:")
        .unwrap_or(item)
        .to_ascii_lowercase();
    // Fast path: single-file read keyed by the trade-bot filename
    // mapping. `normalized` already had the `minecraft:` prefix
    // stripped above; the `:` -> `_` substitution mirrors
    // `crate::types::Pair::sanitize_item_name_for_filename` so this
    // path matches whatever the writer produced.
    let stem = normalized.replace(':', "_");
    if is_safe_pair_stem(&stem) {
        let dir_owned = dir.to_path_buf();
        let stem_for_blocking = stem.clone();
        let direct = tokio::task::spawn_blocking(move || {
            read_pair_file_with_retry(&dir_owned, &stem_for_blocking)
        })
        .await
        .ok()
        .flatten();
        if let Some(p) = direct {
            return Some(p);
        }
    }
    // Fallback: full catalog scan + map lookup. Preserved both for
    // shape-rejected inputs and for any drift between this fast path's
    // mapping and the writer's filename layout.
    let dir_owned = dir.to_path_buf();
    let map = tokio::task::spawn_blocking(move || load_all_in_dir(&dir_owned))
        .await
        .ok()?
        .ok()?;
    map.get(&normalized).cloned()
}

/// Shape gate for the single-file fast path. Strictly lowercase:
/// non-empty, ASCII `[a-z0-9_]` only. This is intentionally stricter
/// than [`crate::types::ItemId::new`] (which accepts uppercase),
/// because on-disk pair filenames are canonical lowercase — the writer
/// normalizes through `ItemId` before hitting the disk. Callers must
/// lowercase mixed-case input before computing the stem, otherwise
/// uppercase queries silently fall through to the catalog-scan
/// fallback. Path-traversal bytes (`/`, `\`, `..`, NUL) are explicitly
/// rejected; they are already excluded by the byte-class check, but
/// listing them keeps the intent obvious to future readers.
fn is_safe_pair_stem(stem: &str) -> bool {
    if stem.is_empty() {
        return false;
    }
    if stem.contains('/') || stem.contains('\\') || stem.contains("..") || stem.contains('\0') {
        return false;
    }
    stem.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Single-file read for the fast path. Uses the same retry-on-rename
/// pattern as [`crate::chat::store_view::user::get_by_uuid_in_dir`]:
/// the trade bot writes pair files via `write_atomic` (tmp + rename),
/// so a chat-side read can land between the unlink and the rename.
/// The retry is sleepless — by the second attempt the rename has
/// settled.
fn read_pair_file_with_retry(dir: &std::path::Path, stem: &str) -> Option<PairView> {
    let path = dir.join(format!("{stem}.json"));
    super::fsread::read_json_with_atomic_retry::<PairView>(&path)
}

/// Inner sync helper, exposed at module scope so tests can point at a
/// temp dir.
pub fn load_all_in_dir(dir: &std::path::Path) -> Result<HashMap<String, PairView>, String> {
    let mut out = HashMap::new();
    if !dir.exists() {
        return Ok(out);
    }
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read_dir pairs: {e}"))?;
    for ent in entries.flatten() {
        let path = ent.path();
        if !path.is_file() || path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        // One retry on NotFound or parse error to absorb the
        // write_atomic rename window — centralized in `fsread`.
        let Some(pair) = super::fsread::read_json_with_atomic_retry::<PairView>(&path) else {
            continue;
        };
        out.insert(pair.item.clone(), pair);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_dir(tag: &str) -> std::path::PathBuf {
        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "cj-store-pair-view-{}-{}-{tag}",
            std::process::id(),
            nanos,
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn load_all_round_trips_pair_json() {
        let dir = fixture_dir("round-trip");
        std::fs::write(
            dir.join("diamond.json"),
            r#"{"item":"diamond","stack_size":64,"item_stock":42,"currency_stock":120.5}"#,
        )
        .unwrap();
        let map = load_all_in_dir(&dir).unwrap();
        let p = map.get("diamond").unwrap();
        assert_eq!(p.stack_size, 64);
        assert_eq!(p.item_stock, 42);
        assert!((p.currency_stock - 120.5).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_all_skips_malformed_files() {
        let dir = fixture_dir("malformed");
        std::fs::write(
            dir.join("ok.json"),
            r#"{"item":"diamond","stack_size":64,"item_stock":42,"currency_stock":120.5}"#,
        )
        .unwrap();
        std::fs::write(dir.join("bad.json"), "not json").unwrap();
        let map = load_all_in_dir(&dir).unwrap();
        assert_eq!(map.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_all_returns_empty_for_missing_dir() {
        let dir = std::env::temp_dir().join(format!(
            "cj-store-pair-view-missing-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        ));
        let map = load_all_in_dir(&dir).unwrap();
        assert!(map.is_empty());
    }

    #[tokio::test]
    async fn get_returns_pair_for_uppercase_query() {
        let dir = fixture_dir("uppercase-query");
        // Fast-path target: lowercase on-disk filename, mirroring what
        // the writer produces.
        std::fs::write(
            dir.join("diamond.json"),
            r#"{"item":"diamond","stack_size":64,"item_stock":42,"currency_stock":120.5}"#,
        )
        .unwrap();
        // Second item to also exercise the catalog-fallback path with
        // a different filename.
        std::fs::write(
            dir.join("netherite_ingot.json"),
            r#"{"item":"netherite_ingot","stack_size":64,"item_stock":7,"currency_stock":3.5}"#,
        )
        .unwrap();

        // Plain uppercase form must resolve.
        let p = get_in_dir("Diamond", &dir).await.expect("Diamond resolves");
        assert_eq!(p.item, "diamond");
        assert_eq!(p.item_stock, 42);

        // `minecraft:`-prefixed uppercase form must also resolve.
        let p = get_in_dir("minecraft:Diamond", &dir)
            .await
            .expect("minecraft:Diamond resolves");
        assert_eq!(p.item, "diamond");
        assert_eq!(p.item_stock, 42);

        // Mixed case on a multi-segment item name.
        let p = get_in_dir("Netherite_Ingot", &dir)
            .await
            .expect("Netherite_Ingot resolves");
        assert_eq!(p.item, "netherite_ingot");
        assert_eq!(p.item_stock, 7);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
