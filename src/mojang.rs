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
#[cfg(not(test))]
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
#[cfg(not(test))]
use tokio::sync::Notify;
use tracing::debug;

use crate::constants::UUID_CACHE_TTL_SECS;
use crate::types::user::MojangResolveError;
#[cfg(not(test))]
use crate::types::User;

/// Map of lowercased username -> (uuid, lookup timestamp).
type UuidCache = HashMap<String, (String, Instant)>;

/// Hard cap on `UUID_CACHE` entries. TTL-only eviction lets a long-running
/// bot (weeks of uptime, hundreds of distinct interactors per day) grow the
/// cache unbounded between `cleanup_uuid_cache` cycles. The cap forces a
/// best-effort cleanup pass + oldest-entry eviction at insert time so the
/// resident set stays bounded even if cleanup never runs.
///
/// 4096 entries × ~80 bytes each (key string + UUID + Instant) ≈ 320 KiB —
/// orders of magnitude larger than any realistic active-player set on a
/// single-server bot, but small enough that even adversarial username
/// flooding can't OOM the process.
#[cfg(not(test))]
const MAX_UUID_CACHE_ENTRIES: usize = 4096;

/// Global UUID cache for Mojang API lookups. TTL-expiry on read AND a
/// hard size cap on insert — stale entries are rejected on read and pruned
/// periodically by [`cleanup_uuid_cache`]; oversized maps trigger an
/// opportunistic cleanup + oldest-entry eviction inline.
static UUID_CACHE: OnceLock<Mutex<UuidCache>> = OnceLock::new();

fn uuid_cache() -> &'static Mutex<UuidCache> {
    UUID_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Per-key in-flight coordinator for `resolve_user_uuid`.
