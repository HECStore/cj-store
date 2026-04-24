//! Formal state machine for the trade lifecycle.
//!
//! ```text
//!   Queued ─► Withdrawing ─► Trading ─► Depositing ─► Committed
//!                              │  └───────────────────►    ▲
//!                              │    (skip Depositing when  │
//!                              │     the payout goes       │
//!                              │     straight to balance)  │
//!               ─────────────┴───────────┴──► RolledBack
//! ```
//!
//! `Trading → Committed` is deliberately allowed: buys whose diamonds go
//! straight to the user balance have no post-trade chest work and bypass
//! `Depositing`. `commit()` therefore accepts either `Trading` or
//! `Depositing` as its predecessor.

use std::fmt;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::messages::TradeItem;
use crate::store::queue::QueuedOrder;
use crate::types::storage::ChestTransfer;

/// Items the bot actually received from the player during the GUI exchange.
///
/// Retained past the `Depositing` phase purely for diagnostics (Debug output
/// on stuck-trade reports, and the persisted crash-resume file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeResult {
    #[allow(dead_code)] // carried for diagnostics/logging via Debug
    pub items_received: Vec<TradeItem>,
}

/// Summary of a fully committed trade (terminal state).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedTrade {
    pub order: QueuedOrder,
    pub item: String,
    pub quantity: i32,
    /// Total diamonds involved (positive for both buys and sells; direction
    /// of flow is implicit in `order.order_type`).
    pub currency_amount: f64,
}

/// The phase an in-flight trade is currently in.
///
/// Stored on `Store::current_trade` while an order is being processed so that
/// status commands and stuck-order diagnostics can report exactly where the
/// trade is. Transition methods consume `self` so the stale previous-phase
/// value cannot be accidentally re-used.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TradeState {
    Queued(QueuedOrder),

    Withdrawing {
        order: QueuedOrder,
        plan: Vec<ChestTransfer>,
    },

    Trading {
        order: QueuedOrder,
        #[allow(dead_code)] // carried for rollback context via Debug
        withdrawn: Vec<ChestTransfer>,
    },

    Depositing {
        order: QueuedOrder,
        #[allow(dead_code)] // carried for diagnostics via Debug
        trade_result: TradeResult,
        #[allow(dead_code)] // carried for diagnostics via Debug
        deposit_plan: Vec<ChestTransfer>,
    },

    Committed(CompletedTrade),

    #[allow(dead_code)] // constructed by `rollback()`; read via Debug/serde
    RolledBack {
        order: QueuedOrder,
        reason: String,
    },
}

impl TradeState {
    /// Initial state when an order is popped from the queue.
    pub fn new(order: QueuedOrder) -> Self {
        TradeState::Queued(order)
    }

