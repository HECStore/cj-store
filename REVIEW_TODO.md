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
- [ ] const `DOUBLE_CHEST_SLOTS`
- [ ] const `SHULKER_BOX_SLOTS`
- [ ] const `DEFAULT_STACK_SIZE`
- [ ] const `HOTBAR_SLOT_0`
- [ ] const `INVENTORY_SLOT_START`
- [ ] const `INVENTORY_SLOT_END`
- [ ] const `CHEST_OPEN_TIMEOUT_TICKS`
- [ ] const `TRADE_TIMEOUT_MS`
- [ ] const `TRADE_WAIT_TIMEOUT_MS`
- [ ] const `CHEST_OP_TIMEOUT_SECS`
- [ ] const `PATHFINDING_TIMEOUT_MS`
- [ ] const `CLIENT_INIT_TIMEOUT_MS`
- [ ] const `DELAY_SHORT_MS`
- [ ] const `DELAY_MEDIUM_MS`
- [ ] const `DELAY_INTERACT_MS`
- [ ] const `DELAY_BLOCK_OP_MS`
- [ ] const `DELAY_LOOK_AT_MS`
- [ ] const `DELAY_SETTLE_MS`
- [ ] const `DELAY_NETWORK_MS`
- [ ] const `DELAY_VALIDATION_BETWEEN_CHESTS_MS`
- [ ] const `DELAY_SHULKER_PLACE_MS`
- [ ] const `DELAY_DISCONNECT_MS`
- [ ] const `DELAY_CONFIG_DEBOUNCE_MS`
- [ ] const `DELAY_DISCONNECT_BUFFER_MS`
- [ ] const `RECONNECT_INITIAL_BACKOFF_SECS`
- [ ] const `RECONNECT_MAX_BACKOFF_SECS`
- [ ] const `CONNECTION_CHECK_INTERVAL_SECS`
- [ ] const `CHEST_OP_MAX_RETRIES`
- [ ] const `CHUNK_RELOAD_EXTRA_RETRIES`
- [ ] const `CHUNK_RELOAD_BASE_DELAY_MS`
- [ ] const `CHUNK_RELOAD_MAX_DELAY_MS`
- [ ] const `SHULKER_OP_MAX_RETRIES`
- [ ] const `NAVIGATION_MAX_RETRIES`
- [ ] const `RETRY_BASE_DELAY_MS`
- [ ] const `RETRY_MAX_DELAY_MS`
- [ ] const `FEE_MIN`
- [ ] const `FEE_MAX`
- [ ] const `MAX_TRANSACTION_QUANTITY`
- [ ] const `MIN_RESERVE_FOR_PRICE`
- [ ] const `CHESTS_PER_NODE`
- [ ] const `NODE_SPACING`
- [ ] const `OVERFLOW_CHEST_ITEM`
- [ ] const `DIAMOND_CHEST_ID`
- [ ] const `OVERFLOW_CHEST_ID`
- [ ] const `MAX_ORDERS_PER_USER`
- [ ] const `MAX_QUEUE_SIZE`
- [ ] const `QUEUE_FILE`
- [ ] const `RATE_LIMIT_BASE_COOLDOWN_MS`
- [ ] const `UUID_CACHE_TTL_SECS`
- [ ] const `RATE_LIMIT_MAX_COOLDOWN_MS`
- [ ] const `RATE_LIMIT_RESET_AFTER_MS`
- [ ] const `CLEANUP_INTERVAL_SECS`
- [ ] const `RATE_LIMIT_STALE_AFTER_SECS`
- [ ] fn `exponential_backoff_delay`

### src/error.rs
- [ ] enum `StoreError`
- [ ] impl `From<StoreError> for String` :: fn `from`
- [ ] impl `From<String> for StoreError` :: fn `from`

### src/fsutil.rs
- [ ] fn `write_atomic`

### src/messages.rs
- [ ] struct `TradeItem`
- [ ] struct `ChestSyncReport`
- [ ] enum `QueuedOrderType`
- [ ] enum `ChestAction`
- [ ] enum `StoreMessage`
- [ ] enum `BotMessage`
- [ ] enum `CliMessage`
- [ ] enum `BotInstruction`

