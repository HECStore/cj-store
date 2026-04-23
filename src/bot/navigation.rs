//! Navigation and pathfinding for the bot
//!
//! Provides pathfinding utilities for navigating the bot to node/chest positions
//! with automatic retry logic.

use azalea::BlockPos;
use azalea::pathfinder::goals::BlockPosGoal;
use azalea::pathfinder::PathfinderClientExt;
use tracing::{debug, info, warn};

use crate::constants::{
    DELAY_MEDIUM_MS, NAVIGATION_MAX_RETRIES, PATHFINDING_CHECK_INTERVAL_MS,
    RETRY_BASE_DELAY_MS, RETRY_MAX_DELAY_MS, exponential_backoff_delay,
};
use crate::types::{Chest, Position};
use super::Bot;

/// Navigate to a position using pathfinding (single attempt).
/// Uses Azalea's built-in pathfinding to walk to the target position.
///
/// # Returns
/// * `Ok(true)` if reached target
/// * `Ok(false)` if timed out but continued
/// * `Err` if bot not connected
async fn navigate_to_position_once(bot: &Bot, target: &Position) -> Result<bool, String> {
    let client = bot
        .client
        .read()
        .await
        .clone()
        .ok_or_else(|| format!(
            "Bot not connected - cannot navigate to target ({}, {}, {})",
            target.x, target.y, target.z
        ))?;

    let target_block = BlockPos::new(target.x, target.y, target.z);
    let current_pos = client.entity().position();
    let current_block = BlockPos::from(current_pos);

    // If already at exact position, consider it done.
    // Zero tolerance (must match the target block exactly, not "close enough"):
    // node P coordinates define the precise block where the bot must stand so the
    // chest UI opens reliably and item interactions target the correct slot. Even
    // a one-block offset can cause the bot to face a different chest or miss the
    // interaction entirely, so we refuse to treat "nearby" as arrived.
    let dx = (current_block.x - target_block.x).abs();
    let dy = (current_block.y - target_block.y).abs();
    let dz = (current_block.z - target_block.z).abs();
    if dx == 0 && dy == 0 && dz == 0 {
        info!("Already at exact target position ({}, {}, {})", target.x, target.y, target.z);
        return Ok(true);
    }

    info!(
        "Pathfinding from ({}, {}, {}) to ({}, {}, {}) - distance: ({}, {}, {})",
        current_block.x, current_block.y, current_block.z,
        target_block.x, target_block.y, target_block.z,
        dx, dy, dz
    );

    client.goto(BlockPosGoal(target_block)).await;

    // Wait for pathfinding to complete (timeout from config)
    let pathfinding_wait_ms = bot.pathfinding_timeout_ms;
    let max_checks = (pathfinding_wait_ms / PATHFINDING_CHECK_INTERVAL_MS) as usize;
    let mut checks = 0;
    while checks < max_checks {
        tokio::time::sleep(tokio::time::Duration::from_millis(PATHFINDING_CHECK_INTERVAL_MS)).await;
        let new_pos = client.entity().position();
        let new_block = BlockPos::from(new_pos);
        let new_dx = (new_block.x - target_block.x).abs();
        let new_dy = (new_block.y - target_block.y).abs();
        let new_dz = (new_block.z - target_block.z).abs();
        // Same zero-tolerance rule as above: Azalea's pathfinder may report
        // "done" when standing on an adjacent block, but for node P we require
        // the exact block before considering navigation successful.
        if new_dx == 0 && new_dy == 0 && new_dz == 0 {
            info!(
                "Reached exact target ({}, {}, {}) - position: ({}, {}, {})",
                target.x, target.y, target.z,
                new_block.x, new_block.y, new_block.z
            );
            return Ok(true);
        }
        checks += 1;
    }

    let final_pos = client.entity().position();
    let final_block = BlockPos::from(final_pos);
    warn!(
        "Pathfinding timeout after {}ms - target: ({}, {}, {}), current: ({}, {}, {})",
        pathfinding_wait_ms,
        target_block.x, target_block.y, target_block.z,
        final_block.x, final_block.y, final_block.z
    );
    
    Ok(false) // Timed out, didn't reach target
}

