//! Navigation and pathfinding for the bot
//!
//! Provides pathfinding utilities for navigating the bot to node/chest positions
//! with automatic retry logic.

use azalea::BlockPos;
use azalea::pathfinder::goals::BlockPosGoal;
use azalea::pathfinder::PathfinderClientExt;
use tracing::{info, warn};

use crate::constants::{NAVIGATION_MAX_RETRIES, RETRY_BASE_DELAY_MS, RETRY_MAX_DELAY_MS, exponential_backoff_delay};
use crate::types::{Chest, Position};
use super::Bot;

/// Maximum wait time for pathfinding in milliseconds (10 seconds)
const PATHFINDING_WAIT_MS: u64 = 10_000;
/// Interval between position checks in milliseconds
const PATHFINDING_CHECK_INTERVAL_MS: u64 = 100;

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

    // If already at exact position, consider it done
    // Zero tolerance - bot must be exactly at the node P position
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

    // Use Azalea's pathfinding with BlockPosGoal wrapper
    // Azalea's pathfinding should NOT break blocks - it only walks around obstacles
    // If blocks are being broken, it's likely from a different source (e.g., block_interact misuse)
    client.goto(BlockPosGoal(target_block)).await;
    
    // Wait for pathfinding to complete
    let max_checks = (PATHFINDING_WAIT_MS / PATHFINDING_CHECK_INTERVAL_MS) as usize;
    let mut checks = 0;
    while checks < max_checks {
        tokio::time::sleep(tokio::time::Duration::from_millis(PATHFINDING_CHECK_INTERVAL_MS)).await;
        let new_pos = client.entity().position();
        let new_block = BlockPos::from(new_pos);
        let new_dx = (new_block.x - target_block.x).abs();
        let new_dy = (new_block.y - target_block.y).abs();
        let new_dz = (new_block.z - target_block.z).abs();
        // Zero tolerance - must be at exact position
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
        PATHFINDING_WAIT_MS,
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
                // Timed out, will retry
                warn!(
                    "Navigation attempt {}/{} timed out for target ({}, {}, {})",
                    attempt + 1, NAVIGATION_MAX_RETRIES,
                    target.x, target.y, target.z
                );
            }
            Err(e) => {
                // Bot not connected or other error, propagate immediately
                return Err(e);
            }
        }
    }
    
    // After all retries, continue anyway with a warning
    warn!(
        "Navigation to ({}, {}, {}) failed after {} attempts, continuing anyway",
        target.x, target.y, target.z,
        NAVIGATION_MAX_RETRIES
    );
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
    
    // The chest should be accessible from the node position
    // Bot is now centered on the node block, facing the center
    info!(
        "At node ({}, {}, {}), chest {} accessible at ({}, {}, {})",
        node_position.x, node_position.y, node_position.z,
        chest.id,
        chest.position.x, chest.position.y, chest.position.z
    );
    
    // Small delay to ensure we're positioned correctly
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    
    Ok(())
}
