//! Chest I/O operations with shulker handling
//!
//! Provides chest interaction and shulker manipulation with automatic retry logic.

use azalea::BlockPos;
use azalea::Vec3;
use azalea::inventory::operations::PickupClick;
use tracing::{debug, error, info, warn};

use super::Bot;
use crate::constants::{
    CHEST_OP_MAX_RETRIES, CHUNK_RELOAD_BASE_DELAY_MS, CHUNK_RELOAD_EXTRA_RETRIES,
    CHUNK_RELOAD_MAX_DELAY_MS, DELAY_BLOCK_OP_MS, DELAY_INTERACT_MS, DELAY_LOOK_AT_MS,
    DELAY_MEDIUM_MS, DELAY_SETTLE_MS, DELAY_SHORT_MS, DELAY_SHULKER_PLACE_MS,
    DOUBLE_CHEST_SLOTS, HOTBAR_SLOT_0, RETRY_BASE_DELAY_MS, RETRY_MAX_DELAY_MS,
    SHULKER_BOX_SLOTS, exponential_backoff_delay,
};
use crate::types::Position;

/// Error prefix used to tag chunk-not-loaded / transient world-state failures.
/// `open_chest_container` checks for this prefix to apply longer backoff and
/// extra retries, since chunks typically reload within ~10 seconds on most
/// servers. Permanent errors (wrong block type, bot disconnected) do NOT carry
/// this prefix and are retried with the normal shorter cadence.
const CHUNK_NOT_LOADED_PREFIX: &str = "[chunk-not-loaded] ";

