### src/main.rs

- fn `main`
- fn `print_usage`
- fn `run_validate_only`
- fn `spawn_config_watcher`

### src/cli.rs

- fn `with_retry`
- fn `cli_task`
- fn `get_balances`
- fn `get_pairs`
- fn `set_operator`
- fn `add_node`
- fn `add_node_with_validation`
- fn `discover_storage`
- fn `remove_node`
- fn `add_pair`
- fn `remove_pair`
- fn `view_storage`
- fn `view_trades`
- fn `restart_bot`
- fn `clear_stuck_order`
- fn `audit_state`

### src/config.rs

- struct `Config`
- fn `default_trade_timeout_ms`
- fn `default_pathfinding_timeout_ms`
- fn `default_max_orders`
- fn `default_max_trades_in_memory`
- fn `default_autosave_interval_secs`
- impl Config :: fn `validate`
- impl Config :: fn `load`

### src/constants.rs

- const `DOUBLE_CHEST_SLOTS`
- const `SHULKER_BOX_SLOTS`
- const `HOTBAR_SLOT_0`
- const `TRADE_TIMEOUT_MS`
- const `CHEST_OP_TIMEOUT_SECS`
- const `PATHFINDING_TIMEOUT_MS`
- const `DELAY_SHORT_MS`
- const `PATHFINDING_CHECK_INTERVAL_MS`
- const `DELAY_MEDIUM_MS`
- const `DELAY_INTERACT_MS`
- const `DELAY_BLOCK_OP_MS`
- const `DELAY_LOOK_AT_MS`
- const `DELAY_SETTLE_MS`
- const `DELAY_VALIDATION_BETWEEN_CHESTS_MS`
- const `DELAY_SHULKER_PLACE_MS`
- const `DELAY_DISCONNECT_MS`
- const `DELAY_CONFIG_DEBOUNCE_MS`
- const `CHEST_OP_MAX_RETRIES`
- const `CHUNK_RELOAD_EXTRA_RETRIES`
- const `CHUNK_RELOAD_BASE_DELAY_MS`
- const `CHUNK_RELOAD_MAX_DELAY_MS`
- const `SHULKER_OP_MAX_RETRIES`
- const `NAVIGATION_MAX_RETRIES`
- const `RETRY_BASE_DELAY_MS`
- const `RETRY_MAX_DELAY_MS`
- const `FEE_MIN`
- const `FEE_MAX`
- const `MAX_TRANSACTION_QUANTITY`
- const `MIN_RESERVE_FOR_PRICE`
- const `CHESTS_PER_NODE`
- const `NODE_SPACING`
- const `OVERFLOW_CHEST_ITEM`
- const `DIAMOND_CHEST_ID`
- const `OVERFLOW_CHEST_ID`
- const `MAX_ORDERS_PER_USER`
- const `MAX_QUEUE_SIZE`
- const `QUEUE_FILE`
- const `RATE_LIMIT_BASE_COOLDOWN_MS`
- const `UUID_CACHE_TTL_SECS`
- const `RATE_LIMIT_MAX_COOLDOWN_MS`
- const `RATE_LIMIT_RESET_AFTER_MS`
- const `CLEANUP_INTERVAL_SECS`
- const `RATE_LIMIT_STALE_AFTER_SECS`
- fn `exponential_backoff_delay`

### src/error.rs

- enum `StoreError`
- impl `From<StoreError> for String` :: fn `from`
- impl `From<String> for StoreError` :: fn `from`

### src/fsutil.rs

- fn `write_atomic`

**Observations:**

- [ ] Add parent-directory `fsync` after rename in `write_atomic` to close the POSIX durability gap: a crash between rename and fsync can lose the name flip.
- [ ] Add a unit test for the rename-failure → copy-fallback path (inject failure via a test-only hook or `cfg`-gated wrapper so it runs portably).

### src/messages.rs

- struct `TradeItem`
- struct `ChestSyncReport`
- enum `QueuedOrderType`
- enum `ChestAction`
- enum `StoreMessage`
- enum `BotMessage`
- enum `CliMessage`
- enum `BotInstruction`

### src/types.rs

- pub mod `chest`
- pub mod `item_id`
- pub mod `node`
- pub mod `order`
- pub mod `pair`
- pub mod `position`
- pub mod `storage`
- pub mod `trade`
- pub mod `user`
- re-export surface (`Chest`, `ItemId`, `Node`, `Order`, `Pair`, `Position`, `Storage`, `Trade`, `TradeType`, `User`)

