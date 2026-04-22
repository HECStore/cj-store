//! Inventory management operations

use azalea::BlockPos;
use azalea::inventory::operations::PickupClick;
use tracing::{debug, error, info, warn};

use crate::constants::HOTBAR_SLOT_0;
use super::{Bot, shulker};

/// Ensure inventory is empty by dumping items to buffer chest if configured
pub async fn ensure_inventory_empty(bot: &Bot) -> Result<(), String> {
    let client = bot
        .client
        .read()
        .await
        .clone()
        .ok_or_else(|| {
            error!("ensure_inventory_empty: Bot not connected");
            "Bot not connected".to_string()
        })?;

    // CRITICAL: First check and clear the cursor if it has items.
    // Items in cursor from previous operations can cause subsequent operations to fail:
    // a held cursor stack makes left-clicks behave as "place" instead of "pick up",
    // which silently corrupts any shift-click / pickup sequence that follows. Leftover
    // cursor state is also invisible to the server until the next click, so we proactively
    // stash it in an empty inventory slot (or drop it outside) before doing anything else.
    let cursor = carried_item(&client);
    if cursor.count() > 0 {
        warn!(
            "ensure_inventory_empty: Cursor has {}x {} from previous operation - clearing it",
            cursor.count(), cursor.kind()
        );
        // Open inventory to access cursor operations
        let inv_handle = client
            .open_inventory()
            .ok_or_else(|| {
                error!("ensure_inventory_empty: Failed to open inventory to clear cursor");
                "Failed to open inventory to clear cursor".to_string()
            })?;
        
        // Find an empty slot to put cursor items
        let slots = inv_handle.slots().ok_or_else(|| "Inventory closed".to_string())?;
        let mut placed = false;
        for i in 9..45 {
            if i < slots.len() && slots[i].count() == 0 {
                inv_handle.click(azalea::inventory::operations::PickupClick::Left {
                    slot: Some(i as u16),
                });
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                placed = true;
                debug!("ensure_inventory_empty: Placed cursor items in slot {}", i);
                break;
            }
        }
        
        if !placed {
            // No empty slot, try to drop outside
            warn!("ensure_inventory_empty: No empty slot for cursor items, dropping outside");
            inv_handle.click(azalea::inventory::operations::PickupClick::LeftOutside);
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        }
        
        // Verify cursor is now empty
        let cursor_after = carried_item(&client);
        if cursor_after.count() > 0 {
            error!(
                "ensure_inventory_empty: Failed to clear cursor - still has {}x {}",
                cursor_after.count(), cursor_after.kind()
            );
        }
        
        drop(inv_handle);
    }

    let Some(pos) = bot.buffer_chest_position else {
        debug!("ensure_inventory_empty: No buffer chest configured, skipping inventory dump");
        return Ok(());
    };

    debug!(
        "ensure_inventory_empty: Checking inventory, buffer chest at ({}, {}, {})", 
        pos.x, pos.y, pos.z
    );

    // Quick check: if there are any non-empty stacks in player inventory/hotbar, attempt to dump.
    let inv_handle = client
        .open_inventory()
        .ok_or_else(|| {
            error!("ensure_inventory_empty: Failed to open inventory (another container is open?)");
            "Failed to open inventory (another container is open?)".to_string()
        })?;
    let slots = inv_handle
        .slots()
        .ok_or_else(|| "Inventory closed".to_string())?;

    // Count items for logging
    let mut total_items = 0i32;
    let mut item_types = 0;
    for slot in slots.iter() {
        if slot.count() > 0 {
            total_items += slot.count();
            item_types += 1;
        }
    }
    drop(inv_handle);
    
    if total_items == 0 {
        debug!("ensure_inventory_empty: Inventory is already empty");
        return Ok(());
    }

    debug!(
        "ensure_inventory_empty: Dumping {} items ({} stacks) to buffer chest",
        total_items, item_types
    );

    // Open buffer chest and shift-click all items from inventory into it.
    let chest = client
        .open_container_at_with_timeout_ticks(
            BlockPos::new(pos.x, pos.y, pos.z),
            Some(300),
        )
        .await
        .ok_or_else(|| {
            error!("ensure_inventory_empty: Failed to open buffer chest at ({}, {}, {})", pos.x, pos.y, pos.z);
            "Failed to open buffer chest".to_string()
        })?;

    let contents_len = chest
        .contents()
        .ok_or_else(|| "Buffer chest closed".to_string())?
        .len();
    let all = chest
        .slots()
        .ok_or_else(|| "Buffer chest closed".to_string())?;
    
    let mut items_moved = 0;
    for i in contents_len..all.len() {
        if all[i].count() > 0 {
            debug!(
                "ensure_inventory_empty: Shift-clicking slot {} ({} x{})", 
                i, all[i].kind(), all[i].count()
            );
            chest.shift_click(i);
            items_moved += 1;
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        }
    }

    debug!("ensure_inventory_empty: Moved {} stacks to buffer chest", items_moved);
    Ok(())
}

