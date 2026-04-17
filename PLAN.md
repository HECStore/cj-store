# cj-store — Quality Assessment & Roadmap

**Current score: 82 / 100**

Rating is a weighted read of architecture, error handling, concurrency, tests,
docs, style, observability, and operations. Scope: everything under `src/`.

## Dimension scores

| Dimension      | Score | Headline                                         |
| -------------- | ----: | ------------------------------------------------ |
| Architecture   |    82 | 3-task design; typed `Command` dispatch          |
| Error handling |    82 | `StoreError` is the handler-chain return type    |
| Concurrency    |    85 | Periodic cleanup + global queue cap              |
| Testing        |    70 | AMM proptest + handler/queue/trade_state unit    |
| Documentation  |    72 | Great README; handler/schema docs still partial  |
| Code style     |    82 | Idiomatic; most `#[allow(dead_code)]` wired up   |
| Observability  |    78 | Structured `tracing` fields; no metrics yet      |
| Config & ops   |    75 | Hot-reload + crash-resume detection; runbook TBD |

## Key strengths (keep)

- Three-task design (Store / Bot / CLI) with mpsc channels — single source of truth in Store.
- AMM pricing validated by 9 proptest invariants ([src/store/pricing.rs](src/store/pricing.rs)).
- Rollback tolerates partial failure instead of aborting ([src/store/rollback.rs](src/store/rollback.rs)).
- Exponential-backoff rate limiter with idle reset ([src/store/rate_limit.rs](src/store/rate_limit.rs)).
- Journal + atomic writes for shulker ops ([src/store/journal.rs](src/store/journal.rs)).
- Clean physical-storage model (`Node` / `Chest` / `Storage` in [src/types/](src/types/)).
- Typed command dispatch: `parse_command` → `Command` enum → handler ([src/store/command.rs](src/store/command.rs)).
- `StoreError` everywhere on the handler chain; `send_message_to_player` returns typed errors.
- Periodic cleanup of UUID cache + rate-limiter stale entries (hourly).
- Global queue cap (`MAX_QUEUE_SIZE = 128`) with a typed `StoreError::QueueFull`.
- `TradeState` is persisted to `data/current_trade.json` at each phase transition for crash detection.

## Key weaknesses (fix)

- **No integration tests.** No `tests/` dir. End-to-end flows through handlers + bot + orders pipeline are not exercised together.
- **Auto-recovery for interrupted trades.** Phase 2 detects and warns the operator; actual re-queue/rollback on startup is deferred to Phase 3 because it needs live-server testing.
- **No metrics.** Zero counters/histograms; post-hoc analysis requires scraping logs.
- **Orders cleared silently on startup.** Operators get no warning of discarded queue.

## Roadmap to 100

Each phase is independently shippable.

### Phase 2 — Structural refactors — **DONE (72 → 82)**

Implemented in this phase:

1. **Periodic cleanup task** — hourly sweep of UUID cache + rate-limiter stale entries, wired into `Store::run()`.
2. **Channel backpressure** — global `MAX_QUEUE_SIZE = 128` cap on `OrderQueue::add`; typed `StoreError::QueueFull`.
3. **`Command` enum + parser** — `parse_command()` in [src/store/command.rs](src/store/command.rs) replaces hand-rolled `parts.get(0)` chains; handlers now take typed args.
4. **Error unification** — `Result<(), StoreError>` is the return type across `send_message_to_player`, all handler dispatchers, `execute_queued_order`, plan validators, `apply_chest_sync`, `assert_invariants`, and `handle_bot_message`. Added `StoreError::InvariantViolation`, `StoreError::QueueFull`, and `From<String> for StoreError` as a bridge.
5. **`TradeState` crash-resume persistence** — `TradeState`, `TradeResult`, `CompletedTrade`, `ChestTransfer`, `TradeItem` now derive `Serialize/Deserialize`; mirror to `data/current_trade.json` at each phase transition; detect + warn on startup when a prior session crashed mid-trade.

Skipped with rationale:

- **`Persistent` trait** — types have genuinely different persistence shapes (HashMap vs VecDeque vs Vec, single-file vs multi-file). A trait would need complex associated types for minimal benefit; `fsutil::write_atomic` is already the shared primitive.
- **`HandlerOutcome` pattern** — current inline `send_message_to_player` calls are pragmatic and already follow validate → plan → execute → commit (documented in [src/store/orders.rs](src/store/orders.rs)). Buffering messages into a struct would add complexity without a clear payoff.

### Phase 3 — Verification & operability (82 → 95)

Already landed:

- **`ARCHITECTURE.md`** (channel flow + trade state machine diagrams) —
  [ARCHITECTURE.md](ARCHITECTURE.md).
- **`DATA_SCHEMA.md`** (JSON on-disk formats + versioning policy) —
  [DATA_SCHEMA.md](DATA_SCHEMA.md).
- **`RECOVERY.md`** runbook for corrupted pairs, stuck journal, orphaned
  shulker, interrupted trade — [RECOVERY.md](RECOVERY.md).
- **`--validate-only` / `--dry-run` flag** for config sanity-check without
  connecting ([src/main.rs](src/main.rs) `run_validate_only`).

Remaining:

1. **Integration test suite** (target ~10 scenarios): happy-path buy/sell, insufficient stock, rollback on trade failure, reconnect mid-order, config hot-reload, journal replay, interrupted-trade recovery.
2. **`MockBot`** returning canned `ChestSyncReport` / `TradeResult` so handler + orders logic is end-to-end testable.
3. **Auto-recovery from persisted `TradeState`** — inspect phase on startup, re-queue or roll back as appropriate. Requires Phase 3 test coverage first.
4. **Proptest expansion** beyond pricing: `TradeState` transition legality, rate-limiter monotonicity, storage-plan invariants.

### Phase 4 — Polish to 100

Already landed:

- **Eliminate unnecessary `#[allow(dead_code)]`** — swept; remaining
  instances are either `#[cfg(test)]` or test-only API surfaces with an
  honest comment.
- **Config validation** strengthened: coordinate bounds, `y ∈ [-64, 320]`,
  server-address format (no scheme, no whitespace, ASCII only, optional
  port must parse as `u16`) — see `Config::validate` in
  [src/config.rs](src/config.rs).
- **Rustdoc coverage gate**: `#![deny(rustdoc::broken_intra_doc_links)]`
  and `#![deny(rustdoc::invalid_html_tags)]` wired in
  [src/main.rs](src/main.rs); `cargo doc --no-deps` is clean.

Remaining:

1. **Reduce `.unwrap()`/`.expect()` to an audited short list**, each justified by a comment.
2. **Benchmarks** (`criterion`) on pricing, storage planning, journal IO; regression thresholds.
3. **Stress test** with simulated concurrent orders + forced disconnects.
4. **Last-known-good snapshotting** of persistent files on clean shutdown.

## Concrete file hotspots

- [src/bot/chest_io.rs](src/bot/chest_io.rs) — extract helpers; large monolith.
- [src/store/trade_state.rs](src/store/trade_state.rs) — persistence done; wire into auto-recovery in Phase 3.
- [src/store/journal.rs](src/store/journal.rs) — replay path + tests.
- [src/error.rs](src/error.rs) — variant set now covers all handler paths; keep in sync as new failure modes appear.
- [src/constants.rs](src/constants.rs) — top-level magic numbers absorbed
  (disconnect/debounce/flush/retry/check-interval); `src/bot/chest_io.rs`
  still has inline `Duration::from_millis(..)` call sites that were left
  untouched pending the chest_io extraction in Phase 3.

## How to verify progress

- `cargo test` — unit + proptest + (new) integration suite green.
- `cargo clippy -- -D warnings` clean.
- `cargo doc --no-deps` builds without missing-docs warnings after Phase 4.
- Manual: run the bot against a test server, exercise a buy, a sell, a forced disconnect mid-trade, and a config hot-reload; confirm logs show structured fields and metrics counters increment.
