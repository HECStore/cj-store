//! Message handlers for the Store.
//!
//! Two layers:
//! - Dispatchers (`player`, `operator`, `cli`) are the public entry points
//!   called from `store::mod` (chat/operator messages) and the CLI loop. They
//!   parse/route a message and delegate to the per-command modules below.
//! - Command modules (`buy`, `sell`, `deposit`, `withdraw`, `info`) hold the
//!   actual business logic, operating on `Store` state via `store::state` and
//!   helpers from `store::utils` / `store::pricing`.
//!
//! `validation` is shared by the command parser (`store::command`) and
//! handlers; it is `pub(crate)` so both can reach it.

pub mod player;
pub mod operator;
pub mod cli;

mod buy;
mod sell;
mod deposit;
mod withdraw;
mod info;
pub(crate) mod validation;