/// Move items from hotbar (slots 36-44) to inventory (slots 9-35).
/// This ensures hotbar slot 0 (36) is always available for shulker boxes.
/// Called after trade completes to organize inventory.
///
/// Slot numbering note: the player inventory container uses a flat slot space where
/// 0-8 are crafting/armor, 9-35 are the main inventory rows, and 36-44 are the hotbar.
/// The in-game "hotbar slot N" (0-8) corresponds to container slot `36 + N`.
pub async fn move_hotbar_to_inventory(bot: &Bot) -> Result<(), String> {
    debug!("move_hotbar_to_inventory: Starting hotbar cleanup");
    
    let client = bot
        .client
        .read()
        .await
        .clone()
        .ok_or_else(|| {
            error!("move_hotbar_to_inventory: Bot not connected");
            "Bot not connected".to_string()
        })?;

    let inv_handle = client
        .open_inventory()
        .ok_or_else(|| {
            error!("move_hotbar_to_inventory: Failed to open inventory (another container is open?)");
            "Failed to open inventory".to_string()
        })?;
    
    let all_slots = inv_handle
        .slots()
        .ok_or_else(|| {
            error!("move_hotbar_to_inventory: Inventory closed unexpectedly");
            "Inventory closed".to_string()
        })?;

    // Log current hotbar state
    debug!("move_hotbar_to_inventory: Current hotbar state:");
    for hotbar_idx in 36..45 {
        if let Some(stack) = all_slots.get(hotbar_idx) {
            if stack.count() > 0 {
                debug!("  Hotbar slot {} (idx {}): {} x{}", hotbar_idx - 36, hotbar_idx, stack.kind(), stack.count());
            }
        }
    }

    // Hotbar slots are 36-44 (inventory indices)
    // Inventory slots are 9-35
    let mut moved_count = 0;
    for hotbar_idx in 36..45 {
        let stack = all_slots.get(hotbar_idx);
        if stack.map(|s| s.count() > 0).unwrap_or(false) {
            let item_kind = stack.map(|s| s.kind().to_string()).unwrap_or_else(|| "unknown".to_string());
            let item_count = stack.map(|s| s.count()).unwrap_or(0);
            
            // Find an empty slot in inventory (9-35)
            let mut empty_slot: Option<usize> = None;
            for inv_idx in 9..36 {
                if all_slots.get(inv_idx).map(|s| s.count() == 0).unwrap_or(true) {
                    empty_slot = Some(inv_idx);
                    break;
                }
            }

            if let Some(empty) = empty_slot {
                debug!(
                    "move_hotbar_to_inventory: Moving {} x{} from hotbar slot {} to inventory slot {}", 
                    item_kind, item_count, hotbar_idx, empty
                );
                
                // Pick up item from hotbar
                inv_handle.click(PickupClick::Left {
                    slot: Some(hotbar_idx as u16),
                });
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                
                // Place in inventory slot
                inv_handle.click(PickupClick::Left {
                    slot: Some(empty as u16),
                });
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                
                // Refresh slots for next iteration
                let _ = inv_handle.slots();
                moved_count += 1;
            } else {
                warn!(
                    "move_hotbar_to_inventory: No empty inventory slot found for {} x{} at hotbar slot {}", 
                    item_kind, item_count, hotbar_idx
                );
            }
        }
    }

    drop(inv_handle);
    debug!("move_hotbar_to_inventory: Moved {} items", moved_count);
    Ok(())
}

/// Quick move items from a container slot (shift-click)
pub async fn quick_move_from_container(
    container: &azalea::container::ContainerHandle,
    slot_index: usize,
) -> Result<i32, String> {
    use azalea::inventory::operations::QuickMoveClick;

    let before = container
        .slots()
        .ok_or_else(|| "Container closed while reading slots".to_string())?;
    let before_slot = before.get(slot_index);
    let before_count = before_slot.map(|s| s.count()).unwrap_or(0);
    let before_item = before_slot.map(|s| s.kind().to_string()).unwrap_or_else(|| "None".to_string());

    debug!(
        "quick_move_from_container: slot {} BEFORE shift-click: {} x{}", 
        slot_index, before_item, before_count
    );

    if before_count == 0 {
        warn!("quick_move_from_container: slot {} is EMPTY, nothing to move", slot_index);
        return Ok(0);
    }

    container.click(QuickMoveClick::Left {
        slot: slot_index
            .try_into()
            .map_err(|_| "Slot index too large".to_string())?,
    });

    // Give the server time to apply the move and send updates back.
    // 400ms is more conservative to handle slower server responses.
    tokio::time::sleep(tokio::time::Duration::from_millis(400)).await;

    let after = container
        .slots()
        .ok_or_else(|| "Container closed while reading slots".to_string())?;
    let after_slot = after.get(slot_index);
    let after_count = after_slot.map(|s| s.count()).unwrap_or(0);
    let after_item = after_slot.map(|s| s.kind().to_string()).unwrap_or_else(|| "None".to_string());
    
    let moved = (before_count - after_count).max(0);
    
    debug!(
        "quick_move_from_container: slot {} AFTER shift-click: {} x{} (moved: {})", 
        slot_index, after_item, after_count, moved
    );
    
    if moved == 0 && before_count > 0 {
        warn!(
            "quick_move_from_container: shift-click moved 0 items from slot {} (had {} x{})", 
            slot_index, before_item, before_count
        );
    }

    Ok(moved)
}

