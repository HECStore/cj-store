//! # User Management
//!
//! Handles user persistence and Mojang UUID lookup.
//! Each user is stored as a separate JSON file: `data/users/{uuid}.json`
//!
//! ## Key Features
//! - UUID-based identity (canonical key, survives username changes)
//! - Diamond balance tracking (f64 for fractional diamonds)
//! - Operator flag for privileged commands (additem, removeitem, addcurrency, removecurrency)
//!
//! ## Mojang API Integration
//! - `get_uuid_async()` calls Mojang's public API to resolve usernames to UUIDs
//! - Returns hyphenated UUID format: `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`
//! - Caching (TTL = `UUID_CACHE_TTL_SECS`) is handled in `crate::mojang::resolve_user_uuid`

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::Path,
    sync::OnceLock,
};
#[cfg(test)]
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use tracing::{debug, info, warn};

use crate::fsutil::write_atomic;

// The Mojang lookup path is gated behind `#[cfg(not(test))]` so tests don't
// issue real HTTP requests. The supporting HTTP client, the request struct,
// and `get_uuid_async` therefore only have callers outside test builds — the
// cfg_attr below silences the test-only dead_code warnings without allowing
// dead code in the production build.

/// Hard timeout for a single Mojang UUID lookup. Referenced in the error
/// message so a tuning change cannot make the log lie.
const MOJANG_TIMEOUT_SECS: u64 = 10;

/// Process-wide reqwest client for Mojang API calls. Singleton so connection
/// pooling amortizes TLS handshakes across lookups for the lifetime of the bot.
#[cfg_attr(test, allow(dead_code))]
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

#[cfg_attr(test, allow(dead_code))]
fn get_http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(MOJANG_TIMEOUT_SECS))
            .connect_timeout(std::time::Duration::from_secs(3))
            .build()
            .expect("Failed to create HTTP client")
    })
}

/// Represents a user in the store system.
///
/// **Persistence**: Saved to `data/users/{uuid}.json`
///
/// **Identity**: UUID is the canonical identifier (survives username changes).
/// Username is updated on each interaction but is not used as a key.
///
/// **Balance**: Stored as `f64` to support fractional diamonds (e.g., from sell orders
/// where bot offers whole diamonds but player receives fractional credit).
///
/// **Operator**: When `true`, user can execute privileged commands:
/// - `additem <item> <quantity>` - Add items to storage
/// - `removeitem <item> <quantity>` - Remove items from storage
/// - `addcurrency <item> <amount>` - Add diamonds to pair reserve
/// - `removecurrency <item> <amount>` - Remove diamonds from pair reserve
///
/// See `README.md` "Player command interface" for command details.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize, Clone)]
pub struct User {
    /// Hyphenated Mojang UUID — canonical identifier (survives username changes).
    pub uuid: String,
    /// Last-seen username; ephemeral, not an identity key.
    pub username: String,
    pub balance: f64,
    /// `#[serde(default)]` so users from pre-operator-flag saves deserialize
    /// as non-operators instead of failing the whole load.
    #[serde(default)]
    pub operator: bool,
}

#[cfg_attr(test, allow(dead_code))]
#[derive(Deserialize)]
struct MojangResponse {
    id: String,
}

