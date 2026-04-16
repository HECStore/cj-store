# cj-store — Quality Assessment & Roadmap

**Current score: 72 / 100**

Rating is a weighted read of architecture, error handling, concurrency, tests,
docs, style, observability, and operations. Scope: everything under `src/`.

## Dimension scores

| Dimension           | Score | Headline                                           |
|---------------------|------:|----------------------------------------------------|
| Architecture        | 75    | Clean 3-task design; handler layer too fat         |
| Error handling      | 68    | `StoreError` exists but handlers still use `String`|
| Concurrency         | 82    | Good lock hygiene; UUID cache uses `std::Mutex`    |
| Testing             | 55    | Strong proptest on AMM; zero handler/bot tests     |
| Documentation       | 70    | Great README; handler/schema docs missing          |
| Code style          | 78    | Idiomatic; `#[allow(dead_code)]` overuse           |
| Observability       | 72    | `tracing` wired; no structured fields, no metrics  |
| Config & ops        | 70    | Hot-reload works; recovery runbook missing         |

## Key strengths (keep)

- Three-task design (Store / Bot / CLI) with mpsc channels — single source of truth in Store.
- AMM pricing validated by 9 proptest invariants ([src/store/pricing.rs](src/store/pricing.rs)).
- Rollback tolerates partial failure instead of aborting ([src/store/rollback.rs](src/store/rollback.rs)).
- Exponential-backoff rate limiter with idle reset ([src/store/rate_limit.rs](src/store/rate_limit.rs)).
- Journal + atomic writes for shulker ops ([src/store/journal.rs](src/store/journal.rs)).
- Clean physical-storage model (`Node` / `Chest` / `Storage` in [src/types/](src/types/)).

## Key weaknesses (fix)

- **`TradeState` is decorative.** Defined in [src/store/trade_state.rs](src/store/trade_state.rs), mutated, then dropped — not used for recovery decisions.
- **Mixed error types.** Handlers return `Result<(), String>`; `StoreError` only used in core. No `BotError` / `TransactionError` variants.
- **No integration tests.** No `tests/` dir. Handlers, bot IO, orders pipeline — untested end-to-end.
- **Unbounded caches.** UUID cache + rate-limiter state have cleanup methods marked dead; no periodic task invokes them.
- **No backpressure.** 128-buffer mpsc blocks senders on saturation; no shedding or timeout.
- **No metrics.** Zero counters/histograms; post-hoc analysis requires scraping logs.
- **Orders cleared silently on startup.** Operators get no warning of discarded queue.

## Roadmap to 100

Each phase is independently shippable.

### Phase 2 — Structural refactors (80 → 88)

1. **`Persistent` trait** (`load_all` / `save` / `path`) — implement for `Users`, `Pairs`, `Orders`, `TradeLog`. Consolidate `fsutil::write_atomic` usage.
2. **Error unification.** Add `StoreError::BotError`, `StoreError::Transaction { attempted, succeeded, items }`. Convert handler signatures to `Result<_, StoreError>`.
3. **`Command` enum + parser.** Replace hand-rolled `parts.get(0)` chains with a typed enum and single `parse_command(&str) -> Result<Command, _>`.
4. **Handler pattern.** Formalize validate → plan → execute → commit; return a `HandlerOutcome { messages, state_delta }` instead of calling `send_message_to_player` directly.
5. **Actually use `TradeState`.** Drive recovery branches off it; persist last phase for crash-resume.
6. **Channel backpressure.** Reject new orders with a typed error when queue > 90% full.
7. **Periodic cleanup task** for UUID cache + rate-limiter stale entries (hourly).

### Phase 3 — Verification & operability (88 → 95)

1. **Integration test suite** (target ~10 scenarios): happy-path buy/sell, insufficient stock, rollback on trade failure, reconnect mid-order, config hot-reload, journal replay.
2. **`MockBot`** returning canned `ChestSyncReport` / `TradeResult` so handler + orders logic is end-to-end testable.
3. **Metrics.** Add `metrics` crate with counters (`trades_completed`, `rollbacks`, `errors_by_kind`) and a latency histogram on order processing.
4. **Proptest expansion** beyond pricing: `TradeState` transition legality, rate-limiter monotonicity, storage-plan invariants.
5. **`ARCHITECTURE.md`** (diagrams of channel flow + trade state machine) and **`data/SCHEMA.md`** (JSON formats + versioning).
6. **`RECOVERY.md`** runbook: corrupted `pairs.json`, stuck journal entry, orphaned shulker in inventory.
7. **`--validate-only` / `--dry-run` flag** for config sanity-check without connecting.

### Phase 4 — Polish to 100

1. **Eliminate `#[allow(dead_code)]`** — use or delete.
2. **Reduce `.unwrap()`/`.expect()` to an audited short list**, each justified by a comment.
3. **Benchmarks** (`criterion`) on pricing, storage planning, journal IO; regression thresholds.
4. **Stress test** with simulated concurrent orders + forced disconnects.
5. **Config validation**: coordinate bounds, server-address format, y ∈ [−64, 320].
6. **Rustdoc coverage gate** in CI (`#![warn(missing_docs)]` on public modules).
7. **Last-known-good snapshotting** of persistent files on clean shutdown.

## Concrete file hotspots

- [src/bot/chest_io.rs](src/bot/chest_io.rs) — extract helpers; large monolith.
- [src/store/trade_state.rs](src/store/trade_state.rs) — wire into recovery.
- [src/store/journal.rs](src/store/journal.rs) — replay path + tests.
- [src/error.rs](src/error.rs) — expand variants, adopt everywhere.
- [src/constants.rs](src/constants.rs) — absorb escaped magic numbers.

## How to verify progress

- `cargo test` — unit + proptest + (new) integration suite green.
- `cargo clippy -- -D warnings` clean.
- `cargo doc --no-deps` builds without missing-docs warnings after Phase 4.
- Manual: run the bot against a test server, exercise a buy, a sell, a forced disconnect mid-trade, and a config hot-reload; confirm logs show structured fields and metrics counters increment.
