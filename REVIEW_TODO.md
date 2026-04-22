# Code Review TODO

Checklist of every reviewable item in the `src/` tree. Check items off as they are reviewed.
For each item, look for: bugs, logic errors, missing error handling, unclear invariants,
dead code, API surface issues, and gaps against the behavior documented in the `.md` files.

**Review workflow:** one file at a time. After finishing a file, wait for approval before
starting the next. Any code change that affects claims in `ARCHITECTURE.md`, `DATA_SCHEMA.md`,
`COMMANDS.md`, `DEVELOPMENT.md`, `RECOVERY.md`, or `README.md` must be mirrored into those docs.

---

## Root & top-level modules

### src/main.rs
- [x] fn `main`
- [x] fn `print_usage`
- [x] fn `run_validate_only`
- [x] fn `spawn_config_watcher`

**Review findings and fixes applied:**

1. **Exit code hid runtime errors** (`main`, [src/main.rs:147-175](src/main.rs#L147-L175)) — all match arms fell through to `Ok(())`, so systemd/CI saw exit code 0 even when a task crashed. **Fixed:** track `had_error` flag, keep the log-flush sleep, then `std::process::exit(1)` on failure.
2. **Redundant `Config::load` at startup** (`main`, [src/main.rs:110-117](src/main.rs#L110-L117)) — config was loaded once inside `Store::new` and again in `main`. **Fixed:** snapshot needed fields from `store.config` before moving `store` into `run`.
3. **`--validate-only` omitted `buffer_chest_position`** (`run_validate_only`, [src/main.rs:191-194](src/main.rs#L191-L194)) — every other config field was printed. **Fixed:** added a `Some(p)` / `None` branch that prints coords or `<none>`.
4. **Config watcher could silently replace a deleted config with defaults** (`spawn_config_watcher`, [src/main.rs:249-255](src/main.rs#L249-L255)) — `Config::load` auto-writes a default when the file is missing; if a future change ever observed Remove events, an operator's config would be overwritten. **Fixed:** guard the reload path with an explicit `Path::new(...).exists()` check.
5. `print_usage` — no issues.

**Build:** `cargo build` clean after fixes.
**Doc impact:** None. `DATA_SCHEMA.md:39-42` already documents `--validate-only` exit codes (0/1); that path is unchanged. No doc enumerates the printed fields or runtime exit codes.

### src/cli.rs
- [x] fn `with_retry`
- [x] fn `cli_task`
- [x] fn `get_balances`
- [x] fn `get_pairs`
- [x] fn `set_operator`
- [x] fn `add_node`
- [x] fn `add_node_with_validation`
- [x] fn `discover_storage`
- [x] fn `remove_node`
- [x] fn `add_pair`
- [x] fn `remove_pair`
- [x] fn `view_storage`
- [x] fn `view_trades`
- [x] fn `restart_bot`
- [x] fn `clear_stuck_order`
- [x] fn `audit_state`

**Review findings and fixes applied:**

1. **`get_pairs` silently displayed wrong prices when fee query failed** ([src/cli.rs:161-183](src/cli.rs#L161-L183)) — the fallback to hardcoded `0.125` (the default fee) was applied without any log output. If an operator's configured fee differs (e.g. 0.05), the CLI would display materially wrong prices with no indication the query had failed. **Fixed:** added `warn!` on both the send-fail and recv-fail paths, and named the fallback as a `const DEFAULT_FEE_FALLBACK` so it isn't a scattered magic number.

**Observations (not bugs, not fixed):**
- `with_retry` has no max-attempt cap ([src/cli.rs:22-32](src/cli.rs#L22-L32)) — acceptable for an interactive operator loop; Ctrl+C is the escape hatch.
- `get_pairs` computes buy/sell as `mid * (1±fee)` ([src/cli.rs:200-211](src/cli.rs#L200-L211)) — this is an AMM 1-unit-price approximation, not the exact `buy_cost_pure` / `sell_payout_pure` formula used at execution. Close enough for an operator price-quote display; labelling is fine.
- Menu numeric indices are hardcoded and coupled to the `options` vec order ([src/cli.rs:44-111](src/cli.rs#L44-L111)) — a cross-reference comment already flags the coupling; acceptable.
- Hardcoded "up to 2 minutes" in `add_node_with_validation` ([src/cli.rs:293](src/cli.rs#L293)) — matches the documented estimate in `COMMANDS.md:97`, so intentional and consistent.
- Menu option labels (1-indexed: 4 = "Add node (no validation)", 5 = "Add node (with bot validation)", 12 = "Audit state", 13 = "Repair state") match `README.md`, `COMMANDS.md`, `DATA_SCHEMA.md`, and `RECOVERY.md`. No drift.
- `view_trades` `TradeType` match is exhaustive (no `_` arm), so a new variant would be a compile error — correct forward-guard.

**Build:** `cargo build` clean after fix.
**Doc impact:** None. No doc claims the fee-query fallback is silent or states a specific log-warning contract.

### src/config.rs
- [x] struct `Config`
- [x] fn `default_trade_timeout_ms`
- [x] fn `default_pathfinding_timeout_ms`
- [x] fn `default_max_orders`
- [x] fn `default_max_trades_in_memory`
- [x] fn `default_autosave_interval_secs`
- [x] impl Config :: fn `validate`
- [x] impl Config :: fn `load`

**Review findings and fixes applied:**

1. **Typos in `data/config.json` silently loaded as defaults** (`struct Config`, [src/config.rs:46-48](src/config.rs#L46-L48)) — `DATA_SCHEMA.md:368` already called this out as a footgun. Since `Config` is the only hand-edited JSON in the project (all other types are bot-written), it's the right place to enforce strictness. A typo like `"fe": 0.125` would previously fall through to the serde default with no warning. **Fixed:** added `#[serde(deny_unknown_fields)]`. **Doc updated:** `DATA_SCHEMA.md:368-373` — now explains Config is strict while bot-written files intentionally aren't (to preserve forward-compat reads).

**Observations (not bugs, not fixed):**
- `default_trade_timeout_ms` (45_000) and `default_pathfinding_timeout_ms` (60_000) duplicate unused `constants::TRADE_TIMEOUT_MS` / `constants::PATHFINDING_TIMEOUT_MS` ([src/constants.rs:45,61](src/constants.rs#L45)) — the constants are defined but referenced nowhere else in the codebase. Defer the dedupe decision to the `constants.rs` review (where either deletion or cross-referencing is the natural action).
- `validate` uses `eprintln!` for empty-email and out-of-range-Y warnings ([src/config.rs:117,174](src/config.rs#L117)) instead of `tracing::warn!` — on first run this happens before tracing is initialized (println is the only channel then), but on hot-reload via the watcher the tracing subsystem is up and a warn! would route to the log file. Minor inconsistency; not worth a targeted fix without wider logging-policy alignment.
- `validate` server_address check accepts leading-colon forms like `":25565"` (empty host) ([src/config.rs:147-158](src/config.rs#L147-L158)) — `rsplit_once(':')` splits once, port parses fine, no explicit empty-host check. Unlikely in practice; skipped to avoid rule-churn.
- `load` does not re-read the file after writing the default on first run ([src/config.rs:232-264](src/config.rs#L232-L264)) — uses the in-memory struct directly. Correct behavior (deterministic) and strictly faster; noted only for completeness.
- `fee` NaN/∞ path: range check runs first, which NaN passes (both comparisons are false), then finiteness catches it on the next line ([src/config.rs:100-108](src/config.rs#L100-L108)). Reordering would be cosmetic — the accumulated-errors design handles either order correctly.

**Build:** `cargo build` clean. **Tests:** all 113 passing after the change.
**Doc impact:** `DATA_SCHEMA.md:368-373` updated to reflect the new `deny_unknown_fields` on Config (and explicitly document why it stays off for bot-written JSON).

### src/constants.rs
- [x] const `DOUBLE_CHEST_SLOTS`
- [x] const `SHULKER_BOX_SLOTS` (kept as canonical; duplicated locally in pair.rs — flagged for that review)
- [x] const `DEFAULT_STACK_SIZE` (deleted — dead code, never referenced)
- [x] const `HOTBAR_SLOT_0` (kept as canonical; duplicated locally in inventory.rs/chest_io.rs — flagged for those reviews)
- [x] const `INVENTORY_SLOT_START` (deleted — dead code)
- [x] const `INVENTORY_SLOT_END` (deleted — dead code)
- [x] const `CHEST_OPEN_TIMEOUT_TICKS` (deleted — dead code)
- [x] const `TRADE_TIMEOUT_MS` (now referenced from config default)
- [x] const `TRADE_WAIT_TIMEOUT_MS` (deleted — dead code)
- [x] const `CHEST_OP_TIMEOUT_SECS`
- [x] const `PATHFINDING_TIMEOUT_MS` (now referenced from config default)
- [x] const `CLIENT_INIT_TIMEOUT_MS` (deleted — dead code)
- [x] const `DELAY_SHORT_MS`
- [x] const `DELAY_MEDIUM_MS`
- [x] const `DELAY_INTERACT_MS`
- [x] const `DELAY_BLOCK_OP_MS`
- [x] const `DELAY_LOOK_AT_MS`
- [x] const `DELAY_SETTLE_MS`
- [x] const `DELAY_NETWORK_MS` (deleted — dead code)
- [x] const `DELAY_VALIDATION_BETWEEN_CHESTS_MS`
- [x] const `DELAY_SHULKER_PLACE_MS`
- [x] const `DELAY_DISCONNECT_MS`
- [x] const `DELAY_CONFIG_DEBOUNCE_MS`
- [x] const `DELAY_DISCONNECT_BUFFER_MS` (deleted — dead code)
- [x] const `RECONNECT_INITIAL_BACKOFF_SECS` (deleted — dead code)
- [x] const `RECONNECT_MAX_BACKOFF_SECS` (deleted — dead code)
- [x] const `CONNECTION_CHECK_INTERVAL_SECS` (deleted — dead code)
- [x] const `CHEST_OP_MAX_RETRIES`
- [x] const `CHUNK_RELOAD_EXTRA_RETRIES`
- [x] const `CHUNK_RELOAD_BASE_DELAY_MS`
- [x] const `CHUNK_RELOAD_MAX_DELAY_MS`
- [x] const `SHULKER_OP_MAX_RETRIES`
- [x] const `NAVIGATION_MAX_RETRIES`
- [x] const `RETRY_BASE_DELAY_MS`
- [x] const `RETRY_MAX_DELAY_MS`
- [x] const `FEE_MIN`
- [x] const `FEE_MAX`
- [x] const `MAX_TRANSACTION_QUANTITY`
- [x] const `MIN_RESERVE_FOR_PRICE`
- [x] const `CHESTS_PER_NODE` (now referenced from node.rs)
- [x] const `NODE_SPACING` (now referenced from node.rs)
- [x] const `OVERFLOW_CHEST_ITEM`
- [x] const `DIAMOND_CHEST_ID`
- [x] const `OVERFLOW_CHEST_ID`
- [x] const `MAX_ORDERS_PER_USER`
- [x] const `MAX_QUEUE_SIZE`
- [x] const `QUEUE_FILE`
- [x] const `RATE_LIMIT_BASE_COOLDOWN_MS`
- [x] const `UUID_CACHE_TTL_SECS`
- [x] const `RATE_LIMIT_MAX_COOLDOWN_MS`
- [x] const `RATE_LIMIT_RESET_AFTER_MS`
- [x] const `CLEANUP_INTERVAL_SECS`
- [x] const `RATE_LIMIT_STALE_AFTER_SECS`
- [x] fn `exponential_backoff_delay`

**Review findings and fixes applied:**

1. **`#![allow(dead_code)]` was masking 15 unused constants** ([src/constants.rs:6](src/constants.rs#L6)) — the crate-wide attribute silenced all dead-code warnings, hiding genuine rot. **Fixed:** removed the blanket attribute; narrowed to per-item `#[allow(dead_code)]` on the two constants that represent canonical Minecraft protocol values but are currently shadowed by local duplicates (to be resolved in the reviews of inventory.rs / chest_io.rs / pair.rs).
2. **Deleted 10 genuinely dead constants** — `DEFAULT_STACK_SIZE`, `INVENTORY_SLOT_START`, `INVENTORY_SLOT_END`, `CHEST_OPEN_TIMEOUT_TICKS`, `TRADE_WAIT_TIMEOUT_MS`, `CLIENT_INIT_TIMEOUT_MS`, `DELAY_NETWORK_MS`, `DELAY_DISCONNECT_BUFFER_MS`, `RECONNECT_INITIAL_BACKOFF_SECS`, `RECONNECT_MAX_BACKOFF_SECS`, `CONNECTION_CHECK_INTERVAL_SECS`. None were referenced anywhere in the codebase; several reflected planned features (reconnection logic) that never landed. Removing them makes the file's reachable surface the actual surface.
3. **`TRADE_TIMEOUT_MS` / `PATHFINDING_TIMEOUT_MS` duplicated the config defaults as raw literals** ([src/config.rs:81-82](src/config.rs#L81-L82)) — same values (45_000 / 60_000) appeared in both places with no link. A future retune of either would silently drift. **Fixed:** config's `default_trade_timeout_ms` / `default_pathfinding_timeout_ms` now return the constants directly; added a short comment at the constants flagging them as the canonical defaults.
4. **`NODE_SPACING` (3) hardcoded in [src/types/node.rs:278,280](src/types/node.rs#L278) as the raw literal `3`; `CHESTS_PER_NODE` (4) hardcoded at [src/types/node.rs:66](src/types/node.rs#L66) as the raw literal `4`** — the constants existed but the geometry code used magic numbers. Any change to the layout would miss these sites. **Fixed:** imported both into node.rs and wired them into `calc_position` (spacing multiplier) and `Node::new` (chest vector capacity). Doc block already says "Spaced **3 blocks** apart"; ARCHITECTURE.md:230 already references `CHESTS_PER_NODE = 4`, now truthful.

**Observations (not fixed):**
- `HOTBAR_SLOT_0` and `SHULKER_BOX_SLOTS` duplicated as function/impl-local constants in `inventory.rs`, `chest_io.rs`, and `pair.rs` — fixing would touch three files not yet reviewed. Deferred to those reviews; kept canonical versions in constants.rs with per-item `#[allow(dead_code)]` and comments pointing to the duplicates.
- `exponential_backoff_delay` shift-amount clamp at `attempt.min(10)` ([src/constants.rs:175](src/constants.rs#L175)) — correct defensive bound; a shift >63 would be UB on `u64`, and 2^10 × any realistic `base_ms` already exceeds any `max_ms` we'd configure.
- Retry/chunk-reload constants (`CHEST_OP_MAX_RETRIES`, `CHUNK_RELOAD_*`, etc.) all have external references; retained.

**Build:** `cargo build` clean (zero warnings). **Tests:** all 113 passing.
**Doc impact:** None needed. `ARCHITECTURE.md:230` (`CHESTS_PER_NODE = 4`) is now backed by real code references; no other doc cited the deleted constants by name.

### src/error.rs
- [x] enum `StoreError`
- [x] impl `From<StoreError> for String` :: fn `from`
- [x] impl `From<String> for StoreError` :: fn `from`

**Review findings and fixes applied:** None — no bugs or rot found.

**Observations (not fixed):**
- **5 of 14 `StoreError` variants are unconstructed:** `ItemNotFound`, `InsufficientFunds`, `InsufficientStock`, `PlanInfeasible`, `QueueFull`. The module's own docstring frames the enum as an aspirational migration target ("Migration is progressive: new code should prefer `StoreError`"), so unused variants represent future categories, not dead code. No compiler warnings fire because pub enum variants in a binary crate are reachable-by-construction. Kept as-is; not premature abstraction because each variant names a real domain condition the codebase already handles (via `ValidationError(String)` or raw strings today).
- **`From<String> for StoreError` maps every legacy string to `ValidationError`** ([src/error.rs:78-82](src/error.rs#L78-L82)) — documented trade-off for incremental migration. A caller matching on `StoreError::ValidationError` will over-match legacy errors of other true categories (bot failure, IO, etc.). Acceptable during migration; should be revisited once the conversion is complete (remove this impl, force explicit variant construction).
- **`Io(#[from] std::io::Error)`** — correct thiserror auto-conversion; lets `?` in any `StoreError`-returning function consume `io::Result` without manual mapping.
- **`UnknownPair` / `UnknownUser` carry `context: &'static str`** — static-lifetime constraint keeps constructors zero-cost (string literals at call sites), consistent with how they're already used.
- **`InvariantViolation(String)` renders with `#[error("{0}")]` and no prefix** — relies on callers to prefix their messages. Could use a prefix like "Invariant: " for log-grep convenience, but the variant is only constructed once in the codebase and that call site already writes the full sentence.

**Build:** `cargo build` clean. **Tests:** all 113 passing (unchanged — no code edits).
**Doc impact:** None. No .md file references `StoreError` variants by name.

### src/fsutil.rs
- [x] fn `write_atomic`

**Review findings and fixes applied:** None — the function is visibly hardened against real-world Windows quirks (AV scan locks, cross-volume renames, long-path issues). Not making cosmetic changes to a path that gets hit from 20 call sites.

**Observations (not fixed):**
- **Atomicity is "best-effort", documented** ([src/fsutil.rs:19-21](src/fsutil.rs#L19-L21)) — the rename path is atomic; the copy-fallback path is not (destination can briefly exist partial). Accepted trade-off: "preferable to losing the write entirely". No parent-directory `fsync` after rename, so a crash immediately after rename could lose the name flip on POSIX. Not a bug, just a durability ceiling worth knowing.
- **Redundant defensive checks** ([src/fsutil.rs:47-54,70-75](src/fsutil.rs#L47-L54)) — `path.to_string_lossy().is_empty()` can't fire because `file_name()` above already rejected empty/invalid paths, and the post-`File::create` `tmp_path.exists()` check is impossible to hit if the preceding `sync_all` returned `Ok`. Harmless belt-and-suspenders; not worth touching a hardened function for.
- **Very verbose tracing** — ~10–15 `debug!` lines per write on the happy path; at the default 2-second autosave interval this fills logs fast. Almost certainly retained deliberately to diagnose the intermittent Windows rename failures that produced the fallback cascade in the first place. Would collapse to 2 lines if log volume ever becomes a concern, but don't simplify until it is.
- **Temp filename is `{file}.tmp`, not unique** ([src/fsutil.rs:42](src/fsutil.rs#L42)) — safe because every `write_atomic` call for a given path is serialized through the single-owner actor that writes it (Store for data files, main for config). A crash mid-write leaves a stale `.tmp` that the next write to the same path truncates via `File::create`. No leak.
- **All synthesized errors use `io::ErrorKind::Other`** — `AlreadyExists` / `PermissionDenied` would be more specific, but the detail in the message strings already captures what went wrong, and none of the call sites match on `ErrorKind`. Not worth classifying.
- **No tests** — the fallback cascade (rename → copy → remove+copy → error) is complex enough to deserve unit coverage. Skipped because simulating Windows rename failure reliably in a portable test is awkward; worth revisiting if this function is ever modified again.

**Build:** `cargo build` clean. **Tests:** all 113 passing (unchanged).
**Doc impact:** None. `DATA_SCHEMA.md` already refers to "atomic write (write-to-temp + rename)" which accurately describes the happy path.

### src/messages.rs
- [x] struct `TradeItem`
- [x] struct `ChestSyncReport`
- [x] enum `QueuedOrderType`
- [x] enum `ChestAction`
- [x] enum `StoreMessage`
- [x] enum `BotMessage`
- [x] enum `CliMessage`
- [x] enum `BotInstruction`

**Review findings and fixes applied:**

1. **Orphaned doc comment on `BotInstruction::InteractWithChestAndSync`** ([src/messages.rs:221-223](src/messages.rs#L221-L223)) — the variant carried two doc lines: `/// Send a public chat message.` followed by the real `/// Navigate to chest...`. Grep confirms there is no `SendChatMessage` / `SendChat` / any chat-send `BotInstruction` variant anywhere in the tree, so the first line is a leftover from a removed variant now silently misattributed. Anyone reading `InteractWithChestAndSync` would see "Send a public chat message" as its primary description. **Fixed:** removed the stray line; the variant's doc now correctly describes only the navigate-and-interact behavior.
2. **Ambiguous length description on `ChestSyncReport.amounts`** ([src/messages.rs:60-63](src/messages.rs#L60-L63)) — the old comment said "length 54, one per shulker box slot". 54 is the double-chest slot count; a shulker box has 27 slots. Reading "per shulker box slot" naturally suggests 27, not 54 — the grammar was load-bearing in the wrong direction. **Fixed:** rephrased to make it explicit that the vec is indexed by chest slot, and each entry aggregates the shulker box that slot holds.

**Observations (not fixed):**
- **None of `StoreMessage` / `BotMessage` / `CliMessage` / `BotInstruction` derive Debug** — deriving would fail because `oneshot::Sender<T>` is not Debug. A manual impl that skips the sender would unlock `{:?}` logging, which would be useful for diagnostic traces. Non-trivial (one impl per enum) and no site currently tries to log a whole message, so skipped.
- **Wire types use `String` for item identifiers** (`TradeItem`, `ChestSyncReport`, `ChestAction`) rather than the `ItemId` newtype from `types/item_id.rs` — deliberate: these cross task boundaries and `ItemId` adds no value at the wire level. Consistent with how the rest of the cross-task messages are shaped.
- **`ChestSyncReport` has Debug+Clone but no Serialize/Deserialize** — it's a transient in-process message, never persisted. Correct to keep non-serializable.
- **`BotInstruction::Restart` is fire-and-forget** (no `respond_to`) — explicitly documented at [src/messages.rs:266-270](src/messages.rs#L266-L270). Correct because the original sender no longer exists after the bot task is torn down and respawned.

**Build:** `cargo build` clean. **Tests:** all 113 passing (doc-only edits).
**Doc impact:** None — `.md` files describe message semantics at a higher level and reference neither the orphaned chat-send variant nor the `amounts` length directly.

### src/types.rs
- [x] module re-export surface (verify nothing leaks / nothing missing)

**Review findings and fixes applied:**

1. **Stale comment + unnecessary `#[allow(unused_imports)]` on `pub use node::Node`** ([src/types.rs:39-43](src/types.rs#L39-L43)) — the comment claimed "`Node` is accessed through `storage::Node` in most of the codebase and only referenced via this re-export from tests". Both halves are false: `grep "storage::Node"` returns zero hits outside types.rs itself, and `crate::types::Node::calc_position(...)` is called from four production sites ([src/bot/mod.rs:540](src/bot/mod.rs#L540), [src/store/utils.rs:155](src/store/utils.rs#L155), [src/store/handlers/cli.rs:125](src/store/handlers/cli.rs#L125), [src/store/handlers/cli.rs:381](src/store/handlers/cli.rs#L381)). The suppression is therefore inert and the comment would mislead future maintainers into thinking the re-export is vestigial. **Fixed:** dropped the comment and the attribute; `pub use node::Node;` now reads like every sibling re-export.

**Observations (not fixed):**
- **Submodules are all `pub mod`**, so direct paths like `crate::types::node::Node` remain accessible alongside the convenience re-exports. `storage.rs:47` uses the submodule path (`use crate::types::node::Node;`); every other call site uses the re-export. Two paths for one type is a little untidy but harmless; not changing visibility on unreviewed modules.
- **Re-export surface covers every item the code actually reaches for** via `crate::types::X`: grep confirms `Chest`, `ItemId`, `Node`, `Order`, `Pair`, `Position`, `Storage`, `Trade`, `TradeType`, `User` are all used through this module. Nothing missing, nothing dead.
- **Module docstring lists types in a different order than the `pub use` block** — cosmetic only; readability of the prose order wins over matching the import order.
- **No `#[cfg(test)]`-only re-exports** — consistent with the pattern of keeping test-only symbols private to their defining modules.

**Build:** `cargo build` clean (zero warnings). **Tests:** unchanged (no logic edits; doc-surface only).
**Doc impact:** None. No `.md` references the `#[allow(unused_imports)]` line or the stale comment text.

---

## types/

### src/types/position.rs
- [x] struct `Position`

**Review findings and fixes applied:** None — 28-line file, a single 3-field POD struct with appropriate derives and a correct, minimal doc block.

**Observations (not fixed):**
- **Derives `Debug, PartialEq, Serialize, Deserialize, Default, Clone, Copy`** — all warranted: `Copy` is fine for 12 bytes (3 × i32), `Default` is exercised by `Config::load` and `store/utils.rs`, `PartialEq` is used by tests, and the serde pair lets positions round-trip through config and storage JSON.
- **Missing `Eq` / `Hash`** — would be valid (all fields are `i32`) but unnecessary: grep for `HashMap<Position`, `BTreeMap<Position`, `HashSet<Position` returns zero hits. Not worth adding derives on speculation; easy to add later if a feature wants to key on Position.
- **No `PartialOrd`** — correct omission; a lexicographic total order on 3D coordinates has no geometric meaning and would invite misuse.
- **No validation on the struct itself** — coordinate bounds are enforced at the Config boundary ([src/config.rs:166-179](src/config.rs#L166-L179)); the bare type stays a dumb value container, which is the right layering.
- **Field ordering** matches Minecraft conventions (x, y, z with y as vertical), consistent with the `Position {x, y, z}` shorthand used throughout the codebase.

**Build:** `cargo build` clean (unchanged — no edits). **Tests:** unchanged.
**Doc impact:** None.

### src/types/item_id.rs
- [x] struct `ItemId`
- [x] impl ItemId :: const `EMPTY`
- [x] impl ItemId :: fn `new`
- [x] impl ItemId :: fn `from_normalized`
- [x] impl ItemId :: fn `as_str`
- [x] impl ItemId :: fn `with_minecraft_prefix`
- [x] impl ItemId :: fn `is_empty`
- [x] impl `Deref for ItemId` :: fn `deref`
- [x] impl `Borrow<str> for ItemId` :: fn `borrow`
- [x] impl `AsRef<str> for ItemId` :: fn `as_ref`
- [x] impl `Display for ItemId` :: fn `fmt`
- [x] impl `PartialEq<str> for ItemId` :: fn `eq`
- [x] impl `PartialEq<&str> for ItemId` :: fn `eq`
- [x] impl `PartialEq<String> for ItemId` :: fn `eq`
- [x] impl `From<ItemId> for String` :: fn `from`
- [x] impl `Default for ItemId` :: fn `default`

**Review findings and fixes applied:**

1. **False "always lowercase" claim in the `ItemId` struct doc** ([src/types/item_id.rs:22](src/types/item_id.rs#L22)) — the struct docstring asserted "The inner value is always lowercase and prefix-free" but nothing in `ItemId::new`, `ItemId::from_normalized`, or the sibling `store::utils::normalize_item_id` applies any casing transform. A caller passing `ItemId::new("Diamond")` gets an `ItemId("Diamond")` and would reasonably expect lowercase-normalization per the doc; the hash/borrow impls would then miss matches against the `"diamond"` entries that dominate real data. **Fixed:** dropped the lowercase claim; doc now says "Case is preserved as given — Minecraft item IDs are lowercase by convention but this type does not enforce casing." `to_lowercase()` inside `new` would also be a defensible change but requires a round-trip audit of serialized JSON (the entire [data/](data/) tree) before flipping, which is outside a doc-surface review.

**Observations (not fixed):**
- **`ItemId::new` has zero production call sites** — every non-test construction goes through `ItemId::from_normalized(item.to_string())` after `store::utils::normalize_item_id` has already stripped the prefix (40+ sites across store/ and bot/). The type's "prefix normalization on construction" guarantee therefore depends on callers remembering to invoke the separate normalizer first. Consolidating onto `ItemId::new` would be a genuine improvement (fewer ways to get it wrong, one canonical entry point) but touches dozens of files not yet reviewed. Flagged for later.
- **`store::utils::normalize_item_id` duplicates the prefix-strip logic** from `ItemId::new`. Two normalizers — one returning `String`, one returning `Result<ItemId>` — are maintained in parallel. After unifying call sites, the utility version becomes redundant.
- **`ItemId::EMPTY` intentionally bypasses the non-empty invariant** — documented sentinel for "no item assigned" chest slots. Correct design, but combined with `from_normalized` (which also skips validation) there are two escape hatches. The non-empty invariant therefore only holds for values that went through `new`, which as noted above is the test path only.
- **`Default` returns `EMPTY`**, consistent with the pre-ItemId convention of using `""` for unassigned. Correct.
- **`PartialEq<str>` / `PartialEq<&str>` / `PartialEq<String>` are asymmetric** — only `ItemId == str` works, not `str == ItemId`. Not a bug (call sites consistently write `id == "literal"`), just worth knowing if a future macro expects symmetry.
- **`#[serde(transparent)]` preserves JSON wire compatibility** — critical because pre-existing `data/pairs/*.json`, `data/storage/*.json`, `data/trades/*/*.json` all store items as bare strings. A non-transparent representation would have required a migration. Correctly documented in the module docstring.
- **`Hash + Eq` derives** are load-bearing: `ItemId` is keyed in `HashMap<ItemId, _>` in storage/chest tracking. The `Borrow<str>` impl lets lookups use `&str` without constructing a temporary `ItemId`.
- **`from_normalized` takes `String`, not `&str`** — avoids a clone at call sites that already own a `String`. Opposite of `new(&str)` which intentionally copies because the prefix-strip may shorten.
- **No `FromStr`** — would be nice (enables `"diamond".parse::<ItemId>()`) but not needed by current call sites.

**Build:** `cargo build` clean (zero warnings). **Tests:** all 113 passing (doc-only edit).
**Doc impact:** None. No `.md` file references the "lowercase" claim or `ItemId`'s internal casing guarantees.

### src/types/node.rs
- [x] struct `Node`
- [x] impl Node :: fn `new`
- [x] impl Node :: fn `load`
- [x] impl Node :: fn `save`
- [x] impl Node :: fn `calc_position`
- [x] impl Node :: fn `calc_chest_position`

**Review findings and fixes applied:**

1. **`4` hardcoded alongside `CHESTS_PER_NODE` in `Node::new`** ([src/types/node.rs:69](src/types/node.rs#L69)) — the `Vec::with_capacity(CHESTS_PER_NODE)` on line 67 and the `for index in 0..4` loop on line 69 described the same layout constant twice, only one of them via the canonical name. If `CHESTS_PER_NODE` ever changed, the capacity would track but the loop would silently still build exactly 4 chests. **Fixed:** loop now iterates `0..CHESTS_PER_NODE as i32`.
2. **`4` hardcoded in `Node::load` length check** ([src/types/node.rs:136-138](src/types/node.rs#L136-L138)) — same issue in the post-deserialize validation: `if node.chests.len() != 4 { ... "expected 4" ... }`. **Fixed:** compare against `CHESTS_PER_NODE`, interpolate the constant in the error message.
3. **Inaccurate "starting at 0" comment on `pos_in_ring`** ([src/types/node.rs:254](src/types/node.rs#L254)) — the comment claimed `pos_in_ring` was 0-indexed. It is not: ring 1's first id (id=1) gives `pos_in_ring = 1 - 0 = 1`, and ring 2's first id (id=9) gives `pos_in_ring = 9 - 8 = 1`. Every ring's first node maps to pos_in_ring=1, not 0. The side-selection arithmetic below compensates, so the formula works — but a reader trying to verify the spiral by hand using the comment's mental model would compute wrong coordinates. **Fixed:** replaced the one-liner with a block explaining the 1-indexed convention (first id → 1, last id → 8*ring) and noting the formula compensates.

**Observations (not fixed):**
- **`calc_chest_position` has an unused `_node_id` param** ([src/types/node.rs:298,313](src/types/node.rs#L313)) labeled "for future use". Per the "don't design for hypothetical future requirements" guideline this is dead weight, but removing it is a 6-site mechanical change including one caller in [src/bot/mod.rs:565](src/bot/mod.rs#L565) that lives in an unreviewed file. Flagged for the bot/mod.rs review — the cheapest time to drop the param is when that file is already being touched.
- **`eprintln!` at the "reserved chest save failed" branch** ([src/types/node.rs:166](src/types/node.rs#L166)) — consistent with the rest of the types/ layer (pair.rs, user.rs, storage.rs all use `eprintln!` for non-fatal load/save warnings). Migrating the whole types/ layer to `tracing::warn!` is a separate, coherent change; touching one site in isolation would just create inconsistency.
- **`Node::load` re-enforces node 0's reserved chest invariants and re-saves on mismatch** — defensive against manual edits of `data/storage/0.json`. Correct: invariants flow through one code path whether the file was freshly created or tampered with.
- **Position fields recomputed from storage origin every load, never trusted from disk** — documented at lines 96-100. Lets operators move the storage origin in config and have existing node files relocate on next load, no data migration needed. Good design.
- **`calc_position` uses an O(sqrt(id)) loop** rather than the closed-form `ring = ceil((sqrt(1 + id/2) - 1))`. Comment at line 247-248 explicitly flags the trade-off (avoids floating-point rounding concerns). Correct choice — the loop is called at most once per node load/creation, never in a hot path.
- **Spiral algorithm is thoroughly tested** with unique-position and deterministic-position checks. The `test_calc_position_ring_1` comment mentions "distance sqrt(9) = 3 or sqrt(18) ~= 4.24" but the assertion checks Chebyshev distance (max(|dx|,|dz|) ≤ ring), not Euclidean; minor comment inaccuracy, not worth touching.
- **Every branch of `calc_chest_position` uses `z - 1`** — documented as the south-facing front block where the bot clicks, not the chest block itself. Avoids asking callers to know this offset.
- **Tests round-trip `Node::new(0, ...)`** to verify the chest-0/chest-1 reserved assignments, but don't test `Node::load` re-enforcement directly. The re-enforcement path is exercised indirectly whenever a storage load happens after a manual edit; a targeted test would be a worthwhile addition but is outside scope.

**Build:** `cargo build` clean (zero warnings). **Tests:** all 113 passing.
**Doc impact:** None. Module doc and [ARCHITECTURE.md:230](ARCHITECTURE.md#L230) already say "4 chests per node" and "3 blocks apart"; both now backed by the `CHESTS_PER_NODE`/`NODE_SPACING` constants at every code site.

### src/types/chest.rs
- [x] struct `Chest`
- [x] impl Chest :: fn `new`

**Review findings and fixes applied:**

1. **Chest-position geometry duplicated between `Chest::new` and `Node::calc_chest_position`** ([old src/types/chest.rs:100-124](src/types/chest.rs) + [old src/types/node.rs:312-340](src/types/node.rs)) — two identical 4-arm `match`es, reachable from different call paths. Worse, `Node::load` did `Chest::new(id, &node.position, chest.index).position` — building a whole `Chest` struct just to extract its position field — because no static accessor existed. A silent drift between the two matches (if one got updated and the other didn't) would relocate chests at either `Chest::new` time or validation time, breaking the bot's navigation silently. **Fixed:** extracted the layout into `Chest::calc_position(&Position, i32) -> Position` as the single source of truth. `Chest::new` now delegates. `Node::calc_chest_position` becomes a thin wrapper that preserves its `node_id`-carrying signature for the bot validation path. Simplified [src/types/node.rs:132](src/types/node.rs#L132) from the `Chest::new(...).position` hack to a direct `Chest::calc_position(&node.position, chest.index)` call.
2. **Hardcoded `4` in `id = node_id * 4 + index`** ([src/types/chest.rs:86](src/types/chest.rs#L86)) — same drift risk as the node.rs case; if `CHESTS_PER_NODE` ever changed, `Node::new` would build N chests but `Chest::new` would still compute IDs with stride 4. **Fixed:** `let id = node_id * CHESTS_PER_NODE as i32 + index;`.
3. **Hardcoded `vec![0; 54]`** ([src/types/chest.rs:132](src/types/chest.rs#L132)) — 54 is the double-chest slot count and already named `DOUBLE_CHEST_SLOTS` in constants.rs. Replaced with the constant. Doc comment on the line ("one entry per slot in a double chest") collapsed since the constant name carries that meaning.
4. **Redundant `assert!` + `_ => unreachable!("Index validated above")`** ([src/types/chest.rs:80-84,121-123](src/types/chest.rs)) — belt-and-suspenders defense that duplicated the panic message. **Fixed:** dropped the upfront assert; the match's catch-all now panics with a single message, same observable behavior.

**Observations (not fixed):**
- **Other call sites still hardcode `54`** — [src/bot/trade.rs:102](src/bot/trade.rs#L102), [src/bot/mod.rs:331,376,581](src/bot/mod.rs#L331), [src/store/orders.rs:1117](src/store/orders.rs#L1117). Deferred to those files' reviews; touching them now is drive-by editing of unreviewed code.
- **Minor doc inconsistency in the old layout ASCII art** — chest.rs pre-fix used `XSNP` where node.rs module doc uses `NSNP` (the `X` was a typo). The new `calc_position` doc uses `NSNP` consistent with node.rs.
- **`Chest` has no free-standing `load`/`save`** — nodes serialize their chests inline, and chest files on disk are vestigial (explicitly noted in the module docstring). Correct: prevents inconsistency between node-embedded and standalone chest data.
- **Doc-level invariant `amounts[i] <= pair.shulker_capacity()`** is enforced elsewhere (Storage / Pair), not at chest construction. That's the right layer — construction doesn't know the item type yet.
- **`Chest` derives `Debug, Serialize, Deserialize, Clone` but not `PartialEq`** — no site compares whole chests for equality, so the absence is correct. `Eq` on `Vec<i32>` would be free but invites confusion about whether equality includes `position` (yes, it would) which is rarely what a caller wants.
- **`ItemId::EMPTY` initializer for `item`** is the correct post-migration default — pre-ItemId code used bare `""`. Comment on the line makes that explicit.

**Build:** `cargo build` clean (zero warnings). **Tests:** all 113 passing (refactor preserved all chest-position assertions in `test_calc_chest_position`).
**Doc impact:** None. [DATA_SCHEMA.md:184](DATA_SCHEMA.md#L184) still correctly says "`amounts.len() == 54`" — that statement is unchanged behaviour, just now expressed via `DOUBLE_CHEST_SLOTS`. The `node_id * 4` formula referenced in [RECOVERY.md:38](RECOVERY.md#L38) and [src/messages.rs:56](src/messages.rs#L56) is still accurate (4 == CHESTS_PER_NODE) and worth leaving prose-readable.

### src/types/trade.rs
- [x] struct `Trade`
- [x] enum `TradeType`
- [x] impl Trade :: fn `new`
- [x] impl Trade :: fn `save`
- [x] impl Trade :: fn `load_all_with_limit`
- [x] impl Trade :: fn `save_all`

**Review findings and fixes applied:**

1. **Double directory scan in `load_all_with_limit`** ([old src/types/trade.rs:135-160](src/types/trade.rs)) — the function did `fs::read_dir(dir_path)?` twice: once to count `.json` files for the log message, then again to actually read them. Two syscalls, and the counts could disagree if a trade was written between scans (harmless-but-confusing log output). **Fixed:** collect the filtered `.json` paths in a single pass into `Vec<PathBuf>`, then iterate for deserialize. `file_count` is now just the vec length. No behavioral change beyond doing it once.

**Observations (not fixed):**
- **`load_all_with_limit` deserializes every trade before trimming to `max_trades`** — a 100K-trade history with `max_trades_in_memory = 50_000` still reads and parses all 100K files before dropping the oldest 50K. A more scalable design would list filenames, sort by filename (RFC3339 with `:` → `-` is lexicographically chronological because every field is fixed-width and year-first), take only the last N, then deserialize those. Skipped because the current load runs once at startup, and the default 50K cap already bounds the damage. Worth revisiting if trade volumes push startup latency over ~1s.
- **`save_all` with an empty `Vec` deletes every file in `data/trades`** — documented behavior ("callers can use this to synchronize after in-memory deletions") but a real foot-gun. Kept as-is; the two actual callers ([src/store/state.rs:79](src/store/state.rs#L79) being the primary) always pass a fully-loaded snapshot.
- **Timestamp-as-filename collision risk** ([src/types/trade.rs:89-91](src/types/trade.rs)) — the comment claims "`Utc::now()` is monotonic per process", which is not true (`chrono::Utc::now()` reflects wall-clock time, which can jump backwards from NTP adjustment or manual change). A collision would require two trades at the same nanosecond, which is vanishingly unlikely in practice; theoretical-only, not fixing.
- **`save_all` reconstructs the filename inline** ([src/types/trade.rs:206-207](src/types/trade.rs#L206-L207)) instead of deriving it from `get_trade_file_path`. Minor duplication; both paths use the same colon-to-dash substitution. Not worth the inline helper extraction.
- **Partial-failure safety of `save_all`** — if one `trade.save()?` fails mid-loop, the error propagates and the orphan-cleanup loop never runs, so already-saved files don't become orphans. Correct behavior.
- **`Trade::default()` is derived but unused in production** — `grep "Trade::default"` returns no hits. The derive is harmless (chrono implements `Default` for `DateTime<Utc>` as epoch) and cheap; removing it would risk breaking future test helpers. Leaving.
- **`item: ItemId`** — correctly typed via the newtype; trade records round-trip through `#[serde(transparent)]` as bare strings. Consistent with the `ItemId` migration.
- **Colon-replacement for Windows filesystem compatibility** — correctly documented at line 93. NTFS reserves `:` for Alternate Data Streams, and writing `2024-01-02T03:04:05Z.json` there silently creates an ADS on the file without `.json` extension.
- **`.is_file()` filter in the load path** guards against subdirectories in `data/trades/`; nothing currently creates them, but defensive.

**Build:** `cargo build` clean (zero warnings). **Tests:** all 113 passing.
**Doc impact:** None. `DATA_SCHEMA.md` describes `data/trades/{timestamp}.json` layout without touching the scan-once detail.

### src/types/order.rs
- [x] struct `Order`
- [x] enum `OrderType`
- [x] impl Order :: fn `save_all_with_limit`

**Review findings and fixes applied:** None — the file is 96 lines, one save method, well-scoped. No bugs or rot found.

**Observations (not fixed):**
- **`const ORDERS_FILE` is private but the filename is duplicated** — [src/store/mod.rs:127](src/store/mod.rs#L127) hardcodes `Path::new("data/orders.json")` instead of referencing the constant. Two sources of truth for the same filename; a path change would have to be made in both places. Fix is trivial (`pub(crate) const ORDERS_FILE` + call it from the store module) but touches store/mod.rs which isn't yet reviewed. Flagged for that review.
- **No `load` method by design** — [src/store/mod.rs:127-152](src/store/mod.rs#L127-L152) explicitly deletes `data/orders.json` at startup. Orders represent in-flight user requests tied to live bot/chest state and replaying half-finished orders across restarts would risk double-charging. Documented at the call site and in [DEVELOPMENT.md:106](DEVELOPMENT.md#L106) / [DATA_SCHEMA.md:21](DATA_SCHEMA.md#L21). Correct design.
- **`OrderType` lacks timestamps, unlike `TradeType`** — intentional: orders are pending (not historical), and ordering in the `VecDeque` carries sequencing. Once an order completes it becomes a `Trade` with a real timestamp. Clean separation.
- **`#[derive(Default)]` on both `Order` and `OrderType`** — never called in production (grep `"Order::default"` returns no hits); derive is harmless and cheap, and removal would risk breaking test helpers. Leaving.
- **`save_all_with_limit` always clones the VecDeque** — even when no pruning is needed, line 86's `orders.clone()` allocates a full copy before serializing. Could be avoided by serializing `orders` directly in the unpruned branch, but `VecDeque` serialization doesn't expose a cheap slice view, and clone cost is negligible vs. the JSON write itself. Not worth a specialized branch.
- **Pruning keeps the most recent `max_orders`** — `iter().skip(len - max_orders)` matches the queue's oldest-first layout (`push_back` / `pop_front`). Consistent with how `Trade::load_all_with_limit` trims.
- **`io::ErrorKind::Other` wrapping `serde_json::Error`** — standard idiom; no call site matches on error kind, so the erasure is fine.
- **Serialized variant names are part of the on-disk format** (documented at lines 23-25). Correct — renaming any `OrderType` variant is a breaking change for operators whose `data/orders.json` might survive a restart in some future scenario. Kept-as-documented.

**Build:** `cargo build` clean (unchanged — no code edits). **Tests:** unchanged.
**Doc impact:** None.

### src/types/pair.rs
- [x] struct `Pair`
- [x] impl Pair :: fn `shulker_capacity_for_stack_size`
- [x] impl Pair :: fn `sanitize_item_name_for_filename`
- [x] impl Pair :: fn `get_pair_file_path`
- [x] impl Pair :: fn `save`
- [x] impl Pair :: fn `load_all`
- [x] impl Pair :: fn `save_all`

**Fixes applied:**
1. Removed local `pub const SHULKER_BOX_SLOTS: i32 = 27;` inside `impl Pair`. `shulker_capacity_for_stack_size` now multiplies `crate::constants::SHULKER_BOX_SLOTS as i32 * stack_size` directly. One constant, one source of truth.
2. Removed the now-stale `#[allow(dead_code)]` + duplication comment from [src/constants.rs](src/constants.rs) for `SHULKER_BOX_SLOTS` (the attribute was only there because of the `pair.rs` shadow).

**Observations (not fixing):**
- `sanitize_item_name_for_filename` and `get_pair_file_path` look correct. Windows-reserved chars (`:`) are replaced with `_`, then `minecraft:` prefix is stripped — the specific order matters because the prefix contains the `:` that would otherwise become an underscore. Comment on line 89-90 says "colon replacement for safety" — fine.
- `save_all` does orphan-cleanup (delete files whose item name is no longer in the map) mirroring `User::save_all`. Symmetric, good.
- No `.unwrap()` in runtime paths.

### src/types/user.rs
- [x] struct `User`
- [x] fn `get_http_client`
- [x] impl User :: async fn `get_uuid_async`
- [x] impl User :: fn `get_user_file_path`
- [x] impl User :: fn `save`
- [x] impl User :: fn `load_all`
- [x] impl User :: fn `save_all`

**Fixes applied:**
1. Corrected module-level `//! ## Mojang API Integration` doc block that claimed a blocking `get_uuid()` wrapper exists alongside `get_uuid_async()`. There is no `get_uuid()` — only the async form. Replaced with an accurate three-line summary that also mentions TTL caching lives in `store::utils::resolve_user_uuid`.
2. Removed `/// **Preferred**: Use this async version instead of the blocking \`get_uuid()\`.` from `get_uuid_async`'s doc for the same reason.
3. Swapped `println!` → `eprintln!` at the "Users directory not found" branch ([src/types/user.rs:186](src/types/user.rs#L186)). Matches the other two warning paths in the same function (lines 205, 211) and goes to stderr like all other diagnostic output in this layer.

**Observations (not fixing):**
- `id.len() != 32` length check on the raw Mojang response is a necessary guard against malformed API responses (`&id[0..8]` panics on non-ASCII or short strings). Correct.
- `#[cfg_attr(test, allow(dead_code))]` on `get_uuid_async` / `HTTP_CLIENT` / `MojangResponse` is the right pattern for "production-only" code — tests run the mock path and never instantiate these.
- `USERS_DIR = "data/users"` is a file-path literal duplicated in a few places outside this file (pair.rs does the same thing for `PAIRS_DIR`, trade.rs for `TRADES_DIR`). Consistent convention — not a bug.

### src/types/storage.rs
- [x] struct `ChestTransfer`
- [x] struct `Storage`
- [x] impl Storage :: fn `save`
- [x] impl Storage :: fn `new`
- [x] impl Storage :: fn `load`
- [x] impl Storage :: fn `add_node`
- [x] impl Storage :: fn `total_item_amount`
- [x] impl Storage :: fn `get_chest_mut`
- [x] impl Storage :: fn `withdraw_item`
- [x] impl Storage :: fn `deposit_item`
- [x] impl Storage :: fn `simulate_withdraw_plan`
- [x] impl Storage :: fn `simulate_deposit_plan`
- [x] impl Storage :: fn `withdraw_plan`
- [x] impl Storage :: fn `deposit_plan`
- [x] impl Storage :: fn `normalize_amounts_len`
- [x] impl Storage :: fn `deposit_into_chest`
- [x] impl Storage :: fn `find_empty_chest_index`
- [x] impl Storage :: fn `get_overflow_chest`
- [x] impl Storage :: fn `get_overflow_chest_mut`
- [x] impl Storage :: fn `get_overflow_chest_position`
- [x] impl Storage :: const fn `overflow_chest_id`

**Fixes applied:**
1. `SLOTS_PER_CHEST` is now an alias for `crate::constants::DOUBLE_CHEST_SLOTS` (was `54` literal). Call sites inside this file still read `Self::SLOTS_PER_CHEST` for readability; the canonical constant backs it.
2. `DEFAULT_SHULKER_CAPACITY` now expands to `(crate::constants::SHULKER_BOX_SLOTS as i32) * 64` (was `27 * 64`). Same value (1728), sourced from the canonical constant.
3. `simulate_deposit_plan` node-0 reserved-chest skip logic: `chest_idx == 0` / `== 1` replaced with `chest_idx == DIAMOND_CHEST_ID as usize` / `chest_idx == OVERFLOW_CHEST_ID as usize`. Ties the intent to the constants already in use elsewhere.
4. Same replacement in `deposit_plan` phase 1 loop.
5. `simulate_deposit_plan` phase-2 empty-chest loop: `node_0.chests[0]` → `node_0.chests.get(DIAMOND_CHEST_ID as usize)`, same for `[1]`. Safer (`.get()` returns `Option` instead of panicking) and self-documenting.
6. Removed dead `let before = qty; ... let _ = before;` pair from `deposit_plan` (lines 545,555). Pure noise — `before` was never read. No behavior change.
7. `find_empty_chest_index` rewritten to use `.get(idx).is_some_and(|c| c.item.is_empty())` instead of `node_0.chests[0].item.is_empty()`. Equivalent in practice (Node::new always creates 4 chests) but no longer panic-shaped if a malformed node sneaks through.
8. `get_overflow_chest` / `get_overflow_chest_mut` now use `self.nodes.first()? / first_mut()?` with `.get(OVERFLOW_CHEST_ID as usize)`. Same semantics as the old `if self.nodes.is_empty() { None } ... self.nodes[0].chests.get(1)` but no hardcoded indices.

**Observations (not fixing):**
- `DEFAULT_SHULKER_CAPACITY` constant has no callers — all code paths use `Pair::shulker_capacity_for_stack_size(stack_size)`. Kept because the docstring clearly warns it is a *default* (item-stack-size-unaware) value reserved for future tooling. Safe to delete later if still unused after the store/ review.
- `withdraw_item` / `deposit_item` convenience wrappers are documented as reserved. Tests exercise them via `deposit_plan` directly — the wrappers themselves have no callers. Same "reserved" rationale as above.
- `add_node` uses `self.nodes.last_mut().unwrap()` — `unwrap` is safe because we push on the line above. Kept with its comment.
- Reserved-chest rules (diamond → node 0 / chest 0, overflow → node 0 / chest 1) are encoded in three places: `simulate_deposit_plan`, `deposit_plan`, `find_empty_chest_index`. A `is_reserved_for(item, node_idx, chest_idx) -> bool` helper would DRY them; deferred as non-mechanical.

---

## bot/

### src/bot/mod.rs
- [x] struct `BotState`
- [x] struct `Bot`
- [x] fn `bot_task`
- [x] fn `validate_node_physically`
- [x] fn `handle_event_fn`
- [x] fn `handle_event`
- [x] fn `handle_chat_message`
- [x] impl BotState :: fn `default`
- [x] impl Bot :: async fn `new`
- [x] impl Bot :: async fn `send_chat_message`
- [x] impl Bot :: async fn `send_whisper`
- [x] impl Bot :: fn `normalize_item_id`
- [x] impl Bot :: fn `chat_subscribe`

**Fixes applied:**
1. Deposit `known_counts` guard: `target_chest.amounts.len() == 54` → `== crate::constants::DOUBLE_CHEST_SLOTS` at [src/bot/mod.rs:331](src/bot/mod.rs#L331).
2. Same change in the withdraw path at [src/bot/mod.rs:376](src/bot/mod.rs#L376).
3. `validate_node_physically` length check at [src/bot/mod.rs:581](src/bot/mod.rs#L581) and its format-string message both use `DOUBLE_CHEST_SLOTS`.
4. `bot_task` reconnect loop: the 20-second polling horizon is now a named `init_timeout` binding, and the 100ms inner-loop sleep uses `DELAY_SHORT_MS` (matches value). Makes the "wait for Event::Init" intent readable at a glance.
7. Dropped redundant `.clone()` on `message_text` at [src/bot/mod.rs:681](src/bot/mod.rs#L681) — the string is never read after the `state.chat_tx.send(...)` call, so the broadcast can take ownership directly.

**Skipped (from the agent's proposed fix list):**
- **Fix 5** and **Fix 6** were proposed based on constants that do not exist in [src/constants.rs](src/constants.rs) (`POST_RECONNECT_INIT_WAIT_MS`, `DELAY_SHUTDOWN_BUFFER_MS`). Introducing those would be a speculative addition, not a mechanical cleanup — deferred.

**Observations (not fixing):**
- `validate_node_physically`'s per-chest error aggregation (lines 554-630) is a good pattern — single pass reports every broken chest, much better than early-return for operator diagnostics.
- `normalize_item_id` is a thin wrapper around `ItemId::normalize_str`. Kept because the bot layer uses it as a stable alias (same reason `Chest::calc_position` is called via `Node::calc_chest_position`).
- `chat_subscribe` returns a fresh `broadcast::Receiver` — callers are responsible for dropping theirs. Correct lagged-receiver semantics (tokio::sync::broadcast drops oldest on lag).
- The `DELAY_VALIDATION_BETWEEN_CHESTS_MS` gate in the chest-validation loop (line 560) is the right place to rate-limit open-close churn.

### src/bot/connection.rs
- [x] async fn `connect`
- [x] async fn `disconnect`

**Fixes applied:**
1. Dropped redundant `let account = account.clone(); let server_address = server_address.clone();` at [src/bot/connection.rs:41-42](src/bot/connection.rs#L41). Both parameters are already owned and were moved into the `async move` closure anyway — the shadow clones were pure noise.
2. Collapsed duplicated disconnect-packet comment block ([src/bot/connection.rs:122-134](src/bot/connection.rs#L122)). Two near-identical ~10-line explanations of why we sleep before abort were replaced with a single 4-line comment that keeps the essential rationale ("Azalea event loop must still be alive to flush the packet; aborting too early would drop it").
3. Rewrote the `/// **Timing**:` line in `disconnect`'s docstring. The old copy said "approximately 4 seconds total"; the new copy explains it's an *upper bound* (exits early once the client clears) and references the two `DELAY_DISCONNECT_MS` waits by name.

**Observations (not fixing):**
- `bot.connecting.swap(true, Ordering::SeqCst)` correctly guards against concurrent `connect()` calls; the early `Ok(())` on re-entry is the right choice (silent idempotence).
- `disconnect` sequence (disconnect → wait for flush → abort → wait for TCP teardown → clear client handle) mirrors the documented shutdown order in README. No races observed.
- The comment about the bevy `LogPlugin` "harmless error" is useful context that a future reader would otherwise rediscover by grepping Azalea's source. Kept.

### src/bot/navigation.rs
- [x] const `PATHFINDING_CHECK_INTERVAL_MS`
- [x] async fn `navigate_to_position_once`
- [x] async fn `navigate_to_position`
- [x] async fn `go_to_node`
- [x] async fn `go_to_chest`

**Fixes applied:**
1. Promoted local `PATHFINDING_CHECK_INTERVAL_MS` (100ms) from a file-local `const` to [src/constants.rs](src/constants.rs) alongside the other `DELAY_*_MS` values. Keeps timing tuning centralized and lets future callers (e.g. navigation retry loops outside this file) reuse it. Import changed to `use crate::constants::{..., PATHFINDING_CHECK_INTERVAL_MS, ...};`.
2. Replaced hardcoded `200` ms at [src/bot/navigation.rs:207](src/bot/navigation.rs#L207) (post-go_to_chest settle) with `DELAY_MEDIUM_MS`. The value already matches `DELAY_MEDIUM_MS = 200`, so this is a pure naming fix.

**Observations (not fixing):**
- `go_to_chest` logs "At node ({x},{y},{z}), chest {id} accessible at ({cx},{cy},{cz})" at info level on every chest visit — verbose; could be demoted to `debug!` but not wrong.
- `navigate_to_position` retry loop uses `exponential_backoff_delay(attempt, RETRY_BASE_DELAY_MS, RETRY_MAX_DELAY_MS)` consistent with the chest-IO retry pattern — good.
- No `.unwrap()` in runtime paths.

### src/bot/inventory.rs
- [x] async fn `ensure_inventory_empty`
- [x] async fn `move_hotbar_to_inventory`
- [x] async fn `quick_move_from_container`
- [x] fn `verify_holding_shulker`
- [x] fn `is_entity_ready`
- [x] async fn `wait_for_entity_ready`
- [x] fn `carried_item`
- [x] async fn `ensure_shulker_in_hotbar_slot_0`
- [x] async fn `recover_shulker_to_slot_0`

**Fixes applied:**
1. Removed two function-local `const HOTBAR_SLOT_0: usize = 36;` shadows (one in `ensure_shulker_in_hotbar_slot_0` at line 433, one in `recover_shulker_to_slot_0` at line 798). Added `use crate::constants::HOTBAR_SLOT_0;` at the module level — all ~50 call sites inside this file continue to reference the unqualified name, now backed by the canonical constant.
2. As a knock-on, removed the `#[allow(dead_code)]` from `HOTBAR_SLOT_0` in [src/constants.rs](src/constants.rs) — no longer dead now that both duplicates are gone.

**Skipped (from the agent's proposed fix list):**
- **Fix 2** (change local `MAX_RETRIES` from 3 → 2 in `recover_shulker_to_slot_0`): this is a behavior change, not a cleanup. The 3-retry policy in the recovery path is intentionally more forgiving than the 2-retry `SHULKER_OP_MAX_RETRIES` because recovery runs after a first-attempt failure. Kept at 3.

**Observations (not fixing):**
- `ensure_shulker_in_hotbar_slot_0` is ~400 lines of sequential click-then-verify logic with three nested "place shulker" paths (cursor holds shulker / hotbar slot 0 occupied / hotbar slot 0 empty). Extracting a `place_shulker_in_hotbar_slot_0(source)` helper would collapse the three branches; high-value refactor, out of scope.
- Extensive debug! logging throughout — useful for field debugging. Not reducing.
- `recover_shulker_to_slot_0` reopens inventory on every retry iteration. Correct — avoids stale state after a failed click.

### src/bot/chest_io.rs
- [x] const `CHUNK_NOT_LOADED_PREFIX`
- [x] fn `find_shulker_in_inventory_view`
- [x] async fn `place_shulker_in_chest_slot_verified`
- [x] async fn `open_chest_container_once`
- [x] async fn `open_chest_container_for_validation`
- [x] async fn `open_chest_container`
- [x] async fn `transfer_items_with_shulker`
- [x] async fn `transfer_withdraw_from_shulker`
- [x] async fn `transfer_deposit_into_shulker`
- [x] async fn `prepare_for_chest_io`
- [x] async fn `automated_chest_io`
- [x] async fn `withdraw_shulkers`
- [x] async fn `deposit_shulkers`

**Fixes applied:**
1. Removed the in-function `const HOTBAR_SLOT_0: usize = 36;` shadow in `deposit_shulkers` (was at :1558). Replaced the bare `slot: Some(36 as u16)` in `withdraw_shulkers` (:1224) with `Some(HOTBAR_SLOT_0 as u16)`. Both now source the constant from the `use crate::constants::{...}` list at the top.
2. Replaced magic `54` comparisons in `transfer_items_with_shulker` (:762-769) with `DOUBLE_CHEST_SLOTS`. The "last slot short-circuit" guard at :1800 (`slot_idx < 53`) is now `slot_idx < DOUBLE_CHEST_SLOTS - 1` — same semantics, reads as "not the last slot".
3. `SHULKER_BOX_SLOTS` added to the `use` list; used where `27` literals participated in arithmetic:
   - `inv_end = inv_start + SHULKER_BOX_SLOTS` at :657 (was `+ 27`).
   - `inventory_end = inv_start + SHULKER_BOX_SLOTS + 9` at :728 and :1635 (were `+ 36`, which is `27 + 9`).
   These are the shulker-contents-size boundary computations; naming them protects against accidental divergence from the canonical constant.
4. `place_shulker_in_chest_slot_verified` — removed local `const CLICK_DELAY_MS: u64 = 300;` (exact match for `DELAY_INTERACT_MS`). All 4 sleep-after-click sites now use `DELAY_INTERACT_MS` directly. **Kept** `VERIFY_DELAY_MS = 250` and `MAX_VERIFICATION_ATTEMPTS = 7` as function-local tuning — `VERIFY_DELAY_MS` doesn't exactly match any crate constant (closest is `DELAY_LOOK_AT_MS = 250` but semantically wrong), and changing it to the agent's suggested `DELAY_MEDIUM_MS = 200` would be a 20% behavior reduction on a verification delay.

**Skipped (from the agent's proposed fix list):**
- **Fix 5** (`timeout_ticks` at :332 and :395): promoting these to module- or crate-level constants would be net positive, but pulling them into `crate::constants` without a clear naming convention for "chest-open timeouts" is bike-shedding. Deferred.

**Observations flagged but not fixed (some are latent bugs):**
- **Latent bug**: `transfer_deposit_into_shulker` hard-codes `64` as max stack size in its partial-transfer branch at ~:842,845. For non-64-stackable items (ender pearls=16, tools=1) that reach this branch, the arithmetic is wrong and may overfill. `deposit_shulkers` already computes `stack_size` and passes it through — this function should take it as a parameter. **Real bug, not a cleanup.** Left unfixed because it is a behavior change that needs its own review / test plan.
- `transfer_items_with_shulker` has an unused `_bot: &Bot` parameter (:500).
- `let client = ...; drop(client);` dance at :955-988 is unnecessary — clone goes out of scope anyway.
- `chest_size = container.contents()?.len()` at :45-48 runs a second `ok_or_else("Shulker closed")` check right after `container.slots()` — single call would be cleaner.
- ~400-line logic duplication between `withdraw_shulkers` (:1090-1394) and `deposit_shulkers` (:1403-1823) sharing the same cursor-clear / take-shulker / close-chest / hotbar-slot-0 / station / open-shulker / pickup / reopen / put-back skeleton. Extracting a `ShulkerRoundTrip` helper is the high-value refactor — out of scope for a mechanical pass.
- `slot_counts: Vec<i32>` from `automated_chest_io` could be `[i32; DOUBLE_CHEST_SLOTS]` (fixed size, no alloc, invariant-encoded). Touches all callers; follow-up.

**Doc impact**: None. Cleanup doesn't change abstraction levels that ARCHITECTURE.md / DATA_SCHEMA.md / DEVELOPMENT.md describe.

### src/bot/shulker.rs
- [x] const `SHULKER_BOX_IDS`
- [x] fn `shulker_station_position`
- [x] fn `is_shulker_box`
- [x] fn `validate_chest_slot_is_shulker` (cfg(test))
- [x] async fn `pickup_shulker_from_station`
- [x] async fn `open_shulker_at_station_once`
- [x] async fn `open_shulker_at_station`
- [x] test `test_is_shulker_box`
- [x] test `test_validate_chest_slot_is_shulker`
- [x] test `test_shulker_station_position`

**Fixes applied:**
1. `pickup_shulker_from_station` post-`look_at` sleep at [src/bot/shulker.rs:154](src/bot/shulker.rs#L154): `from_millis(250)` → `from_millis(DELAY_LOOK_AT_MS)`. Exact value match (250ms) to the canonical look-before-interact delay.
2. `open_shulker_at_station_once` post-`look_at` sleep at [src/bot/shulker.rs:343](src/bot/shulker.rs#L343): `from_millis(300)` → `from_millis(DELAY_INTERACT_MS)`. Exact value match (300ms) to the canonical click/interact delay. Both constants added to the `use crate::constants::{...}` list.

**Observations (not fixing):**
- 450ms delay at line ~347 (post-`block_interact`) doesn't exactly match any crate constant (closest are `DELAY_BLOCK_OP_MS = 350` and `DELAY_SETTLE_MS = 500`). Left local — changing either way would be a behavior shift.
- Local constants `MAX_BREAK_WAIT_MS`, `CHECK_INTERVAL_MS` are mining-specific tuning; keep local.
- 1000ms item-drop-settle delay and 400ms post-navigation delays have no exact crate matches — kept as empirical values.

### src/bot/trade.rs
- [x] fn `trade_bot_offer_slots`
- [x] fn `trade_player_offer_slots`
- [x] fn `trade_player_status_slots`
- [x] fn `trade_accept_slots`
- [x] fn `trade_cancel_slots`
- [x] async fn `wait_for_trade_menu_or_failure`
- [x] async fn `place_items_from_inventory_into_trade`
- [x] fn `validate_player_items`
- [x] async fn `execute_trade_with_player`
- [x] test `test_trade_bot_offer_slots`
- [x] test `test_trade_player_offer_slots`
- [x] test `test_trade_player_status_slots`
- [x] test `test_trade_accept_slots`
- [x] test `test_trade_cancel_slots`
- [x] test `test_trade_slot_sets_disjoint`

**Fixes applied:**
1. `contents_len == 54` at [src/bot/trade.rs:102](src/bot/trade.rs#L102) → `== DOUBLE_CHEST_SLOTS`. Same identity check (54-slot container suggests trade GUI or double chest).
2. Three `i >= contents_len + 27` / `inv_slot >= contents_len + 27` sites (lines ~213, 269, 283) → `+ SHULKER_BOX_SLOTS`. These compute the hotbar boundary within the trade container view (inventory slots end at `contents_len + 27`, hotbar begins).
3. Removed dead `let _bot_slots = trade_bot_offer_slots();` at line 688 — the binding was never read anywhere in the function. The comment at 685 explains the actual clicked slots (accept/cancel/status).
4. Added `use crate::constants::{DOUBLE_CHEST_SLOTS, SHULKER_BOX_SLOTS};` at the top.

**Observations (not fixing):**
- **LATENT BUG**: `execute_trade_with_player` takes a `trade_timeout_ms` parameter (via `bot.trade_timeout_ms`) but the inner validation loop at line ~707 uses hard-coded `from_secs(40)` instead of the config value. This silently ignores user-tuned `TRADE_TIMEOUT_MS` (default 45000ms). Flagging here as a real bug, but fixing it is a behavior change that may have been intentional — out of scope for this mechanical pass.
- 450ms inventory-sync-settle delay at line 203 doesn't match any crate constant; empirical value, kept.
- Slot math helpers (`row * 9 + col`) use the `9` literal intentionally — Minecraft's row width is protocol-fixed; not a candidate for a named constant.

---

## store/

### src/store/mod.rs
- [x] struct `Store`
- [x] impl Store :: fn `new`
- [x] impl Store :: async fn `run`
- [x] impl Store :: async fn `process_next_order`
- [x] impl Store :: fn `reload_config`
- [x] impl Store :: fn `advance_trade`
- [x] impl Store :: async fn `handle_bot_message`
- [x] impl Store :: fn `expect_pair`
- [x] impl Store :: fn `expect_pair_mut`
- [x] impl Store :: fn `expect_user`
- [x] impl Store :: fn `expect_user_mut`
- [x] impl Store :: fn `apply_chest_sync`
- [x] impl Store :: fn `get_node_position`
- [x] impl Store :: fn `new_for_test`

**Fixes applied:**
1. `Store::new` init log at [src/store/mod.rs:197](src/store/mod.rs#L197) now includes `{} trades` between `orders.len()` and `storage.nodes.len()`. `trades` was loaded at line 155 but omitted from startup logging, making the startup state incomplete.

**Observations (not fixing):**
- `"data/orders.json"` string literal at line 127 is hardcoded; intentional because it's a session-only stale file to delete on startup, distinct from the canonical `QUEUE_FILE = "data/queue.json"` persistent order queue.
- `processing_order` flag + `current_trade` state-machine correctly prevents concurrent order execution and mirrors trade state to disk for crash recovery.

### src/store/state.rs
- [x] fn `apply_chest_sync`
- [x] fn `save`
- [x] fn `audit_state`
- [x] fn `assert_invariants`

**Fixes applied:** None — file already uses canonical constants (`crate::constants::DIAMOND_CHEST_ID`, `OVERFLOW_CHEST_ID`, `OVERFLOW_CHEST_ITEM`, `Storage::SLOTS_PER_CHEST`, `Storage::DEFAULT_SHULKER_CAPACITY`).

**Observations (not fixing):**
- **LATENT BUG** at line ~203: `skip(1)` in the repair-applied path assumes `audit_state` with `repair=true` always prepends a "Repair applied..." marker as the first issue. If that invariant is ever broken (issues empty after repair, or code path changes), the first real issue is silently dropped. Not a current bug — `audit_state` is the only caller — but fragile coupling.
- Line ~212 passes literal `8` as indent width to `fmt_issues`; minor cosmetic, kept.
- `-1` slot-sentinel values are documented protocol (chest-sync "unknown" slots).

### src/store/command.rs
- [x] enum `Command`
- [x] fn `parse_command`
- [x] fn `parse_item_quantity`
- [x] fn `parse_item_amount`
- [x] fn `parse_optional_amount`
- [x] fn `parse_price`
- [x] fn `parse_balance`
- [x] fn `parse_pay`
- [x] fn `parse_page`
- [x] fn `parse_cancel`
- [x] tests module

**Fixes applied:** None.

**Observations (not fixing):**
- `1_000_000.0` magic number at line ~179 for `/pay` maximum. Agent proposed extracting a new `MAX_PAYMENT_AMOUNT` constant, but this is outside the "only-exact-matches-to-existing-constants" cleanup scope. Semantically distinct from `MAX_TRANSACTION_QUANTITY` (i32 item-count cap) — merging them would be wrong. Left local.
- Validation layering is clean: parsing in `command.rs`, business-rule checks in `handlers/validation.rs`, economic checks in pricing.
- No latent bugs. Error messages are user-friendly and consistent.

### src/store/journal.rs
- [x] const `JOURNAL_FILE`
- [x] type alias `SharedJournal`
- [x] struct `Journal`
- [x] struct `JournalEntry`
- [x] enum `JournalOp`
- [x] enum `JournalState`
- [x] impl Journal :: fn `load`
- [x] impl Journal :: fn `load_from`
- [x] impl Journal :: fn `clear_leftover`
- [x] impl Journal :: fn `begin`
- [x] impl Journal :: fn `advance`
- [x] impl Journal :: fn `complete`
- [x] impl Journal :: fn `current`
- [x] impl Journal :: fn `persist`
- [x] impl `Default for Journal` :: fn `default`
- [x] tests module

**Fixes applied:** None — the agent's suggestions (`unwrap_or_default()` → `unwrap_or_else(|_| Vec::new())`, removing explicit `Vec<&JournalEntry>` type annotation) are pure style bikeshedding with zero behavior change, rejected.

**Observations (not fixing):**
- `unwrap_or_default()` at line 128 silently recovers from malformed JSON. Intentional per docs ("detection, not resume"), but a startup `tracing::warn!` would make corruption visible. Behavior change; out of scope.
- `NEXT_OPERATION_ID.fetch_add(1, Ordering::Relaxed)` is correct for single-process monotonicity (documented at line 107 as "only unique within a run").
- Tests use `std::process::id()` for per-process temp dirs — correct.

### src/store/orders.rs
- [x] struct `BuyPlan`
- [x] struct `SellPlan`
- [x] enum `ChestDirection`
- [x] async fn `execute_chest_transfers`
- [x] async fn `perform_trade`
- [x] async fn `validate_and_plan_buy`
- [x] async fn `handle_buy_order`
- [x] async fn `validate_and_plan_sell`
- [x] async fn `handle_sell_order`
- [x] async fn `execute_queued_order`
- [x] tests module

**Fixes applied:**
1. Test helper `make_storage` at [src/store/orders.rs:1117](src/store/orders.rs#L1117): `vec![0; 54]` → `vec![0; crate::constants::DOUBLE_CHEST_SLOTS]`. Previously flagged as deferred in the bot/mod.rs review.
2. Test helper `spawn_mock_bot` at [src/store/orders.rs:1165](src/store/orders.rs#L1165): `vec![-1i32; 54]` → `vec![-1i32; crate::constants::DOUBLE_CHEST_SLOTS]`.
3. `spawn_mock_bot` at line 1186: `let _received = bot_offers;` → `let _ = bot_offers;`. The named binding suggested an unused-but-meaningful value; the plain discard makes intent clear (the mock intentionally ignores the bot's side and echoes `player_offers`).

**Observations (not fixing):**
- The mock's choice to always return `player_offers` (regardless of direction) is correct for the current test suite but documented in the comment block — future test authors who miss the comment could be confused.
- Order-execution flow (validate → plan → perform_trade → execute_chest_transfers → commit) is the canonical order-handling pipeline; no dead paths observed.

### src/store/pricing.rs
- [x] const `TEST_FEE`
- [x] fn `validate_fee`
- [x] fn `reserves_sufficient`
- [x] fn `calculate_buy_cost`
- [x] fn `buy_cost_pure`
- [x] fn `calculate_sell_payout`
- [x] fn `sell_payout_pure`
- [x] tests module incl. proptests

**Fixes applied:** None — file is clean.

**Observations (not fixing):**
- No latent bugs. Buy/sell invariants (k-preservation, round-trip loss due to spread, reserve non-negativity) are all verified by property tests.
- Floating-point precision at near-pool-drain (e.g., `amount == item_stock - 1`) is mitigated by the `is_finite()` and positivity guards on every `Option<f64>` return. The `amount >= item_stock` rejection at line 102 prevents the zero-denominator case.
- Code correctly uses canonical `FEE_MIN`, `FEE_MAX`, `MIN_RESERVE_FOR_PRICE` constants.

### src/store/queue.rs
- [x] struct `OrderQueue`
- [x] struct `QueuePersist`
- [x] impl QueuedOrder :: fn `new`
- [x] impl QueuedOrder :: fn `description`
- [x] impl `Default for OrderQueue` :: fn `default`
- [x] impl OrderQueue :: fn `new`
- [x] impl OrderQueue :: fn `load`
- [x] impl OrderQueue :: fn `save`
- [x] impl OrderQueue :: fn `add`
- [x] impl OrderQueue :: fn `pop`
- [x] impl OrderQueue :: fn `is_empty`
- [x] impl OrderQueue :: fn `len`
- [x] impl OrderQueue :: fn `get_position`
- [x] impl OrderQueue :: fn `get_user_position`
- [x] impl OrderQueue :: fn `user_order_count`
- [x] impl OrderQueue :: fn `get_user_orders`
- [x] impl OrderQueue :: fn `cancel`
- [x] impl OrderQueue :: fn `estimate_wait`
- [x] tests module

**Fixes applied:** None — file is clean.

**Observations (not fixing):**
- 30-second per-order estimate in `estimate_wait` is a coarse UI hint documented as not-configurable; kept local.
- `.unwrap()` after `remove(pos)` at line ~301 is safe because `pos` came from a prior `position()` lookup; kept.
- No latent bugs. Order ID sequence, 1-indexed position tracking, atomic persistence, and cancel auth checks all correct.

### src/store/rate_limit.rs
- [x] struct `UserRateLimit`
- [x] struct `RateLimiter`
- [x] fn `calculate_cooldown`
- [x] impl UserRateLimit :: fn `new`
- [x] impl `Default for RateLimiter` :: fn `default`
- [x] impl RateLimiter :: fn `new`
- [x] impl RateLimiter :: fn `check`
- [x] impl RateLimiter :: fn `cleanup_stale`
- [x] tests module

**Fixes applied:** None — the agent-proposed `from_secs(60)` → `from_millis(RATE_LIMIT_MAX_COOLDOWN_MS * 2)` at line 101 would change the backdate from 60s to 120s (behavior change) and couple "new user grace period" to "max cooldown". Rejected as out-of-scope behavior change.

**Observations (not fixing):**
- 60-second new-user backdate at line ~101 is semantically distinct from `RATE_LIMIT_MAX_COOLDOWN_MS`; keeping them independent is correct.
- `cleanup_stale(max_age)` parameter name could be clearer (`stale_threshold`) but the semantic is easy to infer from the function body; minor.
- Uses canonical `RATE_LIMIT_BASE_COOLDOWN_MS` / `MAX_COOLDOWN_MS` / `RESET_AFTER_MS` / `STALE_AFTER_SECS` constants correctly.

### src/store/rollback.rs
- [x] struct `RollbackResult`
- [x] fn `chest_from_transfer`
- [x] async fn `deposit_transfers`
- [x] async fn `rollback_amount_to_storage`
- [x] impl RollbackResult :: fn `has_failures`

**Fixes applied:**
1. `chest_from_transfer` at [src/store/rollback.rs:57-58](src/store/rollback.rs#L57): `t.chest_id / 4` → `/ CHESTS_PER_NODE as i32`, `% 4` → `% CHESTS_PER_NODE as i32`. This is the inverse of `Chest::new`'s `node_id * CHESTS_PER_NODE as i32 + index` formula — the two must stay in sync. Added `CHESTS_PER_NODE` to the `use crate::constants::{...}` list.

**Observations (not fixing):**
- **LATENT BUG**: at line ~136-143, an `apply_chest_sync` failure is logged at `warn` level but still increments `operations_succeeded` and `items_returned`. This can mask state divergence behind a success counter. Fixing requires deciding the right severity level and counter policy; behavior change.
- Other error-handling paths (channel/timeout/bot errors) log at `error` — the asymmetry with `apply_chest_sync` warn is worth tightening later.
- `let _planned = ...` discarded at line 200 is consistent with the "simulated plan, actual work happens elsewhere" pattern.

### src/store/trade_state.rs
- [x] const `TRADE_STATE_FILE`
- [x] struct `TradeResult`
- [x] struct `CompletedTrade`
- [x] enum `TradeState`
- [x] fn `persist`
- [x] fn `load_persisted`
- [x] fn `clear_persisted`
- [x] impl TradeState :: fn `new`
- [x] impl TradeState :: fn `begin_withdrawal`
- [x] impl TradeState :: fn `begin_trading`
- [x] impl TradeState :: fn `begin_depositing`
- [x] impl TradeState :: fn `commit`
- [x] impl TradeState :: fn `rollback`
- [x] impl TradeState :: fn `phase`
- [x] impl TradeState :: fn `is_terminal`
- [x] impl TradeState :: fn `order`
- [x] impl `fmt::Display for TradeState` :: fn `fmt`
- [x] tests module

**Fixes applied:** None — file is clean.

**Observations (not fixing):**
- Module-level lifecycle diagram at line ~16 shows linear `Queued → Withdrawing → Trading → Depositing → Committed` but the implementation allows `Trading → Committed` (skipping deposit for buys that deliver directly to balance). ARCHITECTURE.md should clarify that `Depositing` is optional. **Doc drift** (see below).
- Panic-on-invalid-transition (e.g., `begin_withdrawal` called from `Trading`) is by design — state enum prevents this at type level.
- `#[allow(dead_code)]` occurrences (lines 38, 80, 88-91, 98, 227) all have justifying comments.

**Doc impact:** Yes — one-line update to [ARCHITECTURE.md](ARCHITECTURE.md) noting `Depositing` is optional in the trade state machine. Not applied in this pass (review found, did not change docs).

### src/store/utils.rs
- [x] const `UUID_CACHE`
- [x] type alias `UuidCache`
- [x] fn `uuid_cache`
- [x] fn `normalize_item_id`
- [x] async fn `resolve_user_uuid`
- [x] fn `clear_uuid_cache`
- [x] fn `cleanup_uuid_cache`
- [x] fn `ensure_user_exists`
- [x] fn `is_operator`
- [x] fn `get_node_position`
- [x] async fn `send_message_to_player`
- [x] fn `summarize_transfers`
- [x] fn `fmt_issues`
- [x] tests module

**Fixes applied:**
1. `summarize_transfers` at [src/store/utils.rs:202](src/store/utils.rs#L202): replaced `for (i, t) in ... .enumerate() { let _ = i; ... }` with `for t in ...`. The `enumerate()` + explicit `let _ = i` was an artifact of a previous version that used the index; dropping enumerate removes one allocation-free layer.

**Observations (not fixing):**
- `resolve_user_uuid` takes `_store: &Store` that is documented as retained for call-site stability (lines 49-50); acceptable.
- UUID cache correctly uses canonical `UUID_CACHE_TTL_SECS` and `CLEANUP_INTERVAL_SECS`.

### src/store/handlers/mod.rs
- [x] module declarations surface

No changes; pure re-export surface.

### src/store/handlers/validation.rs
- [x] fn `validate_item_name`
- [x] fn `validate_quantity`
- [x] fn `validate_username`

**Fixes applied:** None — file already uses canonical `MAX_TRANSACTION_QUANTITY`. Username length bounds (3-16) are Minecraft protocol constants, not configurable.

### src/store/handlers/buy.rs
- [x] async fn `handle`

No changes — thin wrapper over `order_queue.add(...)` which enforces `MAX_ORDERS_PER_USER` / `MAX_QUEUE_SIZE` internally.

### src/store/handlers/sell.rs
- [x] async fn `handle`

No changes — mirror of `buy.rs`.

### src/store/handlers/withdraw.rs
- [x] async fn `handle_enqueue`
- [x] async fn `handle_withdraw_balance_queued`

No changes. Local `const MAX_TRADE_DIAMONDS: i32 = 12 * 64;` (768) is correct — vanilla Minecraft trade-UI constraint, documented, and has no canonical equivalent.

**Observations:**
- **LATENT BUG**: when `amount` is `None`, balance is silently capped at `MAX_TRADE_DIAMONDS = 768` without notifying the user. Users with balance > 768 may be confused. Message/cap reconciliation is a behavior change, out of scope.

### src/store/handlers/deposit.rs
- [x] async fn `handle_enqueue`
- [x] async fn `handle_deposit_balance_queued`

No changes — mirror of `withdraw.rs`.

**Observations:**
- **LATENT BUG**: `require_exact_amount: false` + fixed-amount user message may mismatch when `is_flexible=false`; user told an exact amount but any amount is accepted. Behavior change, deferred.

### src/store/handlers/player.rs
- [x] async fn `handle_player_command`

No changes. Operator-gating and re-export structure correct.

### src/store/handlers/operator.rs
- [x] async fn `handle_additem_order`
- [x] async fn `handle_removeitem_order`
- [x] async fn `handle_add_currency`
- [x] async fn `handle_remove_currency`

No changes — hardcoded `/ 4` and `% 4` in chest-id math are now handled via `CHESTS_PER_NODE` in [src/store/rollback.rs](src/store/rollback.rs); duplicates in operator.rs were not surgical-fix targets for this pass.

**Observations:**
- Two-phase commit pattern (trade → storage deposit/withdraw with reverse-trade rollback) is resilient but means that a *second* failure during rollback strands items in the bot's inventory and requires manual operator intervention. Documented; not a bug.
- `debug_assert!` negative-stock guards only fire in debug builds. Production integer-underflow would corrupt stocks silently; raising to `assert!` or adding an invariant check in the save path would be a behavior change.

### src/store/handlers/cli.rs
- [x] async fn `handle_cli_message`

All 16 `CliMessage` variants covered. No changes. Agent suggested extracting `NODE_VALIDATION_TIMEOUT_SECS` (120) and `DIAMOND_ITEM` ("diamond") constants, but both would introduce new crate constants — out of scope.

**Observations:**
- Hardcoded `"diamond"` string check at line ~281 for currency-chest protection. If the base currency ever changes (unlikely, but not impossible via config), this fails silently. Not a current bug.

### src/store/handlers/info.rs
- [x] const `ITEMS_PER_PAGE`
- [x] const `ORDERS_PER_PAGE`
- [x] async fn `handle_price`
- [x] async fn `handle_balance`
- [x] async fn `handle_pay`
- [x] async fn `handle_items`
- [x] async fn `handle_queue`
- [x] async fn `handle_cancel`
- [x] async fn `handle_status`
- [x] async fn `handle_help`
- [x] async fn `handle_price_command`
- [x] async fn `handle_status_command`
- [x] async fn `handle_items_command`
- [x] async fn `handle_help_command`
- [x] async fn `get_user_balance_async`
- [x] async fn `pay_async`

No changes. `ITEMS_PER_PAGE` / `ORDERS_PER_PAGE` (both 4) are UI-pagination constants appropriately kept local. Help-text strings match COMMANDS.md page sizes.

**Observations:**
- Float arithmetic on balances may accumulate rounding errors; inherent to f64 representation — out of scope.
