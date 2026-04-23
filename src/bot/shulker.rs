//! Shulker box handling operations
//!
//! Provides shulker box manipulation with automatic retry logic.

use azalea::BlockPos;
use azalea::Vec3;
use tracing::{debug, error, info, warn};

use super::Bot;
use crate::constants::{
    DELAY_CONTAINER_SYNC_MS, DELAY_INTERACT_MS, DELAY_LOOK_AT_MS, RETRY_BASE_DELAY_MS,
    RETRY_MAX_DELAY_MS, SHULKER_OP_MAX_RETRIES, exponential_backoff_delay,
};
use crate::types::Position;

/// Calculate shulker station position from node position.
///
/// Layout (top down, P is southeast corner):
/// ```text
/// NCCN  <- z-2
/// NCCN  <- z-1
/// XSNP  <- z (S at x-2, P at x)
/// ```
/// Station (S) is two blocks west of the node position at the same Y/Z.
pub fn shulker_station_position(node_position: &Position) -> Position {
    Position {
        x: node_position.x - 2,
        y: node_position.y,
        z: node_position.z,
    }
}

/// Every shulker box item ID: the undyed default plus the 16 dye colors.
/// Must stay in sync with Minecraft's block registry — `is_shulker_box` is the
/// trust boundary used by inventory code to decide whether a slot is storage.
const SHULKER_BOX_IDS: &[&str] = &[
    "minecraft:shulker_box",
    "minecraft:white_shulker_box",
    "minecraft:orange_shulker_box",
    "minecraft:magenta_shulker_box",
    "minecraft:light_blue_shulker_box",
    "minecraft:yellow_shulker_box",
    "minecraft:lime_shulker_box",
    "minecraft:pink_shulker_box",
    "minecraft:gray_shulker_box",
    "minecraft:light_gray_shulker_box",
    "minecraft:cyan_shulker_box",
    "minecraft:purple_shulker_box",
    "minecraft:blue_shulker_box",
    "minecraft:brown_shulker_box",
    "minecraft:green_shulker_box",
    "minecraft:red_shulker_box",
    "minecraft:black_shulker_box",
];

/// True for any of the 17 shulker box variants (undyed + 16 dye colors).
///
/// Accepts IDs with or without the `minecraft:` prefix. This is an exact-match
/// check against `SHULKER_BOX_IDS`, not a substring search — items like
/// `minecraft:shulker_shell` and `minecraft:shulker_spawn_egg` return `false`.
pub fn is_shulker_box(item_id: &str) -> bool {
    let normalized = if item_id.contains(':') {
        item_id.to_string()
    } else {
        format!("minecraft:{}", item_id)
    };

    SHULKER_BOX_IDS.contains(&normalized.as_str())
}

/// Validate that a chest slot contains a shulker box (test-only helper).
#[cfg(test)]
pub fn validate_chest_slot_is_shulker(item_id: &str, slot_index: usize) -> Result<(), String> {
    if item_id.is_empty() {
        return Err(format!(
            "Chest slot {} is empty (expected shulker box). \
             Each chest slot should contain exactly one shulker box.",
            slot_index
        ));
    }

    if !is_shulker_box(item_id) {
        return Err(format!(
            "Chest slot {} contains '{}' instead of a shulker box. \
             Please replace it with a shulker box.",
            slot_index, item_id
        ));
    }

    Ok(())
}