### src/types.rs
- [ ] module re-export surface (verify nothing leaks / nothing missing)

---

## types/

### src/types/position.rs
- [ ] struct `Position`

### src/types/item_id.rs
- [ ] struct `ItemId`
- [ ] impl ItemId :: const `EMPTY`
- [ ] impl ItemId :: fn `new`
- [ ] impl ItemId :: fn `from_normalized`
- [ ] impl ItemId :: fn `as_str`
- [ ] impl ItemId :: fn `with_minecraft_prefix`
- [ ] impl ItemId :: fn `is_empty`
- [ ] impl `Deref for ItemId` :: fn `deref`
- [ ] impl `Borrow<str> for ItemId` :: fn `borrow`
- [ ] impl `AsRef<str> for ItemId` :: fn `as_ref`
- [ ] impl `Display for ItemId` :: fn `fmt`
- [ ] impl `PartialEq<str> for ItemId` :: fn `eq`
- [ ] impl `PartialEq<&str> for ItemId` :: fn `eq`
- [ ] impl `PartialEq<String> for ItemId` :: fn `eq`
- [ ] impl `From<ItemId> for String` :: fn `from`
- [ ] impl `Default for ItemId` :: fn `default`

### src/types/node.rs
- [ ] struct `Node`
- [ ] impl Node :: fn `new`
- [ ] impl Node :: fn `load`
- [ ] impl Node :: fn `save`
- [ ] impl Node :: fn `calc_position`
- [ ] impl Node :: fn `calc_chest_position`

### src/types/chest.rs
- [ ] struct `Chest`
- [ ] impl Chest :: fn `new`

### src/types/trade.rs
- [ ] struct `Trade`
- [ ] enum `TradeType`
- [ ] impl Trade :: fn `new`
- [ ] impl Trade :: fn `save`
- [ ] impl Trade :: fn `load_all_with_limit`
- [ ] impl Trade :: fn `save_all`

### src/types/order.rs
- [ ] struct `Order`
- [ ] enum `OrderType`
- [ ] impl Order :: fn `save_all_with_limit`

### src/types/pair.rs
- [ ] struct `Pair`
- [ ] impl Pair :: fn `shulker_capacity_for_stack_size`
- [ ] impl Pair :: fn `sanitize_item_name_for_filename`
- [ ] impl Pair :: fn `get_pair_file_path`
- [ ] impl Pair :: fn `save`
- [ ] impl Pair :: fn `load_all`
- [ ] impl Pair :: fn `save_all`

### src/types/user.rs
- [ ] struct `User`
- [ ] fn `get_http_client`
- [ ] impl User :: async fn `get_uuid_async`
- [ ] impl User :: fn `get_user_file_path`
- [ ] impl User :: fn `save`
- [ ] impl User :: fn `load_all`
- [ ] impl User :: fn `save_all`

### src/types/storage.rs
- [ ] struct `ChestTransfer`
- [ ] struct `Storage`
- [ ] impl Storage :: fn `save`
- [ ] impl Storage :: fn `new`
- [ ] impl Storage :: fn `load`
- [ ] impl Storage :: fn `add_node`
- [ ] impl Storage :: fn `total_item_amount`
- [ ] impl Storage :: fn `get_chest_mut`
- [ ] impl Storage :: fn `withdraw_item`
- [ ] impl Storage :: fn `deposit_item`
- [ ] impl Storage :: fn `simulate_withdraw_plan`
- [ ] impl Storage :: fn `simulate_deposit_plan`
- [ ] impl Storage :: fn `withdraw_plan`
- [ ] impl Storage :: fn `deposit_plan`
- [ ] impl Storage :: fn `normalize_amounts_len`
- [ ] impl Storage :: fn `deposit_into_chest`
- [ ] impl Storage :: fn `find_empty_chest_index`
- [ ] impl Storage :: fn `get_overflow_chest`
- [ ] impl Storage :: fn `get_overflow_chest_mut`
- [ ] impl Storage :: fn `get_overflow_chest_position`
- [ ] impl Storage :: const fn `overflow_chest_id`

