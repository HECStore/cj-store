# cj-store — Data Schema

Reference for every JSON file the bot reads or writes under `data/`.
Companion to [ARCHITECTURE.md](ARCHITECTURE.md) (runtime owners of each
file) and [RECOVERY.md](RECOVERY.md) (what to do when one is corrupt or
stuck). This document is the quick lookup — *where* each file lives,
*what shape* it has, and *what owns it* at runtime.

All files are hand-writable JSON. Writes go through `fsutil::write_atomic`
(write to `<file>.tmp`, then rename) so a crash mid-save never corrupts the
existing file.

## File map

| Path                             | Owner (in `Store`)    | Written when                                     | Created when              | Versioned? |
| -------------------------------- | --------------------- | ------------------------------------------------ | ------------------------- | ---------- |
| `data/config.json`               | `Store.config`        | on operator edit (hot-reloaded)                  | startup                   | No         |
| `data/pairs/<item>.json`         | `Store.pairs`         | on every trade commit + debounced autosave       | ≥1 before first trade     | No         |
| `data/users/<uuid>.json`         | `Store.users`         | on deposit / withdraw / pay + debounced autosave | created on first observe  | No         |
| `data/storage/<node_id>.json`    | `Store.storage`       | on every `apply_chest_sync` + debounced autosave | ≥1 before first trade     | No         |
| `data/orders.json`               | `Store.orders`        | on debounced autosave (cleared at startup)       | runtime-created           | No         |
| `data/queue.json`                | `Store.order_queue`   | on every push / pop (survives restart)           | runtime-created           | No         |
| `data/journal.json`              | `Journal` (chest I/O) | on every shulker-op phase change                 | runtime-created           | No         |
| `data/current_trade.json`        | `Store.current_trade` | on every `TradeState` transition                 | runtime-created           | No         |
| `data/trades/<timestamp>.json`   | `Store.trades`        | once per committed trade (immutable thereafter)  | runtime-created           | No         |
| `data/logs/store.log`            | `tracing` appender    | on every log line                                | runtime-created           | —          |

Notes:

- "No" under *Versioned?* means there is no `schema_version` field. Adding
  new fields is backwards compatible (serde defaults / `#[serde(default)]`);
  renaming fields or changing enum variant names is a breaking change.
- `data/logs/` is not parsed by the bot — it exists purely for operators.

## `data/config.json`

