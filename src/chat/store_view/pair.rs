//! Read-only view over `data/pairs/*.json`.
//!
//! Path safety: `get` does NOT construct `data/pairs/{user_input}.json`
//! from the chat-supplied item name. It loads the catalog and looks up
//! by the in-memory `item` field, which is normalized through
//! [`crate::types::ItemId`] at write time. A path-traversal item
//! string therefore never reaches the filesystem layer.

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
pub async fn load_all() -> Result<HashMap<String, PairView>, String> {
    tokio::task::spawn_blocking(|| load_all_in_dir(std::path::Path::new(PAIRS_DIR)))
        .await
        .map_err(|e| format!("load_all join: {e}"))?
}

/// Look up one pair by item name. Strips `minecraft:` prefix so either
/// form (`"diamond"` or `"minecraft:diamond"`) hits the same entry.
///
/// Returns `None` when the catalog doesn't contain the item OR the
/// underlying load fails.
pub async fn get(item: &str) -> Option<PairView> {
    let normalized = item.strip_prefix("minecraft:").unwrap_or(item).to_string();
    let map = load_all().await.ok()?;
    map.get(&normalized).cloned()
}

/// Inner sync helper, exposed at module scope so tests can point at a
/// temp dir.
pub fn load_all_in_dir(
    dir: &std::path::Path,
) -> Result<HashMap<String, PairView>, String> {
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
        // write_atomic rename window: the trade bot saves a pair via
        // tmp-file + rename, so a chat-side read can land between the
        // unlink and the rename. The retry is a sleepless re-read; on
        // a busy disk the rename has long completed by the second
        // attempt.
        let mut attempt = 0;
        let body = loop {
            match std::fs::read_to_string(&path) {
                Ok(b) => break Some(b),
                Err(_) if attempt == 0 => {
                    attempt += 1;
                    continue;
                }
                Err(_) => break None,
            }
        };
        let Some(body) = body else { continue };
        let pair: PairView = match serde_json::from_str(&body) {
            Ok(p) => p,
            Err(_) if attempt == 0 => {
                // Re-read once more in case we caught a half-written
                // file mid-rename.
                match std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|b| serde_json::from_str(&b).ok())
                {
                    Some(p) => p,
                    None => continue,
                }
            }
            Err(_) => continue,
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
}
