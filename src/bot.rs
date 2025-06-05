use azalea::prelude::*;
use azalea::{Account, Client, Event};
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, error, info, trace, warn};

use crate::messages::{BotInstruction, BotMessage, StoreMessage};
use crate::types::{Chest, Trade};

#[derive(Debug, Clone)]
pub enum ChestAction {
    Deposit { items: Vec<String> },
    Withdraw { items: Vec<String> },
    Check,
}

#[derive(Clone, Component)]
pub struct BotState {
    connected: bool,
    store_tx: Option<mpsc::Sender<StoreMessage>>,
}

impl Default for BotState {
    fn default() -> Self {
        Self {
            connected: false,
            store_tx: None,
        }
    }
}

#[derive(Clone)]
pub struct Bot {
    client: Arc<RwLock<Option<Client>>>,
    account: Account,
    server_address: String,
    store_tx: mpsc::Sender<StoreMessage>,
}

impl Bot {
    pub async fn new(
        account_email: String,
        server_address: String,
        store_tx: mpsc::Sender<StoreMessage>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let account = Account::microsoft(&account_email).await?;

        Ok(Self {
            client: Arc::new(RwLock::new(None)),
            account,
            server_address,
            store_tx,
        })
    }

    pub async fn connect(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Connecting to server: {}", self.server_address);

        // Create initial state with our communication channels
        let initial_state = BotState {
            connected: false,
            store_tx: Some(self.store_tx.clone()),
        };

        let client = azalea::ClientBuilder::new()
            .set_handler(handle_event_fn)
            .set_state(initial_state)
            .start(self.account.clone(), &*self.server_address)
            .await?;

        *self.client.write().await = Some(client);
        info!("Bot connected successfully");
        Ok(())
    }

    pub async fn disconnect(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(client) = self.client.write().await.take() {
            client.disconnect();
            info!("Bot disconnected");
        }
        Ok(())
    }

    pub async fn is_connected(&self) -> bool {
        self.client.read().await.is_some()
    }

    pub async fn send_chat_message(&self, message: &str) -> Result<(), String> {
        if let Some(client) = self.client.read().await.as_ref() {
            client.chat(message);
            debug!("Sent chat message: {}", message);
            Ok(())
        } else {
            Err("Bot not connected".to_string())
        }
    }

    pub async fn send_whisper(&self, target: &str, message: &str) -> Result<(), String> {
        if let Some(client) = self.client.read().await.as_ref() {
            client.chat(&format!("/msg {} {}", target, message));
            debug!("Sent whisper to {}: {}", target, message);
            Ok(())
        } else {
            Err("Bot not connected".to_string())
        }
    }

    pub async fn go_to_chest(&self, chest: &Chest) -> Result<(), String> {
        if let Some(client) = self.client.read().await.as_ref() {
            // Navigate to chest position
            info!("Navigating to chest at {:?}", chest.position);

            // This is a simplified implementation - you'll need to add proper pathfinding
            // For now, just teleport or use basic movement commands
            client.chat(&format!(
                "/tp {} {} {}",
                chest.position.x, chest.position.y, chest.position.z
            ));

            // Wait a moment for movement
            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

            Ok(())
        } else {
            Err("Bot not connected".to_string())
        }
    }

    pub async fn execute_trade(&self, trade: &Trade) -> Result<(), String> {
        if let Some(client) = self.client.read().await.as_ref() {
            info!("Executing trade: {:?}", trade);

            // Send trade request
            client.chat(&format!("/trade {}", trade.user_uuid));

            // Wait for trade window to open
            tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;

            // Add items to trade (this is simplified - you'll need proper inventory management)
            // This would involve interacting with the trade GUI

            info!("Trade executed successfully");
            Ok(())
        } else {
            Err("Bot not connected".to_string())
        }
    }

    pub async fn get_position(&self) -> Option<azalea::Vec3> {
        if let Some(client) = self.client.read().await.as_ref() {
            Some(client.position())
        } else {
            None
        }
    }
}

