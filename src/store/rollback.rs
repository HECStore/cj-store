//! # Shared Rollback Helpers
//!
//! Single implementation of the "return items/diamonds to storage after a failed
//! bot operation" sequence used by every handler (buy, sell, withdraw, deposit,
//! operator removeitem).
//!
//! Each rollback walks a list of `ChestTransfer` entries in order, sends every
//! one to the bot as a `Deposit` `InteractWithChestAndSync`, waits for the sync
//! report with a timeout, and applies it. Step failures are logged at `error!`
//! but do NOT short-circuit the loop: we attempt every step so partial recovery
//! is possible even when one chest is unreachable, and report the aggregate via
//! `RollbackResult`.

use tokio::sync::oneshot;
use tracing::{error, info, warn};

use crate::constants::{CHEST_OP_TIMEOUT_SECS, CHESTS_PER_NODE};
use crate::messages::{BotInstruction, ChestAction};
use crate::types::storage::ChestTransfer;
use super::Store;

/// Summary of a rollback attempt across potentially many chest operations.
///
/// Callers use the success/failure counters to decide what message to send the
/// player: a clean rollback is reported as "items returned to storage", whereas
/// a partial failure escalates to a warning that some items may still be in the
/// bot's inventory and need operator attention.
#[derive(Debug, Clone, Default)]
pub struct RollbackResult {
    /// Total items successfully returned to storage.
    pub items_returned: i32,
    /// Number of per-chest deposit steps that completed cleanly.
    pub operations_succeeded: usize,
    /// Number of per-chest deposit steps that failed (send error, timeout,
    /// bot-reported error, or dropped channel).
    pub operations_failed: usize,
}

impl RollbackResult {
    /// True if at least one chest operation reported failure.
    pub fn has_failures(&self) -> bool {
        self.operations_failed > 0
    }
}

/// Build a `Chest` addressing stub from a `ChestTransfer` entry.
///
/// Only identity fields (`id`, `node_id`, `index`, `position`, `item`) are
/// meaningful — the `amounts` vector is zero-filled because the bot reads real
/// per-slot state from the world on arrival and returns it via the sync report.
pub fn chest_from_transfer(t: &ChestTransfer) -> crate::types::Chest {
    crate::types::Chest {
        id: t.chest_id,
        node_id: t.chest_id / CHESTS_PER_NODE as i32,
        index: t.chest_id % CHESTS_PER_NODE as i32,
        position: t.position,
        item: t.item.clone(),
        amounts: vec![0; crate::types::Storage::SLOTS_PER_CHEST],
    }
}