/// Locate the first shulker box in the player-inventory portion of an open
/// double-chest container view.
///
/// Double-chest slot layout:
/// - `0..54`  = chest slots
/// - `54..81` = player inventory (27 slots)
/// - `81..90` = player hotbar (9 slots)
///
/// After a shulker is picked up from the station and auto-deposited into
/// the bot's inventory, the bot must reopen the chest and find it again in
/// the player-inventory portion of the container view in order to place it
/// back into its source slot. This helper is the single shared
/// implementation of that search (it used to be copy-pasted three times).
pub fn find_shulker_in_inventory_view(
    container: &azalea::container::ContainerHandle,
) -> Result<Option<usize>, String> {
    let all_slots = container
        .slots()
        .ok_or_else(|| "Chest closed while looking for shulker".to_string())?;
    let chest_size = container
        .contents()
        .ok_or_else(|| "Chest closed".to_string())?
        .len();
    for (i, slot_item) in all_slots.iter().enumerate().skip(chest_size) {
        if slot_item.count() > 0
            && super::shulker::is_shulker_box(&slot_item.kind().to_string())
        {
            return Ok(Some(i));
        }
    }
    Ok(None)
}

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
    tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_INTERACT_MS)).await;

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
            tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_INTERACT_MS)).await;
            
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
    container.click(PickupClick::Left {
        slot: Some(chest_slot as u16),
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_INTERACT_MS)).await;

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
            let slot_type = if idx < DOUBLE_CHEST_SLOTS { "chest" } else { "inventory/hotbar" };
            debug!("  {} slot {}: {}", slot_type, idx, slot.kind());
        }
    }

    // Verify the shulker is now in the chest slot
    let mut verified = false;
    for attempt in 0..MAX_VERIFICATION_ATTEMPTS {
        let updated_slots = container
            .slots()
            .ok_or_else(|| "Container closed while verifying shulker placement".to_string())?;

        if let Some(chest_item) = updated_slots.get(chest_slot)
            && chest_item.count() > 0
                && super::shulker::is_shulker_box(&chest_item.kind().to_string())
            {
                verified = true;
                break;
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
        container.click(PickupClick::Left {
            slot: Some(chest_slot as u16),
        });
        tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_INTERACT_MS)).await;

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
                return Ok(());
            }
        }

        // Log final shulker locations for debugging
        error!("place_shulker_in_chest_slot_verified: Recovery FAILED - shulker locations:");
        for (idx, slot) in final_slots.iter().enumerate() {
            if slot.count() > 0 && super::shulker::is_shulker_box(&slot.kind().to_string()) {
                let slot_type = if idx < DOUBLE_CHEST_SLOTS { "chest" } else { "inventory/hotbar" };
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

    // Check block state before opening — distinguish chunk-not-loaded (transient)
    // from wrong-block-type (likely permanent).
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
            // Block state is None — the chunk containing this block is not loaded.
            // This is a transient condition: after a server restart or when the bot
            // is teleported, chunks take a few seconds to stream in. We tag the
            // error so the retry loop can apply a longer backoff.
            warn!(
                "open_chest_container_once: Block state at ({}, {}, {}) is None (chunk not loaded)",
                chest_pos.x, chest_pos.y, chest_pos.z
            );
            return Err(format!(
                "{}Block state at ({}, {}, {}) is None - chunk not loaded",
                CHUNK_NOT_LOADED_PREFIX,
                chest_pos.x, chest_pos.y, chest_pos.z
            ));
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

    let container = client
        .open_container_at_with_timeout_ticks(chest_pos, Some(timeout_ticks))
        .await;

    match container {
        Some(c) => Ok(c),
        None => {
            // Re-check block state: if the chunk was unloaded between our initial
            // check and the timeout, the open failed because the block entity
            // vanished — tag as transient so the retry loop waits for the chunk.
            let world = client.world();
            let still_loaded = world.read().get_block_state(chest_pos).is_some();
            let prefix = if still_loaded { "" } else { CHUNK_NOT_LOADED_PREFIX };
            Err(format!(
                "{}Failed to open chest at ({}, {}, {}) after {}s timeout",
                prefix, chest_pos.x, chest_pos.y, chest_pos.z, timeout_secs
            ))
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
        Some(c) => Ok(c),
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
    let mut last_error = String::new();
    let mut chunk_not_loaded_seen = false;

    // Start with the normal retry budget; if we detect a chunk-not-loaded
    // condition we extend the budget once so the bot waits for chunks to
    // stream back in rather than giving up immediately.
    let mut max_retries = CHEST_OP_MAX_RETRIES;

    let mut attempt = 0u32;
    while attempt < max_retries {
        if attempt > 0 {
            // Use longer backoff when waiting for chunks to reload
            let (base, max_delay) = if chunk_not_loaded_seen {
                (CHUNK_RELOAD_BASE_DELAY_MS, CHUNK_RELOAD_MAX_DELAY_MS)
            } else {
                (RETRY_BASE_DELAY_MS, RETRY_MAX_DELAY_MS)
            };
            let delay_ms = exponential_backoff_delay(attempt - 1, base, max_delay);
            info!(
                "Retrying chest open at ({}, {}, {}) attempt {}/{} after {}ms{}",
                chest_pos.x, chest_pos.y, chest_pos.z,
                attempt + 1, max_retries, delay_ms,
                if chunk_not_loaded_seen { " (waiting for chunk reload)" } else { "" }
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        }

        match open_chest_container_once(bot, chest_pos).await {
            Ok(container) => return Ok(container),
            Err(e) => {
                // First time we see a chunk-not-loaded error, extend the retry
                // budget so we don't exhaust normal retries on a transient issue.
                if !chunk_not_loaded_seen && e.starts_with(CHUNK_NOT_LOADED_PREFIX) {
                    chunk_not_loaded_seen = true;
                    max_retries = max_retries.saturating_add(CHUNK_RELOAD_EXTRA_RETRIES);
                    warn!(
                        "open_chest_container: Chunk not loaded at ({}, {}, {}), extending retries to {}",
                        chest_pos.x, chest_pos.y, chest_pos.z, max_retries
                    );
                }
                last_error = e;
                warn!(
                    "open_chest_container: Attempt {}/{} FAILED at ({}, {}, {}): {}",
                    attempt + 1,
                    max_retries,
                    chest_pos.x,
                    chest_pos.y,
                    chest_pos.z,
                    last_error
                );
            }
        }
        attempt += 1;
    }

    // Strip the internal prefix from the final user-facing message
    let clean_error = last_error.strip_prefix(CHUNK_NOT_LOADED_PREFIX).unwrap_or(&last_error);
    error!(
        "open_chest_container: FAILED after {} attempts at ({}, {}, {}): {}",
        max_retries, chest_pos.x, chest_pos.y, chest_pos.z, clean_error
    );
    Err(format!(
        "Failed to open chest at ({}, {}, {}) after {} attempts: {}",
        chest_pos.x, chest_pos.y, chest_pos.z, max_retries, clean_error
    ))
}

/// Transfer items from/to a shulker box.
/// direction: "withdraw" = from shulker to bot inventory (slots 9-35, NOT hotbar), "deposit" = from bot inventory (slots 9-35) to shulker
pub async fn transfer_items_with_shulker(
    shulker_container: &azalea::container::ContainerHandle,
    item: &str,
    amount: i32,
    direction: &str,
    stack_size: i32,
) -> Result<i32, String> {
    let target_id = Bot::normalize_item_id(item);

    debug!(
        "transfer_items_with_shulker: {} {} x{}",
        direction, item, amount
    );

    let total_moved = match direction {
        "withdraw" => {
            transfer_withdraw_from_shulker(shulker_container, &target_id, amount).await?
        }
        "deposit" => {
            transfer_deposit_into_shulker(shulker_container, &target_id, amount, stack_size).await?
        }
        _ => {
            error!("transfer_items_with_shulker: Invalid direction: {}", direction);
            return Err("Invalid direction".to_string());
        }
    };

    if total_moved < amount {
        warn!(
            "Incomplete transfer: moved {}/{} ({})",
            total_moved, amount, direction
        );
    }

    Ok(total_moved)
}

/// Withdraw `amount` units of `target_id` from `shulker_container` into the
/// bot's inventory. Used by `transfer_items_with_shulker`'s `"withdraw"` arm.
async fn transfer_withdraw_from_shulker(
    shulker_container: &azalea::container::ContainerHandle,
    target_id: &str,
    amount: i32,
) -> Result<i32, String> {
    use azalea::inventory::operations::PickupClick;

    let mut remaining = amount;
    let mut total_moved = 0;

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
        tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_LOOK_AT_MS)).await;

        let contents = shulker_container.contents().ok_or_else(|| {
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
                    stack.kind(),
                    stack.count(),
                    i
                );
                break;
            }
        }

        let Some((slot, stack_count)) = found else {
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
                tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_MEDIUM_MS)).await;
                let contents_after = shulker_container
                    .contents()
                    .ok_or_else(|| "Shulker closed".to_string())?;
                let current_count = contents_after.get(slot).map(|s| s.count()).unwrap_or(0);

                if current_count < stack_count {
                    // Items DID move, we just didn't detect it
                    let actual_moved = stack_count - current_count;
                    debug!(
                        "Shift-click reported 0 but {} items actually moved",
                        actual_moved
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
            // Pick up the stack from shulker
            shulker_container.click(PickupClick::Left {
                slot: Some(slot as u16),
            });
            tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_INTERACT_MS)).await;

            // Find an empty slot in inventory portion (slots 27-53 in shulker container view)
            let all_slots = shulker_container
                .slots()
                .ok_or_else(|| "Shulker closed".to_string())?;
            let shulker_size = contents.len(); // SHULKER_BOX_SLOTS
            let inv_start = shulker_size;
            let inv_end = inv_start + SHULKER_BOX_SLOTS; // inventory slots 9-35

            let mut target_slot: Option<usize> = None;
            for (i, stack) in all_slots.iter().enumerate().take(inv_end).skip(inv_start) {
                if stack.count() == 0 {
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
                tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_SHORT_MS)).await;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_MEDIUM_MS)).await;

            // Put remaining items back in original slot
            debug!(
                "transfer_items_with_shulker: Returning remaining items to slot {}",
                slot
            );
            shulker_container.click(PickupClick::Left {
                slot: Some(slot as u16),
            });
            tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_INTERACT_MS)).await;

            total_moved += remaining;
            remaining = 0;
        }
    }

    Ok(total_moved)
}

