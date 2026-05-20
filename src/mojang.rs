//! # Mojang UUID resolution
//!
//! Resolves Minecraft usernames to canonical hyphenated UUIDs via the public
//! Mojang API, with an in-process LRU+TTL cache.
//!
//! Lives at the crate root (peer to `store`, `bot`, `cli`, `chat`) because
//! more than one module needs UUIDs: the Store keys users on UUID, and the
//! Chat module keys per-player memory on UUID. Hosting the resolver inside
//! `store::*` would force `chat` to import from `store::*`, which is
//! deliberately forbidden by the chat-module design (chat must never see
//! Store state).
//!
//! ## Cache layout
//!
//! The UUID cache is a `HashMap<String, CachedEntry>` behind a
//! `parking_lot::Mutex`. Each [`CachedEntry`] carries its TTL bookkeeping
//! plus a `last_access: Instant` so cap-overflow eviction is LRU-by-access
//! rather than FIFO. Three entry kinds are stored:
//!
//! - `Found(uuid, fetched_at, last_access)` — positive cache hit.
//! - `NotFound(stored_at, last_access)` — short-TTL negative cache so a
//!   chat-spam loop of "Player 'X' not found" doesn't fan out to N Mojang
//!   round-trips. TTL is governed by [`UUID_NEG_CACHE_TTL_SECS`].
//! - `RateLimited { until, last_access }` — typed cooldown so a fresh
//!   resolver call short-circuits with `MojangResolveError::RateLimited`
//!   without touching the network until `until` has passed.
//!
//! ## Single-flight coalescing
//!
//! Concurrent calls for the same uncached lowercased username are coalesced
//! through a per-key `Arc<Notify>` registered in `IN_FLIGHT`. The race-safe
//! follower path uses `Notified::enable()` to register the waker BEFORE the
//! `IN_FLIGHT` lock is dropped, then re-checks the cache once before
//! `await`ing — without that ordering, a leader that completes between the
//! follower's lock-drop and `notified()` registration would park the
//! follower forever.
//!
//! The leader path uses an RAII [`InFlightGuard`] so any task-cancellation,
//! parent abort, or panic during the resolver call still removes the
//! `IN_FLIGHT` entry and wakes parked followers — without the guard, a
//! cancelled leader would strand the key permanently and every subsequent
//! caller would become a follower of a dead leader.
//!
//! Cache write ordering: on success the leader inserts into `UUID_CACHE`
//! BEFORE removing `IN_FLIGHT` and broadcasting, so a follower that wakes
//! and re-reads the cache is guaranteed to see the fresh entry; a caller
//! that arrives after `notify_waiters()` but before `IN_FLIGHT.remove(...)`
//! would otherwise self-promote to leader and fire a redundant request.
//!
//! ## Thread safety
//!
//! Every static (`UUID_CACHE`, `IN_FLIGHT`) is `Send + Sync` via
//! `parking_lot::Mutex`, so any task may call `resolve_user_uuid`,
//! `cleanup_uuid_cache`, or `cleanup_in_flight` regardless of its task
//! flavor. `parking_lot::Mutex` is sync — its `MutexGuard` is never carried
//! across an `.await`.

use std::collections::HashMap;
#[cfg(test)]
use std::sync::Arc;
use std::sync::OnceLock;
#[cfg(not(test))]
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::sync::Notify;
use tracing::debug;

use crate::constants::UUID_CACHE_TTL_SECS;
#[cfg(not(test))]
use crate::types::User;
use crate::types::user::MojangResolveError;

/// TTL for negative-cache (`NotFound`) entries. Short enough that a player
/// who legitimately registers a Mojang account after a typo-driven miss can
/// be resolved within a minute, long enough to absorb a chat-spam stampede
/// (multiple players whispering the same nonexistent name in quick
/// succession) without N upstream calls.
const UUID_NEG_CACHE_TTL_SECS: u64 = 30;

/// Hard cap on `UUID_CACHE` entries (positive + negative + rate-limited).
/// TTL-only eviction lets a long-running bot (weeks of uptime, hundreds of
/// distinct interactors per day) grow the cache unbounded between
/// `cleanup_uuid_cache` cycles. The cap forces a best-effort cleanup pass
/// plus LRU eviction by `last_access` at insert time so the resident set
/// stays bounded even if cleanup never runs.
///
/// 4096 entries × ~96 bytes each (key string + enum payload + two Instants)
/// ≈ 384 KiB — orders of magnitude larger than any realistic active-player
/// set on a single-server bot, but small enough that even adversarial
/// username flooding can't OOM the process.
const MAX_UUID_CACHE_ENTRIES: usize = 4096;

/// Hard cap on `IN_FLIGHT` entries. Pathological cancellation patterns
/// (every leader gets aborted before populating the cache, every follower
/// fails over to a fresh resolver) plus the RAII drop-guard's
/// "remove-on-drop" semantics keep the steady-state size near zero, but
/// the cap defends against an adversarial username flood that fires a
/// leader-per-call and aborts each one before the `Drop` runs (e.g. the
/// containing task is aborted, but the leader future hadn't yielded yet,
/// so its destructors didn't run).
const MAX_IN_FLIGHT_ENTRIES: usize = 4096;

/// Eviction age for stale `IN_FLIGHT` entries. A leader that has held an
/// entry longer than this is either (a) genuinely doing a 30s+ Mojang
/// resolve (which should never happen — the per-request timeout is 10s, so
/// even a worst-case retried request fits in well under 20s) or (b) dead
/// (cancelled, panicked, and somehow bypassed the RAII drop-guard).
const IN_FLIGHT_STALE_AFTER_SECS: u64 = 30;

