# PLAN.md — Roadmap to 100/100 Code Quality

## Context

The codebase started at ~72/100 and is currently at ~80/100 after the first batch of improvements. Solid architecture (single-owner state, typed channels, no race conditions), correct AMM pricing, thorough rollback logic, and atomic persistence remain the strong foundation. Remaining weaknesses: 600–800 line order/chest-io functions, clone-heavy planning, no tests for the riskiest code paths, and stringly-typed errors everywhere.

### Already shipped
- **1.1 Shared rollback helper** — extracted into `src/store/rollback.rs`, replacing ~400 lines of copy-pasted rollback blocks across `orders.rs`, `player.rs`, and `operator.rs`.
- **1.2 Config wiring** — `trade_timeout_ms` and `pathfinding_timeout_ms` now drive runtime behavior instead of hardcoded durations.
- **2.1 Logging pruned ~60%** — redundant entry/exit traces, step-by-step banners, and per-slot debug noise removed across `src/bot/` and `src/store/`.
- **2.4 Dead code cleanup** — removed genuinely-unused items (`place_shulker_on_station`, `dump_inventory_to_overflow`, `read_chest_amounts`, `BotInstruction::Chat`, `ChestAction::Check`, queue `clear`/`peek`, `with_minecraft_prefix`). Test-only helpers gated with `#[cfg(test)]`. Intentional API surface kept with targeted `#[allow(dead_code)]` + justification comments. `cargo check` is warning-free.

This plan documents the remaining changes needed to reach 100/100, organized into tiers by impact-to-effort ratio.

---

## Tier 1: 80 → 82 (remaining items)

### 1.3 Split mega-functions

**Problem:** `handle_buy_order` (~600 lines), `handle_sell_order` (~700 lines), and `automated_chest_io` (~800 lines) are too long to reason about.

**Files:** `src/store/orders.rs`, `src/bot/chest_io.rs`

**Change for orders.rs:**

```
handle_buy_order → split into:
  - validate_buy_order(store, player, item, qty) → BuyPlan
  - execute_buy_withdrawal(store, plan) → Result
  - execute_buy_trade(store, plan, player) → TradeResult
  - commit_buy(store, plan, trade_result) → Result
  - (rollback uses the shared helper in src/store/rollback.rs)

handle_sell_order → same pattern:
  - validate_sell_order → SellPlan
  - execute_sell_trade → TradeResult
  - execute_sell_deposit → Result
  - commit_sell → Result
```

**Change for chest_io.rs:**

```
automated_chest_io → split into:
  - plan_chest_slots(known_counts, direction, amount) → Vec<SlotPlan>
  - process_single_shulker(bot, chest_pos, slot, direction, ...) → SlotResult
  - (automated_chest_io becomes a thin orchestrator calling these)
```

### 1.4 Introduce a proper error enum

**Problem:** Every function returns `Result<T, String>`. No structured error handling, matching, or categorization.

**Files:** New file `src/error.rs`, then update all `Result<T, String>` call sites progressively.

**Change:** Create:

```rust
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("Item '{0}' not found")]
    ItemNotFound(String),
    #[error("Insufficient funds: need {need:.2}, have {have:.2}")]
    InsufficientFunds { need: f64, have: f64 },
    #[error("Insufficient stock: need {need}, have {have}")]
    InsufficientStock { need: i32, have: i32 },
    #[error("Bot not connected")]
    BotDisconnected,
    #[error("Trade timed out after {0}s")]
    TradeTimeout(u64),
    #[error("Trade rejected: {0}")]
    TradeRejected(String),
    #[error("Bot operation failed: {0}")]
    BotError(String),
    #[error("Validation failed: {0}")]
    ValidationError(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
```

Migrate progressively — start with store handlers, then bot, then types.

---

## Tier 2: 82 → 88 (remaining items)

### 2.2 Add integration tests for order handlers

**Problem:** The riskiest code (buy/sell/deposit/withdraw handlers) has zero tests. Only types and pricing have unit tests.

**Files:** New `src/store/tests/` module or `tests/` directory

**Change:** Create a test harness that:

- Constructs a `Store` with in-memory state (no disk)
- Provides a mock `mpsc::Sender<BotInstruction>` that auto-responds with canned `ChestSyncReport` / `TradeItem` results
- Tests:
  - `test_buy_order_success` — verifies balance deduction, stock update, trade recorded
  - `test_buy_order_insufficient_funds` — verifies rejection message
  - `test_buy_order_rollback_on_trade_failure` — verifies items restored to storage
  - `test_sell_order_success` — verifies payout, stock increase, fractional balance
  - `test_sell_order_validation_rejects_wrong_items`
  - `test_deposit_flexible` — verifies any diamond amount credited
  - `test_withdraw_rollback` — verifies balance NOT deducted on trade failure
  - `test_pay_transfer` — verifies both balances updated atomically
  - `test_rate_limiter_escalation` — verifies cooldown doubles
  - `test_queue_fifo_ordering` — verifies FIFO with multi-user scenario
  - `test_queue_user_limit` — verifies max 8 per user

