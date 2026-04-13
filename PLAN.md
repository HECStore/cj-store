# PLAN.md — Roadmap to 100/100 Code Quality

## Context

The codebase started at ~72/100 and is currently at ~95/100 after Tier 1–3 and most of Tier 4 completed. Solid architecture (single-owner state, typed channels, no race conditions), correct AMM pricing, thorough rollback logic, and atomic persistence remain the strong foundation. 79 tests passing, 0 warnings.

### Ongoing / incremental work from shipped tiers

- **StoreError deep migration** — the hot path (chest/trade helpers) is migrated; handler-level and bot-layer signatures can continue to migrate opportunistically (progressive rollout, not big-bang).
- **Order-handler tests** — the test harness and four seed tests are in place. Additional scenarios can be added incrementally; the mock-bot infrastructure supports them without changes.
- **Journal** — detects crash-interrupted shulker operations on startup (detection, not auto-resume). The journal is cleared after surfacing the warning so the bot can proceed.
- **ItemId** — `store.pairs` map keys remain `String` for minimal churn; field types are `ItemId`. A future pass could migrate the map keys too.

---

## Remaining: Tier 4 (partial)

### 4.2 Metrics and observability

**Problem:** Debugging requires parsing log files. No quantitative view of system health.

**Files:** New `src/metrics.rs`, modifications to `src/store/mod.rs`

**Change:**

- Add `prometheus` crate
- Expose counters: `orders_total{type,status}`, `trades_total{type}`, `rollbacks_total`
- Expose gauges: `queue_depth`, `users_total`, `pairs_total`, `storage_nodes_total`
- Expose histograms: `order_duration_seconds{type}`, `trade_duration_seconds`
- Optional HTTP endpoint (or just write to `data/metrics.json` periodically)

---

## Remaining: Tier 5 — 95 → 100 (Perfection)

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

| Tier        | Score    | Key Changes                                                                                          |
| ----------- | -------- | ---------------------------------------------------------------------------------------------------- |
| 1 (done)    | 72 → ~85 | ✅ Rollback helper, config wiring, orders.rs + chest_io.rs splits, StoreError on hot path            |
| 2 (done)    | ~85 → 88 | ✅ Logging pruned ~60%, dead-code cleanup, lightweight planner, order-handler tests                  |
| 3 (done)    | 88 → 93  | ✅ Crash journal, type-safe ItemId, property-based AMM tests                                         |
| 4 (mostly)  | 93 → 95+ | ✅ Trade state machine, UUID cache, chunk-unload handling (79 tests, 0 warnings). Remaining: metrics |
| 5           | 95 → 100 | E2E tests, formal verification, hot-reload, audit chain                                              |

**Current score:** ~95/100 (Tier 1, Tier 2, Tier 3, and Tier 4.1/4.4/4.5 complete).

**Recommended stopping point for a single-server Minecraft bot:** End of Tier 3 (~93/100). Everything past that is over-engineering unless this becomes a multi-server product.
