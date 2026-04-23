### src/main.rs

- fn `main`
- fn `print_usage`
- fn `run_validate_only`
- fn `spawn_config_watcher`

**TODO:**

- [x] Do comments: fn `main`
- [x] Do comments: fn `print_usage`
- [x] Do comments: fn `run_validate_only`
- [x] Do comments: fn `spawn_config_watcher`

- [x] Do testing: fn `main`
- [x] Do testing: fn `print_usage`
- [x] Do testing: fn `run_validate_only`
- [x] Do testing: fn `spawn_config_watcher`

- [x] Do logging: fn `main`
- [x] Do logging: fn `print_usage`
- [x] Do logging: fn `run_validate_only`
- [x] Do logging: fn `spawn_config_watcher`

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

- [x] Do comments: fn `with_retry`
- [x] Do comments: fn `cli_task`
- [x] Do comments: fn `get_balances`
- [x] Do comments: fn `get_pairs`
- [x] Do comments: fn `set_operator`
- [x] Do comments: fn `add_node`
- [x] Do comments: fn `add_node_with_validation`
- [x] Do comments: fn `discover_storage`
- [x] Do comments: fn `remove_node`
- [x] Do comments: fn `add_pair`
- [x] Do comments: fn `remove_pair`
- [x] Do comments: fn `view_storage`
- [x] Do comments: fn `view_trades`
- [x] Do comments: fn `restart_bot`
- [x] Do comments: fn `clear_stuck_order`
- [x] Do comments: fn `audit_state`

- [x] Do testing: fn `with_retry`
- [x] Do testing: fn `cli_task`
- [x] Do testing: fn `get_balances`
- [x] Do testing: fn `get_pairs`
- [x] Do testing: fn `set_operator`
- [x] Do testing: fn `add_node`
- [x] Do testing: fn `add_node_with_validation`
- [x] Do testing: fn `discover_storage`
- [x] Do testing: fn `remove_node`
- [x] Do testing: fn `add_pair`
- [x] Do testing: fn `remove_pair`
- [x] Do testing: fn `view_storage`
- [x] Do testing: fn `view_trades`
- [x] Do testing: fn `restart_bot`
- [x] Do testing: fn `clear_stuck_order`
- [x] Do testing: fn `audit_state`

- [x] Do logging: fn `with_retry`
- [x] Do logging: fn `cli_task`
- [x] Do logging: fn `get_balances`
- [x] Do logging: fn `get_pairs`
- [x] Do logging: fn `set_operator`
- [x] Do logging: fn `add_node`
- [x] Do logging: fn `add_node_with_validation`
- [x] Do logging: fn `discover_storage`
- [x] Do logging: fn `remove_node`
- [x] Do logging: fn `add_pair`
- [x] Do logging: fn `remove_pair`
- [x] Do logging: fn `view_storage`
- [x] Do logging: fn `view_trades`
- [x] Do logging: fn `restart_bot`
- [x] Do logging: fn `clear_stuck_order`
- [x] Do logging: fn `audit_state`

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

- [x] Do comments: struct `Config`
- [x] Do comments: fn `default_trade_timeout_ms`
- [x] Do comments: fn `default_pathfinding_timeout_ms`
- [x] Do comments: fn `default_max_orders`
- [x] Do comments: fn `default_max_trades_in_memory`
- [x] Do comments: fn `default_autosave_interval_secs`
- [x] Do comments: impl Config :: fn `validate`
- [x] Do comments: impl Config :: fn `load`

- [x] Do testing: struct `Config`
- [x] Do testing: fn `default_trade_timeout_ms`
- [x] Do testing: fn `default_pathfinding_timeout_ms`
- [x] Do testing: fn `default_max_orders`
- [x] Do testing: fn `default_max_trades_in_memory`
- [x] Do testing: fn `default_autosave_interval_secs`
- [x] Do testing: impl Config :: fn `validate`
- [x] Do testing: impl Config :: fn `load`

- [x] Do logging: struct `Config`
- [x] Do logging: fn `default_trade_timeout_ms`
- [x] Do logging: fn `default_pathfinding_timeout_ms`
- [x] Do logging: fn `default_max_orders`
- [x] Do logging: fn `default_max_trades_in_memory`
- [x] Do logging: fn `default_autosave_interval_secs`
- [x] Do logging: impl Config :: fn `validate`
- [x] Do logging: impl Config :: fn `load`

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

- [x] Do comments: const `DOUBLE_CHEST_SLOTS`
- [x] Do comments: const `SHULKER_BOX_SLOTS`
- [x] Do comments: const `HOTBAR_SLOT_0`
- [x] Do comments: const `TRADE_TIMEOUT_MS`
- [x] Do comments: const `CHEST_OP_TIMEOUT_SECS`
- [x] Do comments: const `PATHFINDING_TIMEOUT_MS`
- [x] Do comments: const `DELAY_SHORT_MS`
- [x] Do comments: const `PATHFINDING_CHECK_INTERVAL_MS`
- [x] Do comments: const `DELAY_MEDIUM_MS`
- [x] Do comments: const `DELAY_INTERACT_MS`
- [x] Do comments: const `DELAY_BLOCK_OP_MS`
- [x] Do comments: const `DELAY_LOOK_AT_MS`
- [x] Do comments: const `DELAY_SETTLE_MS`
- [x] Do comments: const `DELAY_VALIDATION_BETWEEN_CHESTS_MS`
- [x] Do comments: const `DELAY_SHULKER_PLACE_MS`
- [x] Do comments: const `DELAY_DISCONNECT_MS`
- [x] Do comments: const `DELAY_CONFIG_DEBOUNCE_MS`
- [x] Do comments: const `CHEST_OP_MAX_RETRIES`
- [x] Do comments: const `CHUNK_RELOAD_EXTRA_RETRIES`
- [x] Do comments: const `CHUNK_RELOAD_BASE_DELAY_MS`
- [x] Do comments: const `CHUNK_RELOAD_MAX_DELAY_MS`
- [x] Do comments: const `SHULKER_OP_MAX_RETRIES`
- [x] Do comments: const `NAVIGATION_MAX_RETRIES`
- [x] Do comments: const `RETRY_BASE_DELAY_MS`
- [x] Do comments: const `RETRY_MAX_DELAY_MS`
- [x] Do comments: const `FEE_MIN`
- [x] Do comments: const `FEE_MAX`
- [x] Do comments: const `MAX_TRANSACTION_QUANTITY`
- [x] Do comments: const `MIN_RESERVE_FOR_PRICE`
- [x] Do comments: const `CHESTS_PER_NODE`
- [x] Do comments: const `NODE_SPACING`
- [x] Do comments: const `OVERFLOW_CHEST_ITEM`
- [x] Do comments: const `DIAMOND_CHEST_ID`
- [x] Do comments: const `OVERFLOW_CHEST_ID`
- [x] Do comments: const `MAX_ORDERS_PER_USER`
- [x] Do comments: const `MAX_QUEUE_SIZE`
- [x] Do comments: const `QUEUE_FILE`
- [x] Do comments: const `RATE_LIMIT_BASE_COOLDOWN_MS`
- [x] Do comments: const `UUID_CACHE_TTL_SECS`
- [x] Do comments: const `RATE_LIMIT_MAX_COOLDOWN_MS`
- [x] Do comments: const `RATE_LIMIT_RESET_AFTER_MS`
- [x] Do comments: const `CLEANUP_INTERVAL_SECS`
- [x] Do comments: const `RATE_LIMIT_STALE_AFTER_SECS`
- [x] Do comments: fn `exponential_backoff_delay`

- [x] Do testing: const `DOUBLE_CHEST_SLOTS`
- [x] Do testing: const `SHULKER_BOX_SLOTS`
- [x] Do testing: const `HOTBAR_SLOT_0`
- [x] Do testing: const `TRADE_TIMEOUT_MS`
- [x] Do testing: const `CHEST_OP_TIMEOUT_SECS`
- [x] Do testing: const `PATHFINDING_TIMEOUT_MS`
- [x] Do testing: const `DELAY_SHORT_MS`
- [x] Do testing: const `PATHFINDING_CHECK_INTERVAL_MS`
- [x] Do testing: const `DELAY_MEDIUM_MS`
- [x] Do testing: const `DELAY_INTERACT_MS`
- [x] Do testing: const `DELAY_BLOCK_OP_MS`
- [x] Do testing: const `DELAY_LOOK_AT_MS`
- [x] Do testing: const `DELAY_SETTLE_MS`
- [x] Do testing: const `DELAY_VALIDATION_BETWEEN_CHESTS_MS`
- [x] Do testing: const `DELAY_SHULKER_PLACE_MS`
- [x] Do testing: const `DELAY_DISCONNECT_MS`
- [x] Do testing: const `DELAY_CONFIG_DEBOUNCE_MS`
- [x] Do testing: const `CHEST_OP_MAX_RETRIES`
- [x] Do testing: const `CHUNK_RELOAD_EXTRA_RETRIES`
- [x] Do testing: const `CHUNK_RELOAD_BASE_DELAY_MS`
- [x] Do testing: const `CHUNK_RELOAD_MAX_DELAY_MS`
- [x] Do testing: const `SHULKER_OP_MAX_RETRIES`
- [x] Do testing: const `NAVIGATION_MAX_RETRIES`
- [x] Do testing: const `RETRY_BASE_DELAY_MS`
- [x] Do testing: const `RETRY_MAX_DELAY_MS`
- [x] Do testing: const `FEE_MIN`
- [x] Do testing: const `FEE_MAX`
- [x] Do testing: const `MAX_TRANSACTION_QUANTITY`
- [x] Do testing: const `MIN_RESERVE_FOR_PRICE`
- [x] Do testing: const `CHESTS_PER_NODE`
- [x] Do testing: const `NODE_SPACING`
- [x] Do testing: const `OVERFLOW_CHEST_ITEM`
- [x] Do testing: const `DIAMOND_CHEST_ID`
- [x] Do testing: const `OVERFLOW_CHEST_ID`
- [x] Do testing: const `MAX_ORDERS_PER_USER`
- [x] Do testing: const `MAX_QUEUE_SIZE`
- [x] Do testing: const `QUEUE_FILE`
- [x] Do testing: const `RATE_LIMIT_BASE_COOLDOWN_MS`
- [x] Do testing: const `UUID_CACHE_TTL_SECS`
- [x] Do testing: const `RATE_LIMIT_MAX_COOLDOWN_MS`
- [x] Do testing: const `RATE_LIMIT_RESET_AFTER_MS`
- [x] Do testing: const `CLEANUP_INTERVAL_SECS`
- [x] Do testing: const `RATE_LIMIT_STALE_AFTER_SECS`
- [x] Do testing: fn `exponential_backoff_delay`

