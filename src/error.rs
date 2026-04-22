//! # Structured error types for store operations
//!
//! Historically every handler returned `Result<T, String>`, which made error
//! categorization impossible: call sites could not distinguish "user typed a
//! bad item name" from "bot crashed mid-trade". `StoreError` is the typed
//! hierarchy so higher-level code can match on the cause and react
//! appropriately (retry, notify player, escalate to operator, etc.).
//!
//! `From<StoreError> for String` is the one-way bridge that lets handlers
//! still return `Result<(), String>` at the outermost boundary (what the bot
//! whisper pipeline expects) without forcing every call site to stringify by
//! hand. There is intentionally **no** `From<String> for StoreError` — a
//! conversion in that direction would silently collapse every legacy error
//! into `ValidationError` regardless of its real category, hiding the
//! migration work we already did to give errors meaningful types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("Unknown pair '{item}' (invariant violation at {context})")]
    UnknownPair { item: String, context: &'static str },

    #[error("Unknown user '{uuid}' (invariant violation at {context})")]
    UnknownUser { uuid: String, context: &'static str },

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

    #[error("{0}")]
    InvariantViolation(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<StoreError> for String {
    fn from(err: StoreError) -> Self {
        err.to_string()
    }
}
