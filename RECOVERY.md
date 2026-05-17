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

**Trade phase ladder.** `TradeState` advances strictly forward. The
common path is `Queued → Withdrawing → Trading → Depositing → Committed`,
but **buy** orders always go `Trading → Committed` directly, skipping
`Depositing` — there is nothing to put back into chests because the bot
only *receives* diamonds in a buy. **Sell** orders always traverse
`Depositing` so the bot can return the player's items to storage. Any
non-terminal phase can transition to `RolledBack` on failure. See
[src/store/trade_state.rs](src/store/trade_state.rs) for the source-of-truth
state machine.

```
Queued ─► Withdrawing ─► Trading ─► Depositing ─► Committed
                            │                          ▲
                            └──────────────────────────┘   (buys skip Depositing)

(any non-terminal state: Queued, Withdrawing, Trading, Depositing) ─► RolledBack
```

---

## 1. Corrupted `data/pairs/<item>.json`

**Symptoms**

- A specific pair disappears from the `pairs` CLI menu and the Store log
  contains one of:
  - `[Pair] quarantining <path> (malformed: …)` /  `(unreadable: …)` /
    `(duplicate key '…' already loaded)` /
    `(stem mismatch: file stem "…" vs expected "…" from item "…")` —
    `Pair::load_all` could not deserialize the file, saw two files
    mapping to the same item, or saw a file whose embedded `item` field
    didn't match its filename stem (e.g. `diamond.json` carrying
    `"item": "cobblestone"`), and renamed the bad file to
    `data/pairs/<item>.json.corrupt.<millis>` so the next `save_all`
    orphan-cleanup pass cannot delete it and so subsequent `load_all`
    calls skip it. If the quarantine rename itself fails (rare) the
    entry is simply skipped this load cycle; you'll see a follow-up
    `[Pair] quarantine rename failed for … (<reason>): …; skipping insert`
    line.
  - `Skipping pair with empty item name …` / `Skipping pair with invalid
    item name '…' (normalized to empty)` — the file deserialized, but
    `Store::new`'s post-load normalization rejected its `item` field; the
    pair is dropped from the in-memory map and the file is then deleted
    by the next `save_all` orphan-cleanup pass.

  Startup completes — neither path blocks the bot.
- AMM prices are suddenly absurd for one pair (see
  `MIN_RESERVE_FOR_PRICE` in [src/constants.rs](src/constants.rs)).

**Fix**