- [x] Do logging: const `DOUBLE_CHEST_SLOTS`
- [x] Do logging: const `SHULKER_BOX_SLOTS`
- [x] Do logging: const `HOTBAR_SLOT_0`
- [x] Do logging: const `TRADE_TIMEOUT_MS`
- [x] Do logging: const `CHEST_OP_TIMEOUT_SECS`
- [x] Do logging: const `PATHFINDING_TIMEOUT_MS`
- [x] Do logging: const `DELAY_SHORT_MS`
- [x] Do logging: const `PATHFINDING_CHECK_INTERVAL_MS`
- [x] Do logging: const `DELAY_MEDIUM_MS`
- [x] Do logging: const `DELAY_INTERACT_MS`
- [x] Do logging: const `DELAY_BLOCK_OP_MS`
- [x] Do logging: const `DELAY_LOOK_AT_MS`
- [x] Do logging: const `DELAY_SETTLE_MS`
- [x] Do logging: const `DELAY_VALIDATION_BETWEEN_CHESTS_MS`
- [x] Do logging: const `DELAY_SHULKER_PLACE_MS`
- [x] Do logging: const `DELAY_DISCONNECT_MS`
- [x] Do logging: const `DELAY_CONFIG_DEBOUNCE_MS`
- [x] Do logging: const `CHEST_OP_MAX_RETRIES`
- [x] Do logging: const `CHUNK_RELOAD_EXTRA_RETRIES`
- [x] Do logging: const `CHUNK_RELOAD_BASE_DELAY_MS`
- [x] Do logging: const `CHUNK_RELOAD_MAX_DELAY_MS`
- [x] Do logging: const `SHULKER_OP_MAX_RETRIES`
- [x] Do logging: const `NAVIGATION_MAX_RETRIES`
- [x] Do logging: const `RETRY_BASE_DELAY_MS`
- [x] Do logging: const `RETRY_MAX_DELAY_MS`
- [x] Do logging: const `FEE_MIN`
- [x] Do logging: const `FEE_MAX`
- [x] Do logging: const `MAX_TRANSACTION_QUANTITY`
- [x] Do logging: const `MIN_RESERVE_FOR_PRICE`
- [x] Do logging: const `CHESTS_PER_NODE`
- [x] Do logging: const `NODE_SPACING`
- [x] Do logging: const `OVERFLOW_CHEST_ITEM`
- [x] Do logging: const `DIAMOND_CHEST_ID`
- [x] Do logging: const `OVERFLOW_CHEST_ID`
- [x] Do logging: const `MAX_ORDERS_PER_USER`
- [x] Do logging: const `MAX_QUEUE_SIZE`
- [x] Do logging: const `QUEUE_FILE`
- [x] Do logging: const `RATE_LIMIT_BASE_COOLDOWN_MS`
- [x] Do logging: const `UUID_CACHE_TTL_SECS`
- [x] Do logging: const `RATE_LIMIT_MAX_COOLDOWN_MS`
- [x] Do logging: const `RATE_LIMIT_RESET_AFTER_MS`
- [x] Do logging: const `CLEANUP_INTERVAL_SECS`
- [x] Do logging: const `RATE_LIMIT_STALE_AFTER_SECS`
- [x] Do logging: fn `exponential_backoff_delay`

### src/error.rs

- enum `StoreError`
- impl `From<StoreError> for String` :: fn `from`

**TODO:**

- [x] Do comments: enum `StoreError`
- [x] Do comments: impl `From<StoreError> for String` :: fn `from`

- [x] Do testing: enum `StoreError`
- [x] Do testing: impl `From<StoreError> for String` :: fn `from`

- [x] Do logging: enum `StoreError`
- [x] Do logging: impl `From<StoreError> for String` :: fn `from`

### src/fsutil.rs

- fn `write_atomic`

**TODO:**

- [x] Do comments: fn `write_atomic`

- [x] Do testing: fn `write_atomic`

- [x] Do logging: fn `write_atomic`

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

- [x] Do comments: struct `TradeItem`
- [x] Do comments: struct `ChestSyncReport`
- [x] Do comments: enum `QueuedOrderType`
- [x] Do comments: enum `ChestAction`
- [x] Do comments: enum `StoreMessage`
- [x] Do comments: enum `BotMessage`
- [x] Do comments: enum `CliMessage`
- [x] Do comments: enum `BotInstruction`

- [x] Do testing: struct `TradeItem`
- [x] Do testing: struct `ChestSyncReport`
- [x] Do testing: enum `QueuedOrderType`
- [x] Do testing: enum `ChestAction`
- [x] Do testing: enum `StoreMessage`
- [x] Do testing: enum `BotMessage`
- [x] Do testing: enum `CliMessage`
- [x] Do testing: enum `BotInstruction`

- [x] Do logging: struct `TradeItem`
- [x] Do logging: struct `ChestSyncReport`
- [x] Do logging: enum `QueuedOrderType`
- [x] Do logging: enum `ChestAction`
- [x] Do logging: enum `StoreMessage`
- [x] Do logging: enum `BotMessage`
- [x] Do logging: enum `CliMessage`
- [x] Do logging: enum `BotInstruction`

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

- [x] Do comments: pub mod `chest`
- [x] Do comments: pub mod `item_id`
- [x] Do comments: pub mod `node`
- [x] Do comments: pub mod `order`
- [x] Do comments: pub mod `pair`
- [x] Do comments: pub mod `position`
- [x] Do comments: pub mod `storage`
- [x] Do comments: pub mod `trade`
- [x] Do comments: pub mod `user`
- [x] Do comments: re-export surface (`Chest`, `ItemId`, `Node`, `Order`, `Pair`, `Position`, `Storage`, `Trade`, `TradeType`, `User`)

- [x] Do testing: pub mod `chest`
- [x] Do testing: pub mod `item_id`
- [x] Do testing: pub mod `node`
- [x] Do testing: pub mod `order`
- [x] Do testing: pub mod `pair`
- [x] Do testing: pub mod `position`
- [x] Do testing: pub mod `storage`
- [x] Do testing: pub mod `trade`
- [x] Do testing: pub mod `user`
- [x] Do testing: re-export surface (`Chest`, `ItemId`, `Node`, `Order`, `Pair`, `Position`, `Storage`, `Trade`, `TradeType`, `User`)

- [x] Do logging: pub mod `chest`
- [x] Do logging: pub mod `item_id`
- [x] Do logging: pub mod `node`
- [x] Do logging: pub mod `order`
- [x] Do logging: pub mod `pair`
- [x] Do logging: pub mod `position`
- [x] Do logging: pub mod `storage`
- [x] Do logging: pub mod `trade`
- [x] Do logging: pub mod `user`
- [x] Do logging: re-export surface (`Chest`, `ItemId`, `Node`, `Order`, `Pair`, `Position`, `Storage`, `Trade`, `TradeType`, `User`)

---

## types/

### src/types/position.rs

- struct `Position`

**TODO:**

- [x] Do comments: struct `Position`

- [x] Do testing: struct `Position`

- [x] Do logging: struct `Position`

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

- [x] Do comments: struct `ItemId`
- [x] Do comments: impl ItemId :: const `EMPTY`
- [x] Do comments: impl ItemId :: fn `new`
- [x] Do comments: impl ItemId :: fn `from_normalized`
- [x] Do comments: impl ItemId :: fn `as_str`
- [x] Do comments: impl ItemId :: fn `with_minecraft_prefix`
- [x] Do comments: impl ItemId :: fn `is_empty`
- [x] Do comments: impl `Deref for ItemId` :: fn `deref`
- [x] Do comments: impl `Borrow<str> for ItemId` :: fn `borrow`
- [x] Do comments: impl `AsRef<str> for ItemId` :: fn `as_ref`
- [x] Do comments: impl `Display for ItemId` :: fn `fmt`
- [x] Do comments: impl `PartialEq<str> for ItemId` :: fn `eq`
- [x] Do comments: impl `PartialEq<&str> for ItemId` :: fn `eq`
- [x] Do comments: impl `PartialEq<String> for ItemId` :: fn `eq`
- [x] Do comments: impl `From<ItemId> for String` :: fn `from`
- [x] Do comments: impl `Default for ItemId` :: fn `default`
- [x] Do comments: tests module

