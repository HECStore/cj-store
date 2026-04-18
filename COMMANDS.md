# cj-store — Commands

Reference for every command the bot accepts. For *how* these are parsed
and dispatched see [ARCHITECTURE.md § Command dispatch pipeline](ARCHITECTURE.md#command-dispatch-pipeline);
for *where* in the tree see [src/store/command.rs](src/store/command.rs)
(parsing) and [src/store/handlers/](src/store/handlers/) (dispatch).

Operator status is toggled via [CLI menu](#cli-menu-operator-interface)
option 3.

## Player commands (all users)

All commands arrive as `/msg <bot> <command>`. Items are normalized
(the `minecraft:` prefix is stripped).

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

Semantics:

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
- **deposit / d [amount]** — queued; cap 768 (12 stacks). No amount →
  credits actual diamonds offered.
- **withdraw / w [amount]** — queued; cap 768; requires ≥1 whole diamond.
  No amount → withdraws full whole-diamond balance (fractional stays).
- **items / queue / cancel / status / help** — inline quick commands.
  `cancel` only works on *pending* orders; an order that has already
  started processing cannot be cancelled. `status` shows one of:
  `Idle. No orders being processed. Queue is empty.` / `Buying cobblestone
  x64. 3 order(s) waiting in queue.` / `Processing deposit (128.00
  diamonds).` — never reveals coordinates.

## Operator commands (require operator status)

Set via CLI menu option 3.

| Command          | Alias | Usage                    | Description                        |
| ---------------- | ----- | ------------------------ | ---------------------------------- |
| `additem`        | `ai`  | `additem <item> <qty>`   | Deposit stock via `/trade`         |
| `removeitem`     | `ri`  | `removeitem <item> <qty>`| Withdraw stock via `/trade`        |
| `addcurrency`    | `ac`  | `addcurrency <item> <amt>` | Add diamonds to pair reserve     |
| `removecurrency` | `rc`  | `removecurrency <item> <amt>` | Remove diamonds from reserve  |

## CLI menu (operator interface)

Blocking dialoguer menu in [src/cli.rs](src/cli.rs). All prompts go through
`with_retry` so a transient terminal-I/O error (e.g. EINTR on resize) is
retried rather than killing the CLI.

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
11. **View recent trades** — trade history (default last 20). Shows
    timestamp, type, amount, item, currency, user UUID per trade.
12. **Audit state** — check invariants, report drift without fixing.
13. **Repair state** — audit + fix safe drift (recomputes `pair.item_stock`).
14. **Restart Bot** — `BotInstruction::Restart`; disconnect + reconnect.
15. **Exit** — graceful shutdown (≈ 5–6 s; see
    [ARCHITECTURE.md § Shutdown sequence](ARCHITECTURE.md#shutdown-sequence)).
