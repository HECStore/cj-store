### src/main.rs

- fn `main`
- fn `print_usage`
- fn `run_validate_only`
- fn `spawn_config_watcher`

**TODO:**

- [ ] Do comments: fn `main`
- [ ] Do comments: fn `print_usage`
- [ ] Do comments: fn `run_validate_only`
- [ ] Do comments: fn `spawn_config_watcher`

- [ ] Do testing: fn `main`
- [ ] Do testing: fn `print_usage`
- [ ] Do testing: fn `run_validate_only`
- [ ] Do testing: fn `spawn_config_watcher`

- [ ] Do logging: fn `main`
- [ ] Do logging: fn `print_usage`
- [ ] Do logging: fn `run_validate_only`
- [ ] Do logging: fn `spawn_config_watcher`

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

- [ ] Do comments: fn `with_retry`
- [ ] Do comments: fn `cli_task`
- [ ] Do comments: fn `get_balances`
- [ ] Do comments: fn `get_pairs`
- [ ] Do comments: fn `set_operator`
- [ ] Do comments: fn `add_node`
- [ ] Do comments: fn `add_node_with_validation`
- [ ] Do comments: fn `discover_storage`
- [ ] Do comments: fn `remove_node`
- [ ] Do comments: fn `add_pair`
- [ ] Do comments: fn `remove_pair`
- [ ] Do comments: fn `view_storage`
- [ ] Do comments: fn `view_trades`
- [ ] Do comments: fn `restart_bot`
- [ ] Do comments: fn `clear_stuck_order`
- [ ] Do comments: fn `audit_state`

- [ ] Do testing: fn `with_retry`
- [ ] Do testing: fn `cli_task`
- [ ] Do testing: fn `get_balances`
- [ ] Do testing: fn `get_pairs`
- [ ] Do testing: fn `set_operator`
- [ ] Do testing: fn `add_node`
- [ ] Do testing: fn `add_node_with_validation`
- [ ] Do testing: fn `discover_storage`
- [ ] Do testing: fn `remove_node`
- [ ] Do testing: fn `add_pair`
- [ ] Do testing: fn `remove_pair`
- [ ] Do testing: fn `view_storage`
- [ ] Do testing: fn `view_trades`
- [ ] Do testing: fn `restart_bot`
- [ ] Do testing: fn `clear_stuck_order`
- [ ] Do testing: fn `audit_state`

- [ ] Do logging: fn `with_retry`
- [ ] Do logging: fn `cli_task`
- [ ] Do logging: fn `get_balances`
- [ ] Do logging: fn `get_pairs`
- [ ] Do logging: fn `set_operator`
- [ ] Do logging: fn `add_node`
- [ ] Do logging: fn `add_node_with_validation`
- [ ] Do logging: fn `discover_storage`
- [ ] Do logging: fn `remove_node`
- [ ] Do logging: fn `add_pair`
- [ ] Do logging: fn `remove_pair`
- [ ] Do logging: fn `view_storage`
- [ ] Do logging: fn `view_trades`
- [ ] Do logging: fn `restart_bot`
- [ ] Do logging: fn `clear_stuck_order`
- [ ] Do logging: fn `audit_state`

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

- [ ] Do comments: struct `Config`
- [ ] Do comments: fn `default_trade_timeout_ms`
- [ ] Do comments: fn `default_pathfinding_timeout_ms`
- [ ] Do comments: fn `default_max_orders`
- [ ] Do comments: fn `default_max_trades_in_memory`
- [ ] Do comments: fn `default_autosave_interval_secs`
- [ ] Do comments: impl Config :: fn `validate`
- [ ] Do comments: impl Config :: fn `load`

- [ ] Do testing: struct `Config`
- [ ] Do testing: fn `default_trade_timeout_ms`
- [ ] Do testing: fn `default_pathfinding_timeout_ms`
- [ ] Do testing: fn `default_max_orders`
- [ ] Do testing: fn `default_max_trades_in_memory`
- [ ] Do testing: fn `default_autosave_interval_secs`
- [ ] Do testing: impl Config :: fn `validate`
- [ ] Do testing: impl Config :: fn `load`

- [ ] Do logging: struct `Config`
- [ ] Do logging: fn `default_trade_timeout_ms`
- [ ] Do logging: fn `default_pathfinding_timeout_ms`
- [ ] Do logging: fn `default_max_orders`
- [ ] Do logging: fn `default_max_trades_in_memory`
- [ ] Do logging: fn `default_autosave_interval_secs`
- [ ] Do logging: impl Config :: fn `validate`
- [ ] Do logging: impl Config :: fn `load`

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

- [ ] Do comments: const `DOUBLE_CHEST_SLOTS`
- [ ] Do comments: const `SHULKER_BOX_SLOTS`
- [ ] Do comments: const `HOTBAR_SLOT_0`
- [ ] Do comments: const `TRADE_TIMEOUT_MS`
- [ ] Do comments: const `CHEST_OP_TIMEOUT_SECS`
- [ ] Do comments: const `PATHFINDING_TIMEOUT_MS`
- [ ] Do comments: const `DELAY_SHORT_MS`
- [ ] Do comments: const `PATHFINDING_CHECK_INTERVAL_MS`
- [ ] Do comments: const `DELAY_MEDIUM_MS`
- [ ] Do comments: const `DELAY_INTERACT_MS`
- [ ] Do comments: const `DELAY_BLOCK_OP_MS`
- [ ] Do comments: const `DELAY_LOOK_AT_MS`
- [ ] Do comments: const `DELAY_SETTLE_MS`
- [ ] Do comments: const `DELAY_VALIDATION_BETWEEN_CHESTS_MS`
- [ ] Do comments: const `DELAY_SHULKER_PLACE_MS`
- [ ] Do comments: const `DELAY_DISCONNECT_MS`
- [ ] Do comments: const `DELAY_CONFIG_DEBOUNCE_MS`
- [ ] Do comments: const `CHEST_OP_MAX_RETRIES`
- [ ] Do comments: const `CHUNK_RELOAD_EXTRA_RETRIES`
- [ ] Do comments: const `CHUNK_RELOAD_BASE_DELAY_MS`
- [ ] Do comments: const `CHUNK_RELOAD_MAX_DELAY_MS`
- [ ] Do comments: const `SHULKER_OP_MAX_RETRIES`
- [ ] Do comments: const `NAVIGATION_MAX_RETRIES`
- [ ] Do comments: const `RETRY_BASE_DELAY_MS`
- [ ] Do comments: const `RETRY_MAX_DELAY_MS`
- [ ] Do comments: const `FEE_MIN`
- [ ] Do comments: const `FEE_MAX`
- [ ] Do comments: const `MAX_TRANSACTION_QUANTITY`
- [ ] Do comments: const `MIN_RESERVE_FOR_PRICE`
- [ ] Do comments: const `CHESTS_PER_NODE`
- [ ] Do comments: const `NODE_SPACING`
- [ ] Do comments: const `OVERFLOW_CHEST_ITEM`
- [ ] Do comments: const `DIAMOND_CHEST_ID`
- [ ] Do comments: const `OVERFLOW_CHEST_ID`
- [ ] Do comments: const `MAX_ORDERS_PER_USER`
- [ ] Do comments: const `MAX_QUEUE_SIZE`
- [ ] Do comments: const `QUEUE_FILE`
- [ ] Do comments: const `RATE_LIMIT_BASE_COOLDOWN_MS`
- [ ] Do comments: const `UUID_CACHE_TTL_SECS`
- [ ] Do comments: const `RATE_LIMIT_MAX_COOLDOWN_MS`
- [ ] Do comments: const `RATE_LIMIT_RESET_AFTER_MS`
- [ ] Do comments: const `CLEANUP_INTERVAL_SECS`
- [ ] Do comments: const `RATE_LIMIT_STALE_AFTER_SECS`
- [ ] Do comments: fn `exponential_backoff_delay`

- [ ] Do testing: const `DOUBLE_CHEST_SLOTS`
- [ ] Do testing: const `SHULKER_BOX_SLOTS`
- [ ] Do testing: const `HOTBAR_SLOT_0`
- [ ] Do testing: const `TRADE_TIMEOUT_MS`
- [ ] Do testing: const `CHEST_OP_TIMEOUT_SECS`
- [ ] Do testing: const `PATHFINDING_TIMEOUT_MS`
- [ ] Do testing: const `DELAY_SHORT_MS`
- [ ] Do testing: const `PATHFINDING_CHECK_INTERVAL_MS`
- [ ] Do testing: const `DELAY_MEDIUM_MS`
- [ ] Do testing: const `DELAY_INTERACT_MS`
- [ ] Do testing: const `DELAY_BLOCK_OP_MS`
- [ ] Do testing: const `DELAY_LOOK_AT_MS`
- [ ] Do testing: const `DELAY_SETTLE_MS`
- [ ] Do testing: const `DELAY_VALIDATION_BETWEEN_CHESTS_MS`
- [ ] Do testing: const `DELAY_SHULKER_PLACE_MS`
- [ ] Do testing: const `DELAY_DISCONNECT_MS`
- [ ] Do testing: const `DELAY_CONFIG_DEBOUNCE_MS`
- [ ] Do testing: const `CHEST_OP_MAX_RETRIES`
- [ ] Do testing: const `CHUNK_RELOAD_EXTRA_RETRIES`
- [ ] Do testing: const `CHUNK_RELOAD_BASE_DELAY_MS`
- [ ] Do testing: const `CHUNK_RELOAD_MAX_DELAY_MS`
- [ ] Do testing: const `SHULKER_OP_MAX_RETRIES`
- [ ] Do testing: const `NAVIGATION_MAX_RETRIES`
- [ ] Do testing: const `RETRY_BASE_DELAY_MS`
- [ ] Do testing: const `RETRY_MAX_DELAY_MS`
- [ ] Do testing: const `FEE_MIN`
- [ ] Do testing: const `FEE_MAX`
- [ ] Do testing: const `MAX_TRANSACTION_QUANTITY`
- [ ] Do testing: const `MIN_RESERVE_FOR_PRICE`
- [ ] Do testing: const `CHESTS_PER_NODE`
- [ ] Do testing: const `NODE_SPACING`
- [ ] Do testing: const `OVERFLOW_CHEST_ITEM`
- [ ] Do testing: const `DIAMOND_CHEST_ID`
- [ ] Do testing: const `OVERFLOW_CHEST_ID`
- [ ] Do testing: const `MAX_ORDERS_PER_USER`
- [ ] Do testing: const `MAX_QUEUE_SIZE`
- [ ] Do testing: const `QUEUE_FILE`
- [ ] Do testing: const `RATE_LIMIT_BASE_COOLDOWN_MS`
- [ ] Do testing: const `UUID_CACHE_TTL_SECS`
- [ ] Do testing: const `RATE_LIMIT_MAX_COOLDOWN_MS`
- [ ] Do testing: const `RATE_LIMIT_RESET_AFTER_MS`
- [ ] Do testing: const `CLEANUP_INTERVAL_SECS`
- [ ] Do testing: const `RATE_LIMIT_STALE_AFTER_SECS`
- [ ] Do testing: fn `exponential_backoff_delay`

