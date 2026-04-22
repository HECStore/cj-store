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
//! - Caching (TTL = `UUID_CACHE_TTL_SECS`) is handled in `store::utils::resolve_user_uuid`

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use serde::{Deserialize, Serialize};

use crate::fsutil::write_atomic;

// The Mojang lookup path is gated behind `#[cfg(not(test))]` so tests don't
// issue real HTTP requests. The supporting HTTP client, the request struct,
// and `get_uuid_async` therefore only have callers outside test builds — the
// cfg_attr below silences the test-only dead_code warnings without allowing
// dead code in the production build.

/// Global async HTTP client for Mojang API calls.
/// Using a single client enables connection pooling and better performance.
#[cfg_attr(test, allow(dead_code))]
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

#[cfg_attr(test, allow(dead_code))]
fn get_http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
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
    /// Hyphenated Mojang UUID (canonical identifier)
    pub uuid: String,
    /// Last-seen username (updated on each interaction, can change)
    pub username: String,
    /// Diamond balance (f64 for fractional support)
    pub balance: f64,
    /// Operator flag: enables privileged commands (additem, removeitem, addcurrency, removecurrency)
    #[serde(default)]
    pub operator: bool,
}

#[cfg_attr(test, allow(dead_code))]
#[derive(Deserialize)]
struct MojangResponse {
    id: String,
}

impl User {
    // Directory where all individual user files will be stored
    const USERS_DIR: &str = "data/users";

    /// Resolves a Minecraft username to a Mojang UUID via the public API (async version).
    ///
    /// **API Endpoint**: `https://api.mojang.com/users/profiles/minecraft/{username}`
    ///
    /// **Returns**: Hyphenated UUID string (e.g., `550e8400-e29b-41d4-a716-446655440000`)
    ///
    /// **Error Cases**:
    /// - HTTP 204: Player not found (username doesn't exist)
    /// - Other HTTP errors: API failure
    /// - Network errors: Connection issues
    /// - Timeout: Request took longer than 10 seconds
    ///
    #[cfg_attr(test, allow(dead_code))]
    pub async fn get_uuid_async(username: &str) -> Result<String, String> {
        let url = format!(
            "https://api.mojang.com/users/profiles/minecraft/{}",
            username
        );

        let client = get_http_client();
        let response = client.get(&url).send().await.map_err(|e| {
            if e.is_timeout() {
                format!("Mojang API timeout after 10s for username '{}'", username)
            } else if e.is_connect() {
                format!("Failed to connect to Mojang API: {}", e)
            } else {
                format!("Mojang API request failed: {}", e)
            }
        })?;

        // Mojang API returns 204 No Content when player doesn't exist
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Err(format!("Player '{}' not found", username));
        }

        if !response.status().is_success() {
            return Err(format!("Mojang API error for '{}': {}", username, response.status()));
        }

        let mojang_response: MojangResponse = response.json().await.map_err(|e| {
            format!("Failed to parse Mojang API response for '{}': {}", username, e)
        })?;

        // Mojang API returns UUID without hyphens; format it with hyphens
        // Format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
        // The length check is a guard against malformed responses before slicing,
        // which would otherwise panic on non-32-char strings.
        let id = &mojang_response.id;
        if id.len() != 32 {
            return Err(format!("Invalid UUID length from Mojang API: {}", id));
        }
        let formatted = format!(
            "{}-{}-{}-{}-{}",
            &id[0..8],
            &id[8..12],
            &id[12..16],
            &id[16..20],
            &id[20..32]
        );

        Ok(formatted)
    }

    // Helper function to get the file path for a single user
    fn get_user_file_path(uuid: &str) -> PathBuf {
        PathBuf::from(Self::USERS_DIR).join(format!("{}.json", uuid))
    }

    /// Saves this single `User` instance to `data/users/{self.uuid}.json`.
    /// Creates the 'data/users' directory if it doesn't exist.
    ///
    /// Uses `write_atomic` (temp file + rename) so a crash mid-write cannot
    /// leave a partially written user file that would fail to deserialize.
    pub fn save(&self) -> io::Result<()> {
        let path = Self::get_user_file_path(&self.uuid);

        // Ensure the directory exists
        if let Some(parent_dir) = path.parent() {
            if !parent_dir.exists() {
                fs::create_dir_all(parent_dir)?;
            }
        }

        let json_str = serde_json::to_string_pretty(self)?; // Serialize the single User
        write_atomic(&path, &json_str)?;
        Ok(())
    }

    /// Loads all `User`s by reading every JSON file in the `data/users/` directory.
    /// It uses the internal deserialization logic for each file.
    /// Files that cannot be deserialized are skipped with a warning.
    /// If the directory does not exist, it returns an empty `HashMap<String, User>`.
    pub fn load_all() -> io::Result<HashMap<String, Self>> {
        let dir_path = Path::new(Self::USERS_DIR);
        let mut users = HashMap::new();

        if !dir_path.exists() {
            eprintln!(
                "Users directory not found at {}. Returning an empty HashMap.",
                dir_path.display()
            );
            return Ok(HashMap::new());
        }

        for entry in fs::read_dir(dir_path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() && path.extension().is_some_and(|ext| ext == "json") {
                match fs::read_to_string(&path) {
                    Ok(json_str) => match serde_json::from_str::<Self>(&json_str) {
                        Ok(user) => {
                            let uuid = user.uuid.clone();
                            users.insert(uuid, user);
                        }
                        Err(e) => eprintln!(
                            "Warning: Could not deserialize user from {}: {}",
                            path.display(),
                            e
                        ),
                    },
                    Err(e) => eprintln!("Warning: Could not read file {}: {}", path.display(), e),
                }
            }
        }
        Ok(users)
    }

    /// Saves a HashMap of `User`s, where each `User` is saved to its own file
    /// in the `data/users/` directory using the `user.save()` method.
    /// This method overwrites existing files and then removes any orphaned files.
    ///
    /// The orphan cleanup pass makes the on-disk directory a faithful mirror
    /// of the in-memory map: users removed from the map are also deleted from
    /// disk, preventing stale state from being resurrected by `load_all`.
    pub fn save_all(users: &HashMap<String, Self>) -> io::Result<()> {
        let dir_path = Path::new(Self::USERS_DIR);

        // Ensure the directory exists
        if !dir_path.exists() {
            fs::create_dir_all(dir_path)?;
        }

        // Keep track of files that should exist after saving
        let mut expected_files = HashSet::new();

        // Save each user individually using the individual user.save() method.
        // Note: filenames are keyed on user.uuid (not the HashMap key) so that
        // on-disk files always match the canonical identity stored inside the
        // User struct, even if the two were ever to diverge.
        for user in users.values() {
            user.save()?;
            let filename = format!("{}.json", user.uuid);
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
