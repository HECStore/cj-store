# cj-store — Development

Developer-facing reference: build setup, error model, item handling,
testing, known limitations, and performance tuning. For runtime topology
see [ARCHITECTURE.md](ARCHITECTURE.md); for on-disk formats see
[DATA_SCHEMA.md](DATA_SCHEMA.md).

## Build notes

- **Rust edition 2024** (stabilized in Rust 1.85, Feb 2025). The **nightly
  toolchain** is pinned in [`rust-toolchain.toml`](rust-toolchain.toml).
  The pin is there because Azalea's transitive dependency graph sometimes
  requires nightly features — it is not a project-internal choice. If
  `.cargo/config.toml` is present in your tree, inspect its `-Z...` flags
  before attempting a stable build; remove them to drop the nightly
  requirement.
- Tested on Windows; Linux and macOS should work unchanged.
- Logs go **only** to `data/logs/store.log`. `stdout` gets a handful of
  startup lines telling the operator how to tail the log (the exact
  commands for PowerShell and bash/tail are printed); every subsequent
  `tracing` event goes to the file.

## Error handling

The codebase uses four distinct error-handling conventions, one per
architectural layer. They meet at the Store boundary.

### Store layer — `StoreError`

[src/error.rs](src/error.rs) defines a single enum that every handler,
`execute_queued_order`, plan validator, `apply_chest_sync`, and
`assert_invariants` returns. Variants: `ItemNotFound`, `UnknownPair`,
`UnknownUser`, `InsufficientFunds`, `InsufficientStock`, `BotDisconnected`,
`TradeTimeout`, `TradeRejected`, `BotError`, `ValidationError`, `ChestOp`,
`PlanInfeasible`, `QueueFull`, `InvariantViolation`, `Io`. `From<String>`
is implemented both directions so `?` still flows through the few
remaining string-returning helpers.

### Bot layer — `Result<T, String>`

Bot-internal operations use stringly-typed errors. They are converted to
the appropriate `StoreError` variant at the Store boundary (typically
`StoreError::BotError` or `ChestOp`).

### Persistence — `Result<(), Box<dyn Error>>`

File I/O wrappers return boxed errors. Save failures leave the Store
marked dirty, so the next autosave tick retries — and the shutdown
handler tries again before exiting.

### Invariant lookups

Use `Store::expect_pair` / `expect_user` ([src/store/mod.rs](src/store/mod.rs))
instead of `.unwrap()`. A missing key becomes `StoreError::UnknownPair` /
`UnknownUser` plus a `tracing::error!` — never a panic of the Store task.

### Bot journal mutex

`parking_lot::Mutex` — no poisoning, no `Result` wrapping. A panic inside
the critical section cannot permanently take the bot offline (unlike
`std::sync::Mutex` where a poisoned guard would have to be explicitly
recovered). The usual async discipline — never hold the guard across
`.await` — applies and is enforced at call sites.

## Item ID handling

- **`ItemId` newtype** ([src/types/item_id.rs](src/types/item_id.rs)) wraps
  every item-referencing field (`Pair::item`, `Chest::item`, `Order::item`,
  `Trade::item`, `ChestTransfer::item`). Construction strips `minecraft:`
  and rejects empty strings, so normalization bugs are compile errors.
- **Serde**: `#[serde(transparent)]` keeps the on-disk form a bare string
  — JSON sees `"item": "diamond"`, not `"item": { "0": "diamond" }` or any
  other tagged shape — so the newtype is fully backwards compatible with
  pre-newtype data files.
- **Bot interaction**: `ItemId::with_minecraft_prefix()` re-adds the prefix
  when matching Azalea item IDs.
- **Player input**: both `diamond` and `minecraft:diamond` are accepted.

## Testing

- `cargo test` runs the full suite (unit + integration + proptest). Coverage:
  pricing invariants, storage planner parity, queue FIFO + per-user limits,
  rate-limiter backoff, journal lifecycle, `ItemId` normalization, trade
  state-machine transitions (happy paths, rollbacks, invalid-transition
  panics), UUID cache TTL, trade-GUI slot math, and the order-handler
  integration suite including `sell`/`deposit`/`withdraw` rejection paths.
  For exact counts at HEAD, see `cargo test --no-run` output.
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
3. **Trade history grows unbounded on disk** — one file per trade under
   `data/trades/`, never pruned. `max_trades_in_memory` (default 50 000)
   caps how many are *loaded into memory* at startup; older files stay on
   disk untouched. Automatic archival / time-based cutoff is not
   implemented — if disk footprint matters, delete old files out-of-band.
4. **Retry logic**: constants live in [src/constants.rs](src/constants.rs).
   Chunk-not-loaded triggers the chunk-aware retry path, which reopens
   stale containers rather than giving up.

   | Operation                 | Trigger                      | Retries                              | Base backoff | Max backoff | Notes                                       |
   | ------------------------- | ---------------------------- | ------------------------------------ | ------------ | ----------- | ------------------------------------------- |
   | Chest open (normal)       | Transient I/O                | 3 (`CHEST_OP_MAX_RETRIES`)           | 500 ms       | 5 s         | Exponential backoff                         |
   | Chest open (chunk reload) | Chunk not loaded             | +2 (`CHUNK_RELOAD_EXTRA_RETRIES`)    | 3 s          | 10 s        | Slower backoff to let the chunk stream in   |
   | Shulker open              | GUI open timeout             | 2 (`SHULKER_OP_MAX_RETRIES`)         | 500 ms       | 5 s         | Exponential backoff                         |
   | Navigation                | Path failure                 | 2 (`NAVIGATION_MAX_RETRIES`)         | 500 ms       | 5 s         | Exponential backoff                         |
   | Validation / discovery    | Fail-fast                    | 0                                    | —            | —           | Fast fail (5 s per op)                      |
5. **Single-server design** — no coordination between instances. Two bots
   pointing at the same `data/` directory will race each other's atomic
   writes (last-write-wins on every file), produce parallel trades that
   drain the same pair's reserves, and generally corrupt the ledger.
   `fsutil::write_atomic` prevents *half-written* files but not
   concurrent writers.
6. **No partial fulfillment** — if the full quantity can't be satisfied,
   the order fails with an error to the player ("Insufficient stock" or
   "Insufficient funds"). The bot never trades a reduced amount without
   the player's explicit request. Players must re-issue with a smaller
   quantity. This is a deliberate design choice: partial fills would
   require the player to accept a ledger change they didn't request, and
   would roughly double the surface area of the rollback logic (a partial
   success/partial rollback has strictly more states than all-or-nothing).
   Rejecting the whole order is simpler to reason about and matches how
   a human store clerk would handle a stock shortage.
7. **Memory usage** — all users, pairs, and (up to `max_trades_in_memory`)
   trades load into memory on startup. Tune in config for large stores.
8. **Interrupted-trade recovery is detection-only** — `current_trade.json`
   is mirrored at every phase. On startup the Store logs and clears it;
   automatic re-queue/rollback is planned but not yet implemented (design
   sketch in [ARCHITECTURE.md § Planned: automatic crash-resume](ARCHITECTURE.md#planned-automatic-crash-resume);
   manual playbook in [RECOVERY.md § 4](RECOVERY.md#4-interrupted-datacurrent_tradejson)).
   CLI menu option 15 "Clear stuck order" releases the `processing_order`
   flag without requiring JSON edits when the only symptom is a frozen
   queue.

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