- [ ] Do logging: const `DOUBLE_CHEST_SLOTS`
- [ ] Do logging: const `SHULKER_BOX_SLOTS`
- [ ] Do logging: const `HOTBAR_SLOT_0`
- [ ] Do logging: const `TRADE_TIMEOUT_MS`
- [ ] Do logging: const `CHEST_OP_TIMEOUT_SECS`
- [ ] Do logging: const `PATHFINDING_TIMEOUT_MS`
- [ ] Do logging: const `DELAY_SHORT_MS`
- [ ] Do logging: const `PATHFINDING_CHECK_INTERVAL_MS`
- [ ] Do logging: const `DELAY_MEDIUM_MS`
- [ ] Do logging: const `DELAY_INTERACT_MS`
- [ ] Do logging: const `DELAY_BLOCK_OP_MS`
- [ ] Do logging: const `DELAY_LOOK_AT_MS`
- [ ] Do logging: const `DELAY_SETTLE_MS`
- [ ] Do logging: const `DELAY_VALIDATION_BETWEEN_CHESTS_MS`
- [ ] Do logging: const `DELAY_SHULKER_PLACE_MS`
- [ ] Do logging: const `DELAY_DISCONNECT_MS`
- [ ] Do logging: const `DELAY_CONFIG_DEBOUNCE_MS`
- [ ] Do logging: const `CHEST_OP_MAX_RETRIES`
- [ ] Do logging: const `CHUNK_RELOAD_EXTRA_RETRIES`
- [ ] Do logging: const `CHUNK_RELOAD_BASE_DELAY_MS`
- [ ] Do logging: const `CHUNK_RELOAD_MAX_DELAY_MS`
- [ ] Do logging: const `SHULKER_OP_MAX_RETRIES`
- [ ] Do logging: const `NAVIGATION_MAX_RETRIES`
- [ ] Do logging: const `RETRY_BASE_DELAY_MS`
- [ ] Do logging: const `RETRY_MAX_DELAY_MS`
- [ ] Do logging: const `FEE_MIN`
- [ ] Do logging: const `FEE_MAX`
- [ ] Do logging: const `MAX_TRANSACTION_QUANTITY`
- [ ] Do logging: const `MIN_RESERVE_FOR_PRICE`
- [ ] Do logging: const `CHESTS_PER_NODE`
- [ ] Do logging: const `NODE_SPACING`
- [ ] Do logging: const `OVERFLOW_CHEST_ITEM`
- [ ] Do logging: const `DIAMOND_CHEST_ID`
- [ ] Do logging: const `OVERFLOW_CHEST_ID`
- [ ] Do logging: const `MAX_ORDERS_PER_USER`
- [ ] Do logging: const `MAX_QUEUE_SIZE`
- [ ] Do logging: const `QUEUE_FILE`
- [ ] Do logging: const `RATE_LIMIT_BASE_COOLDOWN_MS`
- [ ] Do logging: const `UUID_CACHE_TTL_SECS`
- [ ] Do logging: const `RATE_LIMIT_MAX_COOLDOWN_MS`
- [ ] Do logging: const `RATE_LIMIT_RESET_AFTER_MS`
- [ ] Do logging: const `CLEANUP_INTERVAL_SECS`
- [ ] Do logging: const `RATE_LIMIT_STALE_AFTER_SECS`
- [ ] Do logging: fn `exponential_backoff_delay`

### src/error.rs

- enum `StoreError`
- impl `From<StoreError> for String` :: fn `from`
- impl `From<String> for StoreError` :: fn `from`

**TODO:**

- [ ] Do comments: enum `StoreError`
- [ ] Do comments: impl `From<StoreError> for String` :: fn `from`
- [ ] Do comments: impl `From<String> for StoreError` :: fn `from`

- [ ] Do testing: enum `StoreError`
- [ ] Do testing: impl `From<StoreError> for String` :: fn `from`
- [ ] Do testing: impl `From<String> for StoreError` :: fn `from`

- [ ] Do logging: enum `StoreError`
- [ ] Do logging: impl `From<StoreError> for String` :: fn `from`
- [ ] Do logging: impl `From<String> for StoreError` :: fn `from`

### src/fsutil.rs

- fn `write_atomic`

**TODO:**

- [ ] Do comments: fn `write_atomic`

- [ ] Do testing: fn `write_atomic`

- [ ] Do logging: fn `write_atomic`

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

- [ ] Do comments: struct `TradeItem`
- [ ] Do comments: struct `ChestSyncReport`
- [ ] Do comments: enum `QueuedOrderType`
- [ ] Do comments: enum `ChestAction`
- [ ] Do comments: enum `StoreMessage`
- [ ] Do comments: enum `BotMessage`
- [ ] Do comments: enum `CliMessage`
- [ ] Do comments: enum `BotInstruction`

- [ ] Do testing: struct `TradeItem`
- [ ] Do testing: struct `ChestSyncReport`
- [ ] Do testing: enum `QueuedOrderType`
- [ ] Do testing: enum `ChestAction`
- [ ] Do testing: enum `StoreMessage`
- [ ] Do testing: enum `BotMessage`
- [ ] Do testing: enum `CliMessage`
- [ ] Do testing: enum `BotInstruction`

- [ ] Do logging: struct `TradeItem`
- [ ] Do logging: struct `ChestSyncReport`
- [ ] Do logging: enum `QueuedOrderType`
- [ ] Do logging: enum `ChestAction`
- [ ] Do logging: enum `StoreMessage`
- [ ] Do logging: enum `BotMessage`
- [ ] Do logging: enum `CliMessage`
- [ ] Do logging: enum `BotInstruction`

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

- [ ] Do comments: pub mod `chest`
- [ ] Do comments: pub mod `item_id`
- [ ] Do comments: pub mod `node`
- [ ] Do comments: pub mod `order`
- [ ] Do comments: pub mod `pair`
- [ ] Do comments: pub mod `position`
- [ ] Do comments: pub mod `storage`
- [ ] Do comments: pub mod `trade`
- [ ] Do comments: pub mod `user`
- [ ] Do comments: re-export surface (`Chest`, `ItemId`, `Node`, `Order`, `Pair`, `Position`, `Storage`, `Trade`, `TradeType`, `User`)

- [ ] Do testing: pub mod `chest`
- [ ] Do testing: pub mod `item_id`
- [ ] Do testing: pub mod `node`
- [ ] Do testing: pub mod `order`
- [ ] Do testing: pub mod `pair`
- [ ] Do testing: pub mod `position`
- [ ] Do testing: pub mod `storage`
- [ ] Do testing: pub mod `trade`
- [ ] Do testing: pub mod `user`
- [ ] Do testing: re-export surface (`Chest`, `ItemId`, `Node`, `Order`, `Pair`, `Position`, `Storage`, `Trade`, `TradeType`, `User`)

- [ ] Do logging: pub mod `chest`
- [ ] Do logging: pub mod `item_id`
- [ ] Do logging: pub mod `node`
- [ ] Do logging: pub mod `order`
- [ ] Do logging: pub mod `pair`
- [ ] Do logging: pub mod `position`
- [ ] Do logging: pub mod `storage`
- [ ] Do logging: pub mod `trade`
- [ ] Do logging: pub mod `user`
- [ ] Do logging: re-export surface (`Chest`, `ItemId`, `Node`, `Order`, `Pair`, `Position`, `Storage`, `Trade`, `TradeType`, `User`)

---

## types/

### src/types/position.rs

- struct `Position`

**TODO:**

- [ ] Do comments: struct `Position`

- [ ] Do testing: struct `Position`

- [ ] Do logging: struct `Position`

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

- [ ] Do comments: struct `ItemId`
- [ ] Do comments: impl ItemId :: const `EMPTY`
- [ ] Do comments: impl ItemId :: fn `new`
- [ ] Do comments: impl ItemId :: fn `from_normalized`
- [ ] Do comments: impl ItemId :: fn `as_str`
- [ ] Do comments: impl ItemId :: fn `with_minecraft_prefix`
- [ ] Do comments: impl ItemId :: fn `is_empty`
- [ ] Do comments: impl `Deref for ItemId` :: fn `deref`
- [ ] Do comments: impl `Borrow<str> for ItemId` :: fn `borrow`
- [ ] Do comments: impl `AsRef<str> for ItemId` :: fn `as_ref`
- [ ] Do comments: impl `Display for ItemId` :: fn `fmt`
- [ ] Do comments: impl `PartialEq<str> for ItemId` :: fn `eq`
- [ ] Do comments: impl `PartialEq<&str> for ItemId` :: fn `eq`
- [ ] Do comments: impl `PartialEq<String> for ItemId` :: fn `eq`
- [ ] Do comments: impl `From<ItemId> for String` :: fn `from`
- [ ] Do comments: impl `Default for ItemId` :: fn `default`
- [ ] Do comments: tests module