1. Stop the bot.
2. Locate the quarantined sidecar `data/pairs/<item>.json.corrupt.<millis>`
   (the rename the Store performed at load time). Open it. Expected shape
   (see [DATA_SCHEMA.md](DATA_SCHEMA.md#datapairsitemjson)):
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
4. If the sidecar is repairable, edit it and rename it back to
   `data/pairs/<item>.json`. If two `*.json.corrupt.<millis>` files exist
   for the same item (the duplicate-key path), reconcile them into one
   correct `<item>.json` first. If the sidecar is unfixable, leave or
   delete it — the pair will be gone on next startup; operators can
   recreate it via CLI menu option 8 "Add pair", which sets both stocks
   to zero. A pair with zero `item_stock` will refuse buys, and a pair
   with zero `currency_stock` will refuse sells: the AMM price formula
   has no defined value when the relevant reserve is empty, so the order
   is rejected up front with the message
   `Item 'X' is not available for trading (no stock or reserves).` (this
   exact wording is sent for both buys and sells; see
   [src/store/orders.rs](src/store/orders.rs) `validate_and_plan_buy` /
   `validate_and_plan_sell`). A separate fallback message,
   `Internal error: computed price is invalid.` (or
   `Internal error: computed payout is invalid.` for sells), fires when
   the cost/payout calculation returns a non-finite or ≤ 0 value despite
   non-zero reserves — that is *not* the zero-stock path. Either way,
   expect to use `additem` / `addcurrency` operator whispers to seed
   stock before players can trade.
5. Restart the bot. Watch the log for `[Pair] loaded N pairs (quarantined K)`.
6. In the CLI, run `audit-state` to cross-check that pair stocks match the
   chest totals.

**Why this can happen**: hand-edit typo, disk full during an atomic write
(rare — the rename step is atomic on both NTFS and POSIX), a half-synced
backup restore, two pair files on disk both deserializing to the same
`pair.item` (duplicate key — the second-loaded file is quarantined), or
a file whose `item` field was hand-edited to disagree with its filename
(stem mismatch — quarantined to keep a misnamed file from winning a
duplicate-key race against the legitimate one).

---

## 2. Stuck `data/journal.json` entry

The journal records one in-flight shulker operation at a time; it is
cleared whenever the operation finishes. A non-empty file at startup means
the previous run crashed mid chest I/O. Current behavior: the bot logs an
error-level notice and **renames the leftover journal aside** to
`data/journal.leftover-<unix-millis>-<seq>.json` so it's preserved for
operator review (rather than silently overwritten on the next persist).
If the journal file is unreadable on load, it is similarly quarantined to
`data/journal.unreadable-<unix-millis>-<seq>.json` and the bot continues with a
fresh empty journal. See
[src/store/journal.rs](src/store/journal.rs). The world state may be
inconsistent — that's what this playbook is for.

**Symptoms**

- Startup log shows `[Journal] loaded leftover entry: op_id=… type=… chest_id=… slot=… state=…` (info level), followed by `[Bot] Crash recovery: previous run left an in-flight shulker op: …` and `[Bot] Quarantined leftover journal to "data/journal.leftover-<unix-millis>-<seq>.json" — preserve for operator review` (both error level). The first line is `tracing::info!`, the latter two are `tracing::error!`, so an info-or-lower filter shows them all.
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
4. Restart the bot. It will rename the leftover journal aside to
   `data/journal.leftover-<unix-millis>-<seq>.json` (preserved for review)
   and the next trade involving that chest will do a fresh
   `apply_chest_sync`, reconciling per-slot counts from what's actually
   in-world.
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

Almost always, at startup — the bot normally self-heals by renaming any
leftover entry to a `data/journal.leftover-<unix-millis>-<seq>.json`
archive (so the original file is no longer in place to interfere with
the next run). The procedures above are for fixing the *world/ledger
drift* that the journal merely points at — deleting the journal alone
does not fix the drift. The archived `journal.leftover-*.json` files
are forensic-only and may be removed once their corresponding world
state has been reconciled.

> [!WARNING]
> **Rare archive-failed branch.** `bot/mod.rs`'s startup recovery
> (`begin_recovery` / startup handler) only calls
> `Journal::restore_leftover` when *both* `fs::rename` *and* the
> copy+remove fallback fail to move the file aside. In that exotic case
> the leftover stays at `data/journal.json`, but the in-memory entry
> is re-attached with `restored_leftover = true` so the next `begin()`
> archives the on-disk file to a
> `data/journal.begin-replaces-restored-<unix-millis>-<seq>.json`
> sibling *before* persisting the new entry. The forensic record
> therefore survives the next chest operation. Grep
> `data/logs/store.log` for `[Bot] Failed to archive leftover journal:`
> after every startup that reported a leftover entry, and also for
> `[Journal] archived restored-leftover at` to confirm the next
> `begin()` did move the file aside; if the second line is missing
> (e.g. because the bot was stopped before any new chest op ran), the
> only forensic copy is still at `data/journal.json` — snapshot it
> manually (e.g. `cp data/journal.json data/journal.json.bak.$(date -u
> +%s)`) before resuming. **The same caveat does NOT auto-recover for
> `data/current_trade.json`**: `trade_state::clear_persisted()`'s
> fallback in `store/mod.rs` (`Store::new` — the auto-archive path
> documented in §4's WARNING) falls back to delete-and-continue when
> rename + copy+remove both fail, so the file is gone with no archive.
> In that current_trade.json failure mode the **only** surviving copy
> lives in whatever `data.bak.*` snapshot was taken before the failed
> startup.

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
shulkers. Ctrl-C from the terminal skips the disconnect sequence that
returns held shulkers to chests, so if a chest op is in flight at that
moment its shulker is left in the bot's inventory or on the station —
exactly the situation this section recovers from. Always exit via the
CLI "Exit" menu.

---

## 4. Interrupted `data/current_trade.json`

Same family as section 2, but at a higher level: a trade crashed after
being popped from the queue but before reaching a terminal state.

**Symptoms**

- Startup log shows an error-level log line from `Store::new` about a
  leftover trade state file. The verbatim message is
  `Found interrupted trade on startup: {state}. The previous session crashed mid-trade - operator should inspect in-world state ...`
  (emitted via `tracing::error!`, not `warn!` — filtering by WARN will
  miss it).
- A player reports their last buy/sell "never finished" — no trade
  confirmation, no balance change, but items moved.
- While the bot is still running, the log shows an error line
  `Order processing exceeded watchdog; order is stuck.` — the outer
  `ORDER_HARD_TIMEOUT_SECS` (15 min) guard fired because inner timeouts
  never returned. The order remains marked as stuck; no automatic
  recovery happens.

> [!WARNING]
> Current behavior: `Store::new` **archives** any leftover
> `data/current_trade.json` by renaming it aside to a timestamped sibling
> `data/current_trade.leftover-<unix-millis>-<seq>.json` on the next startup
> (mirroring the `data/journal.leftover-*` pattern from §2). The original
> path is then free for the next trade. Operators should consult that
> archived sibling — not `data/current_trade.json` itself, which will be
> empty/absent post-restart — when reconstructing what crashed. The
> archive is forensic-only and may be deleted once the affected trade has
> been reconciled. If both the rename and a copy+remove fallback fail
> (rare; e.g. another process holds a handle and the disk is full), the
> bot logs the error and falls back to deleting the file so startup is
> not blocked — only in that exotic failure mode is the file gone with no
> archive, and operators should recover it from a `data.bak.*` snapshot.

> [!TIP]
> If the *only* symptom is that the queue has stopped advancing (no
> physical/ledger drift suspected), CLI menu option 15 **"Clear stuck
> order"** is the shortest path: it releases `processing_order` and
> returns the blocked queue entry, no JSON editing needed. Use the
> per-phase procedure below only when items, balances, or reserves
> need manual reconciliation.

<a id="commit-math"></a>
**Commit math.** Every committed order mutates pair reserves and user
balance deterministically. Use this table to reconstruct what a crashed
commit *would* have done; each phase-subsection below refers back to it.

| Order type          | `pair.item_stock` | `pair.currency_stock`                                                                   | `user.balance`     |
| ------------------- | ----------------- | --------------------------------------------------------------------------------------- | ------------------ |
| `Buy` (qty, cost)   | `− qty`           | `+ physical_diamonds_deposited + balance_deduction` (≤ cost; see note ‡)                | unchanged†         |
| `Sell` (qty, payout)| `+ qty`           | `− payout`                                                                              | `+ fractional`†    |
| `DepositBalance`    | unchanged         | unchanged                                                                               | `+ amount`         |
| `WithdrawBalance`   | unchanged         | unchanged                                                                               | `− amount`         |

† Buy may debit balance if the player paid via balance; sell pays whole
diamonds via trade and credits only the fractional remainder. See
[src/store/orders.rs](src/store/orders.rs) `execute_queued_order` for the
authoritative math.

‡ For a buy, `currency_stock` grows only by diamonds the bot actually
got into storage plus the balance the player paid from — strictly less
than `cost` whenever the post-trade diamond-deposit step partially
fails (the bot won't credit reserves for diamonds it still physically
holds). The actual physical figure used at commit time is logged on
the `phase = "buy.diamond_deposit"` line as `items_returned`; consult
that value (not `cost`) when reconstructing what the crashed commit
*would* have done. Only when every received diamond reaches storage
does the row collapse to `+ cost`.

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
2. Read/copy `data/current_trade.json` aside if you want a working copy
   under a stable name (the §2 restart will archive it aside to
   `data/current_trade.leftover-<unix-millis>-<seq>.json` via `Store::new`'s
   auto-archive — see the WARNING block above).
3. Branch on whether the journal also has a leftover entry:
   - **If the journal has a leftover entry** (you'll see the §2
     `[Journal] loaded leftover entry: …` startup line — i.e. the crash
     happened *inside* a chest op): follow
     [§ 2 Option A steps 2–4](#2-stuck-datajournaljson-entry) against
     that entry (reseat the shulker named by the journal, reconcile via
     `audit-state`).
   - **Otherwise** (no journal entry — the crash happened *between* two
     chest ops in the withdrawal plan, so the journal had already been
     cleared by the previous successful op): the journal can't tell you
     what to fix, but the saved copy of `data/current_trade.json` can.
     The `Withdrawing` variant carries a `plan: Vec<ChestTransfer>` (the
     list of chest→shulker transfers the bot was about to execute —
     see [src/store/trade_state.rs](src/store/trade_state.rs)). **Note:
     for sell orders the plan is always `[]`** because sell's diamond
     withdrawal is computed inline at runtime rather than stored here.
     For buy orders the plan is the full item-withdrawal list. Walk each
     entry in `plan`, decode its `chest_id`/`slot_index` via the
     [Terminology & decoding](#terminology--decoding) table, and
     physically reseat any shulker from those slots that's now on the
     station, in the bot's inventory, or on the floor — putting it back
     into the slot the plan entry names. For sell orders with a journal
     entry, that entry names the specific diamond-chest slot to reseat.
4. Restart the bot and run `audit-state` from the CLI to verify pair
   `item_stock` matches chest sums; if it doesn't, drop down to
   [§ 2 Option B](#2-stuck-datajournaljson-entry) for the affected pair.
5. Confirm `data/current_trade.json` is absent at the active path — the
   restart already archived it aside to
   `data/current_trade.leftover-<unix-millis>-<seq>.json`; if it still exists at
   the active path because you skipped the restart, delete it now. The
   order is cancelled.
6. Inform the player no trade happened; no balance change needed.

### Phase: `Trading`

The trade GUI was open with the player when the bot crashed. You can
almost always reconstruct whether the trade confirmed by looking at the
bot's inventory — reach for player reports only if that's ambiguous.

1. Stop the bot.
2. Read/copy `data/current_trade.json` aside if you want a working copy
   under a stable name (any §2 restart invoked below will archive it
   aside to `data/current_trade.leftover-<unix-millis>-<seq>.json` via
   `Store::new`'s auto-archive — see the WARNING block at the top of §4).
3. **Physical inventory check first.** Look at the bot's inventory and
   the buffer chest (if configured). The "bot offers" half of the trade
   either:
   - is still in the bot's inventory → the trade **never confirmed**.
     Treat as cancelled. Section 3 applies; put the shulker back into
     its chest slot.
   - is missing (and the player, if online, now has those items) → the
     trade **confirmed** before the crash. Treat as committed: apply the
     [Commit math table](#commit-math) for the
     order's type. (Note: storage counts were *not* synced back after the
     crash, so run `audit-state` after restart.)

   **Buy sub-branch.** Buys are different from sells in that, *after* the
   GUI closes but *before* the trade reaches `Committed`, the bot also
   deposits the diamonds it just received back into chest 0 (the diamond
   chest) — see `handle_buy_order` post-trade `rollback_amount_to_storage`
   for the `"[Buy] diamond-deposit"` step. So a `Trading`-phase crash on
   a buy can leave the diamonds *still in the bot's inventory* even
   though the GUI exchange completed. Procedure: first decode any leftover
   `data/journal.json` entry on `chest_id == 0` (the diamond chest),
   reseat any loose diamond shulker back into its slot, and *only then*
   classify the trade as cancelled vs. committed by the bot-inventory
   check above. Otherwise you'd see the bot's diamonds, conclude "trade
   never confirmed", and miss that the items half already moved.

   **Sell sub-branch.** On a sell, "bot offers" are diamonds the bot
   withdrew from chest 0 *before* opening the GUI (see `handle_sell_order`
   in src/store/orders.rs for the pre-trade withdrawal). A `Trading`-phase
   crash therefore means diamonds are sitting in the bot's inventory
   while chest 0 storage has already been debited. Procedure: (1) reseat
   any loose diamond shulker named by a leftover journal entry on
   `chest_id == 0` first — the journal points at the exact slot to
   restore — and (2) for committed sells, run `audit-state` after the
   restart to confirm `pair.currency_stock` matches storage. Skipping
   step 1 would leave the diamonds in the bot's inventory permanently.
4. **Only if step 3 is ambiguous**, contact the affected player. If they
   say the trade went through, treat as committed; otherwise treat as
   cancelled. Server logs can corroborate either way.
5. Confirm `data/current_trade.json` is absent at the active path — any
   §2 restart already archived it aside to
   `data/current_trade.leftover-<unix-millis>-<seq>.json`; if it still exists
   at the active path because you skipped the restart, delete it now.

### Phase: `Depositing`

The GUI completed; the bot was putting received items back into storage
when it crashed. This is the most common crash point because it involves
multiple chest ops.

1. Stop the bot.
2. Read/copy `data/current_trade.json` aside BEFORE any restart if you
   want a working copy under a stable name (any §2 restart invoked
   below will archive it aside to
   `data/current_trade.leftover-<unix-millis>-<seq>.json` via `Store::new`'s
   auto-archive — see the WARNING block at the top of §4).
3. Read `trade_result.items_received` from the copy of
   `current_trade.json` — that is what the player actually sent. Compare
   against `deposit_plan` (what the bot intended to deposit).
4. Go into the world, find any shulkers on the station / in the bot /
   on the floor near the destination chest, and put them in the chest
   slot named by the relevant plan entry.
5. Manually mirror the commit using the [Commit math table](#commit-math)
   at the top of this section — apply the row for this order's type to
   `data/pairs/<item>.json` and `data/users/<uuid>.json`.
6. Append a manual entry to `data/trades/<now>.json` matching the
   completed trade so the audit log isn't missing it. Shape is in
   [DATA_SCHEMA.md](DATA_SCHEMA.md#datatradestimestampjson).
7. Confirm `data/current_trade.json` is absent at the active path — any
   §2 restart already archived it aside to
   `data/current_trade.leftover-<unix-millis>-<seq>.json`; if it still exists
   at the active path because you skipped the restart, delete it now.
8. Start the bot and run `audit-state` — it must report no drift.

### Phase: `Committed` / `RolledBack`

These are terminal states. The file should already be deleted; finding
one at startup means the bot crashed *after* marking the trade terminal
but *before* removing the file. The ledger mutation itself already
happened (or was already rolled back — the state name tells you which),
so usually nothing is missing.

Don't just delete the file blindly, though:

1. Stop the bot.
2. Read/copy `data/current_trade.json` aside BEFORE any restart if you
   want a working copy under a stable name (any §2 or §Depositing
   restart invoked below will archive it aside to
   `data/current_trade.leftover-<unix-millis>-<seq>.json` via `Store::new`'s
   auto-archive — see the WARNING block at the top of §4).
3. Open the copy of `data/current_trade.json`. Note the `order` body
   (item, quantity, user, `order_type`).
4. For `Committed`: branch on `order.order_type`, because buys and sells
   take different paths to this terminal state:
   - **`Buy` orders.** Items already changed hands during the `Trading`
     GUI exchange (the player handed over diamonds, the bot handed over
     items). There is no formal `Depositing` phase for buys (buys go
     straight `Trading → Committed`; see the trade phase ladder above)
     — but **between GUI close and `commit()`**, `handle_buy_order` does
     still deposit the received diamonds back into chest 0 (the diamond
     chest) via `rollback_amount_to_storage("diamond", …)` so they
     physically back the post-commit `currency_stock` bump. So before
     concluding the buy is fully reconciled: (1) check whether a
     leftover journal entry on `chest_id == 0` exists from the diamond
     deposit (see the [Phase: Trading](#phase-trading) Buy sub-branch
     for the same decoding) and reseat any loose diamonds, then (2)
     reconcile `pair.item_stock`, `pair.currency_stock`, and
     `user.balance` against the [Commit math table](#commit-math). Use
     the `phase = "buy.diamond_deposit"` log line's `items_returned`
     field for the actual `physical_diamonds_deposited` figure that
     went into the row formula. **Do not** otherwise follow the full
     `Depositing` procedure — there is no multi-chest item deposit plan
     for a buy to walk.
   - **`Sell` orders.** Sells always traverse `Depositing` before
     `Committed` (the bot must put the items the player sold into
     storage chests). Verify the [Commit math table](#commit-math) row
     for `Sell`; if reconciliation is needed (counts disagree, audit
     reports drift), follow the [Phase: `Depositing`](#phase-depositing)
     procedure to find any stray shulkers and mirror the commit.
5. For `RolledBack`: verify the ledger is *unchanged* (no pair or
   balance update for this order), and that no stray shulker is in the
   bot's inventory or on the station (section 3 if there is).
6. Confirm `data/current_trade.json` is absent at the active path — any
   §2 or §Depositing restart already archived it aside to
   `data/current_trade.leftover-<unix-millis>-<seq>.json`; if it still exists
   at the active path because you skipped the restart, delete it now.

---

## 4a. Corrupted `data/queue.json`

**Symptoms**

- On startup the Store logs **two** error-level lines, in this order.
  The queue-side line fires FIRST (during `OrderQueue::load_from` in
  [src/store/queue.rs](src/store/queue.rs)) and carries the more useful
  detail — the sidecar path the bad bytes were moved to:
  `[Queue] PENDING ORDERS LOST: corrupt queue file {path:?} moved to {sidecar:?}; parse error: {e}`.
  The Store-level line follows from `Store::new` in
  [src/store/mod.rs](src/store/mod.rs) once the queue load returns the
  error: `PENDING ORDERS LOST: failed to load order queue, starting
  fresh: {e}`. Grep for either string — but if you only have the second
  one, the sidecar path you actually need is in the first.
- A file named `data/queue.json.corrupt-<unix_ms>-<seq>.json` appears
  next to the now-empty `data/queue.json`. `<unix_ms>` is the
  millisecond-resolution Unix epoch when the quarantine ran and `<seq>`
  is a per-process atomic counter so back-to-back loads (rare, but
  possible across rapid restarts or tests) don't clobber each other.
  On disk you'll see something like
  `data/queue.json.corrupt-1714402245123-0.json`. If the queue file
  failed to *read* (permissions, transient I/O error) rather than parse,
  the same path scheme is used with the `unreadable` kind tag —
  `data/queue.json.unreadable-<unix_ms>-<seq>.json`.
- Players whose orders were queued before the restart see no evidence of
  them; their queue positions are gone.

**Fix**

1. In the common case, `OrderQueue::load_from` already moved the bad
   bytes out of the way into
   `data/queue.json.{corrupt,unreadable}-<unix_ms>-<seq>.json` so they
   are not overwritten by the next save — open that sidecar to see what
   was queued. **Rare branch:** if the quarantine itself failed
   (permissions, the disk filled up between the parse error and the
   rename, a held handle that also blocks copy+remove, etc.), no
   sidecar is created and the queue-side log line is the alternate form
   `[Queue] PENDING ORDERS LOST: corrupt queue file {path:?}; parse error: {e}; quarantine also failed: {quarantine_err}`
   (or, for the read-failure path, the `[Queue] could not quarantine
   unreadable queue file ...` warn followed by the original read error
   propagated to the caller). In that case the bad bytes are still
   sitting at `data/queue.json` — the next queue mutation (any
   add/pop/cancel) will rewrite `data/queue.json` and overwrite the bad
   bytes — move them aside before restarting if you want to keep them
   (e.g. `mv data/queue.json data/queue.json.bad` on POSIX, or
   `Move-Item data/queue.json data/queue.json.bad` in PowerShell).
   Operators should grep for **both** queue-side messages, not just the
   moved-to-sidecar form.
2. If the JSON is recoverable by eye (e.g. a trailing comma, a truncated
   last entry), repair it and rename it back to `data/queue.json` **while
   the bot is stopped**. On restart the queue is loaded as normal.
3. If it is not recoverable, leave the sidecar in place as evidence and
   whisper the affected players asking them to re-submit. Keep the
   sidecar at least until every caller has re-queued; it is the only
   surviving record of the lost IDs.
4. Do not edit `data/queue.json` while the bot is running — writes are
   atomic, but a concurrent hand-edit will race the next queue mutation
   (add/pop/cancel/clear-stuck-order).

**Why this can happen**: hand-edit typo, disk full during an atomic
write (rare), or a half-synced backup restore that left the queue file
mismatched with `data/trades/` and `data/users/`.

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
- Mojang API unreachable / unknown username — both successful and
  failing resolutions are cached. Successful lookups live for
  `UUID_CACHE_TTL_SECS` (5 min default). `NotFound` results are
  negatively cached for `UUID_NEG_CACHE_TTL_SECS` (30 s default) so a
  chat-spam loop of "Player 'X' not found" does not fan out to one
  Mojang round-trip per command. A `RateLimited` response stores a typed
  cooldown entry whose `until` instant short-circuits subsequent calls
  with `MojangResolveError::RateLimited` until the cooldown expires; the
  typed error is preserved through `StoreError::MojangRateLimited` so
  callers see the cooldown rather than a generic network error. Single-
  flight coalescing in `mojang.rs::resolve_user_uuid` further collapses
  concurrent lookups for the same lowercased username into one round-trip.

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
