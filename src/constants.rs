//! # Constants Module
//!
//! Centralized constants for timeouts, delays, and other configurable values.
//! This makes it easier to tune the bot's behavior and document what each value means.

pub const DOUBLE_CHEST_SLOTS: usize = 54;

pub const SHULKER_BOX_SLOTS: usize = 27;

/// Hotbar slot 0 in inventory slot numbering (36-44 are hotbar slots).
/// Minecraft's container protocol numbers slots contiguously: 0-8 are crafting/armor,
/// 9-35 are the main inventory, and 36-44 are the hotbar. Add the hotbar index (0-8)
/// to this constant to address a specific hotbar slot.
pub const HOTBAR_SLOT_0: usize = 36;

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

/// Outer timeout (seconds) on the oneshot ack returned by `BotInstruction::Whisper`.
///
/// Whisper handling is serialized with chest operations on the bot's `bot_rx`
/// loop, so the ack can legitimately be delayed behind a long-running chest op.
/// This must therefore be **comfortably larger** than `CHEST_OP_TIMEOUT_SECS`
/// (currently 90s) so the whisper ack only ever fires when the bot is truly
/// wedged — not merely busy. Without this outer timeout, a dropped/forgotten
/// `respond_to.send` on the bot side would hang the entire store loop
/// indefinitely (the non-order `dispatch_message` path has no
/// `ORDER_HARD_TIMEOUT_SECS` watchdog around it).
pub const WHISPER_ACK_TIMEOUT_SECS: u64 = 120;

pub const PATHFINDING_TIMEOUT_MS: u64 = 60_000;

// Delays are intentionally generous to handle server lag. Do not reduce
// without thorough testing.

pub const DELAY_SHORT_MS: u64 = 100;

pub const DELAY_MEDIUM_MS: u64 = 200;

/// Delay after a click / interact that updates container state.
/// Slightly longer than `DELAY_MEDIUM_MS` to give the server time to
/// echo the new slot contents before the next read.
pub const DELAY_INTERACT_MS: u64 = 300;

pub const DELAY_BLOCK_OP_MS: u64 = 350;

pub const DELAY_LOOK_AT_MS: u64 = 250;

/// Long settle delay used after multi-step shulker / pickup sequences,
/// where item-drop physics or chunk updates need extra time to converge
/// before the next read or click.
pub const DELAY_SETTLE_MS: u64 = 500;

pub const DELAY_VALIDATION_BETWEEN_CHESTS_MS: u64 = 750;

pub const DELAY_SHULKER_PLACE_MS: u64 = 750;

/// Delay after block-interact / trade-menu open events where the container
/// content packet is in flight. Sits between `DELAY_BLOCK_OP_MS` (350) and
/// `DELAY_SETTLE_MS` (500) — empirically the shortest wait that reliably
/// produces a sync'd shulker-open or trade-menu inventory read. Shared by
/// `bot/shulker::open_shulker_at_station_once` (after `block_interact`) and
/// `bot/trade::place_items_from_inventory_into_trade` (after trade GUI open).
pub const DELAY_CONTAINER_SYNC_MS: u64 = 450;

pub const DELAY_DISCONNECT_MS: u64 = 2_000;

/// Debounce window for config file-change events (milliseconds).
/// Editors typically emit a burst of writes on save (rename-over-old, metadata
/// touch, final write); we want exactly one reload per user edit, so we wait
/// this long after the first event before reloading and drain anything that
/// arrived in the meantime.
pub const DELAY_CONFIG_DEBOUNCE_MS: u64 = 500;

/// Total attempt count for chest operations (the retry loop runs `0..N`, so
/// `N` is the total number of attempts, not the number of *re*tries).
pub const CHEST_OP_MAX_ATTEMPTS: u32 = 3;

/// Extra attempts added when a chunk-not-loaded condition is detected.
/// Chunks typically reload within ~10s on most servers, so we allow more
/// attempts with a longer base delay before giving up.
pub const CHUNK_RELOAD_EXTRA_RETRIES: u32 = 2;

