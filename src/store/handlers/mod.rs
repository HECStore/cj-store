//! Message handlers for the Store.
//!
//! Dispatches incoming messages to the appropriate handler:
//! player commands, operator commands, and CLI commands.

pub mod player;
pub mod operator;
pub mod cli;

mod buy;
mod sell;
mod deposit;
mod withdraw;
mod info;
mod validation;