/// Break the shulker at the station, walk over to collect the drop, then return to the node.
///
/// Returns `Err` if no shulker ends up in the bot inventory — callers rely on this
/// strict post-condition rather than silently proceeding empty-handed.
pub async fn pickup_shulker_from_station(
    bot: &Bot,
    station_pos: &Position,
    node_position: &Position,
) -> Result<(), String> {
    debug!(
        "pickup_shulker_from_station: station=({}, {}, {})",
        station_pos.x, station_pos.y, station_pos.z
    );

    let client = bot.client.read().await.clone().ok_or_else(|| {
        error!(
            "pickup_shulker_from_station: bot not connected (station=({}, {}, {}))",
            station_pos.x, station_pos.y, station_pos.z
        );
        "Bot not connected".to_string()
    })?;

    let station_block = BlockPos::new(station_pos.x, station_pos.y, station_pos.z);

    {
        let world = client.world();
        let block_state = world.read().get_block_state(station_block);
        match block_state {
            Some(state) => {
                let block_name = format!("{:?}", state);
                if !block_name.to_lowercase().contains("shulker") {
                    warn!(
                        "pickup_shulker_from_station: expected shulker at station ({}, {}, {}) but found {}",
                        station_pos.x, station_pos.y, station_pos.z, block_name
                    );
                }
            }
            None => warn!(
                "pickup_shulker_from_station: no block state at station ({}, {}, {}) — chunk unloaded?",
                station_pos.x, station_pos.y, station_pos.z
            ),
        }
    }

    let station_vec3 = Vec3::new(
        station_pos.x as f64 + 0.5,
        station_pos.y as f64 + 0.5,
        station_pos.z as f64 + 0.5,
    );
    client.look_at(station_vec3);
    tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_LOOK_AT_MS)).await;

    client.start_mining(station_block);

    // Walking away before the server finishes the break cancels mining server-side
    // and leaves the shulker intact, so we poll the block state until it's gone.
    const MAX_BREAK_WAIT_MS: u64 = 7000;
    const CHECK_INTERVAL_MS: u64 = 150;
    let mut waited_ms: u64 = 0;

    loop {
        tokio::time::sleep(tokio::time::Duration::from_millis(CHECK_INTERVAL_MS)).await;
        waited_ms += CHECK_INTERVAL_MS;

        let world = client.world();
        let block_state = world.read().get_block_state(station_block);

        match block_state {
            Some(state) => {
                let block_name = format!("{:?}", state);
                let block_name_lower = block_name.to_lowercase();
                if block_name_lower.contains("air") || !block_name_lower.contains("shulker") {
                    debug!(
                        "pickup_shulker_from_station: broken after {}ms (now {})",
                        waited_ms, block_name
                    );
                    break;
                }
            }
            None => {
                debug!(
                    "pickup_shulker_from_station: broken after {}ms (state=None)",
                    waited_ms
                );
                break;
            }
        }

        if waited_ms >= MAX_BREAK_WAIT_MS {
            error!(
                "pickup_shulker_from_station: timeout after {}ms at station ({}, {}, {}) — proceeding but pickup may fail",
                waited_ms, station_pos.x, station_pos.y, station_pos.z
            );
            break;
        }

        // Re-mine safety net: azalea's mining can silently stop if the look direction
        // drifts or the initial start_mining packet was dropped. Re-aim and re-issue
        // every 500ms so a transient glitch doesn't stall until the 7s timeout.
        if waited_ms.is_multiple_of(500) {
            client.look_at(station_vec3);
            client.start_mining(station_block);
        }
    }

    // The server spawns the dropped item entity on a short delay after the break
    // completes; it needs to settle before the pickup radius can vacuum it up.
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    // X is one block west of S (see layout diagram). Standing at the node itself is
    // out of pickup radius, so we step past S and then walk back.
    let pickup_pos = Position {
        x: node_position.x - 3,
        y: node_position.y,
        z: node_position.z,
    };
    super::navigation::navigate_to_position(bot, &pickup_pos).await?;
    tokio::time::sleep(tokio::time::Duration::from_millis(400)).await;

    super::navigation::navigate_to_position(bot, node_position).await?;
    tokio::time::sleep(tokio::time::Duration::from_millis(400)).await;

    // Post-condition check — see doc comment. A missing shulker here means either the
    // break never completed, the item despawned, or the bot never pathed into range.
    let inv_handle = client.open_inventory();
    if let Some(handle) = inv_handle {
        let slots = handle.slots();
        drop(handle);
        if let Some(slots) = slots {
            for (i, slot) in slots.iter().enumerate() {
                if slot.count() > 0 && is_shulker_box(&slot.kind().to_string()) {
                    debug!(
                        "pickup_shulker_from_station: shulker found in inventory slot {} (station=({}, {}, {}))",
                        i, station_pos.x, station_pos.y, station_pos.z
                    );
                    return Ok(());
                }
            }
        }
    }

    error!(
        "pickup_shulker_from_station: no shulker in inventory after pickup at station ({}, {}, {})",
        station_pos.x, station_pos.y, station_pos.z
    );
    Err(format!(
        "Failed to pick up shulker from station ({}, {}, {}) — not found in inventory after break + walk",
        station_pos.x, station_pos.y, station_pos.z
    ))
}

