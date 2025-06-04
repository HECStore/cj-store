use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub uuid: String,
    pub username: String, // can change
    pub balance: f64,     // might wanna add item balances later on
                          // might wanna add stats like item and currency buy and sell volumes, number of trades, deposits, withdrawals, fees paid and such
                          // (or create a new struct Trade and log all trades instead and make methods to calculcate all that from trades)
}

#[derive(Deserialize)]
struct MojangResponse {
    id: String,
}

impl User {
    // Directory where all individual user files will be stored
    const USERS_DIR: &str = "data/users";

    pub fn new(username: String) -> Self {
        User {
            uuid: Self::get_uuid(&username).unwrap(),
            balance: 0.0,
            username,
        }
    }

    pub fn get_uuid(username: &str) -> Result<String, String> {
        let url = format!(
            "https://api.mojang.com/users/profiles/minecraft/{}",
            username
        );

        let response = reqwest::blocking::get(&url).map_err(|e| e.to_string())?; // might wanna use ureq instead

        if response.status() == 204 {
            return Err("Player not found".to_string());
        }

        if !response.status().is_success() {
            return Err(format!("API error: {}", response.status()));
        }

        let mojang_response: MojangResponse = response.json().map_err(|e| e.to_string())?;

        // Format UUID with hyphens
        let id = &mojang_response.id;
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

    /// Loads a single `User` from `data/users/{uuid}.json`.
    /// Returns an `io::Error` with `ErrorKind::NotFound` if the file does not exist.
    pub fn load(uuid: &str) -> io::Result<Self> {
        let path = Self::get_user_file_path(uuid);

        if path.exists() {
            let json_str = fs::read_to_string(&path)?;
            let user: Self = serde_json::from_str(&json_str)?;
            Ok(user)
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("User file not found: {}", path.display()),
            ))
        }
    }

    /// Saves this single `User` instance to `data/users/{self.uuid}.json`.
    /// Creates the 'data/users' directory if it doesn't exist.
    pub fn save(&self) -> io::Result<()> {
        let path = Self::get_user_file_path(&self.uuid);

        // Ensure the directory exists
        if let Some(parent_dir) = path.parent() {
            if !parent_dir.exists() {
                fs::create_dir_all(parent_dir)?;
            }
        }

        let json_str = serde_json::to_string_pretty(self)?; // Serialize the single User
        fs::write(&path, json_str)?;
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
            println!(
                "Users directory not found at {}. Returning an empty HashMap.",
                dir_path.display()
            );
            return Ok(HashMap::new());
        }

        for entry in fs::read_dir(dir_path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() && path.extension().map_or(false, |ext| ext == "json") {
                // Here, we can't directly call User::load because User::load expects an uuid
                // and attempts to read a file based on that. Instead, we read the file
                // and then deserialize it, which is the core logic of User::load.
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
    pub fn save_all(users: &HashMap<String, Self>) -> io::Result<()> {
        let dir_path = Path::new(Self::USERS_DIR);

        // Ensure the directory exists
        if !dir_path.exists() {
            fs::create_dir_all(dir_path)?;
        }

        // Keep track of files that should exist after saving
        let mut expected_files = HashSet::new();

        // Save each user individually using the individual user.save() method
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
                if path.is_file() && path.extension().map_or(false, |ext| ext == "json") {
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
