//! # Constants Module
//!
//! Centralized constants for timeouts, delays, and other configurable values.
//! This makes it easier to tune the bot's behavior and document what each value means.

// ============================================================================
// INVENTORY CONSTANTS
// ============================================================================

/// Number of slots in a double chest (54 = 6 rows × 9 columns)
pub const DOUBLE_CHEST_SLOTS: usize = 54;

/// Number of slots in a shulker box (27 = 3 rows × 9 columns).
pub const SHULKER_BOX_SLOTS: usize = 27;

/// Hotbar slot 0 in inventory slot numbering (36-44 are hotbar slots).
/// Minecraft's container protocol numbers slots contiguously: 0-8 are crafting/armor,
/// 9-35 are the main inventory, and 36-44 are the hotbar. Add the hotbar index (0-8)
/// to this constant to address a specific hotbar slot.
pub const HOTBAR_SLOT_0: usize = 36;

// ============================================================================
// TIMEOUT CONSTANTS (in milliseconds)
// ============================================================================

/// Timeout for trade operations (45 seconds). Canonical default for the
/// `trade_timeout_ms` config field.
pub const TRADE_TIMEOUT_MS: u64 = 45_000;

/// Timeout for complete chest operations (seconds).
/// This needs to be generous because operations may involve:
/// - Navigating to the chest
/// - Opening multiple shulkers (if some are full/empty)
/// - Breaking and picking up shulkers
/// - Walking to collect dropped items
/// - Waiting for item drop settle time (4s per shulker)
///
/// 90 seconds should handle even complex multi-shulker operations.
pub const CHEST_OP_TIMEOUT_SECS: u64 = 90;

/// Timeout for pathfinding operations (60 seconds). Canonical default for
/// the `pathfinding_timeout_ms` config field.
pub const PATHFINDING_TIMEOUT_MS: u64 = 60_000;

// ============================================================================
// DELAY CONSTANTS (in milliseconds)
// ============================================================================
// NOTE: These delays are intentionally generous to handle server lag and
// ensure reliable operations. Do not reduce without thorough testing.

/// Short delay for quick inventory operations
pub const DELAY_SHORT_MS: u64 = 100;

/// Interval between pathfinding position checks (milliseconds).
/// Shorter intervals mean faster reaction to "arrived at goal", at the cost
/// of extra lock-acquire churn on the entity position component.
pub const PATHFINDING_CHECK_INTERVAL_MS: u64 = 100;

/// Medium delay for standard operations
pub const DELAY_MEDIUM_MS: u64 = 200;

/// Delay after a click / interact that updates container state.
/// Slightly longer than `DELAY_MEDIUM_MS` to give the server time to
/// echo the new slot contents before the next read.
pub const DELAY_INTERACT_MS: u64 = 300;

/// Delay after placing/breaking blocks
pub const DELAY_BLOCK_OP_MS: u64 = 350;

/// Delay after looking at a block before interacting with it
pub const DELAY_LOOK_AT_MS: u64 = 250;

/// Long settle delay used after multi-step shulker / pickup sequences,
/// where item-drop physics or chunk updates need extra time to converge
/// before the next read or click.
pub const DELAY_SETTLE_MS: u64 = 500;

/// Delay between chest operations during validation (allows server to process)
pub const DELAY_VALIDATION_BETWEEN_CHESTS_MS: u64 = 750;

/// Delay after placing shulker on station
pub const DELAY_SHULKER_PLACE_MS: u64 = 750;

/// Delay after block-interact / trade-menu open events where the container
/// content packet is in flight. Sits between `DELAY_BLOCK_OP_MS` (350) and
/// `DELAY_SETTLE_MS` (500) — empirically the shortest wait that reliably
/// produces a sync'd shulker-open or trade-menu inventory read. Shared by
/// `bot/shulker::open_shulker_at_station_once` (after `block_interact`) and
/// `bot/trade::place_items_from_inventory_into_trade` (after trade GUI open).
pub const DELAY_CONTAINER_SYNC_MS: u64 = 450;

/// Delay for disconnect operations (2 seconds)
pub const DELAY_DISCONNECT_MS: u64 = 2_000;

/// Debounce window for config file-change events (milliseconds).
/// Editors typically emit a burst of writes on save (rename-over-old, metadata
/// touch, final write); we want exactly one reload per user edit, so we wait
/// this long after the first event before reloading and drain anything that
/// arrived in the meantime.
pub const DELAY_CONFIG_DEBOUNCE_MS: u64 = 500;

// ============================================================================
// RETRY CONSTANTS
// ============================================================================

/// Maximum number of retries for chest operations (non-chunk-related failures)
pub const CHEST_OP_MAX_RETRIES: u32 = 3;

/// Extra retries added when a chunk-not-loaded condition is detected.
/// Chunks typically reload within ~10s on most servers, so we allow more
/// attempts with a longer base delay before giving up.
pub const CHUNK_RELOAD_EXTRA_RETRIES: u32 = 2;

/// Base delay (ms) when waiting for chunks to reload. Longer than the normal
/// retry base because chunk loading is a server-side operation that can take
/// several seconds, especially on busy or low-TPS servers.
pub const CHUNK_RELOAD_BASE_DELAY_MS: u64 = 3_000;

/// Maximum delay (ms) when waiting for chunks to reload.
pub const CHUNK_RELOAD_MAX_DELAY_MS: u64 = 10_000;

