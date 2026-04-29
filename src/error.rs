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
    /// Invariant violation: caller asserted a pair exists by item slug; `context` is a static call-site identifier (e.g. `"buy/stock-check"`) used only for log triage, never user-facing.
    #[error("Unknown pair '{item}' (invariant violation at {context})")]
    UnknownPair { item: String, context: &'static str },

    /// Invariant violation: caller asserted a user exists by uuid; `context` semantics same as `UnknownPair`.
    #[error("Unknown user '{uuid}' (invariant violation at {context})")]
    UnknownUser { uuid: String, context: &'static str },

    /// Bot is offline / RPC channel unavailable.
    #[error("Bot not connected")]
    BotDisconnected,

    /// Trade or chest op timed out; `after_ms` is the timeout duration in **milliseconds**.
    #[error("Trade timed out after {after_ms}ms")]
    TradeTimeout { after_ms: u64 },

    /// Bot returned a structured trade-failure reason.
    #[error("Trade rejected: {0}")]
    TradeRejected(String),

    /// `bot_tx.send(...)` mpsc `SendError` — the channel to the bot is closed
    /// (bot task panicked or already shut down). Distinct from
    /// `BotResponseDropped` (oneshot side) and `BotReportedError`
    /// (bot returned a structured failure).
    #[error("Bot operation failed: {0}")]
    BotSendFailed(String),

    /// `oneshot::Receiver` `RecvError` — the bot dropped the response
    /// `Sender` before sending a reply (typically because the bot task
    /// crashed mid-operation). Distinct from `BotSendFailed` (mpsc side)
    /// and `BotReportedError` (bot returned a structured failure).
    #[error("Bot operation failed: {0}")]
    BotResponseDropped(String),

    /// Bot completed the round-trip but returned a structured `Err(String)`
    /// in its Whisper-response payload — i.e. the bot itself reported the
    /// failure reason. Distinct from `BotSendFailed` /
    /// `BotResponseDropped`, which are transport-layer failures.
    #[error("Bot operation failed: {0}")]
    BotReportedError(String),

    /// Player-facing input validation failure (rendered to whisper).
    #[error("Validation failed: {0}")]
    ValidationError(String),

    /// Bot reported a chest action failure (after timeouts have been re-routed to `TradeTimeout`).
    #[error("Chest operation failed: {0}")]
    ChestOp(String),

    /// Free-form "should never happen" runtime invariant breach.
    #[error("Invariant violation: {0}")]
    InvariantViolation(String),

    /// Public coercion point: any handler that gains an `io::Result` can `?`-propagate it directly.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<StoreError> for String {
    fn from(err: StoreError) -> Self {
        err.to_string()
    }
}
