//! # Formal state machine for trade lifecycle
//!
//! Trade states (Queued -> Processing -> Trading -> Committed/RolledBack) used
//! to be implicit in code flow.  This module encodes them as an enum so that:
//!
//! - The current phase of an in-flight trade is always inspectable (status
//!   commands, debug logs, stuck-order diagnostics).
//! - Transition functions consume the previous phase's data and produce the
//!   next, making the intended flow explicit in the type system.
//! - Invalid transitions (e.g. Committing -> Queued) cannot be expressed
//!   through the provided API.
//!
//! ## Lifecycle
//!
//! ```text
//!   Queued ─► Withdrawing ─► Trading ─► Depositing ─► Committed
//!               │               │           │
//!               └───────────────┴───────────┴──► RolledBack
//! ```

use std::fmt;

use crate::messages::TradeItem;
use crate::store::queue::QueuedOrder;
use crate::types::storage::ChestTransfer;

// =========================================================================
// Supporting types
// =========================================================================

/// Items the bot actually received from the player during a trade GUI exchange.
#[derive(Debug, Clone)]
pub struct TradeResult {
    /// Items received from the player (may differ from what was requested).
    #[allow(dead_code)] // carried for diagnostics/logging via Debug
    pub items_received: Vec<TradeItem>,
}

/// Summary of a fully committed trade (terminal state).
#[derive(Debug, Clone)]
pub struct CompletedTrade {
    pub order: QueuedOrder,
    /// Canonical item id that was traded.
    pub item: String,
    /// Quantity of items traded.
    pub quantity: i32,
    /// Total currency (diamonds) involved.
    pub currency_amount: f64,
}

// =========================================================================
// State enum
// =========================================================================

/// The phase an in-flight trade is currently in.
///
/// Stored on `Store::current_trade` while an order is being processed so that
/// status commands, debug logging, and stuck-order diagnostics can report
/// exactly where the trade is.
#[derive(Debug, Clone)]
pub enum TradeState {
    /// Order was just popped from the queue; validation / planning has not
    /// started yet.
    Queued(QueuedOrder),

    /// Bot is executing chest transfers to prepare for the trade (e.g.
    /// withdrawing items for a buy, or withdrawing diamonds for a sell).
    Withdrawing {
        order: QueuedOrder,
        plan: Vec<ChestTransfer>,
    },

    /// Bot has opened the trade GUI with the player and is waiting for the
    /// player to confirm.
    Trading {
        order: QueuedOrder,
        #[allow(dead_code)] // carried for rollback context via Debug
        withdrawn: Vec<ChestTransfer>,
    },

    /// Trade GUI completed; bot is depositing received items/diamonds back
    /// into storage before committing ledgers.
    Depositing {
        order: QueuedOrder,
        #[allow(dead_code)] // carried for diagnostics via Debug
        trade_result: TradeResult,
        #[allow(dead_code)] // carried for diagnostics via Debug
        deposit_plan: Vec<ChestTransfer>,
    },

    /// Ledgers updated, trade recorded.  Terminal state.
    Committed(CompletedTrade),

    /// Trade failed at some phase and was rolled back.  Terminal state.
    #[allow(dead_code)] // used in tests; handlers currently set this via advance_trade
    RolledBack {
        order: QueuedOrder,
        reason: String,
    },
}

// =========================================================================
// Transitions
// =========================================================================

impl TradeState {
    // -- constructors (entry points) --------------------------------------

    /// Create the initial state when an order is popped from the queue.
    pub fn new(order: QueuedOrder) -> Self {
        TradeState::Queued(order)
    }

    // -- forward transitions ----------------------------------------------

    /// Queued -> Withdrawing.
    ///
    /// Called once validation succeeds and a chest-transfer plan is ready.
    /// Consumes the `Queued` data so the caller cannot accidentally re-use it.
    pub fn begin_withdrawal(self, plan: Vec<ChestTransfer>) -> Self {
        match self {
            TradeState::Queued(order) => {
                tracing::info!(
                    order_id = order.id,
                    player = %order.username,
                    phase = "withdrawing",
                    "TradeState transition"
                );
                TradeState::Withdrawing { order, plan }
            }
            other => panic!(
                "TradeState::begin_withdrawal called from invalid state: {}",
                other.phase()
            ),
        }
    }

    /// Withdrawing -> Trading.
    ///
    /// Called once all chest withdrawals complete and the bot is about to open
    /// the trade GUI.
    pub fn begin_trading(self) -> Self {
        match self {
            TradeState::Withdrawing { order, plan } => {
                tracing::info!(
                    order_id = order.id,
                    player = %order.username,
                    phase = "trading",
                    "TradeState transition"
                );
                TradeState::Trading {
                    order,
                    withdrawn: plan,
                }
            }
            other => panic!(
                "TradeState::begin_trading called from invalid state: {}",
                other.phase()
            ),
        }
    }