---

## bot/

### src/bot/mod.rs
- [ ] struct `BotState`
- [ ] struct `Bot`
- [ ] fn `bot_task`
- [ ] fn `validate_node_physically`
- [ ] fn `handle_event_fn`
- [ ] fn `handle_event`
- [ ] fn `handle_chat_message`
- [ ] impl BotState :: fn `default`
- [ ] impl Bot :: async fn `new`
- [ ] impl Bot :: async fn `send_chat_message`
- [ ] impl Bot :: async fn `send_whisper`
- [ ] impl Bot :: fn `normalize_item_id`
- [ ] impl Bot :: fn `chat_subscribe`

### src/bot/connection.rs
- [ ] async fn `connect`
- [ ] async fn `disconnect`

### src/bot/navigation.rs
- [ ] const `PATHFINDING_CHECK_INTERVAL_MS`
- [ ] async fn `navigate_to_position_once`
- [ ] async fn `navigate_to_position`
- [ ] async fn `go_to_node`
- [ ] async fn `go_to_chest`

### src/bot/inventory.rs
- [ ] async fn `ensure_inventory_empty`
- [ ] async fn `move_hotbar_to_inventory`
- [ ] async fn `quick_move_from_container`
- [ ] fn `verify_holding_shulker`
- [ ] fn `is_entity_ready`
- [ ] async fn `wait_for_entity_ready`
- [ ] fn `carried_item`
- [ ] async fn `ensure_shulker_in_hotbar_slot_0`
- [ ] async fn `recover_shulker_to_slot_0`

### src/bot/chest_io.rs
- [ ] const `CHUNK_NOT_LOADED_PREFIX`
- [ ] fn `find_shulker_in_inventory_view`
- [ ] async fn `place_shulker_in_chest_slot_verified`
- [ ] async fn `open_chest_container_once`
- [ ] async fn `open_chest_container_for_validation`
- [ ] async fn `open_chest_container`
- [ ] async fn `transfer_items_with_shulker`
- [ ] async fn `transfer_withdraw_from_shulker`
- [ ] async fn `transfer_deposit_into_shulker`
- [ ] async fn `prepare_for_chest_io`
- [ ] async fn `automated_chest_io`
- [ ] async fn `withdraw_shulkers`
- [ ] async fn `deposit_shulkers`

### src/bot/shulker.rs
- [ ] const `SHULKER_BOX_IDS`
- [ ] fn `shulker_station_position`
- [ ] fn `is_shulker_box`
- [ ] fn `validate_chest_slot_is_shulker` (cfg(test))
- [ ] async fn `pickup_shulker_from_station`
- [ ] async fn `open_shulker_at_station_once`
- [ ] async fn `open_shulker_at_station`
- [ ] test `test_is_shulker_box`
- [ ] test `test_validate_chest_slot_is_shulker`
- [ ] test `test_shulker_station_position`

### src/bot/trade.rs
- [ ] fn `trade_bot_offer_slots`
- [ ] fn `trade_player_offer_slots`
- [ ] fn `trade_player_status_slots`
- [ ] fn `trade_accept_slots`
- [ ] fn `trade_cancel_slots`
- [ ] async fn `wait_for_trade_menu_or_failure`
- [ ] async fn `place_items_from_inventory_into_trade`
- [ ] fn `validate_player_items`
- [ ] async fn `execute_trade_with_player`
- [ ] test `test_trade_bot_offer_slots`
- [ ] test `test_trade_player_offer_slots`
- [ ] test `test_trade_player_status_slots`
- [ ] test `test_trade_accept_slots`
- [ ] test `test_trade_cancel_slots`
- [ ] test `test_trade_slot_sets_disjoint`

---

## store/

