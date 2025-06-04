use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pair {
    pub item: String,
    pub item_stock: i32,
    pub currency_stock: f64,
    // might wanna add buy/sell item and currency volume, fees colelcted, number of trades etc. for stats
    // (or create a new struct Trade and log all trades instead and make methods to calculcate all that from trades)
}

impl Pair {
    // Directory where all individual pair files will be stored
    const PAIRS_DIR: &str = "data/pairs";

    // Helper function to get the file path for a single pair
    fn get_pair_file_path(item_name: &str) -> PathBuf {
        PathBuf::from(Self::PAIRS_DIR).join(format!("{}.json", item_name))
    }

    /// Loads a single `Pair` from `data/pairs/{item_name}.json`.
    /// Returns an `io::Error` with `ErrorKind::NotFound` if the file does not exist.
    pub fn load(item_name: &str) -> io::Result<Self> {
        let path = Self::get_pair_file_path(item_name);

        if path.exists() {
            let json_str = fs::read_to_string(&path)?;
            let pair: Self = serde_json::from_str(&json_str)?;
            Ok(pair)
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Pair file not found: {}", path.display()),
            ))
        }
    }

    /// Saves this single `Pair` instance to `data/pairs/{self.item}.json`.
    /// Creates the 'data/pairs' directory if it doesn't exist.
    pub fn save(&self) -> io::Result<()> {
        let path = Self::get_pair_file_path(&self.item);

        // Ensure the directory exists
        if let Some(parent_dir) = path.parent() {
            if !parent_dir.exists() {
                fs::create_dir_all(parent_dir)?;
            }
        }

        let json_str = serde_json::to_string_pretty(self)?; // Serialize the single Pair
        fs::write(&path, json_str)?;
        Ok(())
    }

    /// Loads all `Pair`s by reading every JSON file in the `data/pairs/` directory.
    /// It uses the internal deserialization logic for each file.
    /// Files that cannot be deserialized are skipped with a warning.
    /// If the directory does not exist, it returns an empty `HashMap<String, Pair>`.
    pub fn load_all() -> io::Result<HashMap<String, Self>> {
        let dir_path = Path::new(Self::PAIRS_DIR);
        let mut pairs = HashMap::new();

        if !dir_path.exists() {
            println!(
                "Pairs directory not found at {}. Returning an empty HashMap.",
                dir_path.display()
            );
            return Ok(HashMap::new());
        }

        for entry in fs::read_dir(dir_path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() && path.extension().map_or(false, |ext| ext == "json") {
                // Here, we can't directly call Pair::load because Pair::load expects an item_name
                // and attempts to read a file based on that. Instead, we read the file
                // and then deserialize it, which is the core logic of Pair::load.
                match fs::read_to_string(&path) {
                    Ok(json_str) => match serde_json::from_str::<Self>(&json_str) {
                        Ok(pair) => {
                            let item_name = pair.item.clone();
                            pairs.insert(item_name, pair);
                        }
                        Err(e) => eprintln!(
                            "Warning: Could not deserialize pair from {}: {}",
                            path.display(),
                            e
                        ),
                    },
                    Err(e) => eprintln!("Warning: Could not read file {}: {}", path.display(), e),
                }
            }
        }
        Ok(pairs)
    }

    /// Saves a HashMap of `Pair`s, where each `Pair` is saved to its own file
    /// in the `data/pairs/` directory using the `pair.save()` method.
    /// This method overwrites existing files and then removes any orphaned files.
    pub fn save_all(pairs: &HashMap<String, Self>) -> io::Result<()> {
        let dir_path = Path::new(Self::PAIRS_DIR);

        // Ensure the directory exists
        if !dir_path.exists() {
            fs::create_dir_all(dir_path)?;
        }

        // Keep track of files that should exist after saving
        let mut expected_files = HashSet::new();

        // Save each pair individually using the individual pair.save() method
        for pair in pairs.values() {
            pair.save()?;
            let filename = format!("{}.json", pair.item);
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