- [x] Do testing: struct `ItemId`
- [x] Do testing: impl ItemId :: const `EMPTY`
- [x] Do testing: impl ItemId :: fn `new`
- [x] Do testing: impl ItemId :: fn `from_normalized`
- [x] Do testing: impl ItemId :: fn `as_str`
- [x] Do testing: impl ItemId :: fn `with_minecraft_prefix`
- [x] Do testing: impl ItemId :: fn `is_empty`
- [x] Do testing: impl `Deref for ItemId` :: fn `deref`
- [x] Do testing: impl `Borrow<str> for ItemId` :: fn `borrow`
- [x] Do testing: impl `AsRef<str> for ItemId` :: fn `as_ref`
- [x] Do testing: impl `Display for ItemId` :: fn `fmt`
- [x] Do testing: impl `PartialEq<str> for ItemId` :: fn `eq`
- [x] Do testing: impl `PartialEq<&str> for ItemId` :: fn `eq`
- [x] Do testing: impl `PartialEq<String> for ItemId` :: fn `eq`
- [x] Do testing: impl `From<ItemId> for String` :: fn `from`
- [x] Do testing: impl `Default for ItemId` :: fn `default`
- [x] Do testing: tests module

- [x] Do logging: struct `ItemId`
- [x] Do logging: impl ItemId :: const `EMPTY`
- [x] Do logging: impl ItemId :: fn `new`
- [x] Do logging: impl ItemId :: fn `from_normalized`
- [x] Do logging: impl ItemId :: fn `as_str`
- [x] Do logging: impl ItemId :: fn `with_minecraft_prefix`
- [x] Do logging: impl ItemId :: fn `is_empty`
- [x] Do logging: impl `Deref for ItemId` :: fn `deref`
- [x] Do logging: impl `Borrow<str> for ItemId` :: fn `borrow`
- [x] Do logging: impl `AsRef<str> for ItemId` :: fn `as_ref`
- [x] Do logging: impl `Display for ItemId` :: fn `fmt`
- [x] Do logging: impl `PartialEq<str> for ItemId` :: fn `eq`
- [x] Do logging: impl `PartialEq<&str> for ItemId` :: fn `eq`
- [x] Do logging: impl `PartialEq<String> for ItemId` :: fn `eq`
- [x] Do logging: impl `From<ItemId> for String` :: fn `from`
- [x] Do logging: impl `Default for ItemId` :: fn `default`
- [x] Do logging: tests module

### src/types/node.rs

- struct `Node`
- impl Node :: fn `new`
- impl Node :: fn `load`
- impl Node :: fn `save`
- impl Node :: fn `calc_position`
- impl Node :: fn `calc_chest_position`
- tests module

**TODO:**

- [x] Do comments: struct `Node`
- [x] Do comments: impl Node :: fn `new`
- [x] Do comments: impl Node :: fn `load`
- [x] Do comments: impl Node :: fn `save`
- [x] Do comments: impl Node :: fn `calc_position`
- [x] Do comments: impl Node :: fn `calc_chest_position`
- [x] Do comments: tests module

- [x] Do testing: struct `Node`
- [x] Do testing: impl Node :: fn `new`
- [x] Do testing: impl Node :: fn `load`
- [x] Do testing: impl Node :: fn `save`
- [x] Do testing: impl Node :: fn `calc_position`
- [x] Do testing: impl Node :: fn `calc_chest_position`
- [x] Do testing: tests module

- [x] Do logging: struct `Node`
- [x] Do logging: impl Node :: fn `new`
- [x] Do logging: impl Node :: fn `load`
- [x] Do logging: impl Node :: fn `save`
- [x] Do logging: impl Node :: fn `calc_position`
- [x] Do logging: impl Node :: fn `calc_chest_position`
- [x] Do logging: tests module

### src/types/chest.rs

- struct `Chest`
- impl Chest :: fn `new`
- impl Chest :: fn `calc_position`

**TODO:**

- [x] Do comments: struct `Chest`
- [x] Do comments: impl Chest :: fn `new`
- [x] Do comments: impl Chest :: fn `calc_position`

- [x] Do testing: struct `Chest`
- [x] Do testing: impl Chest :: fn `new`
- [x] Do testing: impl Chest :: fn `calc_position`

- [x] Do logging: struct `Chest`
- [x] Do logging: impl Chest :: fn `new`
- [x] Do logging: impl Chest :: fn `calc_position`

### src/types/trade.rs

- struct `Trade`
- enum `TradeType`
- impl Trade :: fn `new`
- impl Trade :: fn `save`
- impl Trade :: fn `load_all_with_limit`
- impl Trade :: fn `save_all`

**TODO:**

- [x] Do comments: struct `Trade`
- [x] Do comments: enum `TradeType`
- [x] Do comments: impl Trade :: fn `new`
- [x] Do comments: impl Trade :: fn `save`
- [x] Do comments: impl Trade :: fn `load_all_with_limit`
- [x] Do comments: impl Trade :: fn `save_all`

- [x] Do testing: struct `Trade`
- [x] Do testing: enum `TradeType`
- [x] Do testing: impl Trade :: fn `new`
- [x] Do testing: impl Trade :: fn `save`
- [x] Do testing: impl Trade :: fn `load_all_with_limit`
- [x] Do testing: impl Trade :: fn `save_all`

- [x] Do logging: struct `Trade`
- [x] Do logging: enum `TradeType`
- [x] Do logging: impl Trade :: fn `new`
- [x] Do logging: impl Trade :: fn `save`
- [x] Do logging: impl Trade :: fn `load_all_with_limit`
- [x] Do logging: impl Trade :: fn `save_all`

### src/types/order.rs

- struct `Order`
- enum `OrderType`
- impl Order :: fn `save_all_with_limit`

**TODO:**

- [x] Do comments: struct `Order`
- [x] Do comments: enum `OrderType`
- [x] Do comments: impl Order :: fn `save_all_with_limit`

- [x] Do testing: struct `Order`
- [x] Do testing: enum `OrderType`
- [x] Do testing: impl Order :: fn `save_all_with_limit`

- [x] Do logging: struct `Order`
- [x] Do logging: enum `OrderType`
- [x] Do logging: impl Order :: fn `save_all_with_limit`

### src/types/pair.rs

- struct `Pair`
- impl Pair :: fn `shulker_capacity_for_stack_size`
- impl Pair :: fn `sanitize_item_name_for_filename`
- impl Pair :: fn `get_pair_file_path`
- impl Pair :: fn `save`
- impl Pair :: fn `load_all`
- impl Pair :: fn `save_all`

**TODO:**

- [x] Do comments: struct `Pair`
- [x] Do comments: impl Pair :: fn `shulker_capacity_for_stack_size`
- [x] Do comments: impl Pair :: fn `sanitize_item_name_for_filename`
- [x] Do comments: impl Pair :: fn `get_pair_file_path`
- [x] Do comments: impl Pair :: fn `save`
- [x] Do comments: impl Pair :: fn `load_all`
- [x] Do comments: impl Pair :: fn `save_all`

- [x] Do testing: struct `Pair`
- [x] Do testing: impl Pair :: fn `shulker_capacity_for_stack_size`
- [x] Do testing: impl Pair :: fn `sanitize_item_name_for_filename`
- [x] Do testing: impl Pair :: fn `get_pair_file_path`
- [x] Do testing: impl Pair :: fn `save`
- [x] Do testing: impl Pair :: fn `load_all`
- [x] Do testing: impl Pair :: fn `save_all`

- [x] Do logging: struct `Pair`
- [x] Do logging: impl Pair :: fn `shulker_capacity_for_stack_size`
- [x] Do logging: impl Pair :: fn `sanitize_item_name_for_filename`
- [x] Do logging: impl Pair :: fn `get_pair_file_path`
- [x] Do logging: impl Pair :: fn `save`
- [x] Do logging: impl Pair :: fn `load_all`
- [x] Do logging: impl Pair :: fn `save_all`

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

- [x] Do comments: static `HTTP_CLIENT`
- [x] Do comments: struct `User`
- [x] Do comments: struct `MojangResponse`
- [x] Do comments: fn `get_http_client`
- [x] Do comments: impl User :: async fn `get_uuid_async`
- [x] Do comments: impl User :: fn `get_user_file_path`
- [x] Do comments: impl User :: fn `save`
- [x] Do comments: impl User :: fn `load_all`
- [x] Do comments: impl User :: fn `save_all`

- [x] Do testing: static `HTTP_CLIENT`
- [x] Do testing: struct `User`
- [x] Do testing: struct `MojangResponse`
- [x] Do testing: fn `get_http_client`
- [x] Do testing: impl User :: async fn `get_uuid_async`
- [x] Do testing: impl User :: fn `get_user_file_path`
- [x] Do testing: impl User :: fn `save`
- [x] Do testing: impl User :: fn `load_all`
- [x] Do testing: impl User :: fn `save_all`

- [x] Do logging: static `HTTP_CLIENT`
- [x] Do logging: struct `User`
- [x] Do logging: struct `MojangResponse`
- [x] Do logging: fn `get_http_client`
- [x] Do logging: impl User :: async fn `get_uuid_async`
- [x] Do logging: impl User :: fn `get_user_file_path`
- [x] Do logging: impl User :: fn `save`
- [x] Do logging: impl User :: fn `load_all`
- [x] Do logging: impl User :: fn `save_all`

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

