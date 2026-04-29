# cj-store

> **Minecraft automated shop bot** — persistent state, constant-product AMM pricing, automated `/trade` fulfillment over a hand-built chest warehouse.

An in-game "store clerk" that handles trading via whisper commands. Once the
physical warehouse is in place and pairs are seeded, the bot runs the `/trade`
GUI, moves items in and out of shulker-backed storage, and writes every commit
atomically to JSON under `data/`.

## Core concepts

- **Pair** — an (item, diamonds) trading pool with reserves; pricing is AMM.
- **Node** — one standing position with 4 double chests; unit of storage expansion.
- **Chest / shulker** — 54-slot double chest, one shulker box per slot, bot tracks items-per-shulker.

## Docs

| Doc                                  | What's in it                                                                    |
| ------------------------------------ | ------------------------------------------------------------------------------- |
| [ARCHITECTURE.md](ARCHITECTURE.md)   | Runtime topology, trade state machine, AMM pricing, rollback, storage model    |
| [COMMANDS.md](COMMANDS.md)           | Player + operator whisper commands and the operator CLI menu                   |
| [DATA_SCHEMA.md](DATA_SCHEMA.md)     | Every JSON file under `data/` — config, pairs, users, storage, queue, journal  |
| [DEVELOPMENT.md](DEVELOPMENT.md)     | Build setup, error model, item IDs, testing, known limitations, perf tuning    |
| [RECOVERY.md](RECOVERY.md)           | Operator runbook — corrupted pairs, stuck journal, orphaned shulker, troubleshooting |

## What it does

- Serves player whispers (`/msg <bot> …`) — full command list in [COMMANDS.md](COMMANDS.md).
- Prices via **constant-product AMM** (`x × y = k`, Uniswap-style) so big trades pay slippage and pools can't drain.
- Fulfills trades physically: walks to chests, opens shulkers, uses the server's `/trade` GUI, deposits back — all commits atomic JSON under `data/`.

## Quick start

**Prereqs**

- Rust nightly (pinned via [`rust-toolchain.toml`](rust-toolchain.toml)).
- A Microsoft account with Minecraft that you are willing to log the bot in as.
- A Minecraft server you can log into and stand on — whitelists and
  permission plugins are fine, but the bot must be able to reach the
  `position` you'll configure and operate a vanilla `/trade` GUI.

**1. Build and create the config skeleton**

```bash
git clone <repo-url>
cd cj-store
cargo build --release

# First run writes data/config.json with placeholders and exits —
# account_email and server_address are required. Edit, then re-run.
cargo run --release
```

Edit [`data/config.json`](DATA_SCHEMA.md#dataconfigjson) — set at minimum
`account_email`, `server_address`, and `position` (where Node 0 lives in
the world). Validate without connecting:

```bash
cargo run -- --dry-run
```

**2. Build Node 0 in-world** at `config.position`. Layout is in
[ARCHITECTURE.md § Node layout](ARCHITECTURE.md#node-layout). Fill all 4
double chests with shulker boxes (54 per chest). The bot auto-manages its
own inventory and hotbar; no bot-side setup is needed beyond the physical
build.

**3. First run and seeding**

```bash
cargo run --release
```

In the CLI menu, **in this order** (operator status must come before you
send `addcurrency` / `additem` whispers):

1. Option 3: **Set operator status** on your Minecraft username.
2. Option 5: **Add node (with bot validation)** for Node 0.
3. Option 8: **Add pair** for each item you want to trade.
4. In-game whispers: `addcurrency <item> <diamonds>` to seed the diamond
   reserve, `additem <item> <qty>` to seed the physical stock.

Players can then:

```text
/msg <botname> items
/msg <botname> price cobblestone
/msg <botname> buy cobblestone 64
/msg <botname> help
```

## Feature status

What's shipped is described across the other docs. Things that are **not yet
implemented** and that someone reading the code might expect:

- Automatic crash-resume. Today the bot *detects* an interrupted trade or
  chest op on startup, logs it, and clears the journal; reconciling the
  world and ledger is an operator task (see [RECOVERY.md](RECOVERY.md)).
  Planned behavior: [ARCHITECTURE.md § Planned: automatic crash-resume](ARCHITECTURE.md#planned-automatic-crash-resume).
- Order books / limit orders, multi-item trades, statistics.

See [DEVELOPMENT.md § Known limitations](DEVELOPMENT.md#known-limitations)
for the full list of things that are intentionally not handled.

## Security

> [!CAUTION]
> **The bot must never reveal storage coordinates in chat.** All
> player-facing messages are coordinate-free by design. If you extend
> the bot, keep this invariant.

Operator-only commands (`additem`, `removeitem`, `addcurrency`,
`removecurrency`) require `operator: true` on the user record — set via
CLI option 3. All user operations are keyed on Mojang UUID, not username.

**Credentials.** `data/config.json` stores the Microsoft account *email*
— not a password. Azalea signs in via Microsoft's OAuth device-code flow
and caches the refresh token under the OS's standard Minecraft auth
path (outside this repo), so `data/` contains no secrets. `data/users/`
holds Mojang UUIDs and last-seen usernames only. Economic state
(balances, reserves, trade history) in `data/` is still sensitive —
`.gitignore` it before publishing.

## License

See [LICENSE.md](LICENSE.md).