/// Verify that bot is holding a shulker box before placing it.
/// Returns true if holding shulker, false otherwise.
pub fn verify_holding_shulker(client: &azalea::Client) -> bool {
    let carried = carried_item(client);
    let item_kind = carried.kind().to_string();
    let is_shulker = carried.count() > 0 && shulker::is_shulker_box(&item_kind);
    
    if is_shulker {
        debug!("verify_holding_shulker: YES - cursor holds {} (count: {})", item_kind, carried.count());
    } else if carried.count() > 0 {
        debug!("verify_holding_shulker: NO - cursor holds {} (count: {}) - NOT a shulker", item_kind, carried.count());
    } else {
        debug!("verify_holding_shulker: NO - cursor is EMPTY");
    }
    
    is_shulker
}

/// Check if the entity's Inventory component is available (entity fully initialized).
///
/// Use this to verify the bot is ready for inventory operations after connection.
/// Returns true if the Inventory component is available, false otherwise.
///
/// Background: immediately after login, Azalea's ECS may not yet have attached the
/// `Inventory` component to the player entity. Any inventory read (including cursor
/// state) performed during that window will silently report empty, which can cause
/// subsequent click logic to make decisions on stale data. Callers should wait for
/// this check to return true before touching inventory state.
pub fn is_entity_ready(client: &azalea::Client) -> bool {
    client.ecs.read().get::<azalea::entity::inventory::Inventory>(client.entity).is_some()
}

