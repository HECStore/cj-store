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
    path::{Path, PathBuf},
    sync::OnceLock,
};

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
/// or `..` content. Mirrors `crate::chat::tools::validate_uuid`'s acceptance
/// set; intentionally duplicated to keep `types::user` chat-independent.
fn is_valid_uuid_shape(uuid: &str) -> bool {
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
        let id = &mojang_response.id;
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
    pub fn save(&self) -> io::Result<()> {
        let path = Self::get_user_file_path(&self.uuid)?;

        if let Some(parent_dir) = path.parent()
            && !parent_dir.exists() {
                fs::create_dir_all(parent_dir)?;
            }

        let json_str = serde_json::to_string_pretty(self)?;
        write_atomic(&path, &json_str)?;
        debug!("[User] saved {} (balance={}, operator={})", self.uuid, self.balance, self.operator);
        Ok(())
    }

    /// Loads every `{uuid}.json` from the users directory. Malformed or
    /// unreadable files are skipped with a `warn!` log that includes the path.
    pub fn load_all() -> io::Result<HashMap<String, Self>> {
        let dir_path = Path::new(Self::USERS_DIR);
        let mut users = HashMap::new();

        if !dir_path.exists() {
            info!("[User] users directory not found at {}, starting empty", dir_path.display());
            return Ok(HashMap::new());
        }

        let mut skipped = 0usize;
        for entry in fs::read_dir(dir_path)? {
            let entry = entry?;
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
    /// in the `data/users/` directory using the `user.save()` method.
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
    pub fn save_dirty(users: &HashMap<String, Self>, dirty: &HashSet<String>) -> io::Result<()> {
        let dir_path = Path::new(Self::USERS_DIR);

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
                user.save()?;
                written += 1;
            }
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

        info!(
            "[User] save_dirty: wrote {} of {} users, cleaned {} orphan files (skipped {} with invalid uuid shape)",
            written, users.len(), removed, skipped_invalid
        );
        Ok(())
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
}