---

## types/

### src/types/position.rs

- struct `Position`

### src/types/item_id.rs

- struct `ItemId`
- impl ItemId :: const `EMPTY`
- impl ItemId :: fn `new`
- impl ItemId :: fn `from_normalized`
- impl ItemId :: fn `as_str`
- impl ItemId :: fn `with_minecraft_prefix`
- impl ItemId :: fn `is_empty`
- impl `Deref for ItemId` :: fn `deref`
- impl `Borrow<str> for ItemId` :: fn `borrow`
- impl `AsRef<str> for ItemId` :: fn `as_ref`
- impl `Display for ItemId` :: fn `fmt`
- impl `PartialEq<str> for ItemId` :: fn `eq`
- impl `PartialEq<&str> for ItemId` :: fn `eq`
- impl `PartialEq<String> for ItemId` :: fn `eq`
- impl `From<ItemId> for String` :: fn `from`
- impl `Default for ItemId` :: fn `default`
- tests module

**Observations:**

- [ ] Consolidate all `ItemId` construction onto `ItemId::new`. Production currently goes through `ItemId::from_normalized(...)` after `store::utils::normalize_item_id`; unifying gives one canonical entry point (touches dozens of files).
- [ ] Delete `store::utils::normalize_item_id` after the `ItemId::new` consolidation — its prefix-strip logic duplicates `ItemId::new`.
- [ ] Strengthen `ItemId`'s non-empty invariant: validate inside `from_normalized` too, or remove the `EMPTY` / `from_normalized` escape hatches so production code cannot bypass `new`.

### src/types/node.rs

- struct `Node`
- impl Node :: fn `new`
- impl Node :: fn `load`
- impl Node :: fn `save`
- impl Node :: fn `calc_position`
- impl Node :: fn `calc_chest_position`
- tests module

**Observations:**

- [ ] Migrate the whole `types/` layer from `eprintln!` to `tracing::warn!` (currently `node.rs`, `pair.rs`, `user.rs`, `trade.rs` all use `eprintln!` for non-fatal load/save warnings).
- [ ] Add a direct test for `Node::load`'s re-enforcement of node 0's reserved chests (diamond at index 0, overflow at index 1) — currently exercised only indirectly.

### src/types/chest.rs

- struct `Chest`
- impl Chest :: fn `new`
- impl Chest :: fn `calc_position`

### src/types/trade.rs

- struct `Trade`
- enum `TradeType`
- impl Trade :: fn `new`
- impl Trade :: fn `save`
- impl Trade :: fn `load_all_with_limit`
- impl Trade :: fn `save_all`

**Observations:**

- [ ] Make `Trade::load_all_with_limit` scalable: list filenames, sort lexicographically (RFC3339-with-dashes is chronological), take only the last `max_trades`, then deserialize. Current impl reads and parses every file before trimming.
- [ ] Guard `Trade::save_all` against empty-`Vec` input so it cannot silently wipe `data/trades`. Either return an error or require an explicit `clear_all` method for the destructive case.
- [ ] Fix the misleading comment claiming `Utc::now()` is "monotonic per process" (NTP can jump the wall-clock backwards); document the real collision bound (two trades at the same nanosecond) instead.

### src/types/order.rs

- struct `Order`
- enum `OrderType`
- impl Order :: fn `save_all_with_limit`

### src/types/pair.rs

- struct `Pair`
- impl Pair :: fn `shulker_capacity_for_stack_size`
- impl Pair :: fn `sanitize_item_name_for_filename`
- impl Pair :: fn `get_pair_file_path`
- impl Pair :: fn `save`
- impl Pair :: fn `load_all`
- impl Pair :: fn `save_all`

### src/types/user.rs

- static `HTTP_CLIENT`
- struct `User`
- struct `MojangResponse`
- fn `get_http_client`
- impl User :: async fn `get_uuid_async`
- impl User :: fn `get_user_file_path`
- impl User :: fn `save`
- impl User :: fn `load_all`
- impl User :: fn `save_all`

### src/types/storage.rs

