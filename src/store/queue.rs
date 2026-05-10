//! Persistent FIFO order queue.
//!
//! Orders (buy/sell/deposit/withdraw) land here the moment a player command is
//! validated, and are processed one at a time by `Store::run()`. Persisting on
//! every mutation means a restart can't lose a player's place in line.

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

/// Per-process disambiguator for queue archive filenames.
///
/// Mirrors the same-named statics in `journal.rs` and `trade_state.rs`. Two
/// archive operations colliding on `unix_ms` (e.g. both falling back to
/// `unwrap_or(0)` from a clock error) would otherwise overwrite each other.
static ARCHIVE_SEQ: AtomicU64 = AtomicU64::new(0);

use crate::constants::{MAX_ORDERS_PER_USER, MAX_QUEUE_SIZE, QUEUE_FILE};
use crate::fsutil::{archive_aside, write_atomic};
use crate::messages::QueuedOrderType;

/// An order waiting to be processed.
///
/// Serialized as part of the on-disk queue file (see [`QueuePersist`]); any
/// field rename is a persisted-format break.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedOrder {
    pub id: u64,
    pub user_uuid: String,
    pub username: String,
    pub order_type: QueuedOrderType,
    /// Item being traded (buy/sell) or `"diamond"` for deposit/withdraw.
    pub item: String,
    /// Quantity for buy/sell. Ignored for deposit/withdraw, which carry their
    /// amount inside `order_type` (or `None` for flexible).
    pub quantity: u32,
    pub queued_at: DateTime<Utc>,
}

impl QueuedOrder {
    pub fn new(
        id: u64,
        user_uuid: String,
        username: String,
        order_type: QueuedOrderType,
        item: String,
        quantity: u32,
    ) -> Self {
        Self {
            id,
            user_uuid,
            username,
            order_type,
            item,
            quantity,
            queued_at: Utc::now(),
        }
    }

    /// Short human-readable summary used in player messages and log lines.
    pub fn description(&self) -> String {
        match &self.order_type {
            QueuedOrderType::Buy => format!("buy {} {}", self.item, self.quantity),
            QueuedOrderType::Sell => format!("sell {} {}", self.item, self.quantity),
            QueuedOrderType::Deposit { amount } => match amount {
                Some(amt) => format!("deposit {:.2}", amt),
                None => "deposit (flexible)".to_string(),
            },
            QueuedOrderType::Withdraw { amount } => match amount {
                Some(amt) => format!("withdraw {:.2}", amt),
                None => "withdraw (full balance)".to_string(),
            },
        }
    }
}

#[derive(Debug)]
pub struct OrderQueue {
    orders: VecDeque<QueuedOrder>,
    /// Monotonic order ID counter; persisted so IDs don't recycle across restarts.
    next_id: u64,
}

impl Default for OrderQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl OrderQueue {
    pub fn new() -> Self {
        Self {
            orders: VecDeque::new(),
            next_id: 1,
        }
    }

    /// Load queue from `QUEUE_FILE`, or return an empty queue if the file is
    /// absent. Called once at startup to restore pending orders across restarts.
    pub fn load() -> io::Result<Self> {
        Self::load_from(QUEUE_FILE)
    }

    /// Write queue to `QUEUE_FILE`. Uses [`write_atomic`] so a crash mid-write
    /// cannot leave a truncated or corrupt queue file.
    pub fn save(&self) -> io::Result<()> {
        self.save_to(QUEUE_FILE)
    }

    /// Path-parameterized load, separated so tests can round-trip without
    /// touching the production `QUEUE_FILE`.
    ///
    /// On a parse error or a non-`NotFound` IO error, the offending file is
    /// quarantined to a `queue.json.{corrupt,unreadable}-<unix_ms>-<seq>.json`
    /// sibling and an empty queue is returned. Mirrors the patterns in
    /// `journal.rs::load_from` and `trade_state::load_persisted_from`.
    fn load_from(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();

        if !path.exists() {
            info!("[Queue] No queue file at {:?}, starting empty", path);
            return Ok(Self::new());
        }

        let contents = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(read_err) => {
                warn!(
                    "[Queue] failed to read queue file {:?}: {read_err} - attempting to quarantine before falling back to empty queue",
                    path
                );
                match Self::quarantine_to(path, "unreadable") {
                    Ok(archived) => {
                        error!(
                            "[Queue] PENDING ORDERS LOST: quarantined unreadable queue file {:?} to {:?} - preserve for operator review",
                            path, archived
                        );
                        return Ok(Self::new());
                    }
                    Err(quarantine_err) => {
                        error!(
                            "[Queue] could not quarantine unreadable queue file {:?}: {quarantine_err} - returning original read error so caller is aware",
                            path
                        );
                        return Err(read_err);
                    }
                }
            }
        };
        let queue_data: QueuePersist = match serde_json::from_str(&contents) {
            Ok(q) => q,
            Err(e) => match Self::quarantine_to(path, "corrupt") {
                Ok(archived) => {
                    error!(
                        "[Queue] PENDING ORDERS LOST: corrupt queue file {:?} moved to {:?}; parse error: {}",
                        path, archived, e
                    );
                    return Ok(Self::new());
                }
                Err(quarantine_err) => {
                    error!(
                        "[Queue] PENDING ORDERS LOST: corrupt queue file {:?}; parse error: {}; quarantine also failed: {}",
                        path, e, quarantine_err
                    );
                    return Err(io::Error::new(io::ErrorKind::InvalidData, e));
                }
            },
        };

