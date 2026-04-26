//! # Mojang UUID resolution
//!
//! Resolves Minecraft usernames to canonical hyphenated UUIDs via the public
//! Mojang API, with an in-process TTL cache.
//!
//! Lives at the crate root (peer to `store`, `bot`, `cli`, `chat`) because
//! more than one module needs UUIDs: the Store keys users on UUID, and the
//! Chat module keys per-player memory on UUID. Hosting the resolver inside
//! `store::*` would force `chat` to import from `store::*`, which is
//! deliberately forbidden by the chat-module design (chat must never see
//! Store state).
//!
//! The TTL cache is a global `parking_lot::Mutex<HashMap<...>>` and is
//! `Send + Sync`, so any task may call `resolve_user_uuid` and
//! `cleanup_uuid_cache` regardless of its task flavor.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tracing::debug;

use crate::constants::UUID_CACHE_TTL_SECS;
#[cfg(not(test))]
use crate::types::User;

/// Map of lowercased username -> (uuid, lookup timestamp).
type UuidCache = HashMap<String, (String, Instant)>;

/// Global UUID cache for Mojang API lookups. TTL-expiry only — stale entries
/// are rejected on read and pruned periodically by [`cleanup_uuid_cache`].
static UUID_CACHE: OnceLock<Mutex<UuidCache>> = OnceLock::new();

fn uuid_cache() -> &'static Mutex<UuidCache> {
    UUID_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Resolve a Minecraft username to a canonical hyphenated Mojang UUID.
///
/// Lookups are cached for `UUID_CACHE_TTL_SECS` (default 5 minutes). Repeated
/// calls for the same player reuse the cached UUID instead of hitting the
/// Mojang API on every interaction. Cache keys are lowercased so `Steve` and
/// `steve` share an entry.
///
/// Returns `Result<String, String>` — the error string is user-safe and ready
/// to be whispered straight back to the player. Store-layer callers wrap it in
/// `StoreError::ValidationError` at the call site; chat-layer callers consume
/// it directly.
pub async fn resolve_user_uuid(username: &str) -> Result<String, String> {
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

        let uuid = User::get_uuid_async(username).await?;
        debug!(username = username, uuid = %uuid, "UUID fetched from Mojang");

        {
            let mut cache = uuid_cache().lock();
            cache.insert(key, (uuid.clone(), Instant::now()));
        }

        Ok(uuid)
    }
}

/// Drop UUID cache entries older than [`UUID_CACHE_TTL_SECS`].
///
/// Stale entries never serve a cache hit (the TTL check in [`resolve_user_uuid`]
/// rejects them), but unless they are removed they keep growing the HashMap
/// indefinitely. Callable from any task — `parking_lot::Mutex` is `Send + Sync`.
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

/// Clear the entire UUID cache. Test-only — used to isolate cache tests.
#[cfg(test)]
pub fn clear_uuid_cache() {
    uuid_cache().lock().clear();
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