/// Mojang-shape username validator (3-16 chars, ASCII alphanumeric + `_`).
/// Mirrors `crate::chat::tools::validate_username_shape`; intentionally
/// duplicated here so this module has no dependency on `chat::*`. The two
/// must stay in sync — both enforce the in-game protocol's username rules.
fn is_valid_username_shape(username: &str) -> bool {
    (3..=16).contains(&username.len())
        && username
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// UUID-shape validator: canonical 36-char hyphenated lowercase hex
/// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`, OR bare 32-char lowercase hex.
/// Rejects uppercase, missing hyphens, wrong length, and any path-separator
/// or `..` content. Strictly broader than the chat-tool boundary
/// validator at `crate::chat::tools::validate_uuid`, which now accepts
/// canonical-hyphenated only — the storage layer keeps the bare-hex
/// arm because Mojang's API returns the bare form and existing on-disk
/// user files were historically written with it. Intentionally
/// duplicated to keep `types::user` chat-independent.
pub(crate) fn is_valid_uuid_shape(uuid: &str) -> bool {
    let bytes = uuid.as_bytes();
    match bytes.len() {
        32 => bytes
            .iter()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        36 => {
            let hyphen_positions = [8usize, 13, 18, 23];
            for (i, &b) in bytes.iter().enumerate() {
                let expect_hyphen = hyphen_positions.contains(&i);
                if expect_hyphen {
                    if b != b'-' {
                        return false;
                    }
                } else if !(b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
                    return false;
                }
            }
            true
        }
        _ => false,
    }
}

impl User {
    // Directory where all individual user files will be stored
    const USERS_DIR: &str = "data/users";

    /// Resolves a Minecraft username to a hyphenated Mojang UUID via
    /// `https://api.mojang.com/users/profiles/minecraft/{username}`.
    /// HTTP 204 → player not found; other non-2xx or network errors → `Err`.
    /// Rejects out-of-shape usernames before constructing the URL so a
    /// malformed name (slash, query, fragment, control char) cannot escape
    /// the path component into an attacker-chosen Mojang endpoint.
    #[cfg_attr(test, allow(dead_code))]
    pub async fn get_uuid_async(username: &str) -> Result<String, String> {
        if !is_valid_username_shape(username) {
            warn!("[Mojang] rejecting out-of-shape username '{username}' before URL build");
            return Err(format!(
                "Invalid Minecraft username '{username}' (must be 3-16 chars, ASCII alphanumeric or _)"
            ));
        }
        let url = format!("https://api.mojang.com/users/profiles/minecraft/{username}");

        let client = get_http_client();
        let response = client.get(&url).send().await.map_err(|e| {
            if e.is_timeout() {
                warn!("[Mojang] timeout after {MOJANG_TIMEOUT_SECS}s resolving '{username}'");
                format!("Mojang API timeout after {MOJANG_TIMEOUT_SECS}s for username '{username}'")
            } else if e.is_connect() {
                warn!("[Mojang] connect failed resolving '{username}': {e}");
                format!("Failed to connect to Mojang API: {e}")
            } else {
                warn!("[Mojang] request failed resolving '{username}': {e}");
                format!("Mojang API request failed: {e}")
            }
        })?;

        if response.status() == reqwest::StatusCode::NO_CONTENT {
            debug!("[Mojang] username '{username}' not found (204)");
            return Err(format!("Player '{username}' not found"));
        }

        if !response.status().is_success() {
            let status = response.status();
            warn!("[Mojang] non-success resolving '{username}': HTTP {status}");
            return Err(format!("Mojang API error for '{username}': {status}"));
        }

        let mojang_response: MojangResponse = response.json().await.map_err(|e| {
            warn!("[Mojang] malformed response for '{username}': {e}");
            format!("Failed to parse Mojang API response for '{username}': {e}")
        })?;

        // Mojang returns a bare-hex UUID; hyphenate to the canonical
        // xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx form. Both guards are needed:
        // length alone counts BYTES, not chars, so a 32-byte response with
        // any non-ASCII multi-byte char would slice on a non-char-boundary
        // and panic. Requiring every byte to be ASCII hex makes the slice
        // offsets land on valid char boundaries AND prevents non-hex content
        // (which would later become a filename component) from propagating.
        // The persistence layer's `is_valid_uuid_shape` is lowercase-only,
        // so canonicalize here — Mojang doesn't contractually guarantee a
        // case, and an uppercase response would otherwise survive this guard
        // (`is_ascii_hexdigit` accepts A-F) and then be silently rejected by
        // `save_in_dir`/`load_all_in_dir`, dropping the user on next start.
        let id = mojang_response.id.to_ascii_lowercase();
        if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            warn!(
                "[Mojang] invalid UUID shape for '{username}': got {:?}",
                id
            );
            return Err(format!("Invalid UUID from Mojang API: {id}"));
        }
        let formatted = format!(
            "{}-{}-{}-{}-{}",
            &id[0..8],
            &id[8..12],
            &id[12..16],
            &id[16..20],
            &id[20..32]
        );

        debug!("[Mojang] resolved '{username}' -> {formatted}");
        Ok(formatted)
    }

    /// Build the on-disk path for a user file. Validates `uuid` shape so a
    /// tampered or legacy `User.uuid` (e.g. `"../foo"`) cannot turn the next
    /// `save_dirty` cycle into a write-anywhere primitive — `save_dirty`
    /// builds expected filenames from `user.uuid` verbatim and orphan-deletes
    /// any unmatched `.json` in the users directory, so a malformed value
    /// would both write outside the directory AND wipe legitimate files.
    ///
    /// Production code now goes through `save_in_dir`, which inlines the
    /// same shape gate; this helper is retained for the shape-gate tests
    /// and is therefore `#[cfg(test)]`-gated to keep the production binary
    /// free of dead code.
    #[cfg(test)]
    fn get_user_file_path(uuid: &str) -> io::Result<PathBuf> {
        if !is_valid_uuid_shape(uuid) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Invalid user UUID shape: {uuid:?}"),
            ));
        }
        Ok(PathBuf::from(Self::USERS_DIR).join(format!("{uuid}.json")))
    }

    /// Saves this single `User` to `data/users/{self.uuid}.json`, creating
    /// the directory if needed. Uses `write_atomic` so a crash mid-write
    /// cannot leave a partially written user file.
    ///
    /// Retained as a one-liner over `save_in_dir` for symmetry with the
    /// other `Type::save` methods on the storage types (and as a future
    /// callsite if a single-user write path is needed). Production code
    /// reaches the same logic through
    /// `save_dirty` → `save_dirty_in_dir` → `save_in_dir`, so this wrapper
    /// has no live callers — `#[allow(dead_code)]` keeps it available
    /// without a warning.
    #[allow(dead_code)]
    pub fn save(&self) -> io::Result<()> {
        self.save_in_dir(Path::new(Self::USERS_DIR))
    }

    /// Directory-parameterized form of `save`. Validates the embedded uuid
    /// shape before joining it to `dir` so a tampered `self.uuid` cannot
    /// escape the supplied directory. Tests target this directly with a
    /// `tempfile::tempdir()` to exercise the persistence path without
    /// touching `data/users/`.
    fn save_in_dir(&self, dir: &Path) -> io::Result<()> {
        if !is_valid_uuid_shape(&self.uuid) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Invalid user UUID shape: {:?}", self.uuid),
            ));
        }
        let path = dir.join(format!("{}.json", self.uuid));

        if !dir.exists() {
            fs::create_dir_all(dir)?;
        }

        let json_str = serde_json::to_string_pretty(self)?;
        write_atomic(&path, &json_str)?;
        debug!("[User] saved {} (balance={}, operator={})", self.uuid, self.balance, self.operator);
        Ok(())
    }

    /// Loads every `{uuid}.json` from the users directory. Malformed or
    /// unreadable files are skipped with a `warn!` log that includes the path.
    pub fn load_all() -> io::Result<HashMap<String, Self>> {
        Self::load_all_in_dir(Path::new(Self::USERS_DIR))
    }

    /// Directory-parameterized form of `load_all`. Same skip-and-quarantine
    /// rules; tests target this directly with a `tempfile::tempdir()` to
    /// pin the malformed-entry guards without touching `data/users/`.
    fn load_all_in_dir(dir_path: &Path) -> io::Result<HashMap<String, Self>> {
        let mut users = HashMap::new();

        if !dir_path.exists() {
            info!("[User] users directory not found at {}, starting empty", dir_path.display());
            return Ok(HashMap::new());
        }

        let mut skipped = 0usize;
        for entry in fs::read_dir(dir_path)? {
            // Per-entry IO errors (transient lock, deleted-during-iter, EACCES
            // on a single file) skip the entry rather than aborting the whole
            // load. The whole-directory `read_dir` failure above remains fatal.
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    skipped += 1;
                    warn!("[User] skipping unreadable directory entry: {e}");
                    continue;
                }
            };
            let path = entry.path();

            if path.is_file() && path.extension().is_some_and(|ext| ext == "json") {
                match fs::read_to_string(&path) {
                    Ok(json_str) => match serde_json::from_str::<Self>(&json_str) {
                        Ok(user) => {
                            // Defense-in-depth at the load boundary: require
                            // the embedded `uuid` matches a hex-UUID shape
                            // AND equals the file stem. Without this, a
                            // tampered file `foo.json` could carry
                            // `"uuid": "../bar"` and on the next save_dirty
                            // cycle (a) write to an attacker-chosen path and
                            // (b) sweep away every legitimate user file as
                            // an "orphan" (their canonical filenames don't
                            // match the malformed `expected_files` set).
                            if !is_valid_uuid_shape(&user.uuid) {
                                skipped += 1;
                                warn!(
                                    "[User] skipping {}: embedded uuid {:?} fails shape check",
                                    path.display(), user.uuid
                                );
                                continue;
                            }
                            let stem = path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("");
                            if stem != user.uuid {
                                skipped += 1;
                                warn!(
                                    "[User] skipping {}: filename stem {:?} does not match embedded uuid {:?}",
                                    path.display(), stem, user.uuid
                                );
                                continue;
                            }
                            let uuid = user.uuid.clone();
                            users.insert(uuid, user);
                        }
                        Err(e) => {
                            skipped += 1;
                            warn!("[User] skipping malformed {}: {e}", path.display());
                        }
                    },
                    Err(e) => {
                        skipped += 1;
                        warn!("[User] skipping unreadable {}: {e}", path.display());
                    }
                }
            }
        }
        info!("[User] loaded {} users (skipped {})", users.len(), skipped);
        Ok(users)
    }

    /// Saves a HashMap of `User`s, where each `User` is saved to its own file
    /// in the `data/users/` directory through the shared `save_dirty` →
    /// `save_dirty_in_dir` → `save_in_dir` chain.
    /// This method overwrites existing files and then removes any orphaned files.
    ///
    /// The orphan cleanup pass makes the on-disk directory a faithful mirror
    /// of the in-memory map: users removed from the map are also deleted from
    /// disk, preventing stale state from being resurrected by `load_all`.
    ///
    /// Thin wrapper around `save_dirty` that treats every user as dirty;
    /// used at shutdown and for the audit-repair path where the full state
    /// must be flushed. Hot paths (per-order autosave) should use
    /// `save_dirty` with a tracked dirty-set instead, to avoid O(N) fsyncs
    /// per trade.
    #[allow(dead_code)]
    pub fn save_all(users: &HashMap<String, Self>) -> io::Result<()> {
        let all_keys: HashSet<String> = users.keys().cloned().collect();
        Self::save_dirty(users, &all_keys)
    }

    /// Saves only the `User`s whose UUIDs appear in `dirty`, then runs the
    /// orphan-cleanup pass against `users`' current keys so the on-disk
    /// directory still mirrors the in-memory map.
    ///
    /// Skips persisting users that are in `dirty` but no longer present in
    /// `users` — they'll be removed by the orphan sweep below.
    ///
    /// Refuses to operate on an empty `users` map *only when the on-disk users
    /// directory still has `.json` files that the orphan sweep would wipe* —
    /// mirroring `Pair::save_all` and `Trade::save_all`. Empty-map + empty/
    /// missing dir is a no-op `Ok(())` so fresh-install autosaves are not
    /// blocked before the first user lands. A bug that empties the in-memory
    /// map AFTER users have been persisted still fails loud rather than
    /// silently zapping balances and operator flags.
    ///
    /// On a write failure, still completes population of `expected_files` for
    /// the remaining shape-valid users and runs the orphan sweep before
    /// returning the captured error — this preserves the "directory mirrors
    /// map" invariant and prevents stale on-disk files for users dropped from
    /// the map from accumulating across save attempts.
    pub fn save_dirty(users: &HashMap<String, Self>, dirty: &HashSet<String>) -> io::Result<()> {
        Self::save_dirty_in_dir(users, dirty, Path::new(Self::USERS_DIR))
    }

    /// Directory-parameterized form of `save_dirty`. The empty-map guard
    /// lives here (not just in the public wrapper) so tests can exercise
    /// the wipe-refusal invariant directly against a temp dir; the public
    /// `save_dirty` is a thin one-liner over this helper.
    fn save_dirty_in_dir(
        users: &HashMap<String, Self>,
        dirty: &HashSet<String>,
        dir_path: &Path,
    ) -> io::Result<()> {
        // Refuse an empty map only when there are real `.json` files on disk
        // that the orphan sweep below would actually wipe. A fresh install
        // (no users dir, or an empty/stub users dir) is a legitimate no-op:
        // the setup-phase autosave runs before any user has been seen
        // (operator-only flows like `addnode`/`addpair` set `store.dirty`
        // without populating `store.users`), and erroring here would block
        // the entire dirty-flag chain (`state::save` propagates via `?`,
        // the autosave loop never clears `self.dirty`, and a shutdown then
        // loses every staged mutation). Once any user file exists on disk,
        // an empty in-memory map is still treated as "refuse to wipe".
        if users.is_empty() {
            let dir_has_user_files = match fs::read_dir(dir_path) {
                Ok(read_dir) => read_dir
                    .filter_map(|entry| entry.ok())
                    .any(|entry| {
                        let path = entry.path();
                        path.is_file() && path.extension().is_some_and(|ext| ext == "json")
                    }),
                Err(e) if e.kind() == io::ErrorKind::NotFound => false,
                Err(e) => return Err(e),
            };
            if dir_has_user_files {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "save_dirty called with an empty users map but on-disk user files exist; refusing to wipe the users directory",
                ));
            }
            return Ok(());
        }

        if !dir_path.exists() {
            fs::create_dir_all(dir_path)?;
        }

        // Filenames are keyed on `user.uuid` (not the HashMap key) so on-disk
        // files always match the canonical identity inside the User struct,
        // even if the two ever diverge. Skip any user whose `uuid` fails the
        // shape check — including their malformed filename in `expected_files`
        // would also exempt that path from the orphan sweep, defeating the
        // sweep's "directory mirrors map" contract.
        let mut expected_files = HashSet::with_capacity(users.len());
        let mut written = 0usize;
        let mut skipped_invalid = 0usize;
        let mut first_save_err: Option<io::Error> = None;

        let mut attempted = 0usize;
        for (key, user) in users.iter() {
            if !is_valid_uuid_shape(&user.uuid) {
                warn!(
                    "[User] skipping save for {:?} (key {:?}): uuid fails shape check",
                    user.uuid, key
                );
                skipped_invalid += 1;
                continue;
            }
            let filename = format!("{}.json", user.uuid);
            expected_files.insert(filename);
            if dirty.contains(key) {
                attempted += 1;
                // Attempt every dirty user even after a previous failure: each
                // `write_atomic` is independent, so one transient hiccup must
                // not silently drop later users' balance/operator updates.
                // Capture only the first error to surface to the caller.
                if let Err(e) = user.save_in_dir(dir_path) {
                    warn!("[User] save failed for {}: {e}", user.uuid);
                    if first_save_err.is_none() {
                        first_save_err = Some(e);
                    }
                } else {
                    written += 1;
                }
            }
        }

        // Wipe-refusal: if every user in the map failed the shape gate,
        // `expected_files` is empty and the orphan sweep below would delete
        // every legitimate `.json`. Match the explicit empty-map guard above:
        // only refuse when there are real `.json` files on disk to wipe; a
        // fresh / empty users dir is a legitimate no-op so the autosave
        // chain isn't broken by an all-shape-invalid in-memory map.
        if expected_files.is_empty() {
            let dir_has_user_files = match fs::read_dir(dir_path) {
                Ok(read_dir) => read_dir
                    .filter_map(|entry| entry.ok())
                    .any(|entry| {
                        let path = entry.path();
                        path.is_file() && path.extension().is_some_and(|ext| ext == "json")
                    }),
                Err(e) if e.kind() == io::ErrorKind::NotFound => false,
                Err(e) => return Err(e),
            };
            if dir_has_user_files {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "save_dirty: no shape-valid users to mirror but on-disk user files exist; refusing to wipe the users directory",
                ));
            }
            return Ok(());
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
                                warn!("[User] orphan sweep: unreadable entry: {e}");
                                if first_sweep_err.is_none() {
                                    first_sweep_err = Some(e);
                                }
                                continue;
                            }
                        };
                        let path = entry.path();
                        if path.is_file() && path.extension().is_some_and(|ext| ext == "json")
                            && let Some(filename) = path.file_name().and_then(|n| n.to_str())
                            && !expected_files.contains(filename)
                        {
                            if let Err(e) = fs::remove_file(&path) {
                                warn!("[User] orphan sweep: remove_file({}) failed: {e}", path.display());
                                if first_sweep_err.is_none() {
                                    first_sweep_err = Some(e);
                                }
                            } else {
                                removed += 1;
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("[User] orphan sweep: read_dir({}) failed: {e}", dir_path.display());
                    first_sweep_err = Some(e);
                }
            }
        }

        info!(
            "[User] save_dirty: wrote {} of {} attempted (failed {}), cleaned {} orphan files, skipped {} with invalid uuid shape ({} users in map)",
            written, attempted, attempted - written, removed, skipped_invalid, users.len()
        );
        match first_save_err.or(first_sweep_err) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_file_path_uses_uuid_with_json_extension() {
        let p = User::get_user_file_path("550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert!(p.ends_with("550e8400-e29b-41d4-a716-446655440000.json"));
    }

    #[test]
    fn user_file_path_rejects_path_traversal_and_separators() {
        for bad in [
            "../etc/passwd",
            "..\\windows\\system32",
            "/abs/path",
            "550e8400/../escape",
            "UPPER-CASE-IS-NOT-CANONICAL-LOWERCASE",
            "",
            "tooshort",
        ] {
            let err = User::get_user_file_path(bad).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "uuid {bad:?}");
        }
    }

    #[test]
    fn user_file_path_accepts_canonical_and_bare_hex_uuids() {
        // Canonical hyphenated, lowercase.
        assert!(User::get_user_file_path("550e8400-e29b-41d4-a716-446655440000").is_ok());
        // Bare 32-char lowercase hex.
        assert!(User::get_user_file_path("550e8400e29b41d4a716446655440000").is_ok());
    }

    #[test]
    fn serde_round_trip_preserves_all_fields() {
        let u = User {
            uuid: "uuid-1".into(),
            username: "alice".into(),
            balance: 42.5,
            operator: true,
        };
        let json = serde_json::to_string(&u).unwrap();
        let back: User = serde_json::from_str(&json).unwrap();
        assert_eq!(back, u);
    }

    #[test]
    fn operator_defaults_to_false_for_pre_flag_files() {
        // Older saves predate the `operator` field. `#[serde(default)]` must
        // keep them loading cleanly.
        let json = r#"{"uuid":"u","username":"a","balance":1.0}"#;
        let u: User = serde_json::from_str(json).unwrap();
        assert!(!u.operator);
    }

    #[test]
    fn save_dirty_refuses_empty_map_to_prevent_accidental_wipe() {
        // Pre-create two `*.json` files; an empty `users` map must NOT
        // trigger the orphan sweep that would wipe them.
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.json");
        let f2 = dir.path().join("11111111-2222-3333-4444-555555555555.json");
        fs::write(&f1, "{}").unwrap();
        fs::write(&f2, "{}").unwrap();

        let err = User::save_dirty_in_dir(&HashMap::new(), &HashSet::new(), dir.path())
            .expect_err("empty map must error");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(f1.exists(), "pre-existing file 1 must survive");
        assert!(f2.exists(), "pre-existing file 2 must survive");
    }

    #[test]
    fn save_dirty_empty_map_with_empty_dir_is_ok() {
        // Setup-phase autosave: operator-only flows (addnode/addpair/...) flip
        // `store.dirty` without populating `store.users`. With no user files
        // on disk yet, the empty-map guard must return Ok(()) so the dirty
        // flag can clear and the autosave chain isn't broken.

        // Case 1: dir exists but is empty.
        let dir = tempfile::tempdir().unwrap();
        User::save_dirty_in_dir(&HashMap::new(), &HashSet::new(), dir.path())
            .expect("empty map + empty dir must be Ok");

        // Case 2: dir is missing entirely (fresh install).
        let parent = tempfile::tempdir().unwrap();
        let missing = parent.path().join("nonexistent-users");
        assert!(!missing.exists());
        User::save_dirty_in_dir(&HashMap::new(), &HashSet::new(), &missing)
            .expect("empty map + missing dir must be Ok");

        // Case 3: dir has only a non-`.json` sibling file — still no wipe risk.
        let dir3 = tempfile::tempdir().unwrap();
        fs::write(dir3.path().join("README.txt"), "ignore me").unwrap();
        User::save_dirty_in_dir(&HashMap::new(), &HashSet::new(), dir3.path())
            .expect("empty map + dir with no .json files must be Ok");
    }

    #[test]
    fn load_all_in_dir_drops_file_with_tampered_embedded_uuid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.json");
        let json = r#"{"uuid":"../bar","username":"alice","balance":1.0}"#;
        fs::write(&path, json).unwrap();

        let users = User::load_all_in_dir(dir.path()).unwrap();
        assert!(users.is_empty(), "tampered file must be dropped");
        assert!(path.exists(), "load_all does NOT delete; file must remain");
    }

    #[test]
    fn load_all_in_dir_drops_file_with_stem_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.json");
        // Embedded uuid is a different valid canonical UUID than the stem.
        let json = r#"{"uuid":"11111111-2222-3333-4444-555555555555","username":"alice","balance":1.0}"#;
        fs::write(&path, json).unwrap();

        let users = User::load_all_in_dir(dir.path()).unwrap();
        assert!(users.is_empty(), "stem-mismatched file must be dropped");
    }

    #[test]
    fn load_all_in_dir_loads_well_formed_file() {
        let dir = tempfile::tempdir().unwrap();
        let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let path = dir.path().join(format!("{uuid}.json"));
        let user = User {
            uuid: uuid.to_string(),
            username: "alice".to_string(),
            balance: 7.5,
            operator: true,
        };
        fs::write(&path, serde_json::to_string(&user).unwrap()).unwrap();

        let users = User::load_all_in_dir(dir.path()).unwrap();
        assert_eq!(users.len(), 1);
        let loaded = users.get(uuid).expect("keyed by embedded uuid");
        assert_eq!(loaded, &user);
    }

    #[test]
    fn save_dirty_in_dir_skips_user_with_invalid_uuid_shape() {
        // Mount the temp dir under a parent so we can assert no file
        // escaped via the malformed `../etc/passwd` uuid into the parent.
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("users");
        fs::create_dir_all(&dir).unwrap();

        let valid_uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let valid = User {
            uuid: valid_uuid.to_string(),
            username: "alice".to_string(),
            balance: 1.0,
            operator: false,
        };
        let bogus = User {
            uuid: "../etc/passwd".to_string(),
            username: "mallory".to_string(),
            balance: 0.0,
            operator: false,
        };

        let mut users = HashMap::new();
        users.insert(valid_uuid.to_string(), valid);
        users.insert("bogus".to_string(), bogus);

        let mut dirty = HashSet::new();
        dirty.insert(valid_uuid.to_string());
        dirty.insert("bogus".to_string());

        User::save_dirty_in_dir(&users, &dirty, &dir).unwrap();

        // (i) only the valid user's `.json` exists in `dir`.
        let valid_path = dir.join(format!("{valid_uuid}.json"));
        assert!(valid_path.exists(), "valid user must be persisted");
        let entries: Vec<_> = fs::read_dir(&dir).unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        assert_eq!(entries.len(), 1, "only one file expected, got {entries:?}");

        // (ii) no file escaped the directory via the malformed uuid.
        assert!(
            !parent.path().join("etc").exists(),
            "no `etc/` sibling should appear next to dir"
        );
        assert!(
            !parent.path().join("etc/passwd.json").exists(),
            "no escape file outside dir"
        );
    }
}
