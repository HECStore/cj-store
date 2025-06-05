use std::collections::{HashMap, VecDeque};
use std::io;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::messages::{BotInstruction, BotMessage, CliMessage, StoreMessage};
use crate::types::{Order, Pair, Storage, Trade, User};

pub struct Store {
    pub config: Config,
    pub pairs: HashMap<String, Pair>,
    pub users: HashMap<String, User>,
    pub orders: VecDeque<Order>,
    pub trades: Vec<Trade>,
    pub storage: Storage,

    // Communication channels
    bot_tx: mpsc::Sender<BotInstruction>,
}

impl Store {
    /// Creates a new `Store` instance, loading the configuration.
    pub async fn new(bot_tx: mpsc::Sender<BotInstruction>) -> io::Result<Self> {
        info!("Initializing new Store instance");

        let config = Config::load()?;
        let pairs = Pair::load_all()?;
        let users = User::load_all()?;
        let orders = Order::load_all()?;
        let trades = Trade::load_all()?;
        let storage = Storage::load(&config.position).unwrap();

        info!(
            "Store initialized successfully with {} pairs, {} users, {} orders",
            pairs.len(),
            users.len(),
            orders.len()
        );

        Ok(Store {
            config,
            pairs,
            users,
            orders,
            trades,
            storage,
            bot_tx,
        })
    }

    /// Main event loop for the Store
    pub async fn run(
        mut self,
        mut store_rx: mpsc::Receiver<StoreMessage>,
        bot_tx: mpsc::Sender<BotInstruction>,
    ) {
        info!("Store started and listening for messages");

        while let Some(message) = store_rx.recv().await {
            debug!(
                "Received store message: {:?}",
                std::mem::discriminant(&message)
            );

            match message {
                StoreMessage::FromBot(bot_msg) => {
                    if let Err(e) = self.handle_bot_message(bot_msg).await {
                        error!("Error handling bot message: {}", e);
                    }
                }
                StoreMessage::FromCli(cli_msg) => {
                    if let Err(e) = self.handle_cli_message(cli_msg).await {
                        error!("Error handling CLI message: {}", e);
                    }
                }
            }

            // Auto-save after each operation
            if let Err(e) = self.save() {
                error!("Failed to save store data: {}", e);
            } else {
                debug!("Store data saved successfully");
            }
        }

        // Channel closed, perform final shutdown
        info!("Store channel closed, performing final shutdown");
        
        // Save one final time
        if let Err(e) = self.save() {
            error!("Failed to save store data during final shutdown: {}", e);
        }

        // Drop bot_tx to signal bot shutdown
        drop(bot_tx);
        
        info!("Store shutdown complete");
    }

    /// Handle messages from the bot
    async fn handle_bot_message(&mut self, message: BotMessage) -> Result<(), String> {
        match message {
            BotMessage::PlayerCommand {
                player_name,
                command,
            } => {
                info!("Processing command from {}: {}", player_name, command);
                self.handle_player_command(&player_name, &command).await
            }
        }
    }

    /// Handle messages from the CLI
    async fn handle_cli_message(&mut self, message: CliMessage) -> Result<(), String> {
        match message {
            CliMessage::QueryBalances { respond_to } => {
                debug!("Querying user balances");
                let users: Vec<User> = self.users.values().cloned().collect();
                let _ = respond_to.send(users);
                Ok(())
            }
            CliMessage::UpdatePrice {
                item_name,
                new_price,
                respond_to,
            } => {
                info!("Updating price for {} to {}", item_name, new_price);
                let result = self.update_item_price(&item_name, new_price).await;
                let _ = respond_to.send(result);
                Ok(())
            }
            CliMessage::RestartBot { respond_to } => {
                info!("Initiating bot restart");
                // Send restart instruction to bot
                if let Err(e) = self.bot_tx.send(BotInstruction::Restart).await {
                    let error_msg = format!("Failed to send restart instruction: {}", e);
                    error!("{}", error_msg);
                    let _ = respond_to.send(Err(error_msg.clone()));
                    return Err(error_msg);
                }
                let _ = respond_to.send(Ok(()));
                Ok(())
            }
            CliMessage::Shutdown { respond_to } => {
                info!("Initiating graceful shutdown");

                // Signal bot to shutdown
                let (bot_response_tx, bot_response_rx) = oneshot::channel();
                if let Err(e) = self
                    .bot_tx
                    .send(BotInstruction::Shutdown {
                        respond_to: bot_response_tx,
                    })
                    .await
                {
                    error!("Failed to send shutdown instruction to bot: {}", e);
                }

                // Wait for bot shutdown confirmation
                if let Err(e) = bot_response_rx.await {
                    error!("Failed to receive bot shutdown confirmation: {}", e);
                }

                // Save all data before shutdown
                if let Err(e) = self.save() {
                    error!("Failed to save store data during shutdown: {}", e);
                }

                // Signal shutdown complete
                let _ = respond_to.send(());
                Ok(())
            }
        }
    }

