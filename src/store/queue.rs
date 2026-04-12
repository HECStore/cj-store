//! # Order Queue System
//!
//! Manages queued orders (buy, sell, deposit, withdraw) for sequential processing.
//! Orders are processed one at a time to prevent race conditions and ensure
//! reliable bot operations.
//!
//! ## Features
//! - FIFO queue with persistence to disk
//! - Per-user order limit (max 8 orders)
//! - Position tracking for user feedback
//! - Order cancellation
//!
//! ## Data Flow
//! 1. Player sends command -> validated and queued
//! 2. Player gets immediate response with queue position
//! 3. Orders processed sequentially by Store::run() loop
//! 4. Player notified when order starts and completes

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

use crate::constants::{MAX_ORDERS_PER_USER, QUEUE_FILE};
use crate::fsutil::write_atomic;
use crate::messages::QueuedOrderType;

/// A queued order waiting to be processed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedOrder {
    /// Unique identifier for this order
    pub id: u64,
    /// UUID of the user who placed the order
    pub user_uuid: String,
    /// Username of the user (for messaging)
    pub username: String,
    /// Type of order (Buy, Sell, Deposit, Withdraw)
    pub order_type: QueuedOrderType,
    /// Item being traded (for buy/sell) or "diamond" (for deposit/withdraw)
    pub item: String,
    /// Quantity of items (for buy/sell) or 0 for flexible deposit/withdraw
    pub quantity: u32,
    /// When the order was queued
    pub queued_at: DateTime<Utc>,
}

impl QueuedOrder {
    /// Create a new queued order
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

    /// Get a human-readable description of the order
    pub fn description(&self) -> String {
        match &self.order_type {
            QueuedOrderType::Buy => {
                format!("buy {} {}", self.item, self.quantity)
            }
            QueuedOrderType::Sell => {
                format!("sell {} {}", self.item, self.quantity)
            }
            QueuedOrderType::Deposit { amount } => {
                if let Some(amt) = amount {
                    format!("deposit {:.2}", amt)
                } else {
                    "deposit (flexible)".to_string()
                }
            }
            QueuedOrderType::Withdraw { amount } => {
                if let Some(amt) = amount {
                    format!("withdraw {:.2}", amt)
                } else {
                    "withdraw (full balance)".to_string()
                }
            }
        }
    }
}

/// The order queue manager
#[derive(Debug)]
pub struct OrderQueue {
    /// Queue of orders waiting to be processed (FIFO)
    orders: VecDeque<QueuedOrder>,
    /// Next order ID to assign
    next_id: u64,
}

impl Default for OrderQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl OrderQueue {
    /// Create a new empty order queue
    pub fn new() -> Self {
        Self {
            orders: VecDeque::new(),
            next_id: 1,
        }
    }

    /// Load queue from disk, or create empty queue if file doesn't exist.
    ///
    /// Called at startup to restore any orders that were pending when the bot
    /// last shut down, so players don't lose their place in line across restarts.
    pub fn load() -> io::Result<Self> {
        let path = Path::new(QUEUE_FILE);
        
        if !path.exists() {
            info!("No queue file found, starting with empty queue");
            return Ok(Self::new());
        }

        let contents = fs::read_to_string(path)?;
        let queue_data: QueuePersist = serde_json::from_str(&contents)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        info!(
            "Loaded queue from disk: {} orders, next_id={}",
            queue_data.orders.len(),
            queue_data.next_id
        );

        Ok(Self {
            orders: queue_data.orders.into_iter().collect(),
            next_id: queue_data.next_id,
        })
    }

    /// Save queue to disk atomically.
    ///
    /// Uses `write_atomic` (write-to-temp + rename) so a crash mid-write cannot
    /// leave a truncated/corrupt queue file on disk.
    pub fn save(&self) -> io::Result<()> {
        let data = QueuePersist {
            orders: self.orders.iter().cloned().collect(),
            next_id: self.next_id,
        };

        let json = serde_json::to_string_pretty(&data)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        write_atomic(QUEUE_FILE, &json)
    }

