# cj-store — Architecture

Complement to [README.md](README.md). README covers *what* the system does;
this document diagrams *how* the pieces fit together. For the user/operator
command reference see [COMMANDS.md](COMMANDS.md); for developer reference
(build, error model, testing, limitations, perf tuning) see
[DEVELOPMENT.md](DEVELOPMENT.md); for JSON file formats see
[DATA_SCHEMA.md](DATA_SCHEMA.md); for operational playbooks see
[RECOVERY.md](RECOVERY.md).

Top-to-bottom reading order: runtime → dispatch → trade lifecycle → storage
model → pricing → failure handling. For code pointers, jump to
[Where to start reading](#where-to-start-reading) at the end.

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
  inbound to the Store. Buffered at 128: large enough to absorb bursts
  (e.g. many whispers during an event) without blocking senders, small
  enough that sustained back-pressure surfaces as a visible queue stall
  if the Store falls behind.
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
    ┌───────────────────────────────────────────────────────┐
    │  loop {                                               │
    │    if queue non-empty AND !processing_order {         │
    │      process_next_order()    ── runs to completion,   │
    │      save_if_dirty()            no cancel points      │
    │      continue                ── re-check queue before │
    │    }                            blocking on recv      │
    │                                                       │
    │    msg = rx.recv().await    ── single blocking await  │
    │    dispatch(msg)                                      │
    │                                                       │
    │    if dirty AND elapsed >= autosave_interval          │
    │      save_if_dirty()                                  │
    │    if elapsed >= cleanup_interval                     │
    │      prune_caches()                                   │
    │  }                                                    │
    └───────────────────────────────────────────────────────┘
```

Autosave and cleanup are elapsed-time checks on each post-message pass, not
separate tick channels. `tokio::select!` was deliberately avoided — cancelling
an in-flight order's oneshot receivers mid-operation caused stuck trades; see
the inline comment in [src/store/mod.rs](src/store/mod.rs) `Store::run`.

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
[src/error.rs](src/error.rs). Rollback, player messaging, and queue
interactions share a single error surface.

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
                  │
                  ├─── no post-trade chest work needed ────┐
                  ▼                                        │
           ┌─────────────┐                                 │
           │ Depositing  │  bot puts received items        │
           └──────┬──────┘  back into storage              │
                  │  deposit ok                            │
                  ▼                                        ▼
           ┌─────────────┐          ┌──────────────┐
           │  Committed  │          │  RolledBack  │  (terminal)
           └─────────────┘          └──────────────┘
```

`Depositing` is optional — `Trading → Committed` is valid for trades whose
payout goes straight to the user balance (e.g. buys where diamonds are
credited to the ledger rather than written to a chest). `commit()` accepts
either `Trading` or `Depositing` as the predecessor.

`TradeState` is mirrored to `data/current_trade.json` at every transition
and cleared on a terminal state. On startup the Store looks for a leftover
file: its presence means the previous run crashed mid-trade.

**Today** the startup behavior is *log-and-clear*: the leftover state is
written to the log, the file is removed, and the Store starts fresh.
Physical chests and the ledger may be inconsistent with each other — that
is what [RECOVERY.md § 4](RECOVERY.md#4-interrupted-datacurrent_tradejson)
exists for. If the *only* symptom is a frozen queue (no physical drift
suspected), CLI menu entry 15 ("Clear stuck order") releases
`processing_order` and returns the blocked queue entry without any hand
edits; see [COMMANDS.md § CLI menu](COMMANDS.md#cli-menu-operator-interface).

### Planned: automatic crash-resume

Not yet implemented. When it lands, startup in `Store::new` will inspect
the persisted phase and route:

| Phase on disk | Planned startup action                                    |
| ------------- | ---------------------------------------------------------- |
| `Queued`      | Re-add to `OrderQueue` front                               |
| `Withdrawing` | Roll back any completed transfers, reject the order        |
| `Trading`     | Reject the order (player trade state is unknown)           |
| `Depositing`  | Roll back the withdraw, reject the order                   |
| `Committed`   | Nothing (terminal — shouldn't survive a crash here)        |
| `RolledBack`  | Nothing (terminal)                                         |

## Operation journal (chest I/O crash recovery)

The journal sits **one level below** `TradeState`: where `TradeState`
tracks a whole player order (Withdrawing → Trading → Depositing), the
journal tracks the single in-flight *shulker box operation* that the
current phase is executing. The two are deliberately separate because
shulker ops also happen **outside** trades — operator `additem` /
`removeitem` whispers go directly through the chest-I/O layer without
ever creating a `TradeState`. The journal catches crashes in either
code path. See [src/store/journal.rs](src/store/journal.rs).

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
file. Automatic replay of partial shulker ops is
[planned](#planned-automatic-crash-resume) but not yet implemented; today
the operator works through [RECOVERY.md § 2](RECOVERY.md#2-stuck-datajournaljson-entry).

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

> [!IMPORTANT]
> The system **assumes every chest slot contains exactly 1 shulker box**.
> `amounts[i]` tracks items *inside* the shulker in slot `i`, not the
> shulker itself — the shulker is assumed to always be present. If a slot
> is empty or holds a non-shulker item, operations on that chest will
> fail. This is the single load-bearing invariant of the storage model;
> the deposit planner, withdraw planner, and chest-sync all rely on it.

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

Total wall-clock ≈ 5–6 s. The two `DELAY_DISCONNECT_MS` waits are *independent*
events (packet flush, then TCP teardown after the Azalea task is aborted) —
collapsing them lets a subsequent reconnect race Azalea's background task.
The Store breaks out of its loop immediately on shutdown and saves state
twice (shutdown handler + final cleanup) as belt-and-suspenders.

## Source tree

Only the non-obvious files are annotated. Module names carry the rest.

```text
cj-store/
  Cargo.toml
  .cargo/config.toml            # optional fast-build flags (-Z needs nightly)
  src/
    main.rs                     # spawns Store + Bot + CLI; try_join! on shutdown
    store/                      # authoritative state + handlers + autosave
      mod.rs                    # Store struct, run loop, message routing
      handlers/
        player.rs               # whisper dispatcher + rate limiting
        validation.rs
        buy.rs   sell.rs
        deposit.rs  withdraw.rs
        info.rs                 # price, balance, pay, items, queue, cancel, status, help
        operator.rs             # additem, removeitem, add/remove currency
        cli.rs                  # CLI-originated message handlers
      command.rs                # Command enum + parse_command
      journal.rs                # chest-I/O crash-recovery journal
      orders.rs                 # execute_queued_order, handle_buy/sell
      pricing.rs                # constant-product AMM + proptest
      queue.rs                  # OrderQueue persistence
      rate_limit.rs             # anti-spam backoff
      rollback.rs
      state.rs                  # save, audit, invariants
      trade_state.rs            # TradeState SM + crash-resume mirror
      utils.rs                  # UUID cache, send_message_to_player, summarize helpers
    bot/
      mod.rs                    # Bot struct, event loop
      connection.rs  navigation.rs  shulker.rs
      chest_io.rs               # withdraw_shulkers / deposit_shulkers
      trade.rs                  # /trade GUI automation
      inventory.rs              # ensure_inventory_empty, hotbar sweep
    cli.rs                      # dialoguer menu → StoreMessage
    config.rs  constants.rs  error.rs
    fsutil.rs                   # atomic write (temp + rename)
    messages.rs                 # StoreMessage / BotMessage / CliMessage / BotInstruction
    types.rs                    # entry; re-exports + TradeType
    types/
      item_id.rs                # normalized ItemId newtype
      user.rs  pair.rs  order.rs  trade.rs
      storage.rs  node.rs  chest.rs  position.rs
  data/                         # see DATA_SCHEMA.md
```

## Node layout and chest capacity

The previous section covered the in-memory data model. This section
covers the *physical* in-world layout that that data model mirrors. The
bot models storage as chests clustered into **nodes** — each node is 4
double chests the bot can reach from one standing position.

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
- Every chest slot must contain exactly one shulker box — see
  [Storage physical model](#storage-physical-model) for the full invariant.

## Order queue system

The Store uses a FIFO queue so quick commands (balance/price/help) stay
responsive even while trades execute. See [src/store/queue.rs](src/store/queue.rs).

Order lifecycle is the same machine as [§ Trade state machine](#trade-state-machine).
The coarse states a player sees map onto it as: `QUEUED` = `Queued`,
`PROCESSING` = `Withdrawing`/`Trading`/`Depositing`, `SUCCESS` = `Committed`,
`CANCELLED` = `RolledBack`.

### Queue limits

| Property             | Value                         | Details                                                |
| -------------------- | ----------------------------- | ------------------------------------------------------ |
| Max orders per user  | 8                             | Prevents queue monopolization                          |
| Global queue cap     | 128 (`MAX_QUEUE_SIZE`)        | Enqueue rejected on saturation                         |
| Persistence          | `data/queue.json`             | Survives restarts                                      |
| Trade timeout        | `trade_timeout_ms` (45 s)     | Bounds the whole `/trade` lifecycle (request → accept → exchange); order cancelled on expiry |
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

The only mechanism for moving items between bot and player. Each side of
the GUI has **12 offer slots**, so the per-trade cap is `12 × stack_size`:
768 for stack-64, 192 for stack-16, 12 for unstackable. Full shulker boxes
cannot be traded — only loose items.

### Trade lifecycle

```text
  1. Bot whispers player with trade details
  2. Bot sends: /trade <username>
  3. ─┬─ Player accepts ──→ GUI opens
     ├─ Player declines ──→ Trade aborted
     └─ Server emits "not been accepted" ──→ Trade aborted
  4. Player adds items to their offer slots
  5. Player clicks confirm (indicators leave gray)
  6. Bot validates: correct items + counts + no gray indicators
  7. ─┬─ Validation passes ──→ Bot clicks accept ──→ Trade completes
     └─ Validation fails ──→ Trade aborted
  8. ─┬─ Completes before `trade_timeout_ms` ──→ Items exchanged ──→ Success
     └─ Timeout (`trade_timeout_ms`, default 45 s) ──→ Trade cancelled
```

### Timeouts

| Phase                    | Timeout                       | On timeout                                      |
| ------------------------ | ----------------------------- | ----------------------------------------------- |
| `/trade` lifecycle       | `trade_timeout_ms` (45 s)     | One bound covers request → accept → exchange; trade cancelled, rollback attempted |
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

Slot numbering is the standard Minecraft container convention:
`slot = row × 9 + column`, row-major, zero-indexed.

| Area           | Rows | Columns | Slot numbers                   |
| -------------- | ---- | ------- | ------------------------------ |
| Bot offer      | 0-2  | 0-3     | 0-3, 9-12, 18-21               |
| Player offer   | 0-2  | 5-8     | 5-8, 14-17, 23-26              |
| Bot accept     | 4-5  | 0-1     | 36-37, 45-46 (lime wool)       |
| Bot cancel     | 4-5  | 2-3     | 38-39, 47-48 (red wool)        |
| Player status  | 4-5  | 5-8     | 41-44, 50-53 (dyes)            |
| Separator      | all  | 4       | iron bars, non-interactable    |

Player status dye meanings:

| Dye            | State     | Meaning                         |
| -------------- | --------- | ------------------------------- |
| `gray_dye`     | Default   | Player hasn't interacted yet    |
| `magenta_dye` | Waiting   | Player reviewing/modifying      |
| `lime_dye`     | Confirmed | Player accepted the trade       |

The bot clicks accept once no player indicator is still `gray_dye` —
i.e. the player has moved past the default "not interacted" state on
every slot. `magenta_dye` and `lime_dye` both count as ready; strict
correctness is enforced by re-validating the player's offered items on
every tick before the click (see the race-condition notes in
[src/bot/trade.rs](src/bot/trade.rs) around the accept loop).

### Safety validations

Before accepting any trade, the bot verifies:

1. Item types match using normalized item IDs.
2. Item counts are exact (not less, not more).
3. No unexpected items present in the offer.
4. No player indicator is still `gray_dye` (player has interacted).

Failure at any step aborts the trade and notifies the player.

### Withdrawal / deposit flow

Per-shulker hop sequence (same for both directions, differs only in which
way items move at step 3):

1. **Chest → hotbar.** Navigate to node P, open chest, take shulker into
   hotbar slot 0.
2. **Hotbar → station.** Place shulker on station block S (2 blocks west).
3. **Transfer.** Open shulker; move items in (deposit) or out (withdraw)
   between shulker GUI and inventory slots 9–35.
4. **Station → inventory.** Break shulker; walk to pickup X; grab drop.
5. **Inventory → chest.** Walk back to P; place shulker back in its slot.

Repeat until the trade's plan is exhausted. Full implementation in
[src/bot/chest_io.rs](src/bot/chest_io.rs).

### Inventory slot rules

| Slots | Purpose              | Notes                                                 |
| ----- | -------------------- | ----------------------------------------------------- |
| 0-8   | Not used             | Crafting slots (inaccessible)                         |
| 9-35  | Main inventory       | Items go here during operations                       |
| 36    | Reserved for shulker | Hotbar slot 0, always kept clear                      |
| 37-44 | General hotbar       | Cleared to inventory (9-35) after each trade          |

## Pricing (constant-product AMM)

Formulas in [src/store/pricing.rs](src/store/pricing.rs); see
[Uniswap V2 protocol overview](https://docs.uniswap.org/contracts/v2/concepts/protocol-overview/how-uniswap-works)
for the mathematical background.

Each pair holds two reserves:

- `x = item_stock` — items in storage
- `y = currency_stock` — diamonds in the reserve

The invariant is `k = x × y`. A trade must preserve `k` (ignoring fees):
the player takes `Δx` items and pays diamonds such that the new product
equals the old product. Solving that for the trade size gives:

- **Buy** (player takes `q` items): `cost = y × q / (x - q) × (1 + fee)`
- **Sell** (player delivers `q` items): `payout = y × q / (x + q) × (1 - fee)`

The fee is applied *after* the pure-CPMM price. Fees stay in the pool on
both sides, so `k` only ever grows.

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
| 99       | 1000 × 99 / (100-99) × 1.125    | 111,375.00  | 1,125.00  | +9,800 %   |

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