///
/// Without this, N concurrent resolutions of the same uncached username each
/// fire an independent HTTPS request to api.mojang.com (the cache-miss read
/// at the top of the function and the network call further down are not
/// atomic — every racing task observes the miss and proceeds). With it, the
/// first task to miss inserts a `Notify` for that lowercased key, every
/// subsequent task that misses with an existing `Notify` parks on
/// `notified()`, and the leader broadcasts via `notify_waiters()` once it
/// has populated the cache. Followers then re-check the cache and return
/// the freshly-inserted entry.
///
/// Keyed off the lowercased ASCII username — same key as `UUID_CACHE` so a
/// `Steve` / `steve` collision coalesces too.
///
/// `LazyLock` (not `OnceLock` + getter) because there are no cyclic init
/// dependencies and the closure form is read-only after first access.
#[cfg(not(test))]
static IN_FLIGHT: LazyLock<Mutex<HashMap<String, Arc<Notify>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Resolve a Minecraft username to a canonical hyphenated Mojang UUID.
///
/// Lookups are cached for `UUID_CACHE_TTL_SECS` (default 5 minutes). Repeated
/// calls for the same player reuse the cached UUID instead of hitting the
/// Mojang API on every interaction. Cache keys are lowercased so `Steve` and
/// `steve` share an entry. Concurrent calls for the same uncached username
/// are coalesced via a per-key `Notify` so only one HTTPS round-trip ever
/// fires per (lowercased) name even under thundering-herd load.
///
/// Returns a typed [`MojangResolveError`] so call sites can route each
/// failure mode to a sanitized `StoreError` (`UserNotFound` /
/// `ValidationError` / `MojangNetwork`) without ever stringifying a
/// `reqwest::Error` into a player-facing whisper. Chat-layer callers that
/// still want a `String` should rely on the `Display` impl, which is short
/// and entirely author-controlled.
///
/// **Test-build behavior**: under `#[cfg(test)]` the `UUID_CACHE` is
/// bypassed entirely and every call recomputes the deterministic fixture
/// UUID from the username. Tests that need to observe cache behavior
/// (insert/read, TTL expiry, cleanup) must operate on the cache directly
/// via `uuid_cache()` / `clear_uuid_cache()`; calling this function from a
/// test will neither populate nor consult the cache.
pub async fn resolve_user_uuid(username: &str) -> Result<String, MojangResolveError> {
    #[cfg(test)]
    {
        // Offline deterministic UUID for integration tests: avoids hitting the
        // Mojang API (which requires network and introduces flakiness). Format:
        // zero-padded username embedded in the last UUID segment.
        //
        // Validate the username up front with the same Mojang shape rule the
        // production path now enforces. This keeps the test fixture honest:
        // a non-ASCII or out-of-range username matches the production branch's
        // error path instead of silently producing a multi-byte trailing
        // segment that would violate the canonical 36-char UUID contract.
        // Delegates to the single-source-of-truth predicate in `types::user`.
        if !crate::types::user::is_valid_username_shape(username) {
            return Err(MojangResolveError::NotFound {
                username: username.to_string(),
            });
        }
        // After the shape gate the username is guaranteed ASCII, so byte-level
        // trim-then-pad produces exactly 12 ASCII bytes in the trailing
        // segment. `format!("{:0>12}", _)` only LEFT-PADS — it never
        // truncates — so 13-16 char usernames must be trimmed first to keep
        // the canonical 36-char UUID contract documented above. Take the
        // first 12 chars to match the convention used by sibling test
        // helpers in store/orders.rs, store/handlers/info.rs,
        // store/handlers/operator.rs, store/handlers/player.rs.
        let trimmed: String = username.chars().take(12).collect();
        let padded = format!("{:0>12}", trimmed);
        let out = format!("00000000-0000-0000-0000-{}", padded);
        debug_assert_eq!(
            out.len(),
            36,
            "test fixture produced non-canonical UUID for {username:?}: {out:?}"
        );
        Ok(out)
    }
    #[cfg(not(test))]
    {
        // Defense-in-depth shape check before the cache lookup so a junk
        // username can't pollute the cache or burn a Mojang round-trip.
        // `User::get_uuid_async` runs the same check before URL construction;
        // doing it here too means even cache hits/misses see only valid keys.
        // Delegates to the single-source-of-truth predicate in `types::user`.
        if !crate::types::user::is_valid_username_shape(username) {
            return Err(MojangResolveError::InvalidShape);
        }
        // After the shape gate the username is ASCII, so `to_ascii_lowercase`
        // is correct and avoids Unicode case-folding (which is locale-aware
        // and could split a cache entry on e.g. Turkish dotless I).
        let key = username.to_ascii_lowercase();
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

        // Single-flight coalescing: if another task is already resolving
        // the same lowercased name, park on its `Notify` and re-check the
        // cache when it broadcasts. Otherwise install our own `Notify`,
        // do the fetch, then remove the entry and `notify_waiters()` so
        // every parked follower wakes and re-reads the cache. Critically,
        // the `IN_FLIGHT` lock is dropped BEFORE every `await` —
        // `parking_lot::Mutex` is sync; holding it across an `.await`
        // would deadlock the runtime if a follower were polled on the
        // same worker thread that needs the same lock, AND the
        // MutexGuard is not `Send` so the surrounding future would
        // become non-`Send` (breaking `tokio::spawn`).
        //
        // The two arms are split into a leader path and a follower path.
        // Each computes its `Notify` (or absence) inside a tight inner
        // scope so the `MutexGuard` cannot be carried into the `.await`
        // by NLL — even a "this binding might be used later" analysis is
        // foreclosed by the early `return`/binding-out-of-scope.
        enum SingleFlight {
            Leader,
            FollowerOf(Arc<Notify>),
        }
        let role = {
            let mut inflight = IN_FLIGHT.lock();
            if let Some(notify) = inflight.get(&key) {
                SingleFlight::FollowerOf(Arc::clone(notify))
            } else {
                inflight.insert(key.clone(), Arc::new(Notify::new()));
                SingleFlight::Leader
            }
        };

        let leader = match role {
            SingleFlight::FollowerOf(notify) => {
                debug!(username = username, "UUID coalesced behind in-flight resolve");
                notify.notified().await;
                // Leader has populated the cache (or failed). Re-read
                // the cache; on a leader-success we get a fresh hit,
                // on a leader-failure we fall through and try ourselves
                // — which is the right semantics: the leader's typed
                // error is not stored in the cache, and the follower
                // shouldn't synthesize a sibling's error.
                let hit = {
                    let cache = uuid_cache().lock();
                    cache.get(&key).and_then(|(uuid, ts)| {
                        if ts.elapsed() < ttl { Some(uuid.clone()) } else { None }
                    })
                };
                if let Some(uuid) = hit {
                    return Ok(uuid);
                }
                false
            }
            SingleFlight::Leader => true,
        };

        let result = User::get_uuid_async(username).await;

        if leader {
            // Remove our in-flight entry first, then notify, so any
            // follower that wakes immediately and races to re-take the
            // in-flight lock won't observe a stale `Notify` for a leader
            // that already completed.
            let notify = {
                let mut inflight = IN_FLIGHT.lock();
                inflight.remove(&key)
            };
            if let Some(notify) = notify {
                notify.notify_waiters();
            }
        }

        let uuid = result?;
        debug!(username = username, uuid = %uuid, "UUID fetched from Mojang");

        {
            let mut cache = uuid_cache().lock();
            // Cap enforcement: if we're at the limit, run an
            // opportunistic TTL sweep first; if still at the cap, evict
            // the single oldest entry by `inserted_at`. Bounds the
            // resident set to MAX_UUID_CACHE_ENTRIES even if
            // `cleanup_uuid_cache` never runs. The min-by-key sweep is
            // O(N) but only triggered at the cap (4096 entries), not on
            // the hot path.
            if cache.len() >= MAX_UUID_CACHE_ENTRIES {
                let now = Instant::now();
                cache.retain(|_, (_, inserted)| now.duration_since(*inserted) < ttl);
                if cache.len() >= MAX_UUID_CACHE_ENTRIES
                    && let Some(oldest_key) = cache
                        .iter()
                        .min_by_key(|(_, (_, inserted))| *inserted)
                        .map(|(k, _)| k.clone())
                {
                    cache.remove(&oldest_key);
                    debug!(
                        evicted = %oldest_key,
                        cap = MAX_UUID_CACHE_ENTRIES,
                        "UUID cache at cap, evicted oldest entry"
                    );
                }
            }
            cache.insert(key, (uuid.clone(), Instant::now()));
        }

        Ok(uuid)
    }
}

