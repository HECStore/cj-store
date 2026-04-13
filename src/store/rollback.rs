//! # Shared Rollback Helpers
//!
//! This module extracts the "return items/diamonds to storage after a failed trade"
//! pattern that used to be copy-pasted across buy/sell/withdraw/deposit/removeitem
//! handlers. Centralising it means there is exactly one implementation to audit
//! for correctness and one place to tune (timeouts, logging, error handling).
//!
//! The core operation is always the same: we hold a list of `ChestTransfer`
//! entries describing where items currently belong (or where they should go),
//! walk them in order, send a `Deposit` `InteractWithChestAndSync` to the bot,
//! await the result with a timeout, and apply the sync report. Individual step
//! failures are logged but do not short-circuit — we try every step so that as
//! much as possible is restored even when one chest is unreachable.

use tokio::sync::oneshot;
use tracing::{error, info, warn};

use crate::constants::CHEST_OP_TIMEOUT_SECS;
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

/// Build a fresh `Chest` struct from a `ChestTransfer` entry.
///
/// We only need the identity fields (`id`, `node_id`, `index`, `position`,
/// `item`) plus an empty slot vector — the bot reads the real per-slot state
/// from the world when it performs the operation and replies with a sync
/// report. This helper replaces the ~8-line inline struct literal that used to
/// be copy-pasted at every chest interaction site.
pub fn chest_from_transfer(t: &ChestTransfer) -> crate::types::Chest {
    crate::types::Chest {
        id: t.chest_id,
        node_id: t.chest_id / 4,
        index: t.chest_id % 4,
        position: t.position,
        item: t.item.clone(),
        amounts: vec![0; crate::types::Storage::SLOTS_PER_CHEST],
    }
}

/// Replay a list of `ChestTransfer` entries as deposit operations.
///
/// This is the unified rollback primitive used by every handler: whether we are
/// returning withdrawn items to their source chests (using the original
/// `withdraw_plan`) or depositing diamonds via a freshly computed
/// `deposit_plan`, the physical work is identical — send each transfer to the
/// bot as a `Deposit` action and apply the resulting sync report.
///
/// `context` is a short tag (e.g. `"[Buy]"`, `"[Sell] diamond"`) used to make
/// log lines attributable. Individual step failures do NOT abort the loop: we
/// continue trying so that as many items as possible make it back to storage,
/// and report the aggregate outcome via `RollbackResult`.
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

    info!(
        "{} Rollback: replaying {} deposit operation(s) for {}",
        context,
        transfers.len(),
        item
    );

    for (step, t) in transfers.iter().enumerate() {
        let step_num = step + 1;
        let node_position = store.get_node_position(t.chest_id);
        let chest = chest_from_transfer(t);

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
                "{} Rollback step {}/{} FAILED to send: {}",
                context,
                step_num,
                transfers.len(),
                e
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
                if let Err(e) = store.apply_chest_sync(report) {
                    warn!(
                        "{} Rollback step {} chest sync warning: {}",
                        context, step_num, e
                    );
                }
                result.operations_succeeded += 1;
                result.items_returned += t.amount;
            }
            Ok(Ok(Err(e))) => {
                error!(
                    "{} Rollback step {} bot returned error: {}",
                    context, step_num, e
                );
                result.operations_failed += 1;
            }
            Ok(Err(e)) => {
                error!(
                    "{} Rollback step {} channel dropped: {}",
                    context, step_num, e
                );
                result.operations_failed += 1;
            }
            Err(_) => {
                error!(
                    "{} Rollback step {} TIMEOUT after {}s",
                    context, step_num, CHEST_OP_TIMEOUT_SECS
                );
                result.operations_failed += 1;
            }
        }
    }

    info!(
        "{} Rollback complete: {}/{} succeeded, {} items returned ({} failed)",
        context,
        result.operations_succeeded,
        transfers.len(),
        result.items_returned,
        result.operations_failed
    );

    result
}

/// Convenience wrapper: compute a deposit plan for `(item, amount)` and replay it.
///
/// Use this when you know *how much* needs to go back into storage but don't
/// already have a plan on hand — e.g. after a sell trade fails, you have
/// `whole_diamonds` in the bot's inventory and need to stuff them back into the
/// diamond chests.
pub async fn rollback_amount_to_storage(
    store: &mut Store,
    item: &str,
    amount: i32,
    stack_size: i32,
    context: &str,
) -> RollbackResult {
    if amount <= 0 {
        return RollbackResult::default();
    }
    // Use the non-mutating planner so we don't clone all of storage just to
    // compute where items would land. The authoritative state is re-synced by
    // `apply_chest_sync` on each successful step anyway.
    let (plan, _planned) = store.storage.simulate_deposit_plan(item, amount, stack_size);
    deposit_transfers(store, &plan, item, stack_size, context).await
}
