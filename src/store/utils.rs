//! Utility functions for the Store

use tokio::sync::oneshot;
use tracing::debug;

use crate::messages::BotInstruction;
use crate::types::User;
use super::Store;

/// Normalize item ID: strip "minecraft:" prefix if present.
/// This ensures consistent item naming across the codebase (without prefix).
/// "minecraft:diamond" -> "diamond", "diamond" -> "diamond"
/// Returns empty string for empty input (invalid, caller should validate).
pub fn normalize_item_id(item: &str) -> String {
    if item.is_empty() {
        return String::new();
    }
    // Strip "minecraft:" prefix if present
    item.strip_prefix("minecraft:").unwrap_or(item).to_string()
}

/// Add "minecraft:" prefix to an item ID for use with Minecraft server.
/// Use this when sending item IDs to the game (e.g., for trade validation).
#[allow(dead_code)]
pub fn with_minecraft_prefix(item: &str) -> String {
    if item.is_empty() {
        return String::new();
    }
    if item.contains(':') {
        item.to_string()
    } else {
        format!("minecraft:{}", item)
    }
}

/// Resolve username to UUID via Mojang API (async)
///
/// Uses the async Mojang API client for better performance without blocking the runtime.
/// `_store` is currently unused but retained in the signature to allow future
/// caching of lookups without requiring call-site changes.
pub async fn resolve_user_uuid(_store: &Store, username: &str) -> Result<String, String> {
    User::get_uuid_async(username).await
}

/// Ensure user exists in store, creating if missing.
///
/// UUIDs are the canonical identity key (usernames can change), so we look up
/// by UUID and only update the stored username when it has drifted. Marks the
/// store dirty on any mutation so the change is persisted on the next flush.
pub fn ensure_user_exists(store: &mut Store, username: &str, uuid: &str) {
    if !store.users.contains_key(uuid) {
        store.users.insert(
            uuid.to_string(),
            User {
                uuid: uuid.to_string(),
                username: username.to_string(),
                balance: 0.0,
                operator: false,
            },
        );
        store.dirty = true;
    } else if let Some(user) = store.users.get_mut(uuid) {
        if user.username != username {
            user.username = username.to_string();
            store.dirty = true;
        }
    }
}

/// Check if a user is an operator
pub fn is_operator(store: &Store, user_uuid: &str) -> bool {
    store.users.get(user_uuid).is_some_and(|u| u.operator)
}

/// Get node position for a given chest_id.
///
/// Each node holds 4 chests, so the node id is `chest_id / 4`. If the node
/// isn't materialized in `storage.nodes` yet, we deterministically recompute
/// its position from the storage origin so callers always get a valid location.
pub fn get_node_position(store: &Store, chest_id: i32) -> crate::types::Position {
    let node_id = chest_id / 4;
    store.storage.nodes.iter()
        .find(|n| n.id == node_id)
        .map(|n| n.position)
        .unwrap_or_else(|| {
            // Fallback: calculate from storage position
            crate::types::Node::calc_position(node_id, &store.storage.position)
        })
}

/// Send a message to a player via bot whisper.
///
/// Uses a oneshot channel so we can await the bot's acknowledgement and
/// surface send failures (bot disconnected, channel closed) back to the caller
/// instead of silently dropping the message.
pub async fn send_message_to_player(store: &Store, player_name: &str, message: &str) -> Result<(), String> {
    debug!("Sending message to {}: {}", player_name, message);
    let (tx, rx) = oneshot::channel();
    store.bot_tx
        .send(BotInstruction::Whisper {
            target: player_name.to_string(),
            message: message.to_string(),
            respond_to: tx,
        })
        .await
        .map_err(|e| format!("Failed to send bot instruction: {}", e))?;

    rx.await.map_err(|e| format!("Bot response dropped: {}", e))?
}

/// Helper to format transfer summaries (excludes coordinates for security).
///
/// Player-facing output must NEVER leak chest coordinates: exposing them would
/// let customers (or griefers) locate and bypass the storage system directly.
/// Only item + amount pairs are included; long lists are truncated with a
/// "(+N more)" suffix to keep whispers within Minecraft's chat limits.
pub fn summarize_transfers(transfers: &[crate::types::storage::ChestTransfer], max: usize) -> String {
    if transfers.is_empty() {
        return "none".to_string();
    }

    let mut parts: Vec<String> = Vec::new();
    for (i, t) in transfers.iter().take(max).enumerate() {
        let _ = i;
        // DO NOT include coordinates in player-facing messages (security)
        parts.push(format!(
            "{}x {}",
            t.amount, t.item
        ));
    }

    if transfers.len() > max {
        parts.push(format!("(+{} more)", transfers.len() - max));
    }

    parts.join("; ")
}