/// Deposit `amount` units of `target_id` from the bot's inventory into
/// `shulker_container`. Used by `transfer_items_with_shulker`'s `"deposit"` arm.
async fn transfer_deposit_into_shulker(
    shulker_container: &azalea::container::ContainerHandle,
    target_id: &str,
    amount: i32,
    stack_size: i32,
) -> Result<i32, String> {
    use azalea::inventory::operations::PickupClick;

    let mut remaining = amount;
    let mut total_moved = 0;

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
    let inventory_end = inv_start + SHULKER_BOX_SLOTS + 9; // 27 inventory + 9 hotbar = 36 slots (27..63)

    debug!(
        "transfer_items_with_shulker: Deposit - searching slots {}-{} for {}",
        inv_start,
        inventory_end - 1,
        target_id
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
        tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_LOOK_AT_MS)).await;

        let all_slots = shulker_container.slots().ok_or_else(|| {
            error!("transfer_items_with_shulker: Shulker closed during deposit");
            "Shulker closed".to_string()
        })?;
        let inv_end = all_slots.len();
        let mut found: Option<(usize, i32)> = None;
        // Search in BOTH inventory (27-53) AND hotbar (54-62) slots
        // Limit to actual container size
        for (i, stack) in all_slots.iter().enumerate().take(inventory_end.min(inv_end)).skip(inv_start) {
            if stack.count() > 0
                && Bot::normalize_item_id(&stack.kind().to_string()) == target_id
            {
                found = Some((i, stack.count()));
                let slot_type = if i < DOUBLE_CHEST_SLOTS { "inventory" } else { "hotbar" };
                debug!(
                    "transfer_items_with_shulker: Found {} x{} in {} slot {} (container idx {})",
                    stack.kind(),
                    stack.count(),
                    slot_type,
                    if i >= DOUBLE_CHEST_SLOTS { i - DOUBLE_CHEST_SLOTS } else { i - SHULKER_BOX_SLOTS },
                    i
                );
                break;
            }
        }

        let Some((slot, stack_count)) = found else {
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
                tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_BLOCK_OP_MS)).await;
                let slots_after = shulker_container
                    .slots()
                    .ok_or_else(|| "Shulker closed".to_string())?;
                let current_count = slots_after.get(slot).map(|s| s.count()).unwrap_or(0);

                if current_count < stack_count {
                    // Items DID move, we just didn't detect it
                    let actual_moved = stack_count - current_count;
                    debug!(
                        "Shift-click reported 0 but {} items actually moved",
                        actual_moved
                    );
                    total_moved += actual_moved;
                    remaining -= actual_moved;
                    consecutive_failures = 0;
                } else {
                    // Items really didn't move - retry
                    consecutive_failures += 1;
                    warn!(
                        "Shift-click moved 0 items (failure {}/3)",
                        consecutive_failures
                    );
                    if consecutive_failures >= 3 {
                        error!("Shift-click failed 3 times in a row, stopping deposit");
                        break;
                    }
                }
                continue;
            }
            consecutive_failures = 0; // Reset on success
            total_moved += moved;
            remaining -= moved;
        } else {
            // Need only a partial stack - use manual click transfer
            // Pick up the stack from inventory
            shulker_container.click(PickupClick::Left {
                slot: Some(slot as u16),
            });
            tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_INTERACT_MS)).await;

            // Find slots in shulker that can accept items (slots 0-26)
            // Priority: 1) slots with same item type that have room, 2) empty slots
            let shulker_size = shulker_contents.len(); // 27 slots

            // Build list of target slots with their available space
            let mut target_slots: Vec<(usize, i32)> = Vec::new(); // (slot_index, space_available)

            for (i, slot_item) in all_slots.iter().enumerate().take(shulker_size) {
                if slot_item.count() == 0 {
                    // Empty slot - can hold up to one stack
                    target_slots.push((i, stack_size));
                } else if Bot::normalize_item_id(&slot_item.kind().to_string()) == target_id {
                    // Same item type - can add up to (stack_size - current)
                    let space = stack_size - slot_item.count();
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
                tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_INTERACT_MS)).await;
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
                    tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_SHORT_MS)).await;
                }
                items_to_place -= place_count;
                total_moved += place_count;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_MEDIUM_MS)).await;

            // Calculate how many were actually placed
            let placed = remaining - items_to_place;
            remaining = items_to_place;

            // Put remaining items back in original slot (if any left on cursor)
            debug!(
                "transfer_items_with_shulker: Returning remaining items ({}) to slot {}",
                stack_count - placed,
                slot
            );
            shulker_container.click(PickupClick::Left {
                slot: Some(slot as u16),
            });
            tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_INTERACT_MS)).await;

            // If we couldn't place everything, shulker is now full - break to let caller try next shulker
            if remaining > 0 {
                break;
            }
        }
    }

    Ok(total_moved)
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
/// **Returns**: A `Vec<i32>` of length 54 containing item counts for each shulker slot.
/// Slots that were processed contain the accurate count after the operation.
/// Slots that were NOT processed contain -1 (caller should keep existing values for those).
///
/// **known_counts**: Optional pre-existing knowledge about shulker contents.
/// If provided, slots with `known_counts[i] == 0` will be skipped for withdrawals (known empty).
/// For deposits, slots at or above shulker capacity (27 × stack_size) will be skipped.
/// This avoids needlessly taking out and placing back shulkers that are known to be empty/full.
///
/// **stack_size**: The item's maximum stack size (1, 16, or 64). Used to calculate shulker capacity.
/// Prelude common to every `automated_chest_io` call: wait for the entity to be
/// ready, verify the bot is at the exact node position, and free hotbar slot 0
/// so shulker placement can't collide with leftover items.
async fn prepare_for_chest_io(bot: &Bot, node_position: &Position) -> Result<(), String> {
    let client = bot
        .client
        .read()
        .await
        .clone()
        .ok_or_else(|| "Bot not connected".to_string())?;
    if !super::inventory::is_entity_ready(&client) {
        warn!("Entity not ready, waiting for initialization...");
        super::inventory::wait_for_entity_ready(&client).await?;
    }

    let current_pos = client.entity().position();
    let current_block = azalea::BlockPos::from(current_pos);
    let target_block = azalea::BlockPos::new(node_position.x, node_position.y, node_position.z);

    let dx = (current_block.x - target_block.x).abs();
    let dy = (current_block.y - target_block.y).abs();
    let dz = (current_block.z - target_block.z).abs();

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

    if let Err(e) = super::inventory::move_hotbar_to_inventory(bot).await {
        warn!(
            "Failed to clear hotbar before chest operations: {} - proceeding anyway",
            e
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn automated_chest_io(
    bot: &Bot,
    chest_pos: BlockPos,
    chest_id: i32,
    item: &str,
    amount: i32,
    direction: &str,
    node_position: &Position,
    known_counts: Option<&[i32; DOUBLE_CHEST_SLOTS]>,
    stack_size: i32,
) -> Result<[i32; DOUBLE_CHEST_SLOTS], String> {
    prepare_for_chest_io(bot, node_position).await?;

    // Initialize counts: use known_counts if provided, otherwise -1 (meaning "not checked/unchanged")
    let mut slot_counts: [i32; DOUBLE_CHEST_SLOTS] = if let Some(known) = known_counts {
        *known
    } else {
        [-1; DOUBLE_CHEST_SLOTS]
    };

    if amount <= 0 {
        return Ok(slot_counts);
    }

    let target_id = Bot::normalize_item_id(item);
    info!(
        "[ChestIO] {} {}x {} at chest {:?}",
        direction, amount, item, chest_pos
    );
    let container = open_chest_container(bot, chest_pos).await?;
    let station_pos = super::shulker::shulker_station_position(node_position);

    let moved = match direction {
        "withdraw" => {
            withdraw_shulkers(
                bot,
                chest_pos,
                chest_id,
                item,
                &target_id,
                amount,
                node_position,
                &station_pos,
                container,
                &mut slot_counts,
                stack_size,
            )
            .await?
        }
        "deposit" => {
            deposit_shulkers(
                bot,
                chest_pos,
                chest_id,
                item,
                &target_id,
                amount,
                node_position,
                &station_pos,
                container,
                &mut slot_counts,
                stack_size,
                known_counts,
            )
            .await?
        }
        _ => return Err("Invalid chest IO direction".to_string()),
    };

    if moved < amount {
        return Err(format!(
            "Incomplete chest IO: moved {}, needed {}",
            moved, amount
        ));
    }

    info!(
        "Chest IO complete, returning counts for {} processed slots",
        slot_counts.iter().filter(|&&c| c >= 0).count()
    );
    Ok(slot_counts)
}

/// Carries the open shulker container back to the caller after it has been
/// placed on the station and opened. Keeping it as a named struct makes the
/// return type of `place_shulker_on_station` self-documenting.
struct ShulkerOnStation {
    shulker_container: azalea::container::ContainerHandle,
}

/// Common preamble shared by every shulker round-trip:
///   1. Clear cursor (LeftOutside click).
///   2. Begin journal entry.
///   3. Take shulker from chest slot.
///   4. Drop (close) the chest container.
///   5. Settle, then ensure shulker reaches hotbar slot 0.
///   6. Verify the bot is holding a shulker.
///   7. Look at the station floor and place the shulker.
///   8. Advance journal to ShulkerOnStation.
///   9. Open the shulker container.
///
/// Returns the open shulker container so the caller can perform its
/// direction-specific transfer, then call `finish_shulker_round_trip`.
///
/// **`container` is consumed** — the chest must be reopened by the caller
/// (via `finish_shulker_round_trip`) once the shulker has been processed.
#[allow(clippy::too_many_arguments)]
async fn place_shulker_on_station(
    bot: &Bot,
    chest_id: i32,
    slot_idx: usize,
    station_pos: &Position,
    journal_op: crate::store::journal::JournalOp,
    container: azalea::container::ContainerHandle,
    context_label: &str,
) -> Result<ShulkerOnStation, String> {
    use crate::store::journal::JournalState;

    // CRITICAL: Ensure cursor is empty — a non-empty cursor turns the pickup
    // into a swap rather than a clean pick-up of the shulker.
    container.click(PickupClick::LeftOutside);
    tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_MEDIUM_MS)).await;

    // Journal: record intent before touching the shulker.
    {
        if let Err(e) = bot.journal.lock().begin(journal_op, chest_id, slot_idx) {
            warn!("[Journal] begin failed: {}", e);
        }
    }

    // Take shulker from chest slot into cursor.
    container.click(PickupClick::Left {
        slot: Some(slot_idx as u16),
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_INTERACT_MS)).await;

    // IMPORTANT: Close chest FIRST before any inventory operations.
    // The server does not allow opening the player inventory while a container
    // (chest) is open.
    drop(container);
    // CRITICAL: Give the server time to sync the shulker from cursor → inventory.
    tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_SETTLE_MS)).await;

    // Get a fresh client handle for inventory / block interaction.
    let client = bot
        .client
        .read()
        .await
        .clone()
        .ok_or_else(|| "Bot not connected".to_string())?;

    // Move shulker to hotbar slot 0 so the bot can select and place it.
    if let Err(e) = super::inventory::ensure_shulker_in_hotbar_slot_0(bot).await {
        return Err(format!("Failed to ensure shulker in hotbar slot 0: {}", e));
    }

    // Verify the bot is currently holding a shulker; fall back to an explicit
    // inventory-open click on hotbar slot 0 if the initial ensure didn't surface it.
    if !super::inventory::verify_holding_shulker(&client) {
        let inv_handle = client
            .open_inventory()
            .ok_or_else(|| "Failed to open inventory".to_string())?;
        inv_handle.click(PickupClick::Left {
            slot: Some(HOTBAR_SLOT_0 as u16),
        });
        tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_LOOK_AT_MS)).await;
        drop(inv_handle);
        tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_MEDIUM_MS)).await;

        if !super::inventory::verify_holding_shulker(&client) {
            return Err(format!(
                "Bot is not holding shulker before placing on station ({} slot {})",
                context_label, slot_idx
            ));
        }
    }

    // Right-click the floor block below the station position to place the shulker
    // on top of it (standard Minecraft block-placement mechanic).
    let floor_block = BlockPos::new(station_pos.x, station_pos.y - 1, station_pos.z);
    let place_vec3 = Vec3::new(
        station_pos.x as f64 + 0.5,
        // Look slightly below station Y to target the floor block's top face.
        station_pos.y as f64 - 0.4,
        station_pos.z as f64 + 0.5,
    );
    client.look_at(place_vec3);
    tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_LOOK_AT_MS)).await;
    client.block_interact(floor_block);
    // Use the longer shulker-placement delay here (750 ms) because the block
    // entity needs extra time to register before we can open it as a container.
    // The withdraw path previously used DELAY_SETTLE_MS (500 ms) and the deposit
    // path used DELAY_SHULKER_PLACE_MS (750 ms); we take the conservative value.
    tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_SHULKER_PLACE_MS)).await;

    // Journal: shulker is now on the station.
    {
        if let Err(e) = bot.journal.lock().advance(JournalState::ShulkerOnStation) {
            warn!("[Journal] advance(ShulkerOnStation) failed: {}", e);
        }
    }

    // Open the placed shulker as a container.
    let shulker_container = super::shulker::open_shulker_at_station(bot, station_pos).await?;
    Ok(ShulkerOnStation { shulker_container })
}

