//! Trade GUI automation

use azalea::inventory::operations::PickupClick;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::constants::{DELAY_CONTAINER_SYNC_MS, DOUBLE_CHEST_SLOTS, SHULKER_BOX_SLOTS};
use crate::messages::TradeItem;
use super::Bot;

/// Trade slot helper functions
///
/// The trade GUI is a standard 9x6 double-chest container (54 slots indexed 0..53).
/// Layout (rows top-to-bottom, cols left-to-right):
///   Rows 0..2, cols 0..3  -> bot offer slots (12)
///   Row  0..2, col  4     -> divider column (glass panes, unused)
///   Rows 0..2, cols 5..8  -> player offer slots (12)
///   Rows 4..5, cols 0..1  -> lime wool accept buttons (4, duplicated for easier clicking)
///   Rows 4..5, cols 2..3  -> red  wool cancel buttons (4)
///   Rows 4..5, cols 5..8  -> player status dyes (8) - gray=not ready, magenta/lime=ready
/// Index math below uses `row * 9 + col` to map (row, col) into the flat slot index.
pub fn trade_bot_offer_slots() -> Vec<usize> {
    // 9x6 menu:
    // - rows 0..2 are trade item slots
    // - cols 0..3 are bot side (12 slots)
    let mut slots = Vec::new();
    for row in 0..3 {
        for col in 0..4 {
            slots.push(row * 9 + col);
        }
    }
    slots
}

pub fn trade_player_offer_slots() -> Vec<usize> {
    // cols 5..8 are player side (12 slots)
    let mut slots = Vec::new();
    for row in 0..3 {
        for col in 5..9 {
            slots.push(row * 9 + col);
        }
    }
    slots
}

pub fn trade_player_status_slots() -> Vec<usize> {
    // rows 4..5 contain status dyes on the right side (8 slots)
    let mut slots = Vec::new();
    for row in 4..6 {
        for col in 5..9 {
            slots.push(row * 9 + col);
        }
    }
    slots
}

pub fn trade_accept_slots() -> Vec<usize> {
    // rows 4..5 cols 0..1 are lime wool accept buttons (4 slots)
    vec![(4 * 9), 4 * 9 + 1, (5 * 9), 5 * 9 + 1]
}

pub fn trade_cancel_slots() -> Vec<usize> {
    // rows 4..5 cols 2..3 are red wool cancel buttons (4 slots)
    vec![4 * 9 + 2, 4 * 9 + 3, 5 * 9 + 2, 5 * 9 + 3]
}

