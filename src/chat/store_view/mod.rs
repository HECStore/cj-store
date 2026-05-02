//! Chat-side read-only views over the on-disk store data.
//!
//! Why a separate module: the chat composer must answer questions like
//! "what's the latest iron trade?" or "what's my balance?" without
//! depending on `crate::types::*`. Those types pull in `reqwest` (Mojang
//! HTTP), `crate::fsutil::write_atomic`, and orphan-deletion semantics in
//! `save_*` — chat must not reach any of that, even transitively.
//!
//! What lives here: minimal `*View` structs that deserialize the same JSON
//! shapes the trade bot writes, plus filename-level scanning helpers
//! tuned for the chat use cases (filename-prune by `since`, deny on
//! invalid item names).
//!
//! Why CWD-relative paths instead of taking a `data_root: PathBuf` from
//! `ToolContext`: chat runs in the same process as the trade bot, with
//! the same CWD. Threading a path would be theatre — both sides resolve
//! the same `data/` either way. Keeping the constants local to this
//! module preserves the *conceptual* isolation (no chat code imports
//! from `crate::types::`) without inventing a parameter that adds no
//! safety.
//!
//! Operator status redaction (hard rule, see `CHAT_STORE_TOOLS_PLAN.md`):
//! [`user::UserView`] does NOT deserialize the `operator` field. Tools
//! returning a balance hand back a `UserView` directly, so leaking
//! operator status through the chat surface is impossible at the type
//! level — even if a future serialization adds new fields, none of them
//! can be `operator`.

pub mod pair;
pub mod trade;
pub mod user;

#[cfg(test)]
mod tests;
