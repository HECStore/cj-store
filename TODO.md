### src/main.rs

- fn `main`
- fn `print_usage`
- fn `run_validate_only`
- fn `spawn_config_watcher`

**TODO:**

- [ ] Review comments: fn `main`
- [ ] Review comments: fn `print_usage`
- [ ] Review comments: fn `run_validate_only`
- [ ] Review comments: fn `spawn_config_watcher`

- [ ] Review testability: fn `main`
- [ ] Review testability: fn `print_usage`
- [ ] Review testability: fn `run_validate_only`
- [ ] Review testability: fn `spawn_config_watcher`

- [ ] Review logging: fn `main`
- [ ] Review logging: fn `print_usage`
- [ ] Review logging: fn `run_validate_only`
- [ ] Review logging: fn `spawn_config_watcher`

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

**TODO:**

- [ ] Review comments: fn `with_retry`
- [ ] Review comments: fn `cli_task`
- [ ] Review comments: fn `get_balances`
- [ ] Review comments: fn `get_pairs`
- [ ] Review comments: fn `set_operator`
- [ ] Review comments: fn `add_node`
- [ ] Review comments: fn `add_node_with_validation`
- [ ] Review comments: fn `discover_storage`
- [ ] Review comments: fn `remove_node`
- [ ] Review comments: fn `add_pair`
- [ ] Review comments: fn `remove_pair`
- [ ] Review comments: fn `view_storage`
- [ ] Review comments: fn `view_trades`
- [ ] Review comments: fn `restart_bot`
- [ ] Review comments: fn `clear_stuck_order`
- [ ] Review comments: fn `audit_state`

- [ ] Review testability: fn `with_retry`
- [ ] Review testability: fn `cli_task`
- [ ] Review testability: fn `get_balances`
- [ ] Review testability: fn `get_pairs`
- [ ] Review testability: fn `set_operator`
- [ ] Review testability: fn `add_node`
- [ ] Review testability: fn `add_node_with_validation`
- [ ] Review testability: fn `discover_storage`
- [ ] Review testability: fn `remove_node`
- [ ] Review testability: fn `add_pair`
- [ ] Review testability: fn `remove_pair`
- [ ] Review testability: fn `view_storage`
- [ ] Review testability: fn `view_trades`
- [ ] Review testability: fn `restart_bot`
- [ ] Review testability: fn `clear_stuck_order`
- [ ] Review testability: fn `audit_state`

- [ ] Review logging: fn `with_retry`
- [ ] Review logging: fn `cli_task`
- [ ] Review logging: fn `get_balances`
- [ ] Review logging: fn `get_pairs`
- [ ] Review logging: fn `set_operator`
- [ ] Review logging: fn `add_node`
- [ ] Review logging: fn `add_node_with_validation`
- [ ] Review logging: fn `discover_storage`
- [ ] Review logging: fn `remove_node`
- [ ] Review logging: fn `add_pair`
- [ ] Review logging: fn `remove_pair`
- [ ] Review logging: fn `view_storage`
- [ ] Review logging: fn `view_trades`
- [ ] Review logging: fn `restart_bot`
- [ ] Review logging: fn `clear_stuck_order`
- [ ] Review logging: fn `audit_state`

### src/config.rs

- struct `Config`
- fn `default_trade_timeout_ms`
- fn `default_pathfinding_timeout_ms`
- fn `default_max_orders`
- fn `default_max_trades_in_memory`
- fn `default_autosave_interval_secs`
- impl Config :: fn `validate`
- impl Config :: fn `load`

**TODO:**

- [ ] Review comments: struct `Config`
- [ ] Review comments: fn `default_trade_timeout_ms`
- [ ] Review comments: fn `default_pathfinding_timeout_ms`
- [ ] Review comments: fn `default_max_orders`
- [ ] Review comments: fn `default_max_trades_in_memory`
- [ ] Review comments: fn `default_autosave_interval_secs`
- [ ] Review comments: impl Config :: fn `validate`
- [ ] Review comments: impl Config :: fn `load`

- [ ] Review testability: struct `Config`
- [ ] Review testability: fn `default_trade_timeout_ms`
- [ ] Review testability: fn `default_pathfinding_timeout_ms`
- [ ] Review testability: fn `default_max_orders`
- [ ] Review testability: fn `default_max_trades_in_memory`
- [ ] Review testability: fn `default_autosave_interval_secs`
- [ ] Review testability: impl Config :: fn `validate`
- [ ] Review testability: impl Config :: fn `load`

- [ ] Review logging: struct `Config`
- [ ] Review logging: fn `default_trade_timeout_ms`
- [ ] Review logging: fn `default_pathfinding_timeout_ms`
- [ ] Review logging: fn `default_max_orders`
- [ ] Review logging: fn `default_max_trades_in_memory`
- [ ] Review logging: fn `default_autosave_interval_secs`
- [ ] Review logging: impl Config :: fn `validate`
- [ ] Review logging: impl Config :: fn `load`

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

**TODO:**

- [ ] Review comments: const `DOUBLE_CHEST_SLOTS`
- [ ] Review comments: const `SHULKER_BOX_SLOTS`
- [ ] Review comments: const `HOTBAR_SLOT_0`
- [ ] Review comments: const `TRADE_TIMEOUT_MS`
- [ ] Review comments: const `CHEST_OP_TIMEOUT_SECS`
- [ ] Review comments: const `PATHFINDING_TIMEOUT_MS`
- [ ] Review comments: const `DELAY_SHORT_MS`
- [ ] Review comments: const `PATHFINDING_CHECK_INTERVAL_MS`
- [ ] Review comments: const `DELAY_MEDIUM_MS`
- [ ] Review comments: const `DELAY_INTERACT_MS`
- [ ] Review comments: const `DELAY_BLOCK_OP_MS`
- [ ] Review comments: const `DELAY_LOOK_AT_MS`
- [ ] Review comments: const `DELAY_SETTLE_MS`
- [ ] Review comments: const `DELAY_VALIDATION_BETWEEN_CHESTS_MS`
- [ ] Review comments: const `DELAY_SHULKER_PLACE_MS`
- [ ] Review comments: const `DELAY_DISCONNECT_MS`
- [ ] Review comments: const `DELAY_CONFIG_DEBOUNCE_MS`
- [ ] Review comments: const `CHEST_OP_MAX_RETRIES`
- [ ] Review comments: const `CHUNK_RELOAD_EXTRA_RETRIES`
- [ ] Review comments: const `CHUNK_RELOAD_BASE_DELAY_MS`
- [ ] Review comments: const `CHUNK_RELOAD_MAX_DELAY_MS`
- [ ] Review comments: const `SHULKER_OP_MAX_RETRIES`
- [ ] Review comments: const `NAVIGATION_MAX_RETRIES`
- [ ] Review comments: const `RETRY_BASE_DELAY_MS`
- [ ] Review comments: const `RETRY_MAX_DELAY_MS`
- [ ] Review comments: const `FEE_MIN`
- [ ] Review comments: const `FEE_MAX`
- [ ] Review comments: const `MAX_TRANSACTION_QUANTITY`
- [ ] Review comments: const `MIN_RESERVE_FOR_PRICE`
- [ ] Review comments: const `CHESTS_PER_NODE`
- [ ] Review comments: const `NODE_SPACING`
- [ ] Review comments: const `OVERFLOW_CHEST_ITEM`
- [ ] Review comments: const `DIAMOND_CHEST_ID`
- [ ] Review comments: const `OVERFLOW_CHEST_ID`
- [ ] Review comments: const `MAX_ORDERS_PER_USER`
- [ ] Review comments: const `MAX_QUEUE_SIZE`
- [ ] Review comments: const `QUEUE_FILE`
- [ ] Review comments: const `RATE_LIMIT_BASE_COOLDOWN_MS`
- [ ] Review comments: const `UUID_CACHE_TTL_SECS`
- [ ] Review comments: const `RATE_LIMIT_MAX_COOLDOWN_MS`
- [ ] Review comments: const `RATE_LIMIT_RESET_AFTER_MS`
- [ ] Review comments: const `CLEANUP_INTERVAL_SECS`
- [ ] Review comments: const `RATE_LIMIT_STALE_AFTER_SECS`
- [ ] Review comments: fn `exponential_backoff_delay`

- [ ] Review testability: const `DOUBLE_CHEST_SLOTS`
- [ ] Review testability: const `SHULKER_BOX_SLOTS`
- [ ] Review testability: const `HOTBAR_SLOT_0`
- [ ] Review testability: const `TRADE_TIMEOUT_MS`
- [ ] Review testability: const `CHEST_OP_TIMEOUT_SECS`
- [ ] Review testability: const `PATHFINDING_TIMEOUT_MS`
- [ ] Review testability: const `DELAY_SHORT_MS`
- [ ] Review testability: const `PATHFINDING_CHECK_INTERVAL_MS`
- [ ] Review testability: const `DELAY_MEDIUM_MS`
- [ ] Review testability: const `DELAY_INTERACT_MS`
- [ ] Review testability: const `DELAY_BLOCK_OP_MS`
- [ ] Review testability: const `DELAY_LOOK_AT_MS`
- [ ] Review testability: const `DELAY_SETTLE_MS`
- [ ] Review testability: const `DELAY_VALIDATION_BETWEEN_CHESTS_MS`
- [ ] Review testability: const `DELAY_SHULKER_PLACE_MS`
- [ ] Review testability: const `DELAY_DISCONNECT_MS`
- [ ] Review testability: const `DELAY_CONFIG_DEBOUNCE_MS`
- [ ] Review testability: const `CHEST_OP_MAX_RETRIES`
- [ ] Review testability: const `CHUNK_RELOAD_EXTRA_RETRIES`
- [ ] Review testability: const `CHUNK_RELOAD_BASE_DELAY_MS`
- [ ] Review testability: const `CHUNK_RELOAD_MAX_DELAY_MS`
- [ ] Review testability: const `SHULKER_OP_MAX_RETRIES`
- [ ] Review testability: const `NAVIGATION_MAX_RETRIES`
- [ ] Review testability: const `RETRY_BASE_DELAY_MS`
- [ ] Review testability: const `RETRY_MAX_DELAY_MS`
- [ ] Review testability: const `FEE_MIN`
- [ ] Review testability: const `FEE_MAX`
- [ ] Review testability: const `MAX_TRANSACTION_QUANTITY`
- [ ] Review testability: const `MIN_RESERVE_FOR_PRICE`
- [ ] Review testability: const `CHESTS_PER_NODE`
- [ ] Review testability: const `NODE_SPACING`
- [ ] Review testability: const `OVERFLOW_CHEST_ITEM`
- [ ] Review testability: const `DIAMOND_CHEST_ID`
- [ ] Review testability: const `OVERFLOW_CHEST_ID`
- [ ] Review testability: const `MAX_ORDERS_PER_USER`
- [ ] Review testability: const `MAX_QUEUE_SIZE`
- [ ] Review testability: const `QUEUE_FILE`
- [ ] Review testability: const `RATE_LIMIT_BASE_COOLDOWN_MS`
- [ ] Review testability: const `UUID_CACHE_TTL_SECS`
- [ ] Review testability: const `RATE_LIMIT_MAX_COOLDOWN_MS`
- [ ] Review testability: const `RATE_LIMIT_RESET_AFTER_MS`
- [ ] Review testability: const `CLEANUP_INTERVAL_SECS`
- [ ] Review testability: const `RATE_LIMIT_STALE_AFTER_SECS`
- [ ] Review testability: fn `exponential_backoff_delay`