        info!(
            "[Queue] Loaded {} orders from {:?} (next_id={})",
            queue_data.orders.len(),
            path,
            queue_data.next_id
        );

        Ok(Self {
            orders: queue_data.orders.into_iter().collect(),
            next_id: queue_data.next_id,
        })
    }

    /// Move the file at `path` aside to a `queue.json.<kind>-<unix_ms>-<seq>.json`
    /// sibling. Uses rename → copy+remove fallback so a held handle on Windows
    /// (AV scanner, indexer) doesn't leave the bad bytes at the active path.
    fn quarantine_to(path: &Path, kind: &str) -> io::Result<PathBuf> {
        let unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let seq = ARCHIVE_SEQ.fetch_add(1, Ordering::Relaxed);
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "queue.json".to_string());
        let archived_name = format!("{file_name}.{kind}-{unix_ms}-{seq}.json");
        let archived = match path.parent() {
            Some(parent) => parent.join(archived_name),
            None => PathBuf::from(archived_name),
        };
        archive_aside(path, &archived)?;
        Ok(archived)
    }

    /// Path-parameterized save, separated so tests can round-trip without
    /// touching the production `QUEUE_FILE`.
    fn save_to(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let data = QueuePersist {
            orders: self.orders.iter().cloned().collect(),
            next_id: self.next_id,
        };

        let json = serde_json::to_string_pretty(&data)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        write_atomic(path, &json)
    }

    /// Enqueue a new order.
    ///
    /// Returns `Ok((order_id, 1-indexed position))` on success, `Err(message)`
    /// when either cap is hit (both are recoverable rejections).
    pub fn add(
        &mut self,
        user_uuid: String,
        username: String,
        order_type: QueuedOrderType,
        item: String,
        quantity: u32,
    ) -> Result<(u64, usize), String> {
        self.add_at_path(user_uuid, username, order_type, item, quantity, Path::new(QUEUE_FILE))
    }

    /// Path-parameterized enqueue, separated so tests can simulate a save
    /// failure without touching the production `QUEUE_FILE`.
    fn add_at_path(
        &mut self,
        user_uuid: String,
        username: String,
        order_type: QueuedOrderType,
        item: String,
        quantity: u32,
        path: &Path,
    ) -> Result<(u64, usize), String> {
        // Global backpressure. MAX_ORDERS_PER_USER alone is not enough — a
        // coordinated burst of distinct users could still blow the queue past
        // any memory or latency budget.
        if self.orders.len() >= MAX_QUEUE_SIZE {
            warn!(
                "[Queue] Rejected order from {} ({}): global cap reached ({}/{})",
                username, user_uuid, self.orders.len(), MAX_QUEUE_SIZE
            );
            return Err(format!(
                "The store queue is currently full ({} orders). Please try again later.",
                self.orders.len()
            ));
        }

        let user_count = self.user_order_count(&user_uuid);
        if user_count >= MAX_ORDERS_PER_USER {
            warn!(
                "[Queue] Rejected order from {} ({}): per-user cap reached ({}/{}, queue size {})",
                username, user_uuid, user_count, MAX_ORDERS_PER_USER, self.orders.len()
            );
            return Err(format!(
                "Queue full. You have {} pending orders (max {}). Wait for some to complete.",
                user_count, MAX_ORDERS_PER_USER
            ));
        }

        let id = self.next_id;
        self.next_id += 1;

        let order = QueuedOrder::new(
            id,
            user_uuid.clone(),
            username.clone(),
            order_type,
            item.clone(),
            quantity,
        );
        self.orders.push_back(order);

        let position = self.orders.len();

        // Persist on every mutation so an unexpected shutdown never loses a
        // queued order. If the save fails, roll back the in-memory push so we
        // don't return a "queued" confirmation for an order that won't survive
        // a restart.
        //
        // Also decrement `next_id` on rollback. Prior policy was to "burn" the
        // ID on save failure (treat the ID as already-emitted, safer than
        // reuse), but a flapping permission / disk-full failure can cycle this
        // path arbitrarily many times — `next_id` then grows unboundedly even
        // though no order has ever been persisted. The user-visible failure
        // surface is `Err("…please retry.")`, and we have NOT yet emitted an
        // `info!()` line for this ID (that comes AFTER the successful save
        // below), so no log/player surface has been told about it. Decrement
        // the counter so a future successful add reuses the slot. The only
        // way the ID could have been quoted externally is a prior attempt at
        // the same ID that already failed and rolled back; in that case the
        // counter sat at this value before, so re-issuing it is a no-op.
        if let Err(e) = self.save_to(path) {
            error!(
                "[Queue] Failed to persist after adding order #{}: {} (rolling back)",
                id, e
            );
            self.orders.pop_back();
            // Decrement only if it still matches what we set — defensive
            // against a hypothetical concurrent mutation that bumped the
            // counter again before we got here (today: not possible because
            // `add_at_path` takes `&mut self`, but cheap to make the rollback
            // safe-by-construction).
            if self.next_id == id + 1 {
                self.next_id = id;
            }
            return Err("Queue temporarily unavailable, please retry.".to_string());
        }

        info!(
            "[Queue] Order #{} queued at position {} (user={} uuid={} item={} qty={})",
            id, position, username, user_uuid, item, quantity
        );
        Ok((id, position))
    }

    /// Legacy in-memory-only pop kept for the existing
    /// `add_then_pop_returns_same_order_and_empties_queue` test. Production
    /// code uses [`pop_committed`] (and [`peek_front`]) so the on-disk view
    /// is always consistent with the in-memory queue.
    #[cfg(test)]
    pub fn pop(&mut self) -> Option<QueuedOrder> {
        let order = self.orders.pop_front();

        if let Some(ref o) = order {
            debug!(
                "[Queue] Popped order #{}: {} for {} (remaining: {})",
                o.id, o.description(), o.username, self.orders.len()
            );
        }

        order
    }

    /// Borrow the front order without removing it.
    ///
    /// Used by `Store::process_next_order` to inspect the next order so the
    /// trade-state mirror can be persisted BEFORE the order is dropped from
    /// `queue.json`. See `pop_committed` for the second half of the handover.
    pub fn peek_front(&self) -> Option<&QueuedOrder> {
        self.orders.front()
    }

    /// Persist-then-pop variant of [`pop`] used by the queue→trade-state
    /// handover. Verifies the front order's id still matches `order_id`,
    /// writes the new (popped) queue to disk, and only then removes the
    /// order from the in-memory `VecDeque`.
    ///
    /// Order matters: saving FIRST and popping in-memory SECOND closes the
    /// in-memory-vs-disk divergence window the simpler "pop then save with
    /// rollback" pattern still has on a failed write — the same defect the
    /// `add_at_path` rollback path already mitigates.
    pub fn pop_committed(&mut self, order_id: u64) -> Result<QueuedOrder, String> {
        self.pop_committed_at_path(order_id, Path::new(QUEUE_FILE))
    }

    /// Path-parameterized form of [`pop_committed`], separated so tests can
    /// simulate a save failure without touching the production `QUEUE_FILE`.
    fn pop_committed_at_path(
        &mut self,
        order_id: u64,
        path: &Path,
    ) -> Result<QueuedOrder, String> {
        let front = match self.orders.front() {
            Some(o) => o,
            None => {
                warn!(
                    "[Queue] pop_committed(#{}) called on empty queue",
                    order_id
                );
                return Err("queue head changed: queue is empty".to_string());
            }
        };

        if front.id != order_id {
            warn!(
                "[Queue] pop_committed(#{}) but front is now #{} — queue head changed",
                order_id, front.id
            );
            return Err(format!(
                "queue head changed: expected #{}, found #{}",
                order_id, front.id
            ));
        }

        // Clone front so we can save a "queue without it" view BEFORE mutating
        // the in-memory `VecDeque`. On save failure the in-memory state is
        // untouched, which means a retry on the next tick is a clean re-run.
        let cloned = front.clone();

        let projected = QueuePersist {
            orders: self.orders.iter().skip(1).cloned().collect(),
            next_id: self.next_id,
        };
        let json = serde_json::to_string_pretty(&projected)
            .map_err(|e| format!("failed to serialize queue: {}", e))?;
        if let Err(e) = write_atomic(path, &json) {
            error!(
                "[Queue] Failed to persist queue after popping order #{}: {} (leaving in queue for retry)",
                order_id, e
            );
            return Err(format!("failed to persist queue after pop: {}", e));
        }

        // Save succeeded — now remove from the in-memory queue. This cannot
        // fail (we already verified `front` is `Some`).
        let popped = self.orders.pop_front().expect("front was Some above");
        debug_assert_eq!(popped.id, cloned.id);
        debug!(
            "[Queue] Committed pop of order #{}: {} for {} (remaining: {})",
            popped.id, popped.description(), popped.username, self.orders.len()
        );
        Ok(popped)
    }

    pub fn is_empty(&self) -> bool {
        self.orders.is_empty()
    }

    pub fn len(&self) -> usize {
        self.orders.len()
    }

    #[cfg(test)]
    pub fn get_position(&self, order_id: u64) -> Option<usize> {
        self.orders
            .iter()
            .position(|o| o.id == order_id)
            .map(|p| p + 1)
    }

    #[cfg(test)]
    pub fn get_user_position(&self, user_uuid: &str) -> Option<usize> {
        self.orders
            .iter()
            .position(|o| o.user_uuid == user_uuid)
            .map(|p| p + 1)
    }

    pub fn user_order_count(&self, user_uuid: &str) -> usize {
        self.orders.iter().filter(|o| o.user_uuid == user_uuid).count()
    }

    /// All orders for `user_uuid` paired with their 1-indexed queue position.
    pub fn get_user_orders(&self, user_uuid: &str) -> Vec<(&QueuedOrder, usize)> {
        self.orders
            .iter()
            .enumerate()
            .filter(|(_, o)| o.user_uuid == user_uuid)
            .map(|(i, o)| (o, i + 1))
            .collect()
    }

    /// Cancel `order_id` if it belongs to `user_uuid`. Returns an error when
    /// the order is missing or owned by another user (kept distinct in logs
    /// so operators can tell misuse from a stale client).
    pub fn cancel(&mut self, user_uuid: &str, order_id: u64) -> Result<(), String> {
        self.cancel_at_path(user_uuid, order_id, Path::new(QUEUE_FILE))
    }

    /// Path-parameterized cancel, separated so tests can simulate a save
    /// failure without touching the production `QUEUE_FILE`. On save error
    /// we re-insert the removed order at its original position so the
    /// in-memory queue stays consistent with the on-disk view — otherwise
    /// the player would see a "cancelled" reply for an order that the next
    /// restart would silently resurrect and process.
    fn cancel_at_path(
        &mut self,
        user_uuid: &str,
        order_id: u64,
        path: &Path,
    ) -> Result<(), String> {
        let position = self
            .orders
            .iter()
            .position(|o| o.id == order_id && o.user_uuid == user_uuid);

        match position {
            Some(pos) => {
                let order = self
                    .orders
                    .remove(pos)
                    .expect("position just verified above");
                let description = order.description();

                if let Err(e) = self.save_to(path) {
                    error!(
                        "[Queue] Failed to persist after cancelling order #{}: {} (rolling back)",
                        order_id, e
                    );
                    self.orders.insert(pos, order);
                    return Err("Cancellation failed to persist; please retry.".to_string());
                }

                info!(
                    "[Queue] Order #{} cancelled by uuid={} (was: {}, position {})",
                    order_id, user_uuid, description, pos + 1
                );

                Ok(())
            }
            None => {
                if self.orders.iter().any(|o| o.id == order_id) {
                    warn!(
                        "[Queue] uuid={} tried to cancel order #{} owned by another user",
                        user_uuid, order_id
                    );
                    Err("You can only cancel your own orders.".to_string())
                } else {
                    warn!(
                        "[Queue] uuid={} tried to cancel order #{} but it doesn't exist (queue size {})",
                        user_uuid, order_id, self.orders.len()
                    );
                    Err(format!("Order #{} not found in queue.", order_id))
                }
            }
        }
    }

    /// Rough wait-time hint shown to players; assumes ~30s per order ahead.
    /// Coarse by design — real processing time varies by order type, and this
    /// is only used for a player-facing "you'll be served in ~X" string.
    pub fn estimate_wait(&self, position: usize) -> String {
        let orders_ahead = position.saturating_sub(1);
        if orders_ahead == 0 {
            "next in line".to_string()
        } else {
            let seconds = orders_ahead * 30;
            if seconds < 60 {
                format!("~{}s", seconds)
            } else {
                format!("~{} min", (seconds + 30) / 60)
            }
        }
    }
}

