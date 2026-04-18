# cj-store — Development

Developer-facing reference: build setup, error model, item handling,
testing, known limitations, and performance tuning. For runtime topology
see [ARCHITECTURE.md](ARCHITECTURE.md); for on-disk formats see
[DATA_SCHEMA.md](DATA_SCHEMA.md).

## Build notes

- **Rust edition 2024**; **nightly toolchain** pinned via
  `rust-toolchain.toml`. `.cargo/config.toml` uses `-Z...` flags that
  require nightly — remove them to build on stable.
- Tested on Windows; Linux and macOS should work unchanged.
- Logging goes **only** to `data/logs/store.log` (stdout prints a
  "how to tail" hint on startup).

## Error handling

- **`StoreError` enum** ([src/error.rs](src/error.rs)) is the uniform return
  type for every handler, `execute_queued_order`, plan validators,
  `apply_chest_sync`, and `assert_invariants`. Variants: `ItemNotFound`,
  `UnknownPair`, `UnknownUser`, `InsufficientFunds`, `InsufficientStock`,
  `BotDisconnected`, `TradeTimeout`, `TradeRejected`, `BotError`,
  `ValidationError`, `ChestOp`, `PlanInfeasible`, `QueueFull`,
  `InvariantViolation`, `Io`. Bridged to `String` both ways so `?` still
  flows through the few remaining string-returning helpers.
- **Bot operations** return `Result<T, String>` internally, converted to
  the appropriate `StoreError` variant at the Store boundary.
- **Persistence** returns `Result<(), Box<dyn Error>>`. Save failures leave
  the Store dirty so it retries on the next tick and again on shutdown.
- **Invariant lookups** use `Store::expect_pair` / `expect_user`
  ([src/store/mod.rs](src/store/mod.rs)) instead of `.unwrap()`. A missing
  key becomes `StoreError::UnknownPair` / `UnknownUser` and a
  `tracing::error!` — never a panic of the Store task.
- **Bot journal mutex** is a `parking_lot::Mutex` — no poisoning, no
  `Result` wrapping. A panic inside the critical section cannot
  permanently take the bot offline. Callers must not hold the guard across
  `.await`.

## Item ID handling

- **`ItemId` newtype** ([src/types/item_id.rs](src/types/item_id.rs)) wraps
  every item-referencing field (`Pair::item`, `Chest::item`, `Order::item`,
  `Trade::item`, `ChestTransfer::item`). Construction strips `minecraft:`
  and rejects empty strings, so normalization bugs are compile errors.
- **Serde**: `#[serde(transparent)]` keeps the on-disk form a bare string
  — fully backwards compatible.
- **Bot interaction**: `ItemId::with_minecraft_prefix()` re-adds the prefix
  when matching Azalea item IDs.
- **Player input**: both `diamond` and `minecraft:diamond` are accepted.

## Testing

- `cargo test` runs 116 tests. Covers: pricing invariants (12 proptest
  cases), storage planner parity, queue FIFO + per-user limits, rate-limiter
  backoff, journal lifecycle, `ItemId` normalization, trade state-machine
  transitions (happy paths, rollbacks, invalid-transition panics), UUID
  cache TTL, trade-GUI slot math, and the order-handler integration suite
  including `sell` / `deposit` / `withdraw` rejection paths.
- **Property-based AMM tests** via `proptest` assert: `k` never decreases,
  buy cost > sell payout (positive spread), per-item price rises with trade
  size, sell payout bounded by reserve, reserves stay strictly positive and
  finite, buy-then-sell is strictly lossy at resulting reserves, non-positive
  quantity always returns `None`, `x*y=k` exact at `fee=0.0`, fee knob is
  monotonic.
- **`debug_assert!`** guards in [src/store/orders.rs](src/store/orders.rs)
  and [src/store/handlers/operator.rs](src/store/handlers/operator.rs)
  verify non-negativity and finiteness in dev/test builds; compiled out of
  release.
- **Integration tests** build a `Store` in-memory via `Store::new_for_test`
  and spawn a mock bot task; `utils::resolve_user_uuid` is cfg-gated to
  deterministic offline UUIDs under `#[cfg(test)]`.

## Known limitations

1. **Physical node validation is optional** — "Add node (no validation)"
   trusts the operator. Prefer options 5 or 6 when extending storage.
2. **Order audit log is session-only** — `data/orders.json` is cleared on
   each startup. For history use `data/trades/*.json`. The pending queue
   (`queue.json`) IS persistent.
3. **Trade history grows unbounded** — one file per trade. Archive via
   `Trade::archive_old_trades()` (1 year cutoff). Only the newest
   `max_trades_in_memory` (default 50 000) are loaded into memory.
4. **Retry logic**: chest opening has up to 3 retries
   (`CHEST_OP_MAX_RETRIES`), extended to 5 on chunk-not-loaded (3 s base,
   10 s max backoff); validation/discovery uses zero retries with a 5 s
   timeout. Shulker opening: 2 retries. Navigation: 2 retries. Container
   recovery reopens stale chests via the chunk-aware path. Constants in
   [src/constants.rs](src/constants.rs).
5. **Single-server design** — no coordination between instances; multiple
   bots on one data directory will corrupt state.
6. **No partial fulfillment** — if the full quantity can't be satisfied,
   the order fails. No split orders.
7. **Memory usage** — all users, pairs, and (up to `max_trades_in_memory`)
   trades load into memory on startup. Tune in config for large stores.
8. **Interrupted-trade recovery is detection-only** — `current_trade.json`
   is mirrored at every phase. On startup the Store logs and clears it;
   automatic re-queue/rollback is Phase 3 (see [RECOVERY.md](RECOVERY.md)
   section 4 for the manual playbook).

## Performance tuning

- **Slow large withdrawals / deposits** — chest I/O is serialized and
  per-shulker. A 6-stack withdrawal can easily take 30 s. Not a bug.
- **High disk I/O every ~2 s** — the autosave debounce. Tune
  `autosave_interval_secs` if it's thrashing, but remember that raising
  it widens the crash-loss window.
- **Queue stalling during bursts** — orders process one at a time. There is
  no parallelism here, intentionally.
- **Server restarts / chunk unloads mid-operation** — the bot detects the
  transient `ChunkNotLoaded` path and retries with longer backoff (up to
  ~20 s). Stale containers are reopened automatically via the chunk-aware
  retry. No action needed unless the retries exhaust.
