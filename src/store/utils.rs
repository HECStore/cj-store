//! Utility functions for the Store

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use tokio::sync::oneshot;
use tracing::debug;

use crate::constants::UUID_CACHE_TTL_SECS;
use crate::messages::BotInstruction;
use crate::types::User;
use super::Store;

/// Map of lowercased username -> (uuid, lookup timestamp).
type UuidCache = HashMap<String, (String, Instant)>;

/// Global UUID cache for Mojang API lookups. TTL-expiry only — stale entries
/// are rejected on read and pruned periodically by `cleanup_uuid_cache`.
static UUID_CACHE: OnceLock<Mutex<UuidCache>> = OnceLock::new();

fn uuid_cache() -> &'static Mutex<UuidCache> {
    UUID_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Resolve username to UUID via Mojang API (async), with in-memory caching.
///
/// Lookups are cached for `UUID_CACHE_TTL_SECS` (default 5 minutes). Repeated
/// commands from the same player reuse the cached UUID instead of hitting the
/// Mojang API on every interaction. Cache keys are lowercased so `Steve` and
/// `steve` share an entry.
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
        let ttl = Duration::from_secs(UUID_CACHE_TTL_SECS);

        {
            let cache = uuid_cache().lock();
            if let Some((uuid, ts)) = cache.get(&key) {
                if ts.elapsed() < ttl {
                    debug!(username = username, uuid = %uuid, "UUID cache hit");
                    return Ok(uuid.clone());
                }
                debug!(
                    username = username,
                    age_secs = ts.elapsed().as_secs(),
                    "UUID cache stale, refetching"
                );
            } else {
                debug!(username = username, "UUID cache miss");
            }
        }

        let uuid = User::get_uuid_async(username)
            .await
            .map_err(crate::error::StoreError::ValidationError)?;
        debug!(username = username, uuid = %uuid, "UUID fetched from Mojang");

        {
            let mut cache = uuid_cache().lock();
            cache.insert(key, (uuid.clone(), Instant::now()));
        }

        Ok(uuid)
    }
}

/// Clear the entire UUID cache. Test-only — used to isolate cache tests.
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
        debug!(
            removed = removed,
            remaining = cache.len(),
            "Evicted stale UUID cache entries"
        );
    } else {
        debug!(remaining = cache.len(), "UUID cache cleanup: no stale entries");
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
        debug!(uuid = uuid, username = username, "Created new user record");
    } else if let Some(user) = store.users.get_mut(uuid)
        && user.username != username {
            let old = std::mem::replace(&mut user.username, username.to_string());
            store.dirty = true;
            debug!(
                uuid = uuid,
                old_username = %old,
                new_username = username,
                "Updated user's changed username"
            );
        }
}

