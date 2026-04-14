# PLAN.md — Roadmap to 100/100 Code Quality

## Context

The codebase started at ~72/100 and is currently at ~99/100 after Tiers 1–3, most of Tier 4, and Tier 5 completed. Solid architecture (single-owner state, typed channels, no race conditions), correct AMM pricing with 12 proptest invariants and debug_assert guards, thorough rollback logic, atomic persistence, and hot-reloadable config remain the strong foundation. 84 tests passing, 0 warnings.

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

## Summary

| Tier       | Score     | Key Changes                                                                                          |
| ---------- | --------- | ---------------------------------------------------------------------------------------------------- |
| 1 (done)   | 72 → ~85  | ✅ Rollback helper, config wiring, orders.rs + chest_io.rs splits, StoreError on hot path            |
| 2 (done)   | ~85 → 88  | ✅ Logging pruned ~60%, dead-code cleanup, lightweight planner, order-handler tests                  |
| 3 (done)   | 88 → 93   | ✅ Crash journal, type-safe ItemId, property-based AMM tests                                         |
| 4 (mostly) | 93 → 95+  | ✅ Trade state machine, UUID cache, chunk-unload handling. Remaining: metrics (4.2)                  |
| 5 (done)   | 95+ → ~99 | ✅ Expanded proptest suite (12 invariants) + debug_assert guards, hot-reload config via `notify`     |

**Current score:** ~99/100 (Tiers 1, 2, 3, 4.1/4.4/4.5, and 5 complete). Remaining: Tier 4.2 metrics and observability.