- [ ] Review logging: const `DOUBLE_CHEST_SLOTS`
- [ ] Review logging: const `SHULKER_BOX_SLOTS`
- [ ] Review logging: const `HOTBAR_SLOT_0`
- [ ] Review logging: const `TRADE_TIMEOUT_MS`
- [ ] Review logging: const `CHEST_OP_TIMEOUT_SECS`
- [ ] Review logging: const `PATHFINDING_TIMEOUT_MS`
- [ ] Review logging: const `DELAY_SHORT_MS`
- [ ] Review logging: const `PATHFINDING_CHECK_INTERVAL_MS`
- [ ] Review logging: const `DELAY_MEDIUM_MS`
- [ ] Review logging: const `DELAY_INTERACT_MS`
- [ ] Review logging: const `DELAY_BLOCK_OP_MS`
- [ ] Review logging: const `DELAY_LOOK_AT_MS`
- [ ] Review logging: const `DELAY_SETTLE_MS`
- [ ] Review logging: const `DELAY_VALIDATION_BETWEEN_CHESTS_MS`
- [ ] Review logging: const `DELAY_SHULKER_PLACE_MS`
- [ ] Review logging: const `DELAY_DISCONNECT_MS`
- [ ] Review logging: const `DELAY_CONFIG_DEBOUNCE_MS`
- [ ] Review logging: const `CHEST_OP_MAX_RETRIES`
- [ ] Review logging: const `CHUNK_RELOAD_EXTRA_RETRIES`
- [ ] Review logging: const `CHUNK_RELOAD_BASE_DELAY_MS`
- [ ] Review logging: const `CHUNK_RELOAD_MAX_DELAY_MS`
- [ ] Review logging: const `SHULKER_OP_MAX_RETRIES`
- [ ] Review logging: const `NAVIGATION_MAX_RETRIES`
- [ ] Review logging: const `RETRY_BASE_DELAY_MS`
- [ ] Review logging: const `RETRY_MAX_DELAY_MS`
- [ ] Review logging: const `FEE_MIN`
- [ ] Review logging: const `FEE_MAX`
- [ ] Review logging: const `MAX_TRANSACTION_QUANTITY`
- [ ] Review logging: const `MIN_RESERVE_FOR_PRICE`
- [ ] Review logging: const `CHESTS_PER_NODE`
- [ ] Review logging: const `NODE_SPACING`
- [ ] Review logging: const `OVERFLOW_CHEST_ITEM`
- [ ] Review logging: const `DIAMOND_CHEST_ID`
- [ ] Review logging: const `OVERFLOW_CHEST_ID`
- [ ] Review logging: const `MAX_ORDERS_PER_USER`
- [ ] Review logging: const `MAX_QUEUE_SIZE`
- [ ] Review logging: const `QUEUE_FILE`
- [ ] Review logging: const `RATE_LIMIT_BASE_COOLDOWN_MS`
- [ ] Review logging: const `UUID_CACHE_TTL_SECS`
- [ ] Review logging: const `RATE_LIMIT_MAX_COOLDOWN_MS`
- [ ] Review logging: const `RATE_LIMIT_RESET_AFTER_MS`
- [ ] Review logging: const `CLEANUP_INTERVAL_SECS`
- [ ] Review logging: const `RATE_LIMIT_STALE_AFTER_SECS`
- [ ] Review logging: fn `exponential_backoff_delay`

### src/error.rs

- enum `StoreError`
- impl `From<StoreError> for String` :: fn `from`
- impl `From<String> for StoreError` :: fn `from`

**TODO:**

- [ ] Review comments: enum `StoreError`
- [ ] Review comments: impl `From<StoreError> for String` :: fn `from`
- [ ] Review comments: impl `From<String> for StoreError` :: fn `from`

- [ ] Review testability: enum `StoreError`
- [ ] Review testability: impl `From<StoreError> for String` :: fn `from`
- [ ] Review testability: impl `From<String> for StoreError` :: fn `from`

- [ ] Review logging: enum `StoreError`
- [ ] Review logging: impl `From<StoreError> for String` :: fn `from`
- [ ] Review logging: impl `From<String> for StoreError` :: fn `from`

### src/fsutil.rs

- fn `write_atomic`

**TODO:**

- [ ] Review comments: fn `write_atomic`

- [ ] Review testability: fn `write_atomic`

- [ ] Review logging: fn `write_atomic`

- Added parent-directory `fsync` after rename in `write_atomic` (Unix only via `#[cfg(unix)]`; silently ignored on other platforms).
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

**TODO:**

- [ ] Review comments: struct `TradeItem`
- [ ] Review comments: struct `ChestSyncReport`
- [ ] Review comments: enum `QueuedOrderType`
- [ ] Review comments: enum `ChestAction`
- [ ] Review comments: enum `StoreMessage`
- [ ] Review comments: enum `BotMessage`
- [ ] Review comments: enum `CliMessage`
- [ ] Review comments: enum `BotInstruction`

- [ ] Review testability: struct `TradeItem`
- [ ] Review testability: struct `ChestSyncReport`
- [ ] Review testability: enum `QueuedOrderType`
- [ ] Review testability: enum `ChestAction`
- [ ] Review testability: enum `StoreMessage`
- [ ] Review testability: enum `BotMessage`
- [ ] Review testability: enum `CliMessage`
- [ ] Review testability: enum `BotInstruction`

- [ ] Review logging: struct `TradeItem`
- [ ] Review logging: struct `ChestSyncReport`
- [ ] Review logging: enum `QueuedOrderType`
- [ ] Review logging: enum `ChestAction`
- [ ] Review logging: enum `StoreMessage`
- [ ] Review logging: enum `BotMessage`
- [ ] Review logging: enum `CliMessage`
- [ ] Review logging: enum `BotInstruction`

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

**TODO:**

- [ ] Review comments: pub mod `chest`
- [ ] Review comments: pub mod `item_id`
- [ ] Review comments: pub mod `node`
- [ ] Review comments: pub mod `order`
- [ ] Review comments: pub mod `pair`
- [ ] Review comments: pub mod `position`
- [ ] Review comments: pub mod `storage`
- [ ] Review comments: pub mod `trade`
- [ ] Review comments: pub mod `user`
- [ ] Review comments: re-export surface (`Chest`, `ItemId`, `Node`, `Order`, `Pair`, `Position`, `Storage`, `Trade`, `TradeType`, `User`)

- [ ] Review testability: pub mod `chest`
- [ ] Review testability: pub mod `item_id`
- [ ] Review testability: pub mod `node`
- [ ] Review testability: pub mod `order`
- [ ] Review testability: pub mod `pair`
- [ ] Review testability: pub mod `position`
- [ ] Review testability: pub mod `storage`
- [ ] Review testability: pub mod `trade`
- [ ] Review testability: pub mod `user`
- [ ] Review testability: re-export surface (`Chest`, `ItemId`, `Node`, `Order`, `Pair`, `Position`, `Storage`, `Trade`, `TradeType`, `User`)

- [ ] Review logging: pub mod `chest`
- [ ] Review logging: pub mod `item_id`
- [ ] Review logging: pub mod `node`
- [ ] Review logging: pub mod `order`
- [ ] Review logging: pub mod `pair`
- [ ] Review logging: pub mod `position`
- [ ] Review logging: pub mod `storage`
- [ ] Review logging: pub mod `trade`
- [ ] Review logging: pub mod `user`
- [ ] Review logging: re-export surface (`Chest`, `ItemId`, `Node`, `Order`, `Pair`, `Position`, `Storage`, `Trade`, `TradeType`, `User`)

---

## types/

### src/types/position.rs

- struct `Position`

**TODO:**

- [ ] Review comments: struct `Position`

- [ ] Review testability: struct `Position`

- [ ] Review logging: struct `Position`

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

**TODO:**

- [ ] Review comments: struct `ItemId`
- [ ] Review comments: impl ItemId :: const `EMPTY`
- [ ] Review comments: impl ItemId :: fn `new`
- [ ] Review comments: impl ItemId :: fn `from_normalized`
- [ ] Review comments: impl ItemId :: fn `as_str`
- [ ] Review comments: impl ItemId :: fn `with_minecraft_prefix`
- [ ] Review comments: impl ItemId :: fn `is_empty`
- [ ] Review comments: impl `Deref for ItemId` :: fn `deref`
- [ ] Review comments: impl `Borrow<str> for ItemId` :: fn `borrow`
- [ ] Review comments: impl `AsRef<str> for ItemId` :: fn `as_ref`
- [ ] Review comments: impl `Display for ItemId` :: fn `fmt`
- [ ] Review comments: impl `PartialEq<str> for ItemId` :: fn `eq`
- [ ] Review comments: impl `PartialEq<&str> for ItemId` :: fn `eq`
- [ ] Review comments: impl `PartialEq<String> for ItemId` :: fn `eq`
- [ ] Review comments: impl `From<ItemId> for String` :: fn `from`
- [ ] Review comments: impl `Default for ItemId` :: fn `default`
- [ ] Review comments: tests module

