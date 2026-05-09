//! # Structured error types for store operations
//!
//! Historically every handler returned `Result<T, String>`, which made error
//! categorization impossible: call sites could not distinguish "user typed a
//! bad item name" from "bot crashed mid-trade". `StoreError` is the typed
//! hierarchy so higher-level code can match on the cause and react
//! appropriately (retry, notify player, escalate to operator, etc.).
//!
//! There is intentionally **no** `From<StoreError> for String` and **no**
//! `From<String> for StoreError`: the former silently smuggled raw error
//! text (including stringified `reqwest::Error` content) through `?` into
//! `Result<_, String>` boundaries, defeating `user_message()`'s
//! sanitization; the latter would collapse every legacy error into
//! `ValidationError` regardless of its real category, hiding the migration
//! work we already did to give errors meaningful types. Player-facing
//! rendering goes through [`StoreError::user_message`] (preferably via
//! [`crate::store::utils::whisper_error_to_player`]); cross-boundary
//! conversion from typed Mojang-resolver errors goes through
//! [`From<MojangResolveError> for StoreError`].

use std::borrow::Cow;

use thiserror::Error;

use crate::types::user::MojangResolveError;

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

    /// Trade GUI handoff timed out; `after_ms` is the timeout duration in **milliseconds**.
    #[error("Trade timed out after {after_ms}ms")]
    TradeTimeout { after_ms: u64 },

    /// Chest open/close (interact-with-chest-and-sync) operation timed out; `after_ms` is the timeout duration in **milliseconds**.
    #[error("Chest operation timed out after {after_ms}ms")]
    ChestTimeout { after_ms: u64 },

    /// Outer-timeout fired while awaiting a oneshot ack from the bot for a
    /// non-trade/non-chest instruction (currently: whisper send). The inner
    /// string is a short call-site context tag (e.g. `"whisper ack"`) used
    /// for log triage; it is not user-facing. Distinct from
    /// `BotResponseDropped` (channel closed, bot likely crashed) and
    /// `BotDisconnected` (mpsc unavailable): this fires when the bot accepted
    /// the instruction but never produced a reply within the budget.
    #[error("Bot ack timed out: {0}")]
    BotAckTimeout(String),

    /// Bot returned a structured trade-failure reason.
    #[error("Trade rejected: {0}")]
    TradeRejected(String),

    /// `bot_tx.send(...)` mpsc `SendError` — the channel to the bot is closed
    /// (bot task panicked or already shut down). Distinct from
    /// `BotResponseDropped` (oneshot side) and `BotReportedError`
    /// (bot returned a structured failure).
    #[error("Failed to send instruction to bot: {0}")]
    BotSendFailed(String),

    /// `oneshot::Receiver` `RecvError` — the bot dropped the response
    /// `Sender` before sending a reply (typically because the bot task
    /// crashed mid-operation). Distinct from `BotSendFailed` (mpsc side)
    /// and `BotReportedError` (bot returned a structured failure).
    #[error("Bot response channel dropped: {0}")]
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

    /// Mojang resolver failed below the `NotFound`/`InvalidShape` boundary —
    /// a network, timeout, upstream-status, or malformed-response error. The
    /// inner string is the short author-controlled `Display` of
    /// `MojangResolveError`; it is operator-visible only via
    /// `Display`/logs, never via `user_message()` (which collapses to the
    /// generic player-safe string). Distinct from `ValidationError` so the
    /// "garbage username typed by the player" branch can be passed through
    /// while the "Mojang glitched" branch is sanitized.
    #[error("Mojang resolver failed: {0}")]
    MojangNetwork(String),

    /// Mojang reported the username does not exist (HTTP 204) — a known,
    /// safe-to-whisper player-facing condition. The inner `username` is
    /// the original user-supplied input and is rendered into the whisper
    /// verbatim ("Player 'X' not found"). Distinct from
    /// `MojangNetwork`/`ValidationError` so callers can branch on the
    /// "no account yet" / lookup-target-missing case without substring
    /// matching on error text.
    #[error("Player '{username}' not found")]
    UserNotFound { username: String },

    /// Bot reported a chest action failure (after timeouts have been re-routed to `ChestTimeout`).
    #[error("Chest operation failed: {0}")]
    ChestOp(String),

    /// Free-form "should never happen" runtime invariant breach.
    #[error("Invariant violation: {0}")]
    InvariantViolation(String),

    /// Public coercion point: any handler that gains an `io::Result` can `?`-propagate it directly.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl StoreError {
    /// Sanitized, player-facing rendering of this error.
    ///
    /// Distinct from `Display` (which is the full diagnostic string used for
    /// logs): variants whose inner data is author-controlled and known to be
    /// safe to whisper verbatim (`ValidationError`, `TradeRejected`,
    /// `ChestOp`) pass their inner string through; every other variant
    /// collapses to a generic message so internal call-site identifiers
    /// (e.g. `"pay/payer-balance"`) and transport-level details never leak
    /// to players.
    ///
    /// **Prefer [`crate::store::utils::whisper_error_to_player`] over
    /// calling this directly.** The helper is the canonical "tell the player
    /// about a `StoreError`" path; routing every player notification through
    /// it makes the sanitization discipline grep-able from a single name.
    pub fn user_message(&self) -> Cow<'_, str> {
        const GENERIC: &str = "Internal error. Please retry; the operator has been notified.";
        match self {
            // Pass-through variants borrow their inner text (no clone) —
            // callers that need an owned String can `.into_owned()` the Cow.
            StoreError::ValidationError(s)
            | StoreError::TradeRejected(s)
            | StoreError::ChestOp(s) => Cow::Borrowed(s.as_str()),
            // `UserNotFound` is the one Mojang-resolver outcome whose inner
            // text is safe to whisper verbatim — the username comes from
            // the player's own input.
            StoreError::UserNotFound { username } => {
                Cow::Owned(format!("Player '{username}' not found"))
            }
            StoreError::UnknownPair { .. }
            | StoreError::UnknownUser { .. }
            | StoreError::InvariantViolation(_)
            | StoreError::BotSendFailed(_)
            | StoreError::BotResponseDropped(_)
            | StoreError::BotReportedError(_)
            | StoreError::Io(_)
            | StoreError::TradeTimeout { .. }
            | StoreError::ChestTimeout { .. }
            | StoreError::BotAckTimeout(_)
            | StoreError::BotDisconnected
            // `MojangNetwork` collapses to GENERIC: it represents an
            // operator-visible upstream failure, not anything the player
            // can act on. Display still carries the typed reason for
            // logs / `whisper_error_to_player` audit trails.
            | StoreError::MojangNetwork(_) => Cow::Borrowed(GENERIC),
        }
    }
}

