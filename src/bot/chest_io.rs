//! Chest I/O operations with shulker handling
//!
//! Provides chest interaction and shulker manipulation with automatic retry logic.

use azalea::BlockPos;
use azalea::Vec3;
use azalea::inventory::operations::PickupClick;
use tracing::{debug, error, info, warn};

use super::Bot;
use crate::constants::{
    CHEST_OP_MAX_RETRIES, DELAY_LOOK_AT_MS, RETRY_BASE_DELAY_MS, RETRY_MAX_DELAY_MS,
    exponential_backoff_delay,
};
use crate::types::Position;

/// Place a shulker from player inventory back into a chest slot with verification.
///
/// This function handles the full flow of picking up a shulker from the player inventory
/// (as seen through the chest container view) and placing it in the specified chest slot,
/// with proper verification that the placement actually succeeded.
///
/// # Arguments
/// * `container` - The open chest container handle
/// * `container_slot` - The slot in the container view where the shulker is (player inventory portion)
/// * `chest_slot` - The target slot in the chest to place the shulker
///
/// # Returns
/// * `Ok(())` if the shulker was successfully placed and verified
/// * `Err(String)` if placement failed or verification failed
pub async fn place_shulker_in_chest_slot_verified(
    container: &azalea::container::ContainerHandle,
    container_slot: usize,
    chest_slot: usize,
) -> Result<(), String> {
    const MAX_VERIFICATION_ATTEMPTS: u32 = 7;
    const CLICK_DELAY_MS: u64 = 300;
    const VERIFY_DELAY_MS: u64 = 250;

    info!(
        "place_shulker_in_chest_slot_verified: Moving shulker from container slot {} to chest slot {}", 
        container_slot, chest_slot
    );

    // Log state before pickup
    let slots_before = container
        .slots()
        .ok_or_else(|| "Container closed before pickup".to_string())?;
    if let Some(slot_item) = slots_before.get(container_slot) {
        debug!(
            "place_shulker_in_chest_slot_verified: Source slot {} BEFORE: {} x{}", 
            container_slot, slot_item.kind(), slot_item.count()
        );
    }
    if let Some(slot_item) = slots_before.get(chest_slot) {
        debug!(
            "place_shulker_in_chest_slot_verified: Target slot {} BEFORE: {} x{}", 
            chest_slot, slot_item.kind(), slot_item.count()
        );
    }

    // Pick up shulker from the container view's inventory portion
    info!(
        "place_shulker_in_chest_slot_verified: Left-clicking container slot {} to pickup shulker",
        container_slot
    );
    container.click(PickupClick::Left {
        slot: Some(container_slot as u16),
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(CLICK_DELAY_MS)).await;

    // Verify shulker was picked up (slot should now be empty or have different item)
    let slots_after_pickup = container
        .slots()
        .ok_or_else(|| "Container closed while picking up shulker".to_string())?;
    if let Some(slot_item) = slots_after_pickup.get(container_slot) {
        debug!(
            "place_shulker_in_chest_slot_verified: Source slot {} AFTER pickup: {} x{}", 
            container_slot, slot_item.kind(), slot_item.count()
        );
        if slot_item.count() > 0 && super::shulker::is_shulker_box(&slot_item.kind().to_string()) {
            warn!(
                "place_shulker_in_chest_slot_verified: Shulker STILL in slot {} after pickup - retrying click",
                container_slot
            );
            // Try again
            container.click(PickupClick::Left {
                slot: Some(container_slot as u16),
            });
            tokio::time::sleep(tokio::time::Duration::from_millis(CLICK_DELAY_MS)).await;
            
            // Check again
            let slots_retry = container.slots().ok_or_else(|| "Container closed".to_string())?;
            if let Some(slot_item) = slots_retry.get(container_slot) {
                debug!(
                    "place_shulker_in_chest_slot_verified: Source slot {} AFTER retry pickup: {} x{}", 
                    container_slot, slot_item.kind(), slot_item.count()
                );
            }
        }
    }

    // Place back in chest slot
    info!(
        "place_shulker_in_chest_slot_verified: Left-clicking chest slot {} to place shulker", 
        chest_slot
    );
    container.click(PickupClick::Left {
        slot: Some(chest_slot as u16),
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(CLICK_DELAY_MS)).await;

    // Check state after placement
    let updated_slots = container
        .slots()
        .ok_or_else(|| "Container closed".to_string())?;
    debug!(
        "place_shulker_in_chest_slot_verified: Chest slot {} AFTER placement:",
        chest_slot
    );
    if let Some(item) = updated_slots.get(chest_slot) {
        debug!("  Item: {}, Count: {}", item.kind(), item.count());
    } else {
        debug!("  Slot is None/empty");
    }

    // Log all shulker locations for debugging.
    // Slot index mapping in the double-chest container view:
    //   0..54  = chest slots (the 54 big-chest slots)
    //   54..81 = player inventory (27 slots, corresponding to player inventory slots 9..36)
    //   81..90 = player hotbar (9 slots, corresponding to player inventory slots 0..8)
    // `idx < 54` divides "in the chest" from "in the player's side".
    debug!("place_shulker_in_chest_slot_verified: Current shulker locations:");
    for (idx, slot) in updated_slots.iter().enumerate() {
        if slot.count() > 0 && super::shulker::is_shulker_box(&slot.kind().to_string()) {
            let slot_type = if idx < 54 { "chest" } else { "inventory/hotbar" };
            debug!("  {} slot {}: {}", slot_type, idx, slot.kind());
        }
    }

    // Verify the shulker is now in the chest slot
    let mut verified = false;
    for attempt in 0..MAX_VERIFICATION_ATTEMPTS {
        let updated_slots = container
            .slots()
            .ok_or_else(|| "Container closed while verifying shulker placement".to_string())?;

        if let Some(chest_item) = updated_slots.get(chest_slot) {
            if chest_item.count() > 0
                && super::shulker::is_shulker_box(&chest_item.kind().to_string())
            {
                info!(
                    "place_shulker_in_chest_slot_verified: SUCCESS - Shulker verified in chest slot {} on attempt {}",
                    chest_slot, attempt + 1
                );
                verified = true;
                break;
            }
        }

        if attempt < MAX_VERIFICATION_ATTEMPTS - 1 {
            debug!(
                "place_shulker_in_chest_slot_verified: Verification attempt {}/{} - shulker not in chest slot {} yet",
                attempt + 1,
                MAX_VERIFICATION_ATTEMPTS,
                chest_slot
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(VERIFY_DELAY_MS)).await;
        }
    }

    if !verified {
        // Check if shulker is still in cursor (we might need to click again)
        warn!(
            "place_shulker_in_chest_slot_verified: Verification FAILED after {} attempts - attempting recovery",
            MAX_VERIFICATION_ATTEMPTS
        );

        // Try clicking on chest slot again in case shulker is in cursor
        info!("place_shulker_in_chest_slot_verified: Recovery - clicking chest slot {} again", chest_slot);
        container.click(PickupClick::Left {
            slot: Some(chest_slot as u16),
        });
        tokio::time::sleep(tokio::time::Duration::from_millis(CLICK_DELAY_MS)).await;

        // Final verification
        let final_slots = container
            .slots()
            .ok_or_else(|| "Container closed during recovery".to_string())?;
        
        debug!("place_shulker_in_chest_slot_verified: Recovery - checking final state:");
        if let Some(chest_item) = final_slots.get(chest_slot) {
            debug!(
                "  Chest slot {}: {} x{}", 
                chest_slot, chest_item.kind(), chest_item.count()
            );
            if chest_item.count() > 0
                && super::shulker::is_shulker_box(&chest_item.kind().to_string())
            {
                info!(
                    "place_shulker_in_chest_slot_verified: Recovery SUCCESS - Shulker placed in chest slot {}",
                    chest_slot
                );
                return Ok(());
            }
        }

        // Log final shulker locations for debugging
        error!("place_shulker_in_chest_slot_verified: Recovery FAILED - shulker locations:");
        for (idx, slot) in final_slots.iter().enumerate() {
            if slot.count() > 0 && super::shulker::is_shulker_box(&slot.kind().to_string()) {
                let slot_type = if idx < 54 { "chest" } else { "inventory/hotbar" };
                error!("  {} slot {}: {}", slot_type, idx, slot.kind());
            }
        }

        return Err(format!(
            "Failed to place shulker in chest slot {} - verification failed after recovery attempt",
            chest_slot
        ));
    }

    Ok(())
}

/// Open a chest container at the given position (single attempt, no retry).
///
/// Looks at the center of the chest block before attempting to open it,
/// which helps ensure the interaction is successful.
///
/// # Arguments
/// * `bot` - Bot instance
/// * `chest_pos` - Block position of the chest
///
/// # Errors
/// Returns detailed error including position and timeout duration
async fn open_chest_container_once(
    bot: &Bot,
    chest_pos: BlockPos,
) -> Result<azalea::container::ContainerHandle, String> {
    let client = bot.client.read().await.clone().ok_or_else(|| {
        error!(
            "open_chest_container_once: Bot not connected - cannot open chest at ({}, {}, {})",
            chest_pos.x, chest_pos.y, chest_pos.z
        );
        format!(
            "Bot not connected - cannot open chest at ({}, {}, {})",
            chest_pos.x, chest_pos.y, chest_pos.z
        )
    })?;

    debug!(
        "open_chest_container_once: Attempting to open chest at ({}, {}, {})",
        chest_pos.x, chest_pos.y, chest_pos.z
    );

    // Check block state before opening
    {
        let world = client.world();
        let block_state = world.read().get_block_state(chest_pos);
        if let Some(state) = block_state {
            let block_name = format!("{:?}", state);
            debug!("open_chest_container_once: Block at position: {}", block_name);
            if !block_name.to_lowercase().contains("chest") {
                warn!(
                    "open_chest_container_once: Expected chest but found: {} - open may fail!", 
                    block_name
                );
            }
        } else {
            warn!("open_chest_container_once: Block state at position is None");
        }
    }

    // Look at the center of the chest block before opening
    let chest_center = Vec3::new(
        chest_pos.x as f64 + 0.5,
        chest_pos.y as f64 + 0.5,
        chest_pos.z as f64 + 0.5,
    );
    debug!(
        "open_chest_container_once: Looking at chest center ({:.1}, {:.1}, {:.1})",
        chest_center.x, chest_center.y, chest_center.z
    );
    client.look_at(chest_center);
    tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_LOOK_AT_MS)).await;

    // 300 ticks = 15 seconds at 20 TPS
    let timeout_ticks: usize = 300;
    let timeout_secs = timeout_ticks / 20;

    debug!(
        "open_chest_container_once: Opening container with {}s timeout",
        timeout_secs
    );
    let container = client
        .open_container_at_with_timeout_ticks(chest_pos, Some(timeout_ticks))
        .await;

    match container {
        Some(c) => {
            // Log container info
            if let Some(contents) = c.contents() {
                info!(
                    "open_chest_container_once: SUCCESS - Chest opened at ({}, {}, {}), {} slots, {} items",
                    chest_pos.x, chest_pos.y, chest_pos.z,
                    contents.len(),
                    contents.iter().map(|s| s.count() as i32).sum::<i32>()
                );
            } else {
                info!(
                    "open_chest_container_once: SUCCESS - Chest opened at ({}, {}, {}) (contents not available yet)",
                    chest_pos.x, chest_pos.y, chest_pos.z
                );
            }
            Ok(c)
        }
        None => {
            error!(
                "open_chest_container_once: FAILED to open chest at ({}, {}, {}) after {}s timeout",
                chest_pos.x, chest_pos.y, chest_pos.z, timeout_secs
            );
            let error_msg = format!(
                "Failed to open chest at ({}, {}, {}) after {}s timeout - chest may not exist or is obstructed",
                chest_pos.x, chest_pos.y, chest_pos.z, timeout_secs
            );
            Err(error_msg)
        }
    }
}