- struct `ChestTransfer`
- struct `Storage`
- impl Storage :: const `SLOTS_PER_CHEST`
- impl Storage :: const `DEFAULT_SHULKER_CAPACITY`
- impl Storage :: fn `save`
- impl Storage :: fn `new`
- impl Storage :: fn `load`
- impl Storage :: fn `add_node`
- impl Storage :: fn `total_item_amount`
- impl Storage :: fn `get_chest_mut`
- impl Storage :: fn `withdraw_item`
- impl Storage :: fn `deposit_item`
- impl Storage :: fn `simulate_withdraw_plan`
- impl Storage :: fn `simulate_deposit_plan`
- impl Storage :: fn `withdraw_plan`
- impl Storage :: fn `deposit_plan`
- impl Storage :: fn `normalize_amounts_len`
- impl Storage :: fn `deposit_into_chest`
- impl Storage :: fn `find_empty_chest_index`
- impl Storage :: fn `get_overflow_chest`
- impl Storage :: fn `get_overflow_chest_mut`
- impl Storage :: fn `get_overflow_chest_position`
- impl Storage :: const fn `overflow_chest_id`
- tests module

**Observations:**

- [ ] Delete `Storage::DEFAULT_SHULKER_CAPACITY` if still unused. No production callers today; the stack-size-aware `Pair::shulker_capacity_for_stack_size` is the canonical path.
- [ ] Delete the `Storage::withdraw_item` / `Storage::deposit_item` convenience wrappers if still unused. Tests exercise `deposit_plan` / `withdraw_plan` directly.

---

## bot/

### src/bot/mod.rs

- pub mod declarations (`connection`, `navigation`, `trade`, `chest_io`, `shulker`, `inventory`)
- struct `BotState`
- struct `Bot`
- impl `Default for BotState` :: fn `default`
- impl Bot :: async fn `new`
- impl Bot :: async fn `send_chat_message`
- impl Bot :: async fn `send_whisper`
- impl Bot :: fn `normalize_item_id`
- impl Bot :: fn `chat_subscribe`
- async fn `bot_task`
- async fn `validate_node_physically`
- fn `handle_event_fn`
- async fn `handle_event`
- async fn `handle_chat_message`

**Observations:**

- [ ] Promote the 20s post-reconnect init wait and ~2s shutdown buffer in `bot_task` to named crate constants if a second caller ever emerges. Not worth doing speculatively today.

### src/bot/connection.rs

- async fn `connect`
- async fn `disconnect`

### src/bot/navigation.rs

- async fn `navigate_to_position_once`
- async fn `navigate_to_position`
- async fn `go_to_node`
- async fn `go_to_chest`

### src/bot/inventory.rs

- async fn `ensure_inventory_empty`
- async fn `move_hotbar_to_inventory`
- async fn `quick_move_from_container`
- fn `verify_holding_shulker`
- fn `is_entity_ready`
- async fn `wait_for_entity_ready`
- fn `carried_item`
- async fn `ensure_shulker_in_hotbar_slot_0`
- async fn `recover_shulker_to_slot_0`

**Observations:**

- [ ] Refactor `ensure_shulker_in_hotbar_slot_0` (~400 lines of click-then-verify logic): extract a `place_shulker_in_hotbar_slot_0(source)` helper to collapse the three nested "place shulker" branches (cursor-empty / cursor-occupied / other-slot).

### src/bot/chest_io.rs

- const `CHUNK_NOT_LOADED_PREFIX`
- fn `find_shulker_in_inventory_view`
- async fn `place_shulker_in_chest_slot_verified`
- async fn `open_chest_container_once`
- async fn `open_chest_container_for_validation`
- async fn `open_chest_container`
- async fn `transfer_items_with_shulker`
- async fn `transfer_withdraw_from_shulker`
- async fn `transfer_deposit_into_shulker`
- async fn `prepare_for_chest_io`
- async fn `automated_chest_io`
- async fn `withdraw_shulkers`
- async fn `deposit_shulkers`

**Observations:**

- [ ] Extract a `ShulkerRoundTrip` helper from `withdraw_shulkers` / `deposit_shulkers` — the two paths share ~400 lines of cursor-clear / take-shulker / close-chest / hotbar-slot-0 / station / open-shulker / pickup / reopen / put-back skeleton.
- [ ] Change `slot_counts: Vec<i32>` in `automated_chest_io` to `[i32; DOUBLE_CHEST_SLOTS]` (fixed size, no alloc, invariant-encoded). Requires updating `ChestSyncReport` and all callers — the Vec propagates through the serialization layer.

### src/bot/shulker.rs

