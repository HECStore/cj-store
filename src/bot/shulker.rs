//! Shulker box handling operations
//!
//! Provides shulker box manipulation with automatic retry logic.

use azalea::BlockPos;
use azalea::Vec3;
use tracing::{debug, error, info, warn};

use super::Bot;
use crate::constants::{
    DELAY_INTERACT_MS, DELAY_LOOK_AT_MS, RETRY_BASE_DELAY_MS, RETRY_MAX_DELAY_MS,
    SHULKER_OP_MAX_RETRIES, exponential_backoff_delay,
};
use crate::types::Position;

/// Calculate shulker station position from node position.
/// Layout (top down, P is southeast corner):
/// ```
/// NCCN  <- z-2
/// NCCN  <- z-1
/// XSNP  <- z (S at x-2, P at x)
/// ```
/// Shulker station is 2 blocks west of P, at the same Y and Z level.
pub fn shulker_station_position(node_position: &Position) -> Position {
    Position {
        x: node_position.x - 2,
        y: node_position.y,
        z: node_position.z,
    }
}

/// List of all Minecraft shulker box item IDs (including colored variants).
/// Minecraft 1.20+ has 17 shulker box variants: 1 default + 16 colors.
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

/// Check if an item ID is a shulker box (any color).
///
/// This function correctly identifies all 17 shulker box variants:
/// - `minecraft:shulker_box` (default/undyed)
/// - `minecraft:{color}_shulker_box` (16 colored variants)
///
/// # Arguments
/// * `item_id` - The item ID to check (e.g., "minecraft:red_shulker_box")
///
/// # Returns
/// * `true` if the item is any type of shulker box
/// * `false` otherwise
///
/// # Notes
/// - Both `minecraft:` prefixed and non-prefixed IDs are accepted
/// - All colors are treated equally - no distinction based on color
pub fn is_shulker_box(item_id: &str) -> bool {
    // Normalize: add minecraft: prefix if missing
    let normalized = if item_id.contains(':') {
        item_id.to_string()
    } else {
        format!("minecraft:{}", item_id)
    };

    // Check against known shulker box IDs
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

/// Pick up a shulker box from the shulker station.
/// Breaks the shulker first, waits for it to be fully broken, then walks to the X position
/// (x-3 from node position, one block west of S) to pick up the dropped item, then returns
/// to the node position.
pub async fn pickup_shulker_from_station(
    bot: &Bot,
    station_pos: &Position,
    node_position: &Position,
) -> Result<(), String> {
    debug!(
        "pickup_shulker_from_station: Picking up shulker at ({}, {}, {})",
        station_pos.x, station_pos.y, station_pos.z
    );

    let client = bot.client.read().await.clone().ok_or_else(|| {
        error!("pickup_shulker_from_station: Bot not connected");
        "Bot not connected".to_string()
    })?;

    let station_block = BlockPos::new(station_pos.x, station_pos.y, station_pos.z);

    // Check block state before breaking
    {
        let world = client.world();
        let block_state = world.read().get_block_state(station_block);
        if let Some(state) = block_state {
            let block_name = format!("{:?}", state);
            debug!(
                "pickup_shulker_from_station: Block at station before mining: {}",
                block_name
            );
            if !block_name.to_lowercase().contains("shulker") {
                warn!(
                    "pickup_shulker_from_station: Expected shulker at station but found: {}",
                    block_name
                );
            }
        } else {
            warn!("pickup_shulker_from_station: Block state at station is None (no block?)");
        }
    }

    // Break the shulker first (bot should already be positioned to see it)
    // Look at center of shulker block for accurate mining
    let station_vec3 = Vec3::new(
        station_pos.x as f64 + 0.5,
        station_pos.y as f64 + 0.5,
        station_pos.z as f64 + 0.5,
    );
    client.look_at(station_vec3);
    tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_LOOK_AT_MS)).await;

    debug!("pickup_shulker_from_station: Starting mining operation");
    client.start_mining(station_block);

    // Wait for block to actually be broken (check block state in a loop)
    // Shulker boxes break quickly but we need to verify the block is actually gone
    // before moving to pick it up. Walking away before the break completes can cancel
    // mining server-side and leave the shulker intact.
    const MAX_BREAK_WAIT_MS: u64 = 7000; // Maximum 7 seconds to wait for block to break
    const CHECK_INTERVAL_MS: u64 = 150; // Check every 150ms
    let mut waited_ms: u64 = 0;

    loop {
        tokio::time::sleep(tokio::time::Duration::from_millis(CHECK_INTERVAL_MS)).await;
        waited_ms += CHECK_INTERVAL_MS;

        // Check if block is broken (air or not a shulker anymore)
        let world = client.world();
        let block_state = world.read().get_block_state(station_block);

        if let Some(state) = block_state {
            let block_name = format!("{:?}", state);
            let block_name_lower = block_name.to_lowercase();
            // Block is broken if it's air or any non-solid block
            // Use case-insensitive check since debug format produces "ShulkerBox" not "shulker"
            if block_name_lower.contains("air") || !block_name_lower.contains("shulker") {
                debug!(
                    "pickup_shulker_from_station: Block broken after {}ms (now: {})",
                    waited_ms, block_name
                );
                break;
            } else {
                if waited_ms % 1000 == 0 {
                    debug!(
                        "pickup_shulker_from_station: Still mining... {}ms elapsed, block: {}",
                        waited_ms, block_name
                    );
                }
            }
        } else {
            // Block state is None, treat as broken
            debug!(
                "pickup_shulker_from_station: Block broken after {}ms (state=None)",
                waited_ms
            );
            break;
        }

        if waited_ms >= MAX_BREAK_WAIT_MS {
            error!(
                "pickup_shulker_from_station: TIMEOUT waiting for shulker to break after {}ms!",
                waited_ms
            );
            // Check final state
            let world = client.world();
            let block_state = world.read().get_block_state(station_block);
            if let Some(state) = block_state {
                error!(
                    "pickup_shulker_from_station: Block at station after timeout: {:?}",
                    state
                );
            }
            warn!(
                "pickup_shulker_from_station: Proceeding despite timeout - shulker may not have been picked up"
            );
            break;
        }

        // Re-mine safety net: azalea's mining can silently stop if the look direction
        // drifts or the initial start_mining packet was dropped. Every 500ms we re-aim
        // at the block center and re-issue start_mining so a transient glitch doesn't
        // stall the whole pickup until the 7s timeout.
        if waited_ms % 500 == 0 {
            debug!(
                "pickup_shulker_from_station: Re-issuing mining command at {}ms",
                waited_ms
            );
            client.look_at(station_vec3);
            client.start_mining(station_block);
        }
    }

    // Additional delay for the item to drop and settle.
    // After the block is destroyed the server spawns the dropped item entity on a
    // short delay, and it needs a moment to settle onto the floor before the bot's
    // pickup radius can reliably vacuum it up.
    // Wait for the dropped item entity to settle
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    // Walk to X position (x-3 from node position) to pick up the dropped shulker.
    // X is one block west of S (which is at x-2), and coincides with the P position of the node to the west.
    // Standing at the node itself is too far for the item pickup radius to reach the drop,
    // so we deliberately step one block past S and then walk back.
    let pickup_pos = Position {
        x: node_position.x - 3, // One block left of S (which is at x-2)
        y: node_position.y,
        z: node_position.z,
    };
    debug!(
        "pickup_shulker_from_station: Walking to pickup position ({}, {}, {})",
        pickup_pos.x, pickup_pos.y, pickup_pos.z
    );
    super::navigation::navigate_to_position(bot, &pickup_pos).await?;
    // Small pause after arriving so the server has time to deliver any pickup packets
    // triggered by walking over the item before we move again.
    tokio::time::sleep(tokio::time::Duration::from_millis(400)).await;

    // Walk back to node position
    super::navigation::navigate_to_position(bot, node_position).await?;
    tokio::time::sleep(tokio::time::Duration::from_millis(400)).await;

    // Verify we picked up the shulker - this is CRITICAL, return error if not found.
    // A missing shulker here usually means either the break never completed, the item
    // despawned, or the bot failed to path into pickup range - all conditions the
    // caller must handle rather than silently continuing with an empty hand.
    let inv_handle = client.open_inventory();
    if let Some(handle) = inv_handle {
        let slots = handle.slots();
        drop(handle);
        if let Some(slots) = slots {
            for (i, slot) in slots.iter().enumerate() {
                if slot.count() > 0 && is_shulker_box(&slot.kind().to_string()) {
                    debug!("pickup_shulker_from_station: Found shulker in slot {}", i);
                    return Ok(());
                }
            }
        }
    }

    // Shulker not found in inventory - this is a CRITICAL error, not just a warning
    error!(
        "pickup_shulker_from_station: FAILED - No shulker found in inventory after pickup at station ({}, {}, {})",
        station_pos.x, station_pos.y, station_pos.z
    );
    Err("Failed to pick up shulker from station - shulker not found in inventory after breaking and walking to pickup position".to_string())
}