/// Common epilogue shared by every shulker round-trip:
///   1. Close the shulker container.
///   2. Advance journal to ItemsTransferred.
///   3. Clear hotbar (so the shulker lands in slot 0 when auto-picked up).
///   4. Pick up the shulker from the station.
///   5. Advance journal to ShulkerPickedUp.
///   6. Reopen the chest.
///   7. Locate the shulker in the chest-view inventory portion.
///   8. Place shulker back into `chest_slot` (using the verified helper).
///   9. Optionally close and reopen the chest for the next iteration.
///  10. Advance journal to ShulkerReplaced and complete.
///
/// Returns the reopened chest container.
#[allow(clippy::too_many_arguments)]
async fn finish_shulker_round_trip(
    bot: &Bot,
    chest_pos: BlockPos,
    slot_idx: usize,
    station_pos: &Position,
    node_position: &Position,
    shulker_container: azalea::container::ContainerHandle,
    reopen_chest: bool,
) -> Result<azalea::container::ContainerHandle, String> {
    use crate::store::journal::JournalState;

    // Close shulker and let the server process the close event before the bot
    // attempts to break the block.
    shulker_container.close();
    tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_SETTLE_MS)).await;

    // Journal: items have been transferred (or 0 moved, still advancing state).
    {
        if let Err(e) = bot.journal.lock().advance(JournalState::ItemsTransferred) {
            warn!("[Journal] advance(ItemsTransferred) failed: {}", e);
        }
    }

    // CRITICAL: Clear hotbar BEFORE picking up shulker from station.
    // Shift-click transfers may have deposited items in the hotbar; if any
    // hotbar slot is occupied the shulker will not auto-land in slot 0.
    if let Err(e) = super::inventory::move_hotbar_to_inventory(bot).await {
        warn!(
            "Failed to clear hotbar before shulker pickup: {} - proceeding anyway",
            e
        );
    }

    // Break and collect the shulker block from the station.
    super::shulker::pickup_shulker_from_station(bot, station_pos, node_position).await?;

    // Journal: shulker is back in bot inventory; station is clear.
    {
        if let Err(e) = bot.journal.lock().advance(JournalState::ShulkerPickedUp) {
            warn!("[Journal] advance(ShulkerPickedUp) failed: {}", e);
        }
    }

    // Reopen the chest (it was dropped before placing the shulker).
    let container = open_chest_container(bot, chest_pos).await?;

    // Locate the shulker in the player-inventory portion of the chest view and
    // place it back into its original chest slot using the verified helper.
    let shulker_in_container_view = find_shulker_in_inventory_view(&container)?;
    if let Some(container_slot) = shulker_in_container_view {
        place_shulker_in_chest_slot_verified(&container, container_slot, slot_idx).await?;
    } else {
        warn!(
            "Could not find shulker in inventory (via chest container view) to place back in chest slot {}",
            slot_idx
        );
    }

    // Optionally close and reopen the chest so the next iteration starts with a
    // clean container state.
    let container = if reopen_chest {
        container.close();
        tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_BLOCK_OP_MS)).await;
        open_chest_container(bot, chest_pos).await?
    } else {
        container
    };

    // Journal: shulker is back in its chest slot; round-trip complete.
    {
        let mut j = bot.journal.lock();
        if let Err(e) = j.advance(JournalState::ShulkerReplaced) {
            warn!("[Journal] advance(ShulkerReplaced) failed: {}", e);
        }
        if let Err(e) = j.complete() {
            warn!("[Journal] complete failed: {}", e);
        }
    }

    Ok(container)
}