    /// Handle player commands from the bot
    async fn handle_player_command(
        &mut self,
        player_name: &str,
        command: &str,
    ) -> Result<(), String> {
        let parts: Vec<&str> = command.split_whitespace().collect();

        match parts.get(0) {
            Some(&"buy") => {
                if parts.len() >= 3 {
                    let item = parts[1];
                    let quantity: u32 = parts[2].parse().map_err(|_| {
                        warn!("Invalid quantity provided by {}: {}", player_name, parts[2]);
                        "Invalid quantity"
                    })?;
                    debug!(
                        "Processing buy order: {} wants {} of {}",
                        player_name, quantity, item
                    );
                    self.handle_buy_order(player_name, item, quantity).await
                } else {
                    warn!(
                        "Invalid buy command format from {}: {}",
                        player_name, command
                    );
                    // Send usage message back to bot
                    self.send_message_to_player(player_name, "Usage: buy <item> <quantity>")
                        .await
                }
            }
            Some(&"sell") => {
                if parts.len() >= 3 {
                    let item = parts[1];
                    let quantity: u32 = parts[2].parse().map_err(|_| {
                        warn!("Invalid quantity provided by {}: {}", player_name, parts[2]);
                        "Invalid quantity"
                    })?;
                    debug!(
                        "Processing sell order: {} wants to sell {} of {}",
                        player_name, quantity, item
                    );
                    self.handle_sell_order(player_name, item, quantity).await
                } else {
                    warn!(
                        "Invalid sell command format from {}: {}",
                        player_name, command
                    );
                    self.send_message_to_player(player_name, "Usage: sell <item> <quantity>")
                        .await
                }
            }
            Some(&"bal") | Some(&"balance") => {
                debug!("Balance check requested by {}", player_name);
                let balance = self.get_user_balance(player_name);
                let message = format!("{}'s balance: {} diamonds", player_name, balance);
                self.send_message_to_player(player_name, &message).await
            }
            Some(&"pay") => {
                if parts.len() >= 3 {
                    let recipient = parts[1];
                    let amount: f64 = parts[2].parse().map_err(|_| {
                        warn!(
                            "Invalid payment amount provided by {}: {}",
                            player_name, parts[2]
                        );
                        "Invalid amount"
                    })?;
                    info!(
                        "Processing payment: {} -> {} ({})",
                        player_name, recipient, amount
                    );
                    match self.pay(player_name, recipient, amount) {
                        Ok(()) => {
                            let message = format!("Paid {} diamonds to {}", amount, recipient);
                            info!(
                                "Payment successful: {} paid {} to {}",
                                player_name, amount, recipient
                            );
                            self.send_message_to_player(player_name, &message).await
                        }
                        Err(e) => {
                            warn!("Payment failed: {} -> {}: {}", player_name, recipient, e);
                            self.send_message_to_player(player_name, &e).await
                        }
                    }
                } else {
                    warn!(
                        "Invalid pay command format from {}: {}",
                        player_name, command
                    );
                    self.send_message_to_player(player_name, "Usage: pay <player> <amount>")
                        .await
                }
            }
            Some(unknown_cmd) => {
                warn!("Unknown command '{}' from {}", unknown_cmd, player_name);
                self.send_message_to_player(
                    player_name,
                    "Available commands: buy, sell, balance, pay",
                )
                .await
            }
            None => {
                warn!("Empty command received from {}", player_name);
                self.send_message_to_player(
                    player_name,
                    "Available commands: buy, sell, balance, pay",
                )
                .await
            }
        }
    }

    /// Handle buy orders
    async fn handle_buy_order(
        &mut self,
        player_name: &str,
        item: &str,
        quantity: u32,
    ) -> Result<(), String> {
        // Add user if they don't exist
        self.add_user(player_name.to_string());

        // Check if pair exists
        if !self.pairs.contains_key(item) {
            warn!(
                "Player {} attempted to buy unavailable item: {}",
                player_name, item
            );
            return self
                .send_message_to_player(
                    player_name,
                    &format!("Item '{}' is not available for trading", item),
                )
                .await;
        }

        // Create order logic here
        info!(
            "Created buy order: {} x{} for {}",
            item, quantity, player_name
        );
        let message = format!("Created buy order for {} {}", quantity, item);
        self.send_message_to_player(player_name, &message).await
    }