/// Base delay (ms) when waiting for chunks to reload. Longer than the normal
/// retry base because chunk loading is a server-side operation that can take
/// several seconds, especially on busy or low-TPS servers.
pub const CHUNK_RELOAD_BASE_DELAY_MS: u64 = 3_000;

pub const CHUNK_RELOAD_MAX_DELAY_MS: u64 = 10_000;

/// Total attempt count for shulker open operations (the retry loop runs
/// `0..N`, so `N` is the total number of attempts).
pub const SHULKER_OP_MAX_ATTEMPTS: u32 = 2;

/// Total attempt count for placing a shulker on the station. `block_interact`
/// is fire-and-forget and the server can silently drop a rejected placement, so
/// `place_shulker_on_station` verifies the block actually landed and re-issues
/// the place up to this many times before aborting (the loop runs `1..=N`).
pub const SHULKER_PLACE_MAX_ATTEMPTS: u32 = 3;

/// Total attempt count for pathfinding navigation (the retry loop runs
/// `0..N`, so `N` is the total number of attempts).
pub const NAVIGATION_MAX_ATTEMPTS: u32 = 2;

pub const RETRY_BASE_DELAY_MS: u64 = 500;

pub const RETRY_MAX_DELAY_MS: u64 = 5_000;

/// Fast verification-poll cadence (ms). Used by the inner
/// `recover_shulker_to_slot_0` verify loop: the chest GUI is already open
/// and the cursor is on the bot's side, so server echoes of slot changes
/// arrive promptly. Empirically the shortest cadence that doesn't race a
/// `container_set_content` packet on a healthy connection.
pub const VERIFY_POLL_FAST_MS: u64 = 200;

/// Default verification-poll cadence (ms). Used by
/// `place_shulker_in_chest_slot_verified` after a chest-slot click. The
/// extra 50ms over `VERIFY_POLL_FAST_MS` absorbs the round-trip needed
/// for a chest-side slot update (which can lag the player-side update).
pub const VERIFY_POLL_DEFAULT_MS: u64 = 250;

/// Slow verification-poll cadence (ms). Used between outer recovery
/// attempts in `recover_shulker_to_slot_0` to let the server fully sync
/// inventory state before re-reading it; empirically 350ms is the
/// shortest delay that consistently produces a fresh snapshot after a
/// failed pickup/place sequence.
pub const VERIFY_POLL_SLOW_MS: u64 = 350;

/// Exponential backoff delay: `base_ms * 2^attempt`, capped at `max_ms`.
pub fn exponential_backoff_delay(attempt: u32, base_ms: u64, max_ms: u64) -> u64 {
    // Clamp the shift to 10 so `attempt >= 64` does not shift past u64 range;
    // `max_ms` dominates well before this limit matters in practice.
    let delay = base_ms.saturating_mul(1u64 << attempt.min(10));
    delay.min(max_ms)
}

/// Equal-jitter exponential backoff: returns a delay in
/// `[exponential_backoff_delay(attempt, base, max) / 2,
///   exponential_backoff_delay(attempt, base, max)]`.
///
/// Equal-jitter scheme (Marc Brooker / AWS Architecture Blog): take half
/// of the exponential delay as the deterministic floor, then add a random
/// amount in `[0, delay/2]`. This preserves the worst-case bound while
/// breaking the lockstep that causes thundering-herd retries when many
/// callers backoff together.
///
/// The non-jittered [`exponential_backoff_delay`] is intentionally left
/// untouched: tests assert exact values, and the non-retry call sites
/// rely on deterministic delays.
pub fn exponential_backoff_delay_jittered(attempt: u32, base_ms: u64, max_ms: u64) -> u64 {
    let delay = exponential_backoff_delay(attempt, base_ms, max_ms);
    let half = delay / 2;
    // Half is the deterministic floor; the random component lives in [0, half].
    // Use a single byte from getrandom — that's enough entropy for jitter and
    // matches the pattern used in `types/user.rs` for Mojang retries.
    let mut buf = [0u8; 1];
    let rand_extra: u64 = match getrandom::fill(&mut buf) {
        Ok(()) => (buf[0] as u64) * half / 255,
        // RNG failure is exceedingly rare; fall back to the midpoint between
        // half and full delay rather than collapsing to a fixed value.
        Err(_) => half / 2,
    };
    half + rand_extra
}

