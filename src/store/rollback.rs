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

use super::Store;
use crate::constants::{CHEST_OP_TIMEOUT_SECS, CHESTS_PER_NODE};
use crate::messages::{BotInstruction, ChestAction};
use crate::types::storage::ChestTransfer;

/// Summary of a rollback attempt across potentially many chest operations.
///
/// Callers use the success/failure counters to decide what message to send the
/// player: a clean rollback is reported as "items returned to storage", whereas
/// a partial failure escalates to a warning that some items may still be in the
/// bot's inventory and need operator attention.
#[derive(Debug, Clone, Default)]
pub struct RollbackResult {
    /// Total items *physically* returned to storage, credited as soon as the
    /// bot confirms the deposit — even if the subsequent `apply_chest_sync`
    /// errors and `operations_failed` is also incremented (the chest holds
    /// the items, but the in-memory view has drifted and an operator must
    /// reconcile). Timeout, channel-drop, and bot-error branches do NOT
    /// credit this counter: physical state is unknown, so we don't claim it.
    pub items_returned: i32,
    /// Items the planner could NOT place anywhere — storage is full, the item
    /// has no reserved chests, or there are zero growable nodes. These items
    /// remain physically on the bot; no chest step was even attempted for them.
    /// Distinct from `operations_failed`, which counts per-step bot errors,
    /// and from `items_stuck_on_bot`, which counts items in *attempted* steps
    /// that failed at execution time.
    pub items_unplanned: i32,
    /// Items in chest-op steps that the planner placed but execution did not
    /// confirm: send-error tail, bot-reported error, channel drop, timeout.
    /// Note: the `apply_chest_sync`-fail sub-branch is NOT counted here —
    /// items there are physically deposited, only the in-memory view drifted
    /// (already credited via `items_returned`). Together with `items_unplanned`
    /// this is the total inventory the bot may still be holding.
    pub items_stuck_on_bot: i32,
    /// Number of per-chest deposit steps that completed cleanly.
    pub operations_succeeded: usize,
    /// Number of per-chest deposit steps that failed (send error, timeout,
    /// bot-reported error, or dropped channel).
    pub operations_failed: usize,
}

impl RollbackResult {
    /// True if at least one chest operation reported failure OR the planner
    /// could not place every item. Both conditions mean items may still be in
    /// the bot's inventory and operator attention may be required.
    pub fn has_failures(&self) -> bool {
        self.operations_failed > 0 || self.items_unplanned > 0 || self.items_stuck_on_bot > 0
    }

    /// If the rollback ended with items still on the bot (per-step failures
    /// and/or planner shortfall), return a short, player-facing suffix
    /// describing the residue. Returns `None` on a fully clean rollback so
    /// callers can use the "(items rolled back to storage)" wording.
    ///
    /// Designed to be appended to a handler's failure-path message; the
    /// caller still chooses the leading verb ("Buy aborted: …", "Sell aborted: …").
    pub fn partial_message(&self) -> Option<String> {
        if !self.has_failures() {
            return None;
        }
        let unplanned = self.items_unplanned;
        let stuck_on_bot = self.items_stuck_on_bot;
        let failed = self.operations_failed;
        let returned = self.items_returned;
        let mut parts: Vec<String> = Vec::new();
        if returned > 0 {
            parts.push(format!("{} returned to storage", returned));
        }
        if failed > 0 {
            parts.push(format!("{} chest operation(s) failed", failed));
        }
        if stuck_on_bot > 0 {
            parts.push(format!(
                "{} item(s) stuck on the bot from failed chest op(s) — recover manually",
                stuck_on_bot
            ));
        }
        if unplanned > 0 {
            parts.push(format!(
                "{} item(s) could not be placed and remain on the bot — investigate manually",
                unplanned
            ));
        }
        Some(parts.join("; "))
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

    // i64 sum + clamp guards a 50K-step plan against silent i32 overflow in
    // the log line; this counter is log-only and not used for accounting.
    let total_amount: i64 = transfers.iter().map(|t| i64::from(t.amount.max(0))).sum();
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
            result.items_stuck_on_bot = result.items_stuck_on_bot.saturating_add(t.amount.max(0));
            // mpsc Sender::send returning Err is permanent (receiver dropped).
            // Short-circuit: every remaining step would log the same error for
            // one root cause. Mark the truly-not-yet-attempted tail as failed
            // and credit each tail entry's amount to items_stuck_on_bot — the
            // bot is holding all of them since no deposit was ever attempted.
            let skipped = transfers.len().saturating_sub(step_num);
            if skipped > 0 {
                result.operations_failed += skipped;
                let tail_amount: i32 = transfers[step_num..]
                    .iter()
                    .map(|tail| tail.amount.max(0))
                    .sum();
                result.items_stuck_on_bot = result.items_stuck_on_bot.saturating_add(tail_amount);
            }
            error!(
                "{} Rollback step {}/{} chest {} bot channel closed; aborting after this step, {} subsequent step(s) marked failed: {}",
                context, step_num, total_steps, chest_id, skipped, e
            );
            break;
        }