- [ ] Review testability: struct `ItemId`
- [ ] Review testability: impl ItemId :: const `EMPTY`
- [ ] Review testability: impl ItemId :: fn `new`
- [ ] Review testability: impl ItemId :: fn `from_normalized`
- [ ] Review testability: impl ItemId :: fn `as_str`
- [ ] Review testability: impl ItemId :: fn `with_minecraft_prefix`
- [ ] Review testability: impl ItemId :: fn `is_empty`
- [ ] Review testability: impl `Deref for ItemId` :: fn `deref`
- [ ] Review testability: impl `Borrow<str> for ItemId` :: fn `borrow`
- [ ] Review testability: impl `AsRef<str> for ItemId` :: fn `as_ref`
- [ ] Review testability: impl `Display for ItemId` :: fn `fmt`
- [ ] Review testability: impl `PartialEq<str> for ItemId` :: fn `eq`
- [ ] Review testability: impl `PartialEq<&str> for ItemId` :: fn `eq`
- [ ] Review testability: impl `PartialEq<String> for ItemId` :: fn `eq`
- [ ] Review testability: impl `From<ItemId> for String` :: fn `from`
- [ ] Review testability: impl `Default for ItemId` :: fn `default`
- [ ] Review testability: tests module

- [ ] Review logging: struct `ItemId`
- [ ] Review logging: impl ItemId :: const `EMPTY`
- [ ] Review logging: impl ItemId :: fn `new`
- [ ] Review logging: impl ItemId :: fn `from_normalized`
- [ ] Review logging: impl ItemId :: fn `as_str`
- [ ] Review logging: impl ItemId :: fn `with_minecraft_prefix`
- [ ] Review logging: impl ItemId :: fn `is_empty`
- [ ] Review logging: impl `Deref for ItemId` :: fn `deref`
- [ ] Review logging: impl `Borrow<str> for ItemId` :: fn `borrow`
- [ ] Review logging: impl `AsRef<str> for ItemId` :: fn `as_ref`
- [ ] Review logging: impl `Display for ItemId` :: fn `fmt`
- [ ] Review logging: impl `PartialEq<str> for ItemId` :: fn `eq`
- [ ] Review logging: impl `PartialEq<&str> for ItemId` :: fn `eq`
- [ ] Review logging: impl `PartialEq<String> for ItemId` :: fn `eq`
- [ ] Review logging: impl `From<ItemId> for String` :: fn `from`
- [ ] Review logging: impl `Default for ItemId` :: fn `default`
- [ ] Review logging: tests module

- Consolidated `ItemId` construction: all call sites now use `ItemId::new` instead of `from_normalized(normalize_item_id(...))` (changed `store/mod.rs`, `handlers/validation.rs`, `handlers/cli.rs`, `store/command.rs`, `store/state.rs`).
- Deleted `store::utils::normalize_item_id` and its test; `Bot::normalize_item_id` now inlines the strip logic directly.
- Strengthened non-empty invariant: added `debug_assert!(!s.is_empty())` inside `ItemId::from_normalized`; `ItemId::EMPTY` retained as the unassigned-chest-slot sentinel.

### src/types/node.rs

- struct `Node`
- impl Node :: fn `new`
- impl Node :: fn `load`
- impl Node :: fn `save`
- impl Node :: fn `calc_position`
- impl Node :: fn `calc_chest_position`
- tests module

**TODO:**

- [ ] Review comments: struct `Node`
- [ ] Review comments: impl Node :: fn `new`
- [ ] Review comments: impl Node :: fn `load`
- [ ] Review comments: impl Node :: fn `save`
- [ ] Review comments: impl Node :: fn `calc_position`
- [ ] Review comments: impl Node :: fn `calc_chest_position`
- [ ] Review comments: tests module

- [ ] Review testability: struct `Node`
- [ ] Review testability: impl Node :: fn `new`
- [ ] Review testability: impl Node :: fn `load`
- [ ] Review testability: impl Node :: fn `save`
- [ ] Review testability: impl Node :: fn `calc_position`
- [ ] Review testability: impl Node :: fn `calc_chest_position`
- [ ] Review testability: tests module

- [ ] Review logging: struct `Node`
- [ ] Review logging: impl Node :: fn `new`
- [ ] Review logging: impl Node :: fn `load`
- [ ] Review logging: impl Node :: fn `save`
- [ ] Review logging: impl Node :: fn `calc_position`
- [ ] Review logging: impl Node :: fn `calc_chest_position`
- [ ] Review logging: tests module

- Migrated `types/` layer (`node.rs`, `pair.rs`, `user.rs`) from `eprintln!` to `tracing::warn!` for non-fatal load/save warnings (`trade.rs` already used `tracing::warn!`).
- [ ] Add a direct test for `Node::load`'s re-enforcement of node 0's reserved chests (diamond at index 0, overflow at index 1) — currently exercised only indirectly.

### src/types/chest.rs

- struct `Chest`
- impl Chest :: fn `new`
- impl Chest :: fn `calc_position`

**TODO:**

- [ ] Review comments: struct `Chest`
- [ ] Review comments: impl Chest :: fn `new`
- [ ] Review comments: impl Chest :: fn `calc_position`

- [ ] Review testability: struct `Chest`
- [ ] Review testability: impl Chest :: fn `new`
- [ ] Review testability: impl Chest :: fn `calc_position`

- [ ] Review logging: struct `Chest`
- [ ] Review logging: impl Chest :: fn `new`
- [ ] Review logging: impl Chest :: fn `calc_position`

### src/types/trade.rs

- struct `Trade`
- enum `TradeType`
- impl Trade :: fn `new`
- impl Trade :: fn `save`
- impl Trade :: fn `load_all_with_limit`
- impl Trade :: fn `save_all`

**TODO:**

- [ ] Review comments: struct `Trade`
- [ ] Review comments: enum `TradeType`
- [ ] Review comments: impl Trade :: fn `new`
- [ ] Review comments: impl Trade :: fn `save`
- [ ] Review comments: impl Trade :: fn `load_all_with_limit`
- [ ] Review comments: impl Trade :: fn `save_all`

- [ ] Review testability: struct `Trade`
- [ ] Review testability: enum `TradeType`
- [ ] Review testability: impl Trade :: fn `new`
- [ ] Review testability: impl Trade :: fn `save`
- [ ] Review testability: impl Trade :: fn `load_all_with_limit`
- [ ] Review testability: impl Trade :: fn `save_all`

- [ ] Review logging: struct `Trade`
- [ ] Review logging: enum `TradeType`
- [ ] Review logging: impl Trade :: fn `new`
- [ ] Review logging: impl Trade :: fn `save`
- [ ] Review logging: impl Trade :: fn `load_all_with_limit`
- [ ] Review logging: impl Trade :: fn `save_all`

- Made `Trade::load_all_with_limit` scalable: sorts filenames lexicographically, takes the last `max_trades` entries, then deserializes only those files.
- Guarded `Trade::save_all` against empty-`Vec` input: returns `Err(InvalidInput)` immediately to prevent silently wiping `data/trades`.
- Fixed the misleading `Utc::now()` comment: documented the real collision bound (two trades at the exact same nanosecond) instead of the false "monotonic" claim.

### src/types/order.rs

- struct `Order`
- enum `OrderType`
- impl Order :: fn `save_all_with_limit`

**TODO:**

- [ ] Review comments: struct `Order`
- [ ] Review comments: enum `OrderType`
- [ ] Review comments: impl Order :: fn `save_all_with_limit`

- [ ] Review testability: struct `Order`
- [ ] Review testability: enum `OrderType`
- [ ] Review testability: impl Order :: fn `save_all_with_limit`

- [ ] Review logging: struct `Order`
- [ ] Review logging: enum `OrderType`
- [ ] Review logging: impl Order :: fn `save_all_with_limit`

### src/types/pair.rs

- struct `Pair`
- impl Pair :: fn `shulker_capacity_for_stack_size`
- impl Pair :: fn `sanitize_item_name_for_filename`
- impl Pair :: fn `get_pair_file_path`
- impl Pair :: fn `save`
- impl Pair :: fn `load_all`
- impl Pair :: fn `save_all`

**TODO:**

- [ ] Review comments: struct `Pair`
- [ ] Review comments: impl Pair :: fn `shulker_capacity_for_stack_size`
- [ ] Review comments: impl Pair :: fn `sanitize_item_name_for_filename`
- [ ] Review comments: impl Pair :: fn `get_pair_file_path`
- [ ] Review comments: impl Pair :: fn `save`
- [ ] Review comments: impl Pair :: fn `load_all`
- [ ] Review comments: impl Pair :: fn `save_all`

- [ ] Review testability: struct `Pair`
- [ ] Review testability: impl Pair :: fn `shulker_capacity_for_stack_size`
- [ ] Review testability: impl Pair :: fn `sanitize_item_name_for_filename`
- [ ] Review testability: impl Pair :: fn `get_pair_file_path`
- [ ] Review testability: impl Pair :: fn `save`
- [ ] Review testability: impl Pair :: fn `load_all`
- [ ] Review testability: impl Pair :: fn `save_all`

- [ ] Review logging: struct `Pair`
- [ ] Review logging: impl Pair :: fn `shulker_capacity_for_stack_size`
- [ ] Review logging: impl Pair :: fn `sanitize_item_name_for_filename`
- [ ] Review logging: impl Pair :: fn `get_pair_file_path`
- [ ] Review logging: impl Pair :: fn `save`
- [ ] Review logging: impl Pair :: fn `load_all`
- [ ] Review logging: impl Pair :: fn `save_all`

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

**TODO:**

- [ ] Review comments: static `HTTP_CLIENT`
- [ ] Review comments: struct `User`
- [ ] Review comments: struct `MojangResponse`
- [ ] Review comments: fn `get_http_client`
- [ ] Review comments: impl User :: async fn `get_uuid_async`
- [ ] Review comments: impl User :: fn `get_user_file_path`
- [ ] Review comments: impl User :: fn `save`
- [ ] Review comments: impl User :: fn `load_all`
- [ ] Review comments: impl User :: fn `save_all`

- [ ] Review testability: static `HTTP_CLIENT`
- [ ] Review testability: struct `User`
- [ ] Review testability: struct `MojangResponse`
- [ ] Review testability: fn `get_http_client`
- [ ] Review testability: impl User :: async fn `get_uuid_async`
- [ ] Review testability: impl User :: fn `get_user_file_path`
- [ ] Review testability: impl User :: fn `save`
- [ ] Review testability: impl User :: fn `load_all`
- [ ] Review testability: impl User :: fn `save_all`

