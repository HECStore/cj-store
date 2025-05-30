use azalea::prelude::*;
use azalea::{Account, Client, Event};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone, Component)]
pub struct BotState {
    // Add any state you want to track here
    connected: bool,
}

impl Default for BotState {
    fn default() -> Self {
        Self { connected: false }
    }
}

#[derive(Clone)]
pub struct Bot {
    client: Arc<RwLock<Option<Client>>>,
    account: Account,
    server_address: String,
}

impl Bot {
    pub async fn new(account_email: String, server_address: String) -> Self {
        let account = Account::microsoft(account_email.as_str()).await.unwrap();

        Self {
            client: Arc::new(RwLock::new(None)),
            account,
            server_address,
        }
    }

    pub async fn connect(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let client = azalea::ClientBuilder::new()
            .set_handler(handle_event)
            .start(self.account.clone(), &*self.server_address)
            .await?;

        *self.client.write().await = Some(client);
        Ok(())
    }

    pub async fn disconnect(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(client) = self.client.write().await.take() {
            client.disconnect();
        }
        Ok(())
    }

    pub async fn is_connected(&self) -> bool {
        self.client.read().await.is_some()
    }

    pub async fn send_chat_message(
        &self,
        message: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(client) = self.client.read().await.as_ref() {
            client.chat(message);
        }
        Ok(())
    }

    pub async fn send_whisper(
        &self,
        target: &str,
        message: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(client) = self.client.read().await.as_ref() {
            client.chat(&format!("/msg {} {}", target, message));
        }
        Ok(())
    }

    pub async fn send_trade(
        &self,
        target: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(client) = self.client.read().await.as_ref() {
            client.chat(&format!("/trade {}", target));
        }
        Ok(())
    }

    pub async fn get_position(&self) -> Option<azalea::Vec3> {
        if let Some(client) = self.client.read().await.as_ref() {
            Some(client.position())
        } else {
            None
        }
    }
}

// Event handler for the bot
async fn handle_event(client: Client, event: Event, mut state: BotState) -> anyhow::Result<()> {
    match event {
        Event::Init => {
            println!("Bot connected and initialized!");
            state.connected = true;
        }
        Event::Chat(m) => {
            println!("Chat message: {}", m.message());
            // Handle chat messages here - could integrate with your store logic
            handle_chat_message(client, m).await?;
        }
        _ => {}
    }
    Ok(())
}

async fn handle_chat_message(
    client: Client,
    message: azalea::chat::ChatPacket,
) -> anyhow::Result<()> {
    let msg = message.message().to_string();
    let sender = message.sender().unwrap_or("Unknown".to_string());

    // Check if this is a whisper
    if msg.contains("whispers:") {
        // Extract the actual message content after "whispers:"
        let content = msg.split("whispers:").nth(1).unwrap_or("").trim();

        match content {
            "bal" => {
                // Example response - you can modify this based on your needs
                client.chat(&format!("/msg {} Your balance is 100 diamonds", sender));
            }
            _ => {
                // Handle other commands or unknown messages
                client.chat(&format!(
                    "/msg {} Unknown command. Available commands: bal",
                    sender
                ));
            }
        }
    } else {
        println!("Message from {}: {}", sender, msg);
    }

    Ok(())
}