/// Open a chest container at the given position for validation/discovery purposes.
///
/// This uses a short timeout (5 seconds) and NO retries - if there's no chest,
/// it fails fast instead of waiting and retrying. This is ideal for node discovery
/// where we expect to eventually hit positions without chests.
///
/// # Arguments
/// * `bot` - Bot instance
/// * `chest_pos` - Block position of the chest
///
/// # Errors
/// Returns error if chest doesn't exist or can't be opened within 5 seconds
pub async fn open_chest_container_for_validation(
    bot: &Bot,
    chest_pos: BlockPos,
) -> Result<azalea::container::ContainerHandle, String> {
    let client = bot.client.read().await.clone().ok_or_else(|| {
        format!(
            "Bot not connected - cannot open chest at ({}, {}, {})",
            chest_pos.x, chest_pos.y, chest_pos.z
        )
    })?;

    debug!(
        "Attempting to open chest at ({}, {}, {}) for validation (fast, no retry)",
        chest_pos.x, chest_pos.y, chest_pos.z
    );

    // Look at the center of the chest block before opening
    let chest_center = Vec3::new(
        chest_pos.x as f64 + 0.5,
        chest_pos.y as f64 + 0.5,
        chest_pos.z as f64 + 0.5,
    );
    client.look_at(chest_center);
    tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_LOOK_AT_MS)).await;

    // Use short timeout for validation: 100 ticks = 5 seconds at 20 TPS
    // If there's no chest, we'll know quickly
    let timeout_ticks: usize = 100;
    let timeout_secs = timeout_ticks / 20;

    let container = client
        .open_container_at_with_timeout_ticks(chest_pos, Some(timeout_ticks))
        .await;

    match container {
        Some(c) => {
            info!(
                "Successfully opened chest at ({}, {}, {}) for validation",
                chest_pos.x, chest_pos.y, chest_pos.z
            );
            Ok(c)
        }
        None => {
            // No retries for validation - fail fast
            Err(format!(
                "No chest found at ({}, {}, {}) after {}s - position likely doesn't have a chest",
                chest_pos.x, chest_pos.y, chest_pos.z, timeout_secs
            ))
        }
    }
}