/// Typed cache entry. Stored in `UUID_CACHE` keyed by lowercased ASCII
/// username. The `last_access` field is updated on every cache hit so
/// cap-overflow eviction is LRU-by-access rather than FIFO.
#[derive(Debug, Clone)]
enum CachedEntry {
    /// Positive hit: Mojang resolved this username to `uuid` at `fetched_at`.
    /// Expires after [`UUID_CACHE_TTL_SECS`].
    Found {
        uuid: String,
        fetched_at: Instant,
        last_access: Instant,
    },
    /// Negative hit: Mojang returned 204 No Content at `stored_at`. Expires
    /// after [`UUID_NEG_CACHE_TTL_SECS`].
    NotFound {
        stored_at: Instant,
        last_access: Instant,
    },
    /// Cooldown: a recent resolve hit HTTP 429 and `until` is the earliest
    /// time a fresh attempt is allowed. The retry-after Duration has
    /// already been clamped by `parse_retry_after` (1h max).
    RateLimited {
        until: Instant,
        retry_after: Option<Duration>,
        last_access: Instant,
    },
}

impl CachedEntry {
    fn last_access(&self) -> Instant {
        match self {
            CachedEntry::Found { last_access, .. }
            | CachedEntry::NotFound { last_access, .. }
            | CachedEntry::RateLimited { last_access, .. } => *last_access,
        }
    }

    fn touch(&mut self, now: Instant) {
        match self {
            CachedEntry::Found { last_access, .. }
            | CachedEntry::NotFound { last_access, .. }
            | CachedEntry::RateLimited { last_access, .. } => *last_access = now,
        }
    }

    /// Is this entry still within its TTL? Positive and negative entries
    /// have different TTLs; `RateLimited` lives until its `until` time.
    fn is_fresh(&self, now: Instant, ttl: Duration, neg_ttl: Duration) -> bool {
        match self {
            CachedEntry::Found { fetched_at, .. } => now.duration_since(*fetched_at) < ttl,
            CachedEntry::NotFound { stored_at, .. } => now.duration_since(*stored_at) < neg_ttl,
            CachedEntry::RateLimited { until, .. } => now < *until,
        }
    }
}

/// Map of lowercased username -> [`CachedEntry`].
type UuidCache = HashMap<String, CachedEntry>;

/// Global UUID cache for Mojang API lookups. TTL-expiry on read AND a hard
/// size cap on insert — stale entries are rejected on read and pruned
/// periodically by [`cleanup_uuid_cache`]; oversized maps trigger an
/// opportunistic cleanup + LRU-by-access eviction inline. Negative cache
/// (NotFound, RateLimited) entries live here too so a stampede behind a
/// known-bad username can't fan out to N upstream calls.
static UUID_CACHE: OnceLock<Mutex<UuidCache>> = OnceLock::new();

fn uuid_cache() -> &'static Mutex<UuidCache> {
    UUID_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Per-key in-flight coordinator entry.
struct InFlightEntry {
    notify: Arc<Notify>,
    /// Wall-clock-ish time the leader registered. Used by
    /// [`cleanup_in_flight`] to evict stale leaders (cancelled tasks whose
    /// `Drop` somehow didn't run, or genuinely runaway resolvers).
    started_at: Instant,
}

/// Per-key in-flight coordinator for [`resolve_user_uuid`]. See the
/// module-level doc for the full coalescing protocol.
#[cfg(not(test))]
static IN_FLIGHT: LazyLock<Mutex<HashMap<String, InFlightEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(test)]
static IN_FLIGHT_TEST: OnceLock<Mutex<HashMap<String, InFlightEntry>>> = OnceLock::new();

#[cfg(test)]
fn in_flight() -> &'static Mutex<HashMap<String, InFlightEntry>> {
    IN_FLIGHT_TEST.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(not(test))]
fn in_flight() -> &'static Mutex<HashMap<String, InFlightEntry>> {
    &IN_FLIGHT
}

/// RAII guard that removes a leader's `IN_FLIGHT` entry and wakes parked
/// followers on drop. The leader's success path calls
/// [`InFlightGuard::disarm`] *after* writing the cache and explicitly
/// running the remove+notify in the documented order; an error or
/// cancellation path leaves the guard armed so `Drop` runs the same
/// cleanup. Without this, a `tokio::select!` arm racing the leader's
/// `await`, a `JoinHandle::abort()` on the parent task, or a panic inside
/// the resolver would strand the `IN_FLIGHT` key permanently and every
/// subsequent caller would become a follower of a dead leader.
struct InFlightGuard {
    key: Option<String>,
}

impl InFlightGuard {
    fn arm(key: String) -> Self {
        Self { key: Some(key) }
    }
    fn disarm(&mut self) {
        self.key = None;
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            let removed = in_flight().lock().remove(&key);
            if let Some(entry) = removed {
                entry.notify.notify_waiters();
            }
        }
    }
}

/// Pinned boxed future alias for the test-only pluggable resolver. Avoids
/// pulling in the `futures` crate just to alias `BoxFuture` — the inner
/// `Pin<Box<dyn Future + Send>>` is exactly what `BoxFuture` expands to.
#[cfg(test)]
type ResolverFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, MojangResolveError>> + Send>>;

/// Boxed pluggable resolver type. Production passes `User::get_uuid_async`;
/// tests pass a closure that simulates concurrency / cancellation / typed
/// errors so the single-flight protocol can be exercised without HTTP.
#[cfg(test)]
pub type Resolver = std::sync::Arc<dyn Fn(String) -> ResolverFuture + Send + Sync>;