- [ ] Do testing: struct `ItemId`
- [ ] Do testing: impl ItemId :: const `EMPTY`
- [ ] Do testing: impl ItemId :: fn `new`
- [ ] Do testing: impl ItemId :: fn `from_normalized`
- [ ] Do testing: impl ItemId :: fn `as_str`
- [ ] Do testing: impl ItemId :: fn `with_minecraft_prefix`
- [ ] Do testing: impl ItemId :: fn `is_empty`
- [ ] Do testing: impl `Deref for ItemId` :: fn `deref`
- [ ] Do testing: impl `Borrow<str> for ItemId` :: fn `borrow`
- [ ] Do testing: impl `AsRef<str> for ItemId` :: fn `as_ref`
- [ ] Do testing: impl `Display for ItemId` :: fn `fmt`
- [ ] Do testing: impl `PartialEq<str> for ItemId` :: fn `eq`
- [ ] Do testing: impl `PartialEq<&str> for ItemId` :: fn `eq`
- [ ] Do testing: impl `PartialEq<String> for ItemId` :: fn `eq`
- [ ] Do testing: impl `From<ItemId> for String` :: fn `from`
- [ ] Do testing: impl `Default for ItemId` :: fn `default`
- [ ] Do testing: tests module

- [ ] Do logging: struct `ItemId`
- [ ] Do logging: impl ItemId :: const `EMPTY`
- [ ] Do logging: impl ItemId :: fn `new`
- [ ] Do logging: impl ItemId :: fn `from_normalized`
- [ ] Do logging: impl ItemId :: fn `as_str`
- [ ] Do logging: impl ItemId :: fn `with_minecraft_prefix`
- [ ] Do logging: impl ItemId :: fn `is_empty`
- [ ] Do logging: impl `Deref for ItemId` :: fn `deref`
- [ ] Do logging: impl `Borrow<str> for ItemId` :: fn `borrow`
- [ ] Do logging: impl `AsRef<str> for ItemId` :: fn `as_ref`
- [ ] Do logging: impl `Display for ItemId` :: fn `fmt`
- [ ] Do logging: impl `PartialEq<str> for ItemId` :: fn `eq`
- [ ] Do logging: impl `PartialEq<&str> for ItemId` :: fn `eq`
- [ ] Do logging: impl `PartialEq<String> for ItemId` :: fn `eq`
- [ ] Do logging: impl `From<ItemId> for String` :: fn `from`
- [ ] Do logging: impl `Default for ItemId` :: fn `default`
- [ ] Do logging: tests module

### src/types/node.rs

- struct `Node`
- impl Node :: fn `new`
- impl Node :: fn `load`
- impl Node :: fn `save`
- impl Node :: fn `calc_position`
- impl Node :: fn `calc_chest_position`
- tests module

**TODO:**

- [ ] Do comments: struct `Node`
- [ ] Do comments: impl Node :: fn `new`
- [ ] Do comments: impl Node :: fn `load`
- [ ] Do comments: impl Node :: fn `save`
- [ ] Do comments: impl Node :: fn `calc_position`
- [ ] Do comments: impl Node :: fn `calc_chest_position`
- [ ] Do comments: tests module

- [ ] Do testing: struct `Node`
- [ ] Do testing: impl Node :: fn `new`
- [ ] Do testing: impl Node :: fn `load`
- [ ] Do testing: impl Node :: fn `save`
- [ ] Do testing: impl Node :: fn `calc_position`
- [ ] Do testing: impl Node :: fn `calc_chest_position`
- [ ] Do testing: tests module

- [ ] Do logging: struct `Node`
- [ ] Do logging: impl Node :: fn `new`
- [ ] Do logging: impl Node :: fn `load`
- [ ] Do logging: impl Node :: fn `save`
- [ ] Do logging: impl Node :: fn `calc_position`
- [ ] Do logging: impl Node :: fn `calc_chest_position`
- [ ] Do logging: tests module

### src/types/chest.rs

- struct `Chest`
- impl Chest :: fn `new`
- impl Chest :: fn `calc_position`

**TODO:**

- [ ] Do comments: struct `Chest`
- [ ] Do comments: impl Chest :: fn `new`
- [ ] Do comments: impl Chest :: fn `calc_position`

- [ ] Do testing: struct `Chest`
- [ ] Do testing: impl Chest :: fn `new`
- [ ] Do testing: impl Chest :: fn `calc_position`

- [ ] Do logging: struct `Chest`
- [ ] Do logging: impl Chest :: fn `new`
- [ ] Do logging: impl Chest :: fn `calc_position`

### src/types/trade.rs

- struct `Trade`
- enum `TradeType`
- impl Trade :: fn `new`
- impl Trade :: fn `save`
- impl Trade :: fn `load_all_with_limit`
- impl Trade :: fn `save_all`

**TODO:**

- [ ] Do comments: struct `Trade`
- [ ] Do comments: enum `TradeType`
- [ ] Do comments: impl Trade :: fn `new`
- [ ] Do comments: impl Trade :: fn `save`
- [ ] Do comments: impl Trade :: fn `load_all_with_limit`
- [ ] Do comments: impl Trade :: fn `save_all`

- [ ] Do testing: struct `Trade`
- [ ] Do testing: enum `TradeType`
- [ ] Do testing: impl Trade :: fn `new`
- [ ] Do testing: impl Trade :: fn `save`
- [ ] Do testing: impl Trade :: fn `load_all_with_limit`
- [ ] Do testing: impl Trade :: fn `save_all`

- [ ] Do logging: struct `Trade`
- [ ] Do logging: enum `TradeType`
- [ ] Do logging: impl Trade :: fn `new`
- [ ] Do logging: impl Trade :: fn `save`
- [ ] Do logging: impl Trade :: fn `load_all_with_limit`
- [ ] Do logging: impl Trade :: fn `save_all`

### src/types/order.rs

- struct `Order`
- enum `OrderType`
- impl Order :: fn `save_all_with_limit`

**TODO:**

- [ ] Do comments: struct `Order`
- [ ] Do comments: enum `OrderType`
- [ ] Do comments: impl Order :: fn `save_all_with_limit`

- [ ] Do testing: struct `Order`
- [ ] Do testing: enum `OrderType`
- [ ] Do testing: impl Order :: fn `save_all_with_limit`

- [ ] Do logging: struct `Order`
- [ ] Do logging: enum `OrderType`
- [ ] Do logging: impl Order :: fn `save_all_with_limit`

### src/types/pair.rs

- struct `Pair`
- impl Pair :: fn `shulker_capacity_for_stack_size`
- impl Pair :: fn `sanitize_item_name_for_filename`
- impl Pair :: fn `get_pair_file_path`
- impl Pair :: fn `save`
- impl Pair :: fn `load_all`
- impl Pair :: fn `save_all`

**TODO:**

- [ ] Do comments: struct `Pair`
- [ ] Do comments: impl Pair :: fn `shulker_capacity_for_stack_size`
- [ ] Do comments: impl Pair :: fn `sanitize_item_name_for_filename`
- [ ] Do comments: impl Pair :: fn `get_pair_file_path`
- [ ] Do comments: impl Pair :: fn `save`
- [ ] Do comments: impl Pair :: fn `load_all`
- [ ] Do comments: impl Pair :: fn `save_all`

- [ ] Do testing: struct `Pair`
- [ ] Do testing: impl Pair :: fn `shulker_capacity_for_stack_size`
- [ ] Do testing: impl Pair :: fn `sanitize_item_name_for_filename`
- [ ] Do testing: impl Pair :: fn `get_pair_file_path`
- [ ] Do testing: impl Pair :: fn `save`
- [ ] Do testing: impl Pair :: fn `load_all`
- [ ] Do testing: impl Pair :: fn `save_all`

- [ ] Do logging: struct `Pair`
- [ ] Do logging: impl Pair :: fn `shulker_capacity_for_stack_size`
- [ ] Do logging: impl Pair :: fn `sanitize_item_name_for_filename`
- [ ] Do logging: impl Pair :: fn `get_pair_file_path`
- [ ] Do logging: impl Pair :: fn `save`
- [ ] Do logging: impl Pair :: fn `load_all`
- [ ] Do logging: impl Pair :: fn `save_all`

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

- [ ] Do comments: static `HTTP_CLIENT`
- [ ] Do comments: struct `User`
- [ ] Do comments: struct `MojangResponse`
- [ ] Do comments: fn `get_http_client`
- [ ] Do comments: impl User :: async fn `get_uuid_async`
- [ ] Do comments: impl User :: fn `get_user_file_path`
- [ ] Do comments: impl User :: fn `save`
- [ ] Do comments: impl User :: fn `load_all`
- [ ] Do comments: impl User :: fn `save_all`

- [ ] Do testing: static `HTTP_CLIENT`
- [ ] Do testing: struct `User`
- [ ] Do testing: struct `MojangResponse`
- [ ] Do testing: fn `get_http_client`
- [ ] Do testing: impl User :: async fn `get_uuid_async`
- [ ] Do testing: impl User :: fn `get_user_file_path`
- [ ] Do testing: impl User :: fn `save`
- [ ] Do testing: impl User :: fn `load_all`
- [ ] Do testing: impl User :: fn `save_all`

- [ ] Do logging: static `HTTP_CLIENT`
- [ ] Do logging: struct `User`
- [ ] Do logging: struct `MojangResponse`
- [ ] Do logging: fn `get_http_client`
- [ ] Do logging: impl User :: async fn `get_uuid_async`
- [ ] Do logging: impl User :: fn `get_user_file_path`
- [ ] Do logging: impl User :: fn `save`
- [ ] Do logging: impl User :: fn `load_all`
- [ ] Do logging: impl User :: fn `save_all`

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

