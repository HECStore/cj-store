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
   Every procedure below is reversible as long as a snapshot exists.
3. **Validate the config after any edit.** `cargo run -- --dry-run`
   exits 0 if `data/config.json` parses and passes `Config::validate`. It
   does *not* validate the other JSON files — those are checked on Store
   startup.
4. **Check the tail of the log.** Relevant warnings and errors go to
   `data/logs/store.log`. Look for `[Store]`, `[Bot]`, `[Journal]`, and
   `[Connection]` prefixes.

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
   startup; operators can recreate it via the CLI `addpair` flow, which
   sets both stocks to zero. Zero-stock pairs will refuse all buys/sells,
   so expect to also use `additem` / `addcurrency` operator whispers to
   seed stock before players can trade.
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
`chest_id`, `slot_index`, and `state`. Decode `chest_id` to `(node_id,
chest_index)` as `node_id = chest_id / 4`, `chest_index = chest_id % 4`.

Then, physically, in the world:

| `state`              | What's on the ground / in the bot |
| -------------------- | --------------------------------- |
| `ShulkerTaken`       | Shulker is in bot's inventory; chest slot is empty |
| `ShulkerOnStation`   | Shulker is on the station block  |
| `ItemsTransferred`   | Shulker on station; its contents have been moved into/out of bot's inventory |
| `ShulkerPickedUp`    | Shulker is in bot's inventory (post-transfer) |
| `ShulkerReplaced`    | Shulker is back in its chest slot; journal was about to be cleared |

Recovery steps (pick one):

**Option A — let the bot self-correct (preferred when possible)**

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

- Bot log shows `[ChestIO] unexpected shulker in inventory slot N on entry`.
- `buffer_chest_position` is configured and a shulker ends up there on
  every startup.
- Trade GUI rejects a bot offer because a slot the bot meant to place
  diamonds into already had a shulker.

**Fix**

1. Stop the bot.
2. Log in to the server as a normal player (or use /co inspect / whatever
   your server provides) and stand next to the bot.
3. Restart the bot, then immediately in the CLI pick "Restart bot" —
   wait for it to reconnect. On connect the bot will walk to a node for
   its next operation; interrupt this by setting a deliberate pause:
   easier approach below.
4. Alternative: stop the bot, log in yourself at the bot's last-known
   position (around `position` in config), and manually break any shulker
   *you* dropped from the bot's killed process. The server should drop
   the bot's inventory to the ground on a disconnect after a few minutes;
   pick it up and put it back in the correct chest (the one the journal
   entry, if any, names — see section 2).
5. Start the bot. On first chest op it will detect the shulker slot is
   already empty and repopulate it from the journal/sync path.

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
2. Follow section 2 recovery: physically put back any shulker on the
   station / in the bot's inventory / on the ground, and reconcile the
   affected pair's `item_stock` via `audit-state`.
3. Delete `data/current_trade.json` — the order is cancelled.
4. Inform the player no trade happened; no balance change needed.

### Phase: `Trading`

The trade GUI was open with the player. The state is unrecoverable
without player input: you don't know whether the GUI was confirmed,
partial, or cancelled.

1. Stop the bot.
2. Physically check the bot's inventory and any buffer chest. The "bot
   offers" half of the trade GUI items either:
   - is still in the bot's inventory (trade never confirmed) — section 3
     applies; put the shulker back.
   - is missing (trade confirmed) — the player got the items, so you owe
     yourself a ledger entry. Decide: either treat the trade as committed
     (manually deduct from `item_stock` / credit `currency_stock` — mirror
     what a normal `Buy` commit would do for that pair) or eat the loss.
3. Contact the affected player. If they say the trade went through on
   their end, treat as committed; if not, treat as cancelled and Mojang/
   server logs back you up either way.
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
4. Manually update the pair(s):
   - For `Buy` orders: the pair's `item_stock` should *decrease* by
     `quantity`, and `currency_stock` should *increase* by the diamond
     amount paid. Check the order and apply.
   - For `Sell` orders: inverse — `item_stock` up by `items_received`
     sum, `currency_stock` down by the payout.
   - For `Deposit` / `Withdraw`: no pair changes; these move user balance
     only.
5. Manually update the user's `balance` in `data/users/<uuid>.json`
   (add payout for `Sell`, add credit for `Deposit`, deduct for
   `Withdraw`, no change for `Buy`). Reference
   [src/store/orders.rs](src/store/orders.rs) `execute_queued_order` for
   the exact commit math.