/// Replay a list of `ChestTransfer` entries as deposit operations.
///
/// Unified rollback primitive: returning withdrawn items to source chests
/// (handler's original `withdraw_plan`) and depositing recovered currency
/// (freshly computed `deposit_plan`) are physically the same — send each
/// transfer as a `Deposit` action and merge the sync report.
///
/// `context` is a short tag (e.g. `"[Buy]"`, `"[Sell] diamond"`) prefixed on
/// every log line so an operator can attribute rollback activity to the
/// originating handler call. Step failures do NOT abort the loop: we continue
/// so partial recovery is possible and report the aggregate via
/// `RollbackResult`.
pub async fn deposit_transfers(
    store: &mut Store,
    transfers: &[ChestTransfer],
    item: &str,
    stack_size: i32,
    context: &str,
) -> RollbackResult {
    let mut result = RollbackResult::default();
    if transfers.is_empty() {
        return result;
    }

    let total_amount: i32 = transfers.iter().map(|t| t.amount).sum();
    info!(
        "{} Rollback START: {} step(s), returning {} x {} to storage",
        context,
        transfers.len(),
        total_amount,
        item
    );

    let total_steps = transfers.len();
    for (step, t) in transfers.iter().enumerate() {
        let step_num = step + 1;
        let chest_id = t.chest_id;
        let node_position = store.get_node_position(chest_id);
        let chest = chest_from_transfer(t);

        info!(
            "{} Rollback step {}/{}: depositing {} x {} into chest {}",
            context, step_num, total_steps, t.amount, item, chest_id
        );

        let (tx, rx) = oneshot::channel();
        let send_result = store
            .bot_tx
            .send(BotInstruction::InteractWithChestAndSync {
                target_chest: chest,
                node_position,
                action: ChestAction::Deposit {
                    item: item.to_string(),
                    amount: t.amount,
                    from_player: None,
                    stack_size,
                },
                respond_to: tx,
            })
            .await;

        if let Err(e) = send_result {
            error!(
                "{} Rollback step {}/{} chest {} FAILED to send BotInstruction ({} x {} NOT returned): {}",
                context, step_num, total_steps, chest_id, t.amount, item, e
            );
            result.operations_failed += 1;
            continue;
        }

        match tokio::time::timeout(
            tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS),
            rx,
        )
        .await
        {
            Ok(Ok(Ok(report))) => {
                // The bot confirmed the physical transfer, so items ARE back
                // in storage; we always credit `items_returned`. But if
                // `apply_chest_sync` fails, the store's in-memory view has
                // drifted from physical reality — flip `has_failures()` and
                // log at error level so an operator reconciles manually.
                if let Err(e) = store.apply_chest_sync(report) {
                    error!(
                        "{} Rollback step {}/{} chest {} sync FAILED (in-memory state diverged from world, items ARE physically returned): {}",
                        context, step_num, total_steps, chest_id, e
                    );
                    result.operations_failed += 1;
                } else {
                    info!(
                        "{} Rollback step {}/{} chest {} OK: {} x {} returned",
                        context, step_num, total_steps, chest_id, t.amount, item
                    );
                    result.operations_succeeded += 1;
                }
                result.items_returned += t.amount;
            }
            Ok(Ok(Err(e))) => {
                error!(
                    "{} Rollback step {}/{} chest {} bot returned error ({} x {} NOT returned): {}",
                    context, step_num, total_steps, chest_id, t.amount, item, e
                );
                result.operations_failed += 1;
            }
            Ok(Err(e)) => {
                error!(
                    "{} Rollback step {}/{} chest {} response channel dropped ({} x {} status UNKNOWN): {}",
                    context, step_num, total_steps, chest_id, t.amount, item, e
                );
                result.operations_failed += 1;
            }
            Err(_) => {
                error!(
                    "{} Rollback step {}/{} chest {} TIMEOUT after {}s ({} x {} status UNKNOWN)",
                    context, step_num, total_steps, chest_id, CHEST_OP_TIMEOUT_SECS, t.amount, item
                );
                result.operations_failed += 1;
            }
        }
    }

    if result.has_failures() {
        warn!(
            "{} Rollback FINISHED WITH FAILURES: {}/{} succeeded, {} failed, {} x {} returned (operator action may be required — check prior error logs)",
            context,
            result.operations_succeeded,
            total_steps,
            result.operations_failed,
            result.items_returned,
            item
        );
    } else {
        info!(
            "{} Rollback OK: {}/{} succeeded, {} x {} returned",
            context, result.operations_succeeded, total_steps, result.items_returned, item
        );
    }

    result
}

/// Compute a deposit plan for `(item, amount)` and replay it.
///
/// Use when you know *how much* needs to go back into storage but don't already
/// have a plan — e.g. after a sell trade fails, `whole_diamonds` sit in the
/// bot's inventory and must be stuffed back into the diamond chests.
///
/// `amount <= 0` is treated as a no-op (returns an empty `RollbackResult`);
/// `amount == 0` is silent, `amount < 0` logs a warning because it signals a
/// caller-side arithmetic bug.
pub async fn rollback_amount_to_storage(
    store: &mut Store,
    item: &str,
    amount: i32,
    stack_size: i32,
    context: &str,
) -> RollbackResult {
    if amount < 0 {
        warn!(
            "{} Rollback skipped: negative amount {} for {} (caller bug — nothing to return)",
            context, amount, item
        );
        return RollbackResult::default();
    }
    if amount == 0 {
        return RollbackResult::default();
    }
    // Non-mutating planner: avoids cloning storage; `apply_chest_sync` re-syncs
    // the authoritative state per successful step in `deposit_transfers`.
    let (plan, planned) = store.storage.simulate_deposit_plan(item, amount, stack_size);
    if planned < amount {
        // Deposit plan could not accommodate every item (storage is full or
        // `item` has no reserved chests left). Items for which no slot was
        // planned will remain in the bot's inventory — flag this so an operator
        // can free space or manually reconcile.
        warn!(
            "{} Rollback under-planned: only {} of {} x {} will be deposited (remaining {} stay in bot inventory)",
            context,
            planned,
            amount,
            item,
            amount - planned
        );
    }
    deposit_transfers(store, &plan, item, stack_size, context).await
}