- [ ] Do comments: struct `ChestTransfer`
- [ ] Do comments: struct `Storage`
- [ ] Do comments: impl Storage :: const `SLOTS_PER_CHEST`
- [ ] Do comments: impl Storage :: const `DEFAULT_SHULKER_CAPACITY`
- [ ] Do comments: impl Storage :: fn `save`
- [ ] Do comments: impl Storage :: fn `new`
- [ ] Do comments: impl Storage :: fn `load`
- [ ] Do comments: impl Storage :: fn `add_node`
- [ ] Do comments: impl Storage :: fn `total_item_amount`
- [ ] Do comments: impl Storage :: fn `get_chest_mut`
- [ ] Do comments: impl Storage :: fn `withdraw_item`
- [ ] Do comments: impl Storage :: fn `deposit_item`
- [ ] Do comments: impl Storage :: fn `simulate_withdraw_plan`
- [ ] Do comments: impl Storage :: fn `simulate_deposit_plan`
- [ ] Do comments: impl Storage :: fn `withdraw_plan`
- [ ] Do comments: impl Storage :: fn `deposit_plan`
- [ ] Do comments: impl Storage :: fn `normalize_amounts_len`
- [ ] Do comments: impl Storage :: fn `deposit_into_chest`
- [ ] Do comments: impl Storage :: fn `find_empty_chest_index`
- [ ] Do comments: impl Storage :: fn `get_overflow_chest`
- [ ] Do comments: impl Storage :: fn `get_overflow_chest_mut`
- [ ] Do comments: impl Storage :: fn `get_overflow_chest_position`
- [ ] Do comments: impl Storage :: const fn `overflow_chest_id`
- [ ] Do comments: tests module

- [ ] Do testing: struct `ChestTransfer`
- [ ] Do testing: struct `Storage`
- [ ] Do testing: impl Storage :: const `SLOTS_PER_CHEST`
- [ ] Do testing: impl Storage :: const `DEFAULT_SHULKER_CAPACITY`
- [ ] Do testing: impl Storage :: fn `save`
- [ ] Do testing: impl Storage :: fn `new`
- [ ] Do testing: impl Storage :: fn `load`
- [ ] Do testing: impl Storage :: fn `add_node`
- [ ] Do testing: impl Storage :: fn `total_item_amount`
- [ ] Do testing: impl Storage :: fn `get_chest_mut`
- [ ] Do testing: impl Storage :: fn `withdraw_item`
- [ ] Do testing: impl Storage :: fn `deposit_item`
- [ ] Do testing: impl Storage :: fn `simulate_withdraw_plan`
- [ ] Do testing: impl Storage :: fn `simulate_deposit_plan`
- [ ] Do testing: impl Storage :: fn `withdraw_plan`
- [ ] Do testing: impl Storage :: fn `deposit_plan`
- [ ] Do testing: impl Storage :: fn `normalize_amounts_len`
- [ ] Do testing: impl Storage :: fn `deposit_into_chest`
- [ ] Do testing: impl Storage :: fn `find_empty_chest_index`
- [ ] Do testing: impl Storage :: fn `get_overflow_chest`
- [ ] Do testing: impl Storage :: fn `get_overflow_chest_mut`
- [ ] Do testing: impl Storage :: fn `get_overflow_chest_position`
- [ ] Do testing: impl Storage :: const fn `overflow_chest_id`
- [ ] Do testing: tests module

- [ ] Do logging: struct `ChestTransfer`
- [ ] Do logging: struct `Storage`
- [ ] Do logging: impl Storage :: const `SLOTS_PER_CHEST`
- [ ] Do logging: impl Storage :: const `DEFAULT_SHULKER_CAPACITY`
- [ ] Do logging: impl Storage :: fn `save`
- [ ] Do logging: impl Storage :: fn `new`
- [ ] Do logging: impl Storage :: fn `load`
- [ ] Do logging: impl Storage :: fn `add_node`
- [ ] Do logging: impl Storage :: fn `total_item_amount`
- [ ] Do logging: impl Storage :: fn `get_chest_mut`
- [ ] Do logging: impl Storage :: fn `withdraw_item`
- [ ] Do logging: impl Storage :: fn `deposit_item`
- [ ] Do logging: impl Storage :: fn `simulate_withdraw_plan`
- [ ] Do logging: impl Storage :: fn `simulate_deposit_plan`
- [ ] Do logging: impl Storage :: fn `withdraw_plan`
- [ ] Do logging: impl Storage :: fn `deposit_plan`
- [ ] Do logging: impl Storage :: fn `normalize_amounts_len`
- [ ] Do logging: impl Storage :: fn `deposit_into_chest`
- [ ] Do logging: impl Storage :: fn `find_empty_chest_index`
- [ ] Do logging: impl Storage :: fn `get_overflow_chest`
- [ ] Do logging: impl Storage :: fn `get_overflow_chest_mut`
- [ ] Do logging: impl Storage :: fn `get_overflow_chest_position`
- [ ] Do logging: impl Storage :: const fn `overflow_chest_id`
- [ ] Do logging: tests module

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

- [ ] Do comments: pub mod declarations (`connection`, `navigation`, `trade`, `chest_io`, `shulker`, `inventory`)
- [ ] Do comments: struct `BotState`
- [ ] Do comments: struct `Bot`
- [ ] Do comments: impl `Default for BotState` :: fn `default`
- [ ] Do comments: impl Bot :: async fn `new`
- [ ] Do comments: impl Bot :: async fn `send_chat_message`
- [ ] Do comments: impl Bot :: async fn `send_whisper`
- [ ] Do comments: impl Bot :: fn `normalize_item_id`
- [ ] Do comments: impl Bot :: fn `chat_subscribe`
- [ ] Do comments: async fn `bot_task`
- [ ] Do comments: async fn `validate_node_physically`
- [ ] Do comments: fn `handle_event_fn`
- [ ] Do comments: async fn `handle_event`
- [ ] Do comments: async fn `handle_chat_message`

- [ ] Do testing: pub mod declarations (`connection`, `navigation`, `trade`, `chest_io`, `shulker`, `inventory`)
- [ ] Do testing: struct `BotState`
- [ ] Do testing: struct `Bot`
- [ ] Do testing: impl `Default for BotState` :: fn `default`
- [ ] Do testing: impl Bot :: async fn `new`
- [ ] Do testing: impl Bot :: async fn `send_chat_message`
- [ ] Do testing: impl Bot :: async fn `send_whisper`
- [ ] Do testing: impl Bot :: fn `normalize_item_id`
- [ ] Do testing: impl Bot :: fn `chat_subscribe`
- [ ] Do testing: async fn `bot_task`
- [ ] Do testing: async fn `validate_node_physically`
- [ ] Do testing: fn `handle_event_fn`
- [ ] Do testing: async fn `handle_event`
- [ ] Do testing: async fn `handle_chat_message`

- [ ] Do logging: pub mod declarations (`connection`, `navigation`, `trade`, `chest_io`, `shulker`, `inventory`)
- [ ] Do logging: struct `BotState`
- [ ] Do logging: struct `Bot`
- [ ] Do logging: impl `Default for BotState` :: fn `default`
- [ ] Do logging: impl Bot :: async fn `new`
- [ ] Do logging: impl Bot :: async fn `send_chat_message`
- [ ] Do logging: impl Bot :: async fn `send_whisper`
- [ ] Do logging: impl Bot :: fn `normalize_item_id`
- [ ] Do logging: impl Bot :: fn `chat_subscribe`
- [ ] Do logging: async fn `bot_task`
- [ ] Do logging: async fn `validate_node_physically`
- [ ] Do logging: fn `handle_event_fn`
- [ ] Do logging: async fn `handle_event`
- [ ] Do logging: async fn `handle_chat_message`

### src/bot/connection.rs

- async fn `connect`
- async fn `disconnect`

**TODO:**

- [ ] Do comments: async fn `connect`
- [ ] Do comments: async fn `disconnect`

- [ ] Do testing: async fn `connect`
- [ ] Do testing: async fn `disconnect`

- [ ] Do logging: async fn `connect`
- [ ] Do logging: async fn `disconnect`

### src/bot/navigation.rs

- async fn `navigate_to_position_once`
- async fn `navigate_to_position`
- async fn `go_to_node`
- async fn `go_to_chest`

**TODO:**

- [ ] Do comments: async fn `navigate_to_position_once`
- [ ] Do comments: async fn `navigate_to_position`
- [ ] Do comments: async fn `go_to_node`
- [ ] Do comments: async fn `go_to_chest`

- [ ] Do testing: async fn `navigate_to_position_once`
- [ ] Do testing: async fn `navigate_to_position`
- [ ] Do testing: async fn `go_to_node`
- [ ] Do testing: async fn `go_to_chest`

- [ ] Do logging: async fn `navigate_to_position_once`
- [ ] Do logging: async fn `navigate_to_position`
- [ ] Do logging: async fn `go_to_node`
- [ ] Do logging: async fn `go_to_chest`

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

- [ ] Do comments: async fn `ensure_inventory_empty`
- [ ] Do comments: async fn `move_hotbar_to_inventory`
- [ ] Do comments: async fn `quick_move_from_container`
- [ ] Do comments: fn `verify_holding_shulker`
- [ ] Do comments: fn `is_entity_ready`
- [ ] Do comments: async fn `wait_for_entity_ready`
- [ ] Do comments: fn `carried_item`
- [ ] Do comments: async fn `ensure_shulker_in_hotbar_slot_0`
- [ ] Do comments: async fn `recover_shulker_to_slot_0`

