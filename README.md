# cj-store

> **Minecraft automated shop/exchange bot** with persistent state, constant product AMM pricing, and full trade automation.

A feature-complete Minecraft "store clerk" bot that handles in-game trading via whisper commands, with durable on-disk state and physical storage integration.

## Table of Contents

- [Overview](#overview)
- [Goals and Non-Goals](#goals-and-non-goals)
- [Quick Start](#quick-start)
- [Program Structure](#program-structure-source-tree)
- [Runtime Architecture](#runtime-architecture)
- [Configuration](#configuration)
- [Persistence Layout](#persistence-layout-authoritative-spec)
- [Player Command Interface](#player-command-interface)
- [Operator (CLI) Interface](#operator-cli-interface)
- [Logging](#logging)
- [Trade Protocol](#trade-protocol-trade)
- [Build and Run](#build-and-run)
- [Known Issues](#known-issues)
- [Troubleshooting](#troubleshooting)
- [Development Notes](#development-notes)
- [Important Implementation Details](#important-implementation-details)


---

## Overview

This repository contains a small async system with three cooperating components:

| Component | Description |
|-----------|-------------|
| **Store** | Authoritative state + persistence (JSON files under `data/`) |
| **Bot** | Minecraft client I/O ([Azalea](https://github.com/azalea-rs/azalea)) that uses `/msg` (whispers) to receive commands from players, communicate status/prices, and interact with the in-game environment |
| **CLI** | Interactive operator menu (view balances, restart bot, manage storage, etc.) |

### Feature Status

**✅ Implemented (Core Functionality):**

- Persistent schemas for users, pairs, orders, trades, and storage
- **Player commands:** `buy` (b), `sell` (s), `price` (p), `balance` (bal), `pay`, `deposit` (d), `withdraw` (w), `items`, `queue` (q), `cancel` (c), `status`, `help` (h)
- **Operator commands:** `additem` (ai), `removeitem` (ri), `addcurrency` (ac), `removecurrency` (rc)
- Full trade GUI automation with `/trade` protocol
- Storage-backed fulfillment with shulker box handling
- Pathfinding and node navigation
- **Constant product AMM pricing** (x × y = k) with slippage — larger trades have more price impact
- Transactional buy/sell with rollback on failure
- **Order queue system:** Non-blocking message handling, FIFO processing, max 8 orders per user
- **Rate limiting:** Anti-spam with exponential backoff (2s base, doubles per violation, max 60s)

**🔄 Optional Enhancements (Future):**

- Multi-item trades
- Order books / limit orders
- Statistics tracking

---

## Goals and Non-Goals

- **Goal**: Run an in-game "store clerk" bot that players can whisper commands to (buy/sell/price/items/balance/pay/deposit/withdraw/queue/cancel/status/help, with short aliases like b/s/p/d/w/q/c/h), backed by durable on-disk state.
- **Goal**: Model physical storage (nodes — clusters of chests) so the bot can deposit/withdraw items from real chests.
- **Goal**: Implement constant product AMM pricing (x × y = k) where larger trades have more price impact (slippage), similar to [Uniswap](https://docs.uniswap.org/contracts/v2/concepts/protocol-overview/how-uniswap-works).
- **Non-goal (for now)**: A full exchange engine with order books or limit orders.

---

## Quick Start

### 1. Prerequisites

- **Rust nightly toolchain** (pinned via `rust-toolchain.toml`)
- A **Microsoft account** with Minecraft ownership (used for bot authentication)
- Access to a **Minecraft server** (e.g., `corejourney.org`)

### 2. Build and Configure

```bash
# Clone and build
git clone <repo-url>
cd cj-store
cargo build --release

# First run creates data/config.json with defaults, then fails on auth —
# this is expected. Stop the bot (Ctrl+C), edit the config, then run again.
cargo run --release

# Edit configuration — you MUST set account_email and server_address
code data/config.json  # or your preferred editor
```

### 3. In-World Setup

Before the bot can operate, you need to build physical storage nodes in Minecraft:

1. Choose a **storage origin position** (set in `config.json` as `position`)
2. Build **Node 0** at that position (see [Storage Layout](#storage-graph-datastorageltnode_idgtjson) for the exact layout)
3. Fill all 4 double chests with **shulker boxes** (one per slot = 54 shulkers per chest)

The bot auto-manages its own inventory (keeping hotbar slot 0 free for shulker handling), so no manual bot-side setup is needed beyond the physical build.

### 4. Run the Bot

```bash
cargo run --release
```

The CLI will present an interactive menu. Use it to:
- Add your storage nodes (option 5 for validated or 4 for unvalidated)
- Add trading pairs (option 8)
- Set yourself as an operator (option 3)
- Fund pairs with initial currency using the `addcurrency` command in-game

### 5. Player Usage

Once running, players can whisper commands to the bot:

```
/msg <botname> items       # List tradeable items
/msg <botname> price iron  # Check iron price
/msg <botname> buy iron 64 # Buy 64 iron ingots
/msg <botname> sell iron 64 # Sell 64 iron ingots
/msg <botname> help        # Full command list
```

---

## Program Structure (Source Tree)


```text
cj-store/
  Cargo.toml
  .cargo/config.toml            # optional fast-build flags (see "Build notes")
  src/
    main.rs                     # starts Store + Bot + CLI tasks
    store/                       # Store module (authoritative state + message handlers + autosave)
      mod.rs                    # Store struct, run loop (priority-based: orders before messages), message routing
      handlers/                 # Command handlers
        mod.rs                  # Handler module exports
        player.rs              # Player command handlers (buy, sell, balance, pay, queue, cancel)
        operator.rs            # Operator handlers (additem, removeitem, add/remove currency)
        cli.rs                 # CLI message handlers
      journal.rs                # Operation journal for crash recovery (tracks in-flight shulker ops)
      orders.rs                 # Order execution (handle_buy_order, handle_sell_order, execute_queued_order)
      pricing.rs                # Constant product AMM pricing + property-based tests (proptest)
      queue.rs                  # Order queue system (QueuedOrder, OrderQueue, persistence)
      rate_limit.rs             # Anti-spam rate limiting with exponential backoff
      rollback.rs               # Shared rollback helper (items/diamonds back to storage on failure)
      state.rs                  # State management (save, audit_state, assert_invariants)
      utils.rs                  # Helper functions (normalize_item_id, resolve_user_uuid, UUID cache, etc.)
    bot/                        # Bot module (Azalea bot client + whisper parsing → StoreMessage)
      mod.rs                    # Bot struct, BotState, bot_task, event handlers
      connection.rs             # Connection management (connect, disconnect)
      navigation.rs             # Pathfinding (navigate_to_position, go_to_node, go_to_chest)
      shulker.rs               # Shulker operations (place, pickup, open, station position)
      chest_io.rs               # Chest operations — automated_chest_io dispatches to withdraw_shulkers / deposit_shulkers, chunk-not-loaded retry
      trade.rs                  # Trade automation (execute_trade_with_player, trade GUI handling)
      inventory.rs              # Inventory management (ensure_inventory_empty, move_hotbar_to_inventory, etc.)
    cli.rs                      # dialoguer menu → StoreMessage
    config.rs                   # data/config.json loader/creator
    messages.rs                 # StoreMessage / BotMessage / CliMessage / BotInstruction
    types.rs                    # re-exports model types
    error.rs                    # StoreError enum (typed errors for hot-path operations)
    types/
      item_id.rs                # ItemId newtype — normalized, non-empty item identifier
      user.rs                   # per-user persistence + Mojang UUID lookup
      pair.rs                   # per-item "pair" persistence (data/pairs/*.json)
      order.rs                  # global queue persistence (data/orders.json)
      trade.rs                  # per-trade persistence (data/trades/*.json)
      storage.rs                # storage graph loader/saver (data/storage/<node_id>.json)
      node.rs                   # node placement + per-node chest load/save
      chest.rs                  # chest schema + per-chest load/save
      position.rs               # simple x/y/z
  data/
    config.json
    logs/store.log
    journal.json                # In-flight shulker operation (crash recovery, normally empty)
    orders.json                 # Order audit log (session-only, cleared on restart)
    queue.json                  # Pending order queue (persistent, resumes on restart)
    pairs/*.json
    users/*.json
    storage/<node_id>.json
    trades/*.json
```

---

## Runtime Architecture

### Tasks

At runtime `main.rs` spawns three tasks:

- **Store task** (`Store::run`): owns all mutable state (`pairs`, `users`, `orders`, `trades`, `storage`) and handles messages.
- **Bot task** (`bot_task`): maintains an Azalea client connection; listens for `BotInstruction`s from Store.
- **CLI task** (`cli_task`): blocking interactive loop; sends operator actions to Store.

### Message Flow

All cross-component communication is explicit:

- **Bot → Store**: whispers in Minecraft chat are parsed into `BotMessage::PlayerCommand` and delivered as `StoreMessage::FromBot(...)`.
- **CLI → Store**: operator actions become `StoreMessage::FromCli(...)` and often include a oneshot response channel.
- **Store → Bot**: store sends `BotInstruction` (restart/shutdown/trade/chest actions).

### Concurrency and Thread Safety

**Single-threaded state management**: The Store task processes all messages sequentially in a single async task. This ensures:
- **No race conditions**: All state mutations happen in order, one at a time
- **Consistent state**: Invariants are maintained between operations
- **Predictable behavior**: Concurrent buy/sell orders are queued and processed sequentially

**Priority-based event loop**: Each iteration of the Store loop either drains one order from the queue OR blocks on one incoming message — never both concurrently. Orders are given strict priority so an in-flight trade cannot be interrupted:
- **Quick commands** (balance, price, help, items, queue, cancel, status) are validated and executed inline when picked up by the loop
- **Order commands** (buy, sell, deposit, withdraw) are validated and added to a queue, then processed one at a time
- While an order is executing, any incoming messages simply buffer in the channel and are handled as soon as the order completes (orders are typically short, so the delay is usually imperceptible)

**Message channels**: All inter-task communication uses Tokio channels:
- `mpsc::channel` for Store ↔ Bot communication (buffered, 128 messages)
- `oneshot::channel` for request/response patterns (CLI queries, trade confirmations)

**Important behavior**:
- **Autosave**: debounced by `autosave_interval_secs` in config (default 2s); saves only when state is dirty. A final save always happens on shutdown, and a non-debounced save runs immediately after every completed order to guarantee trade/balance/stock updates are never lost to a crash.
- **Order processing**: Multiple players can send commands simultaneously, but order commands are queued and processed one at a time by the Store task, ensuring no conflicts.

### Order Queue System

The bot uses a **FIFO order queue** to handle multiple buy/sell/deposit/withdraw requests from players. This enables non-blocking command handling — players can check prices and balances even while orders are being processed.

#### Order Types

| Order Type | Command | What Happens |
|------------|---------|--------------|
| **Buy** | `buy <item> <qty>` | Bot withdraws items from storage, trades to player |
| **Sell** | `sell <item> <qty>` | Player trades items to bot, bot deposits to storage |
| **Deposit** | `deposit [amount]` | Player trades diamonds to bot, credited to balance |
| **Withdraw** | `withdraw [amount]` | Bot trades diamonds to player, deducted from balance |

#### Order Lifecycle

```
┌──────────────────────────────────────────────────────────────────────┐
│                         ORDER LIFECYCLE                              │
├──────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  QUEUED                                                              │
│    │  Player sends command (e.g., "buy cobblestone 64")              │
│    │  Validation: item exists, quantity valid, user limits ok        │
│    ↓                                                                 │
│  "Order #47 queued (position 3/5). Est. wait: ~2 min."               │
│    │                                                                 │
│    │  (waiting in FIFO queue)                                        │
│    ↓                                                                 │
│  PROCESSING                                                          │
│    │  "Now processing: buy 64 cobblestone..."                        │
│    │  Bot prepares items, sends trade request                        │
│    ↓                                                                 │
│  ─┬─ Trade accepted + completed ──→ SUCCESS                         │
│   │   "Bought 64 cobblestone for 10.50 diamonds. Trade complete."    │
│   │                                                                  │
│   ├─ Trade timeout (30s accept, 45s complete) ──→ CANCELLED         │
│   │   "Trade timed out. Order cancelled."                            │
│   │                                                                  │
│   ├─ Player cancelled trade ──→ CANCELLED                           │
│   │   "Trade cancelled by player."                                   │
│   │                                                                  │
│   └─ Validation failed ──→ CANCELLED + ROLLBACK                     │
│       Items returned to storage (best-effort)                        │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
```

#### Queue Limits and Persistence

| Property | Value | Details |
|----------|-------|---------|
| **Max orders per player** | 8 | Prevents queue monopolization |
| **Queue persistence** | Yes | Saved to `data/queue.json`, survives restarts |
| **Trade accept timeout** | 30 seconds | Order cancelled if player doesn't accept the trade request |
| **Trade completion timeout** | `trade_timeout_ms` (default 45 s) | Order cancelled if trade doesn't complete |
| **Retry on timeout** | No | Timed-out orders are cancelled, not retried |

#### Player Feedback Messages

| Event | Example Message |
|-------|-----------------|
| Order queued | `Order #12 queued (position 2/3). Est. wait: ~1 min. You have 1 order(s) pending.` |
| Processing starts | `Now processing: buy 64 cobblestone...` |
| Pre-trade info | `Buy 64 cobblestone: Total 10.50 diamonds. Please offer 11 diamonds in the trade.` |
| Trade complete | `Bought 64 cobblestone for 10.50 diamonds (fee 1.17). Trade complete.` |
| Trade timeout | `Trade timed out. Order cancelled.` |
| Queue full | `Queue full. You have 8 pending orders (max 8). Wait for some to complete.` |

#### Queue Commands

| Command | Alias | Usage | Description |
|---------|-------|-------|-------------|
| `queue` | `q` | `queue [page]` | Show your pending orders (4 per page) |
| `cancel` | `c` | `cancel <order_id>` | Cancel a pending order by ID |


### Rate Limiting (Anti-Spam)

The bot implements rate limiting to prevent players from spamming commands:

**Base behavior**:
- **Minimum 2 seconds between commands**: Players must wait at least 2 seconds between messages
- **Exponential backoff**: Each time a player messages faster than the required cooldown, the violation counter increments and the *next* required cooldown doubles:
  - 0 violations: 2s cooldown (base)
  - 1 violation: 4s cooldown
  - 2 violations: 8s cooldown
  - 3 violations: 16s cooldown
  - ... capped at 60 seconds maximum
- **Reset after 30s idle**: If a player stops messaging for 30 seconds, their violation count resets to 0

**Example**:
```
Player: buy cobblestone 64      [allowed]
Player: buy iron_ingot 32       [too fast - within 2s]
Bot: Please wait 1.5s before sending another message.
Player: buy iron_ingot 32       [still too fast]
Bot: Please wait 3.8s before sending another message.  [doubled wait time]
[Player waits]
Player: buy iron_ingot 32       [allowed after waiting]
```

**Notes**:
- Rate limiting applies to all commands, not just orders
- Rate limit state is in-memory only (resets on bot restart)
- Each player has their own rate limit tracking (one player spamming doesn't affect others)

### Graceful Shutdown

When the operator selects "Exit" from the CLI menu, the application performs a graceful shutdown sequence:

1. **CLI → Store**: CLI sends `CliMessage::Shutdown` to Store and waits for confirmation
2. **Store → Bot**: Store sends `BotInstruction::Shutdown` to Bot and waits for confirmation
3. **Bot disconnects**: 
   - Calls `client.disconnect()` to send disconnect packet to server
   - Waits up to 2 seconds for disconnect packet to be sent and Disconnect event to be processed
   - Aborts the Azalea client task
   - Waits 2 seconds for OS-level TCP connection closure
   - Additional 1 second wait for final cleanup
4. **Store saves data**: All state (pairs, users, orders, trades, storage) is saved to disk
5. **Store confirms**: Store sends shutdown confirmation to CLI
6. **Store exits**: Store breaks from its message loop and performs final cleanup
7. **CLI exits**: CLI receives confirmation and drops its channel, then exits
8. **Bot exits**: Bot task completes final cleanup and exits
9. **Main exits**: All tasks complete, main() returns

**Total shutdown time**: Approximately 5-6 seconds to ensure:
- Disconnect packet is sent to server
- Server processes the disconnect
- TCP connection is fully closed
- All data is persisted
- Bot leaves the server immediately (no lingering connection)

**Implementation details**:
- The disconnect sequence includes extensive wait times to ensure the bot disconnects cleanly from the server before the application exits
- The Store task breaks from its loop immediately after handling the shutdown message (doesn't wait for channel closure)
- All state is saved twice: once in the shutdown handler, and once in final cleanup as a safety measure
- See `src/bot/connection.rs::disconnect()` for detailed disconnect timing and `src/store/handlers/cli.rs::handle_cli_message()` for shutdown orchestration

---

## Configuration

The configuration file `data/config.json` is loaded on startup. If missing, it's created with default values.

> [!IMPORTANT]
> You **must** edit `data/config.json` to set `account_email` and `server_address` before the bot can connect.

### Core Settings

| Setting | Type | Description |
|---------|------|-------------|
| `position` | `{x, y, z}` | Storage origin coordinates — where Node 0 is located in the world |
| `fee` | `f64` | Fee rate (e.g., `0.125` = 12.5%) — added to buys, subtracted from sells |
| `account_email` | `string` | Microsoft account email for Azalea login (**required**) |
| `server_address` | `string` | Minecraft server hostname (e.g., `"corejourney.org"`) (**required**) |
| `buffer_chest_position` | `{x, y, z}` or `null` | Optional chest where bot can dump items if inventory becomes full |

### Configurable Timeouts and Limits

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `max_orders` | `usize` | `10000` | Prune target for the in-memory order log (session-only; `orders.json` is cleared on each restart) |
| `max_trades_in_memory` | `usize` | `50000` | Max trades loaded into memory on startup (older trades remain on disk) |
| `autosave_interval_secs` | `u64` | `2` | Minimum interval between debounced autosaves |
| `trade_timeout_ms` | `u64` | `45000` | Maximum time to wait for a trade GUI interaction to complete before aborting and rolling back |
| `pathfinding_timeout_ms` | `u64` | `60000` | Maximum time to wait for the bot to reach a destination before aborting the current navigation |


Example (full `data/config.json`):

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

All timeout and limit settings are optional and fall back to the defaults above if omitted.

### Hot-Reload

`data/config.json` is watched at runtime via the [`notify`](https://crates.io/crates/notify) crate. Edits are debounced (~500 ms) and re-validated; if validation fails, the running config is kept and the error is logged. Never crashes the bot on a bad edit.

| Field | Hot-reloadable? | Notes |
|-------|-----------------|-------|
| `fee` | ✅ Yes | Next priced order uses the new rate |
| `autosave_interval_secs` | ✅ Yes | Next Store loop iteration uses the new debounce |
| `trade_timeout_ms` | ❌ Restart | Cached in the Bot task at startup; warning logged on edit |
| `pathfinding_timeout_ms` | ❌ Restart | Cached in the Bot task at startup; warning logged on edit |
| `position`, `buffer_chest_position` | ❌ Restart | World topology; changing mid-run would break in-flight operations |
| `account_email`, `server_address` | ❌ Restart | Identity / connection; requires reconnection |
| `max_orders`, `max_trades_in_memory` | ❌ Restart | Capacity bounds fixed at load time |

Edits to restart-only fields are logged as `warn!("Config field '<name>' changed but requires restart")` — the in-memory config keeps its original value so behavior stays consistent with what the rest of the system was initialized against.

---

## Persistence Layout (Authoritative Spec)

All state is currently stored as JSON under `data/`. There is no database.

### Users (`data/users/<uuid>.json`)

Type: `User` (`src/types/user.rs`)

Fields:
- **uuid**: hyphenated Mojang UUID string
- **username**: last-seen username (can change)
- **balance**: `f64` "diamonds" balance
- **operator**: `bool` false for everyone but the operators (who can use commands like additem, removeitem, add/remove currency etc.)
Note: CLI offers an option to make anyone an operator just by typing their username or uuid in, it then asks if the CLI user wants to set the operator field for them to true or false

Notes:
- `User::get_uuid(username)` calls Mojang's public API (`api.mojang.com`). Both blocking (`get_uuid`) and async (`get_uuid_async`) versions are available, with the async version using connection pooling for better performance. UUID lookups are cached in-memory with a 5-minute TTL so repeated commands from the same player don't hit the API on every interaction.
- `pay_async(...)` in `src/store/handlers/player.rs` uses UUIDs as the canonical key and updates the stored username on each payment.

### Pairs (`data/pairs/<item>.json`)

Type: `Pair` (`src/types/pair.rs`)

Fields:
- **item**: item identifier (string; used directly as filename). Stored WITHOUT the `minecraft:` prefix for cleaner display (e.g., `diamond`, `cobblestone`). The prefix is stripped during normalization.
- **stack_size**: `i32` - maximum stack size for this item (1, 16, or 64). Must be specified when adding a pair. Examples:
  - 64: Most items (cobblestone, diamonds, iron ingot, etc.)
  - 16: Ender pearls, eggs, snowballs, signs, banners, buckets with contents
  - 1: Tools, weapons, armor, potions, enchanted books
- **item_stock**: `i32` - total item count in storage (sum across all chests for this item). Automatically synced with physical storage after each operation.
- **currency_stock**: `f64` - total diamonds in the pair's reserve. Used for pricing calculations and must be sufficient to pay for sell orders.

**Invariants** (enforced by Store):
- `item_stock >= 0` (can be zero if no items in storage)
- `currency_stock >= 0.0` (can be zero, but sell orders will fail)
- `item_stock` should match physical storage (enforced by "Repair state" command)

### Orders Audit Log (`data/orders.json`)

Type: `VecDeque<Order>` (`src/types/order.rs`)

Fields:
- **order_type**: `Buy | Sell | AddItem | RemoveItem | DepositBalance | WithdrawBalance | AddCurrency | RemoveCurrency`
- **item**: string
- **amount**: `i32`
- **user_uuid**: string

Notes:
- **Session-only**: Orders are NOT persisted across bot restarts. The `orders.json` file is cleared on each startup.
- Orders are only recorded after successful completion. Failed or timed-out operations do not create orders.
- `Store` appends buy/sell (and other) order types for runtime tracking during the current session.
- For historical transaction records, use **Trades** (`data/trades/*.json`) which ARE persisted.

### Order Queue (`data/queue.json`)

Type: `OrderQueue` → `QueuedOrder` (`src/store/queue.rs`)

Fields (QueuedOrder):
- **id**: `u64` unique order ID
- **user_uuid**: string (UUID of user who placed the order)
- **username**: string (username for messaging)
- **order_type**: `Buy | Sell | Deposit { amount } | Withdraw { amount }`
- **item**: string
- **quantity**: `u32`
- **queued_at**: RFC3339 timestamp

Notes:
- **Persistent across restarts**: Queue is loaded from disk on startup and processing resumes where it left off
- **Per-user limit**: Maximum 8 orders per user in the queue at any time
- **FIFO processing**: Orders are processed in first-in-first-out order
- **Auto-save**: Queue is saved to disk after each add/pop/cancel operation
- **Trade timeouts**: If a player doesn't accept a trade request within 30 seconds (or complete within the configured `trade_timeout_ms`, default 45 s), the order is cancelled and removed from the queue
- This is separate from `orders.json` (audit log) - the queue holds **pending** orders, while orders.json records **completed** orders

### Operation Journal (`data/journal.json`)

Type: `Vec<JournalEntry>` (`src/store/journal.rs`)

Fields (JournalEntry):
- **operation_id**: `u64` monotonic counter (per-run, not globally unique)
- **operation_type**: `WithdrawFromChest | DepositToChest`
- **chest_id**: `i32` target chest
- **slot_index**: `usize` slot within the chest
- **state**: `ShulkerTaken | ShulkerOnStation | ItemsTransferred | ShulkerPickedUp | ShulkerReplaced`

Notes:
- **Normally empty**: The file contains `[]` when no operation is in flight. A non-empty file after startup means the previous run crashed mid-shulker-operation.
- **Detection only**: On startup, the bot logs any leftover entry at error level and clears the file. It does NOT attempt automatic recovery — the operator should check in-world state.
- **Single entry**: Only one shulker operation runs at a time (chest I/O is serialized), so the array holds at most one element.

### Trades (`data/trades/<timestamp>.json`)

Type: `Trade` (`src/types/trade.rs`)

Fields:
- **trade_type**: `Buy | Sell | AddStock | RemoveStock | DepositBalance | WithdrawBalance | AddCurrency | RemoveCurrency`
- **item**: string
- **amount**: `i32`
- **amount_currency**: `f64`
- **user_uuid**: string
- **timestamp**: RFC3339 timestamp (used as filename with `:` replaced by `-`)

Notes:
- `Store::new` (in `src/store/mod.rs`) loads all trades into memory.
- Trades are persisted by `state::save()` (in `src/store/state.rs`) via `Trade::save_all(&self.trades)`.

### Storage Graph (`data/storage/<node_id>.json`)

Types: `Storage` → `Node` → `Chest` (see `src/types/storage.rs`, `node.rs`, `chest.rs`)

The storage system models a physical layout of chests in Minecraft, organized into **nodes**. Each node is a cluster of 4 double chests that the bot can access from a single standing position.

#### Storage

| Field | Description |
|-------|-------------|
| `position` | Storage origin (x, y, z) from config — defines where Node 0 is located |
| `nodes` | `Vec<Node>`, loaded from individual `data/storage/<id>.json` files on startup |

> [!NOTE]
> If no nodes exist on startup, storage is initialized empty. Use the CLI to add nodes.

#### Node

| Field | Description |
|-------|-------------|
| `id` | Integer identifier (used as filename: `data/storage/{id}.json`) |
| `position` | World position where bot stands to access this node (derived from storage origin + spiral offset) |
| `chests` | Array of 4 chests (indices 0-3), all stored in the node's JSON file |

**Reserved Chests (Node 0 only):**

| Chest | Purpose | Rules |
|-------|---------|-------|
| Chest 0 | **Diamonds (currency)** | Automatically assigned, cannot be changed |
| Chest 1 | **Overflow/failsafe** | For failed trades, unexpected items; deposit-only, never withdraw; allows mixed items |
| Chests 2-3 | **General storage** | Available for any tradeable items |

##### Node Layout (Top-Down View)

Each node occupies a 4×3 block footprint. The bot stands at position **P** (the node origin) facing north:

```
      West ← → East
         
      NCCN  ← z-2 (back of double chests)
      NCCN  ← z-1 (front of double chests, clickable face)
      XSNP  ← z   (working row)
        ↑
      North
```

**Legend:**
| Symbol | Meaning |
|--------|---------|
| `N` | Empty space (nothing) |
| `C` | Double chest block (extends 2 blocks north-south, 2 blocks tall in Y) |
| `P` | **Node position** — where bot stands (southeast corner of the node) |
| `S` | **Shulker station** — where bot places shulker boxes to access them (2 blocks west of P) |
| `X` | **Pickup position** — where bot walks to collect broken shulkers (3 blocks west of P) |

**Chest ID Layout** (when standing at P, looking north):

```
      Chest 0    Chest 1   ← y+1 (top row, eye level)
      Chest 2    Chest 3   ← y   (bottom row, ground level)
      ←─────────────────→
        West         East
```

All 4 chests are accessed from z-1 (their south face).

##### Spiral Expansion Pattern

Nodes are arranged in a clockwise spiral pattern, spaced **3 blocks apart**:

```
    z
    ↑
    │  . 6 7 8 9
    │  . 5 0 1 .
    │  . 4 3 2 .
    │  . . . . .
    └──────────→ x
```

**Node position offsets from storage origin:**

| Node | X offset | Z offset | Description |
|------|----------|----------|-------------|
| 0 | +0 | +0 | Origin (P aligns with storage origin) |
| 1 | +3 | +0 | East |
| 2 | +3 | +3 | Southeast |
| 3 | +0 | +3 | South |
| 4 | -3 | +3 | Southwest |
| 5 | -3 | +0 | West |
| 6 | -3 | -3 | Northwest |
| 7 | +0 | -3 | North |
| 8 | +3 | -3 | Northeast |
| 9 | +6 | -3 | Continue spiral... |

##### Multi-Node Layout Example

When nodes 0-3 are placed together (top-down view):

```
    CCNCCN
    CCNCCN
    SN0SN1    ← Node 0 and 1
    CCNCCN
    CCNCCN
    SN3SN2    ← Node 3 and 2
```

Note: The **X** pickup position of each node coincides with the **P** position of the node to its west, so they share space efficiently.

#### Chest

| Field | Type | Description |
|-------|------|-------------|
| `id` | `i32` | Unique ID across all chests: `node_id × 4 + index` |
| `node_id` | `i32` | Which node this chest belongs to |
| `index` | `0..3` | Position within node (see chest layout diagram above) |
| `position` | `Position` | Absolute world coordinates (derived from node position + offset) |
| `item` | `String` | Item type stored (empty string = unassigned) |
| `amounts` | `Vec<i32>` | Array of 54 values, one per chest slot |

##### Capacity Calculations

Each chest has 54 slots, and **every slot is assumed to contain exactly 1 shulker box**. The `amounts[i]` value represents the item count **inside** the shulker box in slot `i`.

| Item Stack Size | Items per Shulker | Items per Chest (54 shulkers) | Example Items |
|-----------------|-------------------|-------------------------------|---------------|
| 64 | 27 × 64 = **1,728** | 54 × 1,728 = **93,312** | Cobblestone, Iron Ingot, Diamond |
| 16 | 27 × 16 = **432** | 54 × 432 = **23,328** | Ender Pearl, Egg, Snowball, Sign |
| 1 | 27 × 1 = **27** | 54 × 27 = **1,458** | Sword, Pickaxe, Armor, Potion |

##### Storage Behavior

- **Single item type per chest**: Each chest stores only one item type (or is unassigned)
- **Shulker colors don't matter**: The system treats all shulker box colors identically
- **Chest assignment is sticky**: Once a chest is assigned to an item, it keeps that assignment even when drained to zero. Use "Repair state" in the CLI if you need to reclaim empty chests.
- **Auto-assign on overflow**: When existing chests for an item are full, the deposit planner grabs the next empty chest (preferring the same node), and creates a new node if none are available

> [!IMPORTANT]
> The system **assumes** every chest slot contains a shulker box. If a slot is empty or contains a different item, operations will fail.


---

## Player Command Interface

### How Commands Are Received

The bot listens for chat packets and treats messages containing `"whispers:"` as a whisper directed at the bot. It extracts the content and forwards it to the store as:

- `BotMessage::PlayerCommand { player_name, command }`

### Supported Commands

The Store parses the first token of the command string. All commands are case-sensitive for the command name, but item names are normalized (see "Item ID Normalization" below).

#### Quick Reference

| Command | Alias | Usage | Description |
|---------|-------|-------|-------------|
| `buy` | `b` | `buy <item> <qty>` | Buy items from the store |
| `sell` | `s` | `sell <item> <qty>` | Sell items to the store |
| `price` | `p` | `price <item> [qty]` | Check buy/sell prices |
| `balance` | `bal` | `balance [player]` | Check diamond balance |
| `pay` | — | `pay <player> <amount>` | Transfer diamonds to another player |
| `deposit` | `d` | `deposit [amount]` | Deposit physical diamonds to balance |
| `withdraw` | `w` | `withdraw [amount]` | Withdraw balance to physical diamonds |
| `items` | — | `items [page]` | List tradeable items |
| `queue` | `q` | `queue [page]` | View your pending orders |
| `cancel` | `c` | `cancel <order_id>` | Cancel a pending order |
| `status` | — | `status` | Check bot status and queue |
| `help` | `h` | `help [command]` | Show help information |

**Operator-only commands:** `additem` (ai), `removeitem` (ri), `addcurrency` (ac), `removecurrency` (rc)

#### Player Commands (Available to All Users)

**Note**: Most commands have short aliases shown in parentheses for convenience.


- **`buy <item> <quantity>`** (alias: `b`)
  - **Validation**: Checks pair exists, quantity > 0, quantity < item_stock (can't buy entire pool), player has sufficient funds (balance + diamonds to offer in trade), physical stock available
  - **Price calculation**: Constant product AMM: `cost = currency_stock × quantity / (item_stock - quantity) × (1 + fee)`. Larger trades have higher cost per item (slippage).
  - **Pre-trade notification**: Before the trade request, player is whispered the total cost and how many diamonds to offer (e.g., "Buy 64 cobblestone: Total 10.50 diamonds. Please offer 11 diamonds in the trade.")
  - **Flow**: **transactional** - adds `Order` to queue, validates funds + plans withdrawal, bot withdraws items from storage (handles shulkers), completes `/trade` (player offers diamonds if needed), syncs chest contents, commits ledger updates, records `Trade`
  - **Payment**: Flexible payment system - player can pay with any combination of balance and diamonds in trade:
    - If player has sufficient balance, no diamonds needed in trade
    - If player offers fewer diamonds than suggested, the shortfall is deducted from balance (if balance is sufficient)
    - If player offers more diamonds than needed, the surplus is credited to their balance
    - Example: Cost is 3.2, balance is 1.0, suggested diamonds is 3. Player can offer 0-3+ diamonds, with balance covering the rest.
  - **Rollback**: If trade fails after withdrawal, items are deposited back into storage (best-effort)
  
- **`sell <item> <quantity>`** (alias: `s`)
  - **Validation**: Checks pair exists, quantity > 0, quantity <= i32::MAX, store has sufficient currency reserve, physical storage space available
  - **Price calculation**: Constant product AMM: `payout = currency_stock × quantity / (item_stock + quantity) × (1 - fee)`. Larger trades have lower payout per item (slippage).
  - **Pre-trade notification**: Before the trade request, player is whispered their payout (e.g., "Sell 64 cobblestone: You'll receive 8 diamonds in trade + 0.50 to balance (total 8.50).")
  - **Flow**: **transactional** - adds `Order` to queue, validates reserve + plans deposit, bot completes `/trade` (bot offers whole diamonds, player offers items), bot deposits items into storage (handles shulkers), syncs chest contents, commits ledger updates, records `Trade`
  - **Payout**: Player receives whole diamonds in trade, fractional part credited to balance
  - **Rollback**: If deposit fails after trade, player is NOT paid and items are returned via trade-back (best-effort)

- **`price <item> [quantity]`** (alias: `p`)
  - **Validation**: Checks pair exists
  - **Behavior**: Shows buy and sell prices for the specified item using the constant product AMM formula. If quantity is omitted, defaults to one stack (based on item's `stack_size`). Prices depend on trade size due to slippage—larger quantities show worse per-item prices.
  - **Output**: Shows total cost for buying, total payout for selling, per-item prices, and current stock
  - **Example**: `price cobblestone` shows prices for 64 cobblestone, `price ender_pearl 32` shows prices for 32 ender pearls
  
- **`bal [player]` / `balance [player]`**
  - **Validation**: Resolves UUID via Mojang API (cached for 5 minutes, so repeat lookups are instant)
  - **Behavior**: Check your own balance, or specify a player name to see their balance
  - **Examples**: `balance` (your balance), `bal Steve` (Steve's balance)
  
- **`pay <player> <amount>`**
  - **Validation**: Amount > 0, amount is finite, payer exists, payee exists (created if missing), payer has sufficient balance
  - **Behavior**: Transfers `amount` diamonds from payer to payee (UUID-based, not username-based), updates both usernames to latest
  - **Notifications**: 
    - Payer receives confirmation: "Paid X diamonds to Y"
    - Payee receives notification: "You received X diamonds from Y" (only if online; offline players can still receive payments but won't see the notification until they check their balance)

- **`deposit [amount]`** (alias: `d`)
  - **Validation**: If amount specified: amount > 0, amount is finite, amount <= 768 (12 stacks)
  - **Pre-trade notification**: 
    - With amount: "Deposit X diamonds: Please offer Y diamonds in the trade."
    - Without amount: "Deposit: Please offer diamonds in the trade (up to 768 diamonds / 12 stacks). You'll be credited for the actual amount."
  - **Flow**: Player trades diamonds to bot, bot adds actual received diamonds to player's balance
  - **Success feedback**: "Deposited X diamonds to your balance. New balance: Y"
  - **Use case**: Player deposits physical diamonds into their account balance for later use in trades
  - **Flexible deposit**: If no amount specified, player can put any number of diamonds (up to 12 stacks = 768) in the trade GUI and get credited for the exact amount

- **`withdraw [amount]`** (alias: `w`)
  - **Validation**: If amount specified: amount > 0, amount is finite, player has sufficient balance, amount >= 1 whole diamond, amount <= 768 (12 stacks). If no amount: balance must have at least 1 whole diamond.
  - **Pre-trade notification**: "Withdraw X diamonds: You'll receive Y diamonds in trade."
  - **Flow**: Bot deducts `amount` from player's balance, bot trades whole diamonds to player (fractional part remains in balance)
  - **Success feedback**: "Withdrew X diamonds from your balance (Y whole diamonds in trade). Remaining balance: Z"
  - **Use case**: Player withdraws diamonds from their account balance to receive physical diamonds
  - **Full balance withdrawal**: If no amount specified, withdraws entire balance (whole diamonds only, fractional part stays), capped at 12 stacks = 768 diamonds per transaction
  - **Rollback**: If trade fails, balance is restored

- **`items [page]`**
  - **Behavior**: Lists available items for trading with pagination. Shows 4 items per page.
  - **Output**: Shows items with page number like "Items (page 1/3): cobblestone, diamond, iron_ingot, gold_ingot"
  - **Examples**: `items` (shows page 1), `items 2` (shows page 2), `items 3` (shows page 3)

- **`queue [page]`** (alias: `q`)
  - **Behavior**: Show your pending orders in the queue with pagination. Shows 4 orders per page.
  - **Output**: Shows orders with their ID, description, and queue position like "Your queue (page 1/2, 5/8): #47 buy cobblestone 64 (pos 2), #48 sell iron 128 (pos 4), ..."
  - **Examples**: `queue` (shows page 1), `q 2` (shows page 2)
  - **Note**: Also shows total orders in queue from all players

- **`cancel <order_id>`** (alias: `c`)
  - **Validation**: Order must exist and belong to the player
  - **Behavior**: Cancel a pending order by its ID. Use `queue` to see your order IDs.
  - **Examples**: `cancel 47`, `c 48`
  - **Note**: Cannot cancel orders that are currently being processed

- **`status`**
  - **Behavior**: Check what the bot is currently doing and the queue status. Shows a high-level overview without revealing any coordinates or internal details.
  - **Output examples**:
    - When idle: "Status: Idle. No orders being processed. Queue is empty."
    - When busy: "Status: Buying cobblestone x64. 3 order(s) waiting in queue."
    - When depositing: "Status: Processing deposit (128.00 diamonds)."
  - **Note**: This command executes immediately (not queued) and is useful for checking if the bot is stuck or busy

- **`help [command]`** (alias: `h`)
  - **Behavior**: Shows list of available commands, or detailed usage for a specific command
  - **Examples**: `help` (shows all commands), `help buy` (shows buy command usage), `h sell` (shows sell command usage), `help status` (shows status command usage)
  - **Note**: Regular players only see player commands; operators also see operator commands

**Note**: Prices are calculated using the constant product AMM formula (x × y = k), where larger trades have more price impact (slippage). The formula ensures you can never drain the entire pool—buying all items would cost infinity. Prices change dynamically as reserves change with each trade.

**Item ID Normalization**: Item names are normalized automatically:
- The `minecraft:` prefix is stripped if present (e.g., `minecraft:diamond` → `diamond`)
- Item names are stored without the prefix for cleaner display
- Players can use either format: `buy diamond 64` or `buy minecraft:diamond 64` (both work)

#### Operator Commands (Require Operator Status)

These commands can only be run by users who are operators (set via CLI).

- **`additem <item> <quantity>`** (alias: `ai`)
  - **Pre-trade notification**: "Additem X item: Please offer the items in the trade."
  - **Flow**: Adds `Order` to queue, plans deposit, bot receives items via `/trade`, deposits into storage (handles shulkers), syncs chest contents, commits ledger updates, records `Trade`.
  - **Success feedback**: "Added X item to storage. New stock: Y"
  - **Use case**: Operator adds items to store inventory (e.g., initial stock, donations).
- **`removeitem <item> <quantity>`** (alias: `ri`)
  - **Pre-trade notification**: "Removeitem X item: Withdrawing from storage, then trading to you."
  - **Flow**: Adds `Order` to queue, validates stock, plans withdrawal, bot withdraws from storage (handles shulkers), gives items via `/trade`, syncs chest contents, commits ledger updates, records `Trade`.
  - **Success feedback**: "Removed X item from storage. Remaining stock: Y"
  - **Use case**: Operator removes items from store inventory (e.g., maintenance, redistribution).
- **`addcurrency <item> <amount>`** (alias: `ac`)
  - **Flow**: Adds `Order` to queue, adds diamonds to pair's `currency_stock`, records `Trade`.
  - **Success feedback**: "Added X diamonds to item reserve. New reserve: Y"
  - **Use case**: Operator adds diamonds to pair reserve (e.g., initial funding, top-ups).
- **`removecurrency <item> <amount>`** (alias: `rc`)
  - **Flow**: Adds `Order` to queue, validates sufficient reserve, removes diamonds from pair's `currency_stock`, records `Trade`.
  - **Success feedback**: "Removed X diamonds from item reserve. Remaining reserve: Y"
  - **Use case**: Operator removes diamonds from pair reserve (e.g., profit withdrawal, rebalancing).

Note: the bot handles allocation of new chests if it needs more storage for any action, it also handles finding the items it needs to withdraw/trade in existing storage

---

## Operator (CLI) Interface

The CLI is a blocking interactive menu that provides operator controls:

### Available Commands

1. **Get user balances** - Displays all registered users and their diamond balances
2. **Get pairs** - Shows all trading pairs with current stock, currency reserves, and calculated buy/sell prices (using current fee from config)
3. **Set operator status** - Prompts for username/UUID, then sets the `operator` field to true or false. Operators can use commands like `additem`, `removeitem`, `addcurrency`, `removecurrency` in-game
4. **Add node (no validation)** - Creates a new node in the storage model WITHOUT physical validation. The operator should verify the physical node exists in-world before adding it. The system will create the node in memory and on disk, but won't validate that chests actually exist.
5. **Add node (with bot validation)** - Creates a new node WITH bot-based physical validation. The bot will:
   - Navigate to the calculated node position
   - Attempt to open each of the 4 chests (fast 5-second timeout per chest, no retries)
   - Verify each chest slot contains a shulker box
   - Only add the node if all checks pass
   - **Fail fast** if any chest doesn't exist (no wasted time on retries)
   Typically completes in under 30 seconds. Use this option when you want to ensure the physical node is correctly built.
6. **Discover storage (scan for existing nodes)** - Automatically discover and add existing storage nodes. The bot will:
   - Start from node position 0 (or the next unregistered ID)
   - Navigate to each position and check for valid chests with shulkers
   - Add all valid nodes to storage
   - **Stop immediately** when it encounters a position without a chest (no retries, fast 5-second timeout)
   Useful for initializing storage from an existing in-world setup. The fast-fail behavior ensures discovery doesn't waste time retrying positions that don't have chests.
7. **Remove node** - Removes node from model and deletes `data/storage/{node_id}.json` file. **Warning**: This permanently deletes the node data. Ensure the node is empty or you've backed up the data.
8. **Add pair** - Creates a new trading pair. Operator is asked for:
   - Item name (without `minecraft:` prefix, e.g., `cobblestone`, `iron_ingot`)
   - Stack size (selection menu: 64 for most items, 16 for ender pearls/eggs/signs/buckets, 1 for tools/weapons/armor)
   Both `item_stock` and `currency_stock` are initialized to zero. Use `additem` and `addcurrency` commands to add initial stock.
9. **Remove pair** - Removes a trading pair from the store. Cannot remove the diamond pair (used as currency). Warns if the pair has stock but still allows removal.
10. **View storage** - Displays the storage state including origin position, total nodes, and for each node: position and chest details (item type and total item count).
11. **View recent trades** - Shows recent trade history. Prompts for the number of trades to display (default 20). Shows timestamp, type, amount, item, currency, and user UUID.
12. **Audit state** - Checks invariants (balance validity, chest slot counts, pair vs storage consistency) and reports issues without fixing them
13. **Repair state** - Same as audit, but also repairs safe issues (e.g., recomputes `pair.item_stock` from actual storage contents)
14. **Restart Bot** - Sends `BotInstruction::Restart` to the bot, causing it to disconnect and reconnect. Useful if the bot gets stuck or needs to refresh its connection.
15. **Exit** - Requests graceful shutdown: saves all state, disconnects the bot cleanly from the server (ensuring the bot leaves immediately), and exits the application. The shutdown sequence takes approximately 5-6 seconds to ensure clean disconnection. See "Graceful Shutdown" section in Runtime Architecture for details.

---

## Logging

Tracing is configured to write **only** to:
- `data/logs/store.log`

Stdout prints a short "how to tail the log" message, but normal logs are file-only.

---

## Trade Protocol (`/trade`)

This section documents **how `/trade` works on this server** and how the bot uses it. This is the **only** supported mechanism for moving items between the bot and players.

> [!IMPORTANT]
> The bot can trade a maximum of **12 stacks per transaction** (768 items for stack-64 items). Full shulker boxes cannot be traded — only loose items.

### Trade Lifecycle

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           TRADE LIFECYCLE                                │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  1. Bot whispers player with trade details                               │
│              ↓                                                           │
│  2. Bot sends: /trade <username>                                         │
│              ↓                                                           │
│  3. ─┬─ Player accepts (within 30s) ──→ GUI opens                        │
│      │                                       ↓                           │
│      ├─ Player declines ──────────────→ Trade aborted                   │
│      │                                                                   │
│      └─ Timeout (30s) ────────────────→ Trade aborted                   │
│                                                                          │
│  4. Player adds items to their offer slots                               │
│              ↓                                                           │
│  5. Player clicks confirm (indicators turn lime)                         │
│              ↓                                                           │
│  6. Bot validates: correct items + counts + lime indicators              │
│              ↓                                                           │
│  7. ─┬─ Validation passes ──→ Bot clicks accept ──→ Trade completes     │
│      │                                                                   │
│      └─ Validation fails ───────────────────────→ Trade aborted         │
│                                                                          │
│  8. ─┬─ Completed within 45s ──→ Items exchanged ──→ Success            │
│      │                                                                   │
│      └─ Timeout (45s) ──────────────────────────→ Trade cancelled       │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

### Timeouts

| Phase | Timeout | What Happens on Timeout |
|-------|---------|-------------------------|
| Trade request acceptance | **30 seconds** | Order cancelled, player notified |
| Trade completion | `trade_timeout_ms` (default **45 s**) | Trade cancelled, rollback attempted |
| Pathfinding | `pathfinding_timeout_ms` (default **60 s**) | Navigation aborted, current action fails |

### Trade GUI Layout

The trade GUI is a 9×6 container (54 slots). Here's the visual layout:

```
     Col:  0   1   2   3   4   5   6   7   8
         ┌───┬───┬───┬───┬───┬───┬───┬───┬───┐
  Row 0  │ B │ B │ B │ B │ ║ │ P │ P │ P │ P │  ← Offer slots
         ├───┼───┼───┼───┼───┼───┼───┼───┼───┤     (items go here)
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
         
Legend:
  B = Bot offer slot (12 total)
  P = Player offer slot (12 total)  
  ║ = Separator (iron bars, column 4)
  ✓ = Accept button (lime wool, bot side)
  ✗ = Cancel button (red wool, bot side)
  ● = Player status indicator (dyes)
```

### Slot Mapping

| Area | Rows | Columns | Slot Numbers |
|------|------|---------|--------------|
| **Bot offer** | 0-2 | 0-3 | 0-3, 9-12, 18-21 |
| **Player offer** | 0-2 | 5-8 | 5-8, 14-17, 23-26 |
| **Bot accept** | 4-5 | 0-1 | 36-37, 45-46 (lime wool) |
| **Bot cancel** | 4-5 | 2-3 | 38-39, 47-48 (red wool) |
| **Player status** | 4-5 | 5-8 | 41-44, 50-53 (dyes) |
| **Separator** | all | 4 | iron bars, non-interactable |

Slot formula: `slot = row × 9 + column`

### Player Status Indicators (Dyes)

The dye colors in the player status area indicate trade state:

| Dye Color | State | Meaning |
|-----------|-------|---------|
| `gray_dye` | Default | Player hasn't interacted yet |
| `magenta_dye` | Waiting | Player is reviewing/modifying offer |
| `lime_dye` | **Confirmed** | Player has accepted the trade |

The bot **only clicks accept** when all player indicators show `lime_dye`.

### Pre-Trade Notifications

Before opening the trade, the bot whispers exact instructions:

**For Buy orders:**
```
Buy 64 cobblestone: Total 10.50 diamonds. Please offer 11 diamonds in the trade.
```

**For Sell orders:**
```
Sell 64 cobblestone: You'll receive 8 diamonds in trade + 0.50 to balance (total 8.50).
```

### Bot Storage Operations

#### Withdrawal Flow (for Buy orders)

```
1. Navigate to node position P
2. Open chest containing the shulker with needed items
3. Take shulker from chest → hotbar slot 0
4. Place shulker on shulker station (S position, 2 blocks west)
5. Open shulker and transfer items to inventory (slots 9-35)
6. Break shulker (drops as item)
7. Walk to pickup position (X, 3 blocks west of P)
8. Pick up dropped shulker
9. Return to P and put shulker back in chest
10. Repeat if more items needed
```

#### Deposit Flow (for Sell orders)

```
1. Navigate to node position P
2. Find chest with shulkers for this item (or empty chest to assign)
3. Take shulker from chest → hotbar slot 0
4. Place shulker on shulker station (S position)
5. Open shulker and transfer items from inventory
6. If shulker full: put back, get next shulker
7. Break shulker and pick up at X position
8. Put shulker back in chest
9. Repeat if more items to deposit
```

### Inventory Slot Rules

| Slot Range | Purpose | Notes |
|------------|---------|-------|
| **0-8** | Not used | Crafting slots (inaccessible) |
| **9-35** | Main inventory | Items go here during operations |
| **36-44** | Hotbar | Slot 36 reserved for shulker boxes |

After each trade, items in hotbar (36-44) are automatically moved to inventory (9-35) to keep hotbar slot 0 clear.

### Safety Validations

Before accepting any trade, the bot validates:

1. **Item types match** using normalized item IDs
2. **Item counts are exact** (not less, not more)
3. **No unexpected items** are present in the offer
4. **Player has confirmed** (all indicators show `lime_dye`)

If any validation fails, the trade is aborted and the player is notified.

### Implementation Notes

| Component | Location | Function |
|-----------|----------|----------|
| Buy/sell orchestration | `src/store/orders.rs` | `handle_buy_order`, `handle_sell_order` |
| Trade state machine | `src/store/trade_state.rs` | `TradeState` enum, phase transitions |
| Trade instructions | `src/messages.rs` | `BotInstruction::TradeWithPlayer`, `TradeItem` |
| Trade GUI automation | `src/bot/trade.rs` | `execute_trade_with_player` |


## Build and Run

### Requirements

- **Rust edition**: `2024` (latest Rust edition)
- **Toolchain**: **nightly Rust** (pinned via `rust-toolchain.toml`) because Azalea's current transitive dependencies may require nightly features.
- **Network access**:
  - Mojang API for UUID lookup (`User::get_uuid`), cached in-memory with 5-minute TTL
  - Microsoft auth flow via Azalea account login
- **Operating System**: Windows, Linux, or macOS (tested on Windows)

### Build Notes

This repo includes `.cargo/config.toml` with "fast build" flags. Some flags use `-Z...` options which require a **nightly** toolchain.

If you want to build on stable:
- remove or comment out `-Zshare-generics=...` entries in `.cargo/config.toml`, or
- use nightly Rust.

### Run

```bash
cargo run
```

---

## Known Issues

Based on `data/logs/store.log`, you may see:
- **packet decode errors** (e.g. `set_equipment ... Unexpected enum variant`) depending on server/protocol compatibility.
- **duplicate login disconnect** if the same account logs in elsewhere.

Handling status:
- Logging init is **idempotent** (we ignore "global logger already set").
- The bot performs **automatic reconnect with exponential backoff** on disconnects (including protocol decode issues / duplicate login kicks) and prevents concurrent "double-connect" attempts.

---

### Failure and Rollback Behavior

The system is designed for **transactional integrity** — either a trade completes fully or rolls back completely. No partial states are committed.

#### Buy Order Failures

| Failure Point | What Happens | Rollback Action |
|---------------|--------------|-----------------|
| Before withdrawal | Order cancelled | None needed |
| After withdrawal, before trade | Trade aborted | Items deposited back to storage (best-effort) |
| Trade GUI opened, player rejects | Trade aborted | Items deposited back to storage |
| Trade timeout (45s) | Trade cancelled | Items deposited back to storage |
| After trade completes | N/A (success) | Ledger + storage JSON committed |

#### Sell Order Failures

| Failure Point | What Happens | Rollback Action |
|---------------|--------------|-----------------|
| Before trade | Order cancelled | None needed |
| Trade GUI opened, player rejects | Trade aborted | None needed (player kept items) |
| Trade timeout (45s) | Trade cancelled | None needed |
| After trade, storage deposit fails | Items stuck | Player NOT paid; bot attempts trade-back |

> [!WARNING]
> If a sell deposit fails, the **player does not receive payment** but the items are in the bot's inventory. The bot will attempt a trade-back to return items, but this is best-effort.

#### Data Consistency Guarantees

| Property | Guarantee |
|----------|-----------|
| **Ledger updates** | Only committed after successful trade + storage sync |
| **Storage state** | Synced from real chest contents after each operation |
| **Balance changes** | Applied atomically with trade completion |
| **Pair reserves** | Updated only after full transaction success |


## Implementation Status

### ✅ Implemented Features

1. **Storage-backed fulfillment**: `buy/sell` move items via **trade** and do chest container interactions. The bot **syncs the real chest contents** after each withdraw/deposit and returns a 54-slot `amounts` vector. The Store then merges the reported counts into `Chest.amounts` (slots with `-1` mean "not checked" and keep their existing value) before committing state.

2. **In-game trade / inventory automation**: The bot uses `/trade <username>` and automates the trade GUI, including failure detection and basic rollback.

3. **Constant product AMM pricing (x × y = k)**: Prices are calculated using the constant product formula, similar to Uniswap. This means:
   - **Buy cost** = `currency_stock × quantity / (item_stock - quantity) × (1 + fee)` - larger buys have higher cost per item (slippage)
   - **Sell payout** = `currency_stock × quantity / (item_stock + quantity) × (1 - fee)` - larger sells have lower payout per item (slippage)
   - **Pool protection**: You cannot buy the entire stock—as quantity approaches item_stock, cost approaches infinity
   - **k increases with fees**: The constant product k = item_stock × currency_stock only increases (due to fees), never decreases
   - **Price changes**: As reserves change (items bought/sold), prices automatically adjust
   - **Edge cases**: If `item_stock == 0` or `currency_stock == 0`, price calculation returns `None` and trading is disabled for that item

4. **Persistence consistency**: 
   - Autosave is **debounced + dirty-flagged** (only saves when state changed, and at most once every ~2s)
   - All JSON persistence uses an **atomic write** pattern (temp + rename)
   - Save failures keep the store **dirty**, so it will retry on the next autosave tick and also attempts a final save on shutdown

5. **Operator system**: Users can be marked as operators via CLI, enabling access to `additem`, `removeitem`, `addcurrency`, and `removecurrency` commands.

6. **Diamond trade handling**: Buy orders support flexible payment - players can pay with any combination of balance and diamonds in trade. If they offer fewer diamonds than suggested, the shortfall is covered by balance. If they offer more, the surplus is credited to balance. Sell orders have the bot offer whole diamonds in trade, with fractional amounts credited to player balance.

### 🔄 Future Enhancements

- **Order books / limit orders**: Allow players to place buy/sell orders at specific prices
- **Multi-item trades**: Support trading multiple item types in a single transaction
- **Statistics and analytics**: Track trading volumes, fees collected, user activity

---

## Important Security Notes

> [!CAUTION]
> **ABSOLUTELY UNDER NO CIRCUMSTANCES WHISPER OR SAY ANY COORDINATES IN CHAT!**
> 
> The bot must never reveal storage locations, node positions, or any coordinate information in chat messages. This is a critical security requirement to protect the storage system from griefing or theft.

---


## Troubleshooting

### Bot Connection Issues

- **"Failed to connect"**: Check that `account_email` and `server_address` in `data/config.json` are correct
- **"Duplicate login"**: The account is already logged in elsewhere. Log out from other clients first
- **"Protocol decode errors"**: The server may be using a different Minecraft version than Azalea supports. Check Azalea compatibility

### Storage Issues

- **"Chest not found"**: The physical chest may not exist in-world. Verify the node was built correctly before adding it via CLI
- **"Storage mismatch"**: Use "Repair state" in CLI to recompute `pair.item_stock` from actual storage contents. This happens if storage was modified externally or if there was a bug.
- **"Node 0 chest 0" errors**: This chest is dedicated for diamonds. Don't try to assign other items to it. The system automatically enforces this.
- **"Out of physical stock"**: The pair shows stock, but physical storage doesn't have enough items. This can happen if:
  - Items were removed from chests manually
  - Storage data is out of sync
  - Solution: Use "Repair state" to sync, or manually add items via `additem` command
- **"Storage full"**: All chests are full and no empty chests available. The system will create new nodes automatically, but this requires physical nodes to exist in-world. Add nodes via CLI first.

### Trade Issues

- **"Trade timeout"**: Player may not have accepted the trade request within 30 seconds. The bot waits up to 30 seconds for the trade GUI to open. If it doesn't open, the trade is aborted. Try again.
- **"Trade closed before items could be validated"**: The trade menu closed before the bot could validate the items (e.g., player cancelled immediately). The trade is safely aborted with no items or currency exchanged.
- **"Trade cancelled by player before completion"**: The player cancelled the trade after the bot validated items but before the trade completed. No items or currency are exchanged.
- **"Trade validation failed"**: Items in trade GUI don't match expected items. The bot validates:
  - Item types match (using normalized item IDs)
  - Item counts match exactly
  - No unexpected items are present
  - Player confirmation indicators are `lime_dye` (player has confirmed)
  If validation fails, the trade is aborted and the player is notified.
- **"Inventory full"**: Bot's inventory is full. The bot should automatically manage this by:
  - Moving items from hotbar (slots 36-44) to inventory (slots 9-35) after trades
  - Using buffer chest if `buffer_chest_position` is configured
  - Keeping hotbar slot 0 (36) free for shulker boxes
  If issues persist, check `buffer_chest_position` in config or manually clear bot inventory.

### Validation and Edge Cases

- **Invalid quantities**: Negative, zero, or non-numeric quantities are rejected with error messages
- **Quantity overflow**: Quantities > i32::MAX (2,147,483,647) are rejected
- **Non-existent items**: Buying/selling items that don't exist in pairs returns "Item 'X' is not available for trading"
- **Insufficient stock**: Both physical storage and pair stock are checked. If either is insufficient, the order is rejected
- **Insufficient funds**: For buys, both balance and diamonds-to-offer are validated. For sells, currency reserve is validated
- **Price calculation edge cases**: 
  - If `item_stock == 0` or `currency_stock == 0`, price calculation returns `None` and order is rejected
  - If calculated price is non-finite or <= 0, order is rejected with "Internal error: computed price is invalid"
- **UUID lookup failures**: If Mojang API is unavailable or username doesn't exist, command fails with error message. Lookups are cached for 5 minutes, so transient API outages only affect the first command from each player
- **Concurrent orders**: Multiple orders from same or different players are processed sequentially (no race conditions)

### Rate Limiting Issues

- **"Please wait X seconds before sending another message"**: You're sending messages too quickly. Wait the specified time before trying again.
- **Wait time keeps increasing**: You've been rate limited multiple times. The wait time doubles with each violation (2s → 4s → 8s → 16s, max 60s). Stop messaging for 30 seconds to reset your violation count.

### Queue Issues

- **"Queue full. You have 8 pending orders"**: You've reached the maximum of 8 orders per player. Wait for some orders to complete before queuing more.
- **"Order #X not found in queue"**: The order may have already been processed or cancelled. Use `queue` to see your current pending orders.
- **"You can only cancel your own orders"**: You tried to cancel someone else's order. Use `queue` to see only your orders and their IDs.
- **Order still pending after long time**: Check the queue position with `queue`. Orders are processed one at a time, so if there are many orders ahead, yours will take longer.
- **"Trade failed" / Order cancelled**: If you don't accept the trade request within 30 seconds, or don't complete the trade within 45 seconds, the order is automatically cancelled. You'll need to queue a new order.
- **Orders after bot restart**: The queue persists across restarts. If the bot was restarted, your pending orders will resume processing automatically.

### Performance

- **Slow operations**: Large withdrawals/deposits may take time as bot processes multiple shulkers. Be patient
- **High disk I/O**: Autosave runs every 2 seconds when state changes. This is normal but may cause brief pauses
- **Queue processing**: Orders are processed sequentially. During busy periods, queue wait times increase
- **Server restarts / chunk unloading**: If the server restarts or chunks unload mid-operation, the bot detects the transient condition and retries with longer backoff (up to ~20s) while chunks reload. If a chest container becomes stale mid-operation, it is automatically reopened

---

## Development Notes

### Code Organization

- **Message-driven architecture**: All cross-component communication uses explicit message types (`StoreMessage`, `BotInstruction`, `CliMessage`)
- **Single-threaded state**: Store task owns all mutable state, ensuring thread safety. All state mutations happen sequentially in the Store task's message loop.
- **Atomic persistence**: All file writes use atomic operations (temp file + rename) to prevent corruption. See `fsutil::write_atomic()`.
- **Transactional operations**: Buy/sell operations are transactional with rollback on failure:
  - **Buy rollback**: If trade fails after withdrawal, items are deposited back into storage (best-effort)
  - **Sell rollback**: If deposit fails after trade, player is NOT paid and items are returned via trade-back (best-effort)

### Error Handling Patterns

- **`StoreError` enum**: `src/error.rs` defines a typed error enum (via `thiserror`) with a `From<StoreError> for String` shim so existing `Result<T, String>` call sites can migrate progressively. The hot-path helpers `execute_chest_transfers` and `perform_trade` already return typed variants (`BotDisconnected`, `TradeTimeout`, `ChestOp`, `TradeRejected`).
- **Store operations**: Return `Result<(), String>` - errors are logged and sent to player via whisper
- **Bot operations**: Return `Result<T, String>` - errors are logged and propagated to Store
- **Persistence**: Return `Result<(), Box<dyn Error>>` - errors are logged, state remains dirty for retry
- **Panic cases**: Only in unrecoverable situations (e.g., invalid chest index in `Chest::new()` - should never happen in normal operation)
- **Invariant lookups**: Store-state lookups that used to read `store.pairs.get(item).unwrap()` / `store.users.get(uuid).unwrap()` now go through `Store::expect_pair` / `Store::expect_user` (and their `_mut` variants) defined in [src/store/mod.rs](src/store/mod.rs). These return a structured `StoreError::UnknownPair` / `UnknownUser` that propagates via `?` instead of panicking the store task, and emit a `tracing::error!` with the call-site context so a broken invariant is still loud.
- **Bot journal mutex**: [`SharedJournal`](src/store/journal.rs) is a `parking_lot::Mutex` (not `std::sync::Mutex`). Its `lock()` returns the guard directly — no `Result`, no poisoning — so a panic inside the critical section cannot permanently take the bot offline. Callers still must not hold the guard across `.await` points.
- **CLI prompts**: Every `dialoguer` read in [src/cli.rs](src/cli.rs) goes through the `with_retry` helper, which loops on transient terminal I/O errors (e.g. EINTR during resize) with a 200 ms backoff instead of aborting the CLI task on the first failed read.

### Item ID Handling

- **`ItemId` newtype** ([src/types/item_id.rs](src/types/item_id.rs)): All item-referencing fields (`Pair::item`, `Chest::item`, `Order::item`, `Trade::item`, `ChestTransfer::item`) use a dedicated `ItemId` wrapper instead of raw `String`. `ItemId::new()` strips the `minecraft:` prefix and rejects empty strings at construction time, making normalization bugs compile errors. Serialized with `#[serde(transparent)]` so on-disk JSON stays a bare string — fully backwards compatible.
- **Normalization**: `ItemId::new("minecraft:diamond")` → `ItemId("diamond")`; `ItemId::new("diamond")` → `ItemId("diamond")` (unchanged). Also available as `utils::normalize_item_id()` for raw strings.
- **Storage**: Items are stored in pairs/chests WITHOUT the `minecraft:` prefix
- **Minecraft interaction**: Bot-side code re-adds the `minecraft:` prefix via `item_id.with_minecraft_prefix()` where needed (e.g., when matching Azalea item IDs)
- **Player input**: Players can use either format (`diamond` or `minecraft:diamond`) - both work

### Testing

- **Unit and integration tests**: `cargo test` runs 97 tests covering pricing invariants (including 12 property-based tests via `proptest`), storage planner parity, queue FIFO/user-limit behavior, rate-limiter backoff, journal lifecycle, `ItemId` normalization/serialization, trade state-machine transitions (happy paths, rollbacks, invalid-transition panics), UUID cache behavior (insert/lookup, case-insensitive keys, TTL expiry, invalidation, clear), the trade-GUI slot-math helpers in [src/bot/trade.rs](src/bot/trade.rs) (bot/player offer slots, status/accept/cancel slots, slot-set disjointness), and the order-handler integration suite — which now additionally exercises the rejection paths for `sell` (unknown item, zero quantity), `deposit` (non-positive amount, amount over the 768-diamond cap), and `withdraw` (insufficient balance, non-positive amount, full-balance with <1 diamond). The integration tests in [src/store/orders.rs](src/store/orders.rs) build a `Store` in-memory via `Store::new_for_test` and spawn a mock bot task so handler paths can be exercised without disk I/O or Mojang lookups (`utils::resolve_user_uuid` is cfg-gated to return deterministic offline UUIDs under `#[cfg(test)]`).
- **Property-based AMM tests**: `proptest` exercises the pricing functions across thousands of random reserve/quantity combinations, asserting that `k` never decreases, buy cost always exceeds sell payout (positive spread), per-item price increases with trade size (slippage), sell payout is bounded by the currency reserve, buys leave reserves strictly positive and finite, sequential buy-then-sell is strictly lossy at the resulting reserves, non-positive quantities always return `None` (no free-trade escape hatch), the base AMM identity `x*y=k` is preserved exactly when `fee=0.0` (isolating the fee as the sole source of `k` growth), and the fee knob is monotonic (higher fee → higher buy cost and lower sell payout). Stock/currency mutation sites in [src/store/orders.rs](src/store/orders.rs) and [src/store/handlers/operator.rs](src/store/handlers/operator.rs) are additionally guarded by `debug_assert!` that verify non-negativity and finiteness in dev/test builds (compiled out of release).
- Test with small quantities first before large trades
- Verify physical nodes exist before adding them via CLI
- Use "Audit state" regularly to catch inconsistencies early
- Monitor `data/logs/store.log` for errors and warnings

### Known Limitations

The following limitations are documented for awareness:

1. **Physical Node Validation (Optional)**: The CLI offers three options for adding nodes:
   - "Add node (no validation)" - Doesn't verify physical chests exist (operator must ensure manually)
   - "Add node (with bot validation)" - Bot physically validates all 4 chests and their shulker contents
   - "Discover storage" - Bot scans for existing nodes and adds all valid ones automatically
   Use the validation/discovery options when setting up or extending storage to ensure correctness.

2. **Order Audit Log Is Session-Only**: The order audit log (`data/orders.json`) is cleared on each bot restart. It only exists for runtime tracking during the current session. For persistent transaction history, use Trades (`data/trades/*.json`). However, the **pending order queue** (`data/queue.json`) IS persistent - if the bot restarts, pending orders resume processing automatically.

3. **Trade History Growth**: Trade history (`data/trades/*.json`) creates one file per trade. Over time this can result in many files. Trades older than 1 year can be archived using the `Trade::archive_old_trades()` function. Only the most recent `max_trades_in_memory` trades (default 50,000) are loaded into memory.

4. **Retry Logic**: Bot operations include automatic retry with exponential backoff (500ms base, up to 5s max):
   - **Chest opening (normal operations)**: Up to 3 retries (`CHEST_OP_MAX_RETRIES`). If a chunk-not-loaded condition is detected (block state `None`), the budget is extended by 2 extra retries with a longer backoff (3s base, 10s max) to wait for chunks to reload
   - **Chest opening (validation/discovery)**: NO retries, fast 5-second timeout - fails immediately if no chest exists
   - **Shulker opening**: Up to 2 retries (`SHULKER_OP_MAX_RETRIES`)
   - **Navigation/pathfinding**: Up to 2 retries (`NAVIGATION_MAX_RETRIES`)
   - **Container recovery**: If a chest container becomes stale mid-operation (e.g., server restart or chunk unload), the withdraw and deposit loops automatically reopen it via the chunk-aware retry path
   
   If all retries fail, the operation is aborted and the trade may need to be retried manually. Retry constants are defined in `src/constants.rs`. Validation/discovery operations use fast-fail behavior to avoid wasting time on non-existent chests.

5. **Single-Server Design**: The system is designed for a single bot on a single server. Running multiple instances on the same data directory is not supported and may cause data corruption.

6. **No Partial Fulfillment**: If a trade cannot be fully fulfilled (e.g., not enough items in storage), the entire trade fails. Partial fulfillment is not supported.

7. **Memory Usage**: All users, pairs, and trades (up to `max_trades_in_memory`) are loaded into memory on startup. Orders start fresh each session (not loaded from disk). Adjust limits in config for large stores.

---

## Important Implementation Details

### Item Storage Model

- **One item type per chest**: Each chest can only store one type of item (or be unassigned). This simplifies management but means items are segregated by chest.
- **Shulker box assumption**: The system assumes every chest slot contains exactly 1 shulker box. If a slot is empty or contains a different item, the system may malfunction.
- **Item count tracking**: `Chest.amounts[i]` tracks items **inside** the shulker in slot `i`, not the shulker box itself. The shulker box is assumed to always be present.
- **Sticky assignment**: A chest's `item` field is set when items are first deposited and stays even if all 54 slots drain to zero. This avoids churn if the chest is about to be refilled; run "Repair state" in the CLI if you want to reclaim drained chests.

### Price Calculation Details (Constant Product AMM)

The store uses a **constant product automated market maker (AMM)** formula, similar to [Uniswap V2](https://docs.uniswap.org/contracts/v2/concepts/protocol-overview/how-uniswap-works). The key insight: **the product of reserves remains constant** after each trade (before fees).

#### The Constant Product Invariant

```
x × y = k

Where:
  x = item_stock (number of items in the pool)
  y = currency_stock (diamonds in the pool)
  k = constant product (only increases due to fees, never decreases)
```

#### Formulas

**Buying items** (player pays currency, receives items):

```
cost = currency_stock × quantity / (item_stock - quantity) × (1 + fee)
```

**Selling items** (player gives items, receives currency):

```
payout = currency_stock × quantity / (item_stock + quantity) × (1 - fee)
```

#### Why Slippage Occurs

The formula creates **price impact** (slippage) — the more you trade, the worse your per-item price becomes. This happens because:

1. As you buy items, you're removing them from the pool (reducing `item_stock`)
2. As `item_stock` decreases, the denominator `(item_stock - quantity)` shrinks
3. A smaller denominator means a larger result (higher cost)

This is **intentional** — it protects the pool from being drained in a single trade.

#### Example: Buying (with 12.5% fee)

Starting pool: **100 items**, **1000 diamonds** (k = 100,000)

| Quantity | Formula | Cost | Per-Item Price | Slippage |
|----------|---------|------|----------------|----------|
| 1 | 1000 × 1 / (100-1) × 1.125 | **11.36** | 11.36/item | baseline |
| 10 | 1000 × 10 / (100-10) × 1.125 | **125.00** | 12.50/item | +10% |
| 25 | 1000 × 25 / (100-25) × 1.125 | **375.00** | 15.00/item | +32% |
| 50 | 1000 × 50 / (100-50) × 1.125 | **1,125.00** | 22.50/item | +98% |
| 90 | 1000 × 90 / (100-90) × 1.125 | **10,125.00** | 112.50/item | +890% |
| 99 | 1000 × 99 / (100-99) × 1.125 | **111,375.00** | 1,125/item | +9,807% |

> [!NOTE]
> Notice how buying 99 out of 100 items costs 111,375 diamonds — **you can never buy the entire pool** because the cost approaches infinity.

#### Example: Selling (with 12.5% fee)

Starting pool: **100 items**, **1000 diamonds** (k = 100,000)

| Quantity | Formula | Payout | Per-Item Payout | vs Buying |
|----------|---------|--------|-----------------|-----------|
| 1 | 1000 × 1 / (100+1) × 0.875 | **8.66** | 8.66/item | (buy: 11.36) |
| 10 | 1000 × 10 / (100+10) × 0.875 | **79.55** | 7.95/item | (buy: 12.50) |
| 50 | 1000 × 50 / (100+50) × 0.875 | **291.67** | 5.83/item | (buy: 22.50) |

The spread between buy and sell prices is the **fee** (12.5%) plus the **slippage term**.

#### Key Properties

| Property | Description |
|----------|-------------|
| **Pool protection** | Cost → ∞ as quantity → item_stock (can never drain pool) |
| **k only increases** | Each trade adds fees to the pool, growing k over time |
| **Self-balancing** | High demand (many buys) → higher prices → incentivizes sells |
| **No admin pricing** | Prices are fully algorithmic based on supply/demand |

#### Fee Mechanics

| Trade Type | Fee Application | Effect |
|------------|-----------------|--------|
| **Buy** | `cost × (1 + fee)` | Player pays 12.5% more than base price |
| **Sell** | `payout × (1 - fee)` | Player receives 12.5% less than base payout |

The fee is **not taken as a separate line item** — it's built into the price. The fee portion is **added to both reserves**, increasing k.

#### Edge Cases

| Condition | Behavior |
|-----------|----------|
| `item_stock == 0` | Trading disabled — price calculation returns `None` |
| `currency_stock == 0` | Trading disabled — price calculation returns `None` |
| `quantity >= item_stock` | Buy rejected — cannot buy entire pool |
| `cost <= 0` or non-finite | Order rejected with "Internal error: computed price is invalid" |


### Bot Inventory Management

- **Hotbar slot 0 (36)**: Always reserved for shulker boxes. Bot ensures this slot is free before placing shulkers.
- **Inventory slots 9-35**: Used for items withdrawn from shulkers or received from trades.
- **Hotbar slots 36-44**: Automatically cleared after trades (items moved to inventory slots 9-35).
- **Buffer chest**: If `buffer_chest_position` is configured, bot can dump items here if inventory becomes full.

### Data Consistency

- **Autosave**: State is saved automatically when dirty, at most once every 2 seconds. Final save happens on shutdown.
- **Atomic writes**: All file writes use atomic operations (temp file + rename) to prevent corruption.
- **Invariant checking**: Store checks invariants before and after operations. Use "Audit state" to check for issues.
- **Repair command**: "Repair state" recomputes `pair.item_stock` from actual storage contents, fixing drift.

### Security Considerations

- **No coordinate disclosure**: Bot never reveals coordinates in chat messages (security requirement).
- **Operator-only commands**: `additem`, `removeitem`, `addcurrency`, `removecurrency` require operator status (set via CLI).
- **UUID-based identity**: All user operations use UUIDs (not usernames) as the canonical identifier. Usernames are updated on each interaction but don't affect identity. UUID lookups are cached in-memory with a 5-minute TTL to reduce Mojang API calls.