#[cfg(test)]
mod tests {
    //! Unit tests for the rollback primitives.
    //!
    //! `deposit_transfers` owns the bot channel, so each async test spawns a
    //! small mock receiver that either auto-acks, returns a bot-side error, or
    //! drops the response channel, depending on the scenario under test. The
    //! `Store` is constructed via `Store::new_for_test` so no disk I/O or real
    //! Azalea client is involved.
    use super::*;
    use crate::config::Config;
    use crate::messages::{BotInstruction, ChestSyncReport};
    use crate::types::{ItemId, Node, Position, Storage};
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    fn test_config() -> Config {
        Config {
            position: Position { x: 0, y: 64, z: 0 },
            fee: 0.125,
            account_email: String::new(),
            server_address: "test".to_string(),
            buffer_chest_position: None,
            trade_timeout_ms: 5_000,
            pathfinding_timeout_ms: 5_000,
            max_orders: 1000,
            max_trades_in_memory: 1000,
            autosave_interval_secs: 10,
        }
    }

    /// Storage with a single node whose chest 2 is pre-assigned to `item`.
    /// Chest 2 is used because chests 0 and 1 are reserved for diamonds/overflow.
    fn single_node_storage(item: &str) -> Storage {
        let origin = Position { x: 0, y: 64, z: 0 };
        let mut storage = Storage::new(&origin);
        storage.nodes.push(Node::new(0, &origin));
        let chest = &mut storage.nodes[0].chests[2];
        chest.item = ItemId::from_normalized(item.to_string());
        chest.amounts = vec![0; crate::constants::DOUBLE_CHEST_SLOTS];
        storage
    }

    fn make_store(bot_tx: mpsc::Sender<BotInstruction>, storage: Storage) -> Store {
        Store::new_for_test(bot_tx, test_config(), HashMap::new(), HashMap::new(), storage)
    }