/// Wait for the entity to be fully initialized with Inventory component.
/// 
/// Returns Ok(()) when ready, Err after timeout (10 seconds).
pub async fn wait_for_entity_ready(client: &azalea::Client) -> Result<(), String> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(10);
    
    while start.elapsed() < timeout {
        if is_entity_ready(client) {
            return Ok(());
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    
    Err("Entity not ready after 10s timeout - Inventory component not available".to_string())
}

/// Get the item currently carried by the bot
/// 
/// Returns an empty ItemStack if the Inventory component is not yet available
/// (can happen immediately after connection before ECS is fully initialized).
pub fn carried_item(client: &azalea::Client) -> azalea::inventory::ItemStack {
    // Use try_get to avoid panic if Inventory component isn't ready yet
    // This can happen right after connection before Azalea fully initializes the entity
    match client.ecs.read().get::<azalea::entity::inventory::Inventory>(client.entity) {
        Some(inventory) => inventory.carried.clone(),
        None => {
            tracing::warn!("carried_item: Inventory component not available yet (entity not fully initialized)");
            azalea::inventory::ItemStack::Empty
        }
    }
}

/// Ensure shulker is in hotbar slot 0 before placing it.
/// If shulker is in cursor, places it in hotbar slot 0.
/// If shulker is in another slot, moves it to hotbar slot 0.
/// Returns Ok(()) if shulker is now in hotbar slot 0, Err if it couldn't be moved.
///
/// Flow overview (this function is the trickiest piece of state-wrangling in the bot):
///   1. Fast path: shulker already sits in container slot 36 (hotbar slot 0) -> done.
///   2. Cursor path: shulker is currently held on the cursor.
///        a. If slot 36 is empty, just left-click slot 36 to deposit.
///        b. If slot 36 is occupied, we CANNOT use a pick-up/put-down dance because
///           the cursor already holds the shulker - left-clicking would swap the
///           shulker for whatever was in slot 36, then we'd be holding junk. Instead
///           we shift-click slot 36 to evacuate its contents into the main inventory
///           (slots 9-35) without touching the cursor, verify the shulker is still
///           on the cursor (rarely, the shift-click can race and displace it), and
///           then left-click slot 36 to place the shulker.
///        c. If any verification fails, hand off to `recover_shulker_to_slot_0`,
///           which retries from a clean state.
///   3. Slot path: shulker lives in some other inventory/hotbar slot.
///        a. Evacuate slot 36 into an empty inventory slot if needed (pick-up/put-down).
///        b. Pick up the shulker, place it into slot 36, then verify with a short
///           retry loop because server inventory updates lag behind click packets.
///
/// Why slot 0 specifically: the "place shulker" path later uses the hotbar-select
/// packet to hold the shulker for a block_interact, and the rest of the bot assumes
/// hotbar slot 0 as the canonical carry slot. Keeping this invariant centralized here
/// keeps every caller simple.
pub async fn ensure_shulker_in_hotbar_slot_0(bot: &Bot) -> Result<(), String> {
    use azalea::inventory::operations::PickupClick;

    debug!("ensure_shulker_in_hotbar_slot_0: Starting");

    let client = bot
        .client
        .read()
        .await
        .clone()
        .ok_or_else(|| {
            error!("ensure_shulker_in_hotbar_slot_0: Bot not connected");
            "Bot not connected".to_string()
        })?;

    let inv_handle = client
        .open_inventory()
        .ok_or_else(|| {
            error!("ensure_shulker_in_hotbar_slot_0: Failed to open inventory (another container may be open)");
            "Failed to open inventory".to_string()
        })?;
    
    let all_slots = inv_handle
        .slots()
        .ok_or_else(|| {
            error!("ensure_shulker_in_hotbar_slot_0: Inventory closed unexpectedly");
            "Inventory closed".to_string()
        })?;

    // Log current state for debugging
    let cursor_item = carried_item(&client);
    debug!(
        "ensure_shulker_in_hotbar_slot_0: CURSOR state: {} x{}", 
        cursor_item.kind(), cursor_item.count()
    );
    
    // Log all shulker locations in inventory
    debug!("ensure_shulker_in_hotbar_slot_0: Scanning all slots for shulkers:");
    for (i, slot_item) in all_slots.iter().enumerate() {
        if slot_item.count() > 0 && shulker::is_shulker_box(&slot_item.kind().to_string()) {
            let slot_type = if i < 9 {
                "crafting/armor"
            } else if i < 36 {
                "inventory"
            } else if i < 45 {
                "hotbar"
            } else {
                "unknown"
            };
            info!(
                "ensure_shulker_in_hotbar_slot_0: Found shulker in {} slot {} (idx {}): {}", 
                slot_type, if i >= 36 { i - 36 } else { i }, i, slot_item.kind()
            );
        }
    }
    
    // Log hotbar slot 0 state
    if let Some(slot_item) = all_slots.get(HOTBAR_SLOT_0) {
        debug!(
            "ensure_shulker_in_hotbar_slot_0: Hotbar slot 0 (idx 36) contains: {} x{}", 
            slot_item.kind(), slot_item.count()
        );
    }

    // Check if shulker is already in hotbar slot 0
    if let Some(slot_item) = all_slots.get(HOTBAR_SLOT_0) {
        if slot_item.count() > 0 && shulker::is_shulker_box(&slot_item.kind().to_string()) {
            debug!("ensure_shulker_in_hotbar_slot_0: Shulker ALREADY in hotbar slot 0 - no action needed");
            drop(inv_handle);
            return Ok(());
        }
    }

    // Check if shulker is in cursor
    let carried = carried_item(&client);
    if carried.count() > 0 && shulker::is_shulker_box(&carried.kind().to_string()) {
        info!(
            "ensure_shulker_in_hotbar_slot_0: Shulker is in CURSOR ({}), will place in hotbar slot 0", 
            carried.kind()
        );
        
        // CRITICAL: Clear hotbar slot 0 first if needed, but be careful not to lose the shulker in cursor.
        // A naive left-click on slot 36 while holding the shulker would SWAP the two stacks,
        // leaving us with the wrong item on the cursor. We must use shift-click (quick move) to
        // evacuate slot 36 into the main inventory without disturbing cursor state.
        if let Some(slot_item) = all_slots.get(HOTBAR_SLOT_0) {
            if slot_item.count() > 0 {
                info!(
                    "ensure_shulker_in_hotbar_slot_0: Hotbar slot 0 is OCCUPIED by {} x{}, clearing it first", 
                    slot_item.kind(), slot_item.count()
                );
                // Find an empty slot in inventory (slots 9-35)
                let mut empty_slot: Option<usize> = None;
                for i in 9..36 {
                    if all_slots.get(i).map(|s| s.count() == 0).unwrap_or(true) {
                        empty_slot = Some(i);
                        break;
                    }
                }
                if let Some(_empty) = empty_slot {
                    debug!(
                        "ensure_shulker_in_hotbar_slot_0: Will shift-click hotbar slot 0 to clear it (empty slot {} available)", 
                        _empty
                    );
                    // Use shift-click to move item from hotbar slot 0 to inventory without affecting cursor
                    inv_handle.shift_click(HOTBAR_SLOT_0);
                    tokio::time::sleep(tokio::time::Duration::from_millis(350)).await;
                    
                    // Verify slot 0 is now empty and shulker is still in cursor
                    let verify_slots = inv_handle.slots().ok_or_else(|| "Inventory closed".to_string())?;
                    let verify_carried = carried_item(&client);
                    
                    debug!(
                        "ensure_shulker_in_hotbar_slot_0: After shift-click - Hotbar slot 0: {} x{}, Cursor: {} x{}", 
                        verify_slots.get(HOTBAR_SLOT_0).map(|s| s.kind().to_string()).unwrap_or_else(|| "None".to_string()),
                        verify_slots.get(HOTBAR_SLOT_0).map(|s| s.count()).unwrap_or(0),
                        verify_carried.kind(),
                        verify_carried.count()
                    );
                    
                    if verify_slots.get(HOTBAR_SLOT_0).map(|s| s.count() > 0).unwrap_or(false) {
                        error!("ensure_shulker_in_hotbar_slot_0: Failed to clear hotbar slot 0 - item still present after shift-click");
                        return Err("Failed to clear hotbar slot 0 - item still present".to_string());
                    }
                    // Rare race: the shift-click can shuffle stacks in a way that leaves the
                    // shulker elsewhere (e.g. it stacked onto a matching shulker in the main
                    // inventory). If the cursor no longer holds a shulker we fall through to
                    // a full inventory search and move.
                        if verify_carried.count() == 0 || !shulker::is_shulker_box(&verify_carried.kind().to_string()) {
                        // Shulker lost from cursor, search for it in inventory
                        warn!(
                            "ensure_shulker_in_hotbar_slot_0: SHULKER LOST from cursor during hotbar clearing! Cursor now: {} x{}", 
                            verify_carried.kind(), verify_carried.count()
                        );
                        drop(inv_handle);
                        // Re-open inventory and search for shulker
                        let inv_handle = client.open_inventory()
                            .ok_or_else(|| "Failed to open inventory".to_string())?;
                        let all_slots = inv_handle.slots()
                            .ok_or_else(|| "Inventory closed".to_string())?;
                        
                        // Search for shulker in inventory
                        let mut shulker_slot: Option<usize> = None;
                        for (i, slot_item) in all_slots.iter().enumerate() {
                            if slot_item.count() > 0 && super::shulker::is_shulker_box(&slot_item.kind().to_string()) {
                                shulker_slot = Some(i);
                                info!(
                                    "ensure_shulker_in_hotbar_slot_0: Found lost shulker in slot {} after cursor loss", 
                                    i
                                );
                                break;
                            }
                        }
                        
                        if let Some(shulker_idx) = shulker_slot {
                            info!(
                                "ensure_shulker_in_hotbar_slot_0: Recovering shulker from slot {} to hotbar slot 0", 
                                shulker_idx
                            );
                            // Pick up shulker from its current slot
                            inv_handle.click(PickupClick::Left {
                                slot: Some(shulker_idx as u16),
                            });
                            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                            
                            // Place in hotbar slot 0
                            inv_handle.click(PickupClick::Left {
                                slot: Some(HOTBAR_SLOT_0 as u16),
                            });
                            tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                            
                            // Verify it's now in hotbar slot 0
                            let updated_slots = inv_handle.slots().ok_or_else(|| "Inventory closed".to_string())?;
                            if let Some(slot_item) = updated_slots.get(HOTBAR_SLOT_0) {
                                if slot_item.count() > 0 && super::shulker::is_shulker_box(&slot_item.kind().to_string()) {
                                    debug!("ensure_shulker_in_hotbar_slot_0: SUCCESS - Shulker recovered to hotbar slot 0");
                                    drop(inv_handle);
                                    return Ok(());
                                }
                            }
                            error!("ensure_shulker_in_hotbar_slot_0: Failed to move recovered shulker to hotbar slot 0");
                            drop(inv_handle);
                            return Err("Failed to move shulker to hotbar slot 0 after recovery".to_string());
                        } else {
                            error!("ensure_shulker_in_hotbar_slot_0: Shulker LOST and NOT FOUND anywhere in inventory!");
                            drop(inv_handle);
                            return Err("Shulker lost from cursor and not found in inventory".to_string());
                        }
                    }
                    debug!("ensure_shulker_in_hotbar_slot_0: Hotbar slot 0 cleared, shulker still in cursor - proceeding with placement");
                    
                    // Now place shulker from cursor into hotbar slot 0 (which should be empty)
                    debug!("ensure_shulker_in_hotbar_slot_0: Left-clicking on hotbar slot 0 to place shulker from cursor");
                    inv_handle.click(PickupClick::Left {
                        slot: Some(HOTBAR_SLOT_0 as u16),
                    });
                    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                    
                    // Verify it's now in hotbar slot 0
                    let updated_slots = inv_handle.slots().ok_or_else(|| "Inventory closed".to_string())?;
                    let final_cursor = carried_item(&client);
                    debug!(
                        "ensure_shulker_in_hotbar_slot_0: After placement - Hotbar slot 0: {} x{}, Cursor: {} x{}", 
                        updated_slots.get(HOTBAR_SLOT_0).map(|s| s.kind().to_string()).unwrap_or_else(|| "None".to_string()),
                        updated_slots.get(HOTBAR_SLOT_0).map(|s| s.count()).unwrap_or(0),
                        final_cursor.kind(),
                        final_cursor.count()
                    );
                    
                    if let Some(slot_item) = updated_slots.get(HOTBAR_SLOT_0) {
                        if slot_item.count() > 0 && shulker::is_shulker_box(&slot_item.kind().to_string()) {
                            debug!("ensure_shulker_in_hotbar_slot_0: SUCCESS - Shulker placed in hotbar slot 0");
                            drop(inv_handle);
                            return Ok(());
                        }
                    }
                    
                    // Shulker placement failed - search for it in inventory and retry
                    warn!("ensure_shulker_in_hotbar_slot_0: Shulker placement from cursor FAILED, attempting recovery");
                    drop(inv_handle);
                    return recover_shulker_to_slot_0(bot, &client).await;
                } else {
                    error!(
                        "ensure_shulker_in_hotbar_slot_0: No empty inventory slot to move hotbar slot 0 item ({} x{})", 
                        slot_item.kind(), slot_item.count()
                    );
                    drop(inv_handle);
                    return Err("No empty inventory slot to move hotbar slot 0 item".to_string());
                }
            } else {
                // Hotbar slot 0 is already empty, place shulker from cursor
                debug!("ensure_shulker_in_hotbar_slot_0: Hotbar slot 0 is EMPTY, placing shulker from cursor directly");
                inv_handle.click(PickupClick::Left {
                    slot: Some(HOTBAR_SLOT_0 as u16),
                });
                tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                
                // Verify it's now in hotbar slot 0
                let updated_slots = inv_handle.slots().ok_or_else(|| "Inventory closed".to_string())?;
                let final_cursor = carried_item(&client);
                debug!(
                    "ensure_shulker_in_hotbar_slot_0: After placement - Hotbar slot 0: {} x{}, Cursor: {} x{}", 
                    updated_slots.get(HOTBAR_SLOT_0).map(|s| s.kind().to_string()).unwrap_or_else(|| "None".to_string()),
                    updated_slots.get(HOTBAR_SLOT_0).map(|s| s.count()).unwrap_or(0),
                    final_cursor.kind(),
                    final_cursor.count()
                );
                
                if let Some(slot_item) = updated_slots.get(HOTBAR_SLOT_0) {
                    if slot_item.count() > 0 && shulker::is_shulker_box(&slot_item.kind().to_string()) {
                        debug!("ensure_shulker_in_hotbar_slot_0: SUCCESS - Shulker placed in hotbar slot 0");
                        drop(inv_handle);
                        return Ok(());
                    }
                }
                
                // Shulker placement failed - search for it in inventory and retry
                warn!("ensure_shulker_in_hotbar_slot_0: Shulker placement to empty slot 0 FAILED, attempting recovery");
                drop(inv_handle);
                return recover_shulker_to_slot_0(bot, &client).await;
            }
        }
    }

    // Shulker is not in cursor, or placement failed - search for it in inventory/hotbar
    debug!("ensure_shulker_in_hotbar_slot_0: Shulker NOT in cursor, searching inventory/hotbar");
    // Refresh slots in case we need to search
    let all_slots = inv_handle.slots().ok_or_else(|| "Inventory closed".to_string())?;
    let mut shulker_slot: Option<usize> = None;
    for (i, slot_item) in all_slots.iter().enumerate() {
        if slot_item.count() > 0 && shulker::is_shulker_box(&slot_item.kind().to_string()) {
            shulker_slot = Some(i);
            info!(
                "ensure_shulker_in_hotbar_slot_0: Found shulker in slot {} ({})", 
                i, slot_item.kind()
            );
            break;
        }
    }

    if let Some(shulker_idx) = shulker_slot {
        info!(
            "ensure_shulker_in_hotbar_slot_0: Moving shulker from slot {} to hotbar slot 0", 
            shulker_idx
        );
        
        // Clear hotbar slot 0 first if needed
        if let Some(slot_item) = all_slots.get(HOTBAR_SLOT_0) {
            if slot_item.count() > 0 {
                debug!(
                    "ensure_shulker_in_hotbar_slot_0: Clearing hotbar slot 0 (has {} x{})", 
                    slot_item.kind(), slot_item.count()
                );
                // Move item from hotbar slot 0 to inventory
                let mut empty_slot: Option<usize> = None;
                for i in 9..36 {
                    if all_slots.get(i).map(|s| s.count() == 0).unwrap_or(true) {
                        empty_slot = Some(i);
                        break;
                    }
                }
                if let Some(_empty) = empty_slot {
                    debug!("ensure_shulker_in_hotbar_slot_0: Moving hotbar slot 0 item to slot {}", _empty);
                    inv_handle.click(PickupClick::Left {
                        slot: Some(HOTBAR_SLOT_0 as u16),
                    });
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                    inv_handle.click(PickupClick::Left {
                        slot: Some(_empty as u16),
                    });
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                } else {
                    warn!("ensure_shulker_in_hotbar_slot_0: No empty slot to move hotbar item, will attempt swap");
                }
            }
        }
        
        // Pick up shulker from its current slot
        debug!("ensure_shulker_in_hotbar_slot_0: Picking up shulker from slot {}", shulker_idx);
        inv_handle.click(PickupClick::Left {
            slot: Some(shulker_idx as u16),
        });
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        
        // Verify cursor now has shulker
        let cursor_after_pickup = carried_item(&client);
        debug!(
            "ensure_shulker_in_hotbar_slot_0: After pickup - Cursor: {} x{}", 
            cursor_after_pickup.kind(), cursor_after_pickup.count()
        );
        
        // Place in hotbar slot 0
        debug!("ensure_shulker_in_hotbar_slot_0: Placing shulker in hotbar slot 0");
        inv_handle.click(PickupClick::Left {
            slot: Some(HOTBAR_SLOT_0 as u16),
        });
        tokio::time::sleep(tokio::time::Duration::from_millis(400)).await;
        
        // Verify it's now in hotbar slot 0. We poll a few times because the server's
        // inventory-update packet can arrive after our click ACK, so the local slot view
        // may still show the pre-click state on the first read.
        let mut verified = false;
        for verify_attempt in 0..5 {
            let updated_slots = inv_handle.slots().ok_or_else(|| "Inventory closed".to_string())?;
            if let Some(slot_item) = updated_slots.get(HOTBAR_SLOT_0) {
                if slot_item.count() > 0 && super::shulker::is_shulker_box(&slot_item.kind().to_string()) {
                    debug!("ensure_shulker_in_hotbar_slot_0: SUCCESS - Shulker now in hotbar slot 0 (verified on attempt {})", verify_attempt + 1);
                    verified = true;
                    break;
                }
            }
            if verify_attempt < 4 {
                debug!(
                    "ensure_shulker_in_hotbar_slot_0: Verification attempt {} - shulker not in hotbar slot 0 yet, waiting...", 
                    verify_attempt + 1
                );
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
            }
        }
        
        if verified {
            drop(inv_handle);
            return Ok(());
        }
        
        // Log final state on failure
        let final_slots = inv_handle.slots().ok_or_else(|| "Inventory closed".to_string())?;
        let final_cursor = carried_item(&client);
        error!(
            "ensure_shulker_in_hotbar_slot_0: FAILED to move shulker to hotbar slot 0. Final state - Hotbar slot 0: {} x{}, Cursor: {} x{}", 
            final_slots.get(HOTBAR_SLOT_0).map(|s| s.kind().to_string()).unwrap_or_else(|| "None".to_string()),
            final_slots.get(HOTBAR_SLOT_0).map(|s| s.count()).unwrap_or(0),
            final_cursor.kind(),
            final_cursor.count()
        );
        
        drop(inv_handle);
        return Err(format!("Failed to move shulker from slot {} to hotbar slot 0", shulker_idx));
    }

    // Log final state when shulker not found
    let final_cursor = carried_item(&client);
    error!(
        "ensure_shulker_in_hotbar_slot_0: Shulker NOT FOUND in inventory or cursor. Cursor: {} x{}", 
        final_cursor.kind(), final_cursor.count()
    );
    
    drop(inv_handle);
    Err("Shulker not found in inventory or cursor".to_string())
}

/// Recovery function to find a shulker anywhere in the inventory and move it to hotbar slot 0.
/// This is called when a normal placement fails and we need to search for where the shulker ended up.
async fn recover_shulker_to_slot_0(_bot: &Bot, client: &azalea::Client) -> Result<(), String> {
    use azalea::inventory::operations::PickupClick;
    
    const MAX_RETRIES: u32 = 3;
    
    warn!("recover_shulker_to_slot_0: Starting shulker recovery process");
    
    for retry in 0..MAX_RETRIES {
        debug!("recover_shulker_to_slot_0: Recovery attempt {}/{}", retry + 1, MAX_RETRIES);
        
        // Wait a bit for server sync before each attempt
        tokio::time::sleep(tokio::time::Duration::from_millis(350)).await;
        
        let inv_handle = client.open_inventory()
            .ok_or_else(|| {
                error!("recover_shulker_to_slot_0: Failed to open inventory");
                "Failed to open inventory for shulker recovery".to_string()
            })?;
        let all_slots = inv_handle.slots()
            .ok_or_else(|| {
                error!("recover_shulker_to_slot_0: Inventory closed unexpectedly");
                "Inventory closed during shulker recovery".to_string()
            })?;
        
        // Log current state
        let carried = carried_item(client);
        debug!(
            "recover_shulker_to_slot_0: Current state - Cursor: {} x{}, Hotbar slot 0: {} x{}", 
            carried.kind(), 
            carried.count(),
            all_slots.get(HOTBAR_SLOT_0).map(|s| s.kind().to_string()).unwrap_or_else(|| "None".to_string()),
            all_slots.get(HOTBAR_SLOT_0).map(|s| s.count()).unwrap_or(0)
        );
        
        // First check if shulker is already in slot 0 (maybe it arrived late)
        if let Some(slot_item) = all_slots.get(HOTBAR_SLOT_0) {
            if slot_item.count() > 0 && shulker::is_shulker_box(&slot_item.kind().to_string()) {
                debug!("recover_shulker_to_slot_0: SUCCESS - Shulker already in hotbar slot 0 (delayed sync?)");
                drop(inv_handle);
                return Ok(());
            }
        }
        
        // Check if shulker is still in cursor
        if carried.count() > 0 && shulker::is_shulker_box(&carried.kind().to_string()) {
            debug!("recover_shulker_to_slot_0: Shulker found in CURSOR, attempting to place in hotbar slot 0");
            
            // Clear hotbar slot 0 first if needed
            if let Some(slot_item) = all_slots.get(HOTBAR_SLOT_0) {
                if slot_item.count() > 0 {
                    debug!(
                        "recover_shulker_to_slot_0: Clearing hotbar slot 0 (has {} x{})", 
                        slot_item.kind(), slot_item.count()
                    );
                    inv_handle.shift_click(HOTBAR_SLOT_0);
                    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                }
            }
            
            debug!("recover_shulker_to_slot_0: Left-clicking hotbar slot 0 to place shulker from cursor");
            inv_handle.click(PickupClick::Left {
                slot: Some(HOTBAR_SLOT_0 as u16),
            });
            tokio::time::sleep(tokio::time::Duration::from_millis(350)).await;
            
            // Verify placement
            let updated_slots = inv_handle.slots().ok_or_else(|| "Inventory closed".to_string())?;
            let final_cursor = carried_item(client);
            debug!(
                "recover_shulker_to_slot_0: After placement - Hotbar slot 0: {} x{}, Cursor: {} x{}", 
                updated_slots.get(HOTBAR_SLOT_0).map(|s| s.kind().to_string()).unwrap_or_else(|| "None".to_string()),
                updated_slots.get(HOTBAR_SLOT_0).map(|s| s.count()).unwrap_or(0),
                final_cursor.kind(),
                final_cursor.count()
            );
            
            if let Some(slot_item) = updated_slots.get(HOTBAR_SLOT_0) {
                if slot_item.count() > 0 && shulker::is_shulker_box(&slot_item.kind().to_string()) {
                    debug!("recover_shulker_to_slot_0: SUCCESS - Shulker placed in hotbar slot 0 from cursor");
                    drop(inv_handle);
                    return Ok(());
                }
            }
            warn!("recover_shulker_to_slot_0: Placement from cursor failed, will retry");
            drop(inv_handle);
            continue; // Retry
        }
        
        // Search for shulker in ALL slots (inventory + hotbar)
        debug!("recover_shulker_to_slot_0: Searching all inventory slots for shulker");
        let mut shulker_slot: Option<usize> = None;
        for (i, slot_item) in all_slots.iter().enumerate() {
            if slot_item.count() > 0 && shulker::is_shulker_box(&slot_item.kind().to_string()) {
                shulker_slot = Some(i);
                let slot_type = if i < 9 {
                    "crafting/armor"
                } else if i < 36 {
                    "inventory"
                } else if i < 45 {
                    "hotbar"
                } else {
                    "unknown"
                };
                info!(
                    "recover_shulker_to_slot_0: Found shulker in {} slot {} (idx {}): {}", 
                    slot_type, if i >= 36 { i - 36 } else { i }, i, slot_item.kind()
                );
                break;
            }
        }
        
        if let Some(shulker_idx) = shulker_slot {
            if shulker_idx == HOTBAR_SLOT_0 {
                debug!("recover_shulker_to_slot_0: SUCCESS - Shulker is already in hotbar slot 0");
                drop(inv_handle);
                return Ok(());
            }
            
            debug!("recover_shulker_to_slot_0: Moving shulker from slot {} to hotbar slot 0", shulker_idx);
            
            // Clear hotbar slot 0 first if needed
            if let Some(slot_item) = all_slots.get(HOTBAR_SLOT_0) {
                if slot_item.count() > 0 {
                    debug!(
                        "recover_shulker_to_slot_0: Clearing hotbar slot 0 first (has {} x{})", 
                        slot_item.kind(), slot_item.count()
                    );
                    inv_handle.shift_click(HOTBAR_SLOT_0);
                    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                }
            }
            
            // Pick up shulker from its current slot
            debug!("recover_shulker_to_slot_0: Picking up shulker from slot {}", shulker_idx);
            inv_handle.click(PickupClick::Left {
                slot: Some(shulker_idx as u16),
            });
            tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
            
            // Verify pickup
            let cursor_after = carried_item(client);
            debug!(
                "recover_shulker_to_slot_0: After pickup - Cursor: {} x{}", 
                cursor_after.kind(), cursor_after.count()
            );
            
            // Place in hotbar slot 0
            debug!("recover_shulker_to_slot_0: Placing shulker in hotbar slot 0");
            inv_handle.click(PickupClick::Left {
                slot: Some(HOTBAR_SLOT_0 as u16),
            });
            tokio::time::sleep(tokio::time::Duration::from_millis(350)).await;
            
            // Verify placement (with retries for timing)
            for verify_attempt in 0..5 {
                let updated_slots = inv_handle.slots().ok_or_else(|| "Inventory closed".to_string())?;
                if let Some(slot_item) = updated_slots.get(HOTBAR_SLOT_0) {
                    if slot_item.count() > 0 && shulker::is_shulker_box(&slot_item.kind().to_string()) {
                        info!(
                            "recover_shulker_to_slot_0: SUCCESS - Shulker moved to hotbar slot 0 (verified on attempt {})", 
                            verify_attempt + 1
                        );
                        drop(inv_handle);
                        return Ok(());
                    }
                }
                if verify_attempt < 4 {
                    debug!(
                        "recover_shulker_to_slot_0: Verification attempt {} - shulker not in slot 0 yet", 
                        verify_attempt + 1
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                }
            }
            
            // Log final state on verification failure
            let final_slots = inv_handle.slots().ok_or_else(|| "Inventory closed".to_string())?;
            let final_cursor = carried_item(client);
            warn!(
                "recover_shulker_to_slot_0: Verification failed. Hotbar slot 0: {} x{}, Cursor: {} x{}", 
                final_slots.get(HOTBAR_SLOT_0).map(|s| s.kind().to_string()).unwrap_or_else(|| "None".to_string()),
                final_slots.get(HOTBAR_SLOT_0).map(|s| s.count()).unwrap_or(0),
                final_cursor.kind(),
                final_cursor.count()
            );
            
            drop(inv_handle);
            // Continue to next retry attempt
        } else {
            error!("recover_shulker_to_slot_0: Shulker NOT FOUND anywhere in inventory or cursor!");
            drop(inv_handle);
            // No shulker found at all
            return Err("Shulker not found anywhere in inventory during recovery".to_string());
        }
    }
    
    error!("recover_shulker_to_slot_0: FAILED after {} attempts", MAX_RETRIES);
    Err("Failed to recover shulker to hotbar slot 0 after multiple attempts".to_string())
}

