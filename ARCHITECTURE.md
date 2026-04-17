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

Why three tasks rather than one actor per type: Azalea's `ClientBuilder::start`
returns a `!Send` future, so the Bot task must live on a `LocalSet`. Splitting
the Store onto its own `tokio::spawn` keeps state mutation off the Bot's
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
chest, opens it, reads a canonical per-slot count list, and ships it back
as `ChestSyncReport`. The Store then overwrites its view of that chest.
This means the in-game server is the source of truth for *per-shulker*
counts; the Store's view is reconciled after each visit, not predicted.

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

## Where to start reading

| You want to understand…        | Read this                                                        |
| ------------------------------ | ---------------------------------------------------------------- |
| How a whisper becomes an order | [src/store/command.rs](src/store/command.rs), `handlers/player/` |
| AMM pricing                    | [src/store/pricing.rs](src/store/pricing.rs)                     |
| The trade lifecycle end-to-end | [src/store/orders.rs](src/store/orders.rs) `execute_queued_order`|
| Rollback semantics             | [src/store/rollback.rs](src/store/rollback.rs)                   |
| Rate limiting                  | [src/store/rate_limit.rs](src/store/rate_limit.rs)               |
| Chest I/O (the big one)        | [src/bot/chest_io.rs](src/bot/chest_io.rs)                       |
| JSON formats on disk           | [DATA_SCHEMA.md](DATA_SCHEMA.md)                                 |