    /// Auto-ack every `InteractWithChestAndSync` with a sync report whose
    /// slot-0 value matches the deposited amount — the one slot `apply_chest_sync`
    /// will merge, leaving the rest of the chest untouched.
    fn spawn_auto_ack_bot(mut rx: mpsc::Receiver<BotInstruction>) {
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if let BotInstruction::InteractWithChestAndSync {
                    target_chest,
                    action,
                    respond_to,
                    ..
                } = msg
                {
                    let (item, delta) = match action {
                        ChestAction::Deposit { item, amount, .. } => (item, amount),
                        ChestAction::Withdraw { item, amount, .. } => (item, -amount),
                    };
                    let mut amounts = [-1i32; crate::constants::DOUBLE_CHEST_SLOTS];
                    let prior = target_chest.amounts.first().copied().unwrap_or(0);
                    amounts[0] = (prior + delta).max(0);
                    let _ = respond_to.send(Ok(ChestSyncReport {
                        chest_id: target_chest.id,
                        item,
                        amounts,
                    }));
                }
            }
        });
    }

    /// Respond with a bot-side error string for every instruction — simulates
    /// the bot being unable to perform the physical transfer.
    fn spawn_bot_error_bot(mut rx: mpsc::Receiver<BotInstruction>) {
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if let BotInstruction::InteractWithChestAndSync { respond_to, .. } = msg {
                    let _ = respond_to.send(Err("simulated bot failure".to_string()));
                }
            }
        });
    }

    /// Drop the response channel for every instruction — simulates a bot task
    /// that crashes between accepting the message and replying.
    fn spawn_channel_drop_bot(mut rx: mpsc::Receiver<BotInstruction>) {
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if let BotInstruction::InteractWithChestAndSync { respond_to, .. } = msg {
                    drop(respond_to);
                }
            }
        });
    }

    fn transfer(chest_id: i32, item: &str, amount: i32) -> ChestTransfer {
        ChestTransfer {
            chest_id,
            position: Position { x: 0, y: 64, z: 0 },
            item: ItemId::from_normalized(item.to_string()),
            amount,
        }
    }

    // --- chest_from_transfer ---------------------------------------------

    #[test]
    fn chest_from_transfer_derives_node_and_index_from_id() {
        // chest_id 9 on 4-per-node layout => node 2, index 1.
        let t = transfer(9, "cobblestone", 12);
        let c = chest_from_transfer(&t);
        assert_eq!(c.id, 9);
        assert_eq!(c.node_id, 9 / CHESTS_PER_NODE as i32);
        assert_eq!(c.index, 9 % CHESTS_PER_NODE as i32);
        assert_eq!(c.position, t.position);
        assert_eq!(c.item, t.item);
        assert_eq!(c.amounts.len(), crate::types::Storage::SLOTS_PER_CHEST);
        assert!(c.amounts.iter().all(|&a| a == 0), "amounts must be zero-filled");
    }

    // --- RollbackResult --------------------------------------------------

    #[test]
    fn has_failures_returns_false_when_no_operations_failed() {
        let r = RollbackResult { items_returned: 5, operations_succeeded: 2, operations_failed: 0 };
        assert!(!r.has_failures());
    }

    #[test]
    fn has_failures_returns_true_when_any_operation_failed() {
        let r = RollbackResult { items_returned: 0, operations_succeeded: 0, operations_failed: 1 };
        assert!(r.has_failures());
    }

    #[test]
    fn default_rollback_result_reports_no_failures() {
        assert!(!RollbackResult::default().has_failures());
    }

    // --- deposit_transfers -----------------------------------------------

    #[tokio::test]
    async fn deposit_transfers_empty_slice_is_a_noop() {
        let (tx, _rx) = mpsc::channel(4);
        let mut store = make_store(tx, single_node_storage("cobblestone"));
        let result = deposit_transfers(&mut store, &[], "cobblestone", 64, "[Test]").await;
        assert_eq!(result.items_returned, 0);
        assert_eq!(result.operations_succeeded, 0);
        assert_eq!(result.operations_failed, 0);
    }

    #[tokio::test]
    async fn deposit_transfers_records_success_for_every_acked_step() {
        let (tx, rx) = mpsc::channel(4);
        spawn_auto_ack_bot(rx);
        let mut store = make_store(tx, single_node_storage("cobblestone"));

        let plan = vec![transfer(2, "cobblestone", 30), transfer(2, "cobblestone", 20)];
        let result = deposit_transfers(&mut store, &plan, "cobblestone", 64, "[Test]").await;

        assert_eq!(result.operations_succeeded, 2);
        assert_eq!(result.operations_failed, 0);
        assert_eq!(result.items_returned, 50);
        assert!(!result.has_failures());
    }

    #[tokio::test]
    async fn deposit_transfers_counts_send_failure_when_receiver_dropped() {
        // Drop the receiver BEFORE calling — bot_tx.send() returns Err.
        let (tx, rx) = mpsc::channel(4);
        drop(rx);
        let mut store = make_store(tx, single_node_storage("cobblestone"));

        let plan = vec![transfer(2, "cobblestone", 10)];
        let result = deposit_transfers(&mut store, &plan, "cobblestone", 64, "[Test]").await;

        assert_eq!(result.operations_succeeded, 0);
        assert_eq!(result.operations_failed, 1);
        assert_eq!(result.items_returned, 0);
        assert!(result.has_failures());
    }

    #[tokio::test]
    async fn deposit_transfers_counts_bot_reported_error_as_failure() {
        let (tx, rx) = mpsc::channel(4);
        spawn_bot_error_bot(rx);
        let mut store = make_store(tx, single_node_storage("cobblestone"));

        let plan = vec![transfer(2, "cobblestone", 10)];
        let result = deposit_transfers(&mut store, &plan, "cobblestone", 64, "[Test]").await;

        assert_eq!(result.operations_succeeded, 0);
        assert_eq!(result.operations_failed, 1);
        // No items returned because the bot refused the deposit.
        assert_eq!(result.items_returned, 0);
    }

    #[tokio::test]
    async fn deposit_transfers_counts_dropped_response_channel_as_failure() {
        let (tx, rx) = mpsc::channel(4);
        spawn_channel_drop_bot(rx);
        let mut store = make_store(tx, single_node_storage("cobblestone"));

        let plan = vec![transfer(2, "cobblestone", 10)];
        let result = deposit_transfers(&mut store, &plan, "cobblestone", 64, "[Test]").await;

        assert_eq!(result.operations_failed, 1);
        assert_eq!(result.items_returned, 0);
    }

    #[tokio::test]
    async fn deposit_transfers_continues_after_a_failed_step() {
        // Mock bot acks odd-indexed sends but returns an error for the first
        // one, ensuring the loop does not short-circuit on failure.
        let (tx, mut rx) = mpsc::channel(4);
        tokio::spawn(async move {
            let mut seen = 0usize;
            while let Some(msg) = rx.recv().await {
                if let BotInstruction::InteractWithChestAndSync {
                    target_chest,
                    action,
                    respond_to,
                    ..
                } = msg
                {
                    seen += 1;
                    if seen == 1 {
                        let _ = respond_to.send(Err("first step fails".to_string()));
                        continue;
                    }
                    let (item, delta) = match action {
                        ChestAction::Deposit { item, amount, .. } => (item, amount),
                        ChestAction::Withdraw { item, amount, .. } => (item, -amount),
                    };
                    let mut amounts = [-1i32; crate::constants::DOUBLE_CHEST_SLOTS];
                    amounts[0] = (target_chest.amounts.first().copied().unwrap_or(0) + delta).max(0);
                    let _ = respond_to.send(Ok(ChestSyncReport {
                        chest_id: target_chest.id,
                        item,
                        amounts,
                    }));
                }
            }
        });
        let mut store = make_store(tx, single_node_storage("cobblestone"));

        let plan = vec![
            transfer(2, "cobblestone", 10),
            transfer(2, "cobblestone", 7),
            transfer(2, "cobblestone", 3),
        ];
        let result = deposit_transfers(&mut store, &plan, "cobblestone", 64, "[Test]").await;

        assert_eq!(result.operations_succeeded, 2, "steps 2 and 3 should succeed");
        assert_eq!(result.operations_failed, 1, "step 1 should fail");
        // items_returned only credits acked steps: 7 + 3.
        assert_eq!(result.items_returned, 10);
    }

    // --- rollback_amount_to_storage --------------------------------------

    #[tokio::test]
    async fn rollback_amount_to_storage_zero_amount_is_silent_noop() {
        let (tx, _rx) = mpsc::channel(4);
        let mut store = make_store(tx, single_node_storage("cobblestone"));
        let result = rollback_amount_to_storage(&mut store, "cobblestone", 0, 64, "[Test]").await;
        assert_eq!(result.items_returned, 0);
        assert_eq!(result.operations_succeeded, 0);
        assert_eq!(result.operations_failed, 0);
    }

    #[tokio::test]
    async fn rollback_amount_to_storage_negative_amount_returns_empty_result() {
        // A negative amount reaching this function signals a caller-side bug;
        // we must not crash and must not invoke the planner.
        let (tx, _rx) = mpsc::channel(4);
        let mut store = make_store(tx, single_node_storage("cobblestone"));
        let result = rollback_amount_to_storage(&mut store, "cobblestone", -5, 64, "[Test]").await;
        assert_eq!(result.items_returned, 0);
        assert_eq!(result.operations_failed, 0);
        assert!(!result.has_failures());
    }

    #[tokio::test]
    async fn rollback_amount_to_storage_plans_and_returns_items_via_mock_bot() {
        let (tx, rx) = mpsc::channel(8);
        spawn_auto_ack_bot(rx);
        let mut store = make_store(tx, single_node_storage("cobblestone"));

        let result =
            rollback_amount_to_storage(&mut store, "cobblestone", 50, 64, "[Test]").await;

        assert!(!result.has_failures());
        assert_eq!(result.items_returned, 50);
        assert!(result.operations_succeeded >= 1);
    }
}