- [ ] Do testing: async fn `ensure_inventory_empty`
- [ ] Do testing: async fn `move_hotbar_to_inventory`
- [ ] Do testing: async fn `quick_move_from_container`
- [ ] Do testing: fn `verify_holding_shulker`
- [ ] Do testing: fn `is_entity_ready`
- [ ] Do testing: async fn `wait_for_entity_ready`
- [ ] Do testing: fn `carried_item`
- [ ] Do testing: async fn `ensure_shulker_in_hotbar_slot_0`
- [ ] Do testing: async fn `recover_shulker_to_slot_0`

- [ ] Do logging: async fn `ensure_inventory_empty`
- [ ] Do logging: async fn `move_hotbar_to_inventory`
- [ ] Do logging: async fn `quick_move_from_container`
- [ ] Do logging: fn `verify_holding_shulker`
- [ ] Do logging: fn `is_entity_ready`
- [ ] Do logging: async fn `wait_for_entity_ready`
- [ ] Do logging: fn `carried_item`
- [ ] Do logging: async fn `ensure_shulker_in_hotbar_slot_0`
- [ ] Do logging: async fn `recover_shulker_to_slot_0`

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

- [ ] Do comments: const `CHUNK_NOT_LOADED_PREFIX`
- [ ] Do comments: fn `find_shulker_in_inventory_view`
- [ ] Do comments: async fn `place_shulker_in_chest_slot_verified`
- [ ] Do comments: async fn `open_chest_container_once`
- [ ] Do comments: async fn `open_chest_container_for_validation`
- [ ] Do comments: async fn `open_chest_container`
- [ ] Do comments: async fn `transfer_items_with_shulker`
- [ ] Do comments: async fn `transfer_withdraw_from_shulker`
- [ ] Do comments: async fn `transfer_deposit_into_shulker`
- [ ] Do comments: async fn `prepare_for_chest_io`
- [ ] Do comments: async fn `automated_chest_io`
- [ ] Do comments: async fn `withdraw_shulkers`
- [ ] Do comments: async fn `deposit_shulkers`

- [ ] Do testing: const `CHUNK_NOT_LOADED_PREFIX`
- [ ] Do testing: fn `find_shulker_in_inventory_view`
- [ ] Do testing: async fn `place_shulker_in_chest_slot_verified`
- [ ] Do testing: async fn `open_chest_container_once`
- [ ] Do testing: async fn `open_chest_container_for_validation`
- [ ] Do testing: async fn `open_chest_container`
- [ ] Do testing: async fn `transfer_items_with_shulker`
- [ ] Do testing: async fn `transfer_withdraw_from_shulker`
- [ ] Do testing: async fn `transfer_deposit_into_shulker`
- [ ] Do testing: async fn `prepare_for_chest_io`
- [ ] Do testing: async fn `automated_chest_io`
- [ ] Do testing: async fn `withdraw_shulkers`
- [ ] Do testing: async fn `deposit_shulkers`

- [ ] Do logging: const `CHUNK_NOT_LOADED_PREFIX`
- [ ] Do logging: fn `find_shulker_in_inventory_view`
- [ ] Do logging: async fn `place_shulker_in_chest_slot_verified`
- [ ] Do logging: async fn `open_chest_container_once`
- [ ] Do logging: async fn `open_chest_container_for_validation`
- [ ] Do logging: async fn `open_chest_container`
- [ ] Do logging: async fn `transfer_items_with_shulker`
- [ ] Do logging: async fn `transfer_withdraw_from_shulker`
- [ ] Do logging: async fn `transfer_deposit_into_shulker`
- [ ] Do logging: async fn `prepare_for_chest_io`
- [ ] Do logging: async fn `automated_chest_io`
- [ ] Do logging: async fn `withdraw_shulkers`
- [ ] Do logging: async fn `deposit_shulkers`

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

- [ ] Do comments: const `SHULKER_BOX_IDS`
- [ ] Do comments: fn `shulker_station_position`
- [ ] Do comments: fn `is_shulker_box`
- [ ] Do comments: fn `validate_chest_slot_is_shulker` (cfg(test))
- [ ] Do comments: async fn `pickup_shulker_from_station`
- [ ] Do comments: async fn `open_shulker_at_station_once`
- [ ] Do comments: async fn `open_shulker_at_station`
- [ ] Do comments: test `test_is_shulker_box`
- [ ] Do comments: test `test_validate_chest_slot_is_shulker`
- [ ] Do comments: test `test_shulker_station_position`

- [ ] Do testing: const `SHULKER_BOX_IDS`
- [ ] Do testing: fn `shulker_station_position`
- [ ] Do testing: fn `is_shulker_box`
- [ ] Do testing: fn `validate_chest_slot_is_shulker` (cfg(test))
- [ ] Do testing: async fn `pickup_shulker_from_station`
- [ ] Do testing: async fn `open_shulker_at_station_once`
- [ ] Do testing: async fn `open_shulker_at_station`
- [ ] Do testing: test `test_is_shulker_box`
- [ ] Do testing: test `test_validate_chest_slot_is_shulker`
- [ ] Do testing: test `test_shulker_station_position`

- [ ] Do logging: const `SHULKER_BOX_IDS`
- [ ] Do logging: fn `shulker_station_position`
- [ ] Do logging: fn `is_shulker_box`
- [ ] Do logging: fn `validate_chest_slot_is_shulker` (cfg(test))
- [ ] Do logging: async fn `pickup_shulker_from_station`
- [ ] Do logging: async fn `open_shulker_at_station_once`
- [ ] Do logging: async fn `open_shulker_at_station`
- [ ] Do logging: test `test_is_shulker_box`
- [ ] Do logging: test `test_validate_chest_slot_is_shulker`
- [ ] Do logging: test `test_shulker_station_position`

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

- [ ] Do comments: fn `trade_bot_offer_slots`
- [ ] Do comments: fn `trade_player_offer_slots`
- [ ] Do comments: fn `trade_player_status_slots`
- [ ] Do comments: fn `trade_accept_slots`
- [ ] Do comments: fn `trade_cancel_slots`
- [ ] Do comments: async fn `wait_for_trade_menu_or_failure`
- [ ] Do comments: async fn `place_items_from_inventory_into_trade`
- [ ] Do comments: fn `validate_player_items`
- [ ] Do comments: async fn `execute_trade_with_player`
- [ ] Do comments: test `test_trade_bot_offer_slots`
- [ ] Do comments: test `test_trade_player_offer_slots`
- [ ] Do comments: test `test_trade_player_status_slots`
- [ ] Do comments: test `test_trade_accept_slots`
- [ ] Do comments: test `test_trade_cancel_slots`
- [ ] Do comments: test `test_trade_slot_sets_disjoint`

- [ ] Do testing: fn `trade_bot_offer_slots`
- [ ] Do testing: fn `trade_player_offer_slots`
- [ ] Do testing: fn `trade_player_status_slots`
- [ ] Do testing: fn `trade_accept_slots`
- [ ] Do testing: fn `trade_cancel_slots`
- [ ] Do testing: async fn `wait_for_trade_menu_or_failure`
- [ ] Do testing: async fn `place_items_from_inventory_into_trade`
- [ ] Do testing: fn `validate_player_items`
- [ ] Do testing: async fn `execute_trade_with_player`
- [ ] Do testing: test `test_trade_bot_offer_slots`
- [ ] Do testing: test `test_trade_player_offer_slots`
- [ ] Do testing: test `test_trade_player_status_slots`
- [ ] Do testing: test `test_trade_accept_slots`
- [ ] Do testing: test `test_trade_cancel_slots`
- [ ] Do testing: test `test_trade_slot_sets_disjoint`

- [ ] Do logging: fn `trade_bot_offer_slots`
- [ ] Do logging: fn `trade_player_offer_slots`
- [ ] Do logging: fn `trade_player_status_slots`
- [ ] Do logging: fn `trade_accept_slots`
- [ ] Do logging: fn `trade_cancel_slots`
- [ ] Do logging: async fn `wait_for_trade_menu_or_failure`
- [ ] Do logging: async fn `place_items_from_inventory_into_trade`
- [ ] Do logging: fn `validate_player_items`
- [ ] Do logging: async fn `execute_trade_with_player`
- [ ] Do logging: test `test_trade_bot_offer_slots`
- [ ] Do logging: test `test_trade_player_offer_slots`
- [ ] Do logging: test `test_trade_player_status_slots`
- [ ] Do logging: test `test_trade_accept_slots`
- [ ] Do logging: test `test_trade_cancel_slots`
- [ ] Do logging: test `test_trade_slot_sets_disjoint`

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

- [ ] Do comments: pub mod declarations (`command`, `handlers`, `journal`, `orders`, `pricing`, `queue`, `rate_limit`, `rollback`, `state`, `trade_state`, `utils`)
- [ ] Do comments: struct `Store`
- [ ] Do comments: impl Store :: async fn `new`
- [ ] Do comments: impl Store :: async fn `run`
- [ ] Do comments: impl Store :: async fn `process_next_order`
- [ ] Do comments: impl Store :: fn `reload_config`
- [ ] Do comments: impl Store :: fn `advance_trade`
- [ ] Do comments: impl Store :: async fn `handle_bot_message`
- [ ] Do comments: impl Store :: fn `expect_pair`
- [ ] Do comments: impl Store :: fn `expect_pair_mut`
- [ ] Do comments: impl Store :: fn `expect_user`
- [ ] Do comments: impl Store :: fn `expect_user_mut`
- [ ] Do comments: impl Store :: fn `apply_chest_sync`
- [ ] Do comments: impl Store :: fn `get_node_position`
- [ ] Do comments: impl Store :: fn `new_for_test`