/// Single open attempt with no retry. See `open_shulker_at_station` for the retry wrapper.
async fn open_shulker_at_station_once(
    bot: &Bot,
    station_pos: &Position,
) -> Result<azalea::container::ContainerHandle, String> {
    let client = bot.client.read().await.clone().ok_or_else(|| {
        error!(
            "open_shulker_at_station_once: bot not connected (station=({}, {}, {}))",
            station_pos.x, station_pos.y, station_pos.z
        );
        "Bot not connected".to_string()
    })?;

    let station_block = BlockPos::new(station_pos.x, station_pos.y, station_pos.z);

    {
        let world = client.world();
        let block_state = world.read().get_block_state(station_block);
        match block_state {
            Some(state) => {
                let block_name = format!("{:?}", state);
                if !block_name.to_lowercase().contains("shulker") {
                    warn!(
                        "open_shulker_at_station_once: expected shulker at ({}, {}, {}) but found {} — open may fail",
                        station_pos.x, station_pos.y, station_pos.z, block_name
                    );
                }
            }
            None => warn!(
                "open_shulker_at_station_once: no block at station ({}, {}, {}) — chunk unloaded or shulker not placed",
                station_pos.x, station_pos.y, station_pos.z
            ),
        }
    }

    let station_vec3 = Vec3::new(
        station_pos.x as f64 + 0.5,
        station_pos.y as f64 + 0.5,
        station_pos.z as f64 + 0.5,
    );
    client.look_at(station_vec3);
    tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_INTERACT_MS)).await;

    // DELAY_CONTAINER_SYNC_MS (450ms) is the empirical container-open wait —
    // shorter and the contents read races the server packet.
    client.block_interact(station_block);
    tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_CONTAINER_SYNC_MS)).await;

    // 300 ticks (15s) to absorb server lag.
    let result = client
        .open_container_at_with_timeout_ticks(station_block, Some(300))
        .await;

    match result {
        Some(container) => Ok(container),
        None => {
            error!(
                "open_shulker_at_station_once: timeout after 15s at ({}, {}, {})",
                station_pos.x, station_pos.y, station_pos.z
            );
            Err(format!(
                "Failed to open shulker box at ({}, {}, {}) — timeout after 15s",
                station_pos.x, station_pos.y, station_pos.z
            ))
        }
    }
}