- [ ] Review logging: static `HTTP_CLIENT`
- [ ] Review logging: struct `User`
- [ ] Review logging: struct `MojangResponse`
- [ ] Review logging: fn `get_http_client`
- [ ] Review logging: impl User :: async fn `get_uuid_async`
- [ ] Review logging: impl User :: fn `get_user_file_path`
- [ ] Review logging: impl User :: fn `save`
- [ ] Review logging: impl User :: fn `load_all`
- [ ] Review logging: impl User :: fn `save_all`

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

**TODO:**

- [ ] Review comments: struct `ChestTransfer`
- [ ] Review comments: struct `Storage`
- [ ] Review comments: impl Storage :: const `SLOTS_PER_CHEST`
- [ ] Review comments: impl Storage :: const `DEFAULT_SHULKER_CAPACITY`
- [ ] Review comments: impl Storage :: fn `save`
- [ ] Review comments: impl Storage :: fn `new`
- [ ] Review comments: impl Storage :: fn `load`
- [ ] Review comments: impl Storage :: fn `add_node`
- [ ] Review comments: impl Storage :: fn `total_item_amount`
- [ ] Review comments: impl Storage :: fn `get_chest_mut`
- [ ] Review comments: impl Storage :: fn `withdraw_item`
- [ ] Review comments: impl Storage :: fn `deposit_item`
- [ ] Review comments: impl Storage :: fn `simulate_withdraw_plan`
- [ ] Review comments: impl Storage :: fn `simulate_deposit_plan`
- [ ] Review comments: impl Storage :: fn `withdraw_plan`
- [ ] Review comments: impl Storage :: fn `deposit_plan`
- [ ] Review comments: impl Storage :: fn `normalize_amounts_len`
- [ ] Review comments: impl Storage :: fn `deposit_into_chest`
- [ ] Review comments: impl Storage :: fn `find_empty_chest_index`
- [ ] Review comments: impl Storage :: fn `get_overflow_chest`
- [ ] Review comments: impl Storage :: fn `get_overflow_chest_mut`
- [ ] Review comments: impl Storage :: fn `get_overflow_chest_position`
- [ ] Review comments: impl Storage :: const fn `overflow_chest_id`
- [ ] Review comments: tests module

- [ ] Review testability: struct `ChestTransfer`
- [ ] Review testability: struct `Storage`
- [ ] Review testability: impl Storage :: const `SLOTS_PER_CHEST`
- [ ] Review testability: impl Storage :: const `DEFAULT_SHULKER_CAPACITY`
- [ ] Review testability: impl Storage :: fn `save`
- [ ] Review testability: impl Storage :: fn `new`
- [ ] Review testability: impl Storage :: fn `load`
- [ ] Review testability: impl Storage :: fn `add_node`
- [ ] Review testability: impl Storage :: fn `total_item_amount`
- [ ] Review testability: impl Storage :: fn `get_chest_mut`
- [ ] Review testability: impl Storage :: fn `withdraw_item`
- [ ] Review testability: impl Storage :: fn `deposit_item`
- [ ] Review testability: impl Storage :: fn `simulate_withdraw_plan`
- [ ] Review testability: impl Storage :: fn `simulate_deposit_plan`
- [ ] Review testability: impl Storage :: fn `withdraw_plan`
- [ ] Review testability: impl Storage :: fn `deposit_plan`
- [ ] Review testability: impl Storage :: fn `normalize_amounts_len`
- [ ] Review testability: impl Storage :: fn `deposit_into_chest`
- [ ] Review testability: impl Storage :: fn `find_empty_chest_index`
- [ ] Review testability: impl Storage :: fn `get_overflow_chest`
- [ ] Review testability: impl Storage :: fn `get_overflow_chest_mut`
- [ ] Review testability: impl Storage :: fn `get_overflow_chest_position`
- [ ] Review testability: impl Storage :: const fn `overflow_chest_id`
- [ ] Review testability: tests module

- [ ] Review logging: struct `ChestTransfer`
- [ ] Review logging: struct `Storage`
- [ ] Review logging: impl Storage :: const `SLOTS_PER_CHEST`
- [ ] Review logging: impl Storage :: const `DEFAULT_SHULKER_CAPACITY`
- [ ] Review logging: impl Storage :: fn `save`
- [ ] Review logging: impl Storage :: fn `new`
- [ ] Review logging: impl Storage :: fn `load`
- [ ] Review logging: impl Storage :: fn `add_node`
- [ ] Review logging: impl Storage :: fn `total_item_amount`
- [ ] Review logging: impl Storage :: fn `get_chest_mut`
- [ ] Review logging: impl Storage :: fn `withdraw_item`
- [ ] Review logging: impl Storage :: fn `deposit_item`
- [ ] Review logging: impl Storage :: fn `simulate_withdraw_plan`
- [ ] Review logging: impl Storage :: fn `simulate_deposit_plan`
- [ ] Review logging: impl Storage :: fn `withdraw_plan`
- [ ] Review logging: impl Storage :: fn `deposit_plan`
- [ ] Review logging: impl Storage :: fn `normalize_amounts_len`
- [ ] Review logging: impl Storage :: fn `deposit_into_chest`
- [ ] Review logging: impl Storage :: fn `find_empty_chest_index`
- [ ] Review logging: impl Storage :: fn `get_overflow_chest`
- [ ] Review logging: impl Storage :: fn `get_overflow_chest_mut`
- [ ] Review logging: impl Storage :: fn `get_overflow_chest_position`
- [ ] Review logging: impl Storage :: const fn `overflow_chest_id`
- [ ] Review logging: tests module

- Kept `Storage::DEFAULT_SHULKER_CAPACITY`: has two callers in `store/state.rs`.
- Deleted `Storage::withdraw_item` and `Storage::deposit_item` (confirmed no callers).

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

**TODO:**

- [ ] Review comments: pub mod declarations (`connection`, `navigation`, `trade`, `chest_io`, `shulker`, `inventory`)
- [ ] Review comments: struct `BotState`
- [ ] Review comments: struct `Bot`
- [ ] Review comments: impl `Default for BotState` :: fn `default`
- [ ] Review comments: impl Bot :: async fn `new`
- [ ] Review comments: impl Bot :: async fn `send_chat_message`
- [ ] Review comments: impl Bot :: async fn `send_whisper`
- [ ] Review comments: impl Bot :: fn `normalize_item_id`
- [ ] Review comments: impl Bot :: fn `chat_subscribe`
- [ ] Review comments: async fn `bot_task`
- [ ] Review comments: async fn `validate_node_physically`
- [ ] Review comments: fn `handle_event_fn`
- [ ] Review comments: async fn `handle_event`
- [ ] Review comments: async fn `handle_chat_message`

- [ ] Review testability: pub mod declarations (`connection`, `navigation`, `trade`, `chest_io`, `shulker`, `inventory`)
- [ ] Review testability: struct `BotState`
- [ ] Review testability: struct `Bot`
- [ ] Review testability: impl `Default for BotState` :: fn `default`
- [ ] Review testability: impl Bot :: async fn `new`
- [ ] Review testability: impl Bot :: async fn `send_chat_message`
- [ ] Review testability: impl Bot :: async fn `send_whisper`
- [ ] Review testability: impl Bot :: fn `normalize_item_id`
- [ ] Review testability: impl Bot :: fn `chat_subscribe`
- [ ] Review testability: async fn `bot_task`
- [ ] Review testability: async fn `validate_node_physically`
- [ ] Review testability: fn `handle_event_fn`
- [ ] Review testability: async fn `handle_event`
- [ ] Review testability: async fn `handle_chat_message`

- [ ] Review logging: pub mod declarations (`connection`, `navigation`, `trade`, `chest_io`, `shulker`, `inventory`)
- [ ] Review logging: struct `BotState`
- [ ] Review logging: struct `Bot`
- [ ] Review logging: impl `Default for BotState` :: fn `default`
- [ ] Review logging: impl Bot :: async fn `new`
- [ ] Review logging: impl Bot :: async fn `send_chat_message`
- [ ] Review logging: impl Bot :: async fn `send_whisper`
- [ ] Review logging: impl Bot :: fn `normalize_item_id`
- [ ] Review logging: impl Bot :: fn `chat_subscribe`
- [ ] Review logging: async fn `bot_task`
- [ ] Review logging: async fn `validate_node_physically`
- [ ] Review logging: fn `handle_event_fn`
- [ ] Review logging: async fn `handle_event`
- [ ] Review logging: async fn `handle_chat_message`

- [ ] Promote the 20s post-reconnect init wait and ~2s shutdown buffer in `bot_task` to named crate constants if a second caller ever emerges. Not worth doing speculatively today.

### src/bot/connection.rs

- async fn `connect`
- async fn `disconnect`

**TODO:**

- [ ] Review comments: async fn `connect`
- [ ] Review comments: async fn `disconnect`

- [ ] Review testability: async fn `connect`
- [ ] Review testability: async fn `disconnect`

- [ ] Review logging: async fn `connect`
- [ ] Review logging: async fn `disconnect`

### src/bot/navigation.rs

- async fn `navigate_to_position_once`
- async fn `navigate_to_position`
- async fn `go_to_node`
- async fn `go_to_chest`

**TODO:**

- [ ] Review comments: async fn `navigate_to_position_once`
- [ ] Review comments: async fn `navigate_to_position`
- [ ] Review comments: async fn `go_to_node`
- [ ] Review comments: async fn `go_to_chest`

- [ ] Review testability: async fn `navigate_to_position_once`
- [ ] Review testability: async fn `navigate_to_position`
- [ ] Review testability: async fn `go_to_node`
- [ ] Review testability: async fn `go_to_chest`

- [ ] Review logging: async fn `navigate_to_position_once`
- [ ] Review logging: async fn `navigate_to_position`
- [ ] Review logging: async fn `go_to_node`
- [ ] Review logging: async fn `go_to_chest`

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

**TODO:**

- [ ] Review comments: async fn `ensure_inventory_empty`
- [ ] Review comments: async fn `move_hotbar_to_inventory`
- [ ] Review comments: async fn `quick_move_from_container`
- [ ] Review comments: fn `verify_holding_shulker`
- [ ] Review comments: fn `is_entity_ready`
- [ ] Review comments: async fn `wait_for_entity_ready`
- [ ] Review comments: fn `carried_item`
- [ ] Review comments: async fn `ensure_shulker_in_hotbar_slot_0`
- [ ] Review comments: async fn `recover_shulker_to_slot_0`

