# PLAN.md — Roadmap to 100/100 Code Quality

## Context

The codebase started at ~72/100 and is currently at ~94/100 after Tier 1, Tier 2, Tier 3, and the first Tier 4 item completed. Solid architecture (single-owner state, typed channels, no race conditions), correct AMM pricing, thorough rollback logic, and atomic persistence remain the strong foundation. `automated_chest_io` is now a thin dispatcher over `prepare_for_chest_io` + `withdraw_shulkers` + `deposit_shulkers`. The hot helpers (`execute_chest_transfers`, `perform_trade`) return typed `StoreError` with the `From<StoreError> for String` shim letting higher layers continue to use string errors during the progressive rollout. A test harness in `src/store/orders.rs` builds `Store` in-memory (`Store::new_for_test`) and runs a mock bot task so buy/pay handler tests exercise real handler code paths. All item-referencing fields use the `ItemId` newtype for compile-time normalization safety. An operation journal (`data/journal.json`) records in-flight shulker lifecycle states so a subsequent startup can detect and warn about crash-interrupted operations. Property-based tests via `proptest` exercise AMM pricing invariants across thousands of random inputs. A formal `TradeState` state machine (`src/store/trade_state.rs`) tracks every in-flight trade through Queued → Withdrawing → Trading → Depositing → Committed (or RolledBack), with transition functions that make invalid phase jumps unrepresentable.

### Already shipped

- **1.1 Shared rollback helper** — extracted into `src/store/rollback.rs`, replacing ~400 lines of copy-pasted rollback blocks across `orders.rs`, `player.rs`, and `operator.rs`.
- **1.2 Config wiring** — `trade_timeout_ms` and `pathfinding_timeout_ms` now drive runtime behavior instead of hardcoded durations.
- **1.3 Split mega-functions** —
  - `orders.rs`: `handle_buy_order` / `handle_sell_order` refactored around shared `execute_chest_transfers` + `perform_trade` helpers and `BuyPlan` / `SellPlan` structs. Each handler now reads as a linear validate → chest → trade → commit flow.
  - `chest_io.rs`: `automated_chest_io` is now a thin orchestrator that calls `prepare_for_chest_io` (entity readiness + position verification + hotbar clear), opens the chest, and dispatches to `withdraw_shulkers` or `deposit_shulkers`. Each helper owns its loop state (confirmed-empty / confirmed-full sets) and returns the number of items actually moved. `find_shulker_in_inventory_view` was extracted earlier.
- **1.4 StoreError** — `src/error.rs` defines the enum via `thiserror`; `From<StoreError> for String` shims existing `Result<T, String>` boundaries. `execute_chest_transfers` and `perform_trade` (the hot path for every order) now return `Result<_, StoreError>`, so timeout / bot-disconnected / chest-op / trade-rejected all surface as typed variants. Higher-level handler signatures still return `String`; migration propagates transparently via `?`.
- **2.1 Logging pruned ~60%** — redundant entry/exit traces, step-by-step banners, and per-slot debug noise removed across `src/bot/` and `src/store/`.
- **2.2 Integration tests for order handlers** — `src/store/orders.rs` grew a `#[cfg(test)] mod tests` with a mock-bot task (auto-responds to `Whisper`, `InteractWithChestAndSync`, `TradeWithPlayer`) and `Store::new_for_test` constructor that bypasses disk I/O. `utils::resolve_user_uuid` is cfg-gated to return deterministic UUIDs offline so tests never hit the Mojang API. Four integration tests cover buy validation (out-of-stock, unknown item) and pay transfers (success + insufficient balance).
- **2.3 Lightweight planner** — `Storage::simulate_withdraw_plan` / `simulate_deposit_plan` added as non-mutating variants. All `storage.clone()` planning sites (orders.rs, player.rs, operator.rs, rollback.rs) replaced. Unit tests cover non-mutation and parity with mutating versions.
- **2.4 Dead code cleanup** — removed genuinely-unused items (`place_shulker_on_station`, `dump_inventory_to_overflow`, `read_chest_amounts`, `BotInstruction::Chat`, `ChestAction::Check`, queue `clear`/`peek`, `with_minecraft_prefix`). Test-only helpers gated with `#[cfg(test)]`. Intentional API surface kept with targeted `#[allow(dead_code)]` + justification comments. `cargo check` is warning-free.
- **3.1 Operation journal** — `src/store/journal.rs` records in-flight shulker lifecycle states (`ShulkerTaken` → `ShulkerOnStation` → `ItemsTransferred` → `ShulkerPickedUp` → `ShulkerReplaced`) to `data/journal.json`. On startup, any leftover entry is surfaced as an error-level log so the operator can reconcile. `withdraw_shulkers` and `deposit_shulkers` write state transitions at each checkpoint. `Bot` holds a `SharedJournal` (Arc<Mutex>) initialized in `bot_task`. Three unit tests cover ID uniqueness, state transitions, and load/clear round-trips.
- **3.2 Type-safe item IDs** — `src/types/item_id.rs` defines `ItemId(String)` with `#[serde(transparent)]`, `Deref<Target=str>`, `Borrow<str>`, `PartialEq<str>`, and `Display`. `ItemId::new()` strips the `minecraft:` prefix and rejects empty strings. Migrated `Pair::item`, `Chest::item`, `Order::item`, `Trade::item`, and `ChestTransfer::item` from raw `String` to `ItemId`. On-disk JSON format unchanged (transparent serde). Twelve unit tests cover normalization, serialization, deref coercion, and equality.
- **3.3 Property-based AMM tests** — Added `proptest` dev-dependency. Extracted `buy_cost_pure` / `sell_payout_pure` helpers from `pricing.rs` so property tests don't need a full `Store`. Seven proptest cases assert: k never decreases (buy and sell), positive spread, per-item buy price increases with quantity, per-item sell payout decreases with quantity, and sell payout bounded by currency reserve.
- **4.1 Formal trade state machine** — `src/store/trade_state.rs` defines `TradeState` enum with six variants: `Queued`, `Withdrawing`, `Trading`, `Depositing`, `Committed`, `RolledBack`. Transition functions (`begin_withdrawal`, `begin_trading`, `begin_depositing`, `commit`, `rollback`) consume the previous state, making invalid phase jumps (e.g. Queued → Committed) unrepresentable. `Store.current_order` replaced with `Store.current_trade: Option<TradeState>`; all four handlers (buy, sell, deposit, withdraw) advance the state at each phase. Status/cancel commands and CLI stuck-order diagnostics now report the exact phase. Eleven unit tests cover happy paths, rollback from each phase, invalid-transition panics, and display formatting. 74 tests total, all passing.