    /// Trading -> Depositing.
    ///
    /// Called once the trade GUI completes and the bot needs to deposit
    /// received items/diamonds into storage.
    pub fn begin_depositing(self, trade_result: TradeResult, deposit_plan: Vec<ChestTransfer>) -> Self {
        match self {
            TradeState::Trading { order, .. } => {
                tracing::info!(
                    order_id = order.id,
                    player = %order.username,
                    phase = "depositing",
                    "TradeState transition"
                );
                TradeState::Depositing {
                    order,
                    trade_result,
                    deposit_plan,
                }
            }
            other => panic!(
                "TradeState::begin_depositing called from invalid state: {}",
                other.phase()
            ),
        }
    }

    /// Trading | Depositing -> Committed.
    ///
    /// Called after ledgers are updated and the trade is recorded. Accepts
    /// both `Trading` (when there is no post-trade deposit phase, e.g. a buy
    /// where diamonds go straight to balance) and `Depositing`.
    pub fn commit(self, item: String, quantity: i32, currency_amount: f64) -> Self {
        match self {
            TradeState::Trading { order, .. }
            | TradeState::Depositing { order, .. } => {
                tracing::info!(
                    order_id = order.id,
                    player = %order.username,
                    phase = "committed",
                    item = %item,
                    qty = quantity,
                    currency = format_args!("{:.2}", currency_amount),
                    "TradeState transition"
                );
                TradeState::Committed(CompletedTrade {
                    order,
                    item,
                    quantity,
                    currency_amount,
                })
            }
            other => panic!(
                "TradeState::commit called from invalid state: {}",
                other.phase()
            ),
        }
    }

    // -- failure transition -----------------------------------------------

    /// Any non-terminal state -> RolledBack.
    #[allow(dead_code)] // API surface for handlers; used in tests
    pub fn rollback(self, reason: String) -> Self {
        let order = match self {
            TradeState::Queued(order)
            | TradeState::Withdrawing { order, .. }
            | TradeState::Trading { order, .. }
            | TradeState::Depositing { order, .. } => order,
            TradeState::Committed(_) => panic!("Cannot rollback a committed trade"),
            TradeState::RolledBack { .. } => panic!("Trade already rolled back"),
        };
        tracing::info!(
            order_id = order.id,
            player = %order.username,
            phase = "rolled_back",
            reason = %reason,
            "TradeState transition"
        );
        TradeState::RolledBack { order, reason }
    }

    // -- introspection ----------------------------------------------------

    /// Short human-readable label for the current phase.
    pub fn phase(&self) -> &'static str {
        match self {
            TradeState::Queued(_) => "queued",
            TradeState::Withdrawing { .. } => "withdrawing",
            TradeState::Trading { .. } => "trading",
            TradeState::Depositing { .. } => "depositing",
            TradeState::Committed(_) => "committed",
            TradeState::RolledBack { .. } => "rolled_back",
        }
    }

    /// True if the trade has reached a terminal state.
    #[allow(dead_code)] // API surface; used in tests
    pub fn is_terminal(&self) -> bool {
        matches!(self, TradeState::Committed(_) | TradeState::RolledBack { .. })
    }

    /// Reference to the underlying order (available in every non-committed state).
    pub fn order(&self) -> &QueuedOrder {
        match self {
            TradeState::Queued(o)
            | TradeState::Withdrawing { order: o, .. }
            | TradeState::Trading { order: o, .. }
            | TradeState::Depositing { order: o, .. }
            | TradeState::RolledBack { order: o, .. } => o,
            TradeState::Committed(c) => &c.order,
        }
    }
}