/// Open the shulker at `station_pos`, retrying with exponential backoff on failure.
pub async fn open_shulker_at_station(
    bot: &Bot,
    station_pos: &Position,
) -> Result<azalea::container::ContainerHandle, String> {
    let mut last_error = String::new();

    for attempt in 0..SHULKER_OP_MAX_RETRIES {
        if attempt > 0 {
            let delay_ms =
                exponential_backoff_delay(attempt - 1, RETRY_BASE_DELAY_MS, RETRY_MAX_DELAY_MS);
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        }

        match open_shulker_at_station_once(bot, station_pos).await {
            Ok(container) => {
                if attempt > 0 {
                    info!(
                        "open_shulker_at_station: succeeded on attempt {}/{} at ({}, {}, {})",
                        attempt + 1,
                        SHULKER_OP_MAX_RETRIES,
                        station_pos.x,
                        station_pos.y,
                        station_pos.z
                    );
                }
                return Ok(container);
            }
            Err(e) => {
                last_error = e.clone();
                warn!(
                    "open_shulker_at_station: attempt {}/{} failed at ({}, {}, {}): {}",
                    attempt + 1,
                    SHULKER_OP_MAX_RETRIES,
                    station_pos.x,
                    station_pos.y,
                    station_pos.z,
                    last_error
                );
            }
        }
    }

    error!(
        "open_shulker_at_station: exhausted {} attempts at ({}, {}, {}): {}",
        SHULKER_OP_MAX_RETRIES, station_pos.x, station_pos.y, station_pos.z, last_error
    );
    Err(format!(
        "Failed to open shulker at ({}, {}, {}) after {} attempts: {}",
        station_pos.x, station_pos.y, station_pos.z, SHULKER_OP_MAX_RETRIES, last_error
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_shulker_box_accepts_all_17_variants_with_or_without_prefix() {
        // Undyed default.
        assert!(is_shulker_box("minecraft:shulker_box"));
        assert!(is_shulker_box("shulker_box"));

        // All 16 dye colors, prefixed.
        assert!(is_shulker_box("minecraft:white_shulker_box"));
        assert!(is_shulker_box("minecraft:orange_shulker_box"));
        assert!(is_shulker_box("minecraft:magenta_shulker_box"));
        assert!(is_shulker_box("minecraft:light_blue_shulker_box"));
        assert!(is_shulker_box("minecraft:yellow_shulker_box"));
        assert!(is_shulker_box("minecraft:lime_shulker_box"));
        assert!(is_shulker_box("minecraft:pink_shulker_box"));
        assert!(is_shulker_box("minecraft:gray_shulker_box"));
        assert!(is_shulker_box("minecraft:light_gray_shulker_box"));
        assert!(is_shulker_box("minecraft:cyan_shulker_box"));
        assert!(is_shulker_box("minecraft:purple_shulker_box"));
        assert!(is_shulker_box("minecraft:blue_shulker_box"));
        assert!(is_shulker_box("minecraft:brown_shulker_box"));
        assert!(is_shulker_box("minecraft:green_shulker_box"));
        assert!(is_shulker_box("minecraft:red_shulker_box"));
        assert!(is_shulker_box("minecraft:black_shulker_box"));

        // Colors without the minecraft: prefix.
        assert!(is_shulker_box("red_shulker_box"));
        assert!(is_shulker_box("blue_shulker_box"));
    }

    #[test]
    fn is_shulker_box_rejects_empty_and_unrelated_items() {
        assert!(!is_shulker_box(""));
        assert!(!is_shulker_box("minecraft:chest"));
        assert!(!is_shulker_box("minecraft:ender_chest"));
        assert!(!is_shulker_box("minecraft:diamond"));
        assert!(!is_shulker_box("chest"));
    }

    #[test]
    fn is_shulker_box_rejects_items_that_merely_contain_shulker() {
        // Guards against regressing to a substring check — these all contain
        // "shulker" but are NOT storage containers.
        assert!(!is_shulker_box("minecraft:shulker_shell"));
        assert!(!is_shulker_box("minecraft:shulker_spawn_egg"));
        assert!(!is_shulker_box("shulker_shell"));
        assert!(!is_shulker_box("shulker"));
    }

    #[test]
    fn validate_chest_slot_accepts_any_color_shulker() {
        assert!(validate_chest_slot_is_shulker("minecraft:shulker_box", 0).is_ok());
        assert!(validate_chest_slot_is_shulker("minecraft:red_shulker_box", 5).is_ok());
        assert!(validate_chest_slot_is_shulker("minecraft:black_shulker_box", 53).is_ok());
    }

    #[test]
    fn validate_chest_slot_rejects_empty_slot_with_index_in_message() {
        let err = validate_chest_slot_is_shulker("", 10).unwrap_err();
        assert!(err.contains("slot 10 is empty"), "got: {err}");
    }

    #[test]
    fn validate_chest_slot_rejects_non_shulker_item_with_index_and_id_in_message() {
        let err = validate_chest_slot_is_shulker("minecraft:diamond", 20).unwrap_err();
        assert!(err.contains("slot 20 contains"), "got: {err}");
        assert!(err.contains("diamond"), "got: {err}");
    }

    #[test]
    fn validate_chest_slot_rejects_shulker_shell_lookalike() {
        // A chest filled with shulker shells would break storage semantics just
        // as badly as one filled with dirt — confirm the validator catches it.
        let err = validate_chest_slot_is_shulker("minecraft:shulker_shell", 3).unwrap_err();
        assert!(err.contains("slot 3 contains"), "got: {err}");
        assert!(err.contains("shulker_shell"), "got: {err}");
    }

    #[test]
    fn shulker_station_is_two_blocks_west_of_node() {
        let node_pos = Position {
            x: 100,
            y: 64,
            z: 200,
        };
        let station = shulker_station_position(&node_pos);

        assert_eq!(station.x, 98);
        assert_eq!(station.y, 64);
        assert_eq!(station.z, 200);
    }

    #[test]
    fn shulker_station_preserves_negative_and_zero_coordinates() {
        let node_pos = Position {
            x: 0,
            y: -64,
            z: -5,
        };
        let station = shulker_station_position(&node_pos);
        assert_eq!(station.x, -2);
        assert_eq!(station.y, -64);
        assert_eq!(station.z, -5);
    }
}