/// Public entrypoint. Resolves a Minecraft username to a canonical
/// hyphenated Mojang UUID.
///
/// Lookups are cached for [`UUID_CACHE_TTL_SECS`] (default 5 minutes).
/// Repeated calls for the same player reuse the cached UUID instead of
/// hitting the Mojang API on every interaction. Cache keys are lowercased
/// so `Steve` and `steve` share an entry. Concurrent calls for the same
/// uncached username are coalesced via a per-key `Notify` so only one
/// HTTPS round-trip ever fires per (lowercased) name even under
/// thundering-herd load.
///
/// Negative caching: on `NotFound` and `RateLimited`, a short-TTL entry is
/// stored so a stampede behind a known-bad username (or a per-username
/// rate-limit cooldown) short-circuits at the cache instead of fanning out
/// to N upstream calls. The cooldown is per-username, by design — a
/// rate-limit on resolving `alice` does not prevent resolving `bob`.
///
/// Returns a typed [`MojangResolveError`] so call sites can route each
/// failure mode to a sanitized `StoreError` (`UserNotFound` /
/// `ValidationError` / `MojangNetwork` / `MojangRateLimited`) without ever
/// stringifying a `reqwest::Error` into a player-facing whisper.
///
/// **Test-build behavior**: under `#[cfg(test)]` the production cache /
/// single-flight path is bypassed entirely and a deterministic fixture
/// UUID is returned. Tests that need to observe single-flight or cache
/// behavior call [`resolve_user_uuid_with_resolver`] directly with a
/// controllable resolver closure.
pub async fn resolve_user_uuid(username: &str) -> Result<String, MojangResolveError> {
    #[cfg(test)]
    {
        // Offline deterministic UUID for integration tests: avoids hitting the
        // Mojang API (which requires network and introduces flakiness).
        //
        // Returns `InvalidShape` (not `NotFound`) for shape-failing usernames
        // so the typed-error contract matches production's
        // `resolve_user_uuid_inner` path. A previous divergence (returning
        // `NotFound`) silently broke `matches!(err, InvalidShape)` assertions.
        if !crate::types::user::is_valid_username_shape(username) {
            return Err(MojangResolveError::InvalidShape);
        }
        Ok(fixture_uuid(username))
    }
    #[cfg(not(test))]
    {
        resolve_user_uuid_inner(username, |u: String| async move {
            User::get_uuid_async(&u).await
        })
        .await
    }
}

/// Pluggable-resolver entrypoint, exposed under `cfg(test)` so the
/// single-flight tests can drive a deterministic resolver. The production
/// path goes through [`resolve_user_uuid`] which inlines
/// `User::get_uuid_async` as the resolver.
#[cfg(test)]
pub async fn resolve_user_uuid_with_resolver(
    username: &str,
    resolver: Resolver,
) -> Result<String, MojangResolveError> {
    let r = resolver.clone();
    resolve_user_uuid_inner(username, move |u| r(u)).await
}

/// Core single-flight + cache loop. Generic over the resolver function so
/// tests can inject controllable backends and production can wire in
/// `User::get_uuid_async`. See the module-level doc for the protocol.
async fn resolve_user_uuid_inner<F, Fut>(
    username: &str,
    resolver: F,
) -> Result<String, MojangResolveError>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<String, MojangResolveError>>,
{
    // Defense-in-depth shape check before the cache lookup so a junk
    // username can't pollute the cache or burn a Mojang round-trip.
    if !crate::types::user::is_valid_username_shape(username) {
        return Err(MojangResolveError::InvalidShape);
    }
    let key = username.to_ascii_lowercase();
    let ttl = Duration::from_secs(UUID_CACHE_TTL_SECS);
    let neg_ttl = Duration::from_secs(UUID_NEG_CACHE_TTL_SECS);

    // Cache short-circuit: a positive hit returns the UUID; a fresh
    // negative or rate-limited entry returns the typed error without
    // touching the network.
    if let Some(result) = check_cache(&key, username, ttl, neg_ttl) {
        return result;
    }

    // Single-flight coalescing. Three roles are possible:
    //   Leader: no in-flight entry — we install one and run the resolver.
    //   Follower: an in-flight entry exists — we register a `Notified`
    //     waiter atomically (via `enable()`) BEFORE dropping the lock, so
    //     the leader cannot broadcast in the window between our lock-drop
    //     and our `await`.
    //   Late-arriver: enable() registered, we drop the lock and re-check
    //     the cache once; if the leader already populated it, we return
    //     without awaiting at all.
    enum SingleFlight {
        Leader,
        FollowerOf(Arc<Notify>),
    }
    let role = {
        let mut inflight = in_flight().lock();
        // Cap defense for IN_FLIGHT (pathological cancellation may have
        // bypassed Drop). Same opportunistic policy as UUID_CACHE: best-
        // effort age sweep, then evict the oldest entry.
        evict_in_flight_if_needed(&mut inflight);
        if let Some(entry) = inflight.get(&key) {
            SingleFlight::FollowerOf(Arc::clone(&entry.notify))
        } else {
            inflight.insert(
                key.clone(),
                InFlightEntry {
                    notify: Arc::new(Notify::new()),
                    started_at: Instant::now(),
                },
            );
            SingleFlight::Leader
        }
    };

    match role {
        SingleFlight::FollowerOf(notify) => {
            // Race-safe registration: build the `Notified` future and call
            // `enable()` BEFORE dropping any locks / awaiting. `enable()`
            // (tokio 1.13+) registers the waker with the underlying
            // intrusive list so a `notify_waiters()` from the leader that
            // races us is guaranteed to wake us.
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            // Re-check the cache: if the leader populated UUID_CACHE in
            // the gap between our role decision and `enable()`, we can
            // short-circuit without awaiting at all.
            if let Some(result) = check_cache(&key, username, ttl, neg_ttl) {
                return result;
            }

            debug!(
                username = username,
                "UUID coalesced behind in-flight resolve"
            );
            notified.await;

            // Re-read the cache on wake. With change #3 the leader always
            // writes the cache (positive, NotFound, or RateLimited) before
            // notifying, so a non-error leader outcome is guaranteed to
            // show up here. A transport-error leader (NetworkError /
            // NetworkTimeout / UpstreamError / MalformedResponse) leaves
            // no cache entry — the follower falls through and tries the
            // resolver itself, which is the right semantics (the leader's
            // typed transport error isn't ours to synthesize).
            if let Some(result) = check_cache(&key, username, ttl, neg_ttl) {
                return result;
            }
            // Fall through: a transient leader-failure happened. Promote
            // ourselves to leader for this username. Best-effort: a
            // follower-as-leader path doesn't re-claim IN_FLIGHT (the
            // leader's Drop just removed it; another concurrent caller
            // may already be promoting in parallel — that's an acceptable
            // small-cardinality stampede after a hard transport failure).
            resolver(username.to_string()).await
        }
        SingleFlight::Leader => {
            // Arm the RAII guard so any cancellation / panic from this
            // point onward still removes our IN_FLIGHT entry and wakes
            // followers. The explicit-cleanup code path (success or
            // typed-error-with-cache-write) calls `guard.disarm()` after
            // its own cache-write-then-remove+notify sequence.
            let mut guard = InFlightGuard::arm(key.clone());

            let result = resolver(username.to_string()).await;

            // Order matters: write the cache FIRST (positive, NotFound,
            // or RateLimited), THEN remove IN_FLIGHT and broadcast. This
            // closes the window where a caller arriving between
            // `notify_waiters()` and `remove(&key)` would see neither the
            // in-flight entry nor the cache and self-promote to leader,
            // firing a redundant Mojang request.
            let to_return = match &result {
                Ok(uuid) => {
                    insert_found(&key, uuid.clone());
                    Ok(uuid.clone())
                }
                Err(MojangResolveError::NotFound { .. }) => {
                    insert_not_found(&key);
                    result
                }
                Err(MojangResolveError::RateLimited { retry_after }) => {
                    insert_rate_limited(&key, *retry_after);
                    Err(MojangResolveError::RateLimited {
                        retry_after: *retry_after,
                    })
                }
                Err(_) => {
                    // Transport-level error: leave no cache entry.
                    // Followers fall through and try themselves. The
                    // typed error returns to our caller.
                    result
                }
            };

            // Disarm the guard and run the same remove+notify it would
            // have run on drop — but only AFTER the cache write above is
            // visible to a waking follower.
            guard.disarm();
            let removed = in_flight().lock().remove(&key);
            if let Some(entry) = removed {
                entry.notify.notify_waiters();
            }

            to_return
        }
    }
}