    /// Handle sell orders
    async fn handle_sell_order(
        &mut self,
        player_name: &str,
        item: &str,
        quantity: u32,
    ) -> Result<(), String> {
        // Add user if they don't exist
        self.add_user(player_name.to_string());

        // Check if pair exists
        if !self.pairs.contains_key(item) {
            warn!(
                "Player {} attempted to sell unavailable item: {}",
                player_name, item
            );
            return self
                .send_message_to_player(
                    player_name,
                    &format!("Item '{}' is not available for trading", item),
                )
                .await;
        }

        // Create order logic here
        info!(
            "Created sell order: {} x{} for {}",
            item, quantity, player_name
        );
        let message = format!("Created sell order for {} {}", quantity, item);
        self.send_message_to_player(player_name, &message).await
    }

    /// Send a message to a specific player via the bot
    async fn send_message_to_player(&self, player_name: &str, message: &str) -> Result<(), String> {
        debug!("Sending message to {}: {}", player_name, message);
        // You would implement this by sending a message instruction to the bot
        // For now, just log it - you might want to add a message instruction type
        Ok(())
    }

    /// Update price for an item
    async fn update_item_price(&mut self, item_name: &str, new_price: f64) -> Result<(), String> {
        if let Some(_pair) = self.pairs.get_mut(item_name) {
            // You'll need to add a price field to your Pair struct
            info!("Updated price for {} to {}", item_name, new_price);
            Ok(())
        } else {
            warn!(
                "Attempted to update price for non-existent item: {}",
                item_name
            );
            Err(format!("Item '{}' not found", item_name))
        }
    }

    /// Pay method (existing implementation)
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
            error!("Payer with UUID {} not found", payer_uuid);
            return Err(format!("Payer with UUID {} not found", payer_uuid));
        }

        // Create payee if they don't exist
        if !self.users.contains_key(&payee_uuid) {
            info!("Creating new user for payee: {}", payee_username);
            self.users
                .insert(payee_uuid.to_string(), User::new(payee_uuid.to_string()));
        }

        // Check if payer has sufficient balance
        let payer_balance = self.users.get(&payer_uuid).unwrap().balance;
        if payer_balance < amount {
            warn!(
                "Insufficient balance for payment: {} has {}, needs {}",
                payer_username, payer_balance, amount
            );
            return Err(format!(
                "Insufficient balance. Required: {}, Available: {}",
                amount, payer_balance
            ));
        }

        // Check for valid amount
        if amount <= 0.0 {
            warn!("Invalid payment amount attempted: {}", amount);
            return Err("Amount must be positive".to_string());
        }

        // Perform the transfer
        self.users.get_mut(&payer_uuid).unwrap().balance -= amount;
        self.users.get_mut(&payee_uuid).unwrap().balance += amount;

        // Update usernames in case they changed
        self.users.get_mut(&payer_uuid).unwrap().username = payer_username.to_owned();
        self.users.get_mut(&payee_uuid).unwrap().username = payee_username.to_owned();

        debug!(
            "Payment completed: {} -> {} ({})",
            payer_username, payee_username, amount
        );
        Ok(())
    }

    /// Add pair method (existing implementation)
    pub fn add_pair(&mut self, item: String, item_stock: i32, currency_stock: f64) -> &Pair {
        if !self.pairs.contains_key(&item) {
            info!(
                "Adding new trading pair: {} (stock: {}, currency: {})",
                item, item_stock, currency_stock
            );
            let pair = Pair {
                item: item.clone(),
                item_stock,
                currency_stock,
            };
            self.pairs.insert(item.clone(), pair);
        } else {
            debug!("Trading pair {} already exists", item);
        }

        self.pairs.get(&item).unwrap()
    }

    /// Add user method (existing implementation)
    pub fn add_user(&mut self, username: String) -> &User {
        let uuid = User::get_uuid(username.as_str()).unwrap();

        if !self.users.contains_key(&uuid) {
            info!("Adding new user: {} (UUID: {})", username, uuid);
            let user = User::new(username);
            self.users.insert(uuid.clone(), user);
        } else {
            debug!("User {} already exists", username);
        }

        self.users.get(&uuid).unwrap()
    }

    /// Get user balance
    pub fn get_user_balance(&self, username: &str) -> f64 {
        if let Ok(uuid) = User::get_uuid(username) {
            let balance = self
                .users
                .get(&uuid)
                .map(|user| user.balance)
                .unwrap_or(0.0);
            debug!("Balance query for {}: {} diamonds", username, balance);
            balance
        } else {
            warn!("Failed to get UUID for username: {}", username);
            0.0
        }
    }

    /// Saves all store data to disk
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        debug!("Saving store data to disk");
        Pair::save_all(&self.pairs)?;
        User::save_all(&self.users)?;
        Order::save_all(&self.orders)?;
        Storage::save(&self.storage).unwrap();
        debug!("Store data saved successfully");
        Ok(())
    }
}