6. Append a manual entry to `data/trades/<now>.json` matching the
   completed trade so the audit log isn't missing it. Shape is in
   [DATA_SCHEMA.md](DATA_SCHEMA.md#datatradestimestampjson).
7. Delete `data/current_trade.json`.
8. Start the bot and run `audit-state` — it must report no drift.

### Phase: `Committed` / `RolledBack`

These are terminal states; the file should have been deleted. Finding one
at startup means the bot crashed between "mark committed/rolled back" and
"delete file". No recovery needed — just delete the file.

---

## Reference

- The bot itself **never** touches `data.bak.*` directories, so snapshots
  left alongside `data/` are safe.
- Every file listed here is described in
  [DATA_SCHEMA.md](DATA_SCHEMA.md).
- All writes through `fsutil::write_atomic` are durable across power
  loss; the only way to get a half-written JSON file is if someone (or
  something) writes to `data/` outside the bot.
- After any manual edit to `data/pairs/` or `data/storage/`, run the CLI
  `audit-state` with `repair: false` first to see the drift, then with
  `repair: true` only if the suggested repair is what you want.

---

## Known issues (non-actionable warnings)

These show up in `data/logs/store.log` and are **handled** — no operator
action needed. Listed so you recognize them.

- **Packet decode errors** (e.g. `set_equipment ... Unexpected enum variant`).
  Protocol drift between the server build and Azalea's decoder. The bot
  reconnects with exponential backoff.
- **Duplicate-login disconnect**. The same Microsoft account logged in
  somewhere else. The bot reconnects automatically; if it keeps happening,
  log out from other clients.
- **"Global logger already set"** on reinit. The tracing bootstrap is
  idempotent — this is swallowed silently.

If any of these stop self-healing (bot stays offline for more than a few
minutes), the reconnect backoff itself has probably given up; restart via
CLI "Restart Bot" or restart the process.

---

## 5. Bot connection problems

**"Failed to connect"**. Check `account_email` and `server_address` in
`data/config.json`. Re-run with `cargo run -- --dry-run` to validate the
file without attempting login.

**"Duplicate login"**. The account is active elsewhere. Log out of all
other clients (including the Minecraft launcher if it's auto-joining) and
restart the bot.

**"Protocol decode errors" that don't self-heal**. The server is running a
Minecraft version that Azalea can't talk to. Either downgrade the server or
wait for an Azalea update — nothing configuration can fix from this side.

---

## 6. Storage drift and missing stock

**"Chest not found"**. The physical chest doesn't exist where the model
says it should. Verify the node was built correctly before adding it via
CLI option 5 (validated add) instead of option 4 (unvalidated).

**"Storage mismatch" / audit-state reports drift**. Pair stock disagrees
with chest sum. Causes: items moved manually in-world, a crashed chest op
(see section 2), or a legitimate bug. Fix: CLI option 13 "Repair state"
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

- **"Trade timeout"**. Player didn't accept the trade request within 30 s.
  The order is cancelled; player can re-queue. Nothing to fix server-side.
- **"Trade closed before items could be validated"**. Player cancelled
  immediately. Safe — no items or currency exchanged.
- **"Trade cancelled by player before completion"**. Player cancelled after
  validation. Same as above — safe.
- **"Trade validation failed"**. Items in the GUI don't match expected
  (wrong item, wrong count, or extras). Bot aborts and notifies the player.
  No recovery needed.
- **"Inventory full"**. Bot's inventory has filled up unexpectedly. It
  should self-manage via the hotbar-to-inventory sweep after each trade,
  and via `buffer_chest_position` if configured. If it persists, stop the
  bot and clear the inventory manually — see section 3.

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

Player-facing messages that are sometimes misread as errors.

- **"Please wait X seconds before sending another message"** — player
  messaging too fast. Backoff doubles (2 s → 4 s → 8 s → … → 60 s) on each
  violation, resets after 30 s idle. Not an operator concern unless a
  player is claiming the cooldowns are wrong.
- **"Queue full. You have 8 pending orders"** — per-user cap. Wait for
  orders to drain.
- **"Order #X not found in queue"** — the order already processed or was
  cancelled. Player can run `queue` to see current ids.
- **"You can only cancel your own orders"** — players can only cancel
  their own queue entries. Operator intervention not possible via this
  path; edit `data/queue.json` by hand if needed, then restart.
- **Order still pending long after queuing** — FIFO processing; check
  total queue depth with `queue`.
- **Orders resume after restart** — the queue is persistent
  ([DATA_SCHEMA.md](DATA_SCHEMA.md#dataqueuejson)), so a restart doesn't
  drop pending work.

---

## 10. Performance tuning

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