- [ ] Review testability: async fn `ensure_inventory_empty`
- [ ] Review testability: async fn `move_hotbar_to_inventory`
- [ ] Review testability: async fn `quick_move_from_container`
- [ ] Review testability: fn `verify_holding_shulker`
- [ ] Review testability: fn `is_entity_ready`
- [ ] Review testability: async fn `wait_for_entity_ready`
- [ ] Review testability: fn `carried_item`
- [ ] Review testability: async fn `ensure_shulker_in_hotbar_slot_0`
- [ ] Review testability: async fn `recover_shulker_to_slot_0`

- [ ] Review logging: async fn `ensure_inventory_empty`
- [ ] Review logging: async fn `move_hotbar_to_inventory`
- [ ] Review logging: async fn `quick_move_from_container`
- [ ] Review logging: fn `verify_holding_shulker`
- [ ] Review logging: fn `is_entity_ready`
- [ ] Review logging: async fn `wait_for_entity_ready`
- [ ] Review logging: fn `carried_item`
- [ ] Review logging: async fn `ensure_shulker_in_hotbar_slot_0`
- [ ] Review logging: async fn `recover_shulker_to_slot_0`

- Refactored `ensure_shulker_in_hotbar_slot_0`: extracted `ShulkerSource` enum and `place_shulker_in_hotbar_slot_0(source)` helper, collapsing three nested branches into a ~50-line driver.

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

**TODO:**

- [ ] Review comments: const `CHUNK_NOT_LOADED_PREFIX`
- [ ] Review comments: fn `find_shulker_in_inventory_view`
- [ ] Review comments: async fn `place_shulker_in_chest_slot_verified`
- [ ] Review comments: async fn `open_chest_container_once`
- [ ] Review comments: async fn `open_chest_container_for_validation`
- [ ] Review comments: async fn `open_chest_container`
- [ ] Review comments: async fn `transfer_items_with_shulker`
- [ ] Review comments: async fn `transfer_withdraw_from_shulker`
- [ ] Review comments: async fn `transfer_deposit_into_shulker`
- [ ] Review comments: async fn `prepare_for_chest_io`
- [ ] Review comments: async fn `automated_chest_io`
- [ ] Review comments: async fn `withdraw_shulkers`
- [ ] Review comments: async fn `deposit_shulkers`

- [ ] Review testability: const `CHUNK_NOT_LOADED_PREFIX`
- [ ] Review testability: fn `find_shulker_in_inventory_view`
- [ ] Review testability: async fn `place_shulker_in_chest_slot_verified`
- [ ] Review testability: async fn `open_chest_container_once`
- [ ] Review testability: async fn `open_chest_container_for_validation`
- [ ] Review testability: async fn `open_chest_container`
- [ ] Review testability: async fn `transfer_items_with_shulker`
- [ ] Review testability: async fn `transfer_withdraw_from_shulker`
- [ ] Review testability: async fn `transfer_deposit_into_shulker`
- [ ] Review testability: async fn `prepare_for_chest_io`
- [ ] Review testability: async fn `automated_chest_io`
- [ ] Review testability: async fn `withdraw_shulkers`
- [ ] Review testability: async fn `deposit_shulkers`

- [ ] Review logging: const `CHUNK_NOT_LOADED_PREFIX`
- [ ] Review logging: fn `find_shulker_in_inventory_view`
- [ ] Review logging: async fn `place_shulker_in_chest_slot_verified`
- [ ] Review logging: async fn `open_chest_container_once`
- [ ] Review logging: async fn `open_chest_container_for_validation`
- [ ] Review logging: async fn `open_chest_container`
- [ ] Review logging: async fn `transfer_items_with_shulker`
- [ ] Review logging: async fn `transfer_withdraw_from_shulker`
- [ ] Review logging: async fn `transfer_deposit_into_shulker`
- [ ] Review logging: async fn `prepare_for_chest_io`
- [ ] Review logging: async fn `automated_chest_io`
- [ ] Review logging: async fn `withdraw_shulkers`
- [ ] Review logging: async fn `deposit_shulkers`

- Extracted `place_shulker_on_station` and `finish_shulker_round_trip` helpers from `withdraw_shulkers` / `deposit_shulkers`, eliminating ~200 lines of duplicated skeleton.
- Changed `slot_counts` / `amounts` in `automated_chest_io` and `ChestSyncReport` from `Vec<i32>` to `[i32; DOUBLE_CHEST_SLOTS]` (`ChestSyncReport` has no Serde derives and is never persisted).

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

**TODO:**

- [ ] Review comments: const `SHULKER_BOX_IDS`
- [ ] Review comments: fn `shulker_station_position`
- [ ] Review comments: fn `is_shulker_box`
- [ ] Review comments: fn `validate_chest_slot_is_shulker` (cfg(test))
- [ ] Review comments: async fn `pickup_shulker_from_station`
- [ ] Review comments: async fn `open_shulker_at_station_once`
- [ ] Review comments: async fn `open_shulker_at_station`
- [ ] Review comments: test `test_is_shulker_box`
- [ ] Review comments: test `test_validate_chest_slot_is_shulker`
- [ ] Review comments: test `test_shulker_station_position`

- [ ] Review testability: const `SHULKER_BOX_IDS`
- [ ] Review testability: fn `shulker_station_position`
- [ ] Review testability: fn `is_shulker_box`
- [ ] Review testability: fn `validate_chest_slot_is_shulker` (cfg(test))
- [ ] Review testability: async fn `pickup_shulker_from_station`
- [ ] Review testability: async fn `open_shulker_at_station_once`
- [ ] Review testability: async fn `open_shulker_at_station`
- [ ] Review testability: test `test_is_shulker_box`
- [ ] Review testability: test `test_validate_chest_slot_is_shulker`
- [ ] Review testability: test `test_shulker_station_position`

- [ ] Review logging: const `SHULKER_BOX_IDS`
- [ ] Review logging: fn `shulker_station_position`
- [ ] Review logging: fn `is_shulker_box`
- [ ] Review logging: fn `validate_chest_slot_is_shulker` (cfg(test))
- [ ] Review logging: async fn `pickup_shulker_from_station`
- [ ] Review logging: async fn `open_shulker_at_station_once`
- [ ] Review logging: async fn `open_shulker_at_station`
- [ ] Review logging: test `test_is_shulker_box`
- [ ] Review logging: test `test_validate_chest_slot_is_shulker`
- [ ] Review logging: test `test_shulker_station_position`

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

**TODO:**

- [ ] Review comments: fn `trade_bot_offer_slots`
- [ ] Review comments: fn `trade_player_offer_slots`
- [ ] Review comments: fn `trade_player_status_slots`
- [ ] Review comments: fn `trade_accept_slots`
- [ ] Review comments: fn `trade_cancel_slots`
- [ ] Review comments: async fn `wait_for_trade_menu_or_failure`
- [ ] Review comments: async fn `place_items_from_inventory_into_trade`
- [ ] Review comments: fn `validate_player_items`
- [ ] Review comments: async fn `execute_trade_with_player`
- [ ] Review comments: test `test_trade_bot_offer_slots`
- [ ] Review comments: test `test_trade_player_offer_slots`
- [ ] Review comments: test `test_trade_player_status_slots`
- [ ] Review comments: test `test_trade_accept_slots`
- [ ] Review comments: test `test_trade_cancel_slots`
- [ ] Review comments: test `test_trade_slot_sets_disjoint`

- [ ] Review testability: fn `trade_bot_offer_slots`
- [ ] Review testability: fn `trade_player_offer_slots`
- [ ] Review testability: fn `trade_player_status_slots`
- [ ] Review testability: fn `trade_accept_slots`
- [ ] Review testability: fn `trade_cancel_slots`
- [ ] Review testability: async fn `wait_for_trade_menu_or_failure`
- [ ] Review testability: async fn `place_items_from_inventory_into_trade`
- [ ] Review testability: fn `validate_player_items`
- [ ] Review testability: async fn `execute_trade_with_player`
- [ ] Review testability: test `test_trade_bot_offer_slots`
- [ ] Review testability: test `test_trade_player_offer_slots`
- [ ] Review testability: test `test_trade_player_status_slots`
- [ ] Review testability: test `test_trade_accept_slots`
- [ ] Review testability: test `test_trade_cancel_slots`
- [ ] Review testability: test `test_trade_slot_sets_disjoint`

- [ ] Review logging: fn `trade_bot_offer_slots`
- [ ] Review logging: fn `trade_player_offer_slots`
- [ ] Review logging: fn `trade_player_status_slots`
- [ ] Review logging: fn `trade_accept_slots`
- [ ] Review logging: fn `trade_cancel_slots`
- [ ] Review logging: async fn `wait_for_trade_menu_or_failure`
- [ ] Review logging: async fn `place_items_from_inventory_into_trade`
- [ ] Review logging: fn `validate_player_items`
- [ ] Review logging: async fn `execute_trade_with_player`
- [ ] Review logging: test `test_trade_bot_offer_slots`
- [ ] Review logging: test `test_trade_player_offer_slots`
- [ ] Review logging: test `test_trade_player_status_slots`
- [ ] Review logging: test `test_trade_accept_slots`
- [ ] Review logging: test `test_trade_cancel_slots`
- [ ] Review logging: test `test_trade_slot_sets_disjoint`

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

**TODO:**

- [ ] Review comments: pub mod declarations (`command`, `handlers`, `journal`, `orders`, `pricing`, `queue`, `rate_limit`, `rollback`, `state`, `trade_state`, `utils`)
- [ ] Review comments: struct `Store`
- [ ] Review comments: impl Store :: async fn `new`
- [ ] Review comments: impl Store :: async fn `run`
- [ ] Review comments: impl Store :: async fn `process_next_order`
- [ ] Review comments: impl Store :: fn `reload_config`
- [ ] Review comments: impl Store :: fn `advance_trade`
- [ ] Review comments: impl Store :: async fn `handle_bot_message`
- [ ] Review comments: impl Store :: fn `expect_pair`
- [ ] Review comments: impl Store :: fn `expect_pair_mut`
- [ ] Review comments: impl Store :: fn `expect_user`
- [ ] Review comments: impl Store :: fn `expect_user_mut`
- [ ] Review comments: impl Store :: fn `apply_chest_sync`
- [ ] Review comments: impl Store :: fn `get_node_position`
- [ ] Review comments: impl Store :: fn `new_for_test`