- const `SHULKER_BOX_IDS`
- fn `shulker_station_position`
- fn `is_shulker_box`
- fn `validate_chest_slot_is_shulker` (cfg(test))
- async fn `pickup_shulker_from_station`
- async fn `open_shulker_at_station_once`
- async fn `open_shulker_at_station`
- test `test_is_shulker_box`
- test `test_validate_chest_slot_is_shulker`
- test `test_shulker_station_position`

### src/bot/trade.rs

- fn `trade_bot_offer_slots`
- fn `trade_player_offer_slots`
- fn `trade_player_status_slots`
- fn `trade_accept_slots`
- fn `trade_cancel_slots`
- async fn `wait_for_trade_menu_or_failure`
- async fn `place_items_from_inventory_into_trade`
- fn `validate_player_items`
- async fn `execute_trade_with_player`
- test `test_trade_bot_offer_slots`
- test `test_trade_player_offer_slots`
- test `test_trade_player_status_slots`
- test `test_trade_accept_slots`
- test `test_trade_cancel_slots`
- test `test_trade_slot_sets_disjoint`

---

## store/

### src/store/mod.rs

- pub mod declarations (`command`, `handlers`, `journal`, `orders`, `pricing`, `queue`, `rate_limit`, `rollback`, `state`, `trade_state`, `utils`)
- struct `Store`
- impl Store :: async fn `new`
- impl Store :: async fn `run`
- impl Store :: async fn `process_next_order`
- impl Store :: fn `reload_config`
- impl Store :: fn `advance_trade`
- impl Store :: async fn `handle_bot_message`
- impl Store :: fn `expect_pair`
- impl Store :: fn `expect_pair_mut`
- impl Store :: fn `expect_user`
- impl Store :: fn `expect_user_mut`
- impl Store :: fn `apply_chest_sync`
- impl Store :: fn `get_node_position`
- impl Store :: fn `new_for_test`

### src/store/state.rs

- fn `apply_chest_sync`
- fn `save`
- fn `audit_state`
- fn `assert_invariants`

### src/store/command.rs

- enum `Command`
- fn `parse_command`
- fn `parse_item_quantity`
- fn `parse_item_amount`
- fn `parse_optional_amount`
- fn `parse_price`
- fn `parse_balance`
- fn `parse_pay`
- fn `parse_page`
- fn `parse_cancel`
- tests module

### src/store/journal.rs

- const `JOURNAL_FILE`
- static `NEXT_OPERATION_ID`
- type alias `SharedJournal`
- struct `JournalEntry`
- struct `Journal`
- enum `JournalOp`
- enum `JournalState`
- impl `Default for Journal` :: fn `default`
- impl Journal :: fn `load`
- impl Journal :: fn `load_from`
- impl Journal :: fn `clear_leftover`
- impl Journal :: fn `begin`
- impl Journal :: fn `advance`
- impl Journal :: fn `complete`
- impl Journal :: fn `current`
- impl Journal :: fn `persist`
- tests module

### src/store/orders.rs

- struct `BuyPlan`
- struct `SellPlan`
- enum `ChestDirection`
- async fn `execute_chest_transfers`
- async fn `perform_trade`
- async fn `validate_and_plan_buy`
- async fn `handle_buy_order`
- async fn `validate_and_plan_sell`
- async fn `handle_sell_order`
- async fn `execute_queued_order`
- tests module

**Observations:**

- [ ] Clarify `spawn_mock_bot`'s direction ambiguity: either rename `player_offers` to something direction-neutral or split the mock into per-direction helpers, so future test authors don't misread the echo-back logic.

### src/store/pricing.rs

- fn `validate_fee`
- fn `reserves_sufficient`
- fn `calculate_buy_cost`
- fn `buy_cost_pure`
- fn `calculate_sell_payout`
- fn `sell_payout_pure`
- tests module (includes proptests)

### src/store/queue.rs

- struct `QueuedOrder`
- struct `OrderQueue`
- struct `QueuePersist`
- impl QueuedOrder :: fn `new`
- impl QueuedOrder :: fn `description`
- impl `Default for OrderQueue` :: fn `default`
- impl OrderQueue :: fn `new`
- impl OrderQueue :: fn `load`
- impl OrderQueue :: fn `save`
- impl OrderQueue :: fn `add`
- impl OrderQueue :: fn `pop`
- impl OrderQueue :: fn `is_empty`
- impl OrderQueue :: fn `len`
- impl OrderQueue :: fn `get_position`
- impl OrderQueue :: fn `get_user_position`
- impl OrderQueue :: fn `user_order_count`
- impl OrderQueue :: fn `get_user_orders`
- impl OrderQueue :: fn `cancel`
- impl OrderQueue :: fn `estimate_wait`
- tests module

