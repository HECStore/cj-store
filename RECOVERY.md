# cj-store — Recovery Runbook

Operator playbook for the handful of failure modes that require manual
intervention. For normal operation see [README.md](README.md); for
architecture / on-disk formats see [ARCHITECTURE.md](ARCHITECTURE.md) and
[DATA_SCHEMA.md](DATA_SCHEMA.md).

## General principles

1. **Stop the bot first.** All recovery procedures assume the process is
   not running. Every fix below starts with "Exit" in the CLI menu (or
   Ctrl-C if the CLI is unresponsive).
2. **Snapshot `data/` before editing.** A flat copy is enough:
   ```bash
   cp -r data data.bak.$(date -u +%Y%m%d-%H%M%S)
   ```
   PowerShell equivalent:
   ```powershell
   Copy-Item -Recurse data "data.bak.$(Get-Date -Format yyyyMMdd-HHmmss)"
   ```
   Every procedure below is reversible as long as a snapshot exists.
3. **Validate the config after any edit.** `cargo run -- --dry-run`
   exits 0 if `data/config.json` parses and passes `Config::validate`. It
   does *not* validate the other JSON files — those are checked on Store
   startup.
4. **Check the tail of the log.** Relevant warnings and errors go to
   `data/logs/store.log`. Look for `[Store]`, `[Bot]`, `[Journal]`, and
   `[Connection]` prefixes.

## Terminology & decoding

The same scraps of arithmetic and state-name mapping show up in several
recovery procedures. Keep this subsection open in another tab while
working through any of them.

**Chest IDs.**

- Forward: `chest_id = node_id * 4 + chest_index`
- Reverse: `node_id = chest_id / 4`, `chest_index = chest_id % 4`

**Journal state → in-world state.** Use this when decoding a leftover
`data/journal.json` entry:

| `state`              | Where the shulker is                                                         |
| -------------------- | ---------------------------------------------------------------------------- |
| `ShulkerTaken`       | In the bot's inventory; chest slot is empty                                  |
| `ShulkerOnStation`   | On the station block (2 blocks west of the node's standing position)         |
| `ItemsTransferred`   | On the station; its contents moved into/out of the bot's inventory           |
| `ShulkerPickedUp`    | In the bot's inventory again after items were transferred                    |
| `ShulkerReplaced`    | Back in its chest slot; the journal entry was about to be cleared            |

**Trade phase ladder.** `TradeState` advances strictly forward; any two
adjacent phases are at most one crash apart:

```
Queued → Withdrawing → Trading → Depositing → Committed
                                               └── RolledBack (any failure)
```

---

## 1. Corrupted `data/pairs/<item>.json`

**Symptoms**

- Bot fails to start with a JSON parse error mentioning the file.
- A specific pair disappears from the `pairs` CLI menu (the Store logs
  `Skipping pair with empty item name …` or `Skipping pair with invalid
  item name …` on load).
- AMM prices are suddenly absurd for one pair (see
  `MIN_RESERVE_FOR_PRICE` in [src/constants.rs](src/constants.rs)).

**Fix**

1. Stop the bot.
2. Open the offending `data/pairs/<item>.json`. Expected shape (see
   [DATA_SCHEMA.md](DATA_SCHEMA.md#datapairsitemjson)):
   ```json
   {
     "item": "cobblestone",
     "stack_size": 64,
     "item_stock": 0,
     "currency_stock": 21250000.0
   }
   ```
3. Decide the intended reserves. If the file is missing reserves or they
   look wrong:
   - `item_stock` must equal the sum of `amounts[]` across every chest
     whose `item == "<item>"` in `data/storage/*.json`. Add these up and
     write the sum.
   - `currency_stock` is not derivable from elsewhere — consult the last
     known good value (from a `data.bak.*` snapshot or `data/logs/store.log`
     around the last price quote).
4. If the file is unfixable, delete it. The pair will be gone on next
   startup; operators can recreate it via CLI menu option 8 "Add pair",
   which sets both stocks to zero. Zero-stock pairs will refuse all
   buys/sells, so expect to also use `additem` / `addcurrency` operator
   whispers to seed stock before players can trade.
5. Restart the bot. Watch the log for `Loaded N pairs`.
6. In the CLI, run `audit-state` to cross-check that pair stocks match the
   chest totals.

**Why this can happen**: hand-edit typo, disk full during an atomic write
(rare — the rename step is atomic on both NTFS and POSIX), or a half-synced
backup restore.

---

## 2. Stuck `data/journal.json` entry

The journal records one in-flight shulker operation at a time; it is
cleared whenever the operation finishes. A non-empty file at startup means
the previous run crashed mid chest I/O. Current behavior: the Store logs a
loud warning and clears the file automatically (see
[src/store/journal.rs](src/store/journal.rs)). The world state may be
inconsistent — that's what this playbook is for.

**Symptoms**

- Startup log shows `[Journal] Leftover entry found: …`.
- A shulker box is sitting on the station block instead of inside its
  chest.
- A shulker box is in the bot's inventory on login.
- Items are dropped on the floor near the station.
- `audit-state` reports that `pair.item_stock` disagrees with the sum of
  chest `amounts[]`.

**Fix**

First identify the operation from the leftover entry. The fields to look
at are `operation_type` (`WithdrawFromChest` / `DepositToChest`),
`chest_id`, `slot_index`, and `state`. Decode `chest_id` and map `state`
using the [Terminology & decoding](#terminology--decoding) section above.

Recovery steps (pick one):

**Option A — let the bot self-correct (preferred when possible)**

Fewer hand-edits, and `apply_chest_sync` is the authoritative
reconciliation path the bot already runs on every chest visit — so you're
re-using well-exercised code instead of hand-computing a sum.

1. Stop the bot.
2. Physically break and pick up any loose shulker on the station or in the
   area. Put it in the correct chest slot (the slot the journal names).
3. If items are on the ground, pick them up and deposit them into the
   overflow chest (node 0, chest 1) — the bot will triage them via the
   usual deposit flow once running.
4. Restart the bot. It will clear the journal on load, and the next trade
   involving that chest will do a fresh `apply_chest_sync`, reconciling
   per-slot counts from what's actually in-world.
5. Run `audit-state` to confirm the pair's `item_stock` now matches the
   chest sum. If it doesn't, go to Option B.

**Option B — manually reconcile pair stock**

Use this when the previous crash moved *some but not all* items and
Option A doesn't balance out.

1. Stop the bot.
2. Open `data/storage/<node_id>.json`, find the chest by `chest_index`,
   and edit `amounts[slot_index]` to the actual count inside that shulker
   box in-world. Save.
3. Open `data/pairs/<item>.json` for the chest's `item`. Set `item_stock`
   to the sum of `amounts[]` across *all* chests with that `item`. Save.
4. Validate: `cargo run -- --dry-run` (checks config; the pair/storage
   consistency check runs on full startup via `audit-state`).
5. Start the bot and run `audit-state`. It should report no drift.

**When it's safe to just delete `data/journal.json`**

Always, at startup. The bot self-heals by clearing it. The procedures
above are for fixing the *world/ledger drift* that the journal merely
points at — deleting the journal alone does not fix the drift.

---

## 3. Orphaned shulker in bot inventory

The bot expects its inventory to be empty outside of the middle of a
chest operation. A shulker left behind wastes an inventory slot and will
confuse the next chest operation.

**Symptoms**

- `buffer_chest_position` is configured and a shulker ends up there on
  every startup.
- Trade GUI rejects a bot offer because a slot the bot meant to place
  diamonds into already had a shulker.
- Logs from `ensure_shulker_in_hotbar_slot_0` or `recover_shulker_to_slot_0`
  show repeated recovery attempts at the start of a chest op.

**Fix**

1. Stop the bot (CLI "Exit", or Ctrl-C if unresponsive).
2. Log in to the server as a normal player and stand near the bot's
   last-known `position` (from `data/config.json`). When the server drops
   the disconnected bot's inventory to the ground after a few minutes,
   pick up the loose shulker and put it back into the chest slot named
   by any leftover journal entry (see section 2 for decoding `chest_id`
   and `slot_index`).
3. Start the bot. On its first chest op it will `apply_chest_sync` the
   affected chest and reconcile the per-slot counts from what's actually
   in-world.
4. If the CLI also reports a frozen queue ("processing_order stuck"),
   run menu option 15 "Clear stuck order" to release it before the next
   order can be serviced.

**Prevention**: always exit via the CLI "Exit" menu. A graceful shutdown
runs through the full disconnect sequence and never leaves in-flight
shulkers. Ctrl-C from the terminal is not graceful and *will* cause this
in about 5% of cases.

---

## 4. Interrupted `data/current_trade.json`

Same family as section 2, but at a higher level: a trade crashed after
being popped from the queue but before reaching a terminal state.

**Symptoms**

- Startup log shows a warning from `Store::new` about a leftover trade
  state file.
- A player reports their last buy/sell "never finished" — no trade
  confirmation, no balance change, but items moved.

> [!TIP]
> If the *only* symptom is that the queue has stopped advancing (no
> physical/ledger drift suspected), CLI menu option 15 **"Clear stuck
> order"** is the shortest path: it releases `processing_order` and
> returns the blocked queue entry, no JSON editing needed. Use the
> per-phase procedure below only when items, balances, or reserves
> need manual reconciliation.

**Commit math.** Every committed order mutates pair reserves and user
balance deterministically. Use this table to reconstruct what a crashed
commit *would* have done; each phase-subsection below refers back to it.

| Order type          | `pair.item_stock` | `pair.currency_stock` | `user.balance`     |
| ------------------- | ----------------- | --------------------- | ------------------ |
| `Buy` (qty, cost)   | `− qty`           | `+ cost`              | unchanged†         |
| `Sell` (qty, payout)| `+ qty`           | `− payout`            | `+ fractional`†    |
| `DepositBalance`    | unchanged         | unchanged             | `+ amount`         |
| `WithdrawBalance`   | unchanged         | unchanged             | `− amount`         |

† Buy may debit balance if the player paid via balance; sell pays whole
diamonds via trade and credits only the fractional remainder. See
[src/store/orders.rs](src/store/orders.rs) `execute_queued_order` for the
authoritative math.

**Fix**

Open `data/current_trade.json`. The outermost key is the phase name. Then:

### Phase: `Queued`

No physical work was done. The order was popped but validation/planning
hadn't started.

1. Stop the bot.
2. Either delete `data/current_trade.json` (the order is lost — tell the
   player to re-submit) OR copy the `order` object back into
   `data/queue.json` at position 0 of `orders` and bump `next_id` if the
   id would collide.
3. Start the bot.

### Phase: `Withdrawing`

The bot had started moving items out of storage but had not opened the
trade GUI. No player-side effect yet.

1. Stop the bot.
2. Follow [§ 2 Option A steps 2–4](#2-stuck-datajournaljson-entry)
   (reseat shulker, reconcile via `audit-state`).
3. Delete `data/current_trade.json` — the order is cancelled.
4. Inform the player no trade happened; no balance change needed.

### Phase: `Trading`

The trade GUI was open with the player when the bot crashed. You can
almost always reconstruct whether the trade confirmed by looking at the
bot's inventory — reach for player reports only if that's ambiguous.

1. Stop the bot.
2. **Physical inventory check first.** Look at the bot's inventory and
   the buffer chest (if configured). The "bot offers" half of the trade
   either:
   - is still in the bot's inventory → the trade **never confirmed**.
     Treat as cancelled. Section 3 applies; put the shulker back into
     its chest slot.
   - is missing (and the player, if online, now has those items) → the
     trade **confirmed** before the crash. Treat as committed: apply the
     [Commit math table](#4-interrupted-datacurrent_tradejson) for the
     order's type. (Note: storage counts were *not* synced back after the
     crash, so run `audit-state` after restart.)
3. **Only if step 2 is ambiguous**, contact the affected player. If they
   say the trade went through, treat as committed; otherwise treat as
   cancelled. Server logs can corroborate either way.
4. Delete `data/current_trade.json`.

### Phase: `Depositing`

The GUI completed; the bot was putting received items back into storage
when it crashed. This is the most common crash point because it involves
multiple chest ops.

1. Stop the bot.
2. Read `trade_result.items_received` from `current_trade.json` — that
   is what the player actually sent. Compare against `deposit_plan`
   (what the bot intended to deposit).
3. Go into the world, find any shulkers on the station / in the bot /
   on the floor near the destination chest, and put them in the chest
   slot named by the relevant plan entry.
4. Manually mirror the commit using the [Commit math table](#4-interrupted-datacurrent_tradejson)
   at the top of this section — apply the row for this order's type to
   `data/pairs/<item>.json` and `data/users/<uuid>.json`.
5. Append a manual entry to `data/trades/<now>.json` matching the
   completed trade so the audit log isn't missing it. Shape is in
   [DATA_SCHEMA.md](DATA_SCHEMA.md#datatradestimestampjson).
6. Delete `data/current_trade.json`.
7. Start the bot and run `audit-state` — it must report no drift.

### Phase: `Committed` / `RolledBack`

These are terminal states. The file should already be deleted; finding
one at startup means the bot crashed *after* marking the trade terminal
but *before* removing the file. The ledger mutation itself already
happened (or was already rolled back — the state name tells you which),
so usually nothing is missing.

Don't just delete the file blindly, though:

1. Stop the bot.
2. Open `data/current_trade.json`. Note the `order` body (item, quantity,
   user, order type).
3. For `Committed`: verify `pair.item_stock`, `pair.currency_stock`, and
   the user's `balance` match the [Commit math table](#4-interrupted-datacurrent_tradejson).
   If they don't match — the crash landed *between* the state change and
   the file delete but somehow skipped the ledger write — follow the
   `Depositing` procedure to reconcile.
4. For `RolledBack`: verify the ledger is *unchanged* (no pair or
   balance update for this order), and that no stray shulker is in the
   bot's inventory or on the station (section 3 if there is).
5. Delete `data/current_trade.json`.

---

## 5. Bot connection problems (operator action required)

**"Failed to connect"**. Check `account_email` and `server_address` in
`data/config.json`. Re-run with `cargo run -- --dry-run` to validate the
file without attempting login.

**"Duplicate login"**. Minecraft allows exactly one authenticated
connection per account: the moment a second client logs in, the server
kicks whichever one is already connected — in practice, the bot is the
one that gets kicked. If the bot keeps disconnecting with a duplicate-
login reason, log out of every other client using this account
(including any Minecraft launcher that auto-joins on startup) and then
restart the bot, or let its reconnect backoff do it.

**"Protocol decode errors" that don't self-heal**. The server is running a
Minecraft version that Azalea can't talk to. Either downgrade the server
or wait for an Azalea update — nothing on the configuration side can fix
this.

---

## 6. Storage drift and missing stock

**"Chest not found"**. The physical chest doesn't exist where the model
says it should. Verify the node was built correctly before adding it via
CLI option 5 (validated add) instead of option 4 (unvalidated).

**"Storage mismatch" / audit-state reports drift**. Pair stock disagrees
with chest sum. Example: `pair.item_stock` says 100, storage chests for
that item sum to 80 — 20 items went missing without going through the
bot. Causes: items moved manually in-world, a crashed chest op (see
section 2), or a legitimate bug. Fix: CLI option 13 "Repair state"
recomputes `pair.item_stock` from the actual storage contents.

**"Node 0 chest 0" item-assignment errors**. Chest 0 of node 0 is dedicated
to diamonds; the system refuses other items. Don't try to override it.

**"Out of physical stock"** (pair says stock, storage disagrees). Happens
when items were removed externally or after a mid-op crash. Run "Repair
state"; if that doesn't help, add items back via operator `additem`.

**"Storage full"**. All assigned chests are full and no empty chests are
available. The system will assign a new chest in an existing node or
provoke a new node — but a new node requires the physical build to exist
in-world. Add nodes via CLI options 4 / 5 first.

---

## 7. Trade failures seen by players

Most trade-failure messages (`Trade timeout`, `Trade cancelled by player`,
`Trade validation failed`, etc.) are self-explanatory, safe, and require
no operator action — the player can just re-submit. The exception:

- **"Inventory full"**. The hotbar-to-inventory sweep and (optional)
  `buffer_chest_position` normally keep the bot's inventory drained. If
  this persists, stop the bot and clear the inventory manually — see
  section 3.

---

## 8. Validation and edge cases

These show up as inline error messages to players. None require operator
action; they're documented here as a cross-reference.

- Negative, zero, non-numeric, or > `i32::MAX` quantities — rejected.
- Non-existent pair — "Item 'X' is not available for trading".
- Insufficient stock (physical or ledger) — order rejected.
- Insufficient funds (balance + offered diamonds for buys; reserve for
  sells) — order rejected.
- Price calculation impossible (`item_stock == 0` or `currency_stock == 0`,
  or computed cost non-finite / ≤ 0) — order rejected with "Internal
  error: computed price is invalid" (the "internal error" wording is
  intentional — it means a pair has drained and the operator needs to
  re-seed reserves).
- Mojang API unreachable / unknown username — lookup fails for the first
  command from that player; subsequent commands in the 5-minute cache
  window succeed.

---

## 9. Rate-limiter and queue messages

Rate-limit and queue-full messages are intended player feedback, not
errors. The only operator-relevant note:

- To cancel someone else's queue entry (the in-game `cancel` command
  rejects this as `"You can only cancel your own orders"`), stop the
  bot, edit `data/queue.json` by hand, and restart.

See [ARCHITECTURE.md § Rate limiting](ARCHITECTURE.md#rate-limiting-anti-spam)
and [ARCHITECTURE.md § Queue limits](ARCHITECTURE.md#queue-limits) for the
rules behind the messages.

---

## Reference

- The bot itself **never** touches `data.bak.*` directories, so snapshots
  left alongside `data/` are safe.
- Every file listed here is described in
  [DATA_SCHEMA.md](DATA_SCHEMA.md).
- All writes through `fsutil::write_atomic` are durable across power
  loss; the only way to get a half-written JSON file is if someone (or
  something) writes to `data/` outside the bot.
- After any manual edit to `data/pairs/` or `data/storage/`, run CLI
  menu option 12 **"Audit state"** first (read-only — reports drift
  without touching anything); only if the reported drift matches what
  you expected, run option 13 **"Repair state"** to let the Store
  recompute `pair.item_stock` from the chest sums.
- For performance tuning (slow chest I/O, autosave thrash, queue stalls)
  see [DEVELOPMENT.md § Performance tuning](DEVELOPMENT.md#performance-tuning).

---

## Appendix: benign log noise

These show up in `data/logs/store.log`, are self-handled by the bot, and
need no operator action. Listed so you recognize them when scanning logs.

- **Packet decode errors** (e.g. `set_equipment ... Unexpected enum variant`).
  Protocol drift between the server build and Azalea's decoder. Usually
  single-packet; the connection is not dropped.
- **Duplicate-login disconnect, handled automatically**. The bot reconnects
  with exponential backoff. See section 5 for when this stops being
  "handled" and becomes operator work.
- **"Global logger already set"** on reinit. The tracing bootstrap is
  idempotent — the warning is swallowed silently.

If any of these stop self-healing and the bot stays offline for more
than a few minutes, the reconnect backoff has probably given up;
restart via CLI "Restart Bot" or restart the process.