- [ ] Review testability: pub mod declarations (`command`, `handlers`, `journal`, `orders`, `pricing`, `queue`, `rate_limit`, `rollback`, `state`, `trade_state`, `utils`)
- [ ] Review testability: struct `Store`
- [ ] Review testability: impl Store :: async fn `new`
- [ ] Review testability: impl Store :: async fn `run`
- [ ] Review testability: impl Store :: async fn `process_next_order`
- [ ] Review testability: impl Store :: fn `reload_config`
- [ ] Review testability: impl Store :: fn `advance_trade`
- [ ] Review testability: impl Store :: async fn `handle_bot_message`
- [ ] Review testability: impl Store :: fn `expect_pair`
- [ ] Review testability: impl Store :: fn `expect_pair_mut`
- [ ] Review testability: impl Store :: fn `expect_user`
- [ ] Review testability: impl Store :: fn `expect_user_mut`
- [ ] Review testability: impl Store :: fn `apply_chest_sync`
- [ ] Review testability: impl Store :: fn `get_node_position`
- [ ] Review testability: impl Store :: fn `new_for_test`

- [ ] Review logging: pub mod declarations (`command`, `handlers`, `journal`, `orders`, `pricing`, `queue`, `rate_limit`, `rollback`, `state`, `trade_state`, `utils`)
- [ ] Review logging: struct `Store`
- [ ] Review logging: impl Store :: async fn `new`
- [ ] Review logging: impl Store :: async fn `run`
- [ ] Review logging: impl Store :: async fn `process_next_order`
- [ ] Review logging: impl Store :: fn `reload_config`
- [ ] Review logging: impl Store :: fn `advance_trade`
- [ ] Review logging: impl Store :: async fn `handle_bot_message`
- [ ] Review logging: impl Store :: fn `expect_pair`
- [ ] Review logging: impl Store :: fn `expect_pair_mut`
- [ ] Review logging: impl Store :: fn `expect_user`
- [ ] Review logging: impl Store :: fn `expect_user_mut`
- [ ] Review logging: impl Store :: fn `apply_chest_sync`
- [ ] Review logging: impl Store :: fn `get_node_position`
- [ ] Review logging: impl Store :: fn `new_for_test`

### src/store/state.rs

- fn `apply_chest_sync`
- fn `save`
- fn `audit_state`
- fn `assert_invariants`

**TODO:**

- [ ] Review comments: fn `apply_chest_sync`
- [ ] Review comments: fn `save`
- [ ] Review comments: fn `audit_state`
- [ ] Review comments: fn `assert_invariants`

- [ ] Review testability: fn `apply_chest_sync`
- [ ] Review testability: fn `save`
- [ ] Review testability: fn `audit_state`
- [ ] Review testability: fn `assert_invariants`

- [ ] Review logging: fn `apply_chest_sync`
- [ ] Review logging: fn `save`
- [ ] Review logging: fn `audit_state`
- [ ] Review logging: fn `assert_invariants`

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

**TODO:**

- [ ] Review comments: enum `Command`
- [ ] Review comments: fn `parse_command`
- [ ] Review comments: fn `parse_item_quantity`
- [ ] Review comments: fn `parse_item_amount`
- [ ] Review comments: fn `parse_optional_amount`
- [ ] Review comments: fn `parse_price`
- [ ] Review comments: fn `parse_balance`
- [ ] Review comments: fn `parse_pay`
- [ ] Review comments: fn `parse_page`
- [ ] Review comments: fn `parse_cancel`
- [ ] Review comments: tests module

- [ ] Review testability: enum `Command`
- [ ] Review testability: fn `parse_command`
- [ ] Review testability: fn `parse_item_quantity`
- [ ] Review testability: fn `parse_item_amount`
- [ ] Review testability: fn `parse_optional_amount`
- [ ] Review testability: fn `parse_price`
- [ ] Review testability: fn `parse_balance`
- [ ] Review testability: fn `parse_pay`
- [ ] Review testability: fn `parse_page`
- [ ] Review testability: fn `parse_cancel`
- [ ] Review testability: tests module

- [ ] Review logging: enum `Command`
- [ ] Review logging: fn `parse_command`
- [ ] Review logging: fn `parse_item_quantity`
- [ ] Review logging: fn `parse_item_amount`
- [ ] Review logging: fn `parse_optional_amount`
- [ ] Review logging: fn `parse_price`
- [ ] Review logging: fn `parse_balance`
- [ ] Review logging: fn `parse_pay`
- [ ] Review logging: fn `parse_page`
- [ ] Review logging: fn `parse_cancel`
- [ ] Review logging: tests module

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

**TODO:**

- [ ] Review comments: const `JOURNAL_FILE`
- [ ] Review comments: static `NEXT_OPERATION_ID`
- [ ] Review comments: type alias `SharedJournal`
- [ ] Review comments: struct `JournalEntry`
- [ ] Review comments: struct `Journal`
- [ ] Review comments: enum `JournalOp`
- [ ] Review comments: enum `JournalState`
- [ ] Review comments: impl `Default for Journal` :: fn `default`
- [ ] Review comments: impl Journal :: fn `load`
- [ ] Review comments: impl Journal :: fn `load_from`
- [ ] Review comments: impl Journal :: fn `clear_leftover`
- [ ] Review comments: impl Journal :: fn `begin`
- [ ] Review comments: impl Journal :: fn `advance`
- [ ] Review comments: impl Journal :: fn `complete`
- [ ] Review comments: impl Journal :: fn `current`
- [ ] Review comments: impl Journal :: fn `persist`
- [ ] Review comments: tests module

- [ ] Review testability: const `JOURNAL_FILE`
- [ ] Review testability: static `NEXT_OPERATION_ID`
- [ ] Review testability: type alias `SharedJournal`
- [ ] Review testability: struct `JournalEntry`
- [ ] Review testability: struct `Journal`
- [ ] Review testability: enum `JournalOp`
- [ ] Review testability: enum `JournalState`
- [ ] Review testability: impl `Default for Journal` :: fn `default`
- [ ] Review testability: impl Journal :: fn `load`
- [ ] Review testability: impl Journal :: fn `load_from`
- [ ] Review testability: impl Journal :: fn `clear_leftover`
- [ ] Review testability: impl Journal :: fn `begin`
- [ ] Review testability: impl Journal :: fn `advance`
- [ ] Review testability: impl Journal :: fn `complete`
- [ ] Review testability: impl Journal :: fn `current`
- [ ] Review testability: impl Journal :: fn `persist`
- [ ] Review testability: tests module

- [ ] Review logging: const `JOURNAL_FILE`
- [ ] Review logging: static `NEXT_OPERATION_ID`
- [ ] Review logging: type alias `SharedJournal`
- [ ] Review logging: struct `JournalEntry`
- [ ] Review logging: struct `Journal`
- [ ] Review logging: enum `JournalOp`
- [ ] Review logging: enum `JournalState`
- [ ] Review logging: impl `Default for Journal` :: fn `default`
- [ ] Review logging: impl Journal :: fn `load`
- [ ] Review logging: impl Journal :: fn `load_from`
- [ ] Review logging: impl Journal :: fn `clear_leftover`
- [ ] Review logging: impl Journal :: fn `begin`
- [ ] Review logging: impl Journal :: fn `advance`
- [ ] Review logging: impl Journal :: fn `complete`
- [ ] Review logging: impl Journal :: fn `current`
- [ ] Review logging: impl Journal :: fn `persist`
- [ ] Review logging: tests module

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

**TODO:**

- [ ] Review comments: struct `BuyPlan`
- [ ] Review comments: struct `SellPlan`
- [ ] Review comments: enum `ChestDirection`
- [ ] Review comments: async fn `execute_chest_transfers`
- [ ] Review comments: async fn `perform_trade`
- [ ] Review comments: async fn `validate_and_plan_buy`
- [ ] Review comments: async fn `handle_buy_order`
- [ ] Review comments: async fn `validate_and_plan_sell`
- [ ] Review comments: async fn `handle_sell_order`
- [ ] Review comments: async fn `execute_queued_order`
- [ ] Review comments: tests module

- [ ] Review testability: struct `BuyPlan`
- [ ] Review testability: struct `SellPlan`
- [ ] Review testability: enum `ChestDirection`
- [ ] Review testability: async fn `execute_chest_transfers`
- [ ] Review testability: async fn `perform_trade`
- [ ] Review testability: async fn `validate_and_plan_buy`
- [ ] Review testability: async fn `handle_buy_order`
- [ ] Review testability: async fn `validate_and_plan_sell`
- [ ] Review testability: async fn `handle_sell_order`
- [ ] Review testability: async fn `execute_queued_order`
- [ ] Review testability: tests module

- [ ] Review logging: struct `BuyPlan`
- [ ] Review logging: struct `SellPlan`
- [ ] Review logging: enum `ChestDirection`
- [ ] Review logging: async fn `execute_chest_transfers`
- [ ] Review logging: async fn `perform_trade`
- [ ] Review logging: async fn `validate_and_plan_buy`
- [ ] Review logging: async fn `handle_buy_order`
- [ ] Review logging: async fn `validate_and_plan_sell`
- [ ] Review logging: async fn `handle_sell_order`
- [ ] Review logging: async fn `execute_queued_order`
- [ ] Review logging: tests module

- Renamed `player_offers` to `items_player_must_give` in `spawn_mock_bot` to clarify its direction-neutral meaning.

### src/store/pricing.rs

- fn `validate_fee`
- fn `reserves_sufficient`
- fn `calculate_buy_cost`
- fn `buy_cost_pure`
- fn `calculate_sell_payout`
- fn `sell_payout_pure`
- tests module (includes proptests)

**TODO:**

- [ ] Review comments: fn `validate_fee`
- [ ] Review comments: fn `reserves_sufficient`
- [ ] Review comments: fn `calculate_buy_cost`
- [ ] Review comments: fn `buy_cost_pure`
- [ ] Review comments: fn `calculate_sell_payout`
- [ ] Review comments: fn `sell_payout_pure`
- [ ] Review comments: tests module (includes proptests)