- [x] Do comments: struct `ChestTransfer`
- [x] Do comments: struct `Storage`
- [x] Do comments: impl Storage :: const `SLOTS_PER_CHEST`
- [x] Do comments: impl Storage :: const `DEFAULT_SHULKER_CAPACITY`
- [x] Do comments: impl Storage :: fn `save`
- [x] Do comments: impl Storage :: fn `new`
- [x] Do comments: impl Storage :: fn `load`
- [x] Do comments: impl Storage :: fn `add_node`
- [x] Do comments: impl Storage :: fn `total_item_amount`
- [x] Do comments: impl Storage :: fn `get_chest_mut`
- [x] Do comments: impl Storage :: fn `withdraw_item`
- [x] Do comments: impl Storage :: fn `deposit_item`
- [x] Do comments: impl Storage :: fn `simulate_withdraw_plan`
- [x] Do comments: impl Storage :: fn `simulate_deposit_plan`
- [x] Do comments: impl Storage :: fn `withdraw_plan`
- [x] Do comments: impl Storage :: fn `deposit_plan`
- [x] Do comments: impl Storage :: fn `normalize_amounts_len`
- [x] Do comments: impl Storage :: fn `deposit_into_chest`
- [x] Do comments: impl Storage :: fn `find_empty_chest_index`
- [x] Do comments: impl Storage :: fn `get_overflow_chest`
- [x] Do comments: impl Storage :: fn `get_overflow_chest_mut`
- [x] Do comments: impl Storage :: fn `get_overflow_chest_position`
- [x] Do comments: impl Storage :: const fn `overflow_chest_id`
- [x] Do comments: tests module

- [x] Do testing: struct `ChestTransfer`
- [x] Do testing: struct `Storage`
- [x] Do testing: impl Storage :: const `SLOTS_PER_CHEST`
- [x] Do testing: impl Storage :: const `DEFAULT_SHULKER_CAPACITY`
- [x] Do testing: impl Storage :: fn `save`
- [x] Do testing: impl Storage :: fn `new`
- [x] Do testing: impl Storage :: fn `load`
- [x] Do testing: impl Storage :: fn `add_node`
- [x] Do testing: impl Storage :: fn `total_item_amount`
- [x] Do testing: impl Storage :: fn `get_chest_mut`
- [x] Do testing: impl Storage :: fn `withdraw_item`
- [x] Do testing: impl Storage :: fn `deposit_item`
- [x] Do testing: impl Storage :: fn `simulate_withdraw_plan`
- [x] Do testing: impl Storage :: fn `simulate_deposit_plan`
- [x] Do testing: impl Storage :: fn `withdraw_plan`
- [x] Do testing: impl Storage :: fn `deposit_plan`
- [x] Do testing: impl Storage :: fn `normalize_amounts_len`
- [x] Do testing: impl Storage :: fn `deposit_into_chest`
- [x] Do testing: impl Storage :: fn `find_empty_chest_index`
- [x] Do testing: impl Storage :: fn `get_overflow_chest`
- [x] Do testing: impl Storage :: fn `get_overflow_chest_mut`
- [x] Do testing: impl Storage :: fn `get_overflow_chest_position`
- [x] Do testing: impl Storage :: const fn `overflow_chest_id`
- [x] Do testing: tests module

- [x] Do logging: struct `ChestTransfer`
- [x] Do logging: struct `Storage`
- [x] Do logging: impl Storage :: const `SLOTS_PER_CHEST`
- [x] Do logging: impl Storage :: const `DEFAULT_SHULKER_CAPACITY`
- [x] Do logging: impl Storage :: fn `save`
- [x] Do logging: impl Storage :: fn `new`
- [x] Do logging: impl Storage :: fn `load`
- [x] Do logging: impl Storage :: fn `add_node`
- [x] Do logging: impl Storage :: fn `total_item_amount`
- [x] Do logging: impl Storage :: fn `get_chest_mut`
- [x] Do logging: impl Storage :: fn `withdraw_item`
- [x] Do logging: impl Storage :: fn `deposit_item`
- [x] Do logging: impl Storage :: fn `simulate_withdraw_plan`
- [x] Do logging: impl Storage :: fn `simulate_deposit_plan`
- [x] Do logging: impl Storage :: fn `withdraw_plan`
- [x] Do logging: impl Storage :: fn `deposit_plan`
- [x] Do logging: impl Storage :: fn `normalize_amounts_len`
- [x] Do logging: impl Storage :: fn `deposit_into_chest`
- [x] Do logging: impl Storage :: fn `find_empty_chest_index`
- [x] Do logging: impl Storage :: fn `get_overflow_chest`
- [x] Do logging: impl Storage :: fn `get_overflow_chest_mut`
- [x] Do logging: impl Storage :: fn `get_overflow_chest_position`
- [x] Do logging: impl Storage :: const fn `overflow_chest_id`
- [x] Do logging: tests module

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

- [x] Do comments: pub mod declarations (`connection`, `navigation`, `trade`, `chest_io`, `shulker`, `inventory`)
- [x] Do comments: struct `BotState`
- [x] Do comments: struct `Bot`
- [x] Do comments: impl `Default for BotState` :: fn `default`
- [x] Do comments: impl Bot :: async fn `new`
- [x] Do comments: impl Bot :: async fn `send_chat_message`
- [x] Do comments: impl Bot :: async fn `send_whisper`
- [x] Do comments: impl Bot :: fn `normalize_item_id`
- [x] Do comments: impl Bot :: fn `chat_subscribe`
- [x] Do comments: async fn `bot_task`
- [x] Do comments: async fn `validate_node_physically`
- [x] Do comments: fn `handle_event_fn`
- [x] Do comments: async fn `handle_event`
- [x] Do comments: async fn `handle_chat_message`

- [x] Do testing: pub mod declarations (`connection`, `navigation`, `trade`, `chest_io`, `shulker`, `inventory`)
- [x] Do testing: struct `BotState`
- [x] Do testing: struct `Bot`
- [x] Do testing: impl `Default for BotState` :: fn `default`
- [x] Do testing: impl Bot :: async fn `new`
- [x] Do testing: impl Bot :: async fn `send_chat_message`
- [x] Do testing: impl Bot :: async fn `send_whisper`
- [x] Do testing: impl Bot :: fn `normalize_item_id`
- [x] Do testing: impl Bot :: fn `chat_subscribe`
- [x] Do testing: async fn `bot_task`
- [x] Do testing: async fn `validate_node_physically`
- [x] Do testing: fn `handle_event_fn`
- [x] Do testing: async fn `handle_event`
- [x] Do testing: async fn `handle_chat_message`

- [x] Do logging: pub mod declarations (`connection`, `navigation`, `trade`, `chest_io`, `shulker`, `inventory`)
- [x] Do logging: struct `BotState`
- [x] Do logging: struct `Bot`
- [x] Do logging: impl `Default for BotState` :: fn `default`
- [x] Do logging: impl Bot :: async fn `new`
- [x] Do logging: impl Bot :: async fn `send_chat_message`
- [x] Do logging: impl Bot :: async fn `send_whisper`
- [x] Do logging: impl Bot :: fn `normalize_item_id`
- [x] Do logging: impl Bot :: fn `chat_subscribe`
- [x] Do logging: async fn `bot_task`
- [x] Do logging: async fn `validate_node_physically`
- [x] Do logging: fn `handle_event_fn`
- [x] Do logging: async fn `handle_event`
- [x] Do logging: async fn `handle_chat_message`

### src/bot/connection.rs

- async fn `connect`
- async fn `disconnect`

**TODO:**

- [x] Do comments: async fn `connect`
- [x] Do comments: async fn `disconnect`

- [x] Do testing: async fn `connect`
- [x] Do testing: async fn `disconnect`

- [x] Do logging: async fn `connect`
- [x] Do logging: async fn `disconnect`

### src/bot/navigation.rs

- async fn `navigate_to_position_once`
- async fn `navigate_to_position`
- async fn `go_to_node`
- async fn `go_to_chest`

**TODO:**

- [x] Do comments: async fn `navigate_to_position_once`
- [x] Do comments: async fn `navigate_to_position`
- [x] Do comments: async fn `go_to_node`
- [x] Do comments: async fn `go_to_chest`

- [x] Do testing: async fn `navigate_to_position_once`
- [x] Do testing: async fn `navigate_to_position`
- [x] Do testing: async fn `go_to_node`
- [x] Do testing: async fn `go_to_chest`

- [x] Do logging: async fn `navigate_to_position_once`
- [x] Do logging: async fn `navigate_to_position`
- [x] Do logging: async fn `go_to_node`
- [x] Do logging: async fn `go_to_chest`

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

- [x] Do comments: async fn `ensure_inventory_empty`
- [x] Do comments: async fn `move_hotbar_to_inventory`
- [x] Do comments: async fn `quick_move_from_container`
- [x] Do comments: fn `verify_holding_shulker`
- [x] Do comments: fn `is_entity_ready`
- [x] Do comments: async fn `wait_for_entity_ready`
- [x] Do comments: fn `carried_item`
- [x] Do comments: async fn `ensure_shulker_in_hotbar_slot_0`
- [x] Do comments: async fn `recover_shulker_to_slot_0`