    /// Queued -> Withdrawing.
    ///
    /// Panics if called from any other state — the state machine is
    /// intentionally total so a misrouted call fails loudly rather than
    /// silently corrupting an in-flight trade.
    pub fn begin_withdrawal(self, plan: Vec<ChestTransfer>) -> Self {
        match self {
            TradeState::Queued(order) => {
                tracing::debug!(
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
    pub fn begin_trading(self) -> Self {
        match self {
            TradeState::Withdrawing { order, plan } => {
                tracing::debug!(
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
    pub fn begin_depositing(self, trade_result: TradeResult, deposit_plan: Vec<ChestTransfer>) -> Self {
        match self {
            TradeState::Trading { order, .. } => {
                tracing::debug!(
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
    /// Accepts `Trading` directly (payout-to-balance trades have no deposit
    /// phase) and `Depositing` (normal case).
    pub fn commit(self, item: String, quantity: i32, currency_amount: f64) -> Self {
        match self {
            TradeState::Trading { order, .. }
            | TradeState::Depositing { order, .. } => {
                tracing::debug!(
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

    /// Any non-terminal state -> RolledBack.
    ///
    /// Panics on a committed or already-rolled-back trade: the caller is
    /// trying to retreat from a terminal state, which indicates a handler
    /// bug, not a recoverable condition.
    #[allow(dead_code)] // API surface; used in tests
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
            "TradeState rolled back"
        );
        TradeState::RolledBack { order, reason }
    }

    /// Short label for the current phase, stable across serialization and
    /// used as a structured log field.
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

    #[allow(dead_code)] // API surface; used in tests
    pub fn is_terminal(&self) -> bool {
        matches!(self, TradeState::Committed(_) | TradeState::RolledBack { .. })
    }

    /// The underlying order, regardless of phase.
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

/// Mirror file for the in-flight trade state. Presence at startup implies
/// the previous session crashed mid-trade; absence is the normal idle case.
pub const TRADE_STATE_FILE: &str = "data/current_trade.json";

/// Atomically write the current trade state to `TRADE_STATE_FILE`.
pub fn persist(state: &TradeState) -> io::Result<()> {
    persist_to(Path::new(TRADE_STATE_FILE), state)
}

/// Load a persisted trade state if present.
///
/// - `Ok(None)` when the file does not exist (normal startup).
/// - `Ok(Some)` when an interrupted trade is found (crash-resume path).
/// - `Err` on IO or deserialization failure.
pub fn load_persisted() -> io::Result<Option<TradeState>> {
    load_persisted_from(Path::new(TRADE_STATE_FILE))
}

/// Remove the persisted trade state file. No-op if it doesn't exist.
pub fn clear_persisted() -> io::Result<()> {
    clear_persisted_from(Path::new(TRADE_STATE_FILE))
}

/// Path-parameterized persist, separated so tests can round-trip without
/// touching the production `TRADE_STATE_FILE`.
fn persist_to(path: &Path, state: &TradeState) -> io::Result<()> {
    let json = serde_json::to_string_pretty(state)
        .map_err(io::Error::other)?;
    crate::fsutil::write_atomic(path, &json)
}

/// Path-parameterized load, separated so tests can round-trip without
/// touching the production `TRADE_STATE_FILE`.
fn load_persisted_from(path: &Path) -> io::Result<Option<TradeState>> {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let state: TradeState = serde_json::from_str(&content)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            Ok(Some(state))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Path-parameterized clear, separated so tests can round-trip without
/// touching the production `TRADE_STATE_FILE`.
fn clear_persisted_from(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

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

    fn sample_trade_result() -> TradeResult {
        TradeResult {
            items_received: vec![TradeItem {
                item: "cobblestone".to_string(),
                amount: 64,
            }],
        }
    }

    // --- happy-path transitions ------------------------------------------

    #[test]
    fn trading_commits_directly_when_payout_goes_to_balance() {
        // Queued -> Withdrawing -> Trading -> Committed (no Depositing).
        let state = TradeState::new(sample_order());
        assert_eq!(state.phase(), "queued");
        assert!(!state.is_terminal());

        let state = state.begin_withdrawal(sample_transfers());
        assert_eq!(state.phase(), "withdrawing");
        assert!(!state.is_terminal());

        let state = state.begin_trading();
        assert_eq!(state.phase(), "trading");
        assert!(!state.is_terminal());

        let state = state.commit("cobblestone".to_string(), 64, 12.5);
        assert_eq!(state.phase(), "committed");
        assert!(state.is_terminal());

        if let TradeState::Committed(c) = &state {
            assert_eq!(c.order.id, 1);
            assert_eq!(c.quantity, 64);
            assert_eq!(c.item, "cobblestone");
            assert!((c.currency_amount - 12.5).abs() < f64::EPSILON);
        } else {
            panic!("Expected Committed");
        }
    }

    #[test]
    fn depositing_commits_after_post_trade_chest_work() {
        // Queued -> Withdrawing -> Trading -> Depositing -> Committed.
        let state = TradeState::new(sample_order())
            .begin_withdrawal(sample_transfers())
            .begin_trading()
            .begin_depositing(sample_trade_result(), sample_transfers());
        assert_eq!(state.phase(), "depositing");
        assert!(!state.is_terminal());

        let state = state.commit("cobblestone".to_string(), 64, 8.0);
        assert_eq!(state.phase(), "committed");
        assert!(state.is_terminal());
    }

    #[test]
    fn withdrawing_preserves_order_and_plan() {
        let state = TradeState::new(sample_order()).begin_withdrawal(sample_transfers());
        if let TradeState::Withdrawing { order, plan } = &state {
            assert_eq!(order.id, 1);
            assert_eq!(order.username, "TestPlayer");
            assert_eq!(plan.len(), 1);
            assert_eq!(plan[0].amount, 64);
        } else {
            panic!("Expected Withdrawing");
        }
    }

    #[test]
    fn trading_carries_withdrawn_plan_for_rollback() {
        let state = TradeState::new(sample_order())
            .begin_withdrawal(sample_transfers())
            .begin_trading();
        if let TradeState::Trading { withdrawn, .. } = &state {
            assert_eq!(withdrawn.len(), 1, "withdrawn plan must survive into Trading so rollback can reverse it");
        } else {
            panic!("Expected Trading");
        }
    }

    // --- rollback branches ------------------------------------------------

    #[test]
    fn rollback_from_queued_captures_reason() {
        let state = TradeState::new(sample_order()).rollback("cancelled before withdraw".to_string());
        assert_eq!(state.phase(), "rolled_back");
        assert!(state.is_terminal());
        if let TradeState::RolledBack { reason, order } = &state {
            assert_eq!(reason, "cancelled before withdraw");
            assert_eq!(order.id, 1);
        } else {
            panic!("Expected RolledBack");
        }
    }

    #[test]
    fn rollback_from_withdrawing_preserves_order() {
        let state = TradeState::new(sample_order())
            .begin_withdrawal(sample_transfers())
            .rollback("chest timeout".to_string());
        assert_eq!(state.phase(), "rolled_back");
        assert_eq!(state.order().id, 1);
        if let TradeState::RolledBack { reason, .. } = &state {
            assert_eq!(reason, "chest timeout");
        }
    }

    #[test]
    fn rollback_from_trading_preserves_order() {
        let state = TradeState::new(sample_order())
            .begin_withdrawal(sample_transfers())
            .begin_trading()
            .rollback("trade rejected by player".to_string());
        assert_eq!(state.phase(), "rolled_back");
        assert_eq!(state.order().id, 1);
    }

    #[test]
    fn rollback_from_depositing_preserves_order() {
        let state = TradeState::new(sample_order())
            .begin_withdrawal(sample_transfers())
            .begin_trading()
            .begin_depositing(sample_trade_result(), sample_transfers())
            .rollback("deposit failed".to_string());
        assert_eq!(state.phase(), "rolled_back");
        assert_eq!(state.order().id, 1);
    }

    // --- invalid-transition guards ---------------------------------------

    #[test]
    #[should_panic(expected = "begin_trading called from invalid state: queued")]
    fn cannot_skip_withdrawing() {
        let _ = TradeState::new(sample_order()).begin_trading();
    }

    #[test]
    #[should_panic(expected = "begin_depositing called from invalid state: queued")]
    fn cannot_deposit_from_queued() {
        let _ = TradeState::new(sample_order())
            .begin_depositing(sample_trade_result(), sample_transfers());
    }

    #[test]
    #[should_panic(expected = "begin_depositing called from invalid state: withdrawing")]
    fn cannot_deposit_from_withdrawing() {
        let _ = TradeState::new(sample_order())
            .begin_withdrawal(sample_transfers())
            .begin_depositing(sample_trade_result(), sample_transfers());
    }

    #[test]
    #[should_panic(expected = "commit called from invalid state: queued")]
    fn cannot_commit_from_queued() {
        let _ = TradeState::new(sample_order()).commit("x".to_string(), 1, 1.0);
    }

    #[test]
    #[should_panic(expected = "commit called from invalid state: withdrawing")]
    fn cannot_commit_from_withdrawing() {
        let _ = TradeState::new(sample_order())
            .begin_withdrawal(sample_transfers())
            .commit("x".to_string(), 1, 1.0);
    }

    #[test]
    #[should_panic(expected = "begin_withdrawal called from invalid state: withdrawing")]
    fn cannot_re_enter_withdrawing() {
        let _ = TradeState::new(sample_order())
            .begin_withdrawal(sample_transfers())
            .begin_withdrawal(sample_transfers());
    }

    #[test]
    #[should_panic(expected = "Cannot rollback a committed trade")]
    fn cannot_rollback_committed() {
        let state = TradeState::new(sample_order())
            .begin_withdrawal(sample_transfers())
            .begin_trading()
            .commit("cobblestone".to_string(), 64, 10.0);
        let _ = state.rollback("oops".to_string());
    }

    #[test]
    #[should_panic(expected = "Trade already rolled back")]
    fn cannot_double_rollback() {
        let state = TradeState::new(sample_order()).rollback("first".to_string());
        let _ = state.rollback("second".to_string());
    }

    // --- introspection ----------------------------------------------------

    #[test]
    fn phase_label_matches_each_variant() {
        let order = sample_order();
        assert_eq!(TradeState::Queued(order.clone()).phase(), "queued");
        assert_eq!(
            TradeState::new(order.clone()).begin_withdrawal(sample_transfers()).phase(),
            "withdrawing"
        );
        let trading = TradeState::new(order.clone())
            .begin_withdrawal(sample_transfers())
            .begin_trading();
        assert_eq!(trading.phase(), "trading");
        let depositing = trading.begin_depositing(sample_trade_result(), sample_transfers());
        assert_eq!(depositing.phase(), "depositing");
        let committed = depositing.commit("cobblestone".to_string(), 64, 1.0);
        assert_eq!(committed.phase(), "committed");
        assert_eq!(
            TradeState::new(order).rollback("r".to_string()).phase(),
            "rolled_back"
        );
    }

    #[test]
    fn is_terminal_only_for_committed_and_rolled_back() {
        let order = sample_order();
        assert!(!TradeState::Queued(order.clone()).is_terminal());
        let w = TradeState::new(order.clone()).begin_withdrawal(sample_transfers());
        assert!(!w.is_terminal());
        let t = w.begin_trading();
        assert!(!t.is_terminal());
        let d = t.begin_depositing(sample_trade_result(), sample_transfers());
        assert!(!d.is_terminal());
        assert!(d.commit("cobblestone".to_string(), 64, 1.0).is_terminal());
        assert!(TradeState::new(order).rollback("r".to_string()).is_terminal());
    }

    #[test]
    fn order_accessor_returns_same_order_through_every_phase() {
        let state = TradeState::new(sample_order());
        assert_eq!(state.order().id, 1);
        assert_eq!(state.order().username, "TestPlayer");

        let state = state.begin_withdrawal(sample_transfers());
        assert_eq!(state.order().id, 1);

        let state = state.begin_trading();
        assert_eq!(state.order().username, "TestPlayer");

        let state = state.begin_depositing(sample_trade_result(), sample_transfers());
        assert_eq!(state.order().id, 1);

        let state = state.commit("cobblestone".to_string(), 64, 10.0);
        assert_eq!(state.order().id, 1);
    }

    #[test]
    fn order_accessor_returns_order_from_rolled_back() {
        let state = TradeState::new(sample_order()).rollback("x".to_string());
        assert_eq!(state.order().id, 1);
        assert_eq!(state.order().username, "TestPlayer");
    }

    // --- Display ---------------------------------------------------------

    #[test]
    fn display_labels_each_phase_with_order_description() {
        let state = TradeState::new(sample_order());
        let rendered = state.to_string();
        assert!(rendered.starts_with("Queued:"), "got {rendered}");
        assert!(rendered.contains("buy cobblestone 64"));

        let state = state.begin_withdrawal(sample_transfers());
        let rendered = state.to_string();
        assert!(rendered.starts_with("Withdrawing for:"), "got {rendered}");

        let state = state.begin_trading();
        let rendered = state.to_string();
        assert!(rendered.starts_with("Trading with player:"), "got {rendered}");

        let state = state.begin_depositing(sample_trade_result(), sample_transfers());
        assert!(state.to_string().starts_with("Depositing after:"));

        let state = state.commit("cobblestone".to_string(), 64, 12.5);
        let rendered = state.to_string();
        assert!(rendered.contains("64x cobblestone"));
        assert!(rendered.contains("12.50 diamonds"));
    }

    #[test]
    fn display_rolled_back_includes_reason() {
        let rendered = TradeState::new(sample_order())
            .rollback("timeout".to_string())
            .to_string();
        assert!(rendered.starts_with("Rolled back"), "got {rendered}");
        assert!(rendered.contains("timeout"));
    }

    // --- serde / persistence ---------------------------------------------

    #[test]
    fn serde_roundtrip_preserves_phase_and_order_for_every_variant() {
        let order = sample_order();
        let states = vec![
            TradeState::new(order.clone()),
            TradeState::new(order.clone()).begin_withdrawal(sample_transfers()),
            TradeState::new(order.clone())
                .begin_withdrawal(sample_transfers())
                .begin_trading(),
            TradeState::new(order.clone())
                .begin_withdrawal(sample_transfers())
                .begin_trading()
                .begin_depositing(sample_trade_result(), sample_transfers()),
            TradeState::new(order.clone())
                .begin_withdrawal(sample_transfers())
                .begin_trading()
                .commit("cobblestone".to_string(), 64, 5.0),
            TradeState::new(order).rollback("boom".to_string()),
        ];
        for state in &states {
            let json = serde_json::to_string(state).expect("serialize");
            let decoded: TradeState = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(decoded.phase(), state.phase(), "phase mismatch for {}", state.phase());
            assert_eq!(decoded.order().id, state.order().id);
            assert_eq!(decoded.order().username, state.order().username);
        }
    }

    /// Scratch directory under the system temp dir, mirroring the pattern in
    /// `queue::tests` so trade-state round-trip tests don't collide with each
    /// other or the real `TRADE_STATE_FILE`.
    struct TmpDir(std::path::PathBuf);

    impl TmpDir {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "cj-store-trade-state-{}-{}",
                name,
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).unwrap();
            Self(base)
        }

        fn path(&self, name: &str) -> std::path::PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// End-to-end crash-resume: persist mid-trade via `persist_to`, load back
    /// via `load_persisted_from`, verify equivalence, then clear via
    /// `clear_persisted_from` and confirm a second load returns `Ok(None)`.
    /// Exercises the real `write_atomic`, `NotFound`-swallowing and file-IO
    /// paths that the public wrappers delegate to.
    #[test]
    fn persist_load_clear_roundtrip() {
        let dir = TmpDir::new("roundtrip");
        let path = dir.path("current_trade.json");

        let state = TradeState::new(sample_order())
            .begin_withdrawal(sample_transfers())
            .begin_trading();

        persist_to(&path, &state).expect("persist must succeed");

        let loaded = load_persisted_from(&path)
            .expect("load must succeed")
            .expect("persisted state should be present");

        assert_eq!(loaded.phase(), state.phase());
        assert_eq!(loaded.order().id, state.order().id);
        assert_eq!(loaded.order().username, state.order().username);

        clear_persisted_from(&path).expect("clear must succeed");
        assert!(
            load_persisted_from(&path).expect("load after clear").is_none(),
            "second load after clear must return Ok(None)"
        );
    }

    #[test]
    fn load_from_missing_file_returns_none() {
        let dir = TmpDir::new("missing");
        let path = dir.path("absent.json");
        assert!(load_persisted_from(&path).expect("missing file is not an error").is_none());
    }

    #[test]
    fn load_from_malformed_json_returns_err() {
        let dir = TmpDir::new("malformed");
        let path = dir.path("current_trade.json");
        std::fs::write(&path, "{ this is not json").unwrap();
        let err = load_persisted_from(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
