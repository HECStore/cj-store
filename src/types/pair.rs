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
    sync::atomic::AtomicU64,
};

use serde::{Deserialize, Serialize};

use crate::fsutil::{archive_aside, pick_archive_path, write_atomic};
use crate::types::ItemId;

use tracing::{info, warn};

/// Per-module monotonic counter appended to quarantine filenames so two
/// `.json.corrupt-*` archives produced in the same millisecond cannot collide
/// — the prior `fs::rename` + single `unix_ms` suffix would silently
/// overwrite the first archive on the second rename, destroying exactly the
/// forensic evidence quarantine exists to preserve. Mirrors the
/// `ARCHIVE_SEQ` pattern in `store::queue`, `store::journal`, and
/// `store::trade_state`.
static PAIR_ARCHIVE_SEQ: AtomicU64 = AtomicU64::new(0);

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
    ///
    /// Thin wrapper over `get_pair_file_path_in_dir` rooted at `PAIRS_DIR`;
    /// production callsites use this form, while the in-dir variant lets
    /// the `save_all_in_dir` test path target a `tempfile::tempdir()`.
    pub(crate) fn get_pair_file_path(item_name: &str) -> PathBuf {
        Self::get_pair_file_path_in_dir(Path::new(Self::PAIRS_DIR), item_name)
    }

    /// Directory-parameterized form of `get_pair_file_path`. Same sanitization
    /// rule, parameterized on `dir_path` so `save_all_in_dir` (and its tests)
    /// can route writes to a caller-supplied directory rather than the real
    /// `data/pairs/`.
    fn get_pair_file_path_in_dir(dir_path: &Path, item_name: &str) -> PathBuf {
        let sanitized_name = Self::sanitize_item_name_for_filename(item_name);
        dir_path.join(format!("{sanitized_name}.json"))
    }

    /// Saves this single `Pair` instance to `data/pairs/{self.item}.json`.
    /// Creates the 'data/pairs' directory if it doesn't exist.
    /// Returns an error if the item name is empty/sentinel or `stack_size` is
    /// not one of Minecraft's three legal values (1, 16, 64).
    ///
    /// Thin wrapper over `save_in_dir` rooted at `PAIRS_DIR`. Production
    /// callsites today reach the disk through `Pair::save_all` /
    /// `save_all_in_dir`, so the per-pair entry point is exercised only by
    /// tests — `#[allow(dead_code)]` keeps the symbol available for any
    /// future single-pair write path without spamming the warning channel.
    #[allow(dead_code)]
    pub fn save(&self) -> io::Result<()> {
        self.save_in_dir(Path::new(Self::PAIRS_DIR))
    }

    /// Directory-parameterized form of `save`. Same validation rules, just
    /// parameterized on `dir_path` so `save_all_in_dir` can thread its
    /// argument all the way through the per-pair write loop instead of
    /// silently bouncing back to `PAIRS_DIR` on every iteration.
    fn save_in_dir(&self, dir_path: &Path) -> io::Result<()> {
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
        // Defense-in-depth perimeter: `ItemId::from_normalized` does not
        // validate, so a caller that constructs an ItemId from a tampered
        // string could smuggle a path-separator or other unsafe byte through
        // the typed wrapper. Match the byte-class invariant that `ItemId::new`
        // enforces; on violation, refuse to write rather than let the bytes
        // reach `dir_path.join` and potentially escape the pairs directory.
        if !self
            .item
            .as_str()
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "Cannot save pair with non-canonical item bytes: {:?}",
                    self.item.as_str()
                ),
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

        let path = Self::get_pair_file_path_in_dir(dir_path, &self.item);

        if let Some(parent_dir) = path.parent()
            && !parent_dir.exists()
        {
            fs::create_dir_all(parent_dir)?;
        }

        let json_str = serde_json::to_string_pretty(self)?;
        write_atomic(&path, &json_str)?;
        tracing::debug!(
            "[Pair] saved '{}' (stack={}, item_stock={}, currency_stock={})",
            self.item.as_str(),
            self.stack_size,
            self.item_stock,
            self.currency_stock,
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
            info!(
                "[Pair] pairs directory not found at {}, starting empty",
                dir_path.display()
            );
            return Ok(HashMap::new());
        }

        let mut quarantined = 0usize;
        for entry in fs::read_dir(dir_path)? {
            // Per-entry IO errors (transient lock, deleted-during-iter, EACCES
            // on a single file) skip the entry rather than aborting the whole
            // load. The whole-directory `read_dir` failure above remains fatal.
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    quarantined += 1;
                    warn!("[Pair] skipping unreadable directory entry: {e}");
                    continue;
                }
            };
            let path = entry.path();

            if path.is_file() && path.extension().is_some_and(|ext| ext == "json") {
                match fs::read_to_string(&path) {
                    Ok(json_str) => match serde_json::from_str::<Self>(&json_str) {
                        Ok(pair) => {
                            // EMPTY-item pair files are unsalvageable: the
                            // ItemId deserializer maps `""` → ItemId::EMPTY,
                            // but `Pair::save_in_dir` errors on empty item,
                            // so inserting one would silently poison every
                            // future autosave (state::save reports failure;
                            // `self.dirty` never clears; shutdown drops every
                            // staged mutation). Quarantine here at the load
                            // boundary so the bad file is preserved as
                            // forensic evidence and the in-memory map stays
                            // clean.
                            if pair.item.is_empty() {
                                if let Err(e) = quarantine_pair_file(
                                    &path,
                                    "empty item: Pair requires a non-EMPTY ItemId",
                                ) {
                                    warn!(
                                        "[Pair] quarantine rename failed for {} (empty item): {e}; skipping insert",
                                        path.display()
                                    );
                                }
                                quarantined += 1;
                                continue;
                            }
                            // Defense-in-depth at the load boundary: require
                            // the embedded `item` to map back to the file
                            // stem. Without this, `diamond.json` could carry
                            // `"item": "cobblestone"` and win the duplicate-key
                            // race against the legitimate `cobblestone.json`,
                            // causing the legitimate file to be quarantined
                            // while the misnamed/tampered file wins.
                            //
                            // Case-insensitive on the stem: on case-insensitive
                            // filesystems (Windows, default macOS APFS), the
                            // file `Diamond.json` is the same path as
                            // `diamond.json` but the stem reads back as
                            // `"Diamond"`. An operator hand-editing a pair file
                            // shouldn't lose stock to byte-equal stem-mismatch.
                            let expected_stem = Self::sanitize_item_name_for_filename(&pair.item);
                            let stem = path
                                .file_stem()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            if !stem.eq_ignore_ascii_case(&expected_stem) {
                                if let Err(e) = quarantine_pair_file(
                                    &path,
                                    &format!(
                                        "stem mismatch: file stem {stem:?} vs expected {expected_stem:?} from item {:?}",
                                        pair.item.as_str()
                                    ),
                                ) {
                                    warn!(
                                        "[Pair] quarantine rename failed for {} (stem mismatch): {e}; skipping insert",
                                        path.display()
                                    );
                                }
                                quarantined += 1;
                                continue;
                            }
                            let item_name = pair.item.to_string();
                            // Two on-disk files mapping to the same `pair.item`
                            // would silently overwrite each other in memory and
                            // then `save_all`'s orphan-cleanup would delete the
                            // loser as a stale `.json`. Quarantine the second
                            // file (`*.json.dup`) so an operator can reconcile.
                            if pairs.contains_key(&item_name) {
                                if let Err(e) = quarantine_pair_file(
                                    &path,
                                    &format!("duplicate key '{item_name}' already loaded"),
                                ) {
                                    warn!(
                                        "[Pair] quarantine rename failed for {} (duplicate key): {e}; skipping insert",
                                        path.display()
                                    );
                                }
                                quarantined += 1;
                            } else {
                                pairs.insert(item_name, pair);
                            }
                        }
                        Err(e) => {
                            if let Err(qe) = quarantine_pair_file(&path, &format!("malformed: {e}"))
                            {
                                warn!(
                                    "[Pair] quarantine rename failed for {} (malformed): {qe}",
                                    path.display()
                                );
                            }
                            quarantined += 1;
                        }
                    },
                    Err(e) => {
                        if let Err(qe) = quarantine_pair_file(&path, &format!("unreadable: {e}")) {
                            warn!(
                                "[Pair] quarantine rename failed for {} (unreadable): {qe}",
                                path.display()
                            );
                        }
                        quarantined += 1;
                    }
                }
            }
        }
        info!(
            "[Pair] loaded {} pairs (quarantined {})",
            pairs.len(),
            quarantined
        );
        Ok(pairs)
    }

    /// Saves a HashMap of `Pair`s, where each `Pair` is saved to its own file
    /// in the `data/pairs/` directory using the `pair.save_in_dir()` method.
    /// This method overwrites existing files and then removes any orphaned files.
    pub fn save_all(pairs: &HashMap<String, Self>) -> io::Result<()> {
        Self::save_all_in_dir(pairs, Path::new(Self::PAIRS_DIR))
    }

    /// Directory-parameterized form of `save_all`. The empty-map guard lives
    /// here (not just in the public wrapper) so tests can exercise the
    /// wipe-refusal invariant directly against a temp dir; the public
    /// `save_all` is a thin one-liner over this helper.
    fn save_all_in_dir(pairs: &HashMap<String, Self>, dir_path: &Path) -> io::Result<()> {
        // Refuse an empty map only when there are real `.json` files on disk
        // that the orphan sweep below would actually wipe. A fresh install
        // (no pairs dir, or an empty/stub pairs dir) is a legitimate no-op:
        // the setup-phase autosave runs before the operator has created the
        // first pair, and erroring here would block the entire dirty-flag
        // chain (`state::save` aggregates sub-save errors first-error-keep-
        // going and surfaces the first to the caller; the autosave loop
        // therefore never clears `self.dirty`, and a shutdown then loses
        // every staged mutation). Once any pair exists on disk, an empty
        // in-memory map is still treated as "refuse to wipe".
        if pairs.is_empty() {
            let dir_has_pair_files = match fs::read_dir(dir_path) {
                Ok(read_dir) => read_dir.filter_map(|entry| entry.ok()).any(|entry| {
                    let path = entry.path();
                    path.is_file() && path.extension().is_some_and(|ext| ext == "json")
                }),
                Err(e) if e.kind() == io::ErrorKind::NotFound => false,
                Err(e) => return Err(e),
            };
            if dir_has_pair_files {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "save_all called with an empty pairs map but on-disk pair files exist; refusing to wipe the pairs directory",
                ));
            }
            return Ok(());
        }

        if !dir_path.exists() {
            fs::create_dir_all(dir_path)?;
        }

        // Track which filenames are still "live" so we can garbage-collect
        // files for pairs removed from the in-memory map below.
        let mut expected_files = HashSet::new();
        let mut written = 0usize;
        let mut first_save_err: Option<io::Error> = None;

        for pair in pairs.values() {
            let sanitized_name = Self::sanitize_item_name_for_filename(&pair.item);
            let filename = format!("{sanitized_name}.json");
            // Populate `expected_files` regardless of save outcome so the
            // orphan sweep below still runs against the full intended map —
            // a transient write failure must not turn legitimate files into
            // sweep targets.
            expected_files.insert(filename);
            // Attempt every pair even after a previous failure: each
            // `write_atomic` is independent, so one transient hiccup must
            // not silently drop later pairs' updates. Capture only the
            // first error to surface to the caller.
            if let Err(e) = pair.save_in_dir(dir_path) {
                warn!("[Pair] save failed for {}: {e}", pair.item.as_str());
                first_save_err.get_or_insert(e);
            } else {
                written += 1;
            }
        }

        // Orphan sweep: warn-and-continue on per-entry IO errors so a single
        // locked/transient failure doesn't abort the whole sweep, and so a
        // captured `first_save_err` always wins over a sweep-only error
        // (stale orphans self-heal next cycle; a swallowed save error makes
        // callers think state was persisted when it wasn't).
        let mut removed = 0usize;
        let mut first_sweep_err: Option<io::Error> = None;
        if dir_path.exists() {
            match fs::read_dir(dir_path) {
                Ok(read_dir) => {
                    for entry in read_dir {
                        let entry = match entry {
                            Ok(e) => e,
                            Err(e) => {
                                warn!("[Pair] orphan sweep: unreadable entry: {e}");
                                first_sweep_err.get_or_insert(e);
                                continue;
                            }
                        };
                        let path = entry.path();
                        if path.is_file()
                            && path.extension().is_some_and(|ext| ext == "json")
                            && let Some(filename) = path.file_name().and_then(|n| n.to_str())
                            && !expected_files.contains(filename)
                        {
                            if let Err(e) = fs::remove_file(&path) {
                                warn!(
                                    "[Pair] orphan sweep: remove_file({}) failed: {e}",
                                    path.display()
                                );
                                first_sweep_err.get_or_insert(e);
                            } else {
                                removed += 1;
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "[Pair] orphan sweep: read_dir({}) failed: {e}",
                        dir_path.display()
                    );
                    first_sweep_err = Some(e);
                }
            }
        }

        info!(
            "[Pair] save_all: wrote {} of {} pairs (failed {}), cleaned {} orphan files",
            written,
            pairs.len(),
            pairs.len() - written,
            removed
        );
        match first_save_err.or(first_sweep_err) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// Rename a malformed/unreadable pair file aside so the next `save_all`
/// orphan-cleanup cannot delete it (extension is no longer `.json`) and
/// subsequent `load_all` calls do not retry deserializing it.
///
/// Uses [`pick_archive_path`] + [`archive_aside`] (the same primitives
/// `store::journal` / `store::queue` / `store::trade_state` use) rather than
/// a raw `fs::rename` with a single `unix_ms` suffix: two corrupt files in
/// the same millisecond would otherwise collide and the second `fs::rename`
/// would silently overwrite the first archive — destroying exactly the
/// forensic evidence quarantine exists to preserve. `archive_aside` also
/// supplies a `fs::copy + fs::remove_file` fallback for Windows-AV
/// held-handle scenarios that `fs::rename` alone cannot handle.
fn quarantine_pair_file(path: &Path, reason: &str) -> io::Result<()> {
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "pair.json".to_string());
    let archived = pick_archive_path(path.parent(), &base, "corrupt", &PAIR_ARCHIVE_SEQ)?;
    warn!(
        "[Pair] quarantining {} ({}): renaming to {}",
        path.display(),
        reason,
        archived.display(),
    );
    archive_aside(path, &archived)
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
        assert_eq!(
            Pair::sanitize_item_name_for_filename("minecraft:diamond"),
            "diamond"
        );
    }

    #[test]
    fn sanitize_replaces_remaining_colons_after_prefix_strip() {
        // "minecraft:something:odd" -> strip "minecraft:" -> "something:odd" -> "something_odd"
        assert_eq!(
            Pair::sanitize_item_name_for_filename("minecraft:something:odd"),
            "something_odd"
        );
    }

    #[test]
    fn sanitize_bare_minecraft_prefix_produces_empty_name() {
        // This is the edge case save() guards against at the Pair boundary.
        assert_eq!(Pair::sanitize_item_name_for_filename("minecraft:"), "");
    }

    #[test]
    fn sanitize_plain_name_passes_through() {
        assert_eq!(
            Pair::sanitize_item_name_for_filename("cobblestone"),
            "cobblestone"
        );
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
        // Pre-populate a `*.json` file; an empty `pairs` map paired with
        // on-disk pair files must NOT trigger the orphan sweep that would
        // wipe them.
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("cobblestone.json");
        fs::write(&f, "{}").unwrap();

        let err = Pair::save_all_in_dir(&HashMap::new(), dir.path())
            .expect_err("empty map paired with on-disk pair file must error");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("empty pairs map"));
        assert!(f.exists(), "pre-existing pair file must survive");
    }

    #[test]
    fn save_all_with_empty_map_and_empty_dir_is_noop() {
        // Fresh install: no pairs dir / empty pairs dir + empty in-memory
        // map must be a no-op `Ok(())`, not an `InvalidInput` error. Erring
        // here would block the setup-phase autosave (the dirty flag never
        // clears, and a shutdown drops every staged mutation).
        let parent = tempfile::tempdir().unwrap();

        // (i) Missing directory: an empty map must succeed without creating
        //     the directory (the no-op path returns before `create_dir_all`).
        let missing = parent.path().join("does_not_exist");
        Pair::save_all_in_dir(&HashMap::new(), &missing)
            .expect("empty map + missing dir must be a no-op");
        assert!(!missing.exists(), "no-op must not create the dir");

        // (ii) Existing but empty directory: also a no-op.
        let empty = parent.path().join("empty_pairs");
        fs::create_dir_all(&empty).unwrap();
        Pair::save_all_in_dir(&HashMap::new(), &empty)
            .expect("empty map + empty dir must be a no-op");

        // (iii) Existing dir with only non-`.json` siblings: still a no-op
        //       (the guard only fires on real `.json` files the sweep would wipe).
        let with_sibling = parent.path().join("with_sibling");
        fs::create_dir_all(&with_sibling).unwrap();
        fs::write(with_sibling.join("README.txt"), "not a pair file").unwrap();
        Pair::save_all_in_dir(&HashMap::new(), &with_sibling)
            .expect("empty map + non-json siblings must be a no-op");
        assert!(
            with_sibling.join("README.txt").exists(),
            "non-json sibling must survive"
        );
    }

    #[test]
    fn save_all_in_dir_threads_dir_through_writes_and_orphan_sweep() {
        // End-to-end check that `dir_path` is honored everywhere — both the
        // per-pair write loop and the orphan sweep. Before the in-dir thread
        // landed, the per-pair `pair.save()` call routed through `PAIRS_DIR`
        // regardless of the threaded argument, so a non-empty in-memory map
        // would leak writes into the real `data/pairs/` directory and the
        // orphan sweep would wipe unrelated files there.
        let dir = tempfile::tempdir().unwrap();

        // (a) Pre-existing stale `.json` in the temp dir — the orphan sweep
        //     must remove it because no in-memory pair matches its name.
        let stale = dir.path().join("stale_orphan.json");
        fs::write(&stale, "{}").unwrap();

        // (b) Pre-existing stale `.json` in the REAL `data/pairs/` dir — the
        //     sweep must NOT touch it; otherwise the threaded `dir_path` is
        //     a lie. Only seed this guard if the production dir already
        //     exists, so a clean checkout running tests doesn't suddenly
        //     manifest a `data/pairs/` directory just to satisfy this test.
        let real_pairs = Path::new(Pair::PAIRS_DIR);
        let real_pairs_existed = real_pairs.exists();
        let real_guard = real_pairs.join(".pair_threading_test_guard.json");
        let mut real_guard_seeded = false;
        if real_pairs_existed {
            // Skip seeding if the guard somehow already exists from a prior
            // crashed run — leave it alone rather than racing.
            if !real_guard.exists() && fs::write(&real_guard, "{}").is_ok() {
                real_guard_seeded = true;
            }
        }

        // (c) One legitimate pair in the in-memory map — must land in
        //     `dir_path`, not in `data/pairs/`.
        let mut pairs = HashMap::new();
        let item = ItemId::new("cobblestone").expect("valid item id");
        pairs.insert(
            item.to_string(),
            Pair {
                item: item.clone(),
                stack_size: 64,
                item_stock: 1,
                currency_stock: 0.0,
            },
        );

        Pair::save_all_in_dir(&pairs, dir.path()).expect("save_all_in_dir must succeed");

        // The pair file landed in the threaded dir, not in `data/pairs/`.
        let written = dir.path().join("cobblestone.json");
        assert!(written.exists(), "pair file must land in the threaded dir");
        let leaked = real_pairs.join("cobblestone.json");
        // If `data/pairs/cobblestone.json` already existed before the test
        // (a real running shop), we can't make a strong assertion. Otherwise
        // it must not have been created here.
        if !real_pairs_existed {
            assert!(!leaked.exists(), "pair file must not leak into data/pairs/");
        }

        // The orphan sweep removed the temp-dir stale file …
        assert!(
            !stale.exists(),
            "orphan sweep must remove stale .json from threaded dir"
        );

        // … but did NOT touch the real `data/pairs/` guard file.
        if real_guard_seeded {
            assert!(
                real_guard.exists(),
                "orphan sweep must not touch files in data/pairs/ when threaded to a temp dir"
            );
            // Cleanup so subsequent runs / unrelated tests aren't affected.
            let _ = fs::remove_file(&real_guard);
        }
    }
}