/// Open a chest container at the given position with retry logic.
///
/// Uses exponential backoff for retries: 500ms, 1s, 2s, etc.
/// For validation/discovery where you expect to eventually hit missing chests,
/// use `open_chest_container_for_validation` instead.
///
/// # Arguments
/// * `bot` - Bot instance
/// * `chest_pos` - Block position of the chest
///
/// # Errors
/// Returns detailed error including position, attempt count, and all failure reasons
pub async fn open_chest_container(
    bot: &Bot,
    chest_pos: BlockPos,
) -> Result<azalea::container::ContainerHandle, String> {
    info!(
        "open_chest_container: Opening chest at ({}, {}, {}) with up to {} retries",
        chest_pos.x, chest_pos.y, chest_pos.z, CHEST_OP_MAX_RETRIES
    );
    
    let mut last_error = String::new();

    for attempt in 0..CHEST_OP_MAX_RETRIES {
        if attempt > 0 {
            let delay_ms =
                exponential_backoff_delay(attempt - 1, RETRY_BASE_DELAY_MS, RETRY_MAX_DELAY_MS);
            info!(
                "open_chest_container: Retry {}/{} at ({}, {}, {}) after {}ms delay",
                attempt + 1,
                CHEST_OP_MAX_RETRIES,
                chest_pos.x,
                chest_pos.y,
                chest_pos.z,
                delay_ms
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        } else {
            debug!("open_chest_container: Attempt 1/{}", CHEST_OP_MAX_RETRIES);
        }

        match open_chest_container_once(bot, chest_pos).await {
            Ok(container) => {
                if attempt > 0 {
                    info!(
                        "open_chest_container: SUCCESS on attempt {}/{}", 
                        attempt + 1, CHEST_OP_MAX_RETRIES
                    );
                }
                return Ok(container);
            }
            Err(e) => {
                last_error = e.clone();
                warn!(
                    "open_chest_container: Attempt {}/{} FAILED at ({}, {}, {}): {}",
                    attempt + 1,
                    CHEST_OP_MAX_RETRIES,
                    chest_pos.x,
                    chest_pos.y,
                    chest_pos.z,
                    last_error
                );
            }
        }
    }

    error!(
        "open_chest_container: FAILED after {} attempts at ({}, {}, {}): {}",
        CHEST_OP_MAX_RETRIES, chest_pos.x, chest_pos.y, chest_pos.z, last_error
    );
    Err(format!(
        "Failed to open chest at ({}, {}, {}) after {} attempts: {}",
        chest_pos.x, chest_pos.y, chest_pos.z, CHEST_OP_MAX_RETRIES, last_error
    ))
}

/// Transfer items from/to a shulker box.
/// direction: "withdraw" = from shulker to bot inventory (slots 9-35, NOT hotbar), "deposit" = from bot inventory (slots 9-35) to shulker
pub async fn transfer_items_with_shulker(
    _bot: &Bot,
    shulker_container: &azalea::container::ContainerHandle,
    item: &str,
    amount: i32,
    direction: &str,
) -> Result<i32, String> {
    use azalea::inventory::operations::PickupClick;
    
    let target_id = Bot::normalize_item_id(item);
    let mut remaining = amount;
    let mut total_moved = 0;

    info!(
        "transfer_items_with_shulker: Starting {} of {} x{} (target_id: {})", 
        direction, item, amount, target_id
    );

    match direction {
        "withdraw" => {
            // From shulker (contents slots 0-26) to bot inventory (slots 9-35, NOT hotbar 36-44)
            // Hotbar slot 0 (36) is reserved for shulker boxes
            let mut consecutive_failures = 0;
            let mut iteration = 0;
            while remaining > 0 {
                iteration += 1;
                debug!(
                    "transfer_items_with_shulker: Withdraw iteration {}, remaining: {}, moved so far: {}", 
                    iteration, remaining, total_moved
                );
                
                // Wait for container contents to sync from server
                tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
                
                let contents = shulker_container
                    .contents()
                    .ok_or_else(|| {
                        error!("transfer_items_with_shulker: Shulker closed during withdraw");
                        "Shulker closed".to_string()
                    })?;
                let mut found: Option<(usize, i32)> = None;
                for (i, stack) in contents.iter().enumerate() {
                    if stack.count() > 0
                        && Bot::normalize_item_id(&stack.kind().to_string()) == target_id
                    {
                        found = Some((i, stack.count()));
                        debug!(
                            "transfer_items_with_shulker: Found {} x{} in shulker slot {}", 
                            stack.kind(), stack.count(), i
                        );
                        break;
                    }
                }

                let Some((slot, stack_count)) = found else {
                    info!(
                        "transfer_items_with_shulker: No more {} found in shulker, stopping withdraw", 
                        target_id
                    );
                    break;
                };

                // Shift-click vs manual click tradeoff:
                //   * Shift-click (quick_move_from_container) moves the WHOLE stack in a
                //     single click, with the server distributing items to destination slots.
                //     Far faster but we can't pick an exact amount - it's "all or whatever fits".
                //     Only safe when we actually want the whole stack (remaining >= stack_count).
                //   * Manual click (else branch below) picks up the stack, right-clicks N times
                //     to drop exactly N items, then puts the rest back. Slower (N network ops)
                //     but lets us move a precise partial amount.
                if remaining >= stack_count {
                    debug!(
                        "transfer_items_with_shulker: Shift-clicking slot {} ({} items, need {})",
                        slot, stack_count, remaining
                    );
                    let moved =
                        super::inventory::quick_move_from_container(shulker_container, slot).await?;
                    if moved <= 0 {
                        // Shift-click reported 0 moved - could be a timing issue where the
                        // container contents haven't synced yet. Re-check the slot count to
                        // distinguish "silent success" from a real failure before retrying.
                        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                        let contents_after = shulker_container
                            .contents()
                            .ok_or_else(|| "Shulker closed".to_string())?;
                        let current_count = contents_after.get(slot).map(|s| s.count()).unwrap_or(0);
                        
                        if current_count < stack_count {
                            // Items DID move, we just didn't detect it
                            // Count the actual moved amount
                            let actual_moved = stack_count - current_count;
                            info!(
                                "transfer_items_with_shulker: Shift-click reported 0 but {} items actually moved (slot {} now has {})", 
                                actual_moved, slot, current_count
                            );
                            total_moved += actual_moved;
                            remaining -= actual_moved;
                            consecutive_failures = 0;
                        } else {
                            // Items really didn't move - retry
                            consecutive_failures += 1;
                            warn!(
                                "transfer_items_with_shulker: Shift-click moved 0 items (failure {}/3)", 
                                consecutive_failures
                            );
                            if consecutive_failures >= 3 {
                                error!("transfer_items_with_shulker: Shift-click failed 3 times in a row, stopping extraction");
                                break;
                            }
                        }
                        continue;
                    }
                    consecutive_failures = 0; // Reset on success
                    total_moved += moved;
                    remaining -= moved;
                    debug!(
                        "transfer_items_with_shulker: Shift-click moved {}, total: {}, remaining: {}", 
                        moved, total_moved, remaining
                    );
                } else {
                    // Need only a partial stack - use manual click transfer
                    info!(
                        "transfer_items_with_shulker: Partial transfer needed ({} from stack of {})", 
                        remaining, stack_count
                    );
                    // Pick up the stack from shulker
                    debug!("transfer_items_with_shulker: Picking up stack from slot {}", slot);
                    shulker_container.click(PickupClick::Left {
                        slot: Some(slot as u16),
                    });
                    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                    
                    // Find an empty slot in inventory portion (slots 27-53 in shulker container view)
                    let all_slots = shulker_container
                        .slots()
                        .ok_or_else(|| "Shulker closed".to_string())?;
                    let shulker_size = contents.len(); // 27 slots
                    let inv_start = shulker_size; // 27
                    let inv_end = inv_start + 27; // 54 (inventory slots 9-35)
                    
                    let mut target_slot: Option<usize> = None;
                    for i in inv_start..inv_end.min(all_slots.len()) {
                        if all_slots[i].count() == 0 {
                            target_slot = Some(i);
                            break;
                        }
                    }
                    
                    let target = target_slot.ok_or_else(|| {
                        error!("transfer_items_with_shulker: No empty inventory slot for partial transfer");
                        "No empty inventory slot for partial transfer".to_string()
                    })?;
                    
                    debug!(
                        "transfer_items_with_shulker: Right-clicking {} times to slot {} for partial transfer", 
                        remaining, target
                    );
                    // Right-click to place one item at a time into the target slot
                    for _ in 0..remaining {
                        shulker_container.click(PickupClick::Right {
                            slot: Some(target as u16),
                        });
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                    
                    // Put remaining items back in original slot
                    debug!("transfer_items_with_shulker: Returning remaining items to slot {}", slot);
                    shulker_container.click(PickupClick::Left {
                        slot: Some(slot as u16),
                    });
                    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                    
                    total_moved += remaining;
                    remaining = 0;
                }
            }
        }
        "deposit" => {
            // From bot inventory (slots 9-35 AND hotbar 36-44) to shulker.
            // Slot mapping differs from the DOUBLE-chest view because a shulker is
            // a SMALL 27-slot container. When a shulker is open:
            //   0..27  = shulker's own 27 storage slots
            //   27..54 = player inventory (27 slots -> player slots 9..36)
            //   54..63 = player hotbar (9 slots -> player slots 0..8)
            // (Compare with the double-chest case where 0..54 = chest, 54..81 = inv, 81..90 = hotbar.)
            // After villager trades, picked-up items can land in either the inventory or
            // the hotbar, so we search the entire 27..63 range to find depositable items.
            let shulker_contents = shulker_container
                .contents()
                .ok_or_else(|| "Shulker closed".to_string())?;
            let inv_start = shulker_contents.len(); // Bot inventory starts after shulker contents (27)
            let inventory_end = inv_start + 36; // 27 inventory + 9 hotbar = 36 slots (27..63)

            debug!(
                "transfer_items_with_shulker: Deposit - searching slots {}-{} for {}", 
                inv_start, inventory_end - 1, target_id
            );

            let mut consecutive_failures = 0;
            let mut iteration = 0;
            while remaining > 0 {
                iteration += 1;
                debug!(
                    "transfer_items_with_shulker: Deposit iteration {}, remaining: {}, moved so far: {}", 
                    iteration, remaining, total_moved
                );
                
                // Wait for container contents to sync from server
                tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
                
                let all_slots = shulker_container
                    .slots()
                    .ok_or_else(|| {
                        error!("transfer_items_with_shulker: Shulker closed during deposit");
                        "Shulker closed".to_string()
                    })?;
                let inv_end = all_slots.len();
                let mut found: Option<(usize, i32)> = None;
                // Search in BOTH inventory (27-53) AND hotbar (54-62) slots
                // Limit to actual container size
                for i in inv_start..inventory_end.min(inv_end) {
                    let stack = &all_slots[i];
                    if stack.count() > 0
                        && Bot::normalize_item_id(&stack.kind().to_string()) == target_id
                    {
                        found = Some((i, stack.count()));
                        let slot_type = if i < 54 { "inventory" } else { "hotbar" };
                        debug!(
                            "transfer_items_with_shulker: Found {} x{} in {} slot {} (container idx {})", 
                            stack.kind(), stack.count(), slot_type, if i >= 54 { i - 54 } else { i - 27 }, i
                        );
                        break;
                    }
                }

                let Some((slot, stack_count)) = found else {
                    info!(
                        "transfer_items_with_shulker: No more {} found in inventory/hotbar, stopping deposit", 
                        target_id
                    );
                    break;
                };

                // If we need the full stack or more, use shift-click for efficiency
                if remaining >= stack_count {
                    debug!(
                        "transfer_items_with_shulker: Shift-clicking slot {} ({} items, need {})", 
                        slot, stack_count, remaining
                    );
                    let moved =
                        super::inventory::quick_move_from_container(shulker_container, slot).await?;
                    if moved <= 0 {
                        // Shift-click reported 0 moved - could be timing issue
                        // Re-check the slot to see if items actually moved
                        tokio::time::sleep(tokio::time::Duration::from_millis(350)).await;
                        let slots_after = shulker_container
                            .slots()
                            .ok_or_else(|| "Shulker closed".to_string())?;
                        let current_count = slots_after.get(slot).map(|s| s.count()).unwrap_or(0);
                        
                        if current_count < stack_count {
                            // Items DID move, we just didn't detect it
                            let actual_moved = stack_count - current_count;
                            info!(
                                "transfer_items_with_shulker: Shift-click reported 0 but {} items actually moved (slot {} now has {})", 
                                actual_moved, slot, current_count
                            );
                            total_moved += actual_moved;
                            remaining -= actual_moved;
                            consecutive_failures = 0;
                        } else {
                            // Items really didn't move - retry
                            consecutive_failures += 1;
                            warn!(
                                "transfer_items_with_shulker: Shift-click moved 0 items (failure {}/3)", 
                                consecutive_failures
                            );
                            if consecutive_failures >= 3 {
                                error!("transfer_items_with_shulker: Shift-click failed 3 times in a row, stopping deposit");
                                break;
                            }
                        }
                        continue;
                    }
                    consecutive_failures = 0; // Reset on success
                    total_moved += moved;
                    remaining -= moved;
                    debug!(
                        "transfer_items_with_shulker: Shift-click moved {}, total: {}, remaining: {}", 
                        moved, total_moved, remaining
                    );
                } else {
                    // Need only a partial stack - use manual click transfer
                    info!(
                        "transfer_items_with_shulker: Partial transfer needed ({} from stack of {})", 
                        remaining, stack_count
                    );
                    // Pick up the stack from inventory
                    debug!("transfer_items_with_shulker: Picking up stack from slot {}", slot);
                    shulker_container.click(PickupClick::Left {
                        slot: Some(slot as u16),
                    });
                    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                    
                    // Find slots in shulker that can accept items (slots 0-26)
                    // Priority: 1) slots with same item type that have room, 2) empty slots
                    let shulker_size = shulker_contents.len(); // 27 slots
                    
                    // Build list of target slots with their available space
                    let mut target_slots: Vec<(usize, i32)> = Vec::new(); // (slot_index, space_available)
                    
                    for i in 0..shulker_size {
                        let slot_item = &all_slots[i];
                        if slot_item.count() == 0 {
                            // Empty slot - can hold up to 64
                            target_slots.push((i, 64));
                        } else if Bot::normalize_item_id(&slot_item.kind().to_string()) == target_id {
                            // Same item type - can add up to 64 - current
                            let space = 64 - slot_item.count();
                            if space > 0 {
                                target_slots.push((i, space));
                            }
                        }
                    }
                    
                    if target_slots.is_empty() {
                        // No space at all - put items back and let caller handle
                        warn!("transfer_items_with_shulker: Shulker is completely full - no space for partial transfer");
                        shulker_container.click(PickupClick::Left {
                            slot: Some(slot as u16),
                        });
                        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                        // Return what we've moved so far - caller will handle moving to next shulker
                        break;
                    }
                    
                    // Deposit items into available slots, filling each before moving to next
                    let mut items_to_place = remaining;
                    for (target_slot, space) in &target_slots {
                        if items_to_place <= 0 {
                            break;
                        }
                        let place_count = items_to_place.min(*space);
                        debug!(
                            "transfer_items_with_shulker: Right-clicking {} times to slot {} (has {} space)", 
                            place_count, target_slot, space
                        );
                        for _ in 0..place_count {
                            shulker_container.click(PickupClick::Right {
                                slot: Some(*target_slot as u16),
                            });
                            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                        }
                        items_to_place -= place_count;
                        total_moved += place_count;
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                    
                    // Calculate how many were actually placed
                    let placed = remaining - items_to_place;
                    remaining = items_to_place;
                    
                    // Put remaining items back in original slot (if any left on cursor)
                    debug!("transfer_items_with_shulker: Returning remaining items ({}) to slot {}", 
                        stack_count - placed, slot);
                    shulker_container.click(PickupClick::Left {
                        slot: Some(slot as u16),
                    });
                    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                    
                    // If we couldn't place everything, shulker is now full - break to let caller try next shulker
                    if remaining > 0 {
                        info!(
                            "transfer_items_with_shulker: Shulker filled during partial transfer, {} items remaining for next shulker",
                            remaining
                        );
                        break;
                    }
                }
            }
        }
        _ => {
            error!("transfer_items_with_shulker: Invalid direction: {}", direction);
            return Err("Invalid direction".to_string());
        }
    }

    info!(
        "transfer_items_with_shulker: {} complete - moved {} of {} requested", 
        direction, total_moved, amount
    );
    if total_moved < amount {
        warn!(
            "transfer_items_with_shulker: Incomplete transfer - only moved {}/{}", 
            total_moved, amount
        );
    }

    Ok(total_moved)
}

/// Read chest amounts by opening each shulker in the chest and reading its contents.
/// Returns a Vec<i32> of length 54, where each entry is the item count in that shulker slot.
pub async fn read_chest_amounts(
    bot: &Bot,
    chest_pos: BlockPos,
    item: &str,
    node_position: &Position,
) -> Result<Vec<i32>, String> {
    // CRITICAL: Move any items from hotbar to inventory BEFORE starting shulker operations
    // This ensures hotbar slot 0 is free for shulker placement
    info!("Clearing hotbar before read_chest_amounts to ensure slot 0 is available");
    if let Err(e) = super::inventory::move_hotbar_to_inventory(bot).await {
        warn!("Failed to clear hotbar before chest read: {} - proceeding anyway", e);
    }
    
    let mut container = open_chest_container(bot, chest_pos).await?;
    let target_id = Bot::normalize_item_id(item);

    let contents = container
        .contents()
        .ok_or_else(|| "Chest closed while reading contents".to_string())?;
    if contents.len() != 54 {
        return Err(format!(
            "Unexpected chest contents size: {}",
            contents.len()
        ));
    }

    let mut amounts = vec![0i32; 54];
    let station_pos = super::shulker::shulker_station_position(node_position);

    // For each slot in the chest
    for slot_idx in 0..54 {
        // Refresh contents to get current state
        let contents = container
            .contents()
            .ok_or_else(|| "Chest closed".to_string())?;
        if slot_idx >= contents.len() {
            continue;
        }

        let stack = &contents[slot_idx];
        if stack.count() <= 0 {
            continue; // Empty slot
        }

        let id = stack.kind().to_string();
        if !super::shulker::is_shulker_box(&id) {
            warn!("Chest slot {} contains non-shulker item: {}", slot_idx, id);
            continue;
        }

        // CRITICAL: Ensure cursor is empty before picking up shulker
        // If cursor has an item, clicking will swap instead of pick up
        container.click(PickupClick::LeftOutside);
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Take shulker from chest slot (pickup into hand)
        container.click(PickupClick::Left {
            slot: Some(slot_idx as u16),
        });
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

        // Place shulker on station - click on floor block below to place on top
        // In Minecraft, to place a block you right-click on an adjacent solid block
        let floor_block = BlockPos::new(station_pos.x, station_pos.y - 1, station_pos.z);
        let client = bot
            .client
            .read()
            .await
            .clone()
            .ok_or_else(|| "Bot not connected".to_string())?;
        // Look at the top face of the floor block (where we want to place the shulker)
        let place_vec3 = Vec3::new(
            station_pos.x as f64 + 0.5,
            station_pos.y as f64 - 0.4, // Look slightly below station Y to target floor's top face
            station_pos.z as f64 + 0.5,
        );
        client.look_at(place_vec3);
        tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
        client.block_interact(floor_block);
        tokio::time::sleep(tokio::time::Duration::from_millis(600)).await;

        // Open shulker
        let shulker_container = super::shulker::open_shulker_at_station(bot, &station_pos).await?;
        let shulker_contents = shulker_container
            .contents()
            .ok_or_else(|| "Shulker closed".to_string())?;

        // Read item count from shulker (all 27 slots, but we only care about the target item)
        let mut item_count = 0i32;
        for shulker_slot in shulker_contents.iter() {
            if shulker_slot.count() > 0
                && Bot::normalize_item_id(&shulker_slot.kind().to_string()) == target_id
            {
                item_count += shulker_slot.count();
            }
        }

        amounts[slot_idx] = item_count;

        // Close shulker
        shulker_container.close();
        tokio::time::sleep(tokio::time::Duration::from_millis(350)).await;

        // CRITICAL: Clear hotbar BEFORE picking up shulker from station
        // This ensures the shulker ends up in slot 0 when auto-picked up
        if let Err(e) = super::inventory::move_hotbar_to_inventory(bot).await {
            warn!("Failed to clear hotbar before shulker pickup (read_chest_amounts): {} - proceeding anyway", e);
        }

        // Pick up shulker from station (break block)
        super::shulker::pickup_shulker_from_station(bot, &station_pos, node_position).await?;

        // Re-open chest if it was closed (it might have closed when we broke the shulker)
        let chest_still_open = container.contents().is_some();
        if !chest_still_open {
            // Re-open chest
            drop(container);
            container = open_chest_container(bot, chest_pos).await?;
        }

        // Place shulker back in the same chest slot
        // Find shulker in inventory through the chest container's slots
        // When a chest is open, we CANNOT call open_inventory() separately
        // Double chest layout: slots 0-53 = chest, slots 54-80 = player inventory, slots 81-89 = hotbar
        let all_slots = container
            .slots()
            .ok_or_else(|| "Chest closed while looking for shulker".to_string())?;
        let chest_size = container
            .contents()
            .ok_or_else(|| "Chest closed".to_string())?
            .len();

        // Search for shulker in player inventory portion of chest view (slots after chest contents)
        let mut shulker_in_container_view: Option<usize> = None;
        for i in chest_size..all_slots.len() {
            let slot_item = &all_slots[i];
            if slot_item.count() > 0
                && super::shulker::is_shulker_box(&slot_item.kind().to_string())
            {
                shulker_in_container_view = Some(i);
                break;
            }
        }

        if let Some(container_slot) = shulker_in_container_view {
            // Pick up shulker from the container view's inventory portion
            container.click(PickupClick::Left {
                slot: Some(container_slot as u16),
            });
            tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;

            // Place back in chest slot
            container.click(PickupClick::Left {
                slot: Some(slot_idx as u16),
            });
            tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
            
            // Only close/reopen if there might be more slots to process
            if slot_idx < 53 {
                // Close and reopen chest to ensure clean state for next iteration
                // close() takes ownership, so we must reopen immediately
                container.close();
                tokio::time::sleep(tokio::time::Duration::from_millis(350)).await;
                container = open_chest_container(bot, chest_pos).await?;
            }
        } else {
            warn!(
                "Could not find shulker in inventory (via chest container view) to place back in slot {}",
                slot_idx
            );
        }
    }

    Ok(amounts)
}

/// Automated chest I/O with full shulker handling.
///
/// **Model**: Each chest slot contains 1 shulker box. Items are stored **inside** the shulkers.
///
/// **Withdraw Flow** (`direction == "withdraw"`):
/// 1. Open chest
/// 2. For each shulker containing the target item:
///    a. Take shulker from chest slot
///    b. Place on shulker station
///    c. Open shulker
///    d. Transfer items from shulker to bot inventory (shift-click)
///    e. Close shulker, pick it up
///    f. Place shulker back in same chest slot
/// 3. Continue until `amount` items transferred
///
/// **Deposit Flow** (`direction == "deposit"`):
/// 1. Open chest
/// 2. For each shulker with space for the target item:
///    a. Take shulker from chest slot
///    b. Place on shulker station
///    c. Open shulker
///    d. Transfer items from bot inventory to shulker (shift-click)
///    e. Close shulker, pick it up
///    f. Place shulker back in same chest slot
/// 3. Continue until `amount` items deposited
///
/// **Error Handling**: If chest closes during operation, reopens it.
/// If shulker not found in inventory after breaking, logs warning.
///
/// **Note**: This function assumes the bot is already at the node position.
/// Navigation should be handled by the caller.
///
/// **Returns**: A Vec<i32> of length 54 containing item counts for each shulker slot.
/// Slots that were processed contain the accurate count after the operation.
/// Slots that were NOT processed contain -1 (caller should keep existing values for those).
///
/// **known_counts**: Optional pre-existing knowledge about shulker contents.
/// If provided, slots with known_counts[i] == 0 will be skipped for withdrawals (known empty).
/// For deposits, slots at or above shulker capacity (27 × stack_size) will be skipped.
/// This avoids needlessly taking out and placing back shulkers that are known to be empty/full.
///
/// **stack_size**: The item's maximum stack size (1, 16, or 64). Used to calculate shulker capacity.
pub async fn automated_chest_io(
    bot: &Bot,
    chest_pos: BlockPos,
    item: &str,
    amount: i32,
    direction: &str,
    node_position: &Position,
    known_counts: Option<&Vec<i32>>,
    stack_size: i32,
) -> Result<Vec<i32>, String> {
    // CRITICAL: Ensure entity is fully initialized before any inventory operations
    // This prevents panic: "Our client is missing a required component: Inventory"
    let client = bot.client.read().await.clone().ok_or_else(|| "Bot not connected".to_string())?;
    if !super::inventory::is_entity_ready(&client) {
        warn!("Entity not ready, waiting for initialization...");
        super::inventory::wait_for_entity_ready(&client).await?;
        info!("Entity now ready for chest operations");
    }
    
    // CRITICAL: Verify bot is at the EXACT node position before chest operations
    // This ensures the bot navigated correctly and is at the right location
    // Zero tolerance - bot must be exactly on the node P position
    {
        let current_pos = client.entity().position();
        let current_block = azalea::BlockPos::from(current_pos);
        let target_block = azalea::BlockPos::new(node_position.x, node_position.y, node_position.z);
        
        let dx = (current_block.x - target_block.x).abs();
        let dy = (current_block.y - target_block.y).abs();
        let dz = (current_block.z - target_block.z).abs();
        
        // Zero tolerance: must be at exact node position
        if dx != 0 || dy != 0 || dz != 0 {
            error!(
                "Position verification FAILED: Bot at ({}, {}, {}) but must be at EXACT node position ({}, {}, {}) - offset ({}, {}, {})",
                current_block.x, current_block.y, current_block.z,
                node_position.x, node_position.y, node_position.z,
                dx, dy, dz
            );
            return Err(format!(
                "Bot not at exact node position: current ({}, {}, {}), required ({}, {}, {}). Navigation may have failed.",
                current_block.x, current_block.y, current_block.z,
                node_position.x, node_position.y, node_position.z
            ));
        }
        
        info!(
            "Position verified: bot at exact node position ({}, {}, {})",
            current_block.x, current_block.y, current_block.z
        );
    }
    
    drop(client); // Release the reference
    
    // CRITICAL: Move any items from hotbar to inventory BEFORE starting chest operations
    // This ensures hotbar slot 0 is free for shulker placement
    // Without this, if items (like diamonds from a failed trade) are in hotbar slot 0,
    // the shulker operations get messy and can fail
    info!("Clearing hotbar before chest operations to ensure slot 0 is available");
    if let Err(e) = super::inventory::move_hotbar_to_inventory(bot).await {
        warn!("Failed to clear hotbar before chest operations: {} - proceeding anyway", e);
    }
    
    // Initialize counts: use known_counts if provided, otherwise -1 (meaning "not checked/unchanged")
    let mut slot_counts: Vec<i32> = if let Some(known) = known_counts {
        known.clone()
    } else {
        vec![-1; 54]
    };

    // Track which slots we've CONFIRMED empty during THIS operation.
    // This is deliberately separate from `slot_counts` / `known_counts`:
    // a `known_counts[i] == 0` is ambiguous (it could mean "never checked" just
    // as easily as "verified empty"), so we only skip slots we opened ourselves
    // this run. This is the withdraw-side counterpart to `confirmed_full` below.
    let mut confirmed_empty: std::collections::HashSet<usize> = std::collections::HashSet::new();
    
    if amount <= 0 {
        return Ok(slot_counts);
    }

    let target_id = Bot::normalize_item_id(item);
    info!(
        "Starting {} operation: {} items of {} at chest {:?}",
        direction, amount, item, chest_pos
    );
    let mut container = open_chest_container(bot, chest_pos).await?;
    let station_pos = super::shulker::shulker_station_position(node_position);
    info!(
        "Chest opened successfully, shulker station at {:?}",
        station_pos
    );

    let mut remaining = amount;

    match direction {
        "withdraw" => {
            // Find shulkers that contain the target item.
            // Outer loop: restart the scan from slot 0 whenever we still need more items.
            // `checked_slots` tracks slots visited in the CURRENT pass so we don't revisit
            // them mid-pass; at the end of each pass we retain only the confirmed-empty ones
            // (below), so an already-drained shulker stays skipped across passes but a shulker
            // we only partially drained gets another look if we still need more.
            let mut all_shulkers_checked = false;
            let mut checked_slots = std::collections::HashSet::new();

            while remaining > 0 && !all_shulkers_checked {
                all_shulkers_checked = true; // Assume we're done, set to false if we find a shulker

                for slot_idx in 0..54 {
                    if remaining <= 0 {
                        break;
                    }

                    // Skip already checked slots in this iteration
                    if checked_slots.contains(&slot_idx) {
                        continue;
                    }

                    // Skip slots CONFIRMED empty during THIS operation
                    // We don't trust pre-existing 0s from known_counts (could mean "never checked")
                    if confirmed_empty.contains(&slot_idx) {
                        debug!("Skipping slot {}: confirmed empty this operation", slot_idx);
                        continue;
                    }

                    // Refresh contents to get current state
                    let contents = container
                        .contents()
                        .ok_or_else(|| "Chest closed".to_string())?;
                    if slot_idx >= contents.len() {
                        continue;
                    }

                    let stack = &contents[slot_idx];
                    if stack.count() <= 0
                        || !super::shulker::is_shulker_box(&stack.kind().to_string())
                    {
                        continue;
                    }

                    all_shulkers_checked = false; // Found a shulker, not done yet
                    checked_slots.insert(slot_idx);

                    // Per-shulker sequence for one withdraw pass:
                    //   1. clear cursor, take shulker out of the chest slot into cursor
                    //   2. close the chest (can't open the player inventory while a chest is open)
                    //   3. move the shulker into hotbar slot 0 so the bot can place it
                    //   4. right-click the floor block at the station to place the shulker
                    //   5. open the placed shulker as a container
                    //   6. shift-click target items from the shulker into bot inventory
                    //   7. close the shulker, clear hotbar, break+pick up the shulker block
                    //   8. reopen the chest, find the shulker in the chest-view inventory,
                    //      and put it back into the same chest slot
                    // CRITICAL: Ensure cursor is empty before picking up shulker -
                    // if the cursor already holds something the click becomes a swap, not a pickup.
                    info!("Clearing cursor before picking up shulker");
                    container.click(PickupClick::LeftOutside);
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

                    // Take shulker from chest
                    container.click(PickupClick::Left {
                        slot: Some(slot_idx as u16),
                    });
                    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

                    // IMPORTANT: Close chest FIRST before inventory operations
                    // Can't open inventory while a container is open
                    info!("Closing chest to allow inventory operations");
                    drop(container);
                    // CRITICAL: Wait longer for server to sync inventory state after chest closes
                    // The shulker that was in cursor needs time to appear in the inventory
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                    // Get client for inventory operations
                    let client = bot
                        .client
                        .read()
                        .await
                        .clone()
                        .ok_or_else(|| "Bot not connected".to_string())?;

                    // CRITICAL: Ensure shulker is in hotbar slot 0 before placing
                    info!("Ensuring shulker is in hotbar slot 0 before placing on station");
                    if let Err(e) = super::inventory::ensure_shulker_in_hotbar_slot_0(bot).await {
                        return Err(format!("Failed to ensure shulker in hotbar slot 0: {}", e));
                    }

                    // Verify shulker is in cursor/hotbar slot 0 before placing
                    if !super::inventory::verify_holding_shulker(&client) {
                        // Try picking up from hotbar slot 0
                        let inv_handle = client
                            .open_inventory()
                            .ok_or_else(|| "Failed to open inventory".to_string())?;
                        inv_handle.click(PickupClick::Left {
                            slot: Some(36 as u16), // Hotbar slot 0
                        });
                        tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
                        drop(inv_handle);

                        if !super::inventory::verify_holding_shulker(&client) {
                            return Err(format!(
                                "Bot is not holding shulker before placing on station (withdraw from slot {})",
                                slot_idx
                            ));
                        }
                    }

                    // Place on station - click on floor block below to place on top
                    // In Minecraft, to place a block you right-click on an adjacent solid block
                    let floor_block = BlockPos::new(station_pos.x, station_pos.y - 1, station_pos.z);
                    // Look at the top face of the floor block (where we want to place the shulker)
                    let place_vec3 = Vec3::new(
                        station_pos.x as f64 + 0.5,
                        station_pos.y as f64 - 0.4, // Look slightly below station Y to target floor's top face
                        station_pos.z as f64 + 0.5,
                    );
                    client.look_at(place_vec3);
                    tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
                    client.block_interact(floor_block);
                    tokio::time::sleep(tokio::time::Duration::from_millis(600)).await;

                    // Open shulker
                    let shulker_container =
                        super::shulker::open_shulker_at_station(bot, &station_pos).await?;

                    // Check if shulker contains target item and count how much
                    let shulker_contents = shulker_container
                        .contents()
                        .ok_or_else(|| "Shulker closed".to_string())?;
                    let mut shulker_item_count = 0i32;
                    for sh_slot in shulker_contents.iter() {
                        if sh_slot.count() > 0
                            && Bot::normalize_item_id(&sh_slot.kind().to_string()) == target_id
                        {
                            shulker_item_count += sh_slot.count();
                        }
                    }

                    if shulker_item_count > 0 {
                        // Transfer items from shulker to bot inventory (up to remaining amount)
                        let to_withdraw = remaining.min(shulker_item_count);
                        info!(
                            "Shulker contains {} items, withdrawing {} (need {})",
                            shulker_item_count, to_withdraw, remaining
                        );
                        let moved = transfer_items_with_shulker(
                            bot,
                            &shulker_container,
                            item,
                            to_withdraw,
                            "withdraw",
                        )
                        .await?;
                        info!("Withdrew {} items from shulker", moved);
                        
                        // Record the count after withdrawal
                        let remaining_in_slot = shulker_item_count - moved;
                        slot_counts[slot_idx] = remaining_in_slot;
                        
                        // If slot is now empty, mark it as confirmed empty
                        if remaining_in_slot == 0 {
                            confirmed_empty.insert(slot_idx);
                        }
                        
                        if moved > 0 {
                            remaining -= moved;
                        } else {
                            warn!("No items were withdrawn from shulker");
                        }
                    } else {
                        // Shulker doesn't contain target item - record 0 and mark confirmed empty
                        slot_counts[slot_idx] = 0;
                        confirmed_empty.insert(slot_idx);
                        info!(
                            "Shulker does not contain target item {}, skipping",
                            target_id
                        );
                    }

                    // Close shulker
                    shulker_container.close();
                    tokio::time::sleep(tokio::time::Duration::from_millis(350)).await;

                    // CRITICAL: Clear hotbar BEFORE picking up shulker from station
                    // Shift-click transfers may have put items in hotbar, which would cause
                    // the shulker to not end up in slot 0 when auto-picked up
                    if let Err(e) = super::inventory::move_hotbar_to_inventory(bot).await {
                        warn!("Failed to clear hotbar before shulker pickup (withdraw): {} - proceeding anyway", e);
                    }

                    // Pick up shulker
                    super::shulker::pickup_shulker_from_station(bot, &station_pos, node_position)
                        .await?;

                    // Re-open chest (it was closed earlier before inventory operations)
                    container = open_chest_container(bot, chest_pos).await?;

                    // Find shulker in inventory through the chest container's slots
                    // When a chest is open, we CANNOT call open_inventory() separately
                    // Double chest layout: slots 0-53 = chest, slots 54-80 = player inventory, slots 81-89 = hotbar
                    let all_slots = container
                        .slots()
                        .ok_or_else(|| "Chest closed while looking for shulker".to_string())?;
                    let chest_size = container
                        .contents()
                        .ok_or_else(|| "Chest closed".to_string())?
                        .len();

                    // Search for shulker in player inventory portion of chest view (slots after chest contents)
                    let mut shulker_in_container_view: Option<usize> = None;
                    for i in chest_size..all_slots.len() {
                        let slot_item = &all_slots[i];
                        if slot_item.count() > 0
                            && super::shulker::is_shulker_box(&slot_item.kind().to_string())
                        {
                            shulker_in_container_view = Some(i);
                            break;
                        }
                    }

                    if let Some(container_slot) = shulker_in_container_view {
                        // Pick up shulker from the container view's inventory portion
                        container.click(PickupClick::Left {
                            slot: Some(container_slot as u16),
                        });
                        tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;

                        // Place back in chest slot
                        container.click(PickupClick::Left {
                            slot: Some(slot_idx as u16),
                        });
                        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                        
                        // Only close/reopen if we need to continue processing
                        if remaining > 0 {
                            // Close and reopen chest to ensure clean state for next iteration
                            // close() takes ownership, so we must reopen immediately
                            container.close();
                            tokio::time::sleep(tokio::time::Duration::from_millis(350)).await;
                            container = open_chest_container(bot, chest_pos).await?;
                        }
                    } else {
                        warn!(
                            "Could not find shulker in inventory (via chest container view) to place back in chest slot {}",
                            slot_idx
                        );
                    }
                }

                // End of one outer pass over the 54 chest slots.
                // If we still need more items, reset `checked_slots` so the next pass can
                // revisit shulkers. BUT keep the confirmed-empty ones in the set so they
                // stay skipped across passes - there's no point in re-opening a shulker we
                // already drained this operation.
                if remaining > 0 {
                    checked_slots.retain(|&slot| confirmed_empty.contains(&slot));
                }
            }
        }
        "deposit" => {
            // Track which slots we've CONFIRMED full during THIS operation
            // This is separate from known_counts because we verify during operation
            let mut confirmed_full: std::collections::HashSet<usize> = std::collections::HashSet::new();
            
            // First, check if chest has any shulkers at all
            let contents = container
                .contents()
                .ok_or_else(|| "Chest closed".to_string())?;
            let mut has_any_shulker = false;
            for slot_idx in 0..contents.len().min(54) {
                let stack = &contents[slot_idx];
                if stack.count() > 0 && super::shulker::is_shulker_box(&stack.kind().to_string()) {
                    has_any_shulker = true;
                    break;
                }
            }

            if !has_any_shulker {
                warn!(
                    "Chest has no shulkers! Cannot deposit. Chest should be pre-filled with shulkers."
                );
                return Err("Chest has no shulkers - cannot deposit items".to_string());
            }

            info!("Found shulkers in chest, starting deposit process");

            // Calculate shulker capacity based on item's stack size (27 slots × stack_size)
            let shulker_capacity = crate::types::Pair::shulker_capacity_for_stack_size(stack_size);
            
            for slot_idx in 0..54 {
                if remaining <= 0 {
                    break;
                }

                // Skip slots CONFIRMED full during THIS operation
                if confirmed_full.contains(&slot_idx) {
                    debug!("Skipping slot {}: confirmed full this operation", slot_idx);
                    continue;
                }

                // Skip slots KNOWN to be at max capacity from `known_counts`.
                // Unlike the withdraw side (where we distrust a stored 0 because it's
                // ambiguous with "never checked"), a stored count >= capacity is
                // unambiguous: the shulker physically cannot hold more. Skipping here
                // avoids the huge cost of taking the shulker out, placing, opening,
                // discovering it's full, and putting it back.
                if let Some(known) = known_counts {
                    if let Some(&count) = known.get(slot_idx) {
                        if count >= shulker_capacity {
                            debug!(
                                "Skipping slot {}: known full with {} items (max {})",
                                slot_idx, count, shulker_capacity
                            );
                            confirmed_full.insert(slot_idx);
                            continue;
                        }
                    }
                }

                // Ensure chest is open (it might have been closed in previous iteration)
                // Check if container is still valid, if not reopen it
                let chest_open = container.contents().is_some();
                if !chest_open {
                    drop(container);
                    container = open_chest_container(bot, chest_pos).await?;
                }

                // Refresh contents to get current state
                let contents = container
                    .contents()
                    .ok_or_else(|| "Chest closed".to_string())?;
                if slot_idx >= contents.len() {
                    continue;
                }

                let stack = &contents[slot_idx];
                if stack.count() <= 0 || !super::shulker::is_shulker_box(&stack.kind().to_string())
                {
                    debug!("Skipping slot {}: empty or not a shulker", slot_idx);
                    continue;
                }

                info!("Processing shulker in slot {} for deposit", slot_idx);

                // Per-shulker sequence for deposit (mirror of the withdraw sequence):
                //   take shulker from chest -> close chest -> ensure shulker in hotbar 0 ->
                //   place on station -> open shulker -> shift-click items from player inv/hotbar
                //   into shulker -> close shulker -> clear hotbar -> break+pickup shulker ->
                //   reopen chest -> place shulker back in the same chest slot (verified).
                // CRITICAL: Ensure cursor is empty before picking up shulker -
                // if cursor has an item, clicking becomes a swap instead of a pickup.
                info!("Clearing cursor before picking up shulker");
                container.click(PickupClick::LeftOutside);
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

                // Take shulker from chest into cursor
                info!("Taking shulker from chest slot {}", slot_idx);
                container.click(PickupClick::Left {
                    slot: Some(slot_idx as u16),
                });
                tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

                // IMPORTANT: Close chest FIRST to ensure shulker stays in cursor
                // If we open inventory while chest is open, the cursor might be lost
                info!("Closing chest to preserve shulker in cursor");
                drop(container);
                // CRITICAL: Wait longer for server to sync inventory state after chest closes
                // The shulker that was in cursor needs time to appear in the inventory
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                // Get client for inventory operations
                let client = bot
                    .client
                    .read()
                    .await
                    .clone()
                    .ok_or_else(|| "Bot not connected".to_string())?;

                // CRITICAL: Use ensure_shulker_in_hotbar_slot_0() to handle all cases
                info!("Ensuring shulker is in hotbar slot 0 before placing on station");
                if let Err(e) = super::inventory::ensure_shulker_in_hotbar_slot_0(bot).await {
                    return Err(format!("Failed to ensure shulker in hotbar slot 0: {}", e));
                }

                // Verify bot is holding shulker before placing
                const HOTBAR_SLOT_0: usize = 36; // Hotbar slot 0 is inventory slot 36
                if !super::inventory::verify_holding_shulker(&client) {
                    // Shulker should be in hotbar slot 0, pick it up
                    let inv_handle = client
                        .open_inventory()
                        .ok_or_else(|| "Failed to open inventory".to_string())?;
                    inv_handle.click(PickupClick::Left {
                        slot: Some(HOTBAR_SLOT_0 as u16),
                    });
                    tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
                    drop(inv_handle);
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

                    // Verify again
                    if !super::inventory::verify_holding_shulker(&client) {
                        return Err(
                            "Bot is not holding shulker before placing on station".to_string()
                        );
                    }
                }

                // The shulker station position is where we want to place the shulker (same Y as node)
                // In Minecraft, to place a block you right-click on an adjacent solid block
                // We click on the floor block below to place the shulker on top
                let floor_block = BlockPos::new(station_pos.x, station_pos.y - 1, station_pos.z);

                // Look at the top face of the floor block (where we want to place the shulker)
                let place_vec3 = Vec3::new(
                    station_pos.x as f64 + 0.5,
                    station_pos.y as f64 - 0.4, // Look slightly below station Y to target floor's top face
                    station_pos.z as f64 + 0.5,
                );
                client.look_at(place_vec3);
                tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

                // Right-click on the floor block to place the shulker on top
                // block_interact performs a right-click interaction with the item in hand/cursor
                info!("Placing shulker on station by clicking floor block at {:?}", floor_block);
                client.block_interact(floor_block);
                tokio::time::sleep(tokio::time::Duration::from_millis(750)).await;

                // Open shulker (don't reopen chest yet - we'll do that after closing shulker if needed)
                info!("Opening shulker at station");
                let shulker_container =
                    super::shulker::open_shulker_at_station(bot, &station_pos).await?;
                info!("Shulker opened successfully");

                // Check if shulker has space (can hold target item) and count existing items
                // Use the correct stack size for calculating available space
                let shulker_contents = shulker_container
                    .contents()
                    .ok_or_else(|| "Shulker closed".to_string())?;
                let mut total_space = 0i32;
                let mut initial_item_count = 0i32;
                for sh_slot in shulker_contents.iter() {
                    if sh_slot.count() <= 0 {
                        total_space += stack_size; // Empty slot can hold stack_size items
                    } else if Bot::normalize_item_id(&sh_slot.kind().to_string()) == target_id {
                        total_space += (stack_size - sh_slot.count()).max(0); // Space in existing stack
                        initial_item_count += sh_slot.count();
                    }
                }

                info!(
                    "Shulker has {} space for {} (need {}), currently contains {}",
                    total_space, target_id, remaining, initial_item_count
                );

                // Check if bot has items in inventory or hotbar before trying to transfer
                let all_slots = shulker_container
                    .slots()
                    .ok_or_else(|| "Shulker closed".to_string())?;
                let inv_start = shulker_contents.len(); // Bot inventory starts after shulker contents (27)
                // Search BOTH inventory (27-53) AND hotbar (54-62) - items can be anywhere after a trade
                let inventory_and_hotbar_end = inv_start + 36; // 27 inventory + 9 hotbar = 36 slots
                let mut bot_item_count = 0i32;
                for i in inv_start..inventory_and_hotbar_end.min(all_slots.len()) {
                    let stack = &all_slots[i];
                    if stack.count() > 0
                        && Bot::normalize_item_id(&stack.kind().to_string()) == target_id
                    {
                        bot_item_count += stack.count();
                    }
                }
                info!(
                    "Bot has {} items of {} in inventory+hotbar (slots 27-62)",
                    bot_item_count, target_id
                );

                if total_space > 0 && remaining > 0 && bot_item_count > 0 {
                    // Transfer items from bot inventory to shulker
                    let to_deposit = remaining.min(total_space).min(bot_item_count);
                    info!(
                        "Transferring {} items from bot inventory to shulker",
                        to_deposit
                    );
                    let moved = transfer_items_with_shulker(
                        bot,
                        &shulker_container,
                        item,
                        to_deposit,
                        "deposit",
                    )
                    .await?;
                    info!("Transferred {} items into shulker", moved);
                    
                    // Record the count after deposit
                    slot_counts[slot_idx] = initial_item_count + moved;
                    
                    if moved > 0 {
                        remaining -= moved;
                    } else {
                        warn!(
                            "No items were transferred, shulker may be full or bot inventory empty"
                        );
                        // If no items were moved but we have space and items, this is an error
                        if total_space > 0 && bot_item_count > 0 {
                            return Err(format!(
                                "Failed to transfer items to shulker despite having {} space and {} items in inventory",
                                total_space, bot_item_count
                            ));
                        }
                    }
                } else {
                    if bot_item_count == 0 {
                        warn!("Bot has no items of {} in inventory to deposit", target_id);
                        return Err(format!(
                            "Bot inventory is empty - no items of {} to deposit",
                            target_id
                        ));
                    }
                    if total_space == 0 {
                        // Shulker is full or contains different items - put it back and try next shulker
                        // Record the current count (might be full of target item, or 0 if different items)
                        slot_counts[slot_idx] = initial_item_count;
                        // Mark as confirmed full so we skip it if we somehow revisit
                        confirmed_full.insert(slot_idx);
                        info!(
                            "Shulker has no space for {} (full or contains different items), putting back and trying next shulker",
                            target_id
                        );
                        // Close shulker first
                        shulker_container.close();
                        // Longer delay after close to let server process the close event
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                        // CRITICAL: Clear hotbar BEFORE picking up shulker from station
                        // This ensures the shulker ends up in slot 0 when auto-picked up
                        if let Err(e) = super::inventory::move_hotbar_to_inventory(bot).await {
                            warn!("Failed to clear hotbar before shulker pickup (deposit-full): {} - proceeding anyway", e);
                        }

                        // Pick up shulker and put it back
                        super::shulker::pickup_shulker_from_station(
                            bot,
                            &station_pos,
                            node_position,
                        )
                        .await?;

                        // Re-open chest (it was closed when we placed shulker)
                        container = open_chest_container(bot, chest_pos).await?;

                        // Find shulker in inventory through the chest container's slots
                        // When a chest is open, we CANNOT call open_inventory() separately
                        let all_slots = container
                            .slots()
                            .ok_or_else(|| "Chest closed while looking for shulker".to_string())?;
                        let chest_size = container
                            .contents()
                            .ok_or_else(|| "Chest closed".to_string())?
                            .len();

                        // Search for shulker in player inventory portion of chest view
                        let mut shulker_in_container_view: Option<usize> = None;
                        for i in chest_size..all_slots.len() {
                            let slot_item = &all_slots[i];
                            if slot_item.count() > 0
                                && super::shulker::is_shulker_box(&slot_item.kind().to_string())
                            {
                                shulker_in_container_view = Some(i);
                                break;
                            }
                        }

                        if let Some(container_slot) = shulker_in_container_view {
                            place_shulker_in_chest_slot_verified(
                                &container,
                                container_slot,
                                slot_idx,
                            )
                            .await?;
                            
                            // We're about to continue to the next slot, so close and reopen
                            // close() takes ownership, so we must reopen immediately
                            container.close();
                            tokio::time::sleep(tokio::time::Duration::from_millis(350)).await;
                            container = open_chest_container(bot, chest_pos).await?;
                        } else {
                            warn!(
                                "Could not find shulker in inventory (via chest container view) to place back"
                            );
                        }

                        // Continue to next shulker
                        continue;
                    } else {
                        info!("No remaining items to deposit (remaining: {})", remaining);
                    }
                }

                // Close shulker
                info!("Closing shulker");
                shulker_container.close();
                // Longer delay after close to let server process the close event
                // and avoid race conditions with container content events
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                // CRITICAL: Clear hotbar BEFORE picking up shulker from station
                // This ensures the shulker ends up in slot 0 when auto-picked up
                if let Err(e) = super::inventory::move_hotbar_to_inventory(bot).await {
                    warn!("Failed to clear hotbar before shulker pickup (deposit): {} - proceeding anyway", e);
                }

                // Pick up shulker
                info!("Picking up shulker from station");
                super::shulker::pickup_shulker_from_station(bot, &station_pos, node_position)
                    .await?;
                info!("Shulker picked up successfully");

                // Re-open chest (it was closed when we placed shulker)
                info!("Reopening chest to place shulker back");
                container = open_chest_container(bot, chest_pos).await?;

                // Find shulker in inventory through the chest container's slots
                // When a chest is open, we CANNOT call open_inventory() separately
                // Double chest layout: slots 0-53 = chest, slots 54-80 = player inventory, slots 81-89 = hotbar
                let all_slots = container
                    .slots()
                    .ok_or_else(|| "Chest closed while looking for shulker".to_string())?;
                let chest_size = container
                    .contents()
                    .ok_or_else(|| "Chest closed".to_string())?
                    .len();

                // Search for shulker in player inventory portion of chest view (slots after chest contents)
                let mut shulker_in_container_view: Option<usize> = None;
                for i in chest_size..all_slots.len() {
                    let slot_item = &all_slots[i];
                    if slot_item.count() > 0
                        && super::shulker::is_shulker_box(&slot_item.kind().to_string())
                    {
                        shulker_in_container_view = Some(i);
                        break;
                    }
                }

                if let Some(container_slot) = shulker_in_container_view {
                    info!("Trying to place shulker into chest slot {}", slot_idx);
                    place_shulker_in_chest_slot_verified(&container, container_slot, slot_idx)
                        .await?;
                    
                    // Only close/reopen if we need to process more slots
                    // If remaining is 0 or we're at the last slot, we're done
                    if remaining > 0 && slot_idx < 53 {
                        // Close and reopen chest to ensure clean state for next iteration
                        // close() takes ownership, so we must reopen immediately
                        info!("Closing chest after placing shulker back to ensure clean state");
                        container.close();
                        tokio::time::sleep(tokio::time::Duration::from_millis(350)).await;
                        container = open_chest_container(bot, chest_pos).await?;
                    }
                } else {
                    warn!(
                        "Could not find shulker in inventory (via chest container view) to place back in chest slot {}",
                        slot_idx
                    );
                }
            }
        }
        _ => return Err("Invalid chest IO direction".to_string()),
    }

    if remaining > 0 {
        return Err(format!(
            "Incomplete chest IO: moved {}, needed {}",
            amount - remaining,
            amount
        ));
    }

    info!("Chest IO complete, returning counts for {} processed slots", 
          slot_counts.iter().filter(|&&c| c >= 0).count());
    Ok(slot_counts)
}