/// Open a shulker box at the station position (single attempt, no retry).
async fn open_shulker_at_station_once(
    bot: &Bot,
    station_pos: &Position,
) -> Result<azalea::container::ContainerHandle, String> {
    debug!(
        "open_shulker_at_station_once: Attempting to open shulker at ({}, {}, {})",
        station_pos.x, station_pos.y, station_pos.z
    );

    let client = bot.client.read().await.clone().ok_or_else(|| {
        error!("open_shulker_at_station_once: Bot not connected");
        "Bot not connected".to_string()
    })?;

    let station_block = BlockPos::new(station_pos.x, station_pos.y, station_pos.z);

    // Check block state before opening
    {
        let world = client.world();
        let block_state = world.read().get_block_state(station_block);
        if let Some(state) = block_state {
            let block_name = format!("{:?}", state);
            debug!(
                "open_shulker_at_station_once: Block at station: {}",
                block_name
            );
            if !block_name.to_lowercase().contains("shulker") {
                warn!(
                    "open_shulker_at_station_once: Expected shulker but found: {} - open may fail!",
                    block_name
                );
            }
        } else {
            warn!(
                "open_shulker_at_station_once: Block state at station is None - no shulker placed?"
            );
        }
    }

    // Look at the shulker block
    let station_vec3 = Vec3::new(
        station_pos.x as f64 + 0.5,
        station_pos.y as f64 + 0.5,
        station_pos.z as f64 + 0.5,
    );
    debug!(
        "open_shulker_at_station_once: Looking at shulker ({:.1}, {:.1}, {:.1})",
        station_vec3.x, station_vec3.y, station_vec3.z
    );
    client.look_at(station_vec3);
    tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_INTERACT_MS)).await;

    // Right-click to open the shulker, then get the container handle
    debug!("open_shulker_at_station_once: Sending block_interact to open shulker");
    client.block_interact(station_block);
    tokio::time::sleep(tokio::time::Duration::from_millis(450)).await;

    // Get the container handle for the opened shulker
    // Use 300 ticks (15 seconds) timeout to handle server lag
    debug!("open_shulker_at_station_once: Waiting for container handle (15s timeout)");
    let result = client
        .open_container_at_with_timeout_ticks(station_block, Some(300))
        .await;

    match result {
        Some(container) => {
            if let Some(contents) = container.contents() {
                debug!(
                    "open_shulker_at_station_once: Opened, {} slots, {} items",
                    contents.len(),
                    contents.iter().map(|s| s.count() as i32).sum::<i32>()
                );
            }
            Ok(container)
        }
        None => {
            error!(
                "open_shulker_at_station_once: FAILED to open shulker at ({}, {}, {}) - timeout after 15 seconds",
                station_pos.x, station_pos.y, station_pos.z
            );
            Err(format!(
                "Failed to open shulker box at ({}, {}, {}) - timeout after 15 seconds",
                station_pos.x, station_pos.y, station_pos.z
            ))
        }
    }
}

