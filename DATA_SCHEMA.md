# cj-store — Data Schema

Reference for every JSON file the bot reads or writes under `data/`. The
README's [Persistence Layout](README.md#persistence-layout-authoritative-spec)
section walks through the fields in prose; this document is the quick
lookup — *where* each file lives, *what shape* it has, and *what owns it*
at runtime.

All files are hand-writable JSON. Writes go through `fsutil::write_atomic`
(write to `<file>.tmp`, then rename) so a crash mid-save never corrupts the
existing file.

## File map

| Path                             | Owner (in `Store`)    | Written by                              | Required? | Versioned? |
| -------------------------------- | --------------------- | --------------------------------------- | --------- | ---------- |
| `data/config.json`               | `Store.config`        | human edit; hot-reloaded                | ✅        | No         |
| `data/pairs/<item>.json`         | `Store.pairs`         | autosave, trade commits                 | ✅ (≥1)   | No         |
| `data/users/<uuid>.json`         | `Store.users`         | autosave, deposit/withdraw, pay         | created   | No         |
| `data/storage/<node_id>.json`    | `Store.storage`       | `apply_chest_sync`, autosave            | ✅ (≥1)   | No         |
| `data/orders.json`               | `Store.orders`        | autosave (cleared on each startup)      | optional  | No         |
| `data/queue.json`                | `Store.order_queue`   | every `add`/`pop`; survives restart     | optional  | No         |
| `data/journal.json`              | `Journal` (chest I/O) | every shulker-op phase change           | optional  | No         |
| `data/current_trade.json`        | `Store.current_trade` | every `TradeState` transition           | optional  | No         |
| `data/trades/<timestamp>.json`   | `Store.trades`        | one file per committed trade            | optional  | No         |
| `data/logs/store.log`            | `tracing` appender    | every log line                          | optional  | —          |

Notes:

- "No" under *Versioned?* means there is no `schema_version` field. Adding
  new fields is backwards compatible (serde defaults / `#[serde(default)]`);
  renaming fields or changing enum variant names is a breaking change.
- `data/logs/` is not parsed by the bot — it exists purely for operators.

## `data/config.json`

Human-edited. Loaded at startup; hot-reloaded on file change (only a subset
of fields takes effect live — see README *Hot-Reload* table). The
`--validate-only` / `--dry-run` CLI flag validates this file without
connecting to the server.

```json
{
  "position": { "x": 0, "y": -53, "z": 0 },
  "fee": 0.125,
  "account_email": "you@example.com",
  "server_address": "corejourney.org",
  "buffer_chest_position": null,
  "trade_timeout_ms": 45000,
  "pathfinding_timeout_ms": 60000,
  "max_orders": 10000,
  "max_trades_in_memory": 50000,
  "autosave_interval_secs": 2
}
```

Constraints (see `Config::validate` in [src/config.rs](../src/config.rs)):

- `fee ∈ [0.0, 1.0]`
- `position.y ∈ [-64, 320]`
- `server_address` — non-empty; no `://`, no `/`, no whitespace; only
  ASCII alphanum / `.` / `-` / `:`; optional `:port` must parse as `u16`
- all timeouts / limits positive

## `data/pairs/<item>.json`

One file per trading pair. Filename is the canonical item id (no
`minecraft:` prefix). See [src/types/pair.rs](../src/types/pair.rs).

```json
{
  "item": "cobblestone",
  "stack_size": 64,
  "item_stock": 0,
  "currency_stock": 21250000.0
}
```

- `stack_size` ∈ {1, 16, 64}.
- `item_stock` must match the sum of all in-world inventory for this item
  across every chest whose `item == "<item>"`. Drift is flagged by the
  `audit-state` CLI command.
- `currency_stock` is the diamond reserve. The AMM uses `k = item_stock *
  currency_stock` so changing either directly (without the other) re-prices
  the pair — don't hand-edit unless you know what you're doing.

## `data/users/<uuid>.json`

One file per known player. Filename is the hyphenated Mojang UUID. See
[src/types/user.rs](../src/types/user.rs).

```json
{
  "uuid": "00000000-0000-0000-0000-0000000Alice",
  "username": "Alice",
  "balance": 0.0,
  "operator": false
}
```

- `username` is the *last-seen* name — it can change if the player renames
  on Mojang, and the bot updates it on next sighting.
- `balance` is measured in diamonds. Negative balances are not permitted;
  withdraw/pay handlers reject when the result would go below zero.
- `operator: true` unlocks `additem` / `removeitem` / `addcurrency` /
  `removecurrency` in whispers.

## `data/storage/<node_id>.json`

One file per physical node (cluster of 4 chests). Filename is the numeric
`node_id` starting at 0. See [src/types/node.rs](../src/types/node.rs) and
[src/types/chest.rs](../src/types/chest.rs).

```json
{
  "id": 0,
  "position": { "x": 0, "y": 64, "z": 0 },
  "chests": [
    {
      "id": 0,
      "node_id": 0,
      "index": 0,
      "position": { "x": -2, "y": 65, "z": -1 },
      "item": "diamond",
      "amounts": [93312, 0, 0, /* … 54 entries total … */ 0]
    },
    /* … 3 more chests, index 1..3 … */
  ]
}
```