This plan documents the remaining changes needed to reach 100/100, organized into tiers by impact-to-effort ratio.

---

## Tier 1, Tier 2, & Tier 3: done

All Tier 1, Tier 2, and Tier 3 items are shipped (see "Already shipped" above). Notes on ongoing/incremental work:

- **1.4 StoreError deep migration** — the hot path (chest/trade helpers) is migrated; handler-level and bot-layer signatures can continue to migrate opportunistically. This is intentionally a progressive rollout rather than a big-bang rewrite.
- **2.2 Order-handler tests** — the test harness and four seed tests are in place. Additional scenarios (`test_buy_success`, `test_sell_rollback`, `test_deposit_flexible`, etc.) can be added incrementally as behavior evolves; the mock-bot infrastructure supports them without changes.
- **3.1 Journal** — detects crash-interrupted shulker operations on startup (detection, not auto-resume). The journal is cleared after surfacing the warning so the bot can proceed.
- **3.2 ItemId** — `store.pairs` map keys remain `String` for minimal churn; field types are `ItemId`. A future pass could migrate the map keys too.

## Tier 4 (partial): 93 → 94

### 4.1 Formal state machine for trade lifecycle: done

Shipped (see "Already shipped" above). Notes:

- **TradeState tracking** — `Store.current_trade` replaces the old `current_order: Option<QueuedOrder>` with a richer `Option<TradeState>` that carries the full phase context. Status commands, cancel checks, and CLI stuck-order diagnostics all benefit from knowing the exact phase.
- **Deposit/withdraw handlers** — these have a simpler lifecycle (no pre-trade chest withdrawal for deposit; withdraw skips the Depositing phase). They still advance through Queued → Withdrawing(empty) → Trading → Committed for consistency.

---

## Tier 4 (remaining): 94 → 95+ (Diminishing returns)

### 4.2 Metrics and observability

**Problem:** Debugging requires parsing log files. No quantitative view of system health.

**Files:** New `src/metrics.rs`, modifications to `src/store/mod.rs`

**Change:**

- Add `prometheus` crate
- Expose counters: `orders_total{type,status}`, `trades_total{type}`, `rollbacks_total`
- Expose gauges: `queue_depth`, `users_total`, `pairs_total`, `storage_nodes_total`
- Expose histograms: `order_duration_seconds{type}`, `trade_duration_seconds`
- Optional HTTP endpoint (or just write to `data/metrics.json` periodically)

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

| Tier        | Score    | Key Changes                                                                                                         |
| ----------- | -------- | ------------------------------------------------------------------------------------------------------------------- |
| 1 (done)    | 72 → ~85 | ✅ Rollback helper, config wiring, orders.rs + chest_io.rs splits, StoreError on hot path                           |
| 2 (done)    | ~85 → 88 | ✅ Logging pruned ~60%, dead-code cleanup, lightweight planner, order-handler tests                                 |
| 3 (done)    | 88 → 93  | ✅ Crash journal, type-safe ItemId, property-based AMM tests                                                        |
| 4 (partial) | 93 → 94  | ✅ Trade state machine (74 tests, 0 warnings). Remaining: metrics, partial fills, UUID cache, chunk-unload handling |
| 5           | 95 → 100 | E2E tests, formal verification, hot-reload, audit chain                                                             |

**Current score:** ~94/100 (Tier 1, Tier 2, Tier 3, and Tier 4.1 complete).

**Recommended stopping point for a single-server Minecraft bot:** End of Tier 3 (~93/100). Everything past that is over-engineering unless this becomes a multi-server product.