/// Wait for trade menu to open or failure message
pub async fn wait_for_trade_menu_or_failure(
    bot: &Bot,
    timeout: tokio::time::Duration,
    mut chat_rx: broadcast::Receiver<String>,
) -> Result<azalea::container::ContainerHandleRef, String> {
    let client = bot
        .client
        .read()
        .await
        .clone()
        .ok_or_else(|| "Bot not connected".to_string())?;

    let start = tokio::time::Instant::now();
    while start.elapsed() < timeout {
        // 1) Failure messages
        match chat_rx.try_recv() {
            Ok(msg) => {
                let msg_l = msg.to_lowercase();
                if msg_l.contains("not been accepted") {
                    return Err("Your trade request has not been accepted!".to_string());
                }
                if msg_l.contains("aborted the trade") || msg_l.contains("aborted the trade message") {
                    return Err("You aborted the trade.".to_string());
                }
            }
            Err(broadcast::error::TryRecvError::Empty) => {}
            Err(broadcast::error::TryRecvError::Lagged(_)) => {}
            Err(broadcast::error::TryRecvError::Closed) => {}
        }

        // 2) Menu open
        let inv = client.get_inventory();
        if inv.id() != 0 {
            let contents = inv.contents();
            let contents_len = contents.as_ref().map(|c| c.len()).unwrap_or(0);
            if contents_len == DOUBLE_CHEST_SLOTS {
                // A 54-slot container alone is not proof this is the trade GUI -
                // any double chest is also 54. Stronger identification: validate
                // wool buttons and dye indicators exist at their expected slots.
                if let Some(c) = contents {
                    let accept_slots = trade_accept_slots();
                    let cancel_slots = trade_cancel_slots();
                    let status_slots = trade_player_status_slots();

                    let accept_ok = accept_slots.iter().all(|&s| {
                        c.get(s)
                            .map(|st| st.kind().to_string() == "minecraft:lime_wool")
                            .unwrap_or(false)
                    });
                    let cancel_ok = cancel_slots.iter().all(|&s| {
                        c.get(s)
                            .map(|st| st.kind().to_string() == "minecraft:red_wool")
                            .unwrap_or(false)
                    });
                    let status_ok = status_slots.iter().all(|&s| {
                        c.get(s)
                            .map(|st| {
                                let k = st.kind().to_string();
                                k == "minecraft:gray_dye"
                                    || k == "minecraft:magenta_dye"
                                    || k == "minecraft:lime_dye"
                            })
                            .unwrap_or(false)
                    });

                    if !(accept_ok && cancel_ok && status_ok) {
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                        continue;
                    }
                }
                return Ok(inv);
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    Err("Trade request timed out (not accepted)".to_string())
}

/// Place items from bot inventory into trade offer slots
pub async fn place_items_from_inventory_into_trade(
    bot: &Bot,
    inv: &azalea::container::ContainerHandleRef,
    item: &str,
    amount: i32,
) -> Result<(), String> {
    if amount <= 0 {
        return Ok(());
    }

    let target_id = Bot::normalize_item_id(item);
    let client = bot
        .client
        .read()
        .await
        .clone()
        .ok_or_else(|| "Bot not connected".to_string())?;

    let offer_slots = trade_bot_offer_slots();
    let contents_len = inv
        .contents()
        .ok_or_else(|| "Trade menu closed".to_string())?
        .len();

    // CRITICAL: Clear cursor before any inventory operations
    // If cursor has items from a previous operation, clicking will SWAP instead of pick up
    // which corrupts the trade state and can leak items into unintended slots or even drop
    // them onto the floor when clicking outside. This must run before every placement pass.
    let cursor_before = super::inventory::carried_item(&client);
    if cursor_before.count() > 0 {
        warn!(
            "place_items_from_inventory_into_trade: Cursor has {}x {} - clearing it first",
            cursor_before.count(), cursor_before.kind()
        );
        // Click outside to drop cursor items (they'll go back to inventory if possible)
        inv.click(PickupClick::LeftOutside);
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        
        // Verify cursor is now empty
        let cursor_after = super::inventory::carried_item(&client);
        if cursor_after.count() > 0 {
            error!(
                "place_items_from_inventory_into_trade: Failed to clear cursor - still has {}x {}",
                cursor_after.count(), cursor_after.kind()
            );
            return Err(format!(
                "Failed to clear cursor before trade placement - cursor has {}x {}",
                cursor_after.count(), cursor_after.kind()
            ));
        }
        debug!("place_items_from_inventory_into_trade: Cursor cleared successfully");
    }

    // CRITICAL: Wait for inventory to fully sync after trade menu opens.
    // Without this delay, the inventory state may be stale and items won't be found.
    // DELAY_CONTAINER_SYNC_MS (450) is shared with bot/shulker::open_shulker_at_station
    // — same class of wait (container-open ACK from server).
    tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_CONTAINER_SYNC_MS)).await;

    // Log initial inventory state for debugging
    {
        let slots_all = inv.slots().ok_or_else(|| "Trade menu closed".to_string())?;
        let mut found_items: Vec<String> = Vec::new();
        let mut total_count = 0i32;
        for (i, stack) in slots_all.iter().enumerate().skip(contents_len) {
            if stack.count() > 0 && Bot::normalize_item_id(&stack.kind().to_string()) == target_id {
                let slot_type = if i >= contents_len + SHULKER_BOX_SLOTS { "hotbar" } else { "inventory" };
                found_items.push(format!("slot {} ({}): {}x", i, slot_type, stack.count()));
                total_count += stack.count();
            }
        }
        debug!(
            "Trade placement: need {}x {}, found {} total in {} stacks: {:?}",
            amount, item, total_count, found_items.len(), found_items
        );
    }

    let mut remaining = amount;
    let mut placed_count = 0i32;
    
    while remaining > 0 {
        // Wait for inventory state to sync from server
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        let slots_all = inv.slots().ok_or_else(|| "Trade menu closed".to_string())?;

        // Find the best-fit stack from bot inventory (includes both inventory AND hotbar).
        // Trade menu slots: 0-53 (container), 54-80 (inventory slots 9-35), 81-89 (hotbar slots 0-8).
        // Best-fit strategy: prefer the largest stack that still fits into `remaining` so we can
        // place the WHOLE stack in one left-click (fast path). Only fall back to a too-large stack
        // (and then partial right-click placement below) when no fitting stack exists.
        let mut best_slot: Option<(usize, i32)> = None;
        for (i, stack) in slots_all.iter().enumerate().skip(contents_len) {
            if stack.count() <= 0 {
                continue;
            }
            if Bot::normalize_item_id(&stack.kind().to_string()) != target_id {
                continue;
            }
            let c = stack.count();
            match best_slot {
                None => best_slot = Some((i, c)),
                Some((_bs, bc)) => {
                    // Prefer largest stack <= remaining; otherwise smallest stack.
                    let cand_better = if c <= remaining {
                        bc > remaining || c > bc
                    } else {
                        bc > remaining && c < bc
                    };
                    if cand_better {
                        best_slot = Some((i, c));
                    }
                }
            }
        }

        let Some((inv_slot, stack_count)) = best_slot else {
            // Log what items are actually in inventory for debugging
            let mut inventory_contents = Vec::new();
            for (i, stack) in slots_all.iter().enumerate().skip(contents_len) {
                if stack.count() > 0 {
                    let slot_type = if i >= contents_len + SHULKER_BOX_SLOTS { "hotbar" } else { "inv" };
                    inventory_contents.push(format!("slot {} ({}): {}x {}", i, slot_type, stack.count(), stack.kind()));
                }
            }
            warn!(
                "Failed to find {} in inventory (searched slots {}-{}, need {} more, placed {} so far). Inventory contents: {:?}",
                target_id, contents_len, slots_all.len(), remaining, placed_count, inventory_contents
            );
            return Err(format!(
                "Bot inventory lacks required items: missing {}x {} (searched {} slots, placed {} so far)",
                remaining, item, slots_all.len() - contents_len, placed_count
            ));
        };

        let slot_type = if inv_slot >= contents_len + SHULKER_BOX_SLOTS { "hotbar" } else { "inventory" };
        debug!("Found {}x {} in slot {} ({})", stack_count, target_id, inv_slot, slot_type);

        // Find an empty offer slot (or a slot with same item and room).
        let contents = inv.contents().ok_or_else(|| "Trade menu closed".to_string())?;
        let mut target_offer: Option<usize> = None;
        for &s in &offer_slots {
            let st = contents.get(s);
            // Check if slot is empty or has zero items
            if let Some(stack) = st {
                if stack.count() <= 0 {
                    target_offer = Some(s);
                    break;
                }
            } else {
                target_offer = Some(s);
                break;
            }
        }
        let target_offer = target_offer.ok_or_else(|| "Bot trade offer slots are full".to_string())?;

        // Pick up stack from bot inventory.
        debug!("Picking up from slot {}", inv_slot);
        inv.click(PickupClick::Left {
            slot: Some(inv_slot as u16),
        });
        tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;

        let carried = super::inventory::carried_item(&client);
        let carried_count = carried.count();
        if carried_count <= 0 || Bot::normalize_item_id(&carried.kind().to_string()) != target_id {
            // Maybe timing issue - wait and retry check
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
            let carried_retry = super::inventory::carried_item(&client);
            let carried_retry_count = carried_retry.count();
            
            if carried_retry_count <= 0 || Bot::normalize_item_id(&carried_retry.kind().to_string()) != target_id {
                // Put back if something weird happened.
                warn!("Failed to pick up item from slot {} - carried: {}x {}", 
                    inv_slot, carried_retry_count, carried_retry.kind());
                inv.click(PickupClick::Left {
                    slot: Some(inv_slot as u16),
                });
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                return Err(format!("Failed to pick up the expected item stack from slot {}", inv_slot));
            }
        }
        
        // Re-check carried count after potential retry
        let carried = super::inventory::carried_item(&client);
        let carried_count = carried.count();

        if carried_count <= remaining {
            // Fast path: the whole picked-up stack fits into what we still need, so
            // deposit it with a single left-click. This avoids N right-clicks (one per
            // item) and is the common case thanks to the best-fit selection above.
            // Place whole stack into offer.
            debug!("Placing {}x into offer slot {}", carried_count, target_offer);
            inv.click(PickupClick::Left {
                slot: Some(target_offer as u16),
            });
            // Wait longer for server to process the placement
            tokio::time::sleep(tokio::time::Duration::from_millis(350)).await;
            
            // CRITICAL: Verify placement by checking if cursor is NOW EMPTY.
            // Checking the slot contents can give stale data due to server sync delay.
            // If cursor is empty after placing, the items went somewhere (the slot).
            // DO NOT retry by clicking the slot again - that would pick up the items!
            let carried_after = super::inventory::carried_item(&client);
            if carried_after.count() == 0 {
                // Cursor is empty - items were successfully placed
                debug!("Placement verified: cursor is now empty after placing {}x", carried_count);
                remaining -= carried_count;
                placed_count += carried_count;
            } else if Bot::normalize_item_id(&carried_after.kind().to_string()) == target_id {
                // Still holding the same item type - placement might have failed
                // Wait a bit more for server sync before concluding failure
                tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                let carried_recheck = super::inventory::carried_item(&client);
                if carried_recheck.count() == 0 {
                    // Items were placed (just needed more time to sync)
                    debug!("Placement verified after extra wait: cursor is now empty");
                    remaining -= carried_count;
                    placed_count += carried_count;
                } else {
                    // Placement truly failed - cursor still has items
                    // Put items back in original slot instead of dropping
                    warn!("Placement to slot {} may have failed - cursor still has {}x. Putting back in inventory slot {}",
                        target_offer, carried_recheck.count(), inv_slot);
                    inv.click(PickupClick::Left {
                        slot: Some(inv_slot as u16),
                    });
                    tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
                    return Err(format!("Failed to place items in trade slot {} - items returned to inventory", target_offer));
                }
            } else {
                // Cursor has different item type (unusual) - count original as placed
                debug!("Cursor has different item after placement, assuming {}x were placed", carried_count);
                remaining -= carried_count;
                placed_count += carried_count;
            }
        } else {
            // Slow path: picked stack is larger than we need, so we can't dump the whole
            // thing. Right-click places exactly one item per click, then the surplus on
            // cursor is returned to the original inventory slot.
            // Place exactly `remaining` items via right-click (one per click).
            debug!("Placing {} items (partial) into offer slot {}", remaining, target_offer);
            let items_to_place = remaining;
            let cursor_before = carried_count;
            
            for _ in 0..items_to_place {
                inv.click(PickupClick::Right {
                    slot: Some(target_offer as u16),
                });
                // Slightly longer delay between right-clicks for server to process
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
            // Wait for all placements to sync
            tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
            
            // Check how many items remain on cursor to determine how many were placed
            let carried_after_partial = super::inventory::carried_item(&client);
            let cursor_after = carried_after_partial.count();
            let items_actually_placed = cursor_before - cursor_after;
            
            // Put remainder back in inventory.
            if cursor_after > 0 {
                inv.click(PickupClick::Left {
                    slot: Some(inv_slot as u16),
                });
                tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
            }
            
            if items_actually_placed > 0 {
                debug!("Partial placement: placed {} items (cursor went from {} to {})",
                    items_actually_placed, cursor_before, cursor_after);
                placed_count += items_actually_placed;
                remaining -= items_actually_placed;
            } else {
                warn!("Partial placement failed: cursor still has {} items", cursor_after);
                return Err(format!("Failed to place items via right-click in trade slot {}", target_offer));
            }
        }

        // Safety: ensure cursor is empty - put items back in inventory if needed.
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        let carried_now = super::inventory::carried_item(&client);
        if carried_now.is_present() {
            // DO NOT drop outside - that causes items on the floor!
            // Instead, try to put items back into inventory
            warn!("Cursor still has {}x {} after placement - attempting to return to inventory",
                carried_now.count(), carried_now.kind());
            
            // Find an empty inventory slot to put the items back
            let slots_all = inv.slots().ok_or_else(|| "Trade menu closed".to_string())?;
            let mut found_empty = false;
            for (i, stack) in slots_all.iter().enumerate().skip(contents_len) {
                if stack.count() == 0 {
                    inv.click(PickupClick::Left {
                        slot: Some(i as u16),
                    });
                    tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
                    found_empty = true;
                    break;
                }
            }
            
            if !found_empty {
                // No empty slot found - try the original slot as last resort
                warn!("No empty slot found, trying to place back in slot {}", inv_slot);
                inv.click(PickupClick::Left {
                    slot: Some(inv_slot as u16),
                });
                tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
            }
            
            // Verify cursor is now empty
            let carried_final = super::inventory::carried_item(&client);
            if carried_final.is_present() {
                // Still has items - this is a serious issue
                return Err(format!(
                    "Failed to clear cursor - {}x {} still on cursor after trade placement attempt",
                    carried_final.count(), carried_final.kind()
                ));
            }
        }
    }

    debug!("Trade placement complete: placed {} items", placed_count);
    Ok(())
}

/// Validate player items in trade GUI
///
/// Two orthogonal validation modes control how strict amount checks are:
/// - `flexible_validation`: accept any amount >= 1 of each expected item type, ignoring
///   the `amount` field entirely. Used for deposit flows where the user doesn't specify
///   a quantity and we take whatever they drop in.
/// - `require_exact_amount`: reject trades where the player offers MORE than expected.
///   Used for sell orders where the price is fixed and surplus must not be accepted.
///
/// Default behavior (both false): at least `amount` required, surplus is allowed and
/// later credited to the player's balance. Under-supplying is ALWAYS an error - this
/// is what prevents the "pay less than the price" exploit.
///
/// Returns Ok((found_items, validation_errors)) where:
/// - found_items: HashMap of normalized item IDs to amounts found
/// - validation_errors: Vec of validation error messages (empty if all OK)
fn validate_player_items(
    contents: &[azalea::inventory::ItemStack],
    player_slots: &[usize],
    player_offers: &[TradeItem],
    require_exact_amount: bool,
    flexible_validation: bool,
) -> (std::collections::HashMap<String, i32>, Vec<String>) {
    let mut validation_errors = Vec::new();
    let mut found_items: std::collections::HashMap<String, i32> = std::collections::HashMap::new();
    
    // If no player offers expected, all slots must be empty
    if player_offers.is_empty() {
        for &slot_idx in player_slots {
            if let Some(stack) = contents.get(slot_idx)
                && stack.count() > 0 {
                    let item_id = Bot::normalize_item_id(&stack.kind().to_string());
                    validation_errors.push(format!("Unexpected item: {} (no items expected)", item_id));
                }
        }
    } else {
        // Build normalized expected item IDs for quick lookup
        let expected_items: std::collections::HashMap<String, i32> = player_offers
            .iter()
            .map(|ti| (Bot::normalize_item_id(&ti.item), ti.amount))
            .collect();
        
        // Scan all player slots for items
        for &slot_idx in player_slots {
            if let Some(stack) = contents.get(slot_idx)
                && stack.count() > 0 {
                    let item_id = Bot::normalize_item_id(&stack.kind().to_string());
                    *found_items.entry(item_id.clone()).or_insert(0) += stack.count();
                    
                    // Check if this item is expected
                    if !expected_items.contains_key(&item_id) {
                        validation_errors.push(format!("Unexpected item type: {} (not in expected list)", item_id));
                    }
                }
        }
        
        // Validate expected items based on validation mode
        for (expected_id, expected_amount) in &expected_items {
            let got = found_items.get(expected_id).copied().unwrap_or(0);
            
            if flexible_validation {
                // FLEXIBLE MODE: Accept any amount >= 1 (for deposit without specified amount)
                // Only check that at least 1 of the expected item type was provided
                if got < 1 {
                    validation_errors.push(format!(
                        "Expected at least 1 {}, but none found in trade",
                        expected_id
                    ));
                }
            } else if got > *expected_amount {
                // More than expected
                if require_exact_amount {
                    // EXACT MODE: Reject excess items (for sell orders)
                    validation_errors.push(format!(
                        "Too many items: expected exactly {} {}, but {} in trade",
                        expected_amount, expected_id, got
                    ));
                }
                // DEFAULT: More than expected is OK - surplus will be credited to balance
            } else if got < *expected_amount {
                // STRICT: Must have at least the expected amount
                // This prevents exploits where player offers fewer items/diamonds than required
                validation_errors.push(format!(
                    "Insufficient items: expected {} {}, but only {} in trade",
                    expected_amount, expected_id, got
                ));
            }
        }
    }
    
    (found_items, validation_errors)
}

/// Execute a full trade with a player via the trade GUI
/// 
/// Returns the actual items received from the player (may differ from player_offers
/// if player offers fewer items - useful for partial payment with balance).
/// 
/// **Validation modes**:
/// - `require_exact_amount`: If true, reject trades where player offers MORE than expected.
///   Use for sell orders where exact quantity is required.
/// - `flexible_validation`: If true, accept any amount >= 1 of expected items (ignore amount field).
///   Use for deposit commands without a specified amount.
pub async fn execute_trade_with_player(
    bot: &Bot,
    target_username: &str,
    bot_offers: &[TradeItem],
    player_offers: &[TradeItem],
    require_exact_amount: bool,
    flexible_validation: bool,
) -> Result<Vec<TradeItem>, String> {
    let client = bot
        .client
        .read()
        .await
        .clone()
        .ok_or_else(|| "Bot not connected".to_string())?;

    // CRITICAL: Ensure entity is fully initialized before any inventory operations
    // This prevents panic: "Our client is missing a required component: Inventory"
    if !super::inventory::is_entity_ready(&client) {
        warn!("Entity not ready, waiting for initialization...");
        super::inventory::wait_for_entity_ready(&client).await?;
        debug!("Entity now ready for trade operations");
    }

    // Inventory hygiene: clear inventory into buffer chest if configured.
    super::inventory::ensure_inventory_empty(bot).await?;
    
    // CRITICAL: Move any items from hotbar to inventory before trade
    // This ensures hotbar slot 0 is free for any subsequent shulker operations
    // (e.g., if trade fails and rollback needs to do chest operations)
    if let Err(e) = super::inventory::move_hotbar_to_inventory(bot).await {
        warn!("Failed to clear hotbar before trade: {} - proceeding anyway", e);
    }

    // Close any open container first (avoids accidental interactions).
    let current = client.get_inventory();
    if current.id() != 0 {
        current.close();
        tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
    }

    // Send trade request.
    bot.send_chat_message(&format!("/trade {}", target_username)).await?;

    // Wait for GUI open (player acceptance).
    let chat_rx = bot.chat_subscribe();
    let inv = wait_for_trade_menu_or_failure(
        bot,
        tokio::time::Duration::from_millis(bot.trade_timeout_ms),
        chat_rx,
    )
    .await?;

    // Fill bot offers into left 12 slots.
    for ti in bot_offers {
        place_items_from_inventory_into_trade(bot, &inv, &ti.item, ti.amount)
            .await?;
    }

    // Verify bot offers were placed correctly
    {
        let contents = inv.contents().ok_or_else(|| "Trade menu closed".to_string())?;
        let bot_slots = trade_bot_offer_slots();
        let mut total_offered: std::collections::HashMap<String, i32> = std::collections::HashMap::new();
        
        for &slot_idx in &bot_slots {
            if let Some(stack) = contents.get(slot_idx)
                && stack.count() > 0 {
                    let item_id = Bot::normalize_item_id(&stack.kind().to_string());
                    *total_offered.entry(item_id).or_insert(0) += stack.count();
                }
        }
        
        // Check each expected offer
        for ti in bot_offers {
            let expected_id = Bot::normalize_item_id(&ti.item);
            let actual = total_offered.get(&expected_id).copied().unwrap_or(0);
            if actual != ti.amount {
                let error_msg = format!(
                    "Bot offer verification failed: expected {}x {}, but only {}x placed in trade GUI",
                    ti.amount, ti.item, actual
                );
                warn!("{}", error_msg);
                inv.close();
                return Err(error_msg);
            }
            debug!("Bot offer verified: {}x {} in trade GUI", actual, ti.item);
        }
    }

    // Start checking immediately for player confirmation (no gray_dye = either magenta or lime_dye)
    // and validate items, then accept immediately when ready.
    //
    // RACE CONDITION NOTES on accept-button detection:
    // 1) The status dye is the only visible signal that the player pressed accept, but
    //    the player can press accept, let us validate, then swap items before the server
    //    finalizes the trade. We defend against this by re-validating items on EVERY
    //    tick of this loop (not just the first time the dye flips), so a late swap is
    //    caught before we re-click accept.
    // 2) After the bot clicks accept, the menu may close almost immediately. A closed
    //    menu does NOT prove success - the server also closes it on rejection. The
    //    post-close branches below therefore verify success by checking the bot's own
    //    inventory: if the items we placed are still there, the trade was rejected.
    // 3) `last_click_time` rate-limits accept clicks to once per 250ms to avoid spamming
    //    the server and accidentally un-accepting (a second click on lime wool toggles off).
    let status_slots = trade_player_status_slots();
    let player_slots = trade_player_offer_slots();

    let start = tokio::time::Instant::now();
    let accept_slots = trade_accept_slots();
    let mut last_click_time = tokio::time::Instant::now();
    // Track actual items received from player (updated during validation)
    let mut actual_received: Vec<TradeItem> = Vec::new();
    // Track if the bot has clicked accept at least once (meaning we validated and accepted)
    let mut bot_accepted = false;
    // Track if we've ever successfully validated items (used for logging)
    let mut ever_validated = false;
    // Track what items the bot placed in the trade (for verifying trade success)
    let mut bot_items_placed: Vec<TradeItem> = Vec::new();
    for ti in bot_offers {
        bot_items_placed.push(TradeItem {
            item: Bot::normalize_item_id(&ti.item),
            amount: ti.amount,
        });
    }
    
    while start.elapsed() < tokio::time::Duration::from_millis(bot.trade_timeout_ms) {
        // Check if trade menu is still open before trying to get contents
        let current = client.get_inventory();
        if current.id() == 0 {
            // Trade menu closed - need to verify if trade actually succeeded or was rejected
            if bot_accepted && ever_validated {
                // Bot clicked accept and items were validated at some point
                // BUT the trade could have been REJECTED by server if player modified items after we accepted
                // CRITICAL: Verify trade success by checking if bot still has the items it was supposed to give away
                
                // Wait a moment for inventory to sync after trade closes
                tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                
                // Check if bot still has the items it placed in the trade
                // If bot still has them, trade was REJECTED (items returned)
                let inv_handle = client.open_inventory();
                if let Some(inv) = inv_handle {
                    let slots = inv.slots();
                    if let Some(all_slots) = slots {
                        for placed_item in &bot_items_placed {
                            if placed_item.amount <= 0 {
                                continue;
                            }
                            
                            // Count how many of this item the bot currently has in inventory
                            let mut found_in_inv = 0i32;
                            for slot in all_slots.iter() {
                                if slot.count() > 0 {
                                    let slot_item = Bot::normalize_item_id(&slot.kind().to_string());
                                    if slot_item == placed_item.item {
                                        found_in_inv += slot.count();
                                    }
                                }
                            }
                            
                            // If bot still has most of the items it was supposed to give away,
                            // the trade was likely REJECTED
                            // Use 80% threshold to account for possible rounding/partial issues
                            // CRITICAL: Threshold must be at least 1 when amount > 0, otherwise
                            // for amount=1, threshold=0 and "found >= 0" is always true!
                            let threshold = ((placed_item.amount as f64 * 0.8) as i32).max(1);
                            if found_in_inv >= threshold {
                                warn!(
                                    "Trade REJECTED: Bot still has {}x {} in inventory (placed {}x in trade). Items were NOT exchanged!",
                                    found_in_inv, placed_item.item, placed_item.amount
                                );
                                drop(inv);
                                
                                // Move any hotbar items to inventory for easier rollback processing
                                if let Err(e) = super::inventory::move_hotbar_to_inventory(bot).await {
                                    warn!("Failed to consolidate hotbar items after rejected trade: {}", e);
                                }
                                
                                return Err(format!(
                                    "Trade was rejected by server: items returned to bot inventory (found {}x {} after trade closed)",
                                    found_in_inv, placed_item.item
                                ));
                            }
                        }
                    }
                    drop(inv);
                }
                
                // Items not found in bot inventory = trade completed successfully
                info!("Trade completed (menu closed after accept)");
                // Move items from hotbar to inventory before returning
                if let Err(e) = super::inventory::move_hotbar_to_inventory(bot).await {
                    warn!("Failed to move hotbar items to inventory after trade: {}", e);
                }
                return Ok(actual_received);
            } else if ever_validated {
                // We validated but didn't click accept yet - player cancelled
                warn!("Trade cancelled by player before bot could accept (items were validated)");
                return Err("Trade cancelled by player before completion".to_string());
            } else {
                // Menu closed before validation - could be player cancel or server issue
                warn!("Trade menu closed before bot could validate items");
                return Err("Trade closed before items could be validated".to_string());
            }
        }

        let contents = match inv.contents() {
            Some(c) => c,
            None => {
                // Contents unavailable - check if menu closed (trade might have succeeded or failed)
                let check_current = client.get_inventory();
                if check_current.id() == 0 {
                    if bot_accepted && ever_validated {
                        // Need to verify trade actually succeeded (same logic as above)
                        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                        
                        let inv_handle = client.open_inventory();
                        if let Some(inv_check) = inv_handle {
                            if let Some(all_slots) = inv_check.slots() {
                                for placed_item in &bot_items_placed {
                                    if placed_item.amount <= 0 {
                                        continue;
                                    }
                                    let mut found_in_inv = 0i32;
                                    for slot in all_slots.iter() {
                                        if slot.count() > 0 {
                                            let slot_item = Bot::normalize_item_id(&slot.kind().to_string());
                                            if slot_item == placed_item.item {
                                                found_in_inv += slot.count();
                                            }
                                        }
                                    }
                                    // CRITICAL: Threshold must be at least 1 (see main check for explanation)
                                    let threshold = ((placed_item.amount as f64 * 0.8) as i32).max(1);
                                    if found_in_inv >= threshold {
                                        warn!(
                                            "Trade REJECTED (content check): Bot still has {}x {} in inventory",
                                            found_in_inv, placed_item.item
                                        );
                                        drop(inv_check);
                                        
                                        // Move any hotbar items to inventory for easier rollback processing
                                        if let Err(e) = super::inventory::move_hotbar_to_inventory(bot).await {
                                            warn!("Failed to consolidate hotbar items after rejected trade: {}", e);
                                        }
                                        
                                        return Err(format!(
                                            "Trade was rejected by server: items returned to bot inventory (found {}x {})",
                                            found_in_inv, placed_item.item
                                        ));
                                    }
                                }
                            }
                            drop(inv_check);
                        }
                        
                        info!("Trade completed (verified during content check)");
                        // Move items from hotbar to inventory before returning
                        if let Err(e) = super::inventory::move_hotbar_to_inventory(bot).await {
                            warn!("Failed to move hotbar items to inventory after trade: {}", e);
                        }
                        return Ok(actual_received);
                    } else {
                        warn!("Trade menu closed during content check before bot accepted");
                        return Err("Trade closed before items could be validated".to_string());
                    }
                }
                // Menu still open but contents unavailable - wait and retry
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                continue;
            }
        };
        
        // Check if there's no gray_dye (meaning ready: either magenta or lime_dye)
        let mut no_gray_dye = true;
        for &s in &status_slots {
            let kind = contents
                .get(s)
                .map(|st| st.kind().to_string())
                .unwrap_or_default();
            if kind == "minecraft:gray_dye" {
                no_gray_dye = false;
                break;
            }
        }

        if no_gray_dye {
            // Player is ready (no gray_dye = magenta or lime_dye)
            // CRITICAL FIX: Re-validate items EVERY time before clicking accept.
            // This prevents the race condition where the player first places the expected
            // items (we validate, set ever_validated=true), then swaps them out for junk
            // right before pressing accept. Without re-validation each tick we'd accept
            // the swapped state. See the RACE CONDITION NOTES above the outer loop.
            let contents_vec: Vec<azalea::inventory::ItemStack> = contents.to_vec();
            let (found_items, validation_errors) = validate_player_items(
                &contents_vec,
                &player_slots,
                player_offers,
                require_exact_amount,
                flexible_validation,
            );
            
            if !validation_errors.is_empty() {
                // Validation failed - close trade and return error
                error!("Trade validation failed: {}", validation_errors.join("; "));
                inv.close();
                return Err(format!("Trade validation failed: {}", validation_errors.join("; ")));
            }
            
            // Validation passed - update actual_received
            actual_received = found_items
                .into_iter()
                .map(|(item, amount)| TradeItem { item, amount })
                .collect();
            ever_validated = true;
            
            if !bot_accepted {
                let received_summary: Vec<String> = actual_received
                    .iter()
                    .map(|t| format!("{}x {}", t.amount, t.item))
                    .collect();
                info!("Items validated: received [{}] - accepting trade now", received_summary.join(", "));
            }

            // Items are correct and player is ready (no gray_dye), click lime wool accept button
            // Click accept button (but not too frequently - max once per 250ms)
            if last_click_time.elapsed() >= tokio::time::Duration::from_millis(250) {
                debug!("Clicking lime wool accept button (no gray_dye detected)");
                inv.click(PickupClick::Left {
                    slot: Some(accept_slots[0] as u16),
                });
                last_click_time = tokio::time::Instant::now();
                bot_accepted = true; // Mark that bot has clicked accept after validation
                
                // Check if trade completed after clicking
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                let current_after = client.get_inventory();
                if current_after.id() == 0 {
                    // Trade menu closed - need to verify trade actually succeeded
                    // Wait a moment for inventory to sync
                    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                    
                    let inv_handle = client.open_inventory();
                    if let Some(inv_check) = inv_handle {
                        if let Some(all_slots) = inv_check.slots() {
                            for placed_item in &bot_items_placed {
                                if placed_item.amount <= 0 {
                                    continue;
                                }
                                let mut found_in_inv = 0i32;
                                for slot in all_slots.iter() {
                                    if slot.count() > 0 {
                                        let slot_item = Bot::normalize_item_id(&slot.kind().to_string());
                                        if slot_item == placed_item.item {
                                            found_in_inv += slot.count();
                                        }
                                    }
                                }
                                // CRITICAL: Threshold must be at least 1 (see main check for explanation)
                                let threshold = ((placed_item.amount as f64 * 0.8) as i32).max(1);
                                if found_in_inv >= threshold {
                                    warn!(
                                        "Trade REJECTED (after accept): Bot still has {}x {} in inventory",
                                        found_in_inv, placed_item.item
                                    );
                                    drop(inv_check);
                                    
                                    // Move any hotbar items to inventory for easier rollback processing
                                    if let Err(e) = super::inventory::move_hotbar_to_inventory(bot).await {
                                        warn!("Failed to consolidate hotbar items after rejected trade: {}", e);
                                    }
                                    
                                    return Err(format!(
                                        "Trade was rejected by server: items returned to bot inventory (found {}x {})",
                                        found_in_inv, placed_item.item
                                    ));
                                }
                            }
                        }
                        drop(inv_check);
                    }
                    
                    // Trade completed successfully
                    info!("Trade completed (after accept click)");
                    // Move items from hotbar to inventory before returning
                    if let Err(e) = super::inventory::move_hotbar_to_inventory(bot).await {
                        warn!("Failed to move hotbar items to inventory after trade: {}", e);
                    }
                    return Ok(actual_received);
                }
            }
        } else {
            // Gray dye still present - player not ready yet
            if ever_validated {
                debug!("Gray dye detected again, player may be modifying items");
            }
        }

        // Check interval for trade status
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    // Timeout: check if trade completed
    let current = client.get_inventory();
    if current.id() == 0 {
        if bot_accepted && ever_validated {
            // Need to verify trade actually succeeded
            tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
            
            let inv_handle = client.open_inventory();
            if let Some(inv_check) = inv_handle {
                if let Some(all_slots) = inv_check.slots() {
                    for placed_item in &bot_items_placed {
                        if placed_item.amount <= 0 {
                            continue;
                        }
                        let mut found_in_inv = 0i32;
                        for slot in all_slots.iter() {
                            if slot.count() > 0 {
                                let slot_item = Bot::normalize_item_id(&slot.kind().to_string());
                                if slot_item == placed_item.item {
                                    found_in_inv += slot.count();
                                }
                            }
                        }
                        // CRITICAL: Threshold must be at least 1 (see main check for explanation)
                        let threshold = ((placed_item.amount as f64 * 0.8) as i32).max(1);
                        if found_in_inv >= threshold {
                            warn!(
                                "Trade REJECTED (timeout check): Bot still has {}x {} in inventory",
                                found_in_inv, placed_item.item
                            );
                            drop(inv_check);
                            
                            // Move any hotbar items to inventory for easier rollback processing
                            if let Err(e) = super::inventory::move_hotbar_to_inventory(bot).await {
                                warn!("Failed to consolidate hotbar items after rejected trade: {}", e);
                            }
                            
                            return Err(format!(
                                "Trade was rejected by server: items returned to bot inventory (found {}x {})",
                                found_in_inv, placed_item.item
                            ));
                        }
                    }
                }
                drop(inv_check);
            }
            
            // Trade completed - move items from hotbar to inventory
            info!("Trade completed (verified at timeout)");
            if let Err(e) = super::inventory::move_hotbar_to_inventory(bot).await {
                warn!("Failed to move hotbar items to inventory after trade: {}", e);
            }
            return Ok(actual_received);
        } else {
            warn!("Trade menu closed at timeout but bot had not accepted (validated: {}, accepted: {})", ever_validated, bot_accepted);
            return Err("Trade closed unexpectedly before bot could accept".to_string());
        }
    }

    inv.close();
    Err("Trade not ready: player did not confirm or items incorrect within timeout".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_trade_bot_offer_slots() {
        let slots = trade_bot_offer_slots();
        assert_eq!(slots.len(), 12);
        assert_eq!(slots, vec![0, 1, 2, 3, 9, 10, 11, 12, 18, 19, 20, 21]);
    }

    #[test]
    fn test_trade_player_offer_slots() {
        let slots = trade_player_offer_slots();
        assert_eq!(slots.len(), 12);
        assert_eq!(slots, vec![5, 6, 7, 8, 14, 15, 16, 17, 23, 24, 25, 26]);
    }

    #[test]
    fn test_trade_player_status_slots() {
        let slots = trade_player_status_slots();
        assert_eq!(slots.len(), 8);
        assert_eq!(slots, vec![41, 42, 43, 44, 50, 51, 52, 53]);
    }

    #[test]
    fn test_trade_accept_slots() {
        assert_eq!(trade_accept_slots(), vec![36, 37, 45, 46]);
    }

    #[test]
    fn test_trade_cancel_slots() {
        assert_eq!(trade_cancel_slots(), vec![38, 39, 47, 48]);
    }

    #[test]
    fn test_trade_slot_sets_disjoint() {
        let mut seen: HashSet<usize> = HashSet::new();
        for set in [
            trade_bot_offer_slots(),
            trade_player_offer_slots(),
            trade_player_status_slots(),
            trade_accept_slots(),
            trade_cancel_slots(),
        ] {
            for slot in set {
                assert!(seen.insert(slot), "slot {} appears in multiple sets", slot);
                assert!(slot < crate::constants::DOUBLE_CHEST_SLOTS, "slot {} out of double-chest range", slot);
            }
        }
    }
}