### src/store/rate_limit.rs

- struct `UserRateLimit`
- struct `RateLimiter`
- fn `calculate_cooldown`
- impl UserRateLimit :: fn `new`
- impl `Default for RateLimiter` :: fn `default`
- impl RateLimiter :: fn `new`
- impl RateLimiter :: fn `check`
- impl RateLimiter :: fn `cleanup_stale`
- tests module

### src/store/rollback.rs

- struct `RollbackResult`
- impl RollbackResult :: fn `has_failures`
- fn `chest_from_transfer`
- async fn `deposit_transfers`
- async fn `rollback_amount_to_storage`

### src/store/trade_state.rs

- const `TRADE_STATE_FILE`
- struct `TradeResult`
- struct `CompletedTrade`
- enum `TradeState`
- impl TradeState :: fn `new`
- impl TradeState :: fn `begin_withdrawal`
- impl TradeState :: fn `begin_trading`
- impl TradeState :: fn `begin_depositing`
- impl TradeState :: fn `commit`
- impl TradeState :: fn `rollback`
- impl TradeState :: fn `phase`
- impl TradeState :: fn `is_terminal`
- impl TradeState :: fn `order`
- impl `fmt::Display for TradeState` :: fn `fmt`
- fn `persist`
- fn `load_persisted`
- fn `clear_persisted`
- tests module

### src/store/utils.rs

- static `UUID_CACHE`
- type alias `UuidCache`
- fn `uuid_cache`
- fn `normalize_item_id`
- async fn `resolve_user_uuid`
- fn `clear_uuid_cache`
- fn `cleanup_uuid_cache`
- fn `ensure_user_exists`
- fn `is_operator`
- fn `get_node_position`
- async fn `send_message_to_player`
- fn `summarize_transfers`
- fn `fmt_issues`
- tests module

### src/store/handlers/mod.rs

- pub mod `player`
- pub mod `operator`
- pub mod `cli`
- mod `buy`
- mod `sell`
- mod `deposit`
- mod `withdraw`
- mod `info`
- pub(crate) mod `validation`

### src/store/handlers/validation.rs

- fn `validate_item_name`
- fn `validate_quantity`
- fn `validate_username`

### src/store/handlers/buy.rs

- async fn `handle`

### src/store/handlers/sell.rs

- async fn `handle`

### src/store/handlers/withdraw.rs

- async fn `handle_enqueue`
- async fn `handle_withdraw_balance_queued`

### src/store/handlers/deposit.rs

- async fn `handle_enqueue`
- async fn `handle_deposit_balance_queued`

### src/store/handlers/player.rs

- async fn `handle_player_command`

### src/store/handlers/operator.rs

- async fn `handle_additem_order`
- async fn `handle_removeitem_order`
- async fn `handle_add_currency`
- async fn `handle_remove_currency`

**Observations:**

- [ ] Handle rollback-during-rollback failure in the operator two-phase commit. A second failure currently strands items in the bot's inventory with no automatic remediation; at minimum, persist the stuck state and alert the operator (or the player whose trade was in flight).
- [ ] Harden the negative-stock guards in `handlers/operator.rs`: `debug_assert!` only fires in debug builds, so production integer-underflow can silently corrupt stocks. Promote to `assert!` or add an invariant check in the save path.

### src/store/handlers/cli.rs

- async fn `handle_cli_message`

**Observations:**

- [ ] Replace the hardcoded `"diamond"` string check in `handle_cli_message`'s currency-chest protection with a named constant (or config-driven value) so changing the base currency doesn't silently bypass the guard.

### src/store/handlers/info.rs

- async fn `handle_price`
- async fn `handle_balance`
- async fn `handle_pay`
- async fn `handle_items`
- async fn `handle_queue`
- async fn `handle_cancel`
- async fn `handle_status`
- async fn `handle_help`
- async fn `handle_price_command`
- async fn `handle_status_command`
- async fn `handle_items_command`
- async fn `handle_help_command`
- async fn `get_user_balance_async`
- async fn `pay_async`

**Observations:**

- [ ] Evaluate migrating user balances from `f64` to an integer representation (millidiamonds or similar) to eliminate accumulated rounding error on long histories.