    /// Add a new order to the queue
    ///
    /// # Arguments
    /// * `user_uuid` - UUID of the user placing the order
    /// * `username` - Username for messaging
    /// * `order_type` - Type of order
    /// * `item` - Item name
    /// * `quantity` - Quantity (0 for flexible deposit/withdraw)
    ///
    /// # Returns
    /// * `Ok((order_id, position))` - Order was queued, returns ID and 1-indexed position
    /// * `Err(message)` - Queue is full for this user
    pub fn add(
        &mut self,
        user_uuid: String,
        username: String,
        order_type: QueuedOrderType,
        item: String,
        quantity: u32,
    ) -> Result<(u64, usize), String> {
        // Enforce MAX_ORDERS_PER_USER to prevent a single player from flooding
        // the queue and blocking other users behind a long tail of their orders.
        let user_count = self.user_order_count(&user_uuid);
        if user_count >= MAX_ORDERS_PER_USER {
            warn!("[Queue] User {} rejected: already has {} orders (max {})", username, user_count, MAX_ORDERS_PER_USER);
            return Err(format!(
                "Queue full. You have {} pending orders (max {}). Wait for some to complete.",
                user_count, MAX_ORDERS_PER_USER
            ));
        }

        let id = self.next_id;
        self.next_id += 1;

        let order = QueuedOrder::new(id, user_uuid.clone(), username.clone(), order_type, item.clone(), quantity);
        self.orders.push_back(order);

        let position = self.orders.len(); // 1-indexed position

        // Persist on every mutation so an unexpected shutdown never loses a queued order.
        if let Err(e) = self.save() {
            error!("[Queue] Failed to persist after adding order {}: {}", id, e);
        }

        info!("[Queue] Order {} added to queue at position {} (user={} item={} qty={})", 
              id, position, username, item, quantity);
        Ok((id, position))
    }

    /// Pop the next order from the front of the queue
    pub fn pop(&mut self) -> Option<QueuedOrder> {
        let order = self.orders.pop_front();

        if let Some(ref o) = order {
            debug!("[Queue] Popped order #{}: {} for {} (remaining: {})",
                   o.id, o.description(), o.username, self.orders.len());
            if let Err(e) = self.save() {
                error!("[Queue] Failed to persist after popping order #{}: {}", o.id, e);
            }
        }

        order
    }

    /// Check if the queue is empty
    pub fn is_empty(&self) -> bool {
        self.orders.is_empty()
    }

    /// Get the total number of orders in the queue
    pub fn len(&self) -> usize {
        self.orders.len()
    }

    /// Get 1-indexed position of an order by ID (test-only helper).
    #[cfg(test)]
    pub fn get_position(&self, order_id: u64) -> Option<usize> {
        self.orders
            .iter()
            .position(|o| o.id == order_id)
            .map(|p| p + 1)
    }

    /// Get 1-indexed position of a user's first order (test-only helper).
    #[cfg(test)]
    pub fn get_user_position(&self, user_uuid: &str) -> Option<usize> {
        self.orders
            .iter()
            .position(|o| o.user_uuid == user_uuid)
            .map(|p| p + 1)
    }

    /// Count how many orders a user has in the queue
    pub fn user_order_count(&self, user_uuid: &str) -> usize {
        self.orders.iter().filter(|o| o.user_uuid == user_uuid).count()
    }

    /// Get all orders for a specific user with their positions
    /// Returns Vec of (order, 1-indexed position)
    pub fn get_user_orders(&self, user_uuid: &str) -> Vec<(&QueuedOrder, usize)> {
        self.orders
            .iter()
            .enumerate()
            .filter(|(_, o)| o.user_uuid == user_uuid)
            .map(|(i, o)| (o, i + 1)) // Convert to 1-indexed
            .collect()
    }