### 2.3 Replace `storage.clone()` for planning

**Problem:** Every buy/sell clones the entire `Storage` struct to simulate a withdrawal/deposit plan. This is O(nodes × 54) and wasteful.

**Files:** `src/types/storage.rs`, `src/store/orders.rs`

**Change:** Create a lightweight planner that borrows storage:

```rust
pub struct WithdrawPlanner<'a> {
    nodes: &'a [Node],
    adjustments: Vec<(/*node_idx*/usize, /*chest_idx*/usize, /*slot*/usize, /*delta*/i32)>,
}

impl<'a> WithdrawPlanner<'a> {
    pub fn plan(nodes: &'a [Node], item: &str, qty: i32) -> (Self, Vec<ChestTransfer>)
    // Read-only access to nodes, records adjustments without cloning
}
```

The `deposit_plan` / `withdraw_plan` methods currently mutate `self` — refactor them to return a plan + adjustment set without mutating.

---

## Tier 3: 88 → 93 (Polish)

### 3.1 Operation journaling for crash recovery

**Problem:** If the bot crashes mid-shulker-operation (shulker out of chest, on station, or in inventory), recovery requires manual operator intervention.

**Files:** New `src/store/journal.rs`, modifications to `src/bot/chest_io.rs`

journal entry:

```rust
struct OperationJournal {
    operation_id: u64,
    operation_type: JournalOp, // WithdrawFromChest, DepositToChest, Trade
    chest_id: i32,
    slot_index: usize,
    state: JournalState, // ShulkerTaken, ShulkerOnStation, ItemsTransferred, ShulkerPickedUp, ShulkerReplaced
}
```

On startup, check for incomplete journal entries and either resume or abort cleanly. Delete journal entry on successful completion.

### 3.2 Type-safe item IDs

**Problem:** Item identifiers are raw `String` everywhere. Easy to forget normalization, compare `"minecraft:diamond"` vs `"diamond"`, or pass empty strings.

**Files:** New `src/types/item_id.rs`, then update all `item: String` fields

**Change:**

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ItemId(String);

