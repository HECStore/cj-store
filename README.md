# cj-store

> **Minecraft automated shop/exchange bot** — persistent state, constant-product AMM pricing, fully automated `/trade` fulfillment.

A feature-complete in-game "store clerk" that handles trading via whisper
commands, with durable on-disk JSON state and physical chest-storage
integration.

## Docs

| Doc                                  | What's in it                                                                    |
| ------------------------------------ | ------------------------------------------------------------------------------- |
| [ARCHITECTURE.md](ARCHITECTURE.md)   | Runtime topology, trade state machine, AMM pricing, rollback, storage model    |
| [COMMANDS.md](COMMANDS.md)           | Player + operator whisper commands and the 15-option CLI menu                  |
| [DATA_SCHEMA.md](DATA_SCHEMA.md)     | Every JSON file under `data/` — config, pairs, users, storage, queue, journal  |
| [DEVELOPMENT.md](DEVELOPMENT.md)     | Build setup, error model, item IDs, testing, known limitations, perf tuning    |
| [RECOVERY.md](RECOVERY.md)           | Operator runbook — corrupted pairs, stuck journal, orphaned shulker, troubleshooting |

## What it does

- Listens for `/msg <bot> <command>` whispers from players
- Runs **buy** / **sell** / **price** / **balance** / **pay** /
  **deposit** / **withdraw** / **queue** / **cancel** / **status** /
  **help** / **items** — full details in [COMMANDS.md](COMMANDS.md)
- Prices via **constant-product AMM** (`x × y = k`, Uniswap-style) so
  larger trades pay slippage and the pool can never be drained
- Fulfills trades physically: walks to chests, extracts items from
  shulker boxes, uses the server's `/trade` GUI, deposits received items
  back into storage
- Persists every commit atomically to JSON under `data/`
- Operator menu (CLI) for balances, pairs, nodes, audit/repair, restart

## Quick start

**Prereqs**: Rust nightly (pinned via `rust-toolchain.toml`), a Microsoft
account with Minecraft, access to a Minecraft server.

```bash
git clone <repo-url>
cd cj-store
cargo build --release

# First run creates data/config.json and fails on auth — expected.
cargo run --release

# Edit data/config.json — you MUST set account_email and server_address.
# Validate without connecting:
cargo run -- --dry-run
```

**Build Node 0 in-world** at the `position` from `config.json` (layout in
[ARCHITECTURE.md § Node layout](ARCHITECTURE.md#node-layout)), fill all 4
double chests with shulker boxes (54 per chest). The bot auto-manages its
own inventory (keeping hotbar slot 0 clear for shulker handling); no
bot-side setup is needed beyond the physical build. Then:

```bash
cargo run --release
```

From the CLI: add nodes (option 5, validated), add pairs (option 8), set
yourself operator (option 3), fund pairs via in-game `addcurrency` +
`additem` whispers.

Players can then:

```text
/msg <botname> items
/msg <botname> price cobblestone
/msg <botname> buy cobblestone 64
/msg <botname> help
```

## Feature status

**Implemented**

- Persistent schemas for users, pairs, orders, trades, storage
- All player + operator commands listed above
- Trade GUI automation (`/trade`) with timeouts and rollback
- Storage-backed fulfillment with automated shulker-box I/O
- Pathfinding and spiral node layout
- Constant-product AMM pricing with slippage
- Transactional buy/sell with rollback on failure
- Persistent FIFO order queue (8 per user, 128 global)
- Anti-spam rate limiter with exponential backoff (2 s base, 60 s cap)
- Crash-resume detection for in-flight trades and chest ops
- Autosave (debounced + atomic writes)
- Hot-reloading `config.json` for safe fields (see
  [DATA_SCHEMA.md § Hot-reload matrix](DATA_SCHEMA.md#hot-reload-matrix))

**Future**

- Order books / limit orders
- Multi-item trades
- Statistics and analytics

## Security

> [!CAUTION]
> **The bot must never reveal storage coordinates in chat.** All
> player-facing messages are coordinate-free by design. If you extend
> the bot, keep this invariant.

Operator-only commands (`additem`, `removeitem`, `addcurrency`,
`removecurrency`) require `operator: true` on the user record — set via
CLI option 3. All user operations are keyed on Mojang UUID, not username.

## License

See [LICENSE.md](LICENSE.md).