        match tokio::time::timeout(tokio::time::Duration::from_secs(CHEST_OP_TIMEOUT_SECS), rx)
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
                // saturating_add + clamp matches the sister `items_stuck_on_bot`
                // arithmetic; a 50K-step plan must not silently overflow this i32.
                result.items_returned = result.items_returned.saturating_add(t.amount.max(0));
            }
            Ok(Ok(Err(e))) => {
                error!(
                    "{} Rollback step {}/{} chest {} bot returned error ({} x {} NOT returned): {}",
                    context, step_num, total_steps, chest_id, t.amount, item, e
                );
                result.operations_failed += 1;
                result.items_stuck_on_bot =
                    result.items_stuck_on_bot.saturating_add(t.amount.max(0));
            }
            Ok(Err(e)) => {
                error!(
                    "{} Rollback step {}/{} chest {} response channel dropped ({} x {} status UNKNOWN): {}",
                    context, step_num, total_steps, chest_id, t.amount, item, e
                );
                result.operations_failed += 1;
                result.items_stuck_on_bot =
                    result.items_stuck_on_bot.saturating_add(t.amount.max(0));
            }
            Err(_) => {
                error!(
                    "{} Rollback step {}/{} chest {} TIMEOUT after {}s ({} x {} status UNKNOWN)",
                    context, step_num, total_steps, chest_id, CHEST_OP_TIMEOUT_SECS, t.amount, item
                );
                result.operations_failed += 1;
                result.items_stuck_on_bot =
                    result.items_stuck_on_bot.saturating_add(t.amount.max(0));
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
    // Non-mutating planner: avoids cloning storage and — critically — does
    // NOT commit slot counts in `Storage` before the bot replies. Only
    // `apply_chest_sync` (driven by the real bot reply in `deposit_transfers`)
    // is allowed to mutate slot counts; the previous mutating-`deposit_plan`
    // fallback would otherwise claim items the bot was still holding if the
    // subsequent `deposit_transfers` failed.
    let (mut plan, mut planned) = store
        .storage
        .simulate_deposit_plan(item, amount, stack_size);
    let mut unplanned = (amount - planned).max(0);
    // `simulate_deposit_plan` only walks EXISTING chests — it does NOT model
    // node growth. Order pre-flight callers WANT this so growth becomes an
    // operator decision; rollback is the safety net and must accept growth
    // rather than strand items already in the bot's inventory. Grow by
    // calling `add_node` (the only sanctioned non-sync mutation) and re-
    // simulate until the plan covers `amount` or growth stops helping (defense
    // in depth against a misconfigured topology — should be unreachable).
    let mut grow_attempts = 0usize;
    while unplanned > 0 {
        if grow_attempts == 0 {
            info!(
                "{} Rollback simulate under-planned by {}/{}; growing storage by one node and re-simulating",
                context, unplanned, amount
            );
        }
        grow_attempts += 1;
        // Guard against pathological loops: each `add_node` adds one empty
        // node with `CHESTS_PER_NODE` chests, so a healthy run absorbs the
        // shortfall within `unplanned / (CHESTS_PER_NODE * SLOTS_PER_CHEST * stack)`
        // iterations. 16 grows is well beyond any realistic rollback.
        if grow_attempts > 16 {
            warn!(
                "{} Rollback grow-fallback gave up after {} add_node iterations; {} item(s) will remain in bot inventory",
                context, grow_attempts, unplanned
            );
            break;
        }
        store.storage.add_node();
        let (re_plan, re_planned) = store
            .storage
            .simulate_deposit_plan(item, amount, stack_size);
        // If a re-simulation didn't pick up MORE than before, growth isn't
        // helping (e.g. reserved-chest rules block this item from new nodes
        // — currently impossible since reservations apply only to node 0).
        if re_planned <= planned {
            warn!(
                "{} Rollback grow-fallback added a node but simulation still planned {}/{}; aborting growth to avoid infinite loop",
                context, re_planned, amount
            );
            break;
        }
        plan = re_plan;
        planned = re_planned;
        unplanned = (amount - planned).max(0);
    }
    // Populate `items_unplanned` here, BEFORE delegating: `deposit_transfers`
    // is also called by buy/operator handlers with caller-supplied plans where
    // an "unplanned shortfall" is not a meaningful concept.
    let mut result = deposit_transfers(store, &plan, item, stack_size, context).await;
    result.items_unplanned = unplanned;
    result
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
            chat: crate::config::ChatConfig::default(),
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
        Store::new_for_test(
            bot_tx,
            test_config(),
            HashMap::new(),
            HashMap::new(),
            storage,
        )
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

    /// Auto-ack every `InteractWithChestAndSync` with a sync report whose
    /// `chest_id` is hard-coded to `bogus_chest_id` — i.e. not present in the
    /// store's storage — so `apply_chest_sync` will return `Err` while the bot
    /// itself confirms the physical deposit succeeded. Drives the rare
    /// `Ok(Ok(Ok(report)))`-but-`apply_chest_sync`-fails branch in
    /// `deposit_transfers`, where the items are physically returned but the
    /// in-memory chest view has drifted.
    fn spawn_sync_failing_bot(mut rx: mpsc::Receiver<BotInstruction>, bogus_chest_id: i32) {
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if let BotInstruction::InteractWithChestAndSync {
                    action, respond_to, ..
                } = msg
                {
                    let item = match action {
                        ChestAction::Deposit { item, .. } => item,
                        ChestAction::Withdraw { item, .. } => item,
                    };
                    let amounts = [-1i32; crate::constants::DOUBLE_CHEST_SLOTS];
                    let _ = respond_to.send(Ok(ChestSyncReport {
                        chest_id: bogus_chest_id,
                        item,
                        amounts,
                    }));
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
        assert!(
            c.amounts.iter().all(|&a| a == 0),
            "amounts must be zero-filled"
        );
    }

    // --- RollbackResult --------------------------------------------------

    #[test]
    fn has_failures_returns_false_when_no_operations_failed() {
        let r = RollbackResult {
            items_returned: 5,
            operations_succeeded: 2,
            operations_failed: 0,
            ..Default::default()
        };
        assert!(!r.has_failures());
    }

    #[test]
    fn has_failures_returns_true_when_any_operation_failed() {
        let r = RollbackResult {
            items_returned: 0,
            operations_succeeded: 0,
            operations_failed: 1,
            ..Default::default()
        };
        assert!(r.has_failures());
    }

    #[test]
    fn default_rollback_result_reports_no_failures() {
        assert!(!RollbackResult::default().has_failures());
    }

    #[test]
    fn has_failures_returns_true_when_only_items_unplanned_set() {
        // Locks in the second disjunct of `has_failures()`: a planning
        // shortfall alone (no per-step failures) must still surface as a
        // failure so handlers escalate to operator-action wording.
        let r = RollbackResult {
            items_unplanned: 3,
            ..Default::default()
        };
        assert_eq!(r.operations_failed, 0);
        assert!(r.has_failures());
    }

    #[test]
    fn has_failures_ignores_items_returned() {
        // Locks in: physical successes alone don't constitute a failure.
        let r = RollbackResult {
            items_returned: 1000,
            ..Default::default()
        };
        assert!(!r.has_failures());
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

        let plan = vec![
            transfer(2, "cobblestone", 30),
            transfer(2, "cobblestone", 20),
        ];
        let result = deposit_transfers(&mut store, &plan, "cobblestone", 64, "[Test]").await;

        assert_eq!(result.operations_succeeded, 2);
        assert_eq!(result.operations_failed, 0);
        assert_eq!(result.items_returned, 50);
        assert!(!result.has_failures());
    }

    #[tokio::test]
    async fn deposit_transfers_counts_send_failure_when_receiver_dropped() {
        // Drop the receiver BEFORE calling — bot_tx.send() returns Err on the
        // very first step. The function must short-circuit (channel-drop is
        // permanent) and mark all remaining steps as failed without retrying.
        let (tx, rx) = mpsc::channel(4);
        drop(rx);
        let mut store = make_store(tx, single_node_storage("cobblestone"));

        let plan = vec![
            transfer(2, "cobblestone", 10),
            transfer(2, "cobblestone", 7),
            transfer(2, "cobblestone", 3),
        ];
        let result = deposit_transfers(&mut store, &plan, "cobblestone", 64, "[Test]").await;

        assert_eq!(result.operations_succeeded, 0);
        assert_eq!(
            result.operations_failed, 3,
            "all 3 steps must be counted as failed even though only the first was attempted"
        );
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
                    amounts[0] =
                        (target_chest.amounts.first().copied().unwrap_or(0) + delta).max(0);
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

        assert_eq!(
            result.operations_succeeded, 2,
            "steps 2 and 3 should succeed"
        );
        assert_eq!(result.operations_failed, 1, "step 1 should fail");
        // items_returned only credits acked steps: 7 + 3.
        assert_eq!(result.items_returned, 10);
    }

    #[tokio::test]
    async fn deposit_transfers_credits_items_returned_even_when_apply_chest_sync_fails() {
        // Pins the docstring contract on `RollbackResult::items_returned`:
        // when the bot confirms the physical deposit (`Ok(Ok(Ok(report)))`)
        // but `apply_chest_sync` errors — the chest holds the items, but the
        // in-memory view has drifted — `items_returned` MUST still be credited
        // for the physical transfer, while `operations_failed` flips so
        // `has_failures()` surfaces the divergence to the operator.
        //
        // The mock bot replies with a `ChestSyncReport` whose `chest_id`
        // (`9999`) is NOT present in storage, so `apply_chest_sync` returns
        // `Err("Chest 9999 not found in storage")` while the deposit-confirm
        // ack itself is `Ok`.
        const BOGUS_CHEST_ID: i32 = 9999;
        let (tx, rx) = mpsc::channel(4);
        spawn_sync_failing_bot(rx, BOGUS_CHEST_ID);
        let mut store = make_store(tx, single_node_storage("cobblestone"));

        let amount = 17;
        let plan = vec![transfer(2, "cobblestone", amount)];
        let result = deposit_transfers(&mut store, &plan, "cobblestone", 64, "[Test]").await;

        assert_eq!(
            result.items_returned, amount,
            "items must be credited per the docstring: physical deposit succeeded"
        );
        assert_eq!(
            result.operations_failed, 1,
            "apply_chest_sync failure must flip operations_failed so has_failures() escalates"
        );
        assert_eq!(
            result.operations_succeeded, 0,
            "the step did NOT succeed end-to-end: in-memory view diverged"
        );
        assert!(
            result.has_failures(),
            "in-memory drift must surface as a failure for operator escalation"
        );
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

        let result = rollback_amount_to_storage(&mut store, "cobblestone", 50, 64, "[Test]").await;

        assert!(!result.has_failures());
        assert_eq!(result.items_returned, 50);
        assert!(result.operations_succeeded >= 1);
    }

    #[tokio::test]
    async fn rollback_surfaces_dispatch_failure_when_bot_unreachable() {
        // Storage with NO nodes at all — `simulate_deposit_plan` plans nothing,
        // but `rollback_amount_to_storage` then grows storage via `add_node`
        // and re-simulates (the only mutation allowed before the bot replies;
        // `apply_chest_sync` is the only path permitted to mutate slot counts)
        // until the shortfall is absorbed (see the safety-net comment in
        // `rollback.rs` — rollback must accept growth rather than strand items
        // already in the bot's inventory). So `items_unplanned` ends at 0.
        //
        // The actual rollback contract this test now locks in: when the
        // bot dispatch step fails (here because no mock-bot task is
        // reading the channel), `RollbackResult` MUST surface failure via
        // `has_failures()` and `partial_message()` — otherwise callers
        // would tell the player "items returned to storage" while the
        // items were physically still on the bot. Before the original
        // rollback fix, `Default::default()` had `has_failures() == false`,
        // hiding exactly this shape of failure.
        let origin = Position { x: 0, y: 64, z: 0 };
        let storage = Storage::new(&origin); // zero nodes
        let (tx, _rx) = mpsc::channel(4);
        let mut store = make_store(tx, storage);

        let result = rollback_amount_to_storage(&mut store, "cobblestone", 5, 64, "[Test]").await;

        // Grow-fallback absorbed the planning shortfall.
        assert_eq!(
            result.items_unplanned, 0,
            "grow-fallback must absorb the shortfall"
        );
        // No bot to ack the dispatched plan → operations_failed > 0.
        assert!(
            result.operations_failed > 0,
            "dispatch with no live bot must surface as a failed op"
        );
        // Nothing was physically moved; the items are stranded on the bot.
        assert_eq!(
            result.items_returned, 0,
            "no items were physically returned"
        );
        // The combined shape MUST flip has_failures() so callers don't
        // silently report success.
        assert!(
            result.has_failures(),
            "dispatch failure must surface via has_failures()"
        );
        assert!(
            result.partial_message().is_some(),
            "partial_message() must yield a string when items remain on the bot"
        );
    }

    // --- partial_message wording / format --------------------------------

    #[test]
    fn partial_message_returns_none_on_clean_rollback() {
        // A result with successful operations and items returned but no
        // failures and no unplanned items must produce no partial-message
        // suffix — callers fall back to the "(items rolled back to storage)"
        // wording instead.
        let r = RollbackResult {
            items_returned: 5,
            operations_succeeded: 2,
            ..Default::default()
        };
        assert!(r.partial_message().is_none());
    }

    #[test]
    fn partial_message_only_reports_returned_when_no_failures() {
        // Locks in: returned-only != failure. Even though `items_returned`
        // is non-zero, `has_failures()` is false (no operations_failed and
        // no items_unplanned), so `partial_message()` short-circuits to None.
        let r = RollbackResult {
            items_returned: 7,
            ..Default::default()
        };
        assert!(r.partial_message().is_none());
    }

    #[test]
    fn partial_message_combines_returned_failed_and_stuck() {
        let r = RollbackResult {
            items_returned: 5,
            operations_failed: 2,
            items_unplanned: 3,
            ..Default::default()
        };
        let msg = r.partial_message().expect("has failures => Some");
        assert!(msg.contains("5 returned to storage"), "msg was: {msg}");
        assert!(
            msg.contains("2 chest operation(s) failed"),
            "msg was: {msg}"
        );
        assert!(
            msg.contains("3 item(s) could not be placed"),
            "msg was: {msg}"
        );
        assert!(
            msg.contains("; "),
            "clauses must be joined by '; ' (msg was: {msg})"
        );
        // Order is fixed in the function: returned, failed, stuck.
        let returned_idx = msg.find("5 returned to storage").unwrap();
        let failed_idx = msg.find("2 chest operation(s) failed").unwrap();
        let stuck_idx = msg.find("3 item(s) could not be placed").unwrap();
        assert!(
            returned_idx < failed_idx && failed_idx < stuck_idx,
            "clauses must appear in order returned, failed, stuck (msg was: {msg})"
        );
    }

    #[test]
    fn partial_message_omits_zero_counters() {
        // Only items_unplanned is non-zero — the "returned" and "failed"
        // clauses must be omitted entirely (no leading separator, no
        // zero-padded clauses). Uses the literal em-dash (U+2014).
        let r = RollbackResult {
            items_unplanned: 4,
            ..Default::default()
        };
        let msg = r.partial_message().expect("has failures => Some");
        assert_eq!(
            msg,
            "4 item(s) could not be placed and remain on the bot — investigate manually"
        );
    }
}