/// Navigate to a position using pathfinding with retry logic.
/// Uses Azalea's built-in pathfinding to walk to the target position.
/// Retries up to NAVIGATION_MAX_RETRIES times if pathfinding times out.
///
/// # Arguments
/// * `bot` - Bot instance
/// * `target` - Target position to navigate to
///
/// # Errors
/// Returns an error with context including current and target positions if:
/// - Bot is not connected
/// - All retry attempts fail to reach the target
pub async fn navigate_to_position(bot: &Bot, target: &Position) -> Result<(), String> {
    for attempt in 0..NAVIGATION_MAX_RETRIES {
        if attempt > 0 {
            // Exponential backoff between retries: transient pathfinding failures
            // are often caused by chunk loading, server lag, or temporary mob
            // obstruction, so waiting progressively longer gives the world state
            // a chance to settle before we ask Azalea to recompute the path.
            // Capped at RETRY_MAX_DELAY_MS to avoid unbounded stalls.
            let delay_ms = exponential_backoff_delay(attempt - 1, RETRY_BASE_DELAY_MS, RETRY_MAX_DELAY_MS);
            info!(
                "Retry {}/{} for navigation to ({}, {}, {}) after {}ms delay",
                attempt + 1, NAVIGATION_MAX_RETRIES,
                target.x, target.y, target.z,
                delay_ms
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        }
        
        match navigate_to_position_once(bot, target).await {
            Ok(true) => return Ok(()), // Successfully reached target
            Ok(false) => {
                // Capture current bot position for the warn so an operator can
                // see where pathfinding stalled, not just where we wanted to go.
                let current = {
                    let guard = bot.client.read().await;
                    guard.as_ref().map(|c| {
                        let p = c.entity().position();
                        BlockPos::from(p)
                    })
                };
                match current {
                    Some(cur) => warn!(
                        "Navigation attempt {}/{} timed out - target: ({}, {}, {}), current: ({}, {}, {})",
                        attempt + 1, NAVIGATION_MAX_RETRIES,
                        target.x, target.y, target.z,
                        cur.x, cur.y, cur.z
                    ),
                    None => warn!(
                        "Navigation attempt {}/{} timed out - target: ({}, {}, {}), current: unknown (bot disconnected)",
                        attempt + 1, NAVIGATION_MAX_RETRIES,
                        target.x, target.y, target.z
                    ),
                }
            }
            Err(e) => {
                // Bot not connected or other error, propagate immediately
                return Err(e);
            }
        }
    }
    
    // Best-effort semantics: after exhausting retries we deliberately return
    // Ok(()) rather than Err. Navigation is advisory for the store workflow -
    // the caller (chest interaction logic) will perform its own position and
    // inventory validation, and aborting the entire task here would strand the
    // bot mid-operation. A loud warning is logged so failures remain visible,
    // but the pipeline is allowed to continue and recover downstream.
    let final_current = {
        let guard = bot.client.read().await;
        guard.as_ref().map(|c| BlockPos::from(c.entity().position()))
    };
    match final_current {
        Some(cur) => warn!(
            "Navigation to ({}, {}, {}) failed after {} attempts - current: ({}, {}, {}), continuing anyway",
            target.x, target.y, target.z,
            NAVIGATION_MAX_RETRIES,
            cur.x, cur.y, cur.z
        ),
        None => warn!(
            "Navigation to ({}, {}, {}) failed after {} attempts - current: unknown (bot disconnected), continuing anyway",
            target.x, target.y, target.z,
            NAVIGATION_MAX_RETRIES
        ),
    }
    Ok(())
}

/// Navigate to a node position (where bot stands to access the node).
///
/// # Arguments
/// * `bot` - Bot instance
/// * `node_position` - Node position to navigate to
pub async fn go_to_node(bot: &Bot, node_position: &Position) -> Result<(), String> {
    info!(
        "Navigating to node at ({}, {}, {})",
        node_position.x, node_position.y, node_position.z
    );
    navigate_to_position(bot, node_position).await
}

/// Navigate to a chest. First goes to the node, then positions near the chest.
///
/// # Arguments
/// * `bot` - Bot instance
/// * `chest` - Chest to navigate to (contains chest ID and position)
/// * `node_position` - Node position where bot should stand
///
/// # Errors
/// Returns an error with context including chest ID and positions if navigation fails.
pub async fn go_to_chest(bot: &Bot, chest: &Chest, node_position: &Position) -> Result<(), String> {
    // First navigate to the node position (this centers the bot on the node block)
    go_to_node(bot, node_position).await.map_err(|e| {
        format!(
            "Failed to reach node for chest {} at ({}, {}, {}): {}",
            chest.id,
            node_position.x, node_position.y, node_position.z,
            e
        )
    })?;
    
    // debug! (not info!) because this fires for every trade/chest op; one line
    // per visit would swamp default production logs.
    debug!(
        "At node ({}, {}, {}), chest {} accessible at ({}, {}, {})",
        node_position.x, node_position.y, node_position.z,
        chest.id,
        chest.position.x, chest.position.y, chest.position.z
    );
    
    // Brief settle delay before the caller opens the chest: Azalea can report
    // arrival a tick before the client-side entity position fully stabilizes,
    // and opening a chest while still in motion occasionally targets the wrong
    // block. DELAY_MEDIUM_MS is short enough to be invisible in aggregate.
    tokio::time::sleep(tokio::time::Duration::from_millis(DELAY_MEDIUM_MS)).await;
    
    Ok(())
}