Human-edited. Loaded at startup; created with defaults on first run (the
bot then fails on auth — expected; edit the file and run again). The
`--validate-only` / `--dry-run` CLI flag parses this file, runs
`Config::validate`, and exits without logging in — exit code `0` on
success, `1` on validation error (useful in CI or before restarting a
production bot). Hot-reloaded on file change (debounced ≈500 ms via the
[`notify`](https://crates.io/crates/notify) crate); a bad edit keeps the
running config and logs the error.

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

### Fields

| Setting                   | Type             | Default | Description                                                                                                          |
| ------------------------- | ---------------- | ------- | -------------------------------------------------------------------------------------------------------------------- |
| `position`                | `{x, y, z}`      | —       | Storage origin — where Node 0 lives in the world                                                                     |
| `fee`                     | `f64`            | —       | Fee rate (e.g. `0.125` = 12.5 %) — added to buys, subtracted from sells                                              |
| `account_email`           | string           | —       | Microsoft account email for Azalea login (**required**)                                                              |
| `server_address`          | string           | —       | Minecraft server hostname, e.g. `"corejourney.org"` (**required**)                                                   |
| `buffer_chest_position`   | `{x,y,z} \| null`| `null`  | Optional emergency-dump chest. Used when a shulker cannot be returned to its slot (slot unexpectedly occupied, chunk not loaded, etc.) — a non-fatal fallback so the bot doesn't stall mid-operation. Leave `null` and the bot instead keeps the shulker in its inventory and logs an alert |
| `trade_timeout_ms`        | `u64`            | 45000   | Max wait for a trade-GUI interaction before aborting                                                                 |
| `pathfinding_timeout_ms`  | `u64`            | 60000   | Max wait for the bot to reach a destination before aborting                                                          |
| `max_orders`              | `usize`          | 10000   | Prune target for the in-memory order audit log (session-only)                                                        |
| `max_trades_in_memory`    | `usize`          | 50000   | Max trades loaded into memory on startup (older trades stay on disk)                                                 |
| `autosave_interval_secs`  | `u64`            | 2       | Minimum interval between debounced autosaves                                                                         |

All timeout and limit fields are optional and fall back to the defaults
above if omitted.

### Constraints

Enforced by `Config::validate` in [src/config.rs](src/config.rs):

- `fee ∈ [0.0, 1.0]`
- `server_address` non-empty; no `://`, no `/`, no whitespace; only ASCII
  alphanum / `.` / `-` / `:`; optional `:port` must parse as `u16`
- all timeouts / limits positive

A `position.y` outside the modded-vanilla range `[-64, 320]` logs a
warning but does not fail validation — some servers extend world height.

### Hot-reload matrix

| Field                                      | Hot-reloadable? | Notes                                                                   |
| ------------------------------------------ | --------------- | ----------------------------------------------------------------------- |
| `fee`                                      | ✅ Yes          | Next priced order uses the new rate                                     |
| `autosave_interval_secs`                   | ✅ Yes          | Next Store loop iteration uses the new debounce                         |
| `trade_timeout_ms`                         | ❌ Restart      | Cached in the Bot task at startup; warning logged on edit               |
| `pathfinding_timeout_ms`                   | ❌ Restart      | Cached in the Bot task at startup; warning logged on edit               |
| `position`, `buffer_chest_position`        | ❌ Restart      | World topology; navigation state is seeded at startup and changing either mid-run would break in-flight operations |
| `account_email`, `server_address`          | ❌ Restart      | Identity / connection; requires reconnection                            |
| `max_orders`, `max_trades_in_memory`       | ❌ Restart      | Capacity bounds fixed at load time                                      |

Edits to restart-only fields emit `warn!("Config field '<name>' changed
but requires restart")` and the in-memory config keeps its original value
so behavior stays consistent with what the rest of the system was
initialized against.

## `data/pairs/<item>.json`

One file per trading pair. Filename is the canonical item id (no
`minecraft:` prefix). See [src/types/pair.rs](src/types/pair.rs).

```json
{
  "item": "cobblestone",
  "stack_size": 64,
  "item_stock": 0,
  "currency_stock": 21250000.0
}
```

- `stack_size` ∈ {1, 16, 64}. Set at pair creation via CLI option 8 and
  not intended to change afterwards — the AMM and the deposit planner
  both assume it's constant for the lifetime of the pair.
- `item_stock` must match the sum of all in-world inventory for this
  item across every chest whose `item == "<item>"`. Drift is flagged by
  CLI option 12 "Audit state".
- `currency_stock` is the diamond reserve. Normal trades update it as
  part of the commit — credited on buys, debited on sells — so the AMM
  invariant `k = item_stock × currency_stock` grows only by the fee on
  each trade. Changing either stock directly (without the other) re-prices
  the pair instantly; don't hand-edit unless you know what you're doing.

## `data/users/<uuid>.json`

One file per known player. Filename is the hyphenated Mojang UUID. See
[src/types/user.rs](src/types/user.rs).

```json
{
  "uuid": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
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
`node_id` starting at 0. See [src/types/node.rs](src/types/node.rs) and
[src/types/chest.rs](src/types/chest.rs).

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

Invariants (checked by CLI option 12 "Audit state" where noted):

- Exactly 4 chests per node, indices 0..=3. *Enforced at load time.*
- `chest.id = node.id * 4 + chest.index`. *Convention followed by every
  writer; not verified on load.*
- `amounts.len() == 54` (one entry per shulker-box slot in the double
  chest). *Normalized at load time: a vector of the wrong length is
  silently resized to 54 (padded with zeros or truncated).*
- Chest with `item == "overflow"` is the bot's write-only failsafe — the
  only chest that may hold mixed item types. *Enforced at deposit planning;
  the withdraw planner refuses to source from it.*
- `amounts[n] <= max_stack * SHULKER_BOX_SLOTS` where `SHULKER_BOX_SLOTS =
  27`. Exceeding this means the shulker is over-capacity (impossible
  in-world; a schema violation). *Checked by audit-state.*
- For every `pair`, `pair.item_stock == sum(chest.amounts[] for chest.item
  == pair.item across all nodes)`. *Checked by audit-state; repaired by
  CLI option 13.*

## `data/orders.json`

Session-scoped audit log. The Store mirrors it to disk so an operator can
tail the file or view it after a crash, but the bot rebuilds it from scratch
on every startup — the persistent source of truth for historical orders is
always `data/trades/*.json`. This file exists primarily to back CLI
option 11 ("View recent trades") without forcing a full rescan of
`data/trades/` on every invocation. See [src/types/order.rs](src/types/order.rs).

```json
[
  {
    "order_type": "Buy",
    "item": "cobblestone",
    "amount": 500,
    "currency_amount": 0.0,
    "user_uuid": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
  }
]
```

`order_type` is one of `"Buy" | "Sell" | "AddItem" | "RemoveItem" |
"DepositBalance" | "WithdrawBalance" | "AddCurrency" | "RemoveCurrency"` —
see [src/types/order.rs](src/types/order.rs). Only runtime tracking for
the current session; historical records live in `data/trades/*.json`.

`currency_amount` is the diamond-denominated value associated with the
order; for `AddCurrency` / `RemoveCurrency` this is the real amount moved,
and for all other variants it is `0.0`. The field has `#[serde(default)]`
so older snapshots without it still load.

## `data/queue.json`

Pending orders waiting to be processed. Survives restarts. See
[src/store/queue.rs](src/store/queue.rs).

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
  don't collide after a restart — this is what makes `cancel <id>` and
  similar operator references unambiguous across a process restart.
- `order_type` for queue entries uses the `QueuedOrderType` enum which adds
  the `Deposit { amount: Option<f64> }` and `Withdraw { amount: Option<f64> }`
  variants on top of plain `"Buy"` / `"Sell"`.
- `queued_at` is RFC 3339 UTC.
- Length capped by `MAX_QUEUE_SIZE = 128` globally; 8 per user.
- On corrupt-JSON load (`InvalidData`), `OrderQueue::load_from` renames the
  bad file to `data/queue.json.corrupt-<stamp>` before starting with an
  empty queue, so the raw bytes survive for forensic recovery instead of
  being overwritten by the next `save()`. `<stamp>` is an RFC 3339
  timestamp with every `:` replaced by `-` (Windows/NTFS reject `:` in
  filenames; the colon-stripping happens in [src/store/queue.rs](src/store/queue.rs)),
  so on disk you'll see e.g. `data/queue.json.corrupt-2026-04-29T15-30-45.123456789+00-00`.
  The Store logs this as an `error!` with a `PENDING ORDERS LOST` marker.

## `data/journal.json`

Active shulker-box operation, written every phase. A non-empty file at
startup means the previous run crashed mid chest I/O. See
[src/store/journal.rs](src/store/journal.rs).

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
crash. See [src/store/trade_state.rs](src/store/trade_state.rs).

The file holds one `TradeState` serialized as an externally-tagged enum —
shape differs by variant. Two examples matter most for recovery:

`Withdrawing` (bot was pulling from storage, no player interaction yet):

```json
{
  "Withdrawing": {
    "order": {
      "id": 42, "user_uuid": "uuid-X", "username": "PlayerX",
      "order_type": "Buy", "item": "cobblestone", "quantity": 64,
      "queued_at": "2026-04-17T14:41:14Z"
    },
    "plan": [
      {
        "chest_id": 0,
        "position": { "x": 100, "y": 64, "z": 100 },
        "item": "cobblestone",
        "amount": 64
      }
    ]
  }
}
```

`Depositing` (GUI completed; bot was putting items back — hardest to
hand-reconcile, referenced by [RECOVERY.md § 4](RECOVERY.md#phase-depositing)):

```json
{
  "Depositing": {
    "order": { /* same QueuedOrder shape as above */ },
    "trade_result": {
      "items_received": [
        { "item": "cobblestone", "amount": 64 }
      ]
    },
    "deposit_plan": [
      {
        "chest_id": 2,
        "position": { "x": 106, "y": 64, "z": 100 },
        "item": "cobblestone",
        "amount": 64
      }
    ]
  }
}
```

Other variants: `Queued(QueuedOrder)`, `Trading { order, withdrawn }`,
`Committed(CompletedTrade)`, `RolledBack { order, reason }`. See
[RECOVERY.md](RECOVERY.md) for what to do if this file is present on
startup.

## `data/trades/<timestamp>.json`

One immutable file per committed trade. Filename is the commit timestamp
in ISO-8601, but with `:` replaced by `-` because Windows disallows `:`
in filenames: `YYYY-MM-DDTHH-MM-SS.nnnnnnnnn+HH-MM`. The `timestamp`
field inside the file is the real ISO-8601 form (with colons) — use that
when parsing.

```json
{
  "trade_type": "Buy",
  "item": "cobblestone",
  "amount": 500,
  "amount_currency": 11250000.0,
  "user_uuid": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
  "timestamp": "2026-04-12T18:35:25.066418800Z"
}
```

`trade_type` is one of `"Buy" | "Sell" | "AddStock" | "RemoveStock" |
"DepositBalance" | "WithdrawBalance" | "AddCurrency" | "RemoveCurrency"`
— see [src/types/trade.rs](src/types/trade.rs).
On startup the Store loads at most `max_trades_in_memory` files (newest
first); older files stay on disk untouched.

## Versioning policy

There is currently no `schema_version` field on any file. This is
intentional — the project is pre-1.0 and the set of files is small enough
to migrate by hand when needed. Until a versioning scheme is introduced:

- **Additive changes** (new optional fields) are safe. Use
  `#[serde(default)]` on the Rust side so older files still load.
- **Adding an enum variant** is only safe if the variant name does not
  appear in any existing file. Serde encodes Rust enum variants by their
  Rust name, so renaming `Buy` → `BuyOrder` is a breaking change even
  though the wire format "looks the same".
- **Renames and removals** of fields or enum variants are breaking and
  require a one-shot migration script checked in under `tools/` (no
  tooling exists yet because no such migration has been needed).
- `deny_unknown_fields` is set on `Config` (the only hand-edited file), so
  a typo like `"fe": 0.125` or `"buffer_chset_position"` now fails the
  load with a clear error. It is deliberately **not** set on the other
  JSON types (`Pair`, `Trade`, `Order`, `User`, `Storage`, `Queue`,
  journal, trade-state), which are bot-written — adding it there would
  block forward-compat reads of files written by a slightly newer binary.
  Hand-audit those after any schema-shape change.
