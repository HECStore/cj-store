use std::collections::{HashMap, VecDeque};
use std::io;

use crate::bot::Bot;
use crate::config::Config;
use crate::order::Order;
use crate::pair::Pair;
use crate::storage::Storage;
use crate::trade::Trade;
use crate::user::User;

pub struct Store {
    pub config: Config,
    pub pairs: HashMap<String, Pair>,
    pub users: HashMap<String, User>,
    pub orders: VecDeque<Order>,
    pub trades: Vec<Trade>,
    // might wanna add events: Vec<Event> for stuff like paying, deposits, adding stock etc (maybe even merge trades into this, have methods to get certain event types idk)
    // might wanna add logs
    // might wanna have also a list of items and synonyms and shit
    pub storage: Storage,
    pub bot: Option<Bot>,
}

impl Store {
    /// Creates a new `Store` instance, loading the configuration.
    ///
    /// This method attempts to load the `Config` from `data/config.json`.
    /// If the file doesn't exist, a default config is created and saved.
    ///
    /// It returns an `io::Result<Self>` to indicate success or failure.
    pub fn new() -> io::Result<Self> {
        let config = Config::load()?; // Load config, propagating any errors
        let pairs = Pair::load_all()?; // Load pairs, propagating any errors
        let users = User::load_all()?; // Load users, propagating any errors
        let orders = Order::load_all()?; // Load orders
        let trades = Trade::load_all()?; // Load trades
        let storage = Storage::load(&config.position).unwrap(); // Load storage

        Ok(Store {
            config,
            pairs,
            users,
            orders,
            trades,
            storage,
            bot: None,
        })
    }

    // deposit

    // withdraw

    // maybe move to user??
    pub fn pay(
        &mut self,
        payer_username: &str,
        payee_username: &str,
        amount: f64,
    ) -> Result<(), String> {
        // Get UUIDs from usernames
        let payer_uuid = User::get_uuid(payer_username)?;
        let payee_uuid = User::get_uuid(payee_username)?;

        // Check if payer exists
        if !self.users.contains_key(&payer_uuid) {
            return Err(format!("Payer with UUID {} not found", payer_uuid));
        }

        // Create payee if they don't exist
        if !self.users.contains_key(&payee_uuid) {
            self.users
                .insert(payee_uuid.to_string(), User::new(payee_uuid.to_string()));
        }

        // Check if payer has sufficient balance
        let payer_balance = self.users.get(&payer_uuid).unwrap().balance;
        if payer_balance < amount {
            return Err(format!(
                "Insufficient balance. Required: {}, Available: {}",
                amount, payer_balance
            ));
        }

        // Check for valid amount
        if amount <= 0.0 {
            return Err("Amount must be positive".to_string());
        }

        // Perform the transfer
        self.users.get_mut(&payer_uuid).unwrap().balance -= amount;
        self.users.get_mut(&payee_uuid).unwrap().balance += amount;

        // Update usernames in case they changed
        self.users.get_mut(&payer_uuid).unwrap().username = payer_username.to_owned();
        self.users.get_mut(&payee_uuid).unwrap().username = payee_username.to_owned();

        Ok(())
    }

    pub fn add_pair(&mut self, item: String, item_stock: i32, currency_stock: f64) -> &Pair {
        // Only insert if the pair doesn't already exist
        if !self.pairs.contains_key(&item) {
            let pair = Pair {
                item: item.clone(),
                item_stock,
                currency_stock,
            };
            self.pairs.insert(item.clone(), pair);
        }

        self.pairs.get(&item).unwrap()
    }

    pub fn add_user(&mut self, username: String) -> &User {
        let uuid = User::get_uuid(username.as_str()).unwrap();

        // Only insert if the user doesn't already exist
        if !self.users.contains_key(&uuid) {
            let user = User::new(username);
            self.users.insert(uuid.clone(), user);
        }

        self.users.get(&uuid).unwrap()
    }

    pub async fn init_bot(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let bot = Bot::new(
            self.config.account_email.clone(),
            self.config.server_address.clone(),
        )
        .await;
        bot.connect().await?;
        self.bot = Some(bot);
        Ok(())
    }

    pub async fn disconnect_bot(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(bot) = &self.bot {
            bot.disconnect().await?;
        }
        self.bot = None;
        Ok(())
    }

    pub async fn is_bot_connected(&self) -> bool {
        match &self.bot {
            Some(bot) => bot.is_connected().await,
            None => false,
        }
    }

    pub async fn send_trade_notification(
        &self,
        message: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(bot) = &self.bot {
            bot.send_chat_message(message).await?;
        }
        Ok(())
    }

    // Integration method to handle bot commands related to trading
    pub async fn handle_bot_command(
        &mut self,
        sender: &str,
        command: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let parts: Vec<&str> = command.split_whitespace().collect();

        match parts.get(0) {
            Some(&"buy") => {
                if parts.len() >= 3 {
                    let item = parts[1];
                    let quantity: u32 = parts[2].parse().unwrap_or(0);
                    // Handle buy order logic here
                    Ok(format!("Created buy order for {} {}", quantity, item))
                } else {
                    Ok("Usage: buy <item> <quantity>".to_string())
                }
            }
            Some(&"sell") => {
                if parts.len() >= 3 {
                    let item = parts[1];
                    let quantity: u32 = parts[2].parse().unwrap_or(0);
                    // Handle sell order logic here
                    Ok(format!("Created sell order for {} {}", quantity, item))
                } else {
                    Ok("Usage: sell <item> <quantity>".to_string())
                }
            }
            Some(&"bal") => {
                // Return user balance
                Ok(format!(
                    "{}'s balance: {} diamonds",
                    sender,
                    self.get_user_balance(sender)
                ))
            }
            _ => Ok("Available commands: buy, sell, balance, orders".to_string()),
        }
    }

    // Helper methods (you'll need to implement these based on your User/Order structures)
    pub fn get_user_balance(&self, username: &str) -> f64 {
        self.users
            .get(username)
            .map(|user| user.balance)
            .unwrap_or(0.0)
    }

    // process next order

    // all of these below might have to be moved to bot or storage or something idk

    // consolidate chest

    // consolidate item (across all chests)

    // unallocate empty chests

    // allocate new chest for item

    // need a way to fill chests where shulkers are missing, bot should ideally craft new shukers itself

    /// Saves all store data to disk
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Save all pairs
        Pair::save_all(&self.pairs)?;

        // Save all users
        User::save_all(&self.users)?;

        // Save all orders
        Order::save_all(&self.orders)?;

        // Save storage
        self.storage.save().unwrap();

        Ok(())
    }
}