- [x] Do testing: async fn `ensure_inventory_empty`
- [x] Do testing: async fn `move_hotbar_to_inventory`
- [x] Do testing: async fn `quick_move_from_container`
- [x] Do testing: fn `verify_holding_shulker`
- [x] Do testing: fn `is_entity_ready`
- [x] Do testing: async fn `wait_for_entity_ready`
- [x] Do testing: fn `carried_item`
- [x] Do testing: async fn `ensure_shulker_in_hotbar_slot_0`
- [x] Do testing: async fn `recover_shulker_to_slot_0`

- [x] Do logging: async fn `ensure_inventory_empty`
- [x] Do logging: async fn `move_hotbar_to_inventory`
- [x] Do logging: async fn `quick_move_from_container`
- [x] Do logging: fn `verify_holding_shulker`
- [x] Do logging: fn `is_entity_ready`
- [x] Do logging: async fn `wait_for_entity_ready`
- [x] Do logging: fn `carried_item`
- [x] Do logging: async fn `ensure_shulker_in_hotbar_slot_0`
- [x] Do logging: async fn `recover_shulker_to_slot_0`

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

- [x] Do comments: const `CHUNK_NOT_LOADED_PREFIX`
- [x] Do comments: fn `find_shulker_in_inventory_view`
- [x] Do comments: async fn `place_shulker_in_chest_slot_verified`
- [x] Do comments: async fn `open_chest_container_once`
- [x] Do comments: async fn `open_chest_container_for_validation`
- [x] Do comments: async fn `open_chest_container`
- [x] Do comments: async fn `transfer_items_with_shulker`
- [x] Do comments: async fn `transfer_withdraw_from_shulker`
- [x] Do comments: async fn `transfer_deposit_into_shulker`
- [x] Do comments: async fn `prepare_for_chest_io`
- [x] Do comments: async fn `automated_chest_io`
- [x] Do comments: async fn `withdraw_shulkers`
- [x] Do comments: async fn `deposit_shulkers`

- [x] Do testing: const `CHUNK_NOT_LOADED_PREFIX`
- [x] Do testing: fn `find_shulker_in_inventory_view`
- [x] Do testing: async fn `place_shulker_in_chest_slot_verified`
- [x] Do testing: async fn `open_chest_container_once`
- [x] Do testing: async fn `open_chest_container_for_validation`
- [x] Do testing: async fn `open_chest_container`
- [x] Do testing: async fn `transfer_items_with_shulker`
- [x] Do testing: async fn `transfer_withdraw_from_shulker`
- [x] Do testing: async fn `transfer_deposit_into_shulker`
- [x] Do testing: async fn `prepare_for_chest_io`
- [x] Do testing: async fn `automated_chest_io`
- [x] Do testing: async fn `withdraw_shulkers`
- [x] Do testing: async fn `deposit_shulkers`

- [x] Do logging: const `CHUNK_NOT_LOADED_PREFIX`
- [x] Do logging: fn `find_shulker_in_inventory_view`
- [x] Do logging: async fn `place_shulker_in_chest_slot_verified`
- [x] Do logging: async fn `open_chest_container_once`
- [x] Do logging: async fn `open_chest_container_for_validation`
- [x] Do logging: async fn `open_chest_container`
- [x] Do logging: async fn `transfer_items_with_shulker`
- [x] Do logging: async fn `transfer_withdraw_from_shulker`
- [x] Do logging: async fn `transfer_deposit_into_shulker`
- [x] Do logging: async fn `prepare_for_chest_io`
- [x] Do logging: async fn `automated_chest_io`
- [x] Do logging: async fn `withdraw_shulkers`
- [x] Do logging: async fn `deposit_shulkers`

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

- [x] Do comments: const `SHULKER_BOX_IDS`
- [x] Do comments: fn `shulker_station_position`
- [x] Do comments: fn `is_shulker_box`
- [x] Do comments: fn `validate_chest_slot_is_shulker` (cfg(test))
- [x] Do comments: async fn `pickup_shulker_from_station`
- [x] Do comments: async fn `open_shulker_at_station_once`
- [x] Do comments: async fn `open_shulker_at_station`
- [x] Do comments: test `test_is_shulker_box`
- [x] Do comments: test `test_validate_chest_slot_is_shulker`
- [x] Do comments: test `test_shulker_station_position`

- [x] Do testing: const `SHULKER_BOX_IDS`
- [x] Do testing: fn `shulker_station_position`
- [x] Do testing: fn `is_shulker_box`
- [x] Do testing: fn `validate_chest_slot_is_shulker` (cfg(test))
- [x] Do testing: async fn `pickup_shulker_from_station`
- [x] Do testing: async fn `open_shulker_at_station_once`
- [x] Do testing: async fn `open_shulker_at_station`
- [x] Do testing: test `test_is_shulker_box`
- [x] Do testing: test `test_validate_chest_slot_is_shulker`
- [x] Do testing: test `test_shulker_station_position`

- [x] Do logging: const `SHULKER_BOX_IDS`
- [x] Do logging: fn `shulker_station_position`
- [x] Do logging: fn `is_shulker_box`
- [x] Do logging: fn `validate_chest_slot_is_shulker` (cfg(test))
- [x] Do logging: async fn `pickup_shulker_from_station`
- [x] Do logging: async fn `open_shulker_at_station_once`
- [x] Do logging: async fn `open_shulker_at_station`
- [x] Do logging: test `test_is_shulker_box`
- [x] Do logging: test `test_validate_chest_slot_is_shulker`
- [x] Do logging: test `test_shulker_station_position`

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

- [x] Do comments: fn `trade_bot_offer_slots`
- [x] Do comments: fn `trade_player_offer_slots`
- [x] Do comments: fn `trade_player_status_slots`
- [x] Do comments: fn `trade_accept_slots`
- [x] Do comments: fn `trade_cancel_slots`
- [x] Do comments: async fn `wait_for_trade_menu_or_failure`
- [x] Do comments: async fn `place_items_from_inventory_into_trade`
- [x] Do comments: fn `validate_player_items`
- [x] Do comments: async fn `execute_trade_with_player`
- [x] Do comments: test `test_trade_bot_offer_slots`
- [x] Do comments: test `test_trade_player_offer_slots`
- [x] Do comments: test `test_trade_player_status_slots`
- [x] Do comments: test `test_trade_accept_slots`
- [x] Do comments: test `test_trade_cancel_slots`
- [x] Do comments: test `test_trade_slot_sets_disjoint`

- [x] Do testing: fn `trade_bot_offer_slots`
- [x] Do testing: fn `trade_player_offer_slots`
- [x] Do testing: fn `trade_player_status_slots`
- [x] Do testing: fn `trade_accept_slots`
- [x] Do testing: fn `trade_cancel_slots`
- [x] Do testing: async fn `wait_for_trade_menu_or_failure`
- [x] Do testing: async fn `place_items_from_inventory_into_trade`
- [x] Do testing: fn `validate_player_items`
- [x] Do testing: async fn `execute_trade_with_player`
- [x] Do testing: test `test_trade_bot_offer_slots`
- [x] Do testing: test `test_trade_player_offer_slots`
- [x] Do testing: test `test_trade_player_status_slots`
- [x] Do testing: test `test_trade_accept_slots`
- [x] Do testing: test `test_trade_cancel_slots`
- [x] Do testing: test `test_trade_slot_sets_disjoint`

- [x] Do logging: fn `trade_bot_offer_slots`
- [x] Do logging: fn `trade_player_offer_slots`
- [x] Do logging: fn `trade_player_status_slots`
- [x] Do logging: fn `trade_accept_slots`
- [x] Do logging: fn `trade_cancel_slots`
- [x] Do logging: async fn `wait_for_trade_menu_or_failure`
- [x] Do logging: async fn `place_items_from_inventory_into_trade`
- [x] Do logging: fn `validate_player_items`
- [x] Do logging: async fn `execute_trade_with_player`
- [x] Do logging: test `test_trade_bot_offer_slots`
- [x] Do logging: test `test_trade_player_offer_slots`
- [x] Do logging: test `test_trade_player_status_slots`
- [x] Do logging: test `test_trade_accept_slots`
- [x] Do logging: test `test_trade_cancel_slots`
- [x] Do logging: test `test_trade_slot_sets_disjoint`

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

- [x] Do comments: pub mod declarations (`command`, `handlers`, `journal`, `orders`, `pricing`, `queue`, `rate_limit`, `rollback`, `state`, `trade_state`, `utils`)
- [x] Do comments: struct `Store`
- [x] Do comments: impl Store :: async fn `new`
- [x] Do comments: impl Store :: async fn `run`
- [x] Do comments: impl Store :: async fn `process_next_order`
- [x] Do comments: impl Store :: fn `reload_config`
- [x] Do comments: impl Store :: fn `advance_trade`
- [x] Do comments: impl Store :: async fn `handle_bot_message`
- [x] Do comments: impl Store :: fn `expect_pair`
- [x] Do comments: impl Store :: fn `expect_pair_mut`
- [x] Do comments: impl Store :: fn `expect_user`
- [x] Do comments: impl Store :: fn `expect_user_mut`
- [x] Do comments: impl Store :: fn `apply_chest_sync`
- [x] Do comments: impl Store :: fn `get_node_position`
- [x] Do comments: impl Store :: fn `new_for_test`