/// Read the cache for `key`, treating `username` as the original
/// (caller-supplied) casing for use in `NotFound`'s typed error. Returns:
///   `Some(Ok(uuid))` on a fresh positive hit (touches `last_access`),
///   `Some(Err(NotFound))` on a fresh negative entry,
///   `Some(Err(RateLimited))` on an unexpired cooldown,
///   `None` on miss or stale entry.
fn check_cache(
    key: &str,
    username: &str,
    ttl: Duration,
    neg_ttl: Duration,
) -> Option<Result<String, MojangResolveError>> {
    let now = Instant::now();
    let mut cache = uuid_cache().lock();
    let entry = cache.get_mut(key)?;
    if !entry.is_fresh(now, ttl, neg_ttl) {
        return None;
    }
    // Only refresh the LRU rank for positive hits. Touching negative entries
    // lets an adversary spamming distinct nonexistent usernames keep the
    // negative entries at the freshest LRU rank, flushing warm positive
    // identities under cap pressure.
    if matches!(entry, CachedEntry::Found { .. }) {
        entry.touch(now);
    }
    match entry {
        CachedEntry::Found { uuid, .. } => {
            let uuid = uuid.clone();
            debug!(username = username, uuid = %uuid, "UUID cache hit");
            Some(Ok(uuid))
        }
        CachedEntry::NotFound { .. } => {
            debug!(username = username, "UUID negative cache hit (NotFound)");
            Some(Err(MojangResolveError::NotFound {
                username: username.to_string(),
            }))
        }
        CachedEntry::RateLimited {
            until, retry_after, ..
        } => {
            let remaining = until.saturating_duration_since(now);
            debug!(
                username = username,
                remaining_secs = remaining.as_secs(),
                "UUID rate-limited cache hit"
            );
            // Always present a non-None retry_after to the caller (the
            // remaining time), even if the original 429 didn't include a
            // Retry-After header — the cooldown duration is information
            // the caller needs.
            let ra = retry_after.or(Some(remaining));
            Some(Err(MojangResolveError::RateLimited { retry_after: ra }))
        }
    }
}

fn insert_found(key: &str, uuid: String) {
    let now = Instant::now();
    let mut cache = uuid_cache().lock();
    evict_cache_if_needed(&mut cache);
    cache.insert(
        key.to_string(),
        CachedEntry::Found {
            uuid,
            fetched_at: now,
            last_access: now,
        },
    );
}

fn insert_not_found(key: &str) {
    let now = Instant::now();
    let mut cache = uuid_cache().lock();
    evict_cache_if_needed(&mut cache);
    cache.insert(
        key.to_string(),
        CachedEntry::NotFound {
            stored_at: now,
            last_access: now,
        },
    );
}

fn insert_rate_limited(key: &str, retry_after: Option<Duration>) {
    let now = Instant::now();
    // Use the (already-clamped) retry_after if Mojang supplied one, else
    // a conservative default that matches the 30s negative-cache TTL —
    // matches operator expectation that a 429 backs off for at least the
    // same window as a NotFound.
    let cooldown = retry_after.unwrap_or_else(|| Duration::from_secs(UUID_NEG_CACHE_TTL_SECS));
    let mut cache = uuid_cache().lock();
    evict_cache_if_needed(&mut cache);
    cache.insert(
        key.to_string(),
        CachedEntry::RateLimited {
            until: now + cooldown,
            retry_after,
            last_access: now,
        },
    );
}

