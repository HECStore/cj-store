//! Utility functions for the Store

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use tokio::sync::oneshot;
use tracing::debug;
#[cfg(not(test))]
use tracing::info;

use crate::constants::UUID_CACHE_TTL_SECS;
use crate::messages::BotInstruction;
use crate::types::User;
use super::Store;

/// Cached UUID entry: (uuid, timestamp of lookup).
type UuidCache = HashMap<String, (String, Instant)>;

/// Global UUID cache for Mojang API lookups.
/// Uses a simple HashMap with TTL-based expiry — entries older than
/// `UUID_CACHE_TTL_SECS` are treated as stale and re-fetched.
static UUID_CACHE: OnceLock<Mutex<UuidCache>> = OnceLock::new();

fn uuid_cache() -> &'static Mutex<UuidCache> {
    UUID_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Resolve username to UUID via Mojang API (async), with in-memory caching.
///
/// Lookups are cached for `UUID_CACHE_TTL_SECS` (default 5 minutes). Repeated
/// commands from the same player reuse the cached UUID instead of hitting the
/// Mojang API on every interaction.
///
/// Returns a typed `StoreError::ValidationError` on Mojang lookup failure —
/// the text is user-safe and the handlers whisper it straight back to the
/// player.
pub async fn resolve_user_uuid(username: &str) -> Result<String, crate::error::StoreError> {
    #[cfg(test)]
    {
        // Offline deterministic UUID for integration tests: avoids hitting the
        // Mojang API (which requires network and introduces flakiness). Format:
        // zero-padded username embedded in the last UUID segment.
        let trimmed: String = username.chars().take(12).collect();
        let padded = format!("{:0>12}", trimmed);
        Ok(format!("00000000-0000-0000-0000-{}", padded))
    }
    #[cfg(not(test))]
    {
        let key = username.to_lowercase();
        let ttl = std::time::Duration::from_secs(UUID_CACHE_TTL_SECS);

        // Check cache first
        {
            let cache = uuid_cache().lock();
            if let Some((uuid, ts)) = cache.get(&key)
                && ts.elapsed() < ttl {
                    debug!("UUID cache hit for '{}' -> {}", username, uuid);
                    return Ok(uuid.clone());
                }
        }

        // Cache miss or stale — fetch from Mojang API.
        // Map the legacy `String` error into a typed variant explicitly so
        // the conversion is visible at the boundary (no blanket
        // `From<String>` impl any more).
        let uuid = User::get_uuid_async(username)
            .await
            .map_err(crate::error::StoreError::ValidationError)?;
        info!("UUID cache miss for '{}', fetched {}", username, uuid);

        {
            let mut cache = uuid_cache().lock();
            cache.insert(key, (uuid.clone(), Instant::now()));
        }

        Ok(uuid)
    }
}

/// Clear the entire UUID cache. Useful for testing or after long idle periods.
#[cfg(test)]
pub fn clear_uuid_cache() {
    uuid_cache().lock().clear();
}

/// Drop UUID cache entries older than `UUID_CACHE_TTL_SECS`.
///
/// Stale entries never serve a cache hit (the TTL check in `resolve_user_uuid`
/// rejects them), but unless they are removed they keep growing the HashMap
/// indefinitely. The periodic cleanup task calls this to bound memory.
pub fn cleanup_uuid_cache() {
    let mut cache = uuid_cache().lock();
    let now = Instant::now();
    let ttl = Duration::from_secs(UUID_CACHE_TTL_SECS);
    let before = cache.len();
    cache.retain(|_, (_, inserted)| now.duration_since(*inserted) < ttl);
    let removed = before - cache.len();
    if removed > 0 {
        debug!("Cleaned up {} stale UUID cache entries", removed);
    }
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
    } else if let Some(user) = store.users.get_mut(uuid)
        && user.username != username {
            user.username = username.to_string();
            store.dirty = true;
        }
}

/// Check if a user is an operator
pub fn is_operator(store: &Store, user_uuid: &str) -> bool {
    store.users.get(user_uuid).is_some_and(|u| u.operator)
}