impl fmt::Display for TradeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TradeState::Queued(o) => write!(f, "Queued: {}", o.description()),
            TradeState::Withdrawing { order, .. } => {
                write!(f, "Withdrawing for: {}", order.description())
            }
            TradeState::Trading { order, .. } => {
                write!(f, "Trading with player: {}", order.description())
            }
            TradeState::Depositing { order, .. } => {
                write!(f, "Depositing after: {}", order.description())
            }
            TradeState::Committed(c) => {
                write!(f, "Committed: {}x {} ({:.2} diamonds)", c.quantity, c.item, c.currency_amount)
            }
            TradeState::RolledBack { order, reason } => {
                write!(f, "Rolled back {}: {}", order.description(), reason)
            }
        }
    }
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::QueuedOrderType;

    fn sample_order() -> QueuedOrder {
        QueuedOrder::new(
            1,
            "uuid-1234".to_string(),
            "TestPlayer".to_string(),
            QueuedOrderType::Buy,
            "cobblestone".to_string(),
            64,
        )
    }

    fn sample_transfers() -> Vec<ChestTransfer> {
        vec![ChestTransfer {
            chest_id: 0,
            position: crate::types::Position { x: 0, y: 64, z: 0 },
            item: crate::types::ItemId::from_normalized("cobblestone".to_string()),
            amount: 64,
        }]
    }

    #[test]
    fn happy_path_buy_no_deposit() {
        // Queued -> Withdrawing -> Trading -> Committed
        let state = TradeState::new(sample_order());
        assert_eq!(state.phase(), "queued");
        assert!(!state.is_terminal());

        let state = state.begin_withdrawal(sample_transfers());
        assert_eq!(state.phase(), "withdrawing");

        let state = state.begin_trading();
        assert_eq!(state.phase(), "trading");

        let state = state.commit("cobblestone".to_string(), 64, 12.5);
        assert_eq!(state.phase(), "committed");
        assert!(state.is_terminal());

        if let TradeState::Committed(c) = &state {
            assert_eq!(c.quantity, 64);
            assert_eq!(c.item, "cobblestone");
            assert!((c.currency_amount - 12.5).abs() < f64::EPSILON);
        } else {
            panic!("Expected Committed");
        }
    }

    #[test]
    fn happy_path_sell_with_deposit() {
        // Queued -> Withdrawing -> Trading -> Depositing -> Committed
        let state = TradeState::new(sample_order());
        let state = state.begin_withdrawal(sample_transfers());
        let state = state.begin_trading();

        let result = TradeResult {
            items_received: vec![TradeItem {
                item: "cobblestone".to_string(),
                amount: 64,
            }],
        };
        let state = state.begin_depositing(result, sample_transfers());
        assert_eq!(state.phase(), "depositing");

        let state = state.commit("cobblestone".to_string(), 64, 8.0);
        assert!(state.is_terminal());
    }

    #[test]
    fn rollback_from_withdrawing() {
        let state = TradeState::new(sample_order());
        let state = state.begin_withdrawal(sample_transfers());
        let state = state.rollback("chest timeout".to_string());

        assert_eq!(state.phase(), "rolled_back");
        assert!(state.is_terminal());
        if let TradeState::RolledBack { reason, .. } = &state {
            assert_eq!(reason, "chest timeout");
        }
    }

    #[test]
    fn rollback_from_trading() {
        let state = TradeState::new(sample_order());
        let state = state.begin_withdrawal(sample_transfers());
        let state = state.begin_trading();
        let state = state.rollback("trade rejected by player".to_string());

        assert_eq!(state.phase(), "rolled_back");
    }

    #[test]
    fn rollback_from_depositing() {
        let state = TradeState::new(sample_order());
        let state = state.begin_withdrawal(sample_transfers());
        let state = state.begin_trading();
        let result = TradeResult { items_received: vec![] };
        let state = state.begin_depositing(result, vec![]);
        let state = state.rollback("deposit failed".to_string());

        assert_eq!(state.phase(), "rolled_back");
    }

    #[test]
    #[should_panic(expected = "invalid state")]
    fn cannot_trade_from_queued() {
        let state = TradeState::new(sample_order());
        let _ = state.begin_trading(); // skip Withdrawing -> panic
    }

    #[test]
    #[should_panic(expected = "invalid state")]
    fn cannot_commit_from_queued() {
        let state = TradeState::new(sample_order());
        let _ = state.commit("x".to_string(), 1, 1.0);
    }

    #[test]
    #[should_panic(expected = "Cannot rollback a committed trade")]
    fn cannot_rollback_committed() {
        let state = TradeState::new(sample_order());
        let state = state.begin_withdrawal(sample_transfers());
        let state = state.begin_trading();
        let state = state.commit("cobblestone".to_string(), 64, 10.0);
        let _ = state.rollback("oops".to_string());
    }

    #[test]
    #[should_panic(expected = "already rolled back")]
    fn cannot_double_rollback() {
        let state = TradeState::new(sample_order());
        let state = state.rollback("first".to_string());
        let _ = state.rollback("second".to_string());
    }

    #[test]
    fn order_accessible_from_all_states() {
        let state = TradeState::new(sample_order());
        assert_eq!(state.order().id, 1);

        let state = state.begin_withdrawal(sample_transfers());
        assert_eq!(state.order().id, 1);

        let state = state.begin_trading();
        assert_eq!(state.order().username, "TestPlayer");

        let state = state.commit("cobblestone".to_string(), 64, 10.0);
        assert_eq!(state.order().id, 1);
    }

    #[test]
    fn display_formatting() {
        let state = TradeState::new(sample_order());
        assert!(state.to_string().contains("Queued"));

        let state = state.begin_withdrawal(sample_transfers());
        assert!(state.to_string().contains("Withdrawing"));

        let state = state.rollback("timeout".to_string());
        assert!(state.to_string().contains("Rolled back"));
        assert!(state.to_string().contains("timeout"));
    }
}