### src/store/mod.rs
- [ ] struct `Store`
- [ ] impl Store :: fn `new`
- [ ] impl Store :: async fn `run`
- [ ] impl Store :: async fn `process_next_order`
- [ ] impl Store :: fn `reload_config`
- [ ] impl Store :: fn `advance_trade`
- [ ] impl Store :: async fn `handle_bot_message`
- [ ] impl Store :: fn `expect_pair`
- [ ] impl Store :: fn `expect_pair_mut`
- [ ] impl Store :: fn `expect_user`
- [ ] impl Store :: fn `expect_user_mut`
- [ ] impl Store :: fn `apply_chest_sync`
- [ ] impl Store :: fn `get_node_position`
- [ ] impl Store :: fn `new_for_test`

### src/store/state.rs
- [ ] fn `apply_chest_sync`
- [ ] fn `save`
- [ ] fn `audit_state`
- [ ] fn `assert_invariants`

### src/store/command.rs
- [ ] enum `Command`
- [ ] fn `parse_command`
- [ ] fn `parse_item_quantity`
- [ ] fn `parse_item_amount`
- [ ] fn `parse_optional_amount`
- [ ] fn `parse_price`
- [ ] fn `parse_balance`
- [ ] fn `parse_pay`
- [ ] fn `parse_page`
- [ ] fn `parse_cancel`
- [ ] tests module (enumerate and check each individual test during review)

### src/store/journal.rs
- [ ] const `JOURNAL_FILE`
- [ ] type alias `SharedJournal`
- [ ] struct `Journal`
- [ ] struct `JournalEntry`
- [ ] enum `JournalOp`
- [ ] enum `JournalState`
- [ ] impl Journal :: fn `load`
- [ ] impl Journal :: fn `load_from`
- [ ] impl Journal :: fn `clear_leftover`
- [ ] impl Journal :: fn `begin`
- [ ] impl Journal :: fn `advance`
- [ ] impl Journal :: fn `complete`
- [ ] impl Journal :: fn `current`
- [ ] impl Journal :: fn `persist`
- [ ] impl `Default for Journal` :: fn `default`
- [ ] tests module (enumerate and check each individual test during review)

### src/store/orders.rs
- [ ] struct `BuyPlan`
- [ ] struct `SellPlan`
- [ ] enum `ChestDirection`
- [ ] async fn `execute_chest_transfers`
- [ ] async fn `perform_trade`
- [ ] async fn `validate_and_plan_buy`
- [ ] async fn `handle_buy_order`
- [ ] async fn `validate_and_plan_sell`
- [ ] async fn `handle_sell_order`
- [ ] async fn `execute_queued_order`
- [ ] tests module (enumerate and check each individual test during review)

### src/store/pricing.rs
- [ ] const `TEST_FEE`
- [ ] fn `validate_fee`
- [ ] fn `reserves_sufficient`
- [ ] fn `calculate_buy_cost`
- [ ] fn `buy_cost_pure`
- [ ] fn `calculate_sell_payout`
- [ ] fn `sell_payout_pure`
- [ ] tests module incl. proptests (enumerate and check each individual test during review)

### src/store/queue.rs
- [ ] struct `OrderQueue`
- [ ] struct `QueuePersist`
- [ ] impl QueuedOrder :: fn `new`
- [ ] impl QueuedOrder :: fn `description`
- [ ] impl `Default for OrderQueue` :: fn `default`
- [ ] impl OrderQueue :: fn `new`
- [ ] impl OrderQueue :: fn `load`
- [ ] impl OrderQueue :: fn `save`
- [ ] impl OrderQueue :: fn `add`
- [ ] impl OrderQueue :: fn `pop`
- [ ] impl OrderQueue :: fn `is_empty`
- [ ] impl OrderQueue :: fn `len`
- [ ] impl OrderQueue :: fn `get_position`
- [ ] impl OrderQueue :: fn `get_user_position`
- [ ] impl OrderQueue :: fn `user_order_count`
- [ ] impl OrderQueue :: fn `get_user_orders`
- [ ] impl OrderQueue :: fn `cancel`
- [ ] impl OrderQueue :: fn `estimate_wait`
- [ ] tests module (enumerate and check each individual test during review)

