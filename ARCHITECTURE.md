# cj-store — Architecture

Complement to [README.md](README.md). README covers *what* the system does;
this document diagrams *how* the pieces fit together. For JSON file formats
see [DATA_SCHEMA.md](DATA_SCHEMA.md); for operational playbooks see
[RECOVERY.md](RECOVERY.md).

## Runtime topology

Three cooperating tasks, spawned once from [main.rs](src/main.rs) and joined
at shutdown. All cross-task communication goes over typed Tokio channels —
no shared mutable state, no mutexes on business data.

```text
                   ┌──────────────────────────────┐
                   │        Store task             │
                   │  (authoritative state)        │
                   │  Store::run (src/store/mod.rs)│
                   └──────┬──────────┬─────────────┘
                          │          │
           StoreMessage   │          │   BotInstruction
    ┌─────────────────────┘          └──────────────────────┐
    │                                                       │
    │                                                       ▼
┌───┴────────────┐                              ┌────────────────────────┐
│  Bot task      │                              │    (oneshot replies)   │
│  bot_task      │                              │                        │
│ src/bot/mod.rs │ ◄────── BotMessage ──────────┤                        │
└────────────────┘                              └────────────────────────┘
     ▲                                                      ▲
     │ Minecraft I/O                                        │ CliMessage
     │ (Azalea)                                             │
  ┌──┴──────────┐                                    ┌──────┴───────┐
  │  Server     │                                    │  CLI task    │
  │  (players)  │                                    │  cli_task    │
  └─────────────┘                                    │ src/cli.rs   │
                                                     └──────────────┘
```

Channels:

- `mpsc::Sender<StoreMessage>` — shared by Bot and CLI; funnels everything
  inbound to the Store. Buffered at 128; see [main.rs](src/main.rs) for why
  that number.
- `mpsc::Sender<BotInstruction>` — Store to Bot outbound. Also 128.
- `oneshot::Sender<T>` — carried inside specific message variants when a
  request needs a reply (CLI queries, trade results, validation outcomes).
- `mpsc::Sender<StoreMessage>` held by the config watcher — fires
  `StoreMessage::ReloadConfig` on `data/config.json` changes (debounced by
  `DELAY_CONFIG_DEBOUNCE_MS`, see [src/constants.rs](src/constants.rs)).