/// Maximum number of retries for shulker operations
pub const SHULKER_OP_MAX_RETRIES: u32 = 2;

/// Maximum number of retries for navigation
pub const NAVIGATION_MAX_RETRIES: u32 = 2;

/// Base delay for exponential backoff (milliseconds)
pub const RETRY_BASE_DELAY_MS: u64 = 500;

/// Maximum delay for exponential backoff (milliseconds)
pub const RETRY_MAX_DELAY_MS: u64 = 5_000;

/// Calculate exponential backoff delay.
/// 
/// # Arguments
/// * `attempt` - Current attempt number (0-indexed)
/// * `base_ms` - Base delay in milliseconds
/// * `max_ms` - Maximum delay in milliseconds
/// 
/// # Returns
/// Delay in milliseconds with exponential backoff: `base * 2^attempt`, capped at `max_ms`
pub fn exponential_backoff_delay(attempt: u32, base_ms: u64, max_ms: u64) -> u64 {
    // Clamp the shift amount to 10 to avoid shifting past u64 range on pathological
    // attempt counts; `max_ms` will dominate well before this limit matters in practice.
    let delay = base_ms.saturating_mul(1u64 << attempt.min(10));
    delay.min(max_ms)
}

// ============================================================================
// VALIDATION CONSTANTS
// ============================================================================

/// Minimum valid fee (0%)
pub const FEE_MIN: f64 = 0.0;

/// Maximum valid fee (100%)
pub const FEE_MAX: f64 = 1.0;

/// Maximum reasonable quantity for a single transaction
pub const MAX_TRANSACTION_QUANTITY: i32 = 1_000_000;

/// Minimum reserve before price calculation becomes unreliable.
/// Pricing formulas typically divide by reserve; values this small cause
/// numerical blow-up and unrealistic prices, so the bot should refuse to
/// quote trades when a reserve falls below this threshold.
pub const MIN_RESERVE_FOR_PRICE: f64 = 0.001;

// ============================================================================
// NODE CONSTANTS
// ============================================================================

/// Number of chests per node
pub const CHESTS_PER_NODE: usize = 4;

/// Spacing between nodes in blocks
pub const NODE_SPACING: i32 = 3;

// ============================================================================
// SPECIAL CHEST CONSTANTS
// ============================================================================

/// Item name for the overflow/failsafe chest (node 0, chest 1).
/// This chest accepts any items the bot doesn't know what to do with,
/// such as leftover items from failed operations or unexpected items.
/// The bot will only deposit into this chest, never withdraw.
/// This is the only chest that allows mixed item types in its shulkers.
pub const OVERFLOW_CHEST_ITEM: &str = "overflow";

/// Item name for the base currency chest (node 0, chest 0).
/// This is the item used as the store's currency for all trading pairs.
/// All pair prices and user balances are denominated in this item.
pub const BASE_CURRENCY_ITEM: &str = "diamond";

/// Chest ID for diamond storage (node 0, chest 0)
pub const DIAMOND_CHEST_ID: i32 = 0;

/// Chest ID for overflow storage (node 0, chest 1)
pub const OVERFLOW_CHEST_ID: i32 = 1;

// ============================================================================
// ORDER QUEUE CONSTANTS
// ============================================================================

/// Maximum number of orders a single user can have queued at once
pub const MAX_ORDERS_PER_USER: usize = 8;

/// Global cap on the number of orders across all users.
/// Provides backpressure against overload independent of the per-user cap,
/// so a coordinated burst of many users can't exhaust bot memory or stall
/// processing latency into hours.
pub const MAX_QUEUE_SIZE: usize = 128;

/// File path for persisting the order queue
pub const QUEUE_FILE: &str = "data/queue.json";

// ============================================================================
// RATE LIMITING CONSTANTS
// ============================================================================

/// Base cooldown between messages (milliseconds)
/// Players must wait at least this long between commands
pub const RATE_LIMIT_BASE_COOLDOWN_MS: u64 = 2_000;

// ============================================================================
// UUID CACHE CONSTANTS
// ============================================================================

/// Time-to-live for cached Mojang UUID lookups (seconds).
/// 5 minutes balances freshness (username changes are rare) against API load.
pub const UUID_CACHE_TTL_SECS: u64 = 300;

/// Maximum cooldown after repeated violations (milliseconds)
/// Even with exponential backoff, cooldown won't exceed this
pub const RATE_LIMIT_MAX_COOLDOWN_MS: u64 = 60_000;

/// Time after which violation count resets if user stops spamming (milliseconds)
/// After this duration of no messages, consecutive_violations resets to 0
pub const RATE_LIMIT_RESET_AFTER_MS: u64 = 30_000;

// ============================================================================
// PERIODIC CLEANUP CONSTANTS
// ============================================================================

/// Interval between periodic maintenance sweeps (seconds).
/// Hourly is frequent enough to keep caches bounded under normal load
/// without adding noticeable overhead on an otherwise idle store.
pub const CLEANUP_INTERVAL_SECS: u64 = 3_600;

/// Rate-limiter entries older than this are dropped by the periodic sweep (seconds).
/// Five minutes is well past any legitimate cooldown window, so the entry
/// cannot still be throttling a user when it is removed.
pub const RATE_LIMIT_STALE_AFTER_SECS: u64 = 300;