### src/store/rate_limit.rs
- [ ] struct `UserRateLimit`
- [ ] struct `RateLimiter`
- [ ] fn `calculate_cooldown`
- [ ] impl UserRateLimit :: fn `new`
- [ ] impl `Default for RateLimiter` :: fn `default`
- [ ] impl RateLimiter :: fn `new`
- [ ] impl RateLimiter :: fn `check`
- [ ] impl RateLimiter :: fn `cleanup_stale`
- [ ] tests module (enumerate and check each individual test during review)

### src/store/rollback.rs
- [ ] struct `RollbackResult`
- [ ] fn `chest_from_transfer`
- [ ] async fn `deposit_transfers`
- [ ] async fn `rollback_amount_to_storage`
- [ ] impl RollbackResult :: fn `has_failures`

### src/store/trade_state.rs
- [ ] const `TRADE_STATE_FILE`
- [ ] struct `TradeResult`
- [ ] struct `CompletedTrade`
- [ ] enum `TradeState`
- [ ] fn `persist`
- [ ] fn `load_persisted`
- [ ] fn `clear_persisted`
- [ ] impl TradeState :: fn `new`
- [ ] impl TradeState :: fn `begin_withdrawal`
- [ ] impl TradeState :: fn `begin_trading`
- [ ] impl TradeState :: fn `begin_depositing`
- [ ] impl TradeState :: fn `commit`
- [ ] impl TradeState :: fn `rollback`
- [ ] impl TradeState :: fn `phase`
- [ ] impl TradeState :: fn `is_terminal`
- [ ] impl TradeState :: fn `order`
- [ ] impl `fmt::Display for TradeState` :: fn `fmt`
- [ ] tests module (enumerate and check each individual test during review)

### src/store/utils.rs
- [ ] const `UUID_CACHE`
- [ ] type alias `UuidCache`
- [ ] fn `uuid_cache`
- [ ] fn `normalize_item_id`
- [ ] async fn `resolve_user_uuid`
- [ ] fn `clear_uuid_cache`
- [ ] fn `cleanup_uuid_cache`
- [ ] fn `ensure_user_exists`
- [ ] fn `is_operator`
- [ ] fn `get_node_position`
- [ ] async fn `send_message_to_player`
- [ ] fn `summarize_transfers`
- [ ] fn `fmt_issues`
- [ ] tests module (enumerate and check each individual test during review)

### src/store/handlers/mod.rs
- [ ] module declarations surface (verify expected submodules present)

### src/store/handlers/validation.rs
- [ ] fn `validate_item_name`
- [ ] fn `validate_quantity`
- [ ] fn `validate_username`

### src/store/handlers/buy.rs
- [ ] async fn `handle`

### src/store/handlers/sell.rs
- [ ] async fn `handle`

### src/store/handlers/withdraw.rs
- [ ] async fn `handle_enqueue`
- [ ] async fn `handle_withdraw_balance_queued`

### src/store/handlers/deposit.rs
- [ ] async fn `handle_enqueue`
- [ ] async fn `handle_deposit_balance_queued`

### src/store/handlers/player.rs
- [ ] async fn `handle_player_command`
- [ ] (re-check file for any additional free fns not captured during initial inventory)

### src/store/handlers/operator.rs
- [ ] async fn `handle_additem_order`
- [ ] async fn `handle_removeitem_order`
- [ ] async fn `handle_add_currency`
- [ ] async fn `handle_remove_currency`

### src/store/handlers/cli.rs
- [ ] async fn `handle_cli_message` (verify every `CliMessage` variant has an arm)

### src/store/handlers/info.rs
- [ ] const `ITEMS_PER_PAGE`
- [ ] const `ORDERS_PER_PAGE`
- [ ] async fn `handle_price`
- [ ] async fn `handle_balance`
- [ ] async fn `handle_pay`
- [ ] async fn `handle_items`
- [ ] async fn `handle_queue`
- [ ] async fn `handle_cancel`
- [ ] async fn `handle_status`
- [ ] async fn `handle_help`
- [ ] async fn `handle_price_command`
- [ ] async fn `handle_status_command`
- [ ] async fn `handle_items_command`
- [ ] async fn `handle_help_command`
- [ ] async fn `get_user_balance_async`
- [ ] async fn `pay_async`
