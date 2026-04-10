//! # Core Data Types
//!
//! This module contains all the core data types used throughout the cj-store application.
//!
//! ## Type Overview
//!
//! - **[`Position`]**: 3D world coordinates (x, y, z)
//! - **[`Chest`]**: Individual storage chest with 54 shulker box slots
//! - **[`Node`]**: Storage unit containing 4 chests in a 2x2 arrangement
//! - **[`Storage`]**: Complete storage system managing all nodes
//! - **[`Pair`]**: Trading pair (item <-> diamonds) with reserve-based pricing
//! - **[`Order`]**: Audit log entry for transactions
//! - **[`Trade`]**: Executed trade record with timestamp
//! - **[`User`]**: Player account with UUID, balance, and operator status
//!
//! ## Persistence
//!
//! Each type handles its own file-based persistence:
//! - Users: `data/users/{uuid}.json`
//! - Pairs: `data/pairs/{item}.json`
//! - Nodes: `data/storage/{node_id}.json`
//! - Orders: `data/orders.json` (single file, session-only, pruned to 10K)
//! - Trades: `data/trades/{timestamp}.json` (one file per trade)

pub mod chest;
pub mod node;
pub mod order;
pub mod pair;
pub mod position;
pub mod storage;
pub mod trade;
pub mod user;

// Re-export types for convenience so consumers can write `crate::types::Foo`
// instead of reaching into each submodule directly.
pub use chest::Chest;
// `Node` is accessed through `storage::Node` in most of the codebase and only
// referenced via this re-export from tests, so the compiler flags it as unused
// in non-test builds. Keep it exported for API consistency with the other types.
#[allow(unused_imports)]
pub use node::Node;
pub use order::Order;
pub use pair::Pair;
pub use position::Position;
pub use storage::Storage;
pub use trade::Trade;
pub use trade::TradeType;
pub use user::User;