Invariants:

- Exactly 4 chests per node, indices 0..=3.
- `chest.id = node.id * 4 + chest.index`.
- `amounts.len() == 54` (one entry per shulker-box slot in the double
  chest).
- Chest with `item == "overflow"` is the bot's write-only failsafe — the
  only chest that may hold mixed item types.
- `amounts[n] <= max_stack * SHULKER_BOX_SLOTS` where `SHULKER_BOX_SLOTS =
  27`. Exceeding this means the shulker is over-capacity (impossible
  in-world; a schema violation).

## `data/orders.json`

In-memory audit log, mirrored to disk for visibility. Cleared on each
startup — the source of truth for historical orders is
`data/trades/*.json`. See [src/types/order.rs](../src/types/order.rs).

```json
[
  {
    "order_type": "Buy",
    "item": "cobblestone",
    "amount": 500,
    "user_uuid": "00000000-0000-0000-0000-0000000Alice"
  }
]
```

`order_type` is one of `"Buy" | "Sell" | "Deposit" | "Withdraw"`.

## `data/queue.json`

Pending orders waiting to be processed. Survives restarts. See
[src/store/queue.rs](../src/store/queue.rs).

```json
{
  "orders": [
    {
      "id": 1,
      "user_uuid": "uuid-0",
      "username": "player-0",
      "order_type": "Buy",
      "item": "cobblestone",
      "quantity": 1,
      "queued_at": "2026-04-17T14:41:14.596507800Z"
    }
  ],
  "next_id": 2
}
```

- `id` is monotonic across the queue's lifetime; `next_id` persists so ids
  don't collide after a restart.
- `order_type` for queue entries uses the `QueuedOrderType` enum which adds
  the `Deposit { amount: Option<f64> }` and `Withdraw { amount: Option<f64> }`
  variants on top of plain `"Buy"` / `"Sell"`.
- `queued_at` is RFC 3339 UTC.
- Length capped by `MAX_QUEUE_SIZE = 128` globally; 8 per user.

## `data/journal.json`

Active shulker-box operation, written every phase. A non-empty file at
startup means the previous run crashed mid chest I/O. See
[src/store/journal.rs](../src/store/journal.rs).

Historically serialized as a one-entry array for forward compatibility;
`load_from` reads a `Vec<JournalEntry>` and keeps only the last.

```json
[
  {
    "operation_id": 17,
    "operation_type": "WithdrawFromChest",
    "chest_id": 0,
    "slot_index": 3,
    "state": "ItemsTransferred"
  }
]
```

`operation_type`: `"WithdrawFromChest" | "DepositToChest"`.
`state`: `"ShulkerTaken" | "ShulkerOnStation" | "ItemsTransferred" |
"ShulkerPickedUp" | "ShulkerReplaced"`.

## `data/current_trade.json`

In-flight `TradeState` snapshot, rewritten on every phase transition and
deleted on terminal state. Any non-empty file at startup means a mid-trade
crash. See [src/store/trade_state.rs](../src/store/trade_state.rs).

The file holds one `TradeState` serialized as an externally-tagged enum —
shape differs by variant. Example (`Withdrawing`):

```json
{
  "Withdrawing": {
    "order": {
      "id": 42,
      "user_uuid": "uuid-X",
      "username": "PlayerX",
      "order_type": "Buy",
      "item": "cobblestone",
      "quantity": 64,
      "queued_at": "2026-04-17T14:41:14Z"
    },
    "plan": [
      { "chest_id": 0, "slot_index": 3, "amount": 64 }
    ]
  }
}
```

Other variants: `Queued`, `Trading`, `Depositing`, `Committed`, `RolledBack`.
See [RECOVERY.md](../RECOVERY.md) for what to do if this file is present on
startup.

## `data/trades/<timestamp>.json`

One immutable file per committed trade. Filename is the commit timestamp in
the bot's custom ISO-8601 form: `YYYY-MM-DDTHH-MM-SS.nnnnnnnnn+HH-MM`
(colons replaced with dashes because Windows file names don't allow `:`).

```json
{
  "trade_type": "Buy",
  "item": "cobblestone",
  "amount": 500,
  "amount_currency": 11250000.0,
  "user_uuid": "00000000-0000-0000-0000-0000000Alice",
  "timestamp": "2026-04-12T18:35:25.066418800Z"
}
```

`trade_type`: `"Buy" | "Sell" | "Deposit" | "Withdraw"`.
On startup the Store loads at most `max_trades_in_memory` files (newest
first); older files stay on disk untouched.

## Versioning policy

There is currently no `schema_version` field on any file. This is
intentional — the project is pre-1.0 and the set of files is small enough
to migrate by hand when needed. Until a versioning scheme is introduced:

- **Additive changes** (new optional fields, new enum variants that
  existing code ignores) are safe. Use `#[serde(default)]` on the Rust
  side.
- **Renames and removals** are breaking and require a one-shot migration
  script checked in under `tools/` (no tooling exists yet because no such
  migration has been needed).
- Reject-on-unknown is NOT set anywhere, so a garbled field becomes
  default rather than a load error — prefer hand-auditing after
  schema-shape edits.