/// Per-slot withdraw loop, extracted from `automated_chest_io` for readability.
///
/// Walks the 54 chest slots, opening shulkers that might contain the target item,
/// transferring items into the bot's inventory, and putting each shulker back.
/// Restarts the scan from slot 0 whenever items still remain, since a partially
/// drained shulker can legitimately be revisited (confirmed-empty slots are kept
/// in a separate set so they stay skipped across passes).
#[allow(clippy::too_many_arguments)]
async fn withdraw_shulkers(
    bot: &Bot,
    chest_pos: BlockPos,
    chest_id: i32,
    item: &str,
    target_id: &str,
    amount: i32,
    node_position: &Position,
    station_pos: &Position,
    mut container: azalea::container::ContainerHandle,
    slot_counts: &mut [i32],
    stack_size: i32,
) -> Result<i32, String> {
    use crate::store::journal::JournalOp;
    let mut remaining = amount;
    let mut confirmed_empty: std::collections::HashSet<usize> = std::collections::HashSet::new();

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

        for slot_idx in 0..DOUBLE_CHEST_SLOTS {
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

            // Ensure the chest is still open — it may have been closed by a
            // server restart or chunk unload since the last iteration. If the
            // container handle is stale, reopen the chest (which itself uses
            // the chunk-aware retry loop) before reading contents.
            if container.contents().is_none() {
                warn!("withdraw_shulkers: Container lost at slot {} scan, reopening chest", slot_idx);
                drop(container);
                container = open_chest_container(bot, chest_pos).await?;
            }

            // Refresh contents to get current state
            let contents = container
                .contents()
                .ok_or_else(|| "Chest closed after reopen attempt".to_string())?;
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

            // --- Shulker round-trip (withdraw direction) ---
            //
            // Phase 1: Take shulker from chest slot, place on station, open it.
            let ShulkerOnStation { shulker_container } = place_shulker_on_station(
                bot,
                chest_id,
                slot_idx,
                station_pos,
                JournalOp::WithdrawFromChest,
                container,
                "withdraw",
            )
            .await?;

            // Phase 2 (direction-specific): Count target items and withdraw them.
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

            let moved = if shulker_item_count > 0 {
                let to_withdraw = remaining.min(shulker_item_count);
                let moved = transfer_items_with_shulker(
                    &shulker_container,
                    item,
                    to_withdraw,
                    "withdraw",
                    stack_size,
                )
                .await?;
                debug!("Withdrew {} items from shulker", moved);
                moved
            } else {
                debug!("Shulker in slot {} contains no target items", slot_idx);
                0
            };

            // Update slot counts before releasing the shulker container.
            let remaining_in_slot = shulker_item_count - moved;
            slot_counts[slot_idx] = remaining_in_slot;
            if remaining_in_slot == 0 {
                confirmed_empty.insert(slot_idx);
            }

            if moved == 0 && shulker_item_count > 0 {
                warn!("No items were withdrawn from shulker despite {} available", shulker_item_count);
            }

            // Phase 3: Close shulker, pick up, put back in chest.
            // Reopen the chest after put-back only if we still need more items.
            container = finish_shulker_round_trip(
                bot,
                chest_pos,
                slot_idx,
                station_pos,
                node_position,
                shulker_container,
                remaining > moved, // reopen_chest
            )
            .await?;

            if moved > 0 {
                remaining -= moved;
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

    Ok(amount - remaining)
}

/// Per-slot deposit loop, extracted from `automated_chest_io` for readability.
///
/// Walks the 54 chest slots once, opening each shulker that isn't already known
/// full, transferring items from the bot's inventory, and putting the shulker
/// back. Slots that turn out to already be full are remembered (via
/// `confirmed_full`) so nothing revisits them later in the same pass.
#[allow(clippy::too_many_arguments)]
async fn deposit_shulkers(
    bot: &Bot,
    chest_pos: BlockPos,
    chest_id: i32,
    item: &str,
    target_id: &str,
    amount: i32,
    node_position: &Position,
    station_pos: &Position,
    mut container: azalea::container::ContainerHandle,
    slot_counts: &mut [i32],
    stack_size: i32,
    known_counts: Option<&[i32; DOUBLE_CHEST_SLOTS]>,
) -> Result<i32, String> {
    use crate::store::journal::JournalOp;
    let mut remaining = amount;
    let mut confirmed_full: std::collections::HashSet<usize> = std::collections::HashSet::new();

    // Ensure the chest is still open before scanning for shulkers.
    if container.contents().is_none() {
        warn!("deposit_shulkers: Container lost before shulker scan, reopening chest");
        drop(container);
        container = open_chest_container(bot, chest_pos).await?;
    }
    // First, check if chest has any shulkers at all
    let contents = container
        .contents()
        .ok_or_else(|| "Chest closed after reopen attempt".to_string())?;
    let mut has_any_shulker = false;
    for (_slot_idx, entry) in contents.iter().enumerate().take(DOUBLE_CHEST_SLOTS) {
        if entry.count() > 0 && super::shulker::is_shulker_box(&entry.kind().to_string()) {
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

    for slot_idx in 0..DOUBLE_CHEST_SLOTS {
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
        if let Some(known) = known_counts
            && let Some(&count) = known.get(slot_idx)
                && count >= shulker_capacity {
                    debug!(
                        "Skipping slot {}: known full with {} items (max {})",
                        slot_idx, count, shulker_capacity
                    );
                    confirmed_full.insert(slot_idx);
                    continue;
                }

        // Ensure chest is open (it might have been closed by a server restart,
        // chunk unload, or previous shulker iteration). Reopen uses the
        // chunk-aware retry loop so transient unloads are handled.
        if container.contents().is_none() {
            warn!("deposit_shulkers: Container lost at slot {} scan, reopening chest", slot_idx);
            drop(container);
            container = open_chest_container(bot, chest_pos).await?;
        }

        // Refresh contents to get current state
        let contents = container
            .contents()
            .ok_or_else(|| "Chest closed after reopen attempt".to_string())?;
        if slot_idx >= contents.len() {
            continue;
        }

        let stack = &contents[slot_idx];
        if stack.count() <= 0 || !super::shulker::is_shulker_box(&stack.kind().to_string())
        {
            debug!("Skipping slot {}: empty or not a shulker", slot_idx);
            continue;
        }

        debug!("Processing shulker in slot {} for deposit", slot_idx);

        // --- Shulker round-trip (deposit direction) ---
        //
        // Phase 1: Take shulker from chest slot, place on station, open it.
        let ShulkerOnStation { shulker_container } = place_shulker_on_station(
            bot,
            chest_id,
            slot_idx,
            station_pos,
            JournalOp::DepositToChest,
            container,
            "deposit",
        )
        .await?;

        // Phase 2 (direction-specific): measure space, count bot items, deposit.
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

        debug!(
            "Shulker has {} space for {} (need {}), currently contains {}",
            total_space, target_id, remaining, initial_item_count
        );

        // Count how many target items the bot currently has across inventory+hotbar.
        let all_slots = shulker_container
            .slots()
            .ok_or_else(|| "Shulker closed".to_string())?;
        let inv_start = shulker_contents.len(); // Bot inventory starts after shulker contents (27)
        // Search BOTH inventory (27-53) AND hotbar (54-62) - items can be anywhere after a trade
        let inventory_and_hotbar_end = inv_start + SHULKER_BOX_SLOTS + 9; // 27 inventory + 9 hotbar = 36 slots
        let mut bot_item_count = 0i32;
        for (_i, stack) in all_slots.iter().enumerate().take(inventory_and_hotbar_end).skip(inv_start) {
            if stack.count() > 0
                && Bot::normalize_item_id(&stack.kind().to_string()) == target_id
            {
                bot_item_count += stack.count();
            }
        }
        debug!(
            "Bot has {} items of {} in inventory+hotbar",
            bot_item_count, target_id
        );

        // Decide how many items to move (may be 0 if shulker is full/wrong item).
        let moved = if bot_item_count == 0 {
            warn!("Bot has no items of {} in inventory to deposit", target_id);
            // Release container before returning error.
            drop(shulker_container);
            return Err(format!(
                "Bot inventory is empty - no items of {} to deposit",
                target_id
            ));
        } else if total_space == 0 {
            // Shulker is full or contains a different item type — skip it.
            debug!("Shulker full or wrong item for {}, trying next", target_id);
            slot_counts[slot_idx] = initial_item_count;
            confirmed_full.insert(slot_idx);
            0 // no items transferred
        } else {
            let to_deposit = remaining.min(total_space).min(bot_item_count);
            let moved = transfer_items_with_shulker(
                &shulker_container,
                item,
                to_deposit,
                "deposit",
                stack_size,
            )
            .await?;
            debug!("Deposited {} items into shulker", moved);
            if moved == 0 {
                warn!("No items were transferred, shulker may be full or bot inventory empty");
                if total_space > 0 && bot_item_count > 0 {
                    drop(shulker_container);
                    return Err(format!(
                        "Failed to transfer items to shulker despite having {} space and {} items in inventory",
                        total_space, bot_item_count
                    ));
                }
            }
            slot_counts[slot_idx] = initial_item_count + moved;
            moved
        };

        // Phase 3: Close shulker, pick up, put back in chest.
        // Reopen the chest after put-back only when there are more slots to process.
        let reopen_chest = remaining > moved && slot_idx < DOUBLE_CHEST_SLOTS - 1;
        container = finish_shulker_round_trip(
            bot,
            chest_pos,
            slot_idx,
            station_pos,
            node_position,
            shulker_container,
            reopen_chest,
        )
        .await?;

        if moved > 0 {
            remaining -= moved;
        }
    }

    Ok(amount - remaining)
}