Why three tasks rather than one actor per type: [Azalea](https://github.com/azalea-rs/azalea)'s
`ClientBuilder::start` returns a `!Send` future, so the Bot task must live on a
`LocalSet`. Splitting the Store onto its own `tokio::spawn` keeps state mutation off the Bot's
event-loop thread and lets it remain `Send`. The CLI uses
`tokio::task::spawn_blocking` because `dialoguer` does synchronous terminal I/O.

## Store event loop

The Store loop has strict priority: an in-flight order is always drained to
completion before any new inbound message is picked up. See
[src/store/mod.rs](src/store/mod.rs) `Store::run`.

```text
    ┌─────────────────────────────────────────────────┐
    │  loop {                                         │
    │    if queue non-empty AND !processing_order {   │
    │      pop order -> execute_queued_order          │ ◄── blocks other msgs
    │      continue                                   │     until done
    │    }                                            │
    │    select! {                                    │
    │      msg = rx.recv()     -> dispatch(msg)       │
    │      _   = autosave_tick -> save_if_dirty()     │
    │      _   = cleanup_tick  -> prune_caches()      │
    │    }                                            │
    │  }                                              │
    └─────────────────────────────────────────────────┘
```

Tick cadence:

| Tick         | Interval                            | Effect                                    |
| ------------ | ----------------------------------- | ----------------------------------------- |
| autosave     | `autosave_interval_secs` (cfg, 2 s) | saves only when `dirty`                   |
| cleanup      | `CLEANUP_INTERVAL_SECS` (1 h)       | prunes UUID cache + stale rate-limit ents |
| post-trade   | after each commit                   | unconditional non-debounced save          |
| shutdown     | once                                | final save, then channel close            |

## Command dispatch pipeline

Player whispers become structured commands at the boundary, then stay typed
through the handler chain.

```text
  "msg HECStore buy cobblestone 64"
         │
         │ bot parses whisper prefix
         ▼
  BotMessage::PlayerCommand { player, command }
         │
         │ wrapped: StoreMessage::FromBot(..)
         ▼
  [Store::run dispatch]
         │
         │ parse_command(raw) -> Command enum
         ▼                            (src/store/command.rs)
  Command::Buy { item, quantity } ──► handlers::player::handle_buy
         │                                 │
         │                                 │ validate -> plan -> queue
         │                                 ▼
         │                          OrderQueue::add
         │                                 │
         └────── (quick-reply commands     │
                  like balance/price/items │
                  return inline)           ▼
                                   later pop -> execute_queued_order
                                              (src/store/orders.rs)
```

All handler functions return `Result<(), StoreError>` —
[src/error.rs](src/error.rs). The unification landed in Phase 2 so rollback,
messaging, and queue interactions share one error surface.

## Trade state machine

Each popped order rides the `TradeState` enum through its lifecycle. The
enum is in [src/store/trade_state.rs](src/store/trade_state.rs); each
transition function consumes the prior phase's data so invalid jumps are
unrepresentable.

```text
              ┌────────┐
              │ Queued │  (popped from OrderQueue)
              └────┬───┘
                   │  validate + plan chest transfers
                   ▼
           ┌─────────────┐
           │ Withdrawing │  bot pulls items/diamonds out of storage
           └──────┬──────┘
                  │  chest I/O ok (else -> RolledBack)
                  ▼
           ┌─────────────┐
           │   Trading   │  trade GUI opened; wait for player confirm
           └──────┬──────┘
                  │  /trade completed (or timeout -> RolledBack)
                  ▼
           ┌─────────────┐
           │ Depositing  │  bot puts received items back into storage
           └──────┬──────┘
                  │  deposit ok
                  ▼
           ┌─────────────┐          ┌──────────────┐
           │  Committed  │          │  RolledBack  │  (terminal)
           └─────────────┘          └──────────────┘
```

`TradeState` is mirrored to `data/current_trade.json` at every transition
and cleared on a terminal state. On startup the Store looks for a leftover
file: its presence means the previous run crashed mid-trade. The current
behavior is log-and-clear; automatic re-queue/rollback is Phase 3
([PLAN.md](PLAN.md)). When that lands, the startup path in `Store::new`
inspects the persisted phase and routes:

```text
  Queued        -> re-add to OrderQueue front
  Withdrawing   -> roll back any completed transfers, reject order
  Trading       -> reject order (player trade state unknown)
  Depositing    -> rolll back withdraw, reject order
  Committed     -> nothing (transient; shouldn't survive a crash here)
  RolledBack    -> nothing (already terminal)
```

See [RECOVERY.md](RECOVERY.md) for the manual playbook in the meantime.

## Operation journal (chest I/O crash recovery)

Distinct from `TradeState`: the journal tracks a single in-flight *shulker
operation* (the lower-level chest transfer that Withdrawing/Depositing use).
See [src/store/journal.rs](src/store/journal.rs).

```text
  Journal::begin(op_type, chest_id, slot)
    │
    ▼
  ShulkerTaken -> ShulkerOnStation -> ItemsTransferred
                                       │
                                       ▼
                                 ShulkerPickedUp -> ShulkerReplaced
                                                     │
                                                     ▼
                                             Journal::complete() clears
```

Every state change atomically rewrites `data/journal.json`. On startup the
Store reads any leftover entry, logs it as a diagnostic, and clears the
file. Replay of partial shulker ops is deferred to Phase 3.

## Storage physical model

Authoritative state about what is in the world. Built up from small types:

```text
  Storage (src/types/storage.rs)
    └─ Vec<Node>
         └─ 4 × Chest  (CHESTS_PER_NODE = 4)
              ├─ item: String          // or "overflow" (OVERFLOW_CHEST_ITEM)
              └─ amounts: Vec<i32>     // length = DOUBLE_CHEST_SLOTS = 54
                                       // one shulker per slot
```

Slot `n` of a chest represents the contents of the shulker box that lives
in that chest slot. `amounts[n]` is the count of the chest's single item
type inside that shulker. The bot never mixes item types in a shulker
(except the overflow chest, which is write-only from the bot's side).

Mutations land through `Storage::apply_chest_sync` — the bot walks to a
chest, opens it, reads a canonical 54-slot `amounts` vector, and ships it
back as `ChestSyncReport`. Slots reported as `-1` mean "not checked" and
keep their existing value in the Store's view; all other slots are merged
in. This means the in-game server is the source of truth for
*per-shulker* counts; the Store's view is reconciled after each visit,
not predicted.

## Shutdown sequence

1. CLI sends `CliMessage::Shutdown { respond_to }` and blocks on the
   oneshot.
2. Store forwards `BotInstruction::Shutdown { respond_to }` to Bot,
   blocks on the oneshot.
3. Bot calls `client.disconnect()`, waits up to `DELAY_DISCONNECT_MS` for
   the packet to flush, aborts the Azalea task, sleeps another
   `DELAY_DISCONNECT_MS` for TCP teardown. See
   [src/bot/connection.rs](src/bot/connection.rs).
4. Bot signals done. Store performs a final save and replies to CLI.
5. CLI drops `store_tx`; Store's receive loop ends; `try_join!` returns.

Total wall-clock ≈ 5–6 s. The doubled 2 s wait is intentional: flushing the
disconnect packet and releasing the TCP socket are independent delays and
collapsing them causes "address in use" on fast reconnect.

The Store task breaks from its loop immediately after handling the
shutdown message — it does not wait for channel closure. All state is
saved twice: once in the shutdown handler, once in final cleanup as a
safety measure.

## Source tree

```text
cj-store/
  Cargo.toml
  .cargo/config.toml            # optional fast-build flags
  src/
    main.rs                     # starts Store + Bot + CLI tasks
    store/                       # authoritative state + handlers + autosave
      mod.rs                    # Store struct, run loop, message routing
      handlers/
        mod.rs                  # handler module exports
        player.rs              # command dispatcher + rate limiting
        validation.rs           # shared input validators
        buy.rs                  # buy: validation + enqueue
        sell.rs                 # sell: validation + enqueue
        deposit.rs              # deposit: enqueue + handle_deposit_balance_queued
        withdraw.rs             # withdraw: enqueue + handle_withdraw_balance_queued
        info.rs                 # price, balance, pay, items, queue, cancel, status, help
        operator.rs            # additem, removeitem, add/remove currency
        cli.rs                 # CLI message handlers
      command.rs                # Command enum + parse_command() (typed whisper parsing)
      journal.rs                # chest I/O crash-recovery journal
      orders.rs                 # order execution (execute_queued_order, handle_buy/sell)
      pricing.rs                # constant-product AMM + proptest
      queue.rs                  # QueuedOrder, OrderQueue, persistence
      rate_limit.rs             # anti-spam with exponential backoff
      rollback.rs               # shared rollback helper
      state.rs                  # save, audit_state, assert_invariants
      trade_state.rs             # Trade lifecycle SM + crash-resume persistence
      utils.rs                  # normalize_item_id, resolve_user_uuid, UUID cache
    bot/                        # Azalea client + whisper parsing
      mod.rs                    # Bot struct, BotState, bot_task, event handlers
      connection.rs             # connect, disconnect
      navigation.rs             # pathfinding helpers
      shulker.rs               # shulker place/pickup/open, station position
      chest_io.rs               # withdraw_shulkers / deposit_shulkers dispatch
      trade.rs                  # /trade GUI automation
      inventory.rs              # ensure_inventory_empty, move_hotbar_to_inventory
    cli.rs                      # dialoguer menu → StoreMessage
    config.rs                   # data/config.json loader/creator
    constants.rs                # timeouts, retry counts, DOUBLE_CHEST_SLOTS, etc.
    error.rs                    # StoreError enum
    fsutil.rs                   # atomic file write helper (temp + rename)
    messages.rs                 # StoreMessage / BotMessage / CliMessage / BotInstruction
    types/
      item_id.rs                # normalized ItemId newtype
      user.rs                   # per-user persistence + Mojang UUID lookup
      pair.rs                   # per-item "pair" persistence (data/pairs/*.json)
      order.rs                  # global queue persistence (data/orders.json)
      trade.rs                  # per-trade persistence (data/trades/*.json)
      storage.rs                # storage graph loader/saver
      node.rs                   # node placement + per-node chest load/save
      chest.rs                  # chest schema + per-chest load/save
      position.rs               # simple x/y/z
  data/
    config.json
    logs/store.log
    journal.json                # in-flight shulker op (normally empty)
    orders.json                 # session-only audit log
    queue.json                  # persistent pending order queue
    current_trade.json          # in-flight TradeState mirror
    pairs/*.json
    users/*.json
    storage/<node_id>.json
    trades/*.json
```

## Storage physical model (detail)

The bot models a physical storage layout of chests clustered into **nodes**.
Each node is 4 double chests the bot can reach from one standing position.

### Node layout

Each node occupies a 4×3 block footprint. The bot stands at position **P**
(the node origin) facing north:

```
      West ← → East

      NCCN  ← z-2 (back of double chests)
      NCCN  ← z-1 (front of double chests, clickable face)
      XSNP  ← z   (working row)
        ↑
      North
```

| Symbol | Meaning                                                        |
| ------ | -------------------------------------------------------------- |
| `N`    | Empty space (nothing)                                          |
| `C`    | Double chest block (extends 2 blocks N-S, 2 blocks tall in Y)  |
| `P`    | **Node position** — where bot stands (southeast corner)        |
| `S`    | **Shulker station** — 2 blocks west of P                       |
| `X`    | **Pickup position** — 3 blocks west of P                       |

Chest ID layout (standing at P, looking north):

```
      Chest 0    Chest 1   ← y+1 (top row, eye level)
      Chest 2    Chest 3   ← y   (bottom row, ground level)
      ←─────────────────→
        West         East
```

All 4 chests are accessed from z-1 (their south face).

### Reserved chests (Node 0 only)

| Chest    | Purpose                 | Rules                                                   |
| -------- | ----------------------- | ------------------------------------------------------- |
| Chest 0  | **Diamonds (currency)** | Automatically assigned, cannot be changed               |
| Chest 1  | **Overflow/failsafe**   | Deposit-only; never withdraw; allows mixed items        |
| Chests 2-3 | General storage       | Available for any tradeable items                       |

### Spiral expansion

Nodes are arranged in a clockwise spiral, spaced **3 blocks apart**:

```
    z
    ↑
    │  . 6 7 8 9
    │  . 5 0 1 .
    │  . 4 3 2 .
    │  . . . . .
    └──────────→ x
```

| Node | X offset | Z offset | Description      |
| ---- | -------- | -------- | ---------------- |
| 0    | +0       | +0       | Origin           |
| 1    | +3       | +0       | East             |
| 2    | +3       | +3       | Southeast        |
| 3    | +0       | +3       | South            |
| 4    | -3       | +3       | Southwest        |
| 5    | -3       | +0       | West             |
| 6    | -3       | -3       | Northwest        |
| 7    | +0       | -3       | North            |
| 8    | +3       | -3       | Northeast        |
| 9    | +6       | -3       | Continue spiral  |

The **X** pickup position of each node coincides with the **P** position
of the node to its west, so adjacent nodes share space efficiently.

Multi-node example (nodes 0-3, top-down):

```
    CCNCCN
    CCNCCN
    SN0SN1    ← Node 0 and 1
    CCNCCN
    CCNCCN
    SN3SN2    ← Node 3 and 2
```

### Chest capacity

Each chest has 54 slots; every slot is assumed to contain exactly 1 shulker
box. `amounts[i]` is the item count **inside** the shulker in slot `i`.

| Stack size | Items per shulker | Items per chest | Examples                |
| ---------- | ----------------- | --------------- | ----------------------- |
| 64         | 27 × 64 = 1,728   | 93,312          | Cobblestone, Iron, Diamond |
| 16         | 27 × 16 = 432     | 23,328          | Ender Pearl, Egg, Sign  |
| 1          | 27 × 1 = 27       | 1,458           | Sword, Armor, Potion    |

**Rules**:

- One item type per chest (or unassigned).
- Shulker colors are treated identically.
- Chest assignment is **sticky** — a drained chest keeps its `item` until
  "Repair state" reclaims it.
- When existing chests fill, the deposit planner grabs the next empty chest
  (preferring the same node) or provokes a new node.

> [!IMPORTANT]
> The system **assumes every chest slot contains exactly 1 shulker box**.
> `amounts[i]` tracks items *inside* the shulker in slot `i`, not the
> shulker itself — the shulker is assumed to always be present. If a slot
> is empty or holds a non-shulker item, operations on that chest will
> fail.

## Order queue system

The Store uses a FIFO queue so quick commands (balance/price/help) stay
responsive even while trades execute. See [src/store/queue.rs](src/store/queue.rs).

### Order lifecycle

```text
  QUEUED
    │  player sends command
    │  validation: item, quantity, user limits
    ↓
  "Order #47 queued (position 3/5). Est. wait: ~2 min."
    │
    │  (FIFO wait)
    ↓
  PROCESSING
    │  bot prepares items, sends /trade request
    ↓
  ─┬─ Trade accepted + completed ──→ SUCCESS
   ├─ Trade timeout (30s accept, 45s complete) ──→ CANCELLED
   ├─ Player cancelled trade ──→ CANCELLED
   └─ Validation failed ──→ CANCELLED + ROLLBACK
```

### Queue limits

| Property             | Value                         | Details                                                |
| -------------------- | ----------------------------- | ------------------------------------------------------ |
| Max orders per user  | 8                             | Prevents queue monopolization                          |
| Global queue cap     | 128 (`MAX_QUEUE_SIZE`)        | Enqueue rejected on saturation                         |
| Persistence          | `data/queue.json`             | Survives restarts                                      |
| Trade accept timeout | 30 s                          | Order cancelled if player doesn't accept               |
| Trade complete timeout | `trade_timeout_ms` (45 s)   | Order cancelled if trade doesn't complete              |
| Retry on timeout     | No                            | Timed-out orders are cancelled, not retried            |

### Player feedback messages

| Event              | Example                                                                               |
| ------------------ | ------------------------------------------------------------------------------------- |
| Order queued       | `Order #12 queued (position 2/3). Est. wait: ~1 min. You have 1 order(s) pending.`    |
| Processing starts  | `Now processing: buy 64 cobblestone...`                                               |
| Pre-trade (buy)    | `Buy 64 cobblestone: Total 10.50 diamonds. Please offer 11 diamonds in the trade.`    |
| Pre-trade (sell)   | `Sell 64 cobblestone: You'll receive 8 diamonds in trade + 0.50 to balance (total 8.50).` |
| Trade complete     | `Bought 64 cobblestone for 10.50 diamonds (fee 1.17). Trade complete.`                |
| Trade timeout      | `Trade timed out. Order cancelled.`                                                   |
| Queue full         | `Queue full. You have 8 pending orders (max 8). Wait for some to complete.`           |

## Rate limiting (anti-spam)

See [src/store/rate_limit.rs](src/store/rate_limit.rs). State is in-memory;
resets on bot restart.

- **2 s minimum** between commands from the same player.
- **Exponential backoff** on violations: 2 s → 4 s → 8 s → 16 s → 32 s, capped
  at 60 s.
- **Reset after 30 s idle** — violation count returns to 0.
- Periodic cleanup (`CLEANUP_INTERVAL_SECS`, 1 h) prunes stale per-user state.

Example:

```
Player: buy cobblestone 64      [allowed]
Player: buy iron_ingot 32       [too fast - within 2s]
Bot: Please wait 1.5s before sending another message.
Player: buy iron_ingot 32       [still too fast]
Bot: Please wait 3.8s before sending another message.  [doubled wait]
[Player waits]
Player: buy iron_ingot 32       [allowed after waiting]
```

## Trade protocol (`/trade`)

The only mechanism for moving items between bot and player. The bot can
trade at most **12 stacks per transaction** (768 items at stack-64). Full
shulker boxes cannot be traded — only loose items.

### Trade lifecycle

```text
  1. Bot whispers player with trade details
  2. Bot sends: /trade <username>
  3. ─┬─ Player accepts (within 30s) ──→ GUI opens
     ├─ Player declines ──→ Trade aborted
     └─ Timeout (30s) ──→ Trade aborted
  4. Player adds items to their offer slots
  5. Player clicks confirm (indicators turn lime)
  6. Bot validates: correct items + counts + lime indicators
  7. ─┬─ Validation passes ──→ Bot clicks accept ──→ Trade completes
     └─ Validation fails ──→ Trade aborted
  8. ─┬─ Completed within 45s ──→ Items exchanged ──→ Success
     └─ Timeout (45s) ──→ Trade cancelled
```

### Timeouts

| Phase                    | Timeout                       | On timeout                                      |
| ------------------------ | ----------------------------- | ----------------------------------------------- |
| Trade request acceptance | 30 s                          | Order cancelled, player notified                |
| Trade completion         | `trade_timeout_ms` (45 s)     | Trade cancelled, rollback attempted             |
| Pathfinding              | `pathfinding_timeout_ms` (60 s) | Navigation aborted, current action fails      |

### Trade GUI layout (9×6, 54 slots)

```
     Col:  0   1   2   3   4   5   6   7   8
         ┌───┬───┬───┬───┬───┬───┬───┬───┬───┐
  Row 0  │ B │ B │ B │ B │ ║ │ P │ P │ P │ P │  ← Offer slots
         ├───┼───┼───┼───┼───┼───┼───┼───┼───┤
  Row 1  │ B │ B │ B │ B │ ║ │ P │ P │ P │ P │
         ├───┼───┼───┼───┼───┼───┼───┼───┼───┤
  Row 2  │ B │ B │ B │ B │ ║ │ P │ P │ P │ P │
         ├───┼───┼───┼───┼───┼───┼───┼───┼───┤
  Row 3  │   │   │   │   │ ║ │   │   │   │   │  ← Empty row
         ├───┼───┼───┼───┼───┼───┼───┼───┼───┤
  Row 4  │ ✓ │ ✓ │ ✗ │ ✗ │ ║ │ ● │ ● │ ● │ ● │  ← Status/buttons
         ├───┼───┼───┼───┼───┼───┼───┼───┼───┤
  Row 5  │ ✓ │ ✓ │ ✗ │ ✗ │ ║ │ ● │ ● │ ● │ ● │
         └───┴───┴───┴───┴───┴───┴───┴───┴───┘

  B = Bot offer slot (12)     ║ = Separator (iron bars, col 4)
  P = Player offer slot (12)  ✓ = Accept (lime wool, bot)
  ● = Player status (dyes)    ✗ = Cancel (red wool, bot)
```

### Slot mapping

| Area           | Rows | Columns | Slot numbers                   |
| -------------- | ---- | ------- | ------------------------------ |
| Bot offer      | 0-2  | 0-3     | 0-3, 9-12, 18-21               |
| Player offer   | 0-2  | 5-8     | 5-8, 14-17, 23-26              |
| Bot accept     | 4-5  | 0-1     | 36-37, 45-46 (lime wool)       |
| Bot cancel     | 4-5  | 2-3     | 38-39, 47-48 (red wool)        |
| Player status  | 4-5  | 5-8     | 41-44, 50-53 (dyes)            |
| Separator      | all  | 4       | iron bars, non-interactable    |

Slot formula: `slot = row × 9 + column`.

Player status dye meanings:

| Dye            | State     | Meaning                         |
| -------------- | --------- | ------------------------------- |
| `gray_dye`     | Default   | Player hasn't interacted yet    |
| `magenta_dye` | Waiting   | Player reviewing/modifying      |
| `lime_dye`     | Confirmed | Player accepted the trade       |

The bot only clicks accept when all player indicators show `lime_dye`.

### Safety validations

Before accepting any trade, the bot verifies:

1. Item types match using normalized item IDs.
2. Item counts are exact (not less, not more).
3. No unexpected items present in the offer.
4. All player indicators show `lime_dye`.

Failure at any step aborts the trade and notifies the player.

### Withdrawal flow (buy orders)

```
1. Navigate to node position P
2. Open chest containing the needed shulker
3. Take shulker from chest → hotbar slot 0
4. Place shulker on station (S, 2 blocks west)
5. Open shulker and transfer items to inventory (9-35)
6. Break shulker (drops as item)
7. Walk to pickup position (X, 3 blocks west of P)
8. Pick up dropped shulker
9. Return to P and put shulker back in its chest slot
10. Repeat if more items needed
```

### Deposit flow (sell orders)

```
1. Navigate to node position P
2. Find chest for this item (or empty chest to assign)
3. Take shulker from chest → hotbar slot 0
4. Place shulker on station (S)
5. Open shulker and transfer items from inventory
6. If shulker full: put back, get next shulker
7. Break shulker and pick up at X
8. Put shulker back in its chest slot
9. Repeat if more items to deposit
```

### Inventory slot rules

| Slots | Purpose              | Notes                                                 |
| ----- | -------------------- | ----------------------------------------------------- |
| 0-8   | Not used             | Crafting slots (inaccessible)                         |
| 9-35  | Main inventory       | Items go here during operations                       |
| 36    | Reserved for shulker | Hotbar slot 0, always kept clear                      |
| 37-44 | General hotbar       | Cleared to inventory (9-35) after each trade          |

### Pricing (constant-product AMM)

Formulas in [src/store/pricing.rs](src/store/pricing.rs); see
[Uniswap V2 protocol overview](https://docs.uniswap.org/contracts/v2/concepts/protocol-overview/how-uniswap-works)
for the mathematical background. `k = item_stock × currency_stock`:

- **Buy**: `cost = currency_stock × quantity / (item_stock - quantity) × (1 + fee)`
- **Sell**: `payout = currency_stock × quantity / (item_stock + quantity) × (1 - fee)`

Properties:

| Property            | Description                                                  |
| ------------------- | ------------------------------------------------------------ |
| Pool protection     | Cost → ∞ as quantity → item_stock (can never drain pool)     |
| k only increases    | Each trade adds fees to the pool, growing k over time        |
| Self-balancing      | High demand → higher price → incentivizes sells              |
| No admin pricing    | Fully algorithmic based on supply/demand                     |

Edge cases rejected:

- `item_stock == 0` or `currency_stock == 0` — `None`; trading disabled.
- `quantity >= item_stock` — buy rejected.
- Non-finite or non-positive computed cost — "Internal error".

Example — buy into a pool of 100 items, 1000 diamonds, fee 12.5 % (k =
100,000):

| Quantity | Formula                         | Cost        | Per-item  | Slippage   |
| -------- | ------------------------------- | ----------- | --------- | ---------- |
| 1        | 1000 × 1 / (100-1) × 1.125      | 11.36       | 11.36     | baseline   |
| 10       | 1000 × 10 / (100-10) × 1.125    | 125.00      | 12.50     | +10 %      |
| 25       | 1000 × 25 / (100-25) × 1.125    | 375.00      | 15.00     | +32 %      |
| 50       | 1000 × 50 / (100-50) × 1.125    | 1,125.00    | 22.50     | +98 %      |
| 90       | 1000 × 90 / (100-90) × 1.125    | 10,125.00   | 112.50    | +890 %     |
| 99       | 1000 × 99 / (100-99) × 1.125    | 111,375.00  | 1,125.00  | +9,807 %   |

Sell payout at the same reserves (fee 12.5 %):

| Quantity | Formula                         | Payout    | Per-item  | vs buying |
| -------- | ------------------------------- | --------- | --------- | --------- |
| 1        | 1000 × 1 / (100+1) × 0.875      | 8.66      | 8.66      | (11.36)   |
| 10       | 1000 × 10 / (100+10) × 0.875    | 79.55     | 7.95      | (12.50)   |
| 50       | 1000 × 50 / (100+50) × 0.875    | 291.67    | 5.83      | (22.50)   |

The spread between buy and sell prices is the **fee** (12.5 %) plus the
**slippage term**. The fee is not a separate line item — it is built into
the price and accrues to both reserves, so `k` only grows.

## Failure and rollback behavior

Every trade either commits fully or rolls back completely. No partial state
is written. See [src/store/rollback.rs](src/store/rollback.rs).

### Buy order failures

| Failure point                | Rollback action                                 |
| ---------------------------- | ----------------------------------------------- |
| Before withdrawal            | None needed — order cancelled                   |
| After withdrawal, pre-trade  | Items deposited back to storage (best-effort)   |
| Trade rejected by player     | Items deposited back to storage                 |
| Trade timeout (45 s)         | Items deposited back to storage                 |
| After trade commit           | N/A — success; ledger + storage committed       |

### Sell order failures

| Failure point                | Rollback action                                 |
| ---------------------------- | ----------------------------------------------- |
| Before trade                 | None needed — order cancelled                   |
| Trade rejected by player     | None — player kept items                        |
| Trade timeout (45 s)         | None — player kept items                        |
| Storage deposit fails after trade | Player NOT paid; bot attempts trade-back   |

> [!WARNING]
> If a sell deposit fails, the **player does not receive payment** but the
> items are in the bot's inventory. The bot attempts a trade-back; this is
> best-effort. See [RECOVERY.md](RECOVERY.md) section 4 for manual recovery.

### Data consistency guarantees

| Property        | Guarantee                                               |
| --------------- | ------------------------------------------------------- |
| Ledger updates  | Only committed after successful trade + storage sync    |
| Storage state   | Synced from real chest contents after each operation    |
| Balance changes | Applied atomically with trade completion                |
| Pair reserves   | Updated only after full transaction success            |

## Commands

### Player commands (all users)

All commands arrive as `/msg <bot> <command>`. Items are normalized
(the `minecraft:` prefix is stripped). See
[src/store/command.rs](src/store/command.rs) for parsing and
[src/store/handlers/](src/store/handlers/) for dispatch.

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

### Operator commands (require operator status)

Set via CLI menu option 3.

| Command          | Alias | Usage                    | Description                        |
| ---------------- | ----- | ------------------------ | ---------------------------------- |
| `additem`        | `ai`  | `additem <item> <qty>`   | Deposit stock via `/trade`         |
| `removeitem`     | `ri`  | `removeitem <item> <qty>`| Withdraw stock via `/trade`        |
| `addcurrency`    | `ac`  | `addcurrency <item> <amt>` | Add diamonds to pair reserve     |
| `removecurrency` | `rc`  | `removecurrency <item> <amt>` | Remove diamonds from reserve  |

### CLI menu (operator interface)

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
15. **Exit** — graceful shutdown (≈ 5–6 s; see "Shutdown sequence" above).

## Development notes

### Error handling

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

### Item ID handling

- **`ItemId` newtype** ([src/types/item_id.rs](src/types/item_id.rs)) wraps
  every item-referencing field (`Pair::item`, `Chest::item`, `Order::item`,
  `Trade::item`, `ChestTransfer::item`). Construction strips `minecraft:`
  and rejects empty strings, so normalization bugs are compile errors.
- **Serde**: `#[serde(transparent)]` keeps the on-disk form a bare string
  — fully backwards compatible.
- **Bot interaction**: `ItemId::with_minecraft_prefix()` re-adds the prefix
  when matching Azalea item IDs.
- **Player input**: both `diamond` and `minecraft:diamond` are accepted.

### Testing

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

### Known limitations

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

## Where to start reading

| You want to understand…        | Read this                                                        |
| ------------------------------ | ---------------------------------------------------------------- |
| How a whisper becomes an order | [src/store/command.rs](src/store/command.rs), `handlers/player/` |
| Buy/sell orchestration         | [src/store/orders.rs](src/store/orders.rs) `handle_buy_order` / `handle_sell_order` |
| Trade state machine            | [src/store/trade_state.rs](src/store/trade_state.rs) `TradeState` enum |
| Trade instructions             | [src/messages.rs](src/messages.rs) `BotInstruction::TradeWithPlayer`, `TradeItem` |
| Trade GUI automation           | [src/bot/trade.rs](src/bot/trade.rs) `execute_trade_with_player` |
| AMM pricing                    | [src/store/pricing.rs](src/store/pricing.rs)                     |
| Rollback semantics             | [src/store/rollback.rs](src/store/rollback.rs)                   |
| Rate limiting                  | [src/store/rate_limit.rs](src/store/rate_limit.rs)               |
| Chest I/O (the big one)        | [src/bot/chest_io.rs](src/bot/chest_io.rs)                       |
| JSON formats on disk           | [DATA_SCHEMA.md](DATA_SCHEMA.md)                                 |
| Operator recovery              | [RECOVERY.md](RECOVERY.md)                                       |