pub const FEE_MIN: f64 = 0.0;

pub const FEE_MAX: f64 = 1.0;

pub const MAX_TRANSACTION_QUANTITY: i32 = 1_000_000;

/// Maximum diamonds movable in a single trade.
///
/// Minecraft's vanilla trade UI exposes 12 offer slots (4x3 grid); each
/// slot holds at most one 64-stack of diamonds, so a single trade can move
/// at most 768 diamonds. We reject larger requests at the handler rather
/// than silently truncating so the player isn't surprised by a partial
/// transaction.
pub const MAX_TRADE_DIAMONDS: i32 = 12 * 64;

/// Minimum reserve before price calculation becomes unreliable.
/// Pricing formulas typically divide by reserve; values this small cause
/// numerical blow-up and unrealistic prices, so the bot should refuse to
/// quote trades when a reserve falls below this threshold.
pub const MIN_RESERVE_FOR_PRICE: f64 = 0.001;

pub const CHESTS_PER_NODE: usize = 4;

pub const NODE_SPACING: i32 = 3;

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

pub const DIAMOND_CHEST_ID: i32 = 0;

pub const OVERFLOW_CHEST_ID: i32 = 1;

pub const MAX_ORDERS_PER_USER: usize = 8;

/// Global cap on the number of orders across all users.
/// Provides backpressure against overload independent of the per-user cap,
/// so a coordinated burst of many users can't exhaust bot memory or stall
/// processing latency into hours.
pub const MAX_QUEUE_SIZE: usize = 128;

pub const QUEUE_FILE: &str = "data/queue.json";

pub const RATE_LIMIT_BASE_COOLDOWN_MS: u64 = 2_000;

/// Time-to-live for cached Mojang UUID lookups (seconds).
/// 5 minutes balances freshness (username changes are rare) against API load.
pub const UUID_CACHE_TTL_SECS: u64 = 300;

pub const RATE_LIMIT_MAX_COOLDOWN_MS: u64 = 60_000;

/// After this duration of no messages, `consecutive_violations` resets to 0.
pub const RATE_LIMIT_RESET_AFTER_MS: u64 = 90_000;

// Invariant: the idle-reset window must be at least as long as the maximum
// cooldown. Otherwise a spammer pinned at the cap could fall idle for less
// than their cooldown, get violations wiped to 0, and resume at the base
// cooldown — defeating the escalating-backoff design (see the rationale in
// `RateLimiter::check`). Enforced at compile time to prevent future drift.
const _: () = assert!(RATE_LIMIT_RESET_AFTER_MS >= RATE_LIMIT_MAX_COOLDOWN_MS);

/// Interval between periodic maintenance sweeps (seconds).
/// Tied to the staleness budget of the two caches it sweeps
/// (`RATE_LIMIT_STALE_AFTER_SECS` and `UUID_CACHE_TTL_SECS`, both 300):
/// running the sweep at least as often as the staleness budget keeps the
/// rate-limit map bounded by `attacker_rate * STALE_AFTER` (instead of by
/// `attacker_rate * INTERVAL` when INTERVAL > STALE_AFTER). Both retain
/// passes are O(n) on inner HashMaps and dominate nothing on an idle store.
pub const CLEANUP_INTERVAL_SECS: u64 = 300;

/// Rate-limiter entries older than this are dropped by the periodic sweep (seconds).
/// Five minutes is well past any legitimate cooldown window, so the entry
/// cannot still be throttling a user when it is removed.
pub const RATE_LIMIT_STALE_AFTER_SECS: u64 = 300;

