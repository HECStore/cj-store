//! # Constants Module
//!
//! Centralized constants for timeouts, delays, and other configurable values.
//! This makes it easier to tune the bot's behavior and document what each value means.

#![allow(dead_code)]

// ============================================================================
// INVENTORY CONSTANTS
// ============================================================================

/// Number of slots in a double chest (54 = 6 rows × 9 columns)
pub const DOUBLE_CHEST_SLOTS: usize = 54;

/// Number of slots in a shulker box (27 = 3 rows × 9 columns)
pub const SHULKER_BOX_SLOTS: usize = 27;

/// Maximum stack size for most items
pub const DEFAULT_STACK_SIZE: i32 = 64;

/// Hotbar slot 0 in inventory slot numbering (36-44 are hotbar slots).
/// Minecraft's container protocol numbers slots contiguously: 0-8 are crafting/armor,
/// 9-35 are the main inventory, and 36-44 are the hotbar. Add the hotbar index (0-8)
/// to this constant to address a specific hotbar slot.
pub const HOTBAR_SLOT_0: usize = 36;

/// First inventory slot (non-hotbar) in player inventory.
/// Slots 0-8 are reserved for crafting grid and armor, so the main inventory starts at 9.
pub const INVENTORY_SLOT_START: usize = 9;

/// Last inventory slot (non-hotbar) in player inventory.
/// The main inventory ends at 35; slot 36 is where the hotbar begins.
pub const INVENTORY_SLOT_END: usize = 35;

// ============================================================================
// TIMEOUT CONSTANTS (in milliseconds)
// ============================================================================

/// Timeout for opening a chest container (15 seconds = 300 ticks at 20 TPS).
/// Measured in ticks because the chest-open handler runs inside the client's
/// tick loop rather than on a wall-clock timer.
pub const CHEST_OPEN_TIMEOUT_TICKS: u32 = 300;

/// Timeout for trade operations (45 seconds)
pub const TRADE_TIMEOUT_MS: u64 = 45_000;

/// Timeout for trade GUI wait loops (30 seconds)
pub const TRADE_WAIT_TIMEOUT_MS: u64 = 30_000;

/// Timeout for complete chest operations (seconds).
/// This needs to be generous because operations may involve:
/// - Navigating to the chest
/// - Opening multiple shulkers (if some are full/empty)
/// - Breaking and picking up shulkers
/// - Walking to collect dropped items
/// - Waiting for item drop settle time (4s per shulker)
/// 90 seconds should handle even complex multi-shulker operations.
pub const CHEST_OP_TIMEOUT_SECS: u64 = 90;

/// Timeout for pathfinding operations (60 seconds)
pub const PATHFINDING_TIMEOUT_MS: u64 = 60_000;

/// Timeout for waiting for client initialization during reconnect
pub const CLIENT_INIT_TIMEOUT_MS: u64 = 20_000;

// ============================================================================
// DELAY CONSTANTS (in milliseconds)
// ============================================================================
// NOTE: These delays are intentionally generous to handle server lag and
// ensure reliable operations. Do not reduce without thorough testing.

/// Short delay for quick inventory operations
pub const DELAY_SHORT_MS: u64 = 100;

/// Medium delay for standard operations
pub const DELAY_MEDIUM_MS: u64 = 200;

/// Delay after placing/breaking blocks
pub const DELAY_BLOCK_OP_MS: u64 = 350;

/// Delay after looking at a block before interacting with it
pub const DELAY_LOOK_AT_MS: u64 = 250;

/// Delay for network operations that need server round-trip
pub const DELAY_NETWORK_MS: u64 = 450;

/// Delay between chest operations during validation (allows server to process)
pub const DELAY_VALIDATION_BETWEEN_CHESTS_MS: u64 = 750;

/// Delay after placing shulker on station
pub const DELAY_SHULKER_PLACE_MS: u64 = 750;

/// Delay for disconnect operations (2 seconds)
pub const DELAY_DISCONNECT_MS: u64 = 2_000;

/// Additional buffer after disconnect for TCP cleanup.
/// Without this extra pause, reconnect attempts can race the OS releasing
/// the old socket and fail with "address in use" or half-closed state errors.
pub const DELAY_DISCONNECT_BUFFER_MS: u64 = 1_000;

// ============================================================================
// RECONNECTION CONSTANTS
// ============================================================================

/// Initial backoff delay for reconnection attempts
pub const RECONNECT_INITIAL_BACKOFF_SECS: u64 = 2;

/// Maximum backoff delay for reconnection attempts
pub const RECONNECT_MAX_BACKOFF_SECS: u64 = 60;

/// Tick interval for checking connection status
pub const CONNECTION_CHECK_INTERVAL_SECS: u64 = 1;

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

/// Chest ID for diamond storage (node 0, chest 0)
pub const DIAMOND_CHEST_ID: i32 = 0;

/// Chest ID for overflow storage (node 0, chest 1)
pub const OVERFLOW_CHEST_ID: i32 = 1;

// ============================================================================
// ORDER QUEUE CONSTANTS
// ============================================================================

/// Maximum number of orders a single user can have queued at once
pub const MAX_ORDERS_PER_USER: usize = 8;

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
