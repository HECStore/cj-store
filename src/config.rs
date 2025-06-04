use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

use crate::types::Position;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub position: Position, // storage origin
    pub fee: f64, // fee (might wanna have separate fees for buy and sell, deposit and withdraw)
    pub account_email: String,
    pub server_address: String,
}

impl Config {
    /// Loads configuration from `data/config.json`.
    /// If the file doesn't exist, it creates a default `config.json` file
    /// and then loads from it.
    pub fn load() -> io::Result<Self> {
        let path = "data/config.json";
        let config_path = Path::new(path);

        if config_path.exists() {
            // File exists, proceed with loading
            let json_str = fs::read_to_string(config_path)?;
            let config: Config = serde_json::from_str(&json_str)?;
            Ok(config)
        } else {
            // File doesn't exist, create a default config and save it
            let default_config = Config {
                position: Position::default(),
                fee: 0.0,
                account_email: String::new(),
                server_address: String::from("corejourney.org"),
            };

            // Ensure the directory exists
            if let Some(parent_dir) = config_path.parent() {
                if !parent_dir.exists() {
                    fs::create_dir_all(parent_dir)?;
                }
            }

            // Serialize the default config to a pretty JSON string
            let json_str = serde_json::to_string_pretty(&default_config)?;

            // Write the default config to the file
            fs::write(config_path, json_str)?;

            println!("Created default config file at: {}", path); // Informative print

            Ok(default_config)
        }
    }
}