- [ ] Do testing: pub mod declarations (`command`, `handlers`, `journal`, `orders`, `pricing`, `queue`, `rate_limit`, `rollback`, `state`, `trade_state`, `utils`)
- [ ] Do testing: struct `Store`
- [ ] Do testing: impl Store :: async fn `new`
- [ ] Do testing: impl Store :: async fn `run`
- [ ] Do testing: impl Store :: async fn `process_next_order`
- [ ] Do testing: impl Store :: fn `reload_config`
- [ ] Do testing: impl Store :: fn `advance_trade`
- [ ] Do testing: impl Store :: async fn `handle_bot_message`
- [ ] Do testing: impl Store :: fn `expect_pair`
- [ ] Do testing: impl Store :: fn `expect_pair_mut`
- [ ] Do testing: impl Store :: fn `expect_user`
- [ ] Do testing: impl Store :: fn `expect_user_mut`
- [ ] Do testing: impl Store :: fn `apply_chest_sync`
- [ ] Do testing: impl Store :: fn `get_node_position`
- [ ] Do testing: impl Store :: fn `new_for_test`

- [ ] Do logging: pub mod declarations (`command`, `handlers`, `journal`, `orders`, `pricing`, `queue`, `rate_limit`, `rollback`, `state`, `trade_state`, `utils`)
- [ ] Do logging: struct `Store`
- [ ] Do logging: impl Store :: async fn `new`
- [ ] Do logging: impl Store :: async fn `run`
- [ ] Do logging: impl Store :: async fn `process_next_order`
- [ ] Do logging: impl Store :: fn `reload_config`
- [ ] Do logging: impl Store :: fn `advance_trade`
- [ ] Do logging: impl Store :: async fn `handle_bot_message`
- [ ] Do logging: impl Store :: fn `expect_pair`
- [ ] Do logging: impl Store :: fn `expect_pair_mut`
- [ ] Do logging: impl Store :: fn `expect_user`
- [ ] Do logging: impl Store :: fn `expect_user_mut`
- [ ] Do logging: impl Store :: fn `apply_chest_sync`
- [ ] Do logging: impl Store :: fn `get_node_position`
- [ ] Do logging: impl Store :: fn `new_for_test`

### src/store/state.rs

- fn `apply_chest_sync`
- fn `save`
- fn `audit_state`
- fn `assert_invariants`

**TODO:**

- [ ] Do comments: fn `apply_chest_sync`
- [ ] Do comments: fn `save`
- [ ] Do comments: fn `audit_state`
- [ ] Do comments: fn `assert_invariants`

- [ ] Do testing: fn `apply_chest_sync`
- [ ] Do testing: fn `save`
- [ ] Do testing: fn `audit_state`
- [ ] Do testing: fn `assert_invariants`

- [ ] Do logging: fn `apply_chest_sync`
- [ ] Do logging: fn `save`
- [ ] Do logging: fn `audit_state`
- [ ] Do logging: fn `assert_invariants`

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

- [ ] Do comments: enum `Command`
- [ ] Do comments: fn `parse_command`
- [ ] Do comments: fn `parse_item_quantity`
- [ ] Do comments: fn `parse_item_amount`
- [ ] Do comments: fn `parse_optional_amount`
- [ ] Do comments: fn `parse_price`
- [ ] Do comments: fn `parse_balance`
- [ ] Do comments: fn `parse_pay`
- [ ] Do comments: fn `parse_page`
- [ ] Do comments: fn `parse_cancel`
- [ ] Do comments: tests module

- [ ] Do testing: enum `Command`
- [ ] Do testing: fn `parse_command`
- [ ] Do testing: fn `parse_item_quantity`
- [ ] Do testing: fn `parse_item_amount`
- [ ] Do testing: fn `parse_optional_amount`
- [ ] Do testing: fn `parse_price`
- [ ] Do testing: fn `parse_balance`
- [ ] Do testing: fn `parse_pay`
- [ ] Do testing: fn `parse_page`
- [ ] Do testing: fn `parse_cancel`
- [ ] Do testing: tests module

- [ ] Do logging: enum `Command`
- [ ] Do logging: fn `parse_command`
- [ ] Do logging: fn `parse_item_quantity`
- [ ] Do logging: fn `parse_item_amount`
- [ ] Do logging: fn `parse_optional_amount`
- [ ] Do logging: fn `parse_price`
- [ ] Do logging: fn `parse_balance`
- [ ] Do logging: fn `parse_pay`
- [ ] Do logging: fn `parse_page`
- [ ] Do logging: fn `parse_cancel`
- [ ] Do logging: tests module

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

- [ ] Do comments: const `JOURNAL_FILE`
- [ ] Do comments: static `NEXT_OPERATION_ID`
- [ ] Do comments: type alias `SharedJournal`
- [ ] Do comments: struct `JournalEntry`
- [ ] Do comments: struct `Journal`
- [ ] Do comments: enum `JournalOp`
- [ ] Do comments: enum `JournalState`
- [ ] Do comments: impl `Default for Journal` :: fn `default`
- [ ] Do comments: impl Journal :: fn `load`
- [ ] Do comments: impl Journal :: fn `load_from`
- [ ] Do comments: impl Journal :: fn `clear_leftover`
- [ ] Do comments: impl Journal :: fn `begin`
- [ ] Do comments: impl Journal :: fn `advance`
- [ ] Do comments: impl Journal :: fn `complete`
- [ ] Do comments: impl Journal :: fn `current`
- [ ] Do comments: impl Journal :: fn `persist`
- [ ] Do comments: tests module

- [ ] Do testing: const `JOURNAL_FILE`
- [ ] Do testing: static `NEXT_OPERATION_ID`
- [ ] Do testing: type alias `SharedJournal`
- [ ] Do testing: struct `JournalEntry`
- [ ] Do testing: struct `Journal`
- [ ] Do testing: enum `JournalOp`
- [ ] Do testing: enum `JournalState`
- [ ] Do testing: impl `Default for Journal` :: fn `default`
- [ ] Do testing: impl Journal :: fn `load`
- [ ] Do testing: impl Journal :: fn `load_from`
- [ ] Do testing: impl Journal :: fn `clear_leftover`
- [ ] Do testing: impl Journal :: fn `begin`
- [ ] Do testing: impl Journal :: fn `advance`
- [ ] Do testing: impl Journal :: fn `complete`
- [ ] Do testing: impl Journal :: fn `current`
- [ ] Do testing: impl Journal :: fn `persist`
- [ ] Do testing: tests module

- [ ] Do logging: const `JOURNAL_FILE`
- [ ] Do logging: static `NEXT_OPERATION_ID`
- [ ] Do logging: type alias `SharedJournal`
- [ ] Do logging: struct `JournalEntry`
- [ ] Do logging: struct `Journal`
- [ ] Do logging: enum `JournalOp`
- [ ] Do logging: enum `JournalState`
- [ ] Do logging: impl `Default for Journal` :: fn `default`
- [ ] Do logging: impl Journal :: fn `load`
- [ ] Do logging: impl Journal :: fn `load_from`
- [ ] Do logging: impl Journal :: fn `clear_leftover`
- [ ] Do logging: impl Journal :: fn `begin`
- [ ] Do logging: impl Journal :: fn `advance`
- [ ] Do logging: impl Journal :: fn `complete`
- [ ] Do logging: impl Journal :: fn `current`
- [ ] Do logging: impl Journal :: fn `persist`
- [ ] Do logging: tests module

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

- [ ] Do comments: struct `BuyPlan`
- [ ] Do comments: struct `SellPlan`
- [ ] Do comments: enum `ChestDirection`
- [ ] Do comments: async fn `execute_chest_transfers`
- [ ] Do comments: async fn `perform_trade`
- [ ] Do comments: async fn `validate_and_plan_buy`
- [ ] Do comments: async fn `handle_buy_order`
- [ ] Do comments: async fn `validate_and_plan_sell`
- [ ] Do comments: async fn `handle_sell_order`
- [ ] Do comments: async fn `execute_queued_order`
- [ ] Do comments: tests module

- [ ] Do testing: struct `BuyPlan`
- [ ] Do testing: struct `SellPlan`
- [ ] Do testing: enum `ChestDirection`
- [ ] Do testing: async fn `execute_chest_transfers`
- [ ] Do testing: async fn `perform_trade`
- [ ] Do testing: async fn `validate_and_plan_buy`
- [ ] Do testing: async fn `handle_buy_order`
- [ ] Do testing: async fn `validate_and_plan_sell`
- [ ] Do testing: async fn `handle_sell_order`
- [ ] Do testing: async fn `execute_queued_order`
- [ ] Do testing: tests module

- [ ] Do logging: struct `BuyPlan`
- [ ] Do logging: struct `SellPlan`
- [ ] Do logging: enum `ChestDirection`
- [ ] Do logging: async fn `execute_chest_transfers`
- [ ] Do logging: async fn `perform_trade`
- [ ] Do logging: async fn `validate_and_plan_buy`
- [ ] Do logging: async fn `handle_buy_order`
- [ ] Do logging: async fn `validate_and_plan_sell`
- [ ] Do logging: async fn `handle_sell_order`
- [ ] Do logging: async fn `execute_queued_order`
- [ ] Do logging: tests module

### src/store/pricing.rs

- fn `validate_fee`
- fn `reserves_sufficient`
- fn `calculate_buy_cost`
- fn `buy_cost_pure`
- fn `calculate_sell_payout`
- fn `sell_payout_pure`
- tests module (includes proptests)

**TODO:**

- [ ] Do comments: fn `validate_fee`
- [ ] Do comments: fn `reserves_sufficient`
- [ ] Do comments: fn `calculate_buy_cost`
- [ ] Do comments: fn `buy_cost_pure`
- [ ] Do comments: fn `calculate_sell_payout`
- [ ] Do comments: fn `sell_payout_pure`
- [ ] Do comments: tests module (includes proptests)

- [ ] Do testing: fn `validate_fee`
- [ ] Do testing: fn `reserves_sufficient`
- [ ] Do testing: fn `calculate_buy_cost`
- [ ] Do testing: fn `buy_cost_pure`
- [ ] Do testing: fn `calculate_sell_payout`
- [ ] Do testing: fn `sell_payout_pure`
- [ ] Do testing: tests module (includes proptests)

