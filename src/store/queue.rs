//! Persistent FIFO order queue.
//!
//! Orders (buy/sell/deposit/withdraw) land here the moment a player command is
//! validated, and are processed one at a time by `Store::run()`. Persisting on
//! every mutation means a restart can't lose a player's place in line.

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

use crate::constants::{MAX_ORDERS_PER_USER, MAX_QUEUE_SIZE, QUEUE_FILE};
use crate::fsutil::write_atomic;
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
    fn load_from(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();

        if !path.exists() {
            info!("[Queue] No queue file at {:?}, starting empty", path);
            return Ok(Self::new());
        }

        let contents = fs::read_to_string(path)?;
        let queue_data: QueuePersist = match serde_json::from_str(&contents) {
            Ok(q) => q,
            Err(e) => {
                // Preserve the raw bytes on a timestamped sidecar before the
                // caller falls back to an empty queue. Colons are stripped
                // from the RFC3339 stamp because Windows (and some other
                // filesystems) reject them in filenames.
                let stamp = Utc::now().to_rfc3339().replace(':', "-");
                let sidecar = {
                    let mut os = path.as_os_str().to_os_string();
                    os.push(format!(".corrupt-{}", stamp));
                    std::path::PathBuf::from(os)
                };
                match fs::rename(path, &sidecar) {
                    Ok(()) => error!(
                        "[Queue] PENDING ORDERS LOST: corrupt queue file {:?} moved to {:?}; parse error: {}",
                        path, sidecar, e
                    ),
                    Err(rename_err) => error!(
                        "[Queue] PENDING ORDERS LOST: corrupt queue file {:?}; parse error: {}; failed to move to {:?}: {}",
                        path, e, sidecar, rename_err
                    ),
                }
                return Err(io::Error::new(io::ErrorKind::InvalidData, e));
            }
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

        // Persist on every mutation so an unexpected shutdown never loses a queued order.
        if let Err(e) = self.save() {
            error!("[Queue] Failed to persist after adding order #{}: {}", id, e);
        }

        info!(
            "[Queue] Order #{} queued at position {} (user={} uuid={} item={} qty={})",
            id, position, username, user_uuid, item, quantity
        );
        Ok((id, position))
    }

    pub fn pop(&mut self) -> Option<QueuedOrder> {
        let order = self.orders.pop_front();

        if let Some(ref o) = order {
            debug!(
                "[Queue] Popped order #{}: {} for {} (remaining: {})",
                o.id, o.description(), o.username, self.orders.len()
            );
            if let Err(e) = self.save() {
                error!("[Queue] Failed to persist after popping order #{}: {}", o.id, e);
            }
        }

        order
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
        let position = self
            .orders
            .iter()
            .position(|o| o.id == order_id && o.user_uuid == user_uuid);

        match position {
            Some(pos) => {
                let order = self.orders.remove(pos).unwrap();
                info!(
                    "[Queue] Order #{} cancelled by uuid={} (was: {}, position {})",
                    order_id, user_uuid, order.description(), pos + 1
                );

                if let Err(e) = self.save() {
                    error!(
                        "[Queue] Failed to persist after cancelling order #{}: {}",
                        order_id, e
                    );
                }

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

    #[test]
    fn add_then_pop_returns_same_order_and_empties_queue() {
        let mut queue = OrderQueue::new();

        let (id, pos) = queue
            .add(
                "uuid1".to_string(),
                "player1".to_string(),
                QueuedOrderType::Buy,
                "cobblestone".to_string(),
                64,
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
        let mut queue = OrderQueue::new();

        for i in 0..MAX_ORDERS_PER_USER {
            queue
                .add(
                    "uuid1".to_string(),
                    "player1".to_string(),
                    QueuedOrderType::Buy,
                    format!("item{}", i),
                    64,
                )
                .expect("within per-user cap");
        }

        let err = queue
            .add(
                "uuid1".to_string(),
                "player1".to_string(),
                QueuedOrderType::Buy,
                "overflow".to_string(),
                64,
            )
            .expect_err("per-user cap must reject");
        assert!(err.contains(&MAX_ORDERS_PER_USER.to_string()));

        assert!(queue
            .add(
                "uuid2".to_string(),
                "player2".to_string(),
                QueuedOrderType::Buy,
                "different_user".to_string(),
                64,
            )
            .is_ok());
    }

    #[test]
    fn cancel_rejects_other_users_order_and_accepts_own() {
        let mut queue = OrderQueue::new();

        let (id1, _) = queue
            .add(
                "uuid1".to_string(),
                "player1".to_string(),
                QueuedOrderType::Buy,
                "item1".to_string(),
                64,
            )
            .unwrap();

        let (id2, _) = queue
            .add(
                "uuid2".to_string(),
                "player2".to_string(),
                QueuedOrderType::Buy,
                "item2".to_string(),
                64,
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
        let mut queue = OrderQueue::new();

        for i in 0..MAX_QUEUE_SIZE {
            queue
                .add(
                    format!("uuid-{}", i),
                    format!("player-{}", i),
                    QueuedOrderType::Buy,
                    "cobblestone".to_string(),
                    1,
                )
                .expect("within global cap");
        }

        let err = queue
            .add(
                "uuid-overflow".to_string(),
                "overflow-player".to_string(),
                QueuedOrderType::Buy,
                "cobblestone".to_string(),
                1,
            )
            .expect_err("global cap must reject");
        assert!(err.contains("full"));
    }

    #[test]
    fn position_helpers_report_1_indexed_positions() {
        let mut queue = OrderQueue::new();

        queue
            .add("uuid1".to_string(), "p1".to_string(), QueuedOrderType::Buy, "a".to_string(), 1)
            .unwrap();
        let (id2, _) = queue
            .add("uuid2".to_string(), "p2".to_string(), QueuedOrderType::Buy, "b".to_string(), 1)
            .unwrap();
        queue
            .add("uuid1".to_string(), "p1".to_string(), QueuedOrderType::Buy, "c".to_string(), 1)
            .unwrap();

        assert_eq!(queue.get_user_position("uuid1"), Some(1));
        assert_eq!(queue.get_user_position("uuid2"), Some(2));
        assert_eq!(queue.get_position(id2), Some(2));
        assert_eq!(queue.user_order_count("uuid1"), 2);
    }

    #[test]
    fn get_user_orders_returns_every_match_with_positions() {
        let mut queue = OrderQueue::new();
        queue.add("a".into(), "pa".into(), QueuedOrderType::Buy, "x".into(), 1).unwrap();
        queue.add("b".into(), "pb".into(), QueuedOrderType::Buy, "y".into(), 1).unwrap();
        queue.add("a".into(), "pa".into(), QueuedOrderType::Sell, "z".into(), 2).unwrap();

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
    fn load_from_rejects_malformed_json_as_invalid_data() {
        let dir = TmpDir::new("bad-json");
        let path = dir.path("queue.json");
        fs::write(&path, "{ this is not json").unwrap();
        let err = OrderQueue::load_from(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn load_from_moves_corrupt_file_to_timestamped_sidecar() {
        let dir = TmpDir::new("bad-json-sidecar");
        let path = dir.path("queue.json");
        let bad_bytes = "{ this is not json";
        fs::write(&path, bad_bytes).unwrap();

        let err = OrderQueue::load_from(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);

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
}
