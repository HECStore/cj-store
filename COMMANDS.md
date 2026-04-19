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

### Semantics

Every command runs in one of three modes.

- **Inline** — handled synchronously on the Store loop; the bot replies
  in the same tick. No disk writes, no chest I/O, no `/trade` GUI.
  Commands: `price`, `balance`, `pay`, `items`, `queue`, `cancel`,
  `status`, `help`.
- **Queued** — persisted to `data/queue.json` and serviced in FIFO
  order. No physical I/O required when it pops. Commands: `deposit` /
  `withdraw` that move balance without a trade-GUI exchange.
- **Transactional** — queued, and when popped rides the full
  `TradeState` lifecycle: validate → withdraw from storage → `/trade`
  GUI → deposit to storage → commit. Rolls back atomically on any
  failure. Commands: `buy`, `sell`, operator `additem`, operator
  `removeitem`.

### Quick reference

- **buy** — transactional; validates pair, quantity, funds, stock; withdraws
  from storage, trades to player, commits ledger, records Trade. Flexible
  payment: balance + trade diamonds in any combination. Surplus diamonds
  credited to balance.
- **sell** — transactional; validates reserve, space, payout; trades items
  in, deposits to storage, commits ledger, records Trade. Bot offers whole
  diamonds; fractional payout credited to balance.
- **price** — inline; shows buy and sell price for qty (defaults to one
  stack based on item's `stack_size`).
- **balance / bal [player]** — inline; UUID cached 5 min.
- **pay** — inline; validates funds; UUID-based transfer; both usernames
  updated to latest. Payer gets `Paid X diamonds to Y`; payee (if online)
  gets `You received X diamonds from Y`.
- **deposit / d [amount]** — queued; cap 768 (12 trade-GUI slots × 64-item
  max-stack — not an arbitrary limit). No amount → credits actual diamonds
  offered.
- **withdraw / w [amount]** — queued; cap 768 (same derivation); requires
  ≥1 whole diamond. No amount → withdraws full whole-diamond balance
  (fractional stays).
- **items** — inline; paginates tradeable items, 4 per page.
- **queue** — inline; shows your pending orders, 4 per page.
- **cancel** — inline; only works on *pending* orders. An order already
  being processed gets the exact reply `Order #<id> is currently being
  processed (<phase>) and cannot be cancelled.` (`<phase>` is the current
  `TradeState` phase).
- **status** — inline; shows one of: `Idle. No orders being processed.
  Queue is empty.` / `Buying cobblestone x64. 3 order(s) waiting in
  queue.` / `Processing deposit (128.00 diamonds).` — never reveals
  coordinates.
- **help** — inline; per-command help or overview.

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

1. **Get user balances** — list all users + balances.
2. **Get pairs** — all pairs with stock, reserve, calculated buy/sell.
3. **Set operator status** — prompt for username/UUID, toggle `operator`.
4. **Add node (no validation)** — writes model-only; operator must ensure
   the physical node exists.
5. **Add node (with bot validation)** — bot navigates, opens all 4 chests
   with fast 5 s timeout, verifies every slot holds a shulker. Fail-fast;
   typically completes in under 30 seconds.
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