/// Get node position for a given chest_id.
///
/// Each node holds `CHESTS_PER_NODE` chests, so the node id is
/// `chest_id / CHESTS_PER_NODE`. If the node isn't materialized in
/// `storage.nodes` yet, we deterministically recompute its position from the
/// storage origin so callers always get a valid location.
pub fn get_node_position(store: &Store, chest_id: i32) -> crate::types::Position {
    let node_id = chest_id / crate::constants::CHESTS_PER_NODE as i32;
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
///
/// Returns a typed `StoreError` so callers can match on the failure kind
/// (e.g. retry on `BotDisconnected`, escalate on other variants).
pub async fn send_message_to_player(
    store: &Store,
    player_name: &str,
    message: &str,
) -> Result<(), crate::error::StoreError> {
    debug!("Sending message to {}: {}", player_name, message);
    let (tx, rx) = oneshot::channel();
    store
        .bot_tx
        .send(BotInstruction::Whisper {
            target: player_name.to_string(),
            message: message.to_string(),
            respond_to: tx,
        })
        .await
        .map_err(|_| crate::error::StoreError::BotDisconnected)?;

    // Inner result: the bot's own response - if it's Err(String), surface as BotError.
    rx.await
        .map_err(|_| crate::error::StoreError::BotDisconnected)?
        .map_err(crate::error::StoreError::BotError)
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
    for t in transfers.iter().take(max) {
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
            item: crate::types::ItemId::new("diamond").unwrap(),
            amount: 64,
            position: Position::default(),
        }];
        assert_eq!(summarize_transfers(&transfers, 5), "64x diamond");
        
        // Multiple transfers
        let transfers = vec![
            ChestTransfer {
                chest_id: 0,
                item: crate::types::ItemId::new("diamond").unwrap(),
                amount: 64,
                position: Position::default(),
            },
            ChestTransfer {
                chest_id: 1,
                item: crate::types::ItemId::new("iron_ingot").unwrap(),
                amount: 128,
                position: Position::default(),
            },
        ];
        assert_eq!(summarize_transfers(&transfers, 5), "64x diamond; 128x iron_ingot");
    }

    // ========================================================================
    // UUID cache tests
    // ========================================================================

    #[test]
    fn test_uuid_cache_insert_and_lookup() {
        clear_uuid_cache();
        let cache = uuid_cache();
        let key = "testplayer".to_string();
        let uuid = "00000000-0000-0000-0000-000000000001".to_string();

        cache.lock().insert(key.clone(), (uuid.clone(), Instant::now()));

        let cached = cache.lock().get(&key).cloned();
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().0, uuid);
    }

    #[test]
    fn test_uuid_cache_case_insensitive_key() {
        // The cache uses lowercased keys so "Steve" and "steve" hit the same entry
        clear_uuid_cache();
        let cache = uuid_cache();
        let uuid = "00000000-0000-0000-0000-000000000002".to_string();

        cache.lock().insert("steve".to_string(), (uuid.clone(), Instant::now()));

        // Lookup must use the same lowercased key (resolve_user_uuid does this)
        let hit = cache.lock().get("steve").cloned();
        assert_eq!(hit.unwrap().0, uuid);
    }

    #[test]
    fn test_uuid_cache_ttl_expiry() {
        clear_uuid_cache();
        let cache = uuid_cache();
        let key = "expiredplayer".to_string();
        let uuid = "00000000-0000-0000-0000-000000000003".to_string();

        // Insert with a timestamp far in the past
        let old_instant = Instant::now() - std::time::Duration::from_secs(UUID_CACHE_TTL_SECS + 1);
        cache.lock().insert(key.clone(), (uuid.clone(), old_instant));

        // Entry exists but is stale
        let entry = cache.lock().get(&key).cloned().unwrap();
        let ttl = std::time::Duration::from_secs(UUID_CACHE_TTL_SECS);
        assert!(entry.1.elapsed() >= ttl, "Entry should be expired");
    }

    #[test]
    fn test_cleanup_uuid_cache_drops_stale_entries() {
        clear_uuid_cache();
        let cache = uuid_cache();

        // Fresh entry
        cache.lock().insert(
            "fresh".to_string(),
            ("uuid-fresh".to_string(), Instant::now()),
        );
        // Stale entry (older than TTL)
        let stale_ts = Instant::now() - Duration::from_secs(UUID_CACHE_TTL_SECS + 1);
        cache.lock().insert(
            "stale".to_string(),
            ("uuid-stale".to_string(), stale_ts),
        );

        cleanup_uuid_cache();

        let guard = cache.lock();
        assert!(guard.contains_key("fresh"), "fresh entry should be retained");
        assert!(!guard.contains_key("stale"), "stale entry should be dropped");
    }

    #[test]
    fn test_uuid_cache_clear() {
        let cache = uuid_cache();
        cache.lock().insert(
            "a".to_string(),
            ("uuid-a".to_string(), Instant::now()),
        );
        cache.lock().insert(
            "b".to_string(),
            ("uuid-b".to_string(), Instant::now()),
        );

        clear_uuid_cache();
        assert!(cache.lock().is_empty());
    }
}