- [ ] Do logging: fn `validate_fee`
- [ ] Do logging: fn `reserves_sufficient`
- [ ] Do logging: fn `calculate_buy_cost`
- [ ] Do logging: fn `buy_cost_pure`
- [ ] Do logging: fn `calculate_sell_payout`
- [ ] Do logging: fn `sell_payout_pure`
- [ ] Do logging: tests module (includes proptests)

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

- [ ] Do comments: struct `QueuedOrder`
- [ ] Do comments: struct `OrderQueue`
- [ ] Do comments: struct `QueuePersist`
- [ ] Do comments: impl QueuedOrder :: fn `new`
- [ ] Do comments: impl QueuedOrder :: fn `description`
- [ ] Do comments: impl `Default for OrderQueue` :: fn `default`
- [ ] Do comments: impl OrderQueue :: fn `new`
- [ ] Do comments: impl OrderQueue :: fn `load`
- [ ] Do comments: impl OrderQueue :: fn `save`
- [ ] Do comments: impl OrderQueue :: fn `add`
- [ ] Do comments: impl OrderQueue :: fn `pop`
- [ ] Do comments: impl OrderQueue :: fn `is_empty`
- [ ] Do comments: impl OrderQueue :: fn `len`
- [ ] Do comments: impl OrderQueue :: fn `get_position`
- [ ] Do comments: impl OrderQueue :: fn `get_user_position`
- [ ] Do comments: impl OrderQueue :: fn `user_order_count`
- [ ] Do comments: impl OrderQueue :: fn `get_user_orders`
- [ ] Do comments: impl OrderQueue :: fn `cancel`
- [ ] Do comments: impl OrderQueue :: fn `estimate_wait`
- [ ] Do comments: tests module

- [ ] Do testing: struct `QueuedOrder`
- [ ] Do testing: struct `OrderQueue`
- [ ] Do testing: struct `QueuePersist`
- [ ] Do testing: impl QueuedOrder :: fn `new`
- [ ] Do testing: impl QueuedOrder :: fn `description`
- [ ] Do testing: impl `Default for OrderQueue` :: fn `default`
- [ ] Do testing: impl OrderQueue :: fn `new`
- [ ] Do testing: impl OrderQueue :: fn `load`
- [ ] Do testing: impl OrderQueue :: fn `save`
- [ ] Do testing: impl OrderQueue :: fn `add`
- [ ] Do testing: impl OrderQueue :: fn `pop`
- [ ] Do testing: impl OrderQueue :: fn `is_empty`
- [ ] Do testing: impl OrderQueue :: fn `len`
- [ ] Do testing: impl OrderQueue :: fn `get_position`
- [ ] Do testing: impl OrderQueue :: fn `get_user_position`
- [ ] Do testing: impl OrderQueue :: fn `user_order_count`
- [ ] Do testing: impl OrderQueue :: fn `get_user_orders`
- [ ] Do testing: impl OrderQueue :: fn `cancel`
- [ ] Do testing: impl OrderQueue :: fn `estimate_wait`
- [ ] Do testing: tests module

- [ ] Do logging: struct `QueuedOrder`
- [ ] Do logging: struct `OrderQueue`
- [ ] Do logging: struct `QueuePersist`
- [ ] Do logging: impl QueuedOrder :: fn `new`
- [ ] Do logging: impl QueuedOrder :: fn `description`
- [ ] Do logging: impl `Default for OrderQueue` :: fn `default`
- [ ] Do logging: impl OrderQueue :: fn `new`
- [ ] Do logging: impl OrderQueue :: fn `load`
- [ ] Do logging: impl OrderQueue :: fn `save`
- [ ] Do logging: impl OrderQueue :: fn `add`
- [ ] Do logging: impl OrderQueue :: fn `pop`
- [ ] Do logging: impl OrderQueue :: fn `is_empty`
- [ ] Do logging: impl OrderQueue :: fn `len`
- [ ] Do logging: impl OrderQueue :: fn `get_position`
- [ ] Do logging: impl OrderQueue :: fn `get_user_position`
- [ ] Do logging: impl OrderQueue :: fn `user_order_count`
- [ ] Do logging: impl OrderQueue :: fn `get_user_orders`
- [ ] Do logging: impl OrderQueue :: fn `cancel`
- [ ] Do logging: impl OrderQueue :: fn `estimate_wait`
- [ ] Do logging: tests module

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

- [ ] Do comments: struct `UserRateLimit`
- [ ] Do comments: struct `RateLimiter`
- [ ] Do comments: fn `calculate_cooldown`
- [ ] Do comments: impl UserRateLimit :: fn `new`
- [ ] Do comments: impl `Default for RateLimiter` :: fn `default`
- [ ] Do comments: impl RateLimiter :: fn `new`
- [ ] Do comments: impl RateLimiter :: fn `check`
- [ ] Do comments: impl RateLimiter :: fn `cleanup_stale`
- [ ] Do comments: tests module

- [ ] Do testing: struct `UserRateLimit`
- [ ] Do testing: struct `RateLimiter`
- [ ] Do testing: fn `calculate_cooldown`
- [ ] Do testing: impl UserRateLimit :: fn `new`
- [ ] Do testing: impl `Default for RateLimiter` :: fn `default`
- [ ] Do testing: impl RateLimiter :: fn `new`
- [ ] Do testing: impl RateLimiter :: fn `check`
- [ ] Do testing: impl RateLimiter :: fn `cleanup_stale`
- [ ] Do testing: tests module

- [ ] Do logging: struct `UserRateLimit`
- [ ] Do logging: struct `RateLimiter`
- [ ] Do logging: fn `calculate_cooldown`
- [ ] Do logging: impl UserRateLimit :: fn `new`
- [ ] Do logging: impl `Default for RateLimiter` :: fn `default`
- [ ] Do logging: impl RateLimiter :: fn `new`
- [ ] Do logging: impl RateLimiter :: fn `check`
- [ ] Do logging: impl RateLimiter :: fn `cleanup_stale`
- [ ] Do logging: tests module

### src/store/rollback.rs

- struct `RollbackResult`
- impl RollbackResult :: fn `has_failures`
- fn `chest_from_transfer`
- async fn `deposit_transfers`
- async fn `rollback_amount_to_storage`

**TODO:**

- [ ] Do comments: struct `RollbackResult`
- [ ] Do comments: impl RollbackResult :: fn `has_failures`
- [ ] Do comments: fn `chest_from_transfer`
- [ ] Do comments: async fn `deposit_transfers`
- [ ] Do comments: async fn `rollback_amount_to_storage`

- [ ] Do testing: struct `RollbackResult`
- [ ] Do testing: impl RollbackResult :: fn `has_failures`
- [ ] Do testing: fn `chest_from_transfer`
- [ ] Do testing: async fn `deposit_transfers`
- [ ] Do testing: async fn `rollback_amount_to_storage`

- [ ] Do logging: struct `RollbackResult`
- [ ] Do logging: impl RollbackResult :: fn `has_failures`
- [ ] Do logging: fn `chest_from_transfer`
- [ ] Do logging: async fn `deposit_transfers`
- [ ] Do logging: async fn `rollback_amount_to_storage`

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

- [ ] Do comments: const `TRADE_STATE_FILE`
- [ ] Do comments: struct `TradeResult`
- [ ] Do comments: struct `CompletedTrade`
- [ ] Do comments: enum `TradeState`
- [ ] Do comments: impl TradeState :: fn `new`
- [ ] Do comments: impl TradeState :: fn `begin_withdrawal`
- [ ] Do comments: impl TradeState :: fn `begin_trading`
- [ ] Do comments: impl TradeState :: fn `begin_depositing`
- [ ] Do comments: impl TradeState :: fn `commit`
- [ ] Do comments: impl TradeState :: fn `rollback`
- [ ] Do comments: impl TradeState :: fn `phase`
- [ ] Do comments: impl TradeState :: fn `is_terminal`
- [ ] Do comments: impl TradeState :: fn `order`
- [ ] Do comments: impl `fmt::Display for TradeState` :: fn `fmt`
- [ ] Do comments: fn `persist`
- [ ] Do comments: fn `load_persisted`
- [ ] Do comments: fn `clear_persisted`
- [ ] Do comments: tests module

- [ ] Do testing: const `TRADE_STATE_FILE`
- [ ] Do testing: struct `TradeResult`
- [ ] Do testing: struct `CompletedTrade`
- [ ] Do testing: enum `TradeState`
- [ ] Do testing: impl TradeState :: fn `new`
- [ ] Do testing: impl TradeState :: fn `begin_withdrawal`
- [ ] Do testing: impl TradeState :: fn `begin_trading`
- [ ] Do testing: impl TradeState :: fn `begin_depositing`
- [ ] Do testing: impl TradeState :: fn `commit`
- [ ] Do testing: impl TradeState :: fn `rollback`
- [ ] Do testing: impl TradeState :: fn `phase`
- [ ] Do testing: impl TradeState :: fn `is_terminal`
- [ ] Do testing: impl TradeState :: fn `order`
- [ ] Do testing: impl `fmt::Display for TradeState` :: fn `fmt`
- [ ] Do testing: fn `persist`
- [ ] Do testing: fn `load_persisted`
- [ ] Do testing: fn `clear_persisted`
- [ ] Do testing: tests module