/// At-cap eviction: opportunistically TTL-sweep, then if still at cap,
/// evict the single least-recently-accessed entry. Both passes are O(N)
/// but only triggered at cap (4096 entries), not on the hot path.
fn evict_cache_if_needed(cache: &mut UuidCache) {
    if cache.len() < MAX_UUID_CACHE_ENTRIES {
        return;
    }
    let now = Instant::now();
    let ttl = Duration::from_secs(UUID_CACHE_TTL_SECS);
    let neg_ttl = Duration::from_secs(UUID_NEG_CACHE_TTL_SECS);
    cache.retain(|_, entry| entry.is_fresh(now, ttl, neg_ttl));
    if cache.len() < MAX_UUID_CACHE_ENTRIES {
        return;
    }
    // LRU-by-access with a negative-bias tiebreak: when at cap, prefer
    // evicting negative entries (NotFound/RateLimited) over positive ones
    // at equal age. This blunts negative-cache flush attacks against the
    // positive (Found) entries that the bot actually depends on.
    if let Some(oldest_key) = cache
        .iter()
        .min_by_key(|(_, entry)| {
            let is_positive = matches!(entry, CachedEntry::Found { .. });
            (is_positive, entry.last_access())
        })
        .map(|(k, _)| k.clone())
    {
        cache.remove(&oldest_key);
        debug!(
            evicted = %oldest_key,
            cap = MAX_UUID_CACHE_ENTRIES,
            "UUID cache at cap, evicted LRU entry"
        );
    }
}

/// At-cap eviction for IN_FLIGHT: sweep stale entries first, then if still
/// at cap, evict the oldest (by `started_at`). Steady-state size should be
/// near zero — the cap is purely a defense against pathological
/// cancellation that bypasses the RAII drop-guard.
fn evict_in_flight_if_needed(inflight: &mut HashMap<String, InFlightEntry>) {
    if inflight.len() < MAX_IN_FLIGHT_ENTRIES {
        return;
    }
    let now = Instant::now();
    let stale = Duration::from_secs(IN_FLIGHT_STALE_AFTER_SECS);
    let to_remove: Vec<String> = inflight
        .iter()
        .filter(|(_, e)| now.duration_since(e.started_at) >= stale)
        .map(|(k, _)| k.clone())
        .collect();
    for k in to_remove {
        if let Some(entry) = inflight.remove(&k) {
            // Wake any followers parked on this stale leader so they can
            // re-check the cache (likely miss) and self-promote.
            entry.notify.notify_waiters();
        }
    }
    if inflight.len() < MAX_IN_FLIGHT_ENTRIES {
        return;
    }
    if let Some(oldest_key) = inflight
        .iter()
        .min_by_key(|(_, e)| e.started_at)
        .map(|(k, _)| k.clone())
    {
        if let Some(entry) = inflight.remove(&oldest_key) {
            entry.notify.notify_waiters();
        }
    }
}

/// Sync, cache-only UUID lookup. Returns `None` on miss, stale entry, or
/// negative-cache hit — the caller decides whether to fall back to an
/// async fetch. Used by the chat task's reflection trust function, which
/// runs synchronously inside an async block and can't `await` a network
/// call without distorting the surrounding state.
///
/// Touches `last_access` on a positive hit so LRU-by-access eviction
/// reflects sync callers as well.
pub fn lookup_cached_uuid(username: &str) -> Option<String> {
    let key = username.to_ascii_lowercase();
    let ttl = Duration::from_secs(UUID_CACHE_TTL_SECS);
    let now = Instant::now();
    let mut cache = uuid_cache().lock();
    let entry = cache.get_mut(&key)?;
    if let CachedEntry::Found {
        uuid,
        fetched_at,
        last_access,
    } = entry
    {
        if now.duration_since(*fetched_at) < ttl {
            *last_access = now;
            return Some(uuid.clone());
        }
    }
    None
}

/// Drop UUID cache entries that have exceeded their TTL — positive
/// (`Found`) entries past [`UUID_CACHE_TTL_SECS`], negative (`NotFound`)
/// entries past [`UUID_NEG_CACHE_TTL_SECS`], and `RateLimited` entries
/// whose `until` has passed.
///
/// Stale entries never serve a cache hit (the `is_fresh` check in
/// [`check_cache`] rejects them), but unless they are removed they keep
/// growing the HashMap indefinitely. Callable from any task —
/// `parking_lot::Mutex` is `Send + Sync`.
///
/// Also sweeps `IN_FLIGHT` via [`cleanup_in_flight`] so the maintenance
/// schedule that already runs this cache-cleanup also catches stale
/// in-flight leaders without requiring an extra call site.
pub fn cleanup_uuid_cache() {
    {
        let mut cache = uuid_cache().lock();
        let now = Instant::now();
        let ttl = Duration::from_secs(UUID_CACHE_TTL_SECS);
        let neg_ttl = Duration::from_secs(UUID_NEG_CACHE_TTL_SECS);
        let before = cache.len();
        cache.retain(|_, entry| entry.is_fresh(now, ttl, neg_ttl));
        let removed = before - cache.len();
        if removed > 0 {
            debug!(
                removed = removed,
                remaining = cache.len(),
                "Evicted stale UUID cache entries"
            );
        } else {
            debug!(
                remaining = cache.len(),
                "UUID cache cleanup: no stale entries"
            );
        }
    }
    cleanup_in_flight();
}

/// Drop `IN_FLIGHT` entries that have lingered past
/// [`IN_FLIGHT_STALE_AFTER_SECS`]. The RAII drop-guard handles the common
/// case (the leader future is dropped, removing the entry), so steady
/// state is near-zero; this sweeper catches the pathological case where
/// `Drop` was bypassed (panic during destructor unwind, exotic
/// task-cancellation races).
pub fn cleanup_in_flight() {
    let mut inflight = in_flight().lock();
    let now = Instant::now();
    let stale = Duration::from_secs(IN_FLIGHT_STALE_AFTER_SECS);
    let before = inflight.len();
    let to_remove: Vec<String> = inflight
        .iter()
        .filter(|(_, e)| now.duration_since(e.started_at) >= stale)
        .map(|(k, _)| k.clone())
        .collect();
    for k in to_remove {
        if let Some(entry) = inflight.remove(&k) {
            // Wake any parked followers so they can re-check the cache
            // and fall through to self-promotion.
            entry.notify.notify_waiters();
        }
    }
    let removed = before - inflight.len();
    if removed > 0 {
        debug!(
            removed = removed,
            remaining = inflight.len(),
            "Evicted stale IN_FLIGHT entries"
        );
    }
}