    /// Cancel an order by ID (only if it belongs to the user)
    ///
    /// # Returns
    /// * `Ok(())` - Order was cancelled
    /// * `Err(message)` - Order not found or doesn't belong to user
    pub fn cancel(&mut self, user_uuid: &str, order_id: u64) -> Result<(), String> {
        let position = self.orders
            .iter()
            .position(|o| o.id == order_id && o.user_uuid == user_uuid);

        match position {
            Some(pos) => {
                let order = self.orders.remove(pos).unwrap();
                info!(
                    "[Queue] Order #{} CANCELLED by user {} (was: {}, position was {})",
                    order_id, user_uuid, order.description(), pos + 1
                );

                if let Err(e) = self.save() {
                    error!("[Queue] Failed to persist after cancelling order {}: {}", order_id, e);
                }

                Ok(())
            }
            None => {
                // Check if order exists but belongs to someone else
                if self.orders.iter().any(|o| o.id == order_id) {
                    warn!("[Queue] User {} tried to cancel order #{} but it belongs to another user", user_uuid, order_id);
                    Err("You can only cancel your own orders.".to_string())
                } else {
                    warn!("[Queue] User {} tried to cancel order #{} but it doesn't exist", user_uuid, order_id);
                    Err(format!("Order #{} not found in queue.", order_id))
                }
            }
        }
    }

    /// Estimate wait time based on position (rough estimate)
    /// Assumes ~30 seconds per order (actual time varies by order type).
    /// This is only used for player-facing "you'll be served in ~X" hints,
    /// so a coarse constant is preferred over a real moving average.
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

/// Serializable form of the queue for persistence
#[derive(Serialize, Deserialize)]
struct QueuePersist {
    orders: Vec<QueuedOrder>,
    next_id: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_pop() {
        let mut queue = OrderQueue::new();
        
        let (id, pos) = queue.add(
            "uuid1".to_string(),
            "player1".to_string(),
            QueuedOrderType::Buy,
            "cobblestone".to_string(),
            64,
        ).unwrap();

        assert_eq!(id, 1);
        assert_eq!(pos, 1);
        assert_eq!(queue.len(), 1);

        let order = queue.pop().unwrap();
        assert_eq!(order.id, 1);
        assert_eq!(order.item, "cobblestone");
        assert!(queue.is_empty());
    }

    #[test]
    fn test_user_limit() {
        let mut queue = OrderQueue::new();
        
        // Add MAX_ORDERS_PER_USER orders
        for i in 0..MAX_ORDERS_PER_USER {
            let result = queue.add(
                "uuid1".to_string(),
                "player1".to_string(),
                QueuedOrderType::Buy,
                format!("item{}", i),
                64,
            );
            assert!(result.is_ok());
        }

        // Next order should fail
        let result = queue.add(
            "uuid1".to_string(),
            "player1".to_string(),
            QueuedOrderType::Buy,
            "overflow".to_string(),
            64,
        );
        assert!(result.is_err());

        // But a different user can still add
        let result = queue.add(
            "uuid2".to_string(),
            "player2".to_string(),
            QueuedOrderType::Buy,
            "different_user".to_string(),
            64,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_cancel() {
        let mut queue = OrderQueue::new();
        
        let (id1, _) = queue.add(
            "uuid1".to_string(),
            "player1".to_string(),
            QueuedOrderType::Buy,
            "item1".to_string(),
            64,
        ).unwrap();

        let (id2, _) = queue.add(
            "uuid2".to_string(),
            "player2".to_string(),
            QueuedOrderType::Buy,
            "item2".to_string(),
            64,
        ).unwrap();

        // Can't cancel someone else's order
        assert!(queue.cancel("uuid1", id2).is_err());

        // Can cancel own order
        assert!(queue.cancel("uuid1", id1).is_ok());
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn test_position_tracking() {
        let mut queue = OrderQueue::new();
        
        queue.add("uuid1".to_string(), "p1".to_string(), QueuedOrderType::Buy, "a".to_string(), 1).unwrap();
        let (id2, _) = queue.add("uuid2".to_string(), "p2".to_string(), QueuedOrderType::Buy, "b".to_string(), 1).unwrap();
        queue.add("uuid1".to_string(), "p1".to_string(), QueuedOrderType::Buy, "c".to_string(), 1).unwrap();

        // User 1 has position 1 (first order)
        assert_eq!(queue.get_user_position("uuid1"), Some(1));
        // User 2 has position 2
        assert_eq!(queue.get_user_position("uuid2"), Some(2));
        // Order 2 is at position 2
        assert_eq!(queue.get_position(id2), Some(2));
        // User 1 has 2 orders
        assert_eq!(queue.user_order_count("uuid1"), 2);
    }
}
