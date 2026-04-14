# cj-store — Codebase Review & Roadmap

**Rating: 80 / 100**

Solid architecture, typed errors, journal-based crash recovery, and proptest on AMM pricing. Panic surface in store handlers has been replaced by structured `StoreError::Unknown*` variants via `Store::expect_pair` / `expect_user`; the bot journal now uses `parking_lot::Mutex` (no poisoning); CLI reads retry on transient I/O. Points still lost on an untested bot layer and missing engineering hygiene (CI / lints / metrics).

---

## What's good

- Clean module boundaries: [src/store/](src/store/) owns state, [src/bot/](src/bot/) owns Azalea I/O, [src/cli.rs](src/cli.rs) owns operator UX, [src/types/](src/types/) owns persisted models.
- Typed error enum in [src/error.rs](src/error.rs) for hot-path operations.
- Crash recovery via [src/store/journal.rs](src/store/journal.rs) + rollback helper in [src/store/rollback.rs](src/store/rollback.rs).
- Property-based tests on AMM pricing in [src/store/pricing.rs](src/store/pricing.rs); focused unit tests in [store/journal.rs](src/store/journal.rs), [store/trade_state.rs](src/store/trade_state.rs), [store/queue.rs](src/store/queue.rs), [store/rate_limit.rs](src/store/rate_limit.rs), [types/item_id.rs](src/types/item_id.rs), [types/node.rs](src/types/node.rs), [types/storage.rs](src/types/storage.rs).
- Hot-reloadable config (`notify` crate) and structured logging via `tracing` + `tracing-appender`.
- Clear trade state machine in [src/store/trade_state.rs](src/store/trade_state.rs).

## What's weak

- **Bot layer has zero tests.** [src/bot/chest_io.rs](src/bot/chest_io.rs) (1779 LOC), [src/bot/trade.rs](src/bot/trade.rs) (1045), [src/bot/inventory.rs](src/bot/inventory.rs) (998), [src/bot/navigation.rs](src/bot/navigation.rs), and handler modules ([handlers/player.rs](src/store/handlers/player.rs) 1615, [handlers/operator.rs](src/store/handlers/operator.rs) 607, [handlers/cli.rs](src/store/handlers/cli.rs) 535) lack any `#[test]`.
- **Missing infra.** No `.github/workflows/`, no `clippy.toml`, no `rustfmt.toml`, no `cargo-deny`, no coverage tool, no metrics, no benchmarks.
- **Hotspot files too large.** `chest_io.rs` 1779 LOC, `player.rs` 1615, `orders.rs` 1245, `trade.rs` 1045, `storage.rs` 1016 — harder to navigate and review.

---

## Roadmap to 100

Each tier is ~+5 points. Ship in order.

### Tier A → 85: Bot + handler test coverage

- Unit tests for slot math in [src/bot/inventory.rs](src/bot/inventory.rs), station-position calc in [src/bot/shulker.rs](src/bot/shulker.rs), path ordering in [src/bot/navigation.rs](src/bot/navigation.rs).
- Integration harness that drives `Store::run` end-to-end via `StoreMessage` with a mock `BotInstruction` sink. Cover: `buy` / `sell` / `pay` / `deposit` / `withdraw` / `queue` / `cancel`, rollback on simulated bot failure, journal recovery on restart, rate-limit backoff.
- Track coverage with `cargo-tarpaulin`; gate CI at ≥70 % on `store/`, ≥40 % on `bot/`.

### Tier B → 90: Engineering hygiene + refactor

- `.github/workflows/ci.yml`: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --all-features`, `cargo deny check`.
- Add `clippy.toml` (pedantic subset) and `rustfmt.toml`.
- Split [src/bot/chest_io.rs](src/bot/chest_io.rs) → `chest_io/mod.rs`, `withdraw.rs`, `deposit.rs`, `dispatch.rs`.
- Split [src/store/handlers/player.rs](src/store/handlers/player.rs) → one module per command (`buy.rs`, `sell.rs`, `pay.rs`, `deposit.rs`, `withdraw.rs`, `queue.rs`, `status.rs`, `info.rs`).
- Replace ad-hoc log prefixes with `tracing` spans carrying `user`, `item`, `order_id`.

### Tier C → 95: Observability + resilience

- Prometheus metrics (behind a feature flag): orders/sec, queue depth per user, AMM slippage histogram, trade success rate, chest-I/O latency, journal recovery count.
- New `/msg <bot> stats` player command and `Stats` CLI menu entry.
- Periodic tarball snapshot of `data/` with rotation; document restore runbook in [README.md](README.md).
- `cargo-fuzz` target for `messages::parse_whisper` to harden against malformed input.

### Tier D → 100: Correctness + future features

- Formalize invariants in [src/store/state.rs](src/store/state.rs) `assert_invariants`: conservation (`Σ user balances + Σ pair diamond reserves == known total`), item reserves == physical storage counts, queue ordering. Run every autosave in debug; nightly CI job in release.
- `loom` (or `shuttle`) test for the journal + rollback state machine against randomized crash points.
- README "Optional Enhancements" delivered: multi-item trades, order books / limit orders, statistics tracking.
- `criterion` benchmark suite for AMM pricing and queue processing; regression gate in CI.