impl ItemId {
    pub fn new(raw: &str) -> Result<Self, &'static str> {
        let normalized = raw.strip_prefix("minecraft:").unwrap_or(raw);
        if normalized.is_empty() { return Err("empty item ID"); }
        Ok(Self(normalized.to_string()))
    }
    pub fn as_str(&self) -> &str { &self.0 }
    pub fn with_minecraft_prefix(&self) -> String { format!("minecraft:{}", self.0) }
}
```

Replace `item: String` with `item: ItemId` in `Pair`, `Chest`, `Trade`, `Order`, `ChestTransfer`, `TradeItem`, etc. All normalization bugs become compile errors.

### 3.3 Property-based tests for AMM pricing

**Problem:** Pricing tests only cover specific examples. Edge cases in floating-point math could hide bugs.

**Files:** `src/store/pricing.rs` (test section)

**Change:** Add `proptest` dependency and tests:

```rust
proptest! {
    #[test]
    fn k_never_decreases(stock in 1..10000i32, currency in 0.01..100000.0f64, qty in 1..9999i32) {
        // After a buy, new_x * new_y >= old_x * old_y
    }

    #[test]
    fn buy_cost_exceeds_sell_payout(stock in 2..10000i32, currency in 1.0..100000.0f64, qty in 1..stock) {
        // For same qty: buy_cost > sell_payout (spread is always positive)
    }

    #[test]
    fn cost_increases_with_quantity(stock in 10..10000i32, currency in 1.0..100000.0f64) {
        // cost(n+1)/(n+1) > cost(n)/n (per-item price increases with size)
    }
}
```

### 3.4 Replace JSON file-per-entity with SQLite

**Problem:** `data/trades/` creates one file per trade (50,000+ files over time). `data/users/` and `data/pairs/` have orphan-cleanup passes. Atomic writes require temp+rename dance. All data loaded into memory on startup.

**Files:** New `src/persistence.rs` or `src/db.rs`, replace `write_atomic` / `load_all` / `save_all` patterns

**Change:**

- Add `rusqlite` dependency
- Create tables: `users`, `pairs`, `orders`, `trades`, `nodes`, `chests`
- Replace `Pair::load_all()` / `save_all()` with SQL queries
- Replace `Trade::save()` (one file per trade) with `INSERT INTO trades`
- Transactions replace the atomic write pattern
- Remove `fsutil.rs` entirely
- Keep JSON export as a CLI option for debugging

---

## Tier 4: 93 → 95+ (Diminishing returns)

### 4.1 Formal state machine for trade lifecycle

**Problem:** Trade states (Queued → Processing → Trading → Committed/RolledBack) are implicit in code flow, not encoded in types.

**Files:** New `src/store/trade_state.rs`

**Change:**

```rust
enum TradeState {
    Queued(QueuedOrder),
    Withdrawing { order: QueuedOrder, plan: Vec<ChestTransfer> },
    Trading { order: QueuedOrder, withdrawn: Vec<ChestTransfer> },
    Committing { order: QueuedOrder, trade_result: TradeResult },
    Committed(CompletedTrade),
    RolledBack { order: QueuedOrder, reason: String },
}
```

Each transition is a function that consumes the old state and produces the new one. Invalid transitions (e.g., Committing → Queued) are unrepresentable.

### 4.2 Metrics and observability

**Problem:** Debugging requires parsing log files. No quantitative view of system health.

**Files:** New `src/metrics.rs`, modifications to `src/store/mod.rs`

**Change:**

- Add `prometheus` crate
- Expose counters: `orders_total{type,status}`, `trades_total{type}`, `rollbacks_total`
- Expose gauges: `queue_depth`, `users_total`, `pairs_total`, `storage_nodes_total`
- Expose histograms: `order_duration_seconds{type}`, `trade_duration_seconds`
- Optional HTTP endpoint (or just write to `data/metrics.json` periodically)

### 4.3 Graceful partial fulfillment

**Problem:** If a player wants 1000 items but only 800 exist, the entire order fails. No option for "give me what you have."

**Files:** `src/store/orders.rs`, `src/store/handlers/player.rs`, `src/messages.rs`

**Change:**

- Add `allow_partial: bool` field to buy/sell queue entries
- If partial allowed and stock < requested: fulfill available amount, adjust price proportionally
- Notify player: "Partially filled: 800/1000 cobblestone for X diamonds"
- Add `buymax` / `sellmax` command aliases that enable partial fill

### 4.4 Connection pooling for Mojang API

**Problem:** Every `get_uuid_async` call hits the Mojang API. Repeated lookups for the same player waste network round-trips.

**Files:** `src/types/user.rs`, `src/store/utils.rs`

**Change:**

- Add an in-memory LRU cache (`HashMap<String, (String, Instant)>`) with 5-minute TTL
- Cache UUID lookups so repeated commands from the same player don't hit the API
- Invalidate on username change detection

### 4.5 Graceful handling of server restarts / chunk unloading

**Problem:** If the server restarts or chunks unload while the bot is mid-operation, chest operations fail with opaque errors.

**Files:** `src/bot/chest_io.rs`, `src/bot/navigation.rs`

**Change:**

- Detect "chunk not loaded" / "block entity missing" errors specifically
- Wait and retry with backoff (chunks reload after ~10s on most servers)
- Distinguish between "chest doesn't exist" (permanent) and "chunk not loaded" (transient)

---

## Tier 5: 95 → 100 (Perfection)

### 5.1 Full end-to-end test suite with mock Minecraft server

**Problem:** Integration tests from Tier 2 mock the bot channel. True E2E testing requires simulating Minecraft protocol.

**Change:** Build a lightweight mock server that speaks enough of the Minecraft protocol to test trade GUI interactions, chest operations, and whisper parsing end-to-end.

### 5.2 Formal verification of AMM invariants

**Change:** Use `kani` or `prusti` to formally prove that `k` never decreases, balances never go negative, and stock always matches physical storage after any sequence of operations.

### 5.3 Hot-reload config without restart

**Change:** Watch `data/config.json` for changes and reload fee, timeouts, limits without stopping the bot.

### 5.4 Audit log with cryptographic integrity

**Change:** Chain trade records with hash links (each trade includes the hash of the previous trade) so tampering with history is detectable.

### 5.5 Multi-server / multi-bot support

**Change:** Namespace all data by server address, support running multiple bot instances from a single binary with isolated state.

---

## Summary

| Tier        | Score        | Key Changes                                                                |
| ----------- | ------------ | -------------------------------------------------------------------------- |
| 1 (done)    | 72 → ~78     | ✅ Shared rollback helper, config wiring                                    |
| 1 (remain)  | ~78 → 82     | Split mega-functions, error enum                                           |
| 2 (done)    | included     | ✅ Cut logging ~60%, dead-code cleanup                                      |
| 2 (remain)  | 82 → 88      | Integration tests for order handlers, lightweight planner (no clone)       |
| 3           | 88 → 93      | Crash journal, type-safe IDs, property tests, SQLite                       |
| 4           | 93 → 95+     | State machine, metrics, partial fills, UUID cache                          |
| 5           | 95 → 100     | E2E tests, formal verification, hot-reload, audit chain                    |

**Current score:** ~80/100 (Tier 1.1, 1.2, 2.1, 2.4 shipped).

**Recommended stopping point for a single-server Minecraft bot:** End of Tier 2 (~88/100). Everything past that is over-engineering unless this becomes a multi-server product.
