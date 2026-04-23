//! Core domain types.
//!
//! Each submodule owns one data type together with its JSON persistence logic
//! (users, pairs, and nodes are per-entity files under `data/`; orders share a
//! single pruned file; trades are one file per executed trade). This facade
//! re-exports the public types so callers can use `crate::types::Foo` without
//! reaching into the submodule path.

pub mod chest;
pub mod item_id;
pub mod node;
pub mod order;
pub mod pair;
pub mod position;
pub mod storage;
pub mod trade;
pub mod user;

pub use chest::Chest;
pub use item_id::ItemId;
pub use node::Node;
pub use order::Order;
pub use pair::Pair;
pub use position::Position;
pub use storage::Storage;
pub use trade::Trade;
pub use trade::TradeType;
pub use user::User;