/// Clear the entire UUID cache. Test-only — used to isolate cache tests.
#[cfg(test)]
pub fn clear_uuid_cache() {
    uuid_cache().lock().clear();
    in_flight().lock().clear();
}

/// Canonical synthesis of the deterministic `cfg(test)` UUID fixture.
///
/// `resolve_user_uuid`'s test branch and every per-module `test_uuid` helper
/// route through this single function so a test that pre-seeds
/// `store.users` keyed on `fixture_uuid("Alice")` is guaranteed to be found
/// by a handler that resolves "Alice" via `resolve_user_uuid`. Without this
/// shared helper the two formulas drifted: the resolver produced an FNV-1a
/// digest while test helpers pad-with-literal-username — silently breaking
/// auto-create / self-pay / balance-transfer round-trips.
///
/// The padding uses an FNV-1a hex digest of the lowercased username so the
/// resulting UUID satisfies `is_valid_uuid_shape` for any input (including
/// usernames containing non-hex letters like `s`, `t`, `v` or `_`).
#[cfg(test)]
pub fn fixture_uuid(username: &str) -> String {
    let mut acc: u64 = 0xcbf29ce484222325; // FNV-1a 64 offset basis
    for b in username.to_ascii_lowercase().bytes() {
        acc ^= b as u64;
        acc = acc.wrapping_mul(0x100000001b3); // FNV-1a prime
    }
    let padded = format!("{:012x}", acc & 0xFFFF_FFFF_FFFF);
    let out = format!("00000000-0000-0000-0000-{}", padded);
    debug_assert_eq!(
        out.len(),
        36,
        "test fixture produced non-canonical UUID for {username:?}: {out:?}"
    );
    debug_assert!(
        crate::types::user::is_valid_uuid_shape(&out),
        "test fixture produced shape-invalid UUID for {username:?}: {out:?}"
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Cargo runs unit tests in parallel by default, but every test in this
    /// module touches the process-global `UUID_CACHE`. Acquiring this mutex
    /// at the top of each test serializes them so a `clear_uuid_cache()` in
    /// one test cannot wipe the fixture another test just inserted.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_test() -> std::sync::MutexGuard<'static, ()> {
        match TEST_LOCK.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    // ---------- helpers for direct cache poking ----------

    fn insert_found_now(key: &str, uuid: &str) {
        let now = Instant::now();
        uuid_cache().lock().insert(
            key.to_string(),
            CachedEntry::Found {
                uuid: uuid.to_string(),
                fetched_at: now,
                last_access: now,
            },
        );
    }

    fn insert_found_stale(key: &str, uuid: &str) {
        let now = Instant::now();
        let stale = now - Duration::from_secs(UUID_CACHE_TTL_SECS + 1);
        uuid_cache().lock().insert(
            key.to_string(),
            CachedEntry::Found {
                uuid: uuid.to_string(),
                fetched_at: stale,
                last_access: stale,
            },
        );
    }

    // ---------- existing-style cache tests, ported to the typed enum ----------

    #[test]
    fn uuid_cache_insert_then_read_returns_same_entry() {
        let _g = lock_test();
        clear_uuid_cache();
        insert_found_now("testplayer", "00000000-0000-0000-0000-000000000001");
        assert_eq!(
            lookup_cached_uuid("testplayer"),
            Some("00000000-0000-0000-0000-000000000001".to_string())
        );
    }

    #[test]
    fn lookup_cached_uuid_uses_lowercased_key() {
        let _g = lock_test();
        clear_uuid_cache();
        let uuid = "00000000-0000-0000-0000-000000000002";
        insert_found_now("steve", uuid);
        assert_eq!(lookup_cached_uuid("steve"), Some(uuid.to_string()));
        assert_eq!(lookup_cached_uuid("Steve"), Some(uuid.to_string()));
        assert_eq!(lookup_cached_uuid("STEVE"), Some(uuid.to_string()));
    }

    #[test]
    fn lookup_cached_uuid_returns_none_on_miss() {
        let _g = lock_test();
        clear_uuid_cache();
        assert_eq!(lookup_cached_uuid("nobody_here"), None);
    }

    #[test]
    fn lookup_cached_uuid_rejects_stale_entries() {
        let _g = lock_test();
        clear_uuid_cache();
        insert_found_stale("ghost", "00000000-0000-0000-0000-0000000000cc");
        assert_eq!(lookup_cached_uuid("ghost"), None);
    }

    #[test]
    fn lookup_cached_uuid_skips_negative_entries() {
        // Negative cache entries should not surface as positive hits to
        // sync callers — they only short-circuit the async resolver path.
        let _g = lock_test();
        clear_uuid_cache();
        let now = Instant::now();
        uuid_cache().lock().insert(
            "missing".to_string(),
            CachedEntry::NotFound {
                stored_at: now,
                last_access: now,
            },
        );
        assert_eq!(lookup_cached_uuid("missing"), None);
    }

    #[test]
    fn cleanup_uuid_cache_drops_stale_entries_and_keeps_fresh_ones() {
        let _g = lock_test();
        clear_uuid_cache();
        insert_found_now("fresh", "uuid-fresh");
        insert_found_stale("stale", "uuid-stale");

        cleanup_uuid_cache();

        let guard = uuid_cache().lock();
        assert!(
            guard.contains_key("fresh"),
            "fresh entry should be retained"
        );
        assert!(
            !guard.contains_key("stale"),
            "stale entry should be dropped"
        );
    }

    #[test]
    fn cleanup_uuid_cache_drops_expired_negative_entries() {
        let _g = lock_test();
        clear_uuid_cache();
        let now = Instant::now();
        // Fresh NotFound.
        uuid_cache().lock().insert(
            "fresh_404".to_string(),
            CachedEntry::NotFound {
                stored_at: now,
                last_access: now,
            },
        );
        // Stale NotFound (past UUID_NEG_CACHE_TTL_SECS).
        let stale = now - Duration::from_secs(UUID_NEG_CACHE_TTL_SECS + 1);
        uuid_cache().lock().insert(
            "stale_404".to_string(),
            CachedEntry::NotFound {
                stored_at: stale,
                last_access: stale,
            },
        );
        // Expired RateLimited.
        uuid_cache().lock().insert(
            "expired_429".to_string(),
            CachedEntry::RateLimited {
                until: now - Duration::from_secs(1),
                retry_after: None,
                last_access: now - Duration::from_secs(5),
            },
        );

        cleanup_uuid_cache();

        let guard = uuid_cache().lock();
        assert!(guard.contains_key("fresh_404"));
        assert!(!guard.contains_key("stale_404"));
        assert!(!guard.contains_key("expired_429"));
    }

    // ---------- resolve_user_uuid (cfg(test) fixture path) ----------

    #[tokio::test]
    async fn resolve_user_uuid_cfg_test_branch_pads_deterministically() {
        // The fixture pads with a hex digest of the lowercased username so
        // the resulting UUID always passes `is_valid_uuid_shape`. We assert
        // the shape and the determinism (same input -> same output, casing
        // canonicalized), not the specific hash bytes.
        let abc = "abc";
        let first = resolve_user_uuid(abc).await.unwrap();
        let second = resolve_user_uuid(abc).await.unwrap();
        assert_eq!(first, second, "fixture must be deterministic");
        assert!(
            crate::types::user::is_valid_uuid_shape(&first),
            "fixture UUID must satisfy is_valid_uuid_shape: {first}"
        );
        // Case-insensitive: usernames are lowercased before digesting.
        let mixed = resolve_user_uuid("AbC").await.unwrap();
        assert_eq!(first, mixed);
    }

    #[tokio::test]
    async fn resolve_user_uuid_cfg_test_branch_rejects_out_of_shape_usernames() {
        for bad in [
            "ab",
            "averylongusername",
            "has space",
            "has-dash",
            "has.dot",
        ] {
            assert!(resolve_user_uuid(bad).await.is_err());
        }
    }

    // ---------- single-flight tests (via resolve_user_uuid_with_resolver) ----------

    /// Build a resolver Arc that increments `counter` on each call and
    /// awaits `gate.notified()` before returning. Used to deterministically
    /// hold the leader inside the resolver while followers are issued.
    fn counted_blocking_resolver(
        counter: Arc<AtomicUsize>,
        gate: Arc<Notify>,
        outcome: Result<String, MojangResolveError>,
    ) -> Resolver {
        Arc::new(move |_u: String| {
            let c = counter.clone();
            let g = gate.clone();
            let out = outcome.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                g.notified().await;
                out
            })
        })
    }

    #[tokio::test]
    async fn single_flight_n_followers_one_backend_call() {
        let _g = lock_test();
        clear_uuid_cache();
        let counter = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Notify::new());
        let resolver = counted_blocking_resolver(
            counter.clone(),
            gate.clone(),
            Ok("00000000-0000-0000-0000-000000000ddd".to_string()),
        );

        // Spawn 16 concurrent resolves for the same username.
        let mut handles = Vec::new();
        for _ in 0..16 {
            let r = resolver.clone();
            handles.push(tokio::spawn(async move {
                resolve_user_uuid_with_resolver("alpha", r).await
            }));
        }

        // Give followers time to register on the leader's Notify.
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Release the leader.
        gate.notify_waiters();

        for h in handles {
            let res = h.await.unwrap();
            assert_eq!(res.unwrap(), "00000000-0000-0000-0000-000000000ddd");
        }

        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "single backend call expected"
        );
    }

    #[tokio::test]
    async fn single_flight_leader_cancellation_leaves_in_flight_clean() {
        let _g = lock_test();
        clear_uuid_cache();
        let counter = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Notify::new());
        let resolver = counted_blocking_resolver(
            counter.clone(),
            gate.clone(),
            Ok("00000000-0000-0000-0000-0000000eeeee".to_string()),
        );

        // Spawn a leader that we'll abort mid-await.
        let r = resolver.clone();
        let leader_handle =
            tokio::spawn(async move { resolve_user_uuid_with_resolver("bravo", r).await });

        // Wait until the leader has registered IN_FLIGHT and is inside
        // the resolver (counter incremented).
        for _ in 0..50 {
            if counter.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        leader_handle.abort();
        // Yield enough for the Drop guard to run.
        for _ in 0..20 {
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(5)).await;
            if !in_flight().lock().contains_key("bravo") {
                break;
            }
        }
        assert!(
            !in_flight().lock().contains_key("bravo"),
            "IN_FLIGHT must be cleared by Drop guard after leader abort"
        );

        // Fresh resolve must succeed (no dead-leader hang) — the new
        // leader gets a new gate via the resolver factory.
        let counter2 = Arc::new(AtomicUsize::new(0));
        let gate2 = Arc::new(Notify::new());
        let resolver2 = counted_blocking_resolver(
            counter2.clone(),
            gate2.clone(),
            Ok("00000000-0000-0000-0000-000000fffffff"
                .chars()
                .take(36)
                .collect()),
        );
        let r2 = resolver2.clone();
        let new_handle =
            tokio::spawn(async move { resolve_user_uuid_with_resolver("bravo", r2).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        gate2.notify_waiters();
        let _ = new_handle.await.unwrap();
        assert_eq!(counter2.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn single_flight_late_follower_observes_cache_via_enable() {
        // Lost-wakeup race: drive a deterministic interleaving where the
        // leader's notify_waiters() races the follower's enable() / await.
        // With the enable()-before-unlock ordering, the follower is
        // guaranteed to observe either (a) the cache write directly via
        // the re-check after enable() or (b) the wake via the registered
        // Notified future. Either way it must NOT hang.
        let _g = lock_test();
        clear_uuid_cache();
        let counter = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Notify::new());
        let resolver = counted_blocking_resolver(
            counter.clone(),
            gate.clone(),
            Ok("00000000-0000-0000-0000-000000aaaaaa".to_string()),
        );

        // Leader spawned first.
        let r1 = resolver.clone();
        let leader =
            tokio::spawn(async move { resolve_user_uuid_with_resolver("charlie", r1).await });
        // Wait for leader to enter the resolver.
        for _ in 0..50 {
            if counter.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // Spawn follower while leader is still parked inside the resolver.
        let r2 = resolver.clone();
        let follower =
            tokio::spawn(async move { resolve_user_uuid_with_resolver("charlie", r2).await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        // Release the leader. The follower must wake without firing a
        // second backend call.
        gate.notify_waiters();

        let leader_res = tokio::time::timeout(Duration::from_secs(2), leader)
            .await
            .expect("leader must not hang")
            .unwrap();
        let follower_res = tokio::time::timeout(Duration::from_secs(2), follower)
            .await
            .expect("follower must not hang")
            .unwrap();
        assert_eq!(leader_res.unwrap(), "00000000-0000-0000-0000-000000aaaaaa");
        assert_eq!(
            follower_res.unwrap(),
            "00000000-0000-0000-0000-000000aaaaaa"
        );
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn negative_cache_short_circuits_n_followers() {
        let _g = lock_test();
        clear_uuid_cache();
        let counter = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Notify::new());
        // Leader returns NotFound; subsequent callers must hit the negative cache.
        let resolver = counted_blocking_resolver(
            counter.clone(),
            gate.clone(),
            Err(MojangResolveError::NotFound {
                username: "delta".to_string(),
            }),
        );

        let r = resolver.clone();
        let leader = tokio::spawn(async move { resolve_user_uuid_with_resolver("delta", r).await });
        // Let leader enter the resolver.
        for _ in 0..50 {
            if counter.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        gate.notify_waiters();
        let res = leader.await.unwrap();
        assert!(matches!(res, Err(MojangResolveError::NotFound { .. })));

        // Now 8 fresh callers — none should hit the resolver.
        let mut handles = Vec::new();
        for _ in 0..8 {
            let r2 = resolver.clone();
            handles.push(tokio::spawn(async move {
                resolve_user_uuid_with_resolver("delta", r2).await
            }));
        }
        for h in handles {
            let r = h.await.unwrap();
            assert!(matches!(r, Err(MojangResolveError::NotFound { .. })));
        }
        // Counter must still be 1 — followers short-circuited at the
        // negative cache.
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn rate_limited_cache_short_circuits_without_network() {
        let _g = lock_test();
        clear_uuid_cache();
        let counter = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Notify::new());
        let resolver = counted_blocking_resolver(
            counter.clone(),
            gate.clone(),
            Err(MojangResolveError::RateLimited {
                retry_after: Some(Duration::from_secs(60)),
            }),
        );

        let r = resolver.clone();
        let leader = tokio::spawn(async move { resolve_user_uuid_with_resolver("echo", r).await });
        for _ in 0..50 {
            if counter.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        gate.notify_waiters();
        let _ = leader.await.unwrap();

        // Follower must short-circuit.
        let r2 = resolver.clone();
        let follower_res = resolve_user_uuid_with_resolver("echo", r2).await;
        assert!(matches!(
            follower_res,
            Err(MojangResolveError::RateLimited { .. })
        ));
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn lru_eviction_respects_access_order() {
        let _g = lock_test();
        clear_uuid_cache();
        let cache = uuid_cache();
        // Fill the cache to exactly the cap with positive entries that
        // all share roughly the same `last_access`.
        let now = Instant::now();
        for i in 0..MAX_UUID_CACHE_ENTRIES {
            cache.lock().insert(
                format!("u{i:06}"),
                CachedEntry::Found {
                    uuid: format!("uuid-{i:06}"),
                    fetched_at: now,
                    last_access: now - Duration::from_micros(i as u64),
                },
            );
        }
        // The oldest `last_access` belongs to the highest-index key (we
        // gave each successive entry a slightly older last_access).
        let oldest_key = format!("u{:06}", MAX_UUID_CACHE_ENTRIES - 1);
        assert!(cache.lock().contains_key(&oldest_key));

        // Touch the oldest so it's now the freshest.
        let _ = lookup_cached_uuid(&oldest_key);

        // Trigger an at-cap insert via the resolver insertion helper.
        insert_found(&"newcomer".to_string(), "uuid-newcomer".to_string());

        // After eviction: newcomer is in, oldest_key (just touched) is in,
        // but the next-oldest key got evicted.
        let guard = cache.lock();
        assert!(guard.contains_key("newcomer"), "newcomer must be inserted");
        assert!(
            guard.contains_key(&oldest_key),
            "touched entry must NOT be evicted (LRU-by-access)"
        );
        // The originally-second-oldest is the next-oldest-untouched.
        let next_oldest = format!("u{:06}", MAX_UUID_CACHE_ENTRIES - 2);
        assert!(
            !guard.contains_key(&next_oldest),
            "next-oldest untouched must be evicted"
        );
    }

    #[test]
    fn cleanup_in_flight_drops_stale_entries() {
        let _g = lock_test();
        clear_uuid_cache();
        let now = Instant::now();
        let stale = now - Duration::from_secs(IN_FLIGHT_STALE_AFTER_SECS + 1);
        in_flight().lock().insert(
            "ghost_leader".to_string(),
            InFlightEntry {
                notify: Arc::new(Notify::new()),
                started_at: stale,
            },
        );
        in_flight().lock().insert(
            "live_leader".to_string(),
            InFlightEntry {
                notify: Arc::new(Notify::new()),
                started_at: now,
            },
        );
        cleanup_in_flight();
        let guard = in_flight().lock();
        assert!(!guard.contains_key("ghost_leader"));
        assert!(guard.contains_key("live_leader"));
    }
}