/// Sync, cache-only UUID lookup. Returns `None` on miss or stale entry —
/// the caller decides whether to fall back to an async fetch. Used by
/// the chat task's reflection trust function, which runs synchronously
/// inside an async block and can't `await` a network call without
/// distorting the surrounding state.
pub fn lookup_cached_uuid(username: &str) -> Option<String> {
    // Production cache keys are lowercased ASCII (post shape-gate). Use
    // `to_ascii_lowercase` here too so the lookup matches insert exactly,
    // independent of any Unicode case-folding quirks in arbitrary callers.
    let key = username.to_ascii_lowercase();
    let ttl = Duration::from_secs(UUID_CACHE_TTL_SECS);
    let cache = uuid_cache().lock();
    cache.get(&key).and_then(|(uuid, ts)| {
        if ts.elapsed() < ttl {
            Some(uuid.clone())
        } else {
            None
        }
    })
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

    /// Cargo runs unit tests in parallel by default, but every test in this
    /// module touches the process-global `UUID_CACHE`. Acquiring this mutex
    /// at the top of each test serializes them so a `clear_uuid_cache()` in
    /// one test cannot wipe the fixture another test just inserted.
    /// Cheap to acquire in the uncontended case; only matters during
    /// `cargo test` with the default thread pool.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Take the test lock, swallowing poisoning so a panicking test doesn't
    /// cascade-fail every subsequent one.
    fn lock_test() -> std::sync::MutexGuard<'static, ()> {
        match TEST_LOCK.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    #[test]
    fn uuid_cache_insert_then_read_returns_same_entry() {
        let _g = lock_test();
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
        // The cache stores keys lowercased; `lookup_cached_uuid` lowercases
        // the lookup argument. Verify mixed-case callers all resolve to the
        // same cached entry — this is the contract chat/mod.rs:718 relies on
        // when handling whatever capitalisation a sender's name arrives in.
        let _g = lock_test();
        clear_uuid_cache();
        let uuid = "00000000-0000-0000-0000-000000000002".to_string();

        uuid_cache()
            .lock()
            .insert("steve".to_string(), (uuid.clone(), Instant::now()));

        assert_eq!(lookup_cached_uuid("steve"), Some(uuid.clone()));
        assert_eq!(lookup_cached_uuid("Steve"), Some(uuid.clone()));
        assert_eq!(lookup_cached_uuid("STEVE"), Some(uuid));
    }

    #[test]
    fn lookup_cached_uuid_returns_none_on_miss() {
        let _g = lock_test();
        clear_uuid_cache();
        assert_eq!(lookup_cached_uuid("nobody_here"), None);
    }

    #[test]
    fn lookup_cached_uuid_returns_cached_value_on_hit() {
        let _g = lock_test();
        clear_uuid_cache();
        let uuid = "00000000-0000-0000-0000-0000000000aa".to_string();
        uuid_cache()
            .lock()
            .insert("alice".to_string(), (uuid.clone(), Instant::now()));

        assert_eq!(lookup_cached_uuid("alice"), Some(uuid));
    }

    #[test]
    fn lookup_cached_uuid_lowercases_mixed_case_lookups() {
        // Insert under the lowercase key the production path uses, then
        // confirm every casing variant the chat layer might pass routes to
        // the same entry.
        let _g = lock_test();
        clear_uuid_cache();
        let uuid = "00000000-0000-0000-0000-0000000000bb".to_string();
        uuid_cache()
            .lock()
            .insert("steve".to_string(), (uuid.clone(), Instant::now()));

        assert_eq!(lookup_cached_uuid("Steve"), Some(uuid.clone()));
        assert_eq!(lookup_cached_uuid("STEVE"), Some(uuid.clone()));
        assert_eq!(lookup_cached_uuid("steve"), Some(uuid));
    }

    #[test]
    fn lookup_cached_uuid_rejects_stale_entries() {
        let _g = lock_test();
        clear_uuid_cache();
        let uuid = "00000000-0000-0000-0000-0000000000cc".to_string();
        let stale_ts = Instant::now() - Duration::from_secs(UUID_CACHE_TTL_SECS + 1);
        uuid_cache()
            .lock()
            .insert("ghost".to_string(), (uuid, stale_ts));

        assert_eq!(lookup_cached_uuid("ghost"), None);
    }

    #[test]
    fn cleanup_uuid_cache_drops_stale_entries_and_keeps_fresh_ones() {
        let _g = lock_test();
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
        let _g = lock_test();
        clear_uuid_cache();
        let cache = uuid_cache();
        cache.lock().insert("a".to_string(), ("uuid-a".to_string(), Instant::now()));
        cache.lock().insert("b".to_string(), ("uuid-b".to_string(), Instant::now()));

        cleanup_uuid_cache();

        assert_eq!(cache.lock().len(), 2);
    }

    #[test]
    fn clear_uuid_cache_empties_the_cache() {
        let _g = lock_test();
        let cache = uuid_cache();
        cache.lock().insert("a".to_string(), ("uuid-a".to_string(), Instant::now()));
        cache.lock().insert("b".to_string(), ("uuid-b".to_string(), Instant::now()));

        clear_uuid_cache();
        assert!(cache.lock().is_empty());
    }

    #[tokio::test]
    async fn resolve_user_uuid_cfg_test_branch_pads_deterministically() {
        // Mirror the production cfg(test) recipe so we never hand-count. The
        // post-shape-gate branch trims to the first 12 chars and then
        // left-pads, so for any valid Mojang-shape username (3-16 ASCII
        // alphanumeric+_) the trailing segment is the first 12 chars zero-
        // padded on the left to width 12.
        fn expected_test_uuid(username: &str) -> String {
            let trimmed: String = username.chars().take(12).collect();
            format!("00000000-0000-0000-0000-{:0>12}", trimmed)
        }

        // (a) normal short username — left-padded with zeros.
        let abc = "abc";
        assert_eq!(
            resolve_user_uuid(abc).await.unwrap(),
            expected_test_uuid(abc),
        );
        assert_eq!(
            resolve_user_uuid(abc).await.unwrap(),
            "00000000-0000-0000-0000-000000000abc",
        );

        // (b) "steve" — pinned because other tests in the crate rely on it.
        let steve = "steve";
        assert_eq!(
            resolve_user_uuid(steve).await.unwrap(),
            expected_test_uuid(steve),
        );
        assert_eq!(
            resolve_user_uuid(steve).await.unwrap(),
            "00000000-0000-0000-0000-0000000steve",
        );

        // (c) exactly-12-char username — no padding needed.
        let twelve = "abcdefghijkl";
        assert_eq!(twelve.len(), 12);
        assert_eq!(
            resolve_user_uuid(twelve).await.unwrap(),
            expected_test_uuid(twelve),
        );

        // (d) maximum-length valid username (16 chars) — explicitly trimmed
        // to the first 12 chars before padding, so the trailing segment is
        // exactly 12 bytes and the full UUID stays canonical 36-char.
        let sixteen = "abcdefghijklmnop";
        assert_eq!(sixteen.len(), 16);
        assert_eq!(
            resolve_user_uuid(sixteen).await.unwrap(),
            expected_test_uuid(sixteen),
        );
        assert_eq!(
            resolve_user_uuid(sixteen).await.unwrap(),
            "00000000-0000-0000-0000-abcdefghijkl",
        );
        assert_eq!(resolve_user_uuid(sixteen).await.unwrap().len(), 36);
    }

    #[tokio::test]
    async fn resolve_user_uuid_cfg_test_branch_rejects_out_of_shape_usernames() {
        // The cfg(test) branch now mirrors the production shape gate so test
        // fixtures match production error semantics: too-short, too-long, and
        // non-ASCII names all produce a "not found" Err.
        for bad in ["ab", "averylongusername", "has space", "has-dash", "has.dot"] {
            assert!(
                resolve_user_uuid(bad).await.is_err(),
                "expected Err for out-of-shape username {bad:?}"
            );
        }
    }
}
