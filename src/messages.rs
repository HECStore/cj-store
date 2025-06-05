use crate::{
    bot::ChestAction,
    types::{Chest, Trade, User},
};
use tokio::sync::oneshot;

/// Messages sent to the Store from other components.
pub enum StoreMessage {
    FromBot(BotMessage),
    FromCli(CliMessage),
}

/// Messages from Bot to Store.
pub enum BotMessage {
    /// Player sent a command (e.g., "/msg HECStore buy cobblestone 256").
    PlayerCommand {
        player_name: String,
        command: String,
    },
}

/// Messages from CLI to Store.
pub enum CliMessage {
    /// Request all user balances.
    QueryBalances {
        respond_to: oneshot::Sender<Vec<User>>,
    },
    /// Update price for an item.
    UpdatePrice {
        item_name: String,
        new_price: f64,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Request bot restart.
    RestartBot {
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Signal graceful shutdown.
    Shutdown {
        respond_to: oneshot::Sender<()>,
    },
}

/// Instructions from Store to Bot.
pub enum BotInstruction {
    /// Navigate to chest and perform action.
    InteractWithChest {
        target_chest: Chest,
        action: ChestAction,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Execute a player trade.
    ProcessTrade {
        trade_details: Trade,
        respond_to: oneshot::Sender<Result<(), String>>,
    },
    /// Restart the bot.
    Restart,
    /// Shutdown the bot gracefully.
    Shutdown {
        respond_to: oneshot::Sender<()>,
    },
}
