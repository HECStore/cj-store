# cj-store — Commands

Reference for every command the bot accepts. For *how* these are parsed
and dispatched see [ARCHITECTURE.md § Command dispatch pipeline](ARCHITECTURE.md#command-dispatch-pipeline);
for *where* in the tree see [src/store/command.rs](src/store/command.rs)
(parsing) and [src/store/handlers/](src/store/handlers/) (dispatch).

Operator status is toggled via [CLI menu](#cli-menu-operator-interface)
option 3.

## Player commands (all users)

All commands arrive as `/msg <bot> <command>`. Items are normalized
(the `minecraft:` prefix is stripped). Every command name accepts the
short alias shown in the table — `/msg <bot> b cobblestone 64` is
equivalent to `/msg <bot> buy cobblestone 64`.

| Command   | Alias | Usage                        | Description                                        |
| --------- | ----- | ---------------------------- | -------------------------------------------------- |
| `buy`     | `b`   | `buy <item> <qty>`           | Buy items from the store                           |
| `sell`    | `s`   | `sell <item> <qty>`          | Sell items to the store                            |
| `price`   | `p`   | `price <item> [qty]`         | Check buy/sell prices                              |
| `balance` | `bal` | `balance [player]`           | Check diamond balance                              |
| `pay`     | —     | `pay <player> <amount>`      | Transfer diamonds to another player                |
| `deposit` | `d`   | `deposit [amount]`           | Deposit physical diamonds to balance               |
| `withdraw`| `w`   | `withdraw [amount]`          | Withdraw balance to physical diamonds              |
| `items`   | —     | `items [page]`               | List tradeable items (4 per page)                  |
| `queue`   | `q`   | `queue [page]`               | View your pending orders (4 per page)              |
| `cancel`  | `c`   | `cancel <order_id>`          | Cancel a pending order                             |
| `status`  | —     | `status`                     | Check bot status and queue                         |
| `help`    | `h`   | `help [command]`             | Show help                                          |

### Per-command detail

Each command runs in one of three modes:

- **Inline** — answered in the same Store-loop tick; no disk, no I/O, no `/trade`.
- **Queued** — persisted to `data/queue.json`; serviced FIFO; no `/trade`.
- **Transactional** — queued, then rides the full `TradeState` lifecycle
  (validate → withdraw → `/trade` → deposit → commit) with atomic rollback.

| Command | Mode | Behavior |
| ------- | ---- | -------- |
| `buy` | Transactional | Validates pair/qty/funds/stock. Payment is flexible: balance + trade diamonds in any combo; surplus diamonds are credited back to balance. |
| `sell` | Transactional | Validates reserve/space/payout. Bot offers whole diamonds only; fractional payout is credited to balance. |
| `price` | Inline | Buy and sell price for `qty` (default: one stack of the item's `stack_size`). |
| `balance` | Inline | UUID cached for 5 min. |
| `pay` | Inline | UUID-based transfer; both usernames refreshed. Payer: `Paid X diamonds to Y`; payee (if online): `You received X diamonds from Y`. |
| `deposit` | Queued | Cap = `12 × 64 = 768` (trade GUI offer slots × max stack). No `amount` → credits whatever the player offers. |
| `withdraw` | Queued | Cap = 768 (same derivation). Requires ≥1 whole diamond. No `amount` → withdraws the whole-diamond balance, capped at 768 per transaction; if the balance exceeds 768 the bot whispers an explicit cap notice so the player knows to issue `/withdraw` again for the rest. Fractional balance stays. |
| `items` / `queue` | Inline | Paginated, 4 per page. |
| `cancel` | Inline | *Pending* orders only. A processing order replies `Order #<id> is currently being processed (<phase>) and cannot be cancelled.` |
| `status` | Inline | Never reveals coordinates. Examples below. |
| `help` | Inline | Per-command or overview. |

`status` replies — every message starts with `Status:`; the `[phase]` tag is
the lowercase phase name from `TradeState::phase()`; the trailing
`N order(s) waiting in queue.` is appended only when the queue is non-empty:

| State                          | Reply                                                                                     |
| ------------------------------ | ----------------------------------------------------------------------------------------- |
| Idle, empty queue              | `Status: Idle. No orders being processed. Queue is empty.`                                |
| Queue pending, not yet running | `Status: Ready. 2 order(s) in queue, processing will start shortly.`                      |
| Withdrawing (bot fetching)     | `Status: Withdrawing for: buy cobblestone 64 [withdrawing]. 3 order(s) waiting in queue.` |
| Trading with player            | `Status: Trading with player: buy cobblestone 64 [trading].`                              |
| Depositing (post-trade)        | `Status: Depositing after: sell iron_ingot 128 [depositing].`                             |

## Operator commands (require operator status)

| Command          | Alias | Usage                    | Description                        |
| ---------------- | ----- | ------------------------ | ---------------------------------- |
| `additem`        | `ai`  | `additem <item> <qty>`   | Deposit stock via `/trade`         |
| `removeitem`     | `ri`  | `removeitem <item> <qty>`| Withdraw stock via `/trade`        |
| `addcurrency`    | `ac`  | `addcurrency <item> <amt>` | Add diamonds to pair reserve     |
| `removecurrency` | `rc`  | `removecurrency <item> <amt>` | Remove diamonds from reserve  |

`additem` and `removeitem` open a `/trade` GUI with the operator: for
`additem` the operator offers the stock items and the bot's side of the
trade is empty (the bot then deposits what it received); for `removeitem`
the roles are reversed. `addcurrency` and `removecurrency` mutate the
pair's `currency_stock` directly — these are bookkeeping-only changes
to the AMM reserve; no in-game diamonds move.

## CLI menu (operator interface)

Blocking dialoguer menu in [src/cli.rs](src/cli.rs) — 16 entries. All
prompts go through `with_retry` so a transient terminal-I/O error (e.g.
EINTR on resize) is retried rather than killing the CLI.

> Numbering below is **1-based** (how the menu renders to the operator).
> In [src/cli.rs](src/cli.rs) the items are 0-indexed, so "option 15
> Clear stuck order" is index `14` in the source.

1. **Get user balances** — list all users + balances.
2. **Get pairs** — all pairs with stock, reserve, calculated buy/sell.
3. **Set operator status** — prompt for username/UUID, toggle `operator`.
4. **Add node (no validation)** — writes model-only; operator must ensure
   the physical node exists.
5. **Add node (with bot validation)** — bot navigates, opens all 4 chests
   with fast 5 s timeout, verifies every slot holds a shulker. Fail-fast;
   typically completes in well under a minute, but allow up to 2 minutes
   on a laggy server.
6. **Discover storage (scan)** — bot starts at the next unregistered id
   and walks the spiral, adding every valid node. Stops on the first
   missing/invalid position.
7. **Remove node** — deletes `data/storage/{id}.json`. Destructive.
8. **Add pair** — prompts for item + stack size {1, 16, 64}. Stocks start
   zero; seed via `additem` / `addcurrency`.
9. **Remove pair** — warns if stock > 0. Cannot remove `diamond`.
10. **View storage** — origin, node count, per-node chest summary.
11. **View recent trades** — trade history, newest first (default last
    20; operator can type a custom count). Shows timestamp, type, amount,
    item, currency, user UUID per trade.
12. **Audit state** — check invariants, report drift without fixing.
13. **Repair state** — audit + fix safe drift (recomputes `pair.item_stock`).
14. **Restart Bot** — `BotInstruction::Restart`; disconnect + reconnect.
15. **Clear stuck order** — force-releases the Store's `processing_order`
    flag and returns the in-flight queue entry that was blocking it. Use
    after a crash mid-trade (non-empty `data/current_trade.json`) to let
    the queue resume without editing JSON by hand. Returns the cleared
    order description to the CLI. Note this only unblocks the queue — it
    does **not** reconcile physical chests or ledger state. If you
    suspect drift (a shulker left on the station, items missing, pair
    stock off), run through
    [RECOVERY.md § 4](RECOVERY.md#4-interrupted-datacurrent_tradejson)
    first.
16. **Exit** — graceful shutdown (≈ 5–6 s; see
    [ARCHITECTURE.md § Shutdown sequence](ARCHITECTURE.md#shutdown-sequence)).
    Pending queue entries in `data/queue.json` are preserved and resume
    on the next startup.