- [x] Do testing: pub mod declarations (`command`, `handlers`, `journal`, `orders`, `pricing`, `queue`, `rate_limit`, `rollback`, `state`, `trade_state`, `utils`)
- [x] Do testing: struct `Store`
- [x] Do testing: impl Store :: async fn `new`
- [x] Do testing: impl Store :: async fn `run`
- [x] Do testing: impl Store :: async fn `process_next_order`
- [x] Do testing: impl Store :: fn `reload_config`
- [x] Do testing: impl Store :: fn `advance_trade`
- [x] Do testing: impl Store :: async fn `handle_bot_message`
- [x] Do testing: impl Store :: fn `expect_pair`
- [x] Do testing: impl Store :: fn `expect_pair_mut`
- [x] Do testing: impl Store :: fn `expect_user`
- [x] Do testing: impl Store :: fn `expect_user_mut`
- [x] Do testing: impl Store :: fn `apply_chest_sync`
- [x] Do testing: impl Store :: fn `get_node_position`
- [x] Do testing: impl Store :: fn `new_for_test`

- [x] Do logging: pub mod declarations (`command`, `handlers`, `journal`, `orders`, `pricing`, `queue`, `rate_limit`, `rollback`, `state`, `trade_state`, `utils`)
- [x] Do logging: struct `Store`
- [x] Do logging: impl Store :: async fn `new`
- [x] Do logging: impl Store :: async fn `run`
- [x] Do logging: impl Store :: async fn `process_next_order`
- [x] Do logging: impl Store :: fn `reload_config`
- [x] Do logging: impl Store :: fn `advance_trade`
- [x] Do logging: impl Store :: async fn `handle_bot_message`
- [x] Do logging: impl Store :: fn `expect_pair`
- [x] Do logging: impl Store :: fn `expect_pair_mut`
- [x] Do logging: impl Store :: fn `expect_user`
- [x] Do logging: impl Store :: fn `expect_user_mut`
- [x] Do logging: impl Store :: fn `apply_chest_sync`
- [x] Do logging: impl Store :: fn `get_node_position`
- [x] Do logging: impl Store :: fn `new_for_test`

### src/store/state.rs

- fn `apply_chest_sync`
- fn `save`
- fn `audit_state`
- fn `assert_invariants`

**TODO:**

- [x] Do comments: fn `apply_chest_sync`
- [x] Do comments: fn `save`
- [x] Do comments: fn `audit_state`
- [x] Do comments: fn `assert_invariants`

- [x] Do testing: fn `apply_chest_sync`
- [x] Do testing: fn `save`
- [x] Do testing: fn `audit_state`
- [x] Do testing: fn `assert_invariants`

- [x] Do logging: fn `apply_chest_sync`
- [x] Do logging: fn `save`
- [x] Do logging: fn `audit_state`
- [x] Do logging: fn `assert_invariants`

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

- [x] Do comments: enum `Command`
- [x] Do comments: fn `parse_command`
- [x] Do comments: fn `parse_item_quantity`
- [x] Do comments: fn `parse_item_amount`
- [x] Do comments: fn `parse_optional_amount`
- [x] Do comments: fn `parse_price`
- [x] Do comments: fn `parse_balance`
- [x] Do comments: fn `parse_pay`
- [x] Do comments: fn `parse_page`
- [x] Do comments: fn `parse_cancel`
- [x] Do comments: tests module

- [x] Do testing: enum `Command`
- [x] Do testing: fn `parse_command`
- [x] Do testing: fn `parse_item_quantity`
- [x] Do testing: fn `parse_item_amount`
- [x] Do testing: fn `parse_optional_amount`
- [x] Do testing: fn `parse_price`
- [x] Do testing: fn `parse_balance`
- [x] Do testing: fn `parse_pay`
- [x] Do testing: fn `parse_page`
- [x] Do testing: fn `parse_cancel`
- [x] Do testing: tests module

- [x] Do logging: enum `Command`
- [x] Do logging: fn `parse_command`
- [x] Do logging: fn `parse_item_quantity`
- [x] Do logging: fn `parse_item_amount`
- [x] Do logging: fn `parse_optional_amount`
- [x] Do logging: fn `parse_price`
- [x] Do logging: fn `parse_balance`
- [x] Do logging: fn `parse_pay`
- [x] Do logging: fn `parse_page`
- [x] Do logging: fn `parse_cancel`
- [x] Do logging: tests module

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

- [x] Do comments: const `JOURNAL_FILE`
- [x] Do comments: static `NEXT_OPERATION_ID`
- [x] Do comments: type alias `SharedJournal`
- [x] Do comments: struct `JournalEntry`
- [x] Do comments: struct `Journal`
- [x] Do comments: enum `JournalOp`
- [x] Do comments: enum `JournalState`
- [x] Do comments: impl `Default for Journal` :: fn `default`
- [x] Do comments: impl Journal :: fn `load`
- [x] Do comments: impl Journal :: fn `load_from`
- [x] Do comments: impl Journal :: fn `clear_leftover`
- [x] Do comments: impl Journal :: fn `begin`
- [x] Do comments: impl Journal :: fn `advance`
- [x] Do comments: impl Journal :: fn `complete`
- [x] Do comments: impl Journal :: fn `current`
- [x] Do comments: impl Journal :: fn `persist`
- [x] Do comments: tests module

- [x] Do testing: const `JOURNAL_FILE`
- [x] Do testing: static `NEXT_OPERATION_ID`
- [x] Do testing: type alias `SharedJournal`
- [x] Do testing: struct `JournalEntry`
- [x] Do testing: struct `Journal`
- [x] Do testing: enum `JournalOp`
- [x] Do testing: enum `JournalState`
- [x] Do testing: impl `Default for Journal` :: fn `default`
- [x] Do testing: impl Journal :: fn `load`
- [x] Do testing: impl Journal :: fn `load_from`
- [x] Do testing: impl Journal :: fn `clear_leftover`
- [x] Do testing: impl Journal :: fn `begin`
- [x] Do testing: impl Journal :: fn `advance`
- [x] Do testing: impl Journal :: fn `complete`
- [x] Do testing: impl Journal :: fn `current`
- [x] Do testing: impl Journal :: fn `persist`
- [x] Do testing: tests module

- [x] Do logging: const `JOURNAL_FILE`
- [x] Do logging: static `NEXT_OPERATION_ID`
- [x] Do logging: type alias `SharedJournal`
- [x] Do logging: struct `JournalEntry`
- [x] Do logging: struct `Journal`
- [x] Do logging: enum `JournalOp`
- [x] Do logging: enum `JournalState`
- [x] Do logging: impl `Default for Journal` :: fn `default`
- [x] Do logging: impl Journal :: fn `load`
- [x] Do logging: impl Journal :: fn `load_from`
- [x] Do logging: impl Journal :: fn `clear_leftover`
- [x] Do logging: impl Journal :: fn `begin`
- [x] Do logging: impl Journal :: fn `advance`
- [x] Do logging: impl Journal :: fn `complete`
- [x] Do logging: impl Journal :: fn `current`
- [x] Do logging: impl Journal :: fn `persist`
- [x] Do logging: tests module

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

- [x] Do comments: struct `BuyPlan`
- [x] Do comments: struct `SellPlan`
- [x] Do comments: enum `ChestDirection`
- [x] Do comments: async fn `execute_chest_transfers`
- [x] Do comments: async fn `perform_trade`
- [x] Do comments: async fn `validate_and_plan_buy`
- [x] Do comments: async fn `handle_buy_order`
- [x] Do comments: async fn `validate_and_plan_sell`
- [x] Do comments: async fn `handle_sell_order`
- [x] Do comments: async fn `execute_queued_order`
- [x] Do comments: tests module

- [x] Do testing: struct `BuyPlan`
- [x] Do testing: struct `SellPlan`
- [x] Do testing: enum `ChestDirection`
- [x] Do testing: async fn `execute_chest_transfers`
- [x] Do testing: async fn `perform_trade`
- [x] Do testing: async fn `validate_and_plan_buy`
- [x] Do testing: async fn `handle_buy_order`
- [x] Do testing: async fn `validate_and_plan_sell`
- [x] Do testing: async fn `handle_sell_order`
- [x] Do testing: async fn `execute_queued_order`
- [x] Do testing: tests module

- [x] Do logging: struct `BuyPlan`
- [x] Do logging: struct `SellPlan`
- [x] Do logging: enum `ChestDirection`
- [x] Do logging: async fn `execute_chest_transfers`
- [x] Do logging: async fn `perform_trade`
- [x] Do logging: async fn `validate_and_plan_buy`
- [x] Do logging: async fn `handle_buy_order`
- [x] Do logging: async fn `validate_and_plan_sell`
- [x] Do logging: async fn `handle_sell_order`
- [x] Do logging: async fn `execute_queued_order`
- [x] Do logging: tests module

### src/store/pricing.rs

- fn `validate_fee`
- fn `reserves_sufficient`
- fn `calculate_buy_cost`
- fn `buy_cost_pure`
- fn `calculate_sell_payout`
- fn `sell_payout_pure`
- tests module (includes proptests)

**TODO:**

- [x] Do comments: fn `validate_fee`
- [x] Do comments: fn `reserves_sufficient`
- [x] Do comments: fn `calculate_buy_cost`
- [x] Do comments: fn `buy_cost_pure`
- [x] Do comments: fn `calculate_sell_payout`
- [x] Do comments: fn `sell_payout_pure`
- [x] Do comments: tests module (includes proptests)

- [x] Do testing: fn `validate_fee`
- [x] Do testing: fn `reserves_sufficient`
- [x] Do testing: fn `calculate_buy_cost`
- [x] Do testing: fn `buy_cost_pure`
- [x] Do testing: fn `calculate_sell_payout`
- [x] Do testing: fn `sell_payout_pure`
- [x] Do testing: tests module (includes proptests)

