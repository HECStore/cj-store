use crate::types::{Chest, Trade, User};
use tokio::sync::oneshot;

/// Messages sent to the Store from Bot or CLI.
pub enum StoreMessage {
    FromBot(BotToStore),
    FromCli(CliToStore),
}

/// Messages from Bot to Store (e.g., player commands).
pub enum BotToStore {
    /// Player sent a message (e.g., "/msg HECStore buy cobblestone 256").
    PlayerMessage { player: String, message: String },
}

/// CLI commands to Store (e.g., query balances, set prices).
pub enum CliToStore {
    /// Request user balances.
    GetBalances {
        response_channel: oneshot::Sender<Vec<User>>,
    },
    /// Set price for an item.
    SetPrice {
        item: String,
        price: f64,
        response_channel: oneshot::Sender<Result<(), String>>,
    },
    /// Reboot the Bot.
    RebootBot {
        response_channel: oneshot::Sender<Result<(), String>>,
    },
}

/// Instructions from Store to Bot (e.g., navigate to chest, execute trade).
pub enum StoreToBot {
    /// Instruct Bot to go to a chest and perform an action.
    GoToChest {
        chest: Chest,
        action: ChestAction,
        response_channel: oneshot::Sender<Result<(), String>>,
    },
    /// Instruct Bot to execute a trade.
    ExecuteTrade {
        trade: Trade,
        response_channel: oneshot::Sender<Result<(), String>>,
    },
    /// Reboot the Bot.
    Reboot,
}