- [ ] Review testability: fn `validate_fee`
- [ ] Review testability: fn `reserves_sufficient`
- [ ] Review testability: fn `calculate_buy_cost`
- [ ] Review testability: fn `buy_cost_pure`
- [ ] Review testability: fn `calculate_sell_payout`
- [ ] Review testability: fn `sell_payout_pure`
- [ ] Review testability: tests module (includes proptests)

- [ ] Review logging: fn `validate_fee`
- [ ] Review logging: fn `reserves_sufficient`
- [ ] Review logging: fn `calculate_buy_cost`
- [ ] Review logging: fn `buy_cost_pure`
- [ ] Review logging: fn `calculate_sell_payout`
- [ ] Review logging: fn `sell_payout_pure`
- [ ] Review logging: tests module (includes proptests)

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

**TODO:**

- [ ] Review comments: struct `QueuedOrder`
- [ ] Review comments: struct `OrderQueue`
- [ ] Review comments: struct `QueuePersist`
- [ ] Review comments: impl QueuedOrder :: fn `new`
- [ ] Review comments: impl QueuedOrder :: fn `description`
- [ ] Review comments: impl `Default for OrderQueue` :: fn `default`
- [ ] Review comments: impl OrderQueue :: fn `new`
- [ ] Review comments: impl OrderQueue :: fn `load`
- [ ] Review comments: impl OrderQueue :: fn `save`
- [ ] Review comments: impl OrderQueue :: fn `add`
- [ ] Review comments: impl OrderQueue :: fn `pop`
- [ ] Review comments: impl OrderQueue :: fn `is_empty`
- [ ] Review comments: impl OrderQueue :: fn `len`
- [ ] Review comments: impl OrderQueue :: fn `get_position`
- [ ] Review comments: impl OrderQueue :: fn `get_user_position`
- [ ] Review comments: impl OrderQueue :: fn `user_order_count`
- [ ] Review comments: impl OrderQueue :: fn `get_user_orders`
- [ ] Review comments: impl OrderQueue :: fn `cancel`
- [ ] Review comments: impl OrderQueue :: fn `estimate_wait`
- [ ] Review comments: tests module

- [ ] Review testability: struct `QueuedOrder`
- [ ] Review testability: struct `OrderQueue`
- [ ] Review testability: struct `QueuePersist`
- [ ] Review testability: impl QueuedOrder :: fn `new`
- [ ] Review testability: impl QueuedOrder :: fn `description`
- [ ] Review testability: impl `Default for OrderQueue` :: fn `default`
- [ ] Review testability: impl OrderQueue :: fn `new`
- [ ] Review testability: impl OrderQueue :: fn `load`
- [ ] Review testability: impl OrderQueue :: fn `save`
- [ ] Review testability: impl OrderQueue :: fn `add`
- [ ] Review testability: impl OrderQueue :: fn `pop`
- [ ] Review testability: impl OrderQueue :: fn `is_empty`
- [ ] Review testability: impl OrderQueue :: fn `len`
- [ ] Review testability: impl OrderQueue :: fn `get_position`
- [ ] Review testability: impl OrderQueue :: fn `get_user_position`
- [ ] Review testability: impl OrderQueue :: fn `user_order_count`
- [ ] Review testability: impl OrderQueue :: fn `get_user_orders`
- [ ] Review testability: impl OrderQueue :: fn `cancel`
- [ ] Review testability: impl OrderQueue :: fn `estimate_wait`
- [ ] Review testability: tests module

- [ ] Review logging: struct `QueuedOrder`
- [ ] Review logging: struct `OrderQueue`
- [ ] Review logging: struct `QueuePersist`
- [ ] Review logging: impl QueuedOrder :: fn `new`
- [ ] Review logging: impl QueuedOrder :: fn `description`
- [ ] Review logging: impl `Default for OrderQueue` :: fn `default`
- [ ] Review logging: impl OrderQueue :: fn `new`
- [ ] Review logging: impl OrderQueue :: fn `load`
- [ ] Review logging: impl OrderQueue :: fn `save`
- [ ] Review logging: impl OrderQueue :: fn `add`
- [ ] Review logging: impl OrderQueue :: fn `pop`
- [ ] Review logging: impl OrderQueue :: fn `is_empty`
- [ ] Review logging: impl OrderQueue :: fn `len`
- [ ] Review logging: impl OrderQueue :: fn `get_position`
- [ ] Review logging: impl OrderQueue :: fn `get_user_position`
- [ ] Review logging: impl OrderQueue :: fn `user_order_count`
- [ ] Review logging: impl OrderQueue :: fn `get_user_orders`
- [ ] Review logging: impl OrderQueue :: fn `cancel`
- [ ] Review logging: impl OrderQueue :: fn `estimate_wait`
- [ ] Review logging: tests module

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

**TODO:**

- [ ] Review comments: struct `UserRateLimit`
- [ ] Review comments: struct `RateLimiter`
- [ ] Review comments: fn `calculate_cooldown`
- [ ] Review comments: impl UserRateLimit :: fn `new`
- [ ] Review comments: impl `Default for RateLimiter` :: fn `default`
- [ ] Review comments: impl RateLimiter :: fn `new`
- [ ] Review comments: impl RateLimiter :: fn `check`
- [ ] Review comments: impl RateLimiter :: fn `cleanup_stale`
- [ ] Review comments: tests module

- [ ] Review testability: struct `UserRateLimit`
- [ ] Review testability: struct `RateLimiter`
- [ ] Review testability: fn `calculate_cooldown`
- [ ] Review testability: impl UserRateLimit :: fn `new`
- [ ] Review testability: impl `Default for RateLimiter` :: fn `default`
- [ ] Review testability: impl RateLimiter :: fn `new`
- [ ] Review testability: impl RateLimiter :: fn `check`
- [ ] Review testability: impl RateLimiter :: fn `cleanup_stale`
- [ ] Review testability: tests module

- [ ] Review logging: struct `UserRateLimit`
- [ ] Review logging: struct `RateLimiter`
- [ ] Review logging: fn `calculate_cooldown`
- [ ] Review logging: impl UserRateLimit :: fn `new`
- [ ] Review logging: impl `Default for RateLimiter` :: fn `default`
- [ ] Review logging: impl RateLimiter :: fn `new`
- [ ] Review logging: impl RateLimiter :: fn `check`
- [ ] Review logging: impl RateLimiter :: fn `cleanup_stale`
- [ ] Review logging: tests module

### src/store/rollback.rs

- struct `RollbackResult`
- impl RollbackResult :: fn `has_failures`
- fn `chest_from_transfer`
- async fn `deposit_transfers`
- async fn `rollback_amount_to_storage`

**TODO:**

- [ ] Review comments: struct `RollbackResult`
- [ ] Review comments: impl RollbackResult :: fn `has_failures`
- [ ] Review comments: fn `chest_from_transfer`
- [ ] Review comments: async fn `deposit_transfers`
- [ ] Review comments: async fn `rollback_amount_to_storage`

- [ ] Review testability: struct `RollbackResult`
- [ ] Review testability: impl RollbackResult :: fn `has_failures`
- [ ] Review testability: fn `chest_from_transfer`
- [ ] Review testability: async fn `deposit_transfers`
- [ ] Review testability: async fn `rollback_amount_to_storage`

- [ ] Review logging: struct `RollbackResult`
- [ ] Review logging: impl RollbackResult :: fn `has_failures`
- [ ] Review logging: fn `chest_from_transfer`
- [ ] Review logging: async fn `deposit_transfers`
- [ ] Review logging: async fn `rollback_amount_to_storage`

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

**TODO:**

- [ ] Review comments: const `TRADE_STATE_FILE`
- [ ] Review comments: struct `TradeResult`
- [ ] Review comments: struct `CompletedTrade`
- [ ] Review comments: enum `TradeState`
- [ ] Review comments: impl TradeState :: fn `new`
- [ ] Review comments: impl TradeState :: fn `begin_withdrawal`
- [ ] Review comments: impl TradeState :: fn `begin_trading`
- [ ] Review comments: impl TradeState :: fn `begin_depositing`
- [ ] Review comments: impl TradeState :: fn `commit`
- [ ] Review comments: impl TradeState :: fn `rollback`
- [ ] Review comments: impl TradeState :: fn `phase`
- [ ] Review comments: impl TradeState :: fn `is_terminal`
- [ ] Review comments: impl TradeState :: fn `order`
- [ ] Review comments: impl `fmt::Display for TradeState` :: fn `fmt`
- [ ] Review comments: fn `persist`
- [ ] Review comments: fn `load_persisted`
- [ ] Review comments: fn `clear_persisted`
- [ ] Review comments: tests module

- [ ] Review testability: const `TRADE_STATE_FILE`
- [ ] Review testability: struct `TradeResult`
- [ ] Review testability: struct `CompletedTrade`
- [ ] Review testability: enum `TradeState`
- [ ] Review testability: impl TradeState :: fn `new`
- [ ] Review testability: impl TradeState :: fn `begin_withdrawal`
- [ ] Review testability: impl TradeState :: fn `begin_trading`
- [ ] Review testability: impl TradeState :: fn `begin_depositing`
- [ ] Review testability: impl TradeState :: fn `commit`
- [ ] Review testability: impl TradeState :: fn `rollback`
- [ ] Review testability: impl TradeState :: fn `phase`
- [ ] Review testability: impl TradeState :: fn `is_terminal`
- [ ] Review testability: impl TradeState :: fn `order`
- [ ] Review testability: impl `fmt::Display for TradeState` :: fn `fmt`
- [ ] Review testability: fn `persist`
- [ ] Review testability: fn `load_persisted`
- [ ] Review testability: fn `clear_persisted`
- [ ] Review testability: tests module

- [ ] Review logging: const `TRADE_STATE_FILE`
- [ ] Review logging: struct `TradeResult`
- [ ] Review logging: struct `CompletedTrade`
- [ ] Review logging: enum `TradeState`
- [ ] Review logging: impl TradeState :: fn `new`
- [ ] Review logging: impl TradeState :: fn `begin_withdrawal`
- [ ] Review logging: impl TradeState :: fn `begin_trading`
- [ ] Review logging: impl TradeState :: fn `begin_depositing`
- [ ] Review logging: impl TradeState :: fn `commit`
- [ ] Review logging: impl TradeState :: fn `rollback`
- [ ] Review logging: impl TradeState :: fn `phase`
- [ ] Review logging: impl TradeState :: fn `is_terminal`
- [ ] Review logging: impl TradeState :: fn `order`
- [ ] Review logging: impl `fmt::Display for TradeState` :: fn `fmt`
- [ ] Review logging: fn `persist`
- [ ] Review logging: fn `load_persisted`
- [ ] Review logging: fn `clear_persisted`
- [ ] Review logging: tests module

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