// Invariant: the cleanup cadence cannot exceed the staleness budget, or
// stale entries pile up between sweeps and the bound on the rate-limit map
// degrades from `attacker_rate * STALE_AFTER` to `attacker_rate * INTERVAL`.
const _: () = assert!(CLEANUP_INTERVAL_SECS <= RATE_LIMIT_STALE_AFTER_SECS);
// Invariant: cleanup must not evict users still inside the violation-reset
// window — otherwise an actively-throttled spammer gets a free escalation
// reset when their entry is freed before `RATE_LIMIT_RESET_AFTER_MS` has
// elapsed. The runtime `clamp_stale_threshold` (rate_limit.rs) handles
// misconfiguration with a one-shot warn; this catches it at build time.
const _: () =
    assert!(RATE_LIMIT_STALE_AFTER_SECS.saturating_mul(1_000) >= RATE_LIMIT_RESET_AFTER_MS,);

/// Outer watchdog on `Store::process_next_order` (seconds).
/// Individual bot operations have their own timeouts (`TRADE_TIMEOUT_MS`,
/// `CHEST_OP_TIMEOUT_SECS`, `PATHFINDING_TIMEOUT_MS`), but a bug, deadlock,
/// or lost channel response could still wedge the outer future. A single
/// multi-step order realistically completes well under 5 minutes; 15 minutes
/// is generous enough that legitimate orders never trip this, while wedged
/// orders eventually return control to the main loop so the operator's
/// `ClearStuckOrder` CLI command can be received.
pub const ORDER_HARD_TIMEOUT_SECS: u64 = 15 * 60;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponential_backoff_grows_then_saturates_at_max() {
        assert_eq!(exponential_backoff_delay(0, 500, 5_000), 500);
        assert_eq!(exponential_backoff_delay(1, 500, 5_000), 1_000);
        assert_eq!(exponential_backoff_delay(3, 500, 5_000), 4_000);
        // Doubling past max_ms must clamp, not overflow.
        assert_eq!(exponential_backoff_delay(4, 500, 5_000), 5_000);
        assert_eq!(exponential_backoff_delay(20, 500, 5_000), 5_000);
    }

    #[test]
    fn exponential_backoff_clamps_shift_to_avoid_overflow() {
        // attempt >> 64 would shift past u64 range without the internal clamp.
        assert_eq!(exponential_backoff_delay(u32::MAX, 1, u64::MAX), 1u64 << 10);
    }

    #[test]
    fn exponential_backoff_jittered_stays_within_equal_jitter_bounds() {
        // Equal-jitter: delay in [base*2^attempt / 2, base*2^attempt],
        // capped at max_ms. Sample many times to exercise the random path
        // without flaking on a single unlucky draw.
        for attempt in 0..6u32 {
            let nominal = exponential_backoff_delay(attempt, 500, 5_000);
            let lower = nominal / 2;
            let upper = nominal;
            for _ in 0..64 {
                let got = exponential_backoff_delay_jittered(attempt, 500, 5_000);
                assert!(
                    got >= lower && got <= upper,
                    "jittered delay {got}ms outside [{lower}, {upper}] for attempt {attempt}"
                );
            }
        }
    }

    #[test]
    fn exponential_backoff_jittered_respects_max_cap() {
        // When the nominal delay saturates at max_ms, the jittered delay
        // must stay in [max_ms/2, max_ms] — never exceed the cap.
        for _ in 0..64 {
            let got = exponential_backoff_delay_jittered(20, 500, 5_000);
            assert!(got >= 2_500 && got <= 5_000, "got {got}ms");
        }
    }

    #[test]
    fn exponential_backoff_jittered_lower_bound_at_attempt_zero() {
        // attempt=0, base=500: nominal=500, jittered in [250, 500].
        for _ in 0..64 {
            let got = exponential_backoff_delay_jittered(0, 500, 5_000);
            assert!(got >= 250 && got <= 500, "got {got}ms");
        }
    }
}