/// Open a shulker box at the station position with retry logic.
///
/// Uses exponential backoff for retries: 500ms, 1s, 2s, etc.
pub async fn open_shulker_at_station(
    bot: &Bot,
    station_pos: &Position,
) -> Result<azalea::container::ContainerHandle, String> {
    debug!(
        "open_shulker_at_station: Opening at ({}, {}, {})",
        station_pos.x, station_pos.y, station_pos.z
    );

    let mut last_error = String::new();

    for attempt in 0..SHULKER_OP_MAX_RETRIES {
        if attempt > 0 {
            let delay_ms =
                exponential_backoff_delay(attempt - 1, RETRY_BASE_DELAY_MS, RETRY_MAX_DELAY_MS);
            debug!(
                "open_shulker_at_station: Retry {}/{} after {}ms",
                attempt + 1, SHULKER_OP_MAX_RETRIES, delay_ms
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        } else {
            debug!(
                "open_shulker_at_station: Attempt 1/{}",
                SHULKER_OP_MAX_RETRIES
            );
        }

        match open_shulker_at_station_once(bot, station_pos).await {
            Ok(container) => {
                if attempt > 0 {
                    info!(
                        "open_shulker_at_station: SUCCESS on attempt {}/{}",
                        attempt + 1,
                        SHULKER_OP_MAX_RETRIES
                    );
                }
                return Ok(container);
            }
            Err(e) => {
                last_error = e.clone();
                warn!(
                    "open_shulker_at_station: Attempt {}/{} FAILED at ({}, {}, {}): {}",
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
        "open_shulker_at_station: FAILED after {} attempts at ({}, {}, {}): {}",
        SHULKER_OP_MAX_RETRIES, station_pos.x, station_pos.y, station_pos.z, last_error
    );
    Err(format!(
        "Failed to open shulker at ({}, {}, {}) after {} attempts: {}",
        station_pos.x, station_pos.y, station_pos.z, SHULKER_OP_MAX_RETRIES, last_error
    ))
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_shulker_box() {
        // Default shulker box
        assert!(is_shulker_box("minecraft:shulker_box"));
        assert!(is_shulker_box("shulker_box"));

        // All colored variants
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

        // Without minecraft: prefix
        assert!(is_shulker_box("red_shulker_box"));
        assert!(is_shulker_box("blue_shulker_box"));

        // Non-shulker items
        assert!(!is_shulker_box("minecraft:chest"));
        assert!(!is_shulker_box("minecraft:diamond"));
        assert!(!is_shulker_box("minecraft:ender_chest"));
        assert!(!is_shulker_box("chest"));
        assert!(!is_shulker_box(""));
    }

    #[test]
    fn test_validate_chest_slot_is_shulker() {
        // Valid shulker boxes
        assert!(validate_chest_slot_is_shulker("minecraft:shulker_box", 0).is_ok());
        assert!(validate_chest_slot_is_shulker("minecraft:red_shulker_box", 5).is_ok());

        // Empty slot
        let err = validate_chest_slot_is_shulker("", 10).unwrap_err();
        assert!(err.contains("slot 10 is empty"));

        // Non-shulker item
        let err = validate_chest_slot_is_shulker("minecraft:diamond", 20).unwrap_err();
        assert!(err.contains("slot 20 contains"));
        assert!(err.contains("diamond"));
    }

    #[test]
    fn test_shulker_station_position() {
        let node_pos = Position {
            x: 100,
            y: 64,
            z: 200,
        };
        let station = shulker_station_position(&node_pos);

        // Station should be two blocks left (west) of node position
        assert_eq!(station.x, 98); // x - 2
        assert_eq!(station.y, 64); // same Y
        assert_eq!(station.z, 200); // same Z
    }
}
