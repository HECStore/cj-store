//! # Structured error types for store operations
//!
//! Historically every handler returned `Result<T, String>`, which made error
//! categorization impossible: call sites could not distinguish "user typed a
//! bad item name" from "bot crashed mid-trade". `StoreError` introduces a
//! typed hierarchy so higher-level code can match on the cause and react
//! appropriately (retry, notify player, escalate to operator, etc.).
//!
//! Migration is progressive: new code should prefer `StoreError`, but the
//! existing `Result<T, String>` boundary is preserved via `From<StoreError>
//! for String` so we can introduce the type incrementally without a big-bang
//! refactor.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("Item '{0}' not found")]
    ItemNotFound(String),

    #[error("Insufficient funds: need {need:.2}, have {have:.2}")]
    InsufficientFunds { need: f64, have: f64 },

    #[error("Insufficient stock for '{item}': need {need}, have {have}")]
    InsufficientStock { item: String, need: i32, have: i32 },

    #[error("Bot not connected")]
    BotDisconnected,

    #[error("Trade timed out after {0}s")]
    TradeTimeout(u64),

    #[error("Trade rejected: {0}")]
    TradeRejected(String),

    #[error("Bot operation failed: {0}")]
    BotError(String),

    #[error("Validation failed: {0}")]
    ValidationError(String),

    #[error("Chest operation failed: {0}")]
    ChestOp(String),

    #[error("Plan infeasible: {0}")]
    PlanInfeasible(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<StoreError> for String {
    fn from(err: StoreError) -> Self {
        err.to_string()
    }
}