**TODO:**

- [ ] Review comments: static `UUID_CACHE`
- [ ] Review comments: type alias `UuidCache`
- [ ] Review comments: fn `uuid_cache`
- [ ] Review comments: fn `normalize_item_id`
- [ ] Review comments: async fn `resolve_user_uuid`
- [ ] Review comments: fn `clear_uuid_cache`
- [ ] Review comments: fn `cleanup_uuid_cache`
- [ ] Review comments: fn `ensure_user_exists`
- [ ] Review comments: fn `is_operator`
- [ ] Review comments: fn `get_node_position`
- [ ] Review comments: async fn `send_message_to_player`
- [ ] Review comments: fn `summarize_transfers`
- [ ] Review comments: fn `fmt_issues`
- [ ] Review comments: tests module

- [ ] Review testability: static `UUID_CACHE`
- [ ] Review testability: type alias `UuidCache`
- [ ] Review testability: fn `uuid_cache`
- [ ] Review testability: fn `normalize_item_id`
- [ ] Review testability: async fn `resolve_user_uuid`
- [ ] Review testability: fn `clear_uuid_cache`
- [ ] Review testability: fn `cleanup_uuid_cache`
- [ ] Review testability: fn `ensure_user_exists`
- [ ] Review testability: fn `is_operator`
- [ ] Review testability: fn `get_node_position`
- [ ] Review testability: async fn `send_message_to_player`
- [ ] Review testability: fn `summarize_transfers`
- [ ] Review testability: fn `fmt_issues`
- [ ] Review testability: tests module

- [ ] Review logging: static `UUID_CACHE`
- [ ] Review logging: type alias `UuidCache`
- [ ] Review logging: fn `uuid_cache`
- [ ] Review logging: fn `normalize_item_id`
- [ ] Review logging: async fn `resolve_user_uuid`
- [ ] Review logging: fn `clear_uuid_cache`
- [ ] Review logging: fn `cleanup_uuid_cache`
- [ ] Review logging: fn `ensure_user_exists`
- [ ] Review logging: fn `is_operator`
- [ ] Review logging: fn `get_node_position`
- [ ] Review logging: async fn `send_message_to_player`
- [ ] Review logging: fn `summarize_transfers`
- [ ] Review logging: fn `fmt_issues`
- [ ] Review logging: tests module

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

**TODO:**

- [ ] Review comments: pub mod `player`
- [ ] Review comments: pub mod `operator`
- [ ] Review comments: pub mod `cli`
- [ ] Review comments: mod `buy`
- [ ] Review comments: mod `sell`
- [ ] Review comments: mod `deposit`
- [ ] Review comments: mod `withdraw`
- [ ] Review comments: mod `info`
- [ ] Review comments: pub(crate) mod `validation`

- [ ] Review testability: pub mod `player`
- [ ] Review testability: pub mod `operator`
- [ ] Review testability: pub mod `cli`
- [ ] Review testability: mod `buy`
- [ ] Review testability: mod `sell`
- [ ] Review testability: mod `deposit`
- [ ] Review testability: mod `withdraw`
- [ ] Review testability: mod `info`
- [ ] Review testability: pub(crate) mod `validation`

- [ ] Review logging: pub mod `player`
- [ ] Review logging: pub mod `operator`
- [ ] Review logging: pub mod `cli`
- [ ] Review logging: mod `buy`
- [ ] Review logging: mod `sell`
- [ ] Review logging: mod `deposit`
- [ ] Review logging: mod `withdraw`
- [ ] Review logging: mod `info`
- [ ] Review logging: pub(crate) mod `validation`

### src/store/handlers/validation.rs

- fn `validate_item_name`
- fn `validate_quantity`
- fn `validate_username`

**TODO:**

- [ ] Review comments: fn `validate_item_name`
- [ ] Review comments: fn `validate_quantity`
- [ ] Review comments: fn `validate_username`

- [ ] Review testability: fn `validate_item_name`
- [ ] Review testability: fn `validate_quantity`
- [ ] Review testability: fn `validate_username`

- [ ] Review logging: fn `validate_item_name`
- [ ] Review logging: fn `validate_quantity`
- [ ] Review logging: fn `validate_username`

### src/store/handlers/buy.rs

- async fn `handle`

**TODO:**

- [ ] Review comments: async fn `handle`

- [ ] Review testability: async fn `handle`

- [ ] Review logging: async fn `handle`

### src/store/handlers/sell.rs

- async fn `handle`

**TODO:**

- [ ] Review comments: async fn `handle`

- [ ] Review testability: async fn `handle`

- [ ] Review logging: async fn `handle`

### src/store/handlers/withdraw.rs

- async fn `handle_enqueue`
- async fn `handle_withdraw_balance_queued`

**TODO:**

- [ ] Review comments: async fn `handle_enqueue`
- [ ] Review comments: async fn `handle_withdraw_balance_queued`

- [ ] Review testability: async fn `handle_enqueue`
- [ ] Review testability: async fn `handle_withdraw_balance_queued`

- [ ] Review logging: async fn `handle_enqueue`
- [ ] Review logging: async fn `handle_withdraw_balance_queued`

### src/store/handlers/deposit.rs

- async fn `handle_enqueue`
- async fn `handle_deposit_balance_queued`

**TODO:**

- [ ] Review comments: async fn `handle_enqueue`
- [ ] Review comments: async fn `handle_deposit_balance_queued`

- [ ] Review testability: async fn `handle_enqueue`
- [ ] Review testability: async fn `handle_deposit_balance_queued`

- [ ] Review logging: async fn `handle_enqueue`
- [ ] Review logging: async fn `handle_deposit_balance_queued`

### src/store/handlers/player.rs

- async fn `handle_player_command`

**TODO:**

- [ ] Review comments: async fn `handle_player_command`

- [ ] Review testability: async fn `handle_player_command`

- [ ] Review logging: async fn `handle_player_command`

### src/store/handlers/operator.rs

- async fn `handle_additem_order`
- async fn `handle_removeitem_order`
- async fn `handle_add_currency`
- async fn `handle_remove_currency`

**TODO:**

- [ ] Review comments: async fn `handle_additem_order`
- [ ] Review comments: async fn `handle_removeitem_order`
- [ ] Review comments: async fn `handle_add_currency`
- [ ] Review comments: async fn `handle_remove_currency`

- [ ] Review testability: async fn `handle_additem_order`
- [ ] Review testability: async fn `handle_removeitem_order`
- [ ] Review testability: async fn `handle_add_currency`
- [ ] Review testability: async fn `handle_remove_currency`

- [ ] Review logging: async fn `handle_additem_order`
- [ ] Review logging: async fn `handle_removeitem_order`
- [ ] Review logging: async fn `handle_add_currency`
- [ ] Review logging: async fn `handle_remove_currency`

- Added `tracing::error!` with full context (item, player, quantities) when rollback-during-rollback fails in `handle_removeitem_order`; operator also receives a player-facing CRITICAL ERROR message.
- Promoted all 4 negative-stock `debug_assert!` calls to `assert!` with descriptive panic messages that fire in release builds.

### src/store/handlers/cli.rs

- async fn `handle_cli_message`

**TODO:**

- [ ] Review comments: async fn `handle_cli_message`

- [ ] Review testability: async fn `handle_cli_message`

- [ ] Review logging: async fn `handle_cli_message`

- Added `BASE_CURRENCY_ITEM: &str = "diamond"` constant in `constants.rs`; replaced all 4 hardcoded `"diamond"` string checks in `cli.rs` with it.

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

**TODO:**

- [ ] Review comments: async fn `handle_price`
- [ ] Review comments: async fn `handle_balance`
- [ ] Review comments: async fn `handle_pay`
- [ ] Review comments: async fn `handle_items`
- [ ] Review comments: async fn `handle_queue`
- [ ] Review comments: async fn `handle_cancel`
- [ ] Review comments: async fn `handle_status`
- [ ] Review comments: async fn `handle_help`
- [ ] Review comments: async fn `handle_price_command`
- [ ] Review comments: async fn `handle_status_command`
- [ ] Review comments: async fn `handle_items_command`
- [ ] Review comments: async fn `handle_help_command`
- [ ] Review comments: async fn `get_user_balance_async`
- [ ] Review comments: async fn `pay_async`

- [ ] Review testability: async fn `handle_price`
- [ ] Review testability: async fn `handle_balance`
- [ ] Review testability: async fn `handle_pay`
- [ ] Review testability: async fn `handle_items`
- [ ] Review testability: async fn `handle_queue`
- [ ] Review testability: async fn `handle_cancel`
- [ ] Review testability: async fn `handle_status`
- [ ] Review testability: async fn `handle_help`
- [ ] Review testability: async fn `handle_price_command`
- [ ] Review testability: async fn `handle_status_command`
- [ ] Review testability: async fn `handle_items_command`
- [ ] Review testability: async fn `handle_help_command`
- [ ] Review testability: async fn `get_user_balance_async`
- [ ] Review testability: async fn `pay_async`

- [ ] Review logging: async fn `handle_price`
- [ ] Review logging: async fn `handle_balance`
- [ ] Review logging: async fn `handle_pay`
- [ ] Review logging: async fn `handle_items`
- [ ] Review logging: async fn `handle_queue`
- [ ] Review logging: async fn `handle_cancel`
- [ ] Review logging: async fn `handle_status`
- [ ] Review logging: async fn `handle_help`
- [ ] Review logging: async fn `handle_price_command`
- [ ] Review logging: async fn `handle_status_command`
- [ ] Review logging: async fn `handle_items_command`
- [ ] Review logging: async fn `handle_help_command`
- [ ] Review logging: async fn `get_user_balance_async`
- [ ] Review logging: async fn `pay_async`

- [ ] Evaluate migrating user balances from `f64` to an integer representation (millidiamonds or similar) to eliminate accumulated rounding error on long histories.
