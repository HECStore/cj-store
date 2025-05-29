use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum OrderType {
    #[default] // Mark Buy as the default variant
    Buy,
    Sell,
    Deposit,
    Withdraw,
    // might wanna do AddItem, RemoveItem, AddCurrency, RemoveCurrency, AddItemC, RemoveItemC or something, maybe handle those admin actions differently idk
}

#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Order {
    pub order_type: OrderType,
    pub item: String,
    pub amount: i32,
    pub user_uuid: String, // maybe also name or even User
}

impl Order {
    const ORDERS_FILE: &'static str = "data/orders.json";

    /// Loads all orders from a single JSON file into a VecDeque
    pub fn load_all() -> io::Result<VecDeque<Self>> {
        let file_path = Path::new(Self::ORDERS_FILE);

        if !file_path.exists() {
            println!(
                "Orders file not found at {}. Returning an empty VecDeque.",
                file_path.display()
            );
            return Ok(VecDeque::new());
        }

        match fs::read_to_string(file_path) {
            Ok(json_str) => match serde_json::from_str::<VecDeque<Self>>(&json_str) {
                Ok(orders) => Ok(orders),
                Err(e) => {
                    eprintln!(
                        "Warning: Could not deserialize orders from {}: {}",
                        file_path.display(),
                        e
                    );
                    Ok(VecDeque::new())
                }
            },
            Err(e) => {
                eprintln!(
                    "Warning: Could not read file {}: {}",
                    file_path.display(),
                    e
                );
                Ok(VecDeque::new())
            }
        }
    }

    /// Saves a VecDeque of Orders to a single JSON file
    pub fn save_all(orders: &VecDeque<Self>) -> io::Result<()> {
        let file_path = Path::new(Self::ORDERS_FILE);

        // Ensure the parent directory exists
        if let Some(parent) = file_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }

        let json_str = serde_json::to_string_pretty(orders)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        fs::write(file_path, json_str)?;
        Ok(())
    }
}