/// On-disk shape for the queue. Field renames break existing queue files.
#[derive(Serialize, Deserialize)]
struct QueuePersist {
    orders: Vec<QueuedOrder>,
    next_id: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scratch directory under the system temp dir, mirroring the pattern in
    /// `fsutil::tests` so queue round-trip tests don't collide with each other
    /// or the real `QUEUE_FILE`.
    struct TmpDir(std::path::PathBuf);

    impl TmpDir {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "cj-store-queue-{}-{}",
                name,
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&base).unwrap();
            Self(base)
        }

        fn path(&self, name: &str) -> std::path::PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Test-only `add` helper that routes the persistence path to a
    /// per-test scratch directory rather than the production `QUEUE_FILE`.
    /// Without this, parallel tests race on `data/queue.json.tmp`/`.bak`
    /// and trip over each other's writes.
    fn add_to(
        queue: &mut OrderQueue,
        path: &Path,
        user_uuid: &str,
        username: &str,
        order_type: QueuedOrderType,
        item: &str,
        quantity: u32,
    ) -> Result<(u64, usize), String> {
        queue.add_at_path(
            user_uuid.to_string(),
            username.to_string(),
            order_type,
            item.to_string(),
            quantity,
            path,
        )
    }

    #[test]
    fn add_then_pop_returns_same_order_and_empties_queue() {
        let dir = TmpDir::new("add-then-pop");
        let path = dir.path("queue.json");
        let mut queue = OrderQueue::new();

        let (id, pos) = add_to(
            &mut queue, &path,
            "uuid1", "player1", QueuedOrderType::Buy, "cobblestone", 64,
        )
        .unwrap();

        assert_eq!(id, 1);
        assert_eq!(pos, 1);
        assert_eq!(queue.len(), 1);

        let order = queue.pop().unwrap();
        assert_eq!(order.id, 1);
        assert_eq!(order.item, "cobblestone");
        assert!(queue.is_empty());
    }

    #[test]
    fn per_user_cap_rejects_ninth_order_but_other_users_unaffected() {
        let dir = TmpDir::new("per-user-cap");
        let path = dir.path("queue.json");
        let mut queue = OrderQueue::new();

        for i in 0..MAX_ORDERS_PER_USER {
            add_to(
                &mut queue, &path,
                "uuid1", "player1", QueuedOrderType::Buy, &format!("item{}", i), 64,
            )
            .expect("within per-user cap");
        }

        let err = add_to(
            &mut queue, &path,
            "uuid1", "player1", QueuedOrderType::Buy, "overflow", 64,
        )
        .expect_err("per-user cap must reject");
        assert!(err.contains(&MAX_ORDERS_PER_USER.to_string()));

        assert!(add_to(
            &mut queue, &path,
            "uuid2", "player2", QueuedOrderType::Buy, "different_user", 64,
        )
        .is_ok());
    }

    #[test]
    fn cancel_rejects_other_users_order_and_accepts_own() {
        let dir = TmpDir::new("cancel-rejects");
        let path = dir.path("queue.json");
        let mut queue = OrderQueue::new();

        let (id1, _) = add_to(
            &mut queue, &path,
            "uuid1", "player1", QueuedOrderType::Buy, "item1", 64,
        )
        .unwrap();

        let (id2, _) = add_to(
            &mut queue, &path,
            "uuid2", "player2", QueuedOrderType::Buy, "item2", 64,
        )
        .unwrap();

        assert!(queue.cancel("uuid1", id2).is_err());
        assert!(queue.cancel("uuid1", id1).is_ok());
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn cancel_missing_order_reports_not_found() {
        let mut queue = OrderQueue::new();
        let err = queue.cancel("uuid1", 9999).expect_err("missing id must fail");
        assert!(err.contains("9999"));
    }

    #[test]
    fn global_cap_rejects_even_fresh_users() {
        let dir = TmpDir::new("global-cap");
        let path = dir.path("queue.json");
        let mut queue = OrderQueue::new();

        for i in 0..MAX_QUEUE_SIZE {
            add_to(
                &mut queue, &path,
                &format!("uuid-{}", i), &format!("player-{}", i),
                QueuedOrderType::Buy, "cobblestone", 1,
            )
            .expect("within global cap");
        }

        let err = add_to(
            &mut queue, &path,
            "uuid-overflow", "overflow-player", QueuedOrderType::Buy, "cobblestone", 1,
        )
        .expect_err("global cap must reject");
        assert!(err.contains("full"));
    }

    #[test]
    fn position_helpers_report_1_indexed_positions() {
        let dir = TmpDir::new("position-helpers");
        let path = dir.path("queue.json");
        let mut queue = OrderQueue::new();

        add_to(&mut queue, &path, "uuid1", "p1", QueuedOrderType::Buy, "a", 1).unwrap();
        let (id2, _) =
            add_to(&mut queue, &path, "uuid2", "p2", QueuedOrderType::Buy, "b", 1).unwrap();
        add_to(&mut queue, &path, "uuid1", "p1", QueuedOrderType::Buy, "c", 1).unwrap();

        assert_eq!(queue.get_user_position("uuid1"), Some(1));
        assert_eq!(queue.get_user_position("uuid2"), Some(2));
        assert_eq!(queue.get_position(id2), Some(2));
        assert_eq!(queue.user_order_count("uuid1"), 2);
    }

    #[test]
    fn get_user_orders_returns_every_match_with_positions() {
        let dir = TmpDir::new("get-user-orders");
        let path = dir.path("queue.json");
        let mut queue = OrderQueue::new();
        add_to(&mut queue, &path, "a", "pa", QueuedOrderType::Buy, "x", 1).unwrap();
        add_to(&mut queue, &path, "b", "pb", QueuedOrderType::Buy, "y", 1).unwrap();
        add_to(&mut queue, &path, "a", "pa", QueuedOrderType::Sell, "z", 2).unwrap();

        let orders = queue.get_user_orders("a");
        assert_eq!(orders.len(), 2);
        assert_eq!(orders[0].1, 1);
        assert_eq!(orders[1].1, 3);
        assert_eq!(orders[0].0.item, "x");
        assert_eq!(orders[1].0.item, "z");
    }

    #[test]
    fn description_renders_every_order_variant() {
        let buy = QueuedOrder::new(1, "u".into(), "p".into(), QueuedOrderType::Buy, "diamond".into(), 5);
        assert_eq!(buy.description(), "buy diamond 5");

        let sell = QueuedOrder::new(2, "u".into(), "p".into(), QueuedOrderType::Sell, "iron".into(), 10);
        assert_eq!(sell.description(), "sell iron 10");

        let dep_some = QueuedOrder::new(
            3, "u".into(), "p".into(),
            QueuedOrderType::Deposit { amount: Some(1.5) }, "diamond".into(), 0,
        );
        assert_eq!(dep_some.description(), "deposit 1.50");

        let dep_flex = QueuedOrder::new(
            4, "u".into(), "p".into(),
            QueuedOrderType::Deposit { amount: None }, "diamond".into(), 0,
        );
        assert_eq!(dep_flex.description(), "deposit (flexible)");

        let wd_some = QueuedOrder::new(
            5, "u".into(), "p".into(),
            QueuedOrderType::Withdraw { amount: Some(2.25) }, "diamond".into(), 0,
        );
        assert_eq!(wd_some.description(), "withdraw 2.25");

        let wd_full = QueuedOrder::new(
            6, "u".into(), "p".into(),
            QueuedOrderType::Withdraw { amount: None }, "diamond".into(), 0,
        );
        assert_eq!(wd_full.description(), "withdraw (full balance)");
    }

    #[test]
    fn estimate_wait_crosses_second_minute_and_next_in_line_boundaries() {
        let queue = OrderQueue::new();
        assert_eq!(queue.estimate_wait(0), "next in line");
        assert_eq!(queue.estimate_wait(1), "next in line");
        // position 2 -> 1 ahead -> 30s, still sub-minute.
        assert_eq!(queue.estimate_wait(2), "~30s");
        // position 3 -> 2 ahead -> 60s, flips to minutes.
        assert_eq!(queue.estimate_wait(3), "~1 min");
        // position 5 -> 4 ahead -> 120s -> 2 min.
        assert_eq!(queue.estimate_wait(5), "~2 min");
    }

    #[test]
    fn default_matches_new() {
        let a = OrderQueue::default();
        let b = OrderQueue::new();
        assert_eq!(a.len(), b.len());
        assert!(a.is_empty());
    }

    #[test]
    fn load_returns_empty_when_file_missing() {
        let dir = TmpDir::new("load-missing");
        let path = dir.path("absent.json");
        let q = OrderQueue::load_from(&path).unwrap();
        assert!(q.is_empty());
        assert_eq!(q.next_id, 1);
    }

    #[test]
    fn save_and_load_round_trip_preserves_orders_and_next_id() {
        let dir = TmpDir::new("roundtrip");
        let path = dir.path("queue.json");

        let mut queue = OrderQueue::new();
        queue.orders.push_back(QueuedOrder::new(
            42, "uuid-a".into(), "alice".into(), QueuedOrderType::Buy, "diamond".into(), 7,
        ));
        queue.orders.push_back(QueuedOrder::new(
            43, "uuid-b".into(), "bob".into(),
            QueuedOrderType::Deposit { amount: Some(1.5) }, "diamond".into(), 0,
        ));
        queue.orders.push_back(QueuedOrder::new(
            44, "uuid-c".into(), "carol".into(),
            QueuedOrderType::Withdraw { amount: None }, "diamond".into(), 0,
        ));
        queue.next_id = 45;

        queue.save_to(&path).expect("save must succeed");

        let loaded = OrderQueue::load_from(&path).expect("load must succeed");
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded.next_id, 45);

        let ids: Vec<u64> = loaded.orders.iter().map(|o| o.id).collect();
        assert_eq!(ids, vec![42, 43, 44]);

        // Every variant shape survives the JSON boundary intact.
        assert!(matches!(loaded.orders[0].order_type, QueuedOrderType::Buy));
        assert!(matches!(
            loaded.orders[1].order_type,
            QueuedOrderType::Deposit { amount: Some(_) }
        ));
        assert!(matches!(
            loaded.orders[2].order_type,
            QueuedOrderType::Withdraw { amount: None }
        ));

        assert_eq!(loaded.orders[0].username, "alice");
        assert_eq!(loaded.orders[0].item, "diamond");
        assert_eq!(loaded.orders[0].quantity, 7);
    }

    #[test]
    fn load_from_returns_empty_on_corrupt_json_after_quarantine() {
        let dir = TmpDir::new("bad-json");
        let path = dir.path("queue.json");
        fs::write(&path, "{ this is not json").unwrap();
        // After T4 fixes: corrupt JSON quarantined, returns Ok(empty),
        // mirroring trade_state's parse-error-with-quarantine-success path.
        let queue = OrderQueue::load_from(&path).expect("quarantine succeeds → Ok");
        assert_eq!(queue.len(), 0);
        assert!(!path.exists(), "corrupt file must be moved aside");
    }

    #[test]
    fn add_at_path_rolls_back_on_save_failure() {
        // Simulate a save failure by pointing `add_at_path` at a destination
        // whose parent is a regular file rather than a directory. The atomic
        // write goes through `fs::create_dir_all` for the parent, which fails
        // portably on Unix and Windows when the parent path is a file.
        let dir = TmpDir::new("add-rollback");
        let parent_as_file = dir.path("not-a-dir");
        fs::write(&parent_as_file, "i am a file, not a directory").unwrap();
        let dest = parent_as_file.join("queue.json");

        let mut queue = OrderQueue::new();
        let next_id_before = queue.next_id;
        let len_before = queue.len();

        let result = queue.add_at_path(
            "uuid-rollback".to_string(),
            "rollback-player".to_string(),
            QueuedOrderType::Buy,
            "diamond".to_string(),
            1,
            &dest,
        );

        // (a) add_at_path returns Err.
        let err = result.expect_err("save failure must surface as Err");
        assert!(
            err.contains("retry"),
            "user-facing error should suggest retry, got: {}",
            err
        );

        // (b) queue.len() unchanged after the failed call (the push was
        // rolled back via pop_back).
        assert_eq!(queue.len(), len_before, "len must roll back on save failure");

        // (c) queue.next_id IS rolled back on save failure. The success-path
        // `info!()` line never ran (it's printed AFTER `save_to` returns Ok)
        // and no player-visible surface has been told the ID, so the safer
        // policy is to release the slot for reuse. Otherwise a flapping
        // permission / disk-full failure would unbound `next_id` even though
        // no order was ever persisted.
        assert_eq!(
            queue.next_id,
            next_id_before,
            "next_id must be decremented on save failure (no log/player surface saw the ID yet)"
        );
    }

    #[test]
    fn pop_committed_save_failure_leaves_queue_intact() {
        // Simulate a save failure by pointing `pop_committed_at_path` at a
        // destination whose parent is a regular file rather than a directory.
        // The atomic write goes through `fs::create_dir_all` for the parent,
        // which fails portably on Unix and Windows when the parent is a file.
        let dir = TmpDir::new("pop-committed-save-fail");
        let parent_as_file = dir.path("not-a-dir");
        fs::write(&parent_as_file, "i am a file, not a directory").unwrap();
        let dest = parent_as_file.join("queue.json");

        let mut queue = OrderQueue::new();
        let writable = dir.path("queue.json");
        let (id, _) = add_to(
            &mut queue, &writable,
            "uuid-stay", "stayplayer", QueuedOrderType::Buy, "diamond", 1,
        )
        .unwrap();

        let len_before = queue.len();
        let front_id_before = queue.peek_front().map(|o| o.id);

        let err = queue
            .pop_committed_at_path(id, &dest)
            .expect_err("save failure must surface as Err");
        assert!(
            err.contains("persist") || err.contains("queue"),
            "error should mention persistence/queue, got: {}",
            err
        );

        assert_eq!(
            queue.len(),
            len_before,
            "in-memory queue must be unchanged on save failure"
        );
        assert_eq!(
            queue.peek_front().map(|o| o.id),
            front_id_before,
            "front order must be unchanged on save failure"
        );
    }

    #[test]
    fn pop_committed_id_mismatch_returns_err() {
        let dir = TmpDir::new("pop-committed-id-mismatch");
        let path = dir.path("queue.json");
        let mut queue = OrderQueue::new();

        let (id, _) = add_to(
            &mut queue, &path,
            "uuid-7", "player7", QueuedOrderType::Buy, "diamond", 1,
        )
        .unwrap();
        assert_eq!(id, 1, "first add gets id 1; reusing variable name to avoid confusion");

        // Pre-load id 7 by burning ids until next_id = 7. Cleanest path: just
        // assert against a wrong id directly.
        let wrong_id: u64 = 99;
        assert_ne!(wrong_id, id, "test sanity: wrong_id must differ");

        let len_before = queue.len();
        let err = queue
            .pop_committed_at_path(wrong_id, &path)
            .expect_err("id mismatch must surface as Err");
        assert!(
            err.contains("queue head changed"),
            "error should mention head change, got: {}",
            err
        );
        assert_eq!(queue.len(), len_before, "queue length must be unchanged");
    }

    #[test]
    fn peek_front_does_not_mutate() {
        let dir = TmpDir::new("peek-front");
        let path = dir.path("queue.json");
        let mut queue = OrderQueue::new();

        // Empty queue: peek returns None and stays empty.
        assert!(queue.peek_front().is_none());
        assert!(queue.is_empty());

        let (id, _) = add_to(
            &mut queue, &path,
            "uuid-peek", "peekplayer", QueuedOrderType::Buy, "diamond", 3,
        )
        .unwrap();

        let len_before = queue.len();
        let peeked_id = queue.peek_front().expect("front present after add").id;
        assert_eq!(peeked_id, id);
        // Peek again — still there, length unchanged.
        let peeked_id2 = queue.peek_front().expect("still present").id;
        assert_eq!(peeked_id2, id);
        assert_eq!(queue.len(), len_before);
    }

    #[test]
    fn cancel_at_path_rolls_back_on_save_failure() {
        // Simulate a save failure by pointing `cancel_at_path` at a destination
        // whose parent is a regular file rather than a directory. The atomic
        // write goes through `fs::create_dir_all` for the parent, which fails
        // portably on Unix and Windows when the parent path is a file.
        let dir = TmpDir::new("cancel-rollback");
        let writable = dir.path("queue.json");
        let parent_as_file = dir.path("not-a-dir");
        fs::write(&parent_as_file, "i am a file, not a directory").unwrap();
        let dest = parent_as_file.join("queue.json");

        let mut queue = OrderQueue::new();

        // Pre-load two orders so we can verify the rollback restores the
        // original front position (not just the count).
        let (id1, _) = add_to(
            &mut queue, &writable,
            "uuid-cancel", "cancelplayer", QueuedOrderType::Buy, "diamond", 1,
        )
        .unwrap();
        add_to(
            &mut queue, &writable,
            "uuid-cancel", "cancelplayer", QueuedOrderType::Sell, "iron", 2,
        )
        .unwrap();

        let len_before = queue.len();
        let front_id_before = queue.peek_front().map(|o| o.id);

        // (a) the call returns Err.
        let err = queue
            .cancel_at_path("uuid-cancel", id1, &dest)
            .expect_err("save failure must surface as Err");
        assert!(
            err.contains("retry"),
            "user-facing error should suggest retry, got: {}",
            err
        );

        // (b) queue.len() unchanged.
        assert_eq!(queue.len(), len_before, "len must roll back on save failure");

        // (c) front order id is the same as before — i.e. the rollback
        // insert restored the original position, not just appended somewhere.
        assert_eq!(
            queue.peek_front().map(|o| o.id),
            front_id_before,
            "front order id must be restored to its original position on rollback"
        );
    }

    #[test]
    fn load_from_moves_corrupt_file_to_timestamped_sidecar() {
        let dir = TmpDir::new("bad-json-sidecar");
        let path = dir.path("queue.json");
        let bad_bytes = "{ this is not json";
        fs::write(&path, bad_bytes).unwrap();

        let queue = OrderQueue::load_from(&path).expect("quarantine succeeds → Ok");
        assert_eq!(queue.len(), 0);

        // (a) A `.corrupt-*` sibling exists next to the original path.
        let parent = path.parent().expect("path has parent");
        let prefix = format!(
            "{}.corrupt-",
            path.file_name().unwrap().to_string_lossy()
        );
        let mut sidecar_found: Option<std::path::PathBuf> = None;
        for entry in fs::read_dir(parent).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) {
                sidecar_found = Some(entry.path());
                break;
            }
        }
        let sidecar = sidecar_found.expect("expected a .corrupt-* sidecar next to queue.json");

        // (b) The original path no longer contains the bad bytes (moved, not copied).
        assert!(
            !path.exists(),
            "original queue.json should have been renamed away, not left in place"
        );

        // Sanity: the sidecar holds the exact bytes we wrote.
        let preserved = fs::read_to_string(&sidecar).unwrap();
        assert_eq!(preserved, bad_bytes);
    }

    #[test]
    fn load_from_quarantines_unreadable_file() {
        // Pre-create a directory at the queue path so read_to_string fails
        // with a non-NotFound error; quarantine must move the directory
        // aside (or fail cleanly) and load_from returns Ok(empty).
        let dir = TmpDir::new("unreadable-queue");
        let path = dir.path("queue.json");
        fs::create_dir_all(&path).unwrap();
        let queue = OrderQueue::load_from(&path).expect("quarantine succeeds → Ok");
        assert_eq!(queue.len(), 0);
        let parent = path.parent().expect("path has parent");
        let prefix = format!(
            "{}.unreadable-",
            path.file_name().unwrap().to_string_lossy()
        );
        let archived = fs::read_dir(parent)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with(&prefix));
        assert!(archived, "expected a queue.json.unreadable-* sibling");
    }

    #[test]
    fn load_from_quarantine_disambiguates_rapid_successive_calls() {
        let dir = TmpDir::new("queue-quarantine-rapid");
        let parent = dir.path("");
        let path1 = parent.join("queue.json");
        fs::write(&path1, "garbage 1").unwrap();
        let _ = OrderQueue::load_from(&path1).unwrap();
        fs::write(&path1, "garbage 2").unwrap();
        let _ = OrderQueue::load_from(&path1).unwrap();
        let count = fs::read_dir(&parent)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("queue.json.corrupt-"))
            .count();
        assert_eq!(count, 2, "two rapid quarantines must produce two distinct sibling files");
    }
}