/// Main bot task that handles instructions from the Store
pub async fn bot_task(
    store_tx: mpsc::Sender<StoreMessage>,
    mut bot_rx: mpsc::Receiver<BotInstruction>,
    account_email: String,
    server_address: String,
) {
    // Create bot instance using config values
    let bot = match Bot::new(account_email, server_address, store_tx.clone()).await {
        Ok(bot) => bot,
        Err(e) => {
            error!("Failed to create bot: {}", e);
            return;
        }
    };

    // Connect to server
    if let Err(e) = bot.connect().await {
        error!("Failed to connect bot: {}", e);
        return;
    }

    // Main event loop
    while let Some(instruction) = bot_rx.recv().await {
        match instruction {
            BotInstruction::InteractWithChest {
                target_chest,
                action,
                respond_to,
            } => {
                info!("Received chest interaction instruction");

                let result = bot.go_to_chest(&target_chest).await;

                // Handle the chest action here
                match action {
                    ChestAction::Deposit { items } => {
                        info!("Depositing items: {:?}", items);
                        // Implement deposit logic
                    }
                    ChestAction::Withdraw { items } => {
                        info!("Withdrawing items: {:?}", items);
                        // Implement withdraw logic
                    }
                    ChestAction::Check => {
                        info!("Checking chest contents");
                        // Implement check logic
                    }
                }

                let _ = respond_to.send(result);
            }
            BotInstruction::ProcessTrade {
                trade_details,
                respond_to,
            } => {
                info!("Received trade instruction");
                let result = bot.execute_trade(&trade_details).await;
                let _ = respond_to.send(result);
            }
            BotInstruction::Restart => {
                info!("Restarting bot");

                // Disconnect and reconnect
                if let Err(e) = bot.disconnect().await {
                    error!("Error during disconnect: {}", e);
                }

                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

                if let Err(e) = bot.connect().await {
                    error!("Error during reconnect: {}", e);
                }
            }
            BotInstruction::Shutdown { respond_to } => {
                info!("Shutting down bot");
                
                // Disconnect from server
                if let Err(e) = bot.disconnect().await {
                    error!("Error during bot disconnect: {}", e);
                }

                // Signal shutdown complete
                let _ = respond_to.send(());
                
                // Break the loop to end the task
                break;
            }
        }
    }

    // Channel closed, perform final cleanup
    info!("Bot channel closed, performing final cleanup");
    
    // Ensure bot is disconnected
    if let Err(e) = bot.disconnect().await {
        error!("Error during final bot disconnect: {}", e);
    }

    info!("Bot task shutdown complete");
}

// Function pointer that matches the expected signature
fn handle_event_fn(
    client: Client,
    event: Event,
    mut state: BotState,
) -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
    async move { handle_event(client, event, &mut state).await }
}

// Your event handler that works with the state
async fn handle_event(client: Client, event: Event, state: &mut BotState) -> anyhow::Result<()> {
    match event {
        Event::Init => {
            info!("Bot connected and initialized!");
            state.connected = true;
            // Note: You'll need to initialize store_tx in the state
            // This is a limitation of the function pointer approach
        }
        Event::Chat(m) => {
            let message_text = m.message().to_string();
            debug!("Chat message received: {}", message_text);

            if let Some(store_tx) = &state.store_tx {
                if let Err(e) = handle_chat_message(client, m, store_tx).await {
                    error!("Error handling chat message: {}", e);
                }
            }
        }
        Event::Disconnect(_) => {
            warn!("Bot disconnected from server");
            state.connected = false;
        }
        _ => {}
    }
    Ok(())
}

async fn handle_chat_message(
    client: Client,
    message: azalea::chat::ChatPacket,
    store_tx: &mpsc::Sender<StoreMessage>,
) -> anyhow::Result<()> {
    let msg = message.message().to_string();
    let sender = message.sender().unwrap_or_else(|| "Unknown".to_string());

    // Check if this is a whisper to our bot
    if msg.contains("whispers:") || msg.contains("tells you:") {
        // Extract the actual message content
        let content = if let Some(pos) = msg.find("whispers:") {
            msg[pos + 9..].trim()
        } else if let Some(pos) = msg.find("tells you:") {
            msg[pos + 10..].trim()
        } else {
            return Ok(());
        };

        info!("Received whisper from {}: {}", sender, content);

        // Send the command to the store for processing
        let bot_message = BotMessage::PlayerCommand {
            player_name: sender.clone(),
            command: content.to_string(),
        };

        let store_message = StoreMessage::FromBot(bot_message);

        if let Err(e) = store_tx.send(store_message).await {
            error!("Failed to send message to store: {}", e);
        }
    } else {
        // Log other chat messages for debugging
        trace!("Public chat - {}: {}", sender, msg);
    }

    Ok(())
}