- [x] Do logging: fn `validate_fee`
- [x] Do logging: fn `reserves_sufficient`
- [x] Do logging: fn `calculate_buy_cost`
- [x] Do logging: fn `buy_cost_pure`
- [x] Do logging: fn `calculate_sell_payout`
- [x] Do logging: fn `sell_payout_pure`
- [x] Do logging: tests module (includes proptests)

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

- [x] Do comments: struct `QueuedOrder`
- [x] Do comments: struct `OrderQueue`
- [x] Do comments: struct `QueuePersist`
- [x] Do comments: impl QueuedOrder :: fn `new`
- [x] Do comments: impl QueuedOrder :: fn `description`
- [x] Do comments: impl `Default for OrderQueue` :: fn `default`
- [x] Do comments: impl OrderQueue :: fn `new`
- [x] Do comments: impl OrderQueue :: fn `load`
- [x] Do comments: impl OrderQueue :: fn `save`
- [x] Do comments: impl OrderQueue :: fn `add`
- [x] Do comments: impl OrderQueue :: fn `pop`
- [x] Do comments: impl OrderQueue :: fn `is_empty`
- [x] Do comments: impl OrderQueue :: fn `len`
- [x] Do comments: impl OrderQueue :: fn `get_position`
- [x] Do comments: impl OrderQueue :: fn `get_user_position`
- [x] Do comments: impl OrderQueue :: fn `user_order_count`
- [x] Do comments: impl OrderQueue :: fn `get_user_orders`
- [x] Do comments: impl OrderQueue :: fn `cancel`
- [x] Do comments: impl OrderQueue :: fn `estimate_wait`
- [x] Do comments: tests module

- [x] Do testing: struct `QueuedOrder`
- [x] Do testing: struct `OrderQueue`
- [x] Do testing: struct `QueuePersist`
- [x] Do testing: impl QueuedOrder :: fn `new`
- [x] Do testing: impl QueuedOrder :: fn `description`
- [x] Do testing: impl `Default for OrderQueue` :: fn `default`
- [x] Do testing: impl OrderQueue :: fn `new`
- [x] Do testing: impl OrderQueue :: fn `load`
- [x] Do testing: impl OrderQueue :: fn `save`
- [x] Do testing: impl OrderQueue :: fn `add`
- [x] Do testing: impl OrderQueue :: fn `pop`
- [x] Do testing: impl OrderQueue :: fn `is_empty`
- [x] Do testing: impl OrderQueue :: fn `len`
- [x] Do testing: impl OrderQueue :: fn `get_position`
- [x] Do testing: impl OrderQueue :: fn `get_user_position`
- [x] Do testing: impl OrderQueue :: fn `user_order_count`
- [x] Do testing: impl OrderQueue :: fn `get_user_orders`
- [x] Do testing: impl OrderQueue :: fn `cancel`
- [x] Do testing: impl OrderQueue :: fn `estimate_wait`
- [x] Do testing: tests module

- [x] Do logging: struct `QueuedOrder`
- [x] Do logging: struct `OrderQueue`
- [x] Do logging: struct `QueuePersist`
- [x] Do logging: impl QueuedOrder :: fn `new`
- [x] Do logging: impl QueuedOrder :: fn `description`
- [x] Do logging: impl `Default for OrderQueue` :: fn `default`
- [x] Do logging: impl OrderQueue :: fn `new`
- [x] Do logging: impl OrderQueue :: fn `load`
- [x] Do logging: impl OrderQueue :: fn `save`
- [x] Do logging: impl OrderQueue :: fn `add`
- [x] Do logging: impl OrderQueue :: fn `pop`
- [x] Do logging: impl OrderQueue :: fn `is_empty`
- [x] Do logging: impl OrderQueue :: fn `len`
- [x] Do logging: impl OrderQueue :: fn `get_position`
- [x] Do logging: impl OrderQueue :: fn `get_user_position`
- [x] Do logging: impl OrderQueue :: fn `user_order_count`
- [x] Do logging: impl OrderQueue :: fn `get_user_orders`
- [x] Do logging: impl OrderQueue :: fn `cancel`
- [x] Do logging: impl OrderQueue :: fn `estimate_wait`
- [x] Do logging: tests module

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

- [x] Do comments: struct `UserRateLimit`
- [x] Do comments: struct `RateLimiter`
- [x] Do comments: fn `calculate_cooldown`
- [x] Do comments: impl UserRateLimit :: fn `new`
- [x] Do comments: impl `Default for RateLimiter` :: fn `default`
- [x] Do comments: impl RateLimiter :: fn `new`
- [x] Do comments: impl RateLimiter :: fn `check`
- [x] Do comments: impl RateLimiter :: fn `cleanup_stale`
- [x] Do comments: tests module

- [x] Do testing: struct `UserRateLimit`
- [x] Do testing: struct `RateLimiter`
- [x] Do testing: fn `calculate_cooldown`
- [x] Do testing: impl UserRateLimit :: fn `new`
- [x] Do testing: impl `Default for RateLimiter` :: fn `default`
- [x] Do testing: impl RateLimiter :: fn `new`
- [x] Do testing: impl RateLimiter :: fn `check`
- [x] Do testing: impl RateLimiter :: fn `cleanup_stale`
- [x] Do testing: tests module

- [x] Do logging: struct `UserRateLimit`
- [x] Do logging: struct `RateLimiter`
- [x] Do logging: fn `calculate_cooldown`
- [x] Do logging: impl UserRateLimit :: fn `new`
- [x] Do logging: impl `Default for RateLimiter` :: fn `default`
- [x] Do logging: impl RateLimiter :: fn `new`
- [x] Do logging: impl RateLimiter :: fn `check`
- [x] Do logging: impl RateLimiter :: fn `cleanup_stale`
- [x] Do logging: tests module

### src/store/rollback.rs

- struct `RollbackResult`
- impl RollbackResult :: fn `has_failures`
- fn `chest_from_transfer`
- async fn `deposit_transfers`
- async fn `rollback_amount_to_storage`

**TODO:**

- [x] Do comments: struct `RollbackResult`
- [x] Do comments: impl RollbackResult :: fn `has_failures`
- [x] Do comments: fn `chest_from_transfer`
- [x] Do comments: async fn `deposit_transfers`
- [x] Do comments: async fn `rollback_amount_to_storage`

- [x] Do testing: struct `RollbackResult`
- [x] Do testing: impl RollbackResult :: fn `has_failures`
- [x] Do testing: fn `chest_from_transfer`
- [x] Do testing: async fn `deposit_transfers`
- [x] Do testing: async fn `rollback_amount_to_storage`

- [x] Do logging: struct `RollbackResult`
- [x] Do logging: impl RollbackResult :: fn `has_failures`
- [x] Do logging: fn `chest_from_transfer`
- [x] Do logging: async fn `deposit_transfers`
- [x] Do logging: async fn `rollback_amount_to_storage`

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

- [x] Do comments: const `TRADE_STATE_FILE`
- [x] Do comments: struct `TradeResult`
- [x] Do comments: struct `CompletedTrade`
- [x] Do comments: enum `TradeState`
- [x] Do comments: impl TradeState :: fn `new`
- [x] Do comments: impl TradeState :: fn `begin_withdrawal`
- [x] Do comments: impl TradeState :: fn `begin_trading`
- [x] Do comments: impl TradeState :: fn `begin_depositing`
- [x] Do comments: impl TradeState :: fn `commit`
- [x] Do comments: impl TradeState :: fn `rollback`
- [x] Do comments: impl TradeState :: fn `phase`
- [x] Do comments: impl TradeState :: fn `is_terminal`
- [x] Do comments: impl TradeState :: fn `order`
- [x] Do comments: impl `fmt::Display for TradeState` :: fn `fmt`
- [x] Do comments: fn `persist`
- [x] Do comments: fn `load_persisted`
- [x] Do comments: fn `clear_persisted`
- [x] Do comments: tests module

- [x] Do testing: const `TRADE_STATE_FILE`
- [x] Do testing: struct `TradeResult`
- [x] Do testing: struct `CompletedTrade`
- [x] Do testing: enum `TradeState`
- [x] Do testing: impl TradeState :: fn `new`
- [x] Do testing: impl TradeState :: fn `begin_withdrawal`
- [x] Do testing: impl TradeState :: fn `begin_trading`
- [x] Do testing: impl TradeState :: fn `begin_depositing`
- [x] Do testing: impl TradeState :: fn `commit`
- [x] Do testing: impl TradeState :: fn `rollback`
- [x] Do testing: impl TradeState :: fn `phase`
- [x] Do testing: impl TradeState :: fn `is_terminal`
- [x] Do testing: impl TradeState :: fn `order`
- [x] Do testing: impl `fmt::Display for TradeState` :: fn `fmt`
- [x] Do testing: fn `persist`
- [x] Do testing: fn `load_persisted`
- [x] Do testing: fn `clear_persisted`
- [x] Do testing: tests module