- [ ] Do logging: const `TRADE_STATE_FILE`
- [ ] Do logging: struct `TradeResult`
- [ ] Do logging: struct `CompletedTrade`
- [ ] Do logging: enum `TradeState`
- [ ] Do logging: impl TradeState :: fn `new`
- [ ] Do logging: impl TradeState :: fn `begin_withdrawal`
- [ ] Do logging: impl TradeState :: fn `begin_trading`
- [ ] Do logging: impl TradeState :: fn `begin_depositing`
- [ ] Do logging: impl TradeState :: fn `commit`
- [ ] Do logging: impl TradeState :: fn `rollback`
- [ ] Do logging: impl TradeState :: fn `phase`
- [ ] Do logging: impl TradeState :: fn `is_terminal`
- [ ] Do logging: impl TradeState :: fn `order`
- [ ] Do logging: impl `fmt::Display for TradeState` :: fn `fmt`
- [ ] Do logging: fn `persist`
- [ ] Do logging: fn `load_persisted`
- [ ] Do logging: fn `clear_persisted`
- [ ] Do logging: tests module

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

- [ ] Do comments: static `UUID_CACHE`
- [ ] Do comments: type alias `UuidCache`
- [ ] Do comments: fn `uuid_cache`
- [ ] Do comments: fn `normalize_item_id`
- [ ] Do comments: async fn `resolve_user_uuid`
- [ ] Do comments: fn `clear_uuid_cache`
- [ ] Do comments: fn `cleanup_uuid_cache`
- [ ] Do comments: fn `ensure_user_exists`
- [ ] Do comments: fn `is_operator`
- [ ] Do comments: fn `get_node_position`
- [ ] Do comments: async fn `send_message_to_player`
- [ ] Do comments: fn `summarize_transfers`
- [ ] Do comments: fn `fmt_issues`
- [ ] Do comments: tests module

- [ ] Do testing: static `UUID_CACHE`
- [ ] Do testing: type alias `UuidCache`
- [ ] Do testing: fn `uuid_cache`
- [ ] Do testing: fn `normalize_item_id`
- [ ] Do testing: async fn `resolve_user_uuid`
- [ ] Do testing: fn `clear_uuid_cache`
- [ ] Do testing: fn `cleanup_uuid_cache`
- [ ] Do testing: fn `ensure_user_exists`
- [ ] Do testing: fn `is_operator`
- [ ] Do testing: fn `get_node_position`
- [ ] Do testing: async fn `send_message_to_player`
- [ ] Do testing: fn `summarize_transfers`
- [ ] Do testing: fn `fmt_issues`
- [ ] Do testing: tests module

- [ ] Do logging: static `UUID_CACHE`
- [ ] Do logging: type alias `UuidCache`
- [ ] Do logging: fn `uuid_cache`
- [ ] Do logging: fn `normalize_item_id`
- [ ] Do logging: async fn `resolve_user_uuid`
- [ ] Do logging: fn `clear_uuid_cache`
- [ ] Do logging: fn `cleanup_uuid_cache`
- [ ] Do logging: fn `ensure_user_exists`
- [ ] Do logging: fn `is_operator`
- [ ] Do logging: fn `get_node_position`
- [ ] Do logging: async fn `send_message_to_player`
- [ ] Do logging: fn `summarize_transfers`
- [ ] Do logging: fn `fmt_issues`
- [ ] Do logging: tests module

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

- [ ] Do comments: pub mod `player`
- [ ] Do comments: pub mod `operator`
- [ ] Do comments: pub mod `cli`
- [ ] Do comments: mod `buy`
- [ ] Do comments: mod `sell`
- [ ] Do comments: mod `deposit`
- [ ] Do comments: mod `withdraw`
- [ ] Do comments: mod `info`
- [ ] Do comments: pub(crate) mod `validation`

- [ ] Do testing: pub mod `player`
- [ ] Do testing: pub mod `operator`
- [ ] Do testing: pub mod `cli`
- [ ] Do testing: mod `buy`
- [ ] Do testing: mod `sell`
- [ ] Do testing: mod `deposit`
- [ ] Do testing: mod `withdraw`
- [ ] Do testing: mod `info`
- [ ] Do testing: pub(crate) mod `validation`

- [ ] Do logging: pub mod `player`
- [ ] Do logging: pub mod `operator`
- [ ] Do logging: pub mod `cli`
- [ ] Do logging: mod `buy`
- [ ] Do logging: mod `sell`
- [ ] Do logging: mod `deposit`
- [ ] Do logging: mod `withdraw`
- [ ] Do logging: mod `info`
- [ ] Do logging: pub(crate) mod `validation`

### src/store/handlers/validation.rs

- fn `validate_item_name`
- fn `validate_quantity`
- fn `validate_username`

**TODO:**

- [ ] Do comments: fn `validate_item_name`
- [ ] Do comments: fn `validate_quantity`
- [ ] Do comments: fn `validate_username`

- [ ] Do testing: fn `validate_item_name`
- [ ] Do testing: fn `validate_quantity`
- [ ] Do testing: fn `validate_username`

- [ ] Do logging: fn `validate_item_name`
- [ ] Do logging: fn `validate_quantity`
- [ ] Do logging: fn `validate_username`

### src/store/handlers/buy.rs

- async fn `handle`

**TODO:**

- [ ] Do comments: async fn `handle`

- [ ] Do testing: async fn `handle`

- [ ] Do logging: async fn `handle`

### src/store/handlers/sell.rs

- async fn `handle`

**TODO:**

- [ ] Do comments: async fn `handle`

- [ ] Do testing: async fn `handle`

- [ ] Do logging: async fn `handle`

### src/store/handlers/withdraw.rs

- async fn `handle_enqueue`
- async fn `handle_withdraw_balance_queued`

**TODO:**

- [ ] Do comments: async fn `handle_enqueue`
- [ ] Do comments: async fn `handle_withdraw_balance_queued`

- [ ] Do testing: async fn `handle_enqueue`
- [ ] Do testing: async fn `handle_withdraw_balance_queued`

- [ ] Do logging: async fn `handle_enqueue`
- [ ] Do logging: async fn `handle_withdraw_balance_queued`

### src/store/handlers/deposit.rs

- async fn `handle_enqueue`
- async fn `handle_deposit_balance_queued`

**TODO:**

- [ ] Do comments: async fn `handle_enqueue`
- [ ] Do comments: async fn `handle_deposit_balance_queued`

- [ ] Do testing: async fn `handle_enqueue`
- [ ] Do testing: async fn `handle_deposit_balance_queued`

- [ ] Do logging: async fn `handle_enqueue`
- [ ] Do logging: async fn `handle_deposit_balance_queued`

### src/store/handlers/player.rs

- async fn `handle_player_command`

**TODO:**

- [ ] Do comments: async fn `handle_player_command`

- [ ] Do testing: async fn `handle_player_command`

- [ ] Do logging: async fn `handle_player_command`

### src/store/handlers/operator.rs

- async fn `handle_additem_order`
- async fn `handle_removeitem_order`
- async fn `handle_add_currency`
- async fn `handle_remove_currency`

**TODO:**

- [ ] Do comments: async fn `handle_additem_order`
- [ ] Do comments: async fn `handle_removeitem_order`
- [ ] Do comments: async fn `handle_add_currency`
- [ ] Do comments: async fn `handle_remove_currency`

- [ ] Do testing: async fn `handle_additem_order`
- [ ] Do testing: async fn `handle_removeitem_order`
- [ ] Do testing: async fn `handle_add_currency`
- [ ] Do testing: async fn `handle_remove_currency`

- [ ] Do logging: async fn `handle_additem_order`
- [ ] Do logging: async fn `handle_removeitem_order`
- [ ] Do logging: async fn `handle_add_currency`
- [ ] Do logging: async fn `handle_remove_currency`

### src/store/handlers/cli.rs

- async fn `handle_cli_message`

**TODO:**

- [ ] Do comments: async fn `handle_cli_message`

- [ ] Do testing: async fn `handle_cli_message`

- [ ] Do logging: async fn `handle_cli_message`

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

- [ ] Do comments: async fn `handle_price`
- [ ] Do comments: async fn `handle_balance`
- [ ] Do comments: async fn `handle_pay`
- [ ] Do comments: async fn `handle_items`
- [ ] Do comments: async fn `handle_queue`
- [ ] Do comments: async fn `handle_cancel`
- [ ] Do comments: async fn `handle_status`
- [ ] Do comments: async fn `handle_help`
- [ ] Do comments: async fn `handle_price_command`
- [ ] Do comments: async fn `handle_status_command`
- [ ] Do comments: async fn `handle_items_command`
- [ ] Do comments: async fn `handle_help_command`
- [ ] Do comments: async fn `get_user_balance_async`
- [ ] Do comments: async fn `pay_async`

- [ ] Do testing: async fn `handle_price`
- [ ] Do testing: async fn `handle_balance`
- [ ] Do testing: async fn `handle_pay`
- [ ] Do testing: async fn `handle_items`
- [ ] Do testing: async fn `handle_queue`
- [ ] Do testing: async fn `handle_cancel`
- [ ] Do testing: async fn `handle_status`
- [ ] Do testing: async fn `handle_help`
- [ ] Do testing: async fn `handle_price_command`
- [ ] Do testing: async fn `handle_status_command`
- [ ] Do testing: async fn `handle_items_command`
- [ ] Do testing: async fn `handle_help_command`
- [ ] Do testing: async fn `get_user_balance_async`
- [ ] Do testing: async fn `pay_async`

- [ ] Do logging: async fn `handle_price`
- [ ] Do logging: async fn `handle_balance`
- [ ] Do logging: async fn `handle_pay`
- [ ] Do logging: async fn `handle_items`
- [ ] Do logging: async fn `handle_queue`
- [ ] Do logging: async fn `handle_cancel`
- [ ] Do logging: async fn `handle_status`
- [ ] Do logging: async fn `handle_help`
- [ ] Do logging: async fn `handle_price_command`
- [ ] Do logging: async fn `handle_status_command`
- [ ] Do logging: async fn `handle_items_command`
- [ ] Do logging: async fn `handle_help_command`
- [ ] Do logging: async fn `get_user_balance_async`
- [ ] Do logging: async fn `pay_async`