/// Returns true iff the user with `user_uuid` exists and has the operator flag set.
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
    debug!(player = player_name, message = message, "Whispering to player");
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
        // Intentionally omit t.position — coordinates must not leak to players.
        parts.push(format!("{}x {}", t.amount, t.item));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_issues_empty_returns_bare_prefix() {
        assert_eq!(fmt_issues("Errors", &[], 5), "Errors");
    }

    #[test]
    fn fmt_issues_single_item_separates_with_colon() {
        assert_eq!(
            fmt_issues("Errors", &["Issue 1".to_string()], 5),
            "Errors: Issue 1"
        );
    }

    #[test]
    fn fmt_issues_joins_multiple_items_with_semicolons() {
        assert_eq!(
            fmt_issues("Errors", &["A".to_string(), "B".to_string()], 5),
            "Errors: A; B"
        );
    }

    #[test]
    fn fmt_issues_at_max_does_not_append_more_suffix() {
        // Exactly `max` issues: no truncation suffix
        let issues: Vec<String> = (1..=3).map(|i| format!("I{}", i)).collect();
        assert_eq!(fmt_issues("E", &issues, 3), "E: I1; I2; I3");
    }

    #[test]
    fn fmt_issues_truncates_with_more_suffix_above_max() {
        let issues: Vec<String> = (1..=10).map(|i| format!("Issue {}", i)).collect();
        let result = fmt_issues("Errors", &issues, 3);
        assert_eq!(result, "Errors: Issue 1; Issue 2; Issue 3 (+7 more)");
    }

    #[test]
    fn summarize_transfers_empty_returns_none_literal() {
        assert_eq!(summarize_transfers(&[], 5), "none");
    }

    #[test]
    fn summarize_transfers_formats_single_entry_without_coords() {
        use crate::types::storage::ChestTransfer;
        use crate::types::Position;

        let transfers = vec![ChestTransfer {
            chest_id: 0,
            item: crate::types::ItemId::new("diamond").unwrap(),
            amount: 64,
            position: Position { x: 123, y: 64, z: -456 },
        }];
        let s = summarize_transfers(&transfers, 5);
        assert_eq!(s, "64x diamond");
        // Security invariant: coordinates must never appear in the summary
        assert!(!s.contains("123"));
        assert!(!s.contains("-456"));
    }

    #[test]
    fn summarize_transfers_joins_multiple_with_semicolons() {
        use crate::types::storage::ChestTransfer;
        use crate::types::Position;

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
        assert_eq!(
            summarize_transfers(&transfers, 5),
            "64x diamond; 128x iron_ingot"
        );
    }

    #[test]
    fn summarize_transfers_truncates_above_max_with_more_suffix() {
        use crate::types::storage::ChestTransfer;
        use crate::types::Position;

        let transfers: Vec<ChestTransfer> = (0..5)
            .map(|i| ChestTransfer {
                chest_id: i,
                item: crate::types::ItemId::new("stone").unwrap(),
                amount: 1,
                position: Position::default(),
            })
            .collect();
        let s = summarize_transfers(&transfers, 2);
        assert_eq!(s, "1x stone; 1x stone; (+3 more)");
    }

    #[test]
    fn uuid_cache_insert_then_read_returns_same_entry() {
        clear_uuid_cache();
        let cache = uuid_cache();
        let key = "testplayer".to_string();
        let uuid = "00000000-0000-0000-0000-000000000001".to_string();

        cache.lock().insert(key.clone(), (uuid.clone(), Instant::now()));

        let cached = cache.lock().get(&key).cloned();
        assert_eq!(cached.map(|(u, _)| u), Some(uuid));
    }

    #[test]
    fn uuid_cache_lookup_uses_lowercased_key() {
        // resolve_user_uuid lowercases before inserting, so lookup callers
        // must also lowercase — verify the contract holds for "steve".
        clear_uuid_cache();
        let cache = uuid_cache();
        let uuid = "00000000-0000-0000-0000-000000000002".to_string();

        cache.lock().insert("steve".to_string(), (uuid.clone(), Instant::now()));

        let hit = cache.lock().get("steve").cloned();
        assert_eq!(hit.map(|(u, _)| u), Some(uuid));
    }

    #[test]
    fn uuid_cache_entry_older_than_ttl_is_treated_as_stale() {
        clear_uuid_cache();
        let cache = uuid_cache();
        let key = "expiredplayer".to_string();
        let uuid = "00000000-0000-0000-0000-000000000003".to_string();

        let old_instant = Instant::now() - Duration::from_secs(UUID_CACHE_TTL_SECS + 1);
        cache.lock().insert(key.clone(), (uuid, old_instant));

        let entry = cache.lock().get(&key).cloned().unwrap();
        let ttl = Duration::from_secs(UUID_CACHE_TTL_SECS);
        assert!(entry.1.elapsed() >= ttl, "Entry should be past TTL");
    }

    #[test]
    fn cleanup_uuid_cache_drops_stale_entries_and_keeps_fresh_ones() {
        clear_uuid_cache();
        let cache = uuid_cache();

        cache.lock().insert(
            "fresh".to_string(),
            ("uuid-fresh".to_string(), Instant::now()),
        );
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
    fn cleanup_uuid_cache_is_noop_when_all_entries_are_fresh() {
        clear_uuid_cache();
        let cache = uuid_cache();
        cache.lock().insert("a".to_string(), ("uuid-a".to_string(), Instant::now()));
        cache.lock().insert("b".to_string(), ("uuid-b".to_string(), Instant::now()));

        cleanup_uuid_cache();

        assert_eq!(cache.lock().len(), 2);
    }

    #[test]
    fn clear_uuid_cache_empties_the_cache() {
        let cache = uuid_cache();
        cache.lock().insert("a".to_string(), ("uuid-a".to_string(), Instant::now()));
        cache.lock().insert("b".to_string(), ("uuid-b".to_string(), Instant::now()));

        clear_uuid_cache();
        assert!(cache.lock().is_empty());
    }

    // ------------------------------------------------------------------------
    // ensure_user_exists / is_operator / get_node_position each need a Store.
    // Use `Store::new_for_test` to bypass disk I/O.
    // ------------------------------------------------------------------------
    fn test_store() -> Store {
        use tokio::sync::mpsc;
        let (tx, _rx) = mpsc::channel::<BotInstruction>(1);
        let config = crate::config::Config {
            position: crate::types::Position { x: 0, y: 64, z: 0 },
            fee: 0.125,
            account_email: String::new(),
            server_address: "test".to_string(),
            buffer_chest_position: None,
            trade_timeout_ms: 5_000,
            pathfinding_timeout_ms: 5_000,
            max_orders: 1000,
            max_trades_in_memory: 1000,
            autosave_interval_secs: 10,
        };
        Store::new_for_test(tx, config, HashMap::new(), HashMap::new(), crate::types::Storage::default())
    }

    #[test]
    fn ensure_user_exists_creates_new_user_and_marks_dirty() {
        let mut store = test_store();
        ensure_user_exists(&mut store, "Alice", "uuid-a");
        let u = store.users.get("uuid-a").expect("user inserted");
        assert_eq!(u.username, "Alice");
        assert_eq!(u.balance, 0.0);
        assert!(!u.operator);
        assert!(store.dirty);
    }

    #[test]
    fn ensure_user_exists_updates_username_on_drift_and_marks_dirty() {
        let mut store = test_store();
        ensure_user_exists(&mut store, "Alice", "uuid-a");
        store.dirty = false;

        ensure_user_exists(&mut store, "AliceRenamed", "uuid-a");
        assert_eq!(store.users.get("uuid-a").unwrap().username, "AliceRenamed");
        assert!(store.dirty);
    }

    #[test]
    fn ensure_user_exists_is_noop_when_username_matches() {
        let mut store = test_store();
        ensure_user_exists(&mut store, "Alice", "uuid-a");
        store.dirty = false;

        ensure_user_exists(&mut store, "Alice", "uuid-a");
        assert!(!store.dirty, "no change should not mark dirty");
    }

    #[test]
    fn is_operator_returns_false_for_unknown_uuid() {
        let store = test_store();
        assert!(!is_operator(&store, "missing"));
    }

    #[test]
    fn is_operator_returns_false_for_regular_user() {
        let mut store = test_store();
        ensure_user_exists(&mut store, "Alice", "uuid-a");
        assert!(!is_operator(&store, "uuid-a"));
    }

    #[test]
    fn is_operator_returns_true_when_operator_flag_set() {
        let mut store = test_store();
        ensure_user_exists(&mut store, "Alice", "uuid-a");
        store.users.get_mut("uuid-a").unwrap().operator = true;
        assert!(is_operator(&store, "uuid-a"));
    }

    #[test]
    fn get_node_position_falls_back_to_calc_when_node_absent() {
        let store = test_store();
        // Storage has no nodes; fallback must equal Node::calc_position for
        // the derived node_id (chest_id / CHESTS_PER_NODE).
        let chest_id = 5; // node_id = 5 / 4 = 1
        let pos = get_node_position(&store, chest_id);
        let expected = crate::types::Node::calc_position(
            chest_id / crate::constants::CHESTS_PER_NODE as i32,
            &store.storage.position,
        );
        assert_eq!(pos, expected);
    }

    #[test]
    fn get_node_position_uses_materialized_node_when_present() {
        use crate::types::Position;
        let mut store = test_store();
        let explicit = Position { x: 999, y: 64, z: -999 };
        store.storage.nodes.push(crate::types::Node {
            id: 1,
            position: explicit,
            chests: Vec::new(),
        });
        // chest_id 5 -> node_id 1 -> should pick up the materialized node,
        // not the calc_position fallback.
        assert_eq!(get_node_position(&store, 5), explicit);
    }
}