/// Single canonical mapping from a typed Mojang-resolver error to a
/// `StoreError`. Every store-layer call site that funnels a Mojang lookup
/// into the error type goes through this conversion so the routing rules
/// stay grep-able from one place:
/// - `NotFound` → `UserNotFound` (player-safe whisper, name passed through)
/// - `InvalidShape` → `ValidationError` (player typed garbage, tell them)
/// - everything else (network / timeout / upstream / decode / rate-limited) →
///   `MojangNetwork` (operator-visible Display only; player gets the
///   generic sanitized whisper from `user_message()`). The new
///   `RateLimited` variant routes here too so callers that want to
///   distinguish "retry later" from "upstream broken" can match on the
///   typed `MojangResolveError` directly *before* the conversion; once
///   collapsed into `StoreError::MojangNetwork`, the retry-after hint is
///   only available through the inner `Display` text.
impl From<MojangResolveError> for StoreError {
    fn from(err: MojangResolveError) -> Self {
        match err {
            MojangResolveError::NotFound { username } => {
                StoreError::UserNotFound { username }
            }
            MojangResolveError::InvalidShape => {
                StoreError::ValidationError("Invalid Minecraft username".to_string())
            }
            other @ (MojangResolveError::NetworkTimeout
            | MojangResolveError::NetworkError
            | MojangResolveError::UpstreamError
            | MojangResolveError::MalformedResponse
            | MojangResolveError::RateLimited { .. }) => {
                StoreError::MojangNetwork(other.to_string())
            }
        }
    }
}