/// Helper to format issue lists.
///
/// Produces a compact "prefix: a; b; c (+N more)" string for operator reports
/// and whispers. The `max` cap prevents one pathological operation from
/// flooding chat when many issues accumulate.
pub fn fmt_issues(prefix: &str, issues: &[String], max: usize) -> String {
    if issues.is_empty() {
        return prefix.to_string();
    }
    let mut out = String::new();
    out.push_str(prefix);
    out.push_str(": ");
    for (i, s) in issues.iter().take(max).enumerate() {
        if i > 0 {
            out.push_str("; ");
        }
        out.push_str(s);
    }
    if issues.len() > max {
        out.push_str(&format!(" (+{} more)", issues.len() - max));
    }
    out
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_normalize_item_id() {
        // Should keep items without prefix unchanged
        assert_eq!(normalize_item_id("diamond"), "diamond");
        assert_eq!(normalize_item_id("cobblestone"), "cobblestone");
        assert_eq!(normalize_item_id("iron_ingot"), "iron_ingot");
        
        // Should strip minecraft: prefix
        assert_eq!(normalize_item_id("minecraft:diamond"), "diamond");
        assert_eq!(normalize_item_id("minecraft:cobblestone"), "cobblestone");
        
        // Should preserve custom namespaces (only strips "minecraft:" prefix)
        assert_eq!(normalize_item_id("modid:custom_item"), "modid:custom_item");
        
        // Empty string returns empty string (invalid, caller should validate)
        assert_eq!(normalize_item_id(""), "");
    }
    
    #[test]
    fn test_with_minecraft_prefix() {
        // Should add minecraft: prefix
        assert_eq!(with_minecraft_prefix("diamond"), "minecraft:diamond");
        assert_eq!(with_minecraft_prefix("cobblestone"), "minecraft:cobblestone");
        
        // Should preserve existing prefix
        assert_eq!(with_minecraft_prefix("minecraft:diamond"), "minecraft:diamond");
        
        // Should preserve custom namespaces
        assert_eq!(with_minecraft_prefix("modid:custom_item"), "modid:custom_item");
        
        // Empty string returns empty string
        assert_eq!(with_minecraft_prefix(""), "");
    }
    
    #[test]
    fn test_fmt_issues() {
        // Empty issues
        assert_eq!(fmt_issues("Errors", &[], 5), "Errors");
        
        // Single issue
        assert_eq!(
            fmt_issues("Errors", &["Issue 1".to_string()], 5),
            "Errors: Issue 1"
        );
        
        // Multiple issues within limit
        assert_eq!(
            fmt_issues("Errors", &["A".to_string(), "B".to_string()], 5),
            "Errors: A; B"
        );
        
        // Issues exceeding limit
        let issues: Vec<String> = (1..=10).map(|i| format!("Issue {}", i)).collect();
        let result = fmt_issues("Errors", &issues, 3);
        assert!(result.contains("Issue 1"));
        assert!(result.contains("Issue 2"));
        assert!(result.contains("Issue 3"));
        assert!(result.contains("(+7 more)"));
    }
    
    #[test]
    fn test_summarize_transfers() {
        use crate::types::storage::ChestTransfer;
        use crate::types::Position;
        
        // Empty transfers
        assert_eq!(summarize_transfers(&[], 5), "none");
        
        // Single transfer
        let transfers = vec![ChestTransfer {
            chest_id: 0,
            item: "diamond".to_string(),
            amount: 64,
            position: Position::default(),
        }];
        assert_eq!(summarize_transfers(&transfers, 5), "64x diamond");
        
        // Multiple transfers
        let transfers = vec![
            ChestTransfer {
                chest_id: 0,
                item: "diamond".to_string(),
                amount: 64,
                position: Position::default(),
            },
            ChestTransfer {
                chest_id: 1,
                item: "iron_ingot".to_string(),
                amount: 128,
                position: Position::default(),
            },
        ];
        assert_eq!(summarize_transfers(&transfers, 5), "64x diamond; 128x iron_ingot");
    }
}