- [x] Do logging: const `TRADE_STATE_FILE`
- [x] Do logging: struct `TradeResult`
- [x] Do logging: struct `CompletedTrade`
- [x] Do logging: enum `TradeState`
- [x] Do logging: impl TradeState :: fn `new`
- [x] Do logging: impl TradeState :: fn `begin_withdrawal`
- [x] Do logging: impl TradeState :: fn `begin_trading`
- [x] Do logging: impl TradeState :: fn `begin_depositing`
- [x] Do logging: impl TradeState :: fn `commit`
- [x] Do logging: impl TradeState :: fn `rollback`
- [x] Do logging: impl TradeState :: fn `phase`
- [x] Do logging: impl TradeState :: fn `is_terminal`
- [x] Do logging: impl TradeState :: fn `order`
- [x] Do logging: impl `fmt::Display for TradeState` :: fn `fmt`
- [x] Do logging: fn `persist`
- [x] Do logging: fn `load_persisted`
- [x] Do logging: fn `clear_persisted`
- [x] Do logging: tests module

### src/store/utils.rs

- static `UUID_CACHE`
- type alias `UuidCache`
- fn `uuid_cache`
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

- [x] Do comments: static `UUID_CACHE`
- [x] Do comments: type alias `UuidCache`
- [x] Do comments: fn `uuid_cache`
- [x] Do comments: async fn `resolve_user_uuid`
- [x] Do comments: fn `clear_uuid_cache`
- [x] Do comments: fn `cleanup_uuid_cache`
- [x] Do comments: fn `ensure_user_exists`
- [x] Do comments: fn `is_operator`
- [x] Do comments: fn `get_node_position`
- [x] Do comments: async fn `send_message_to_player`
- [x] Do comments: fn `summarize_transfers`
- [x] Do comments: fn `fmt_issues`
- [x] Do comments: tests module

- [x] Do testing: static `UUID_CACHE`
- [x] Do testing: type alias `UuidCache`
- [x] Do testing: fn `uuid_cache`
- [x] Do testing: async fn `resolve_user_uuid`
- [x] Do testing: fn `clear_uuid_cache`
- [x] Do testing: fn `cleanup_uuid_cache`
- [x] Do testing: fn `ensure_user_exists`
- [x] Do testing: fn `is_operator`
- [x] Do testing: fn `get_node_position`
- [x] Do testing: async fn `send_message_to_player`
- [x] Do testing: fn `summarize_transfers`
- [x] Do testing: fn `fmt_issues`
- [x] Do testing: tests module

- [x] Do logging: static `UUID_CACHE`
- [x] Do logging: type alias `UuidCache`
- [x] Do logging: fn `uuid_cache`
- [x] Do logging: async fn `resolve_user_uuid`
- [x] Do logging: fn `clear_uuid_cache`
- [x] Do logging: fn `cleanup_uuid_cache`
- [x] Do logging: fn `ensure_user_exists`
- [x] Do logging: fn `is_operator`
- [x] Do logging: fn `get_node_position`
- [x] Do logging: async fn `send_message_to_player`
- [x] Do logging: fn `summarize_transfers`
- [x] Do logging: fn `fmt_issues`
- [x] Do logging: tests module

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

- [x] Do comments: pub mod `player`
- [x] Do comments: pub mod `operator`
- [x] Do comments: pub mod `cli`
- [x] Do comments: mod `buy`
- [x] Do comments: mod `sell`
- [x] Do comments: mod `deposit`
- [x] Do comments: mod `withdraw`
- [x] Do comments: mod `info`
- [x] Do comments: pub(crate) mod `validation`

- [x] Do testing: pub mod `player`
- [x] Do testing: pub mod `operator`
- [x] Do testing: pub mod `cli`
- [x] Do testing: mod `buy`
- [x] Do testing: mod `sell`
- [x] Do testing: mod `deposit`
- [x] Do testing: mod `withdraw`
- [x] Do testing: mod `info`
- [x] Do testing: pub(crate) mod `validation`

- [x] Do logging: pub mod `player`
- [x] Do logging: pub mod `operator`
- [x] Do logging: pub mod `cli`
- [x] Do logging: mod `buy`
- [x] Do logging: mod `sell`
- [x] Do logging: mod `deposit`
- [x] Do logging: mod `withdraw`
- [x] Do logging: mod `info`
- [x] Do logging: pub(crate) mod `validation`

### src/store/handlers/validation.rs

- fn `validate_item_name`
- fn `validate_quantity`
- fn `validate_username`

**TODO:**

- [x] Do comments: fn `validate_item_name`
- [x] Do comments: fn `validate_quantity`
- [x] Do comments: fn `validate_username`

- [x] Do testing: fn `validate_item_name`
- [x] Do testing: fn `validate_quantity`
- [x] Do testing: fn `validate_username`

- [x] Do logging: fn `validate_item_name`
- [x] Do logging: fn `validate_quantity`
- [x] Do logging: fn `validate_username`

### src/store/handlers/buy.rs

- async fn `handle`

**TODO:**

- [x] Do comments: async fn `handle`

- [x] Do testing: async fn `handle`

- [x] Do logging: async fn `handle`

### src/store/handlers/sell.rs

- async fn `handle`

**TODO:**

- [x] Do comments: async fn `handle`

- [x] Do testing: async fn `handle`

- [x] Do logging: async fn `handle`

### src/store/handlers/withdraw.rs

- async fn `handle_enqueue`
- async fn `handle_withdraw_balance_queued`

**TODO:**

- [x] Do comments: async fn `handle_enqueue`
- [x] Do comments: async fn `handle_withdraw_balance_queued`

- [x] Do testing: async fn `handle_enqueue`
- [x] Do testing: async fn `handle_withdraw_balance_queued`

- [x] Do logging: async fn `handle_enqueue`
- [x] Do logging: async fn `handle_withdraw_balance_queued`

### src/store/handlers/deposit.rs

- async fn `handle_enqueue`
- async fn `handle_deposit_balance_queued`

**TODO:**

- [x] Do comments: async fn `handle_enqueue`
- [x] Do comments: async fn `handle_deposit_balance_queued`

- [x] Do testing: async fn `handle_enqueue`
- [x] Do testing: async fn `handle_deposit_balance_queued`

- [x] Do logging: async fn `handle_enqueue`
- [x] Do logging: async fn `handle_deposit_balance_queued`

### src/store/handlers/player.rs

- async fn `handle_player_command`

**TODO:**

- [x] Do comments: async fn `handle_player_command`

- [x] Do testing: async fn `handle_player_command`

- [x] Do logging: async fn `handle_player_command`

### src/store/handlers/operator.rs

- async fn `handle_additem_order`
- async fn `handle_removeitem_order`
- async fn `handle_add_currency`
- async fn `handle_remove_currency`

**TODO:**

- [x] Do comments: async fn `handle_additem_order`
- [x] Do comments: async fn `handle_removeitem_order`
- [x] Do comments: async fn `handle_add_currency`
- [x] Do comments: async fn `handle_remove_currency`

- [x] Do testing: async fn `handle_additem_order`
- [x] Do testing: async fn `handle_removeitem_order`
- [x] Do testing: async fn `handle_add_currency`
- [x] Do testing: async fn `handle_remove_currency`

- [x] Do logging: async fn `handle_additem_order`
- [x] Do logging: async fn `handle_removeitem_order`
- [x] Do logging: async fn `handle_add_currency`
- [x] Do logging: async fn `handle_remove_currency`

### src/store/handlers/cli.rs

- async fn `handle_cli_message`

**TODO:**

- [x] Do comments: async fn `handle_cli_message`

- [x] Do testing: async fn `handle_cli_message`

- [x] Do logging: async fn `handle_cli_message`

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

- [x] Do comments: async fn `handle_price`
- [x] Do comments: async fn `handle_balance`
- [x] Do comments: async fn `handle_pay`
- [x] Do comments: async fn `handle_items`
- [x] Do comments: async fn `handle_queue`
- [x] Do comments: async fn `handle_cancel`
- [x] Do comments: async fn `handle_status`
- [x] Do comments: async fn `handle_help`
- [x] Do comments: async fn `handle_price_command`
- [x] Do comments: async fn `handle_status_command`
- [x] Do comments: async fn `handle_items_command`
- [x] Do comments: async fn `handle_help_command`
- [x] Do comments: async fn `get_user_balance_async`
- [x] Do comments: async fn `pay_async`

- [x] Do testing: async fn `handle_price`
- [x] Do testing: async fn `handle_balance`
- [x] Do testing: async fn `handle_pay`
- [x] Do testing: async fn `handle_items`
- [x] Do testing: async fn `handle_queue`
- [x] Do testing: async fn `handle_cancel`
- [x] Do testing: async fn `handle_status`
- [x] Do testing: async fn `handle_help`
- [x] Do testing: async fn `handle_price_command`
- [x] Do testing: async fn `handle_status_command`
- [x] Do testing: async fn `handle_items_command`
- [x] Do testing: async fn `handle_help_command`
- [x] Do testing: async fn `get_user_balance_async`
- [x] Do testing: async fn `pay_async`

- [x] Do logging: async fn `handle_price`
- [x] Do logging: async fn `handle_balance`
- [x] Do logging: async fn `handle_pay`
- [x] Do logging: async fn `handle_items`
- [x] Do logging: async fn `handle_queue`
- [x] Do logging: async fn `handle_cancel`
- [x] Do logging: async fn `handle_status`
- [x] Do logging: async fn `handle_help`
- [x] Do logging: async fn `handle_price_command`
- [x] Do logging: async fn `handle_status_command`
- [x] Do logging: async fn `handle_items_command`
- [x] Do logging: async fn `handle_help_command`
- [x] Do logging: async fn `get_user_balance_async`
- [x] Do logging: async fn `pay_async`
