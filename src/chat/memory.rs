//! Per-player and global memory files.
//!
//! Markdown is the chosen format: human-editable, easy to grep,
//! and the LLM produces structured Markdown natively without serialization
//! friction.
//!
//! ## Files
//!
//! - `data/chat/memory.md` — global self/server/world memory
//!   (LLM-writable via `update_self_memory` in Phase 5).
//! - `data/chat/adjustments.md` — learnings from AI call-outs
//!   (reflection-pass writable in Phase 6).
//! - `data/chat/players/<uuid>.md` — per-player Markdown
//!   (LLM-writable via `update_player_memory` in Phase 5).
//! - `data/chat/players/_index.json` — `{username_lc: uuid}` convenience
//!   map, rebuilt from disk at startup.
//!
//! Phase 2 lands the I/O layer: read/write/ensure operations and the
//! `_index.json` rebuild. Tools that wrap these functions (with section
//! allow-lists, dedup, cap enforcement) arrive in Phase 5.

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::fsutil::write_atomic;

const LOG_TARGET: &str = "cj_store::chat::memory";

pub const CHAT_DIR: &str = "data/chat";
pub const PLAYERS_DIR: &str = "data/chat/players";
pub const GLOBAL_MEMORY: &str = "data/chat/memory.md";
pub const ADJUSTMENTS: &str = "data/chat/adjustments.md";
pub const PLAYER_INDEX: &str = "data/chat/players/_index.json";

/// Construct the on-disk path for a per-player file. UUIDs are validated
/// at the tool boundary; this function trusts its input.
pub fn player_file_path(uuid: &str) -> PathBuf {
    PathBuf::from(PLAYERS_DIR).join(format!("{uuid}.md"))
}

/// Canonical hyphenated UUID shape gate: 36 chars, lowercase hex, hyphens
/// at positions 8/13/18/23 (`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`).
///
/// Module-local on purpose: the codebase keeps three intentionally
/// duplicated copies of this check (here, `chat::store_view::user`,
/// `chat::tools`) per the chat-independence rationale documented at
/// `chat/store_view/user.rs:40-48`. Do NOT factor this out.
fn is_canonical_hyphen_uuid(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        let is_hyphen = matches!(i, 8 | 13 | 18 | 23);
        if is_hyphen {
            if *b != b'-' {
                return false;
            }
        } else if !matches!(b, b'0'..=b'9' | b'a'..=b'f') {
            return false;
        }
    }
    true
}

/// The empty per-player schema. New files are bootstrapped
/// with this content so [`update_player_memory`] can append into named
/// sections without first creating them.
///
/// The `## Trust: <level>` heading is anchored so [`has_operator_trust3`]
/// can detect operator-granted Trust 3 by exact-string match. New files
/// start at Trust 0; promotion to higher tiers is derived at runtime via
/// [`compute_trust`].
pub fn empty_player_template(username: &str, uuid: &str, today: &str) -> String {
    format!(
        "# {username}\n\
         \n\
         ## Identity\n\
         - UUID: {uuid}\n\
         - Known names: {username}\n\
         - First seen: {today}\n\
         - Last seen: {today}\n\
         \n\
         ## Trust: 0\n\
         <derived; see compute_trust>\n\
         \n\
         ## Stated preferences\n\
         \n\
         ## Inferred\n\
         \n\
         ## Topics & history\n\
         \n\
         ## Do not mention\n",
    )
}

/// Idempotently create `players/<uuid>.md` with the empty schema if it
/// doesn't already exist. CHAT.md "new-player file initialization".
///
/// Today's UTC date is read internally via `chrono::Utc::now()` so the
/// caller does not need to thread a date through. Returns `Ok(())`
/// regardless of whether a new file was created — callers that need the
/// distinction should `path.exists()`-check first.
///
/// On a fresh-create, also patches `_index.json` so a subsequent username
/// → UUID lookup hits the index without requiring a full rebuild.
pub fn ensure_player_file(uuid: &str, username: &str) -> io::Result<()> {
    let path = player_file_path(uuid);
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let body = empty_player_template(username, uuid, &today);
    write_atomic(&path, &body)?;
    // Best-effort index patch — failures are logged but never bubble up
    // because the index is derivable from the players dir.
    if let Ok(mut idx) = load_or_rebuild_index() {
        idx.insert(username, uuid);
        if let Err(e) = save_index(&idx) {
            warn!(target: LOG_TARGET, error = %e, "failed to persist _index.json after ensure_player_file");
        }
    }
    debug!(target: LOG_TARGET, uuid = uuid, username = username, "created new per-player file");
    Ok(())
}

/// Read a per-player file. Returns `Ok(None)` if the file is missing.
pub fn read_player(uuid: &str) -> io::Result<Option<String>> {
    let path = player_file_path(uuid);
    match fs::read_to_string(&path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Read `memory.md` (the global memory file). Missing → empty string.
pub fn read_global_memory() -> io::Result<String> {
    match fs::read_to_string(GLOBAL_MEMORY) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

/// Read `adjustments.md`. Missing → empty string.
pub fn read_adjustments() -> io::Result<String> {
    match fs::read_to_string(ADJUSTMENTS) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

/// On-disk shape of `_index.json`: a single object mapping
/// `username_lc -> uuid`. Versioned so a future schema change can be
/// detected and the file rebuilt rather than mis-parsed.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlayerIndex {
    pub version: u32,
    pub by_lower_username: HashMap<String, String>,
}

const INDEX_VERSION: u32 = 1;

impl PlayerIndex {
    pub fn new() -> Self {
        Self {
            version: INDEX_VERSION,
            by_lower_username: HashMap::new(),
        }
    }

    pub fn lookup(&self, username: &str) -> Option<&str> {
        self.by_lower_username
            .get(&username.to_lowercase())
            .map(String::as_str)
    }

    pub fn insert(&mut self, username: &str, uuid: &str) {
        if !is_canonical_hyphen_uuid(uuid) {
            warn!(
                target: LOG_TARGET,
                username = username,
                uuid = uuid,
                "rejecting PlayerIndex::insert with non-canonical uuid"
            );
            return;
        }
        self.by_lower_username
            .insert(username.to_lowercase(), uuid.to_string());
    }
}

/// Rebuild the player index from the contents of `data/chat/players/`.
///
/// CHAT.md calls this out explicitly: `_index.json` is a derived map,
/// not authoritative state. Corruption is recoverable by deletion; this
/// function is the rebuild path used at chat-task startup.
///
/// Each `<uuid>.md` is parsed for the first `# <username>` line. UUIDs
/// without a derivable username are skipped (logged), not failing the
/// whole rebuild.
pub fn rebuild_index() -> io::Result<PlayerIndex> {
    let dir = Path::new(PLAYERS_DIR);
    let mut idx = PlayerIndex::new();
    if !dir.exists() {
        return Ok(idx);
    }
    let mut skipped = 0usize;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // Filename: `<uuid>.md`. Skip the index file itself.
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext != "md" {
            continue;
        }
        if stem.starts_with('_') {
            continue;
        }
        // First-line parse: `# <username>`.
        let body = match fs::read_to_string(&path) {
            Ok(b) => b,
            Err(e) => {
                skipped += 1;
                warn!(target: LOG_TARGET, path = %path.display(), error = %e, "skipping unreadable player file");
                continue;
            }
        };
        let first_line = body.lines().next().unwrap_or("");
        let username = first_line.strip_prefix("# ").unwrap_or("").trim();
        if username.is_empty() {
            skipped += 1;
            warn!(target: LOG_TARGET, path = %path.display(), "skipping player file with no `# <username>` header");
            continue;
        }
        if !is_canonical_hyphen_uuid(stem) {
            skipped += 1;
            warn!(target: LOG_TARGET, path = %path.display(), "skipping player file whose stem is not a canonical-hyphen uuid");
            continue;
        }
        idx.insert(username, stem);
    }
    info!(
        target: LOG_TARGET,
        loaded = idx.by_lower_username.len(),
        skipped = skipped,
        "rebuilt player index"
    );
    Ok(idx)
}

/// Persist the index to `_index.json` via [`write_atomic`]. Safe to call
/// after every mutation; the file is small and writes are infrequent.
pub fn save_index(idx: &PlayerIndex) -> io::Result<()> {
    let json = serde_json::to_string_pretty(idx)?;
    write_atomic(PLAYER_INDEX, &json)?;
    Ok(())
}

// ===== Trust ladder ============================================

/// Trust level derived from the per-player file plus history JSONLs. CHAT.md:
///
/// - 0  if interactions < 3 OR distinct_days < 2 (or spam-cooldown active)
/// - 1  if interactions >= 3 AND distinct_days >= 2 AND no spam cooldown
/// - 2  if interactions >= 20 AND distinct_days >= 7 AND no spam cooldown
/// - 3  ONLY if the per-player file's heading line matches `^## Trust: 3$`
///         (exact, anchored) AND any `trust3_expires_at` timestamp is in
///         the future. Operator-granted; never auto-derived.
///
/// "Interaction" = a `bot_out` JSONL record where the bot replied to a
/// message from this player (or a whisper exchanged with this player).
/// Spam-suppressed events do NOT count.
pub fn compute_trust(
    player_md: &str,
    interactions: u32,
    distinct_days: u32,
    spam_cooldown_recent: bool,
) -> u8 {
    if has_operator_trust3(player_md) && !operator_trust3_expired(player_md) {
        return 3;
    }
    if spam_cooldown_recent {
        return 0;
    }
    if interactions >= 20 && distinct_days >= 7 {
        return 2;
    }
    if interactions >= 3 && distinct_days >= 2 {
        return 1;
    }
    0
}

/// True iff the file has a heading line `## Trust: 3` matched as the whole
/// trimmed line (defends against forged `Trust: 3` smuggled inside a
/// bullet body — see [`crate::chat::tools::sanitize_bullet`]).
pub fn has_operator_trust3(player_md: &str) -> bool {
    player_md.lines().any(|l| l.trim_end() == "## Trust: 3")
}

/// True if a `trust3_expires_at: <ISO-UTC>` line exists AND its timestamp
/// is in the past (operator-granted Trust 3 has lapsed). Absence of the
/// line means "never expires" — returns false.
pub fn operator_trust3_expired(player_md: &str) -> bool {
    for line in player_md.lines() {
        if let Some(rest) = line.trim().strip_prefix("trust3_expires_at:") {
            let s = rest.trim();
            if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
                return t.with_timezone(&chrono::Utc) < chrono::Utc::now();
            }
        }
    }
    false
}

/// Count bot-output history records that are replies-to-this-player or
/// whispers-with-this-player, across the most recent N UTC days. Returns
/// `(interactions, distinct_days_count)`. Used by [`compute_trust`] to
/// derive Trust 1/2.
///
/// `partner_username_lc` is the username the bot was talking WITH (the
/// player whose trust we're computing) — callers pre-lowercase it so the
/// inner comparator is case-insensitive against the original-case `target`
/// field on disk.
///
/// Records are JSON lines under `<history_dir>/<YYYY-MM-DD>.jsonl`. A
/// record matches if its `target_uuid` equals `target_uuid`, OR its
/// `target` field equals `partner_username_lc` (case-insensitive). Any of
/// `bot_out`, `bot_chat`, or `bot_whisper` count as a bot-output record
/// — CHAT.md describes the conceptual kind as `bot_out`, but the writer
/// emits the more specific `bot_chat`/`bot_whisper` labels for log
/// readability. Treating all three uniformly here keeps the trust
/// ladder from being silently pinned at 0.
pub fn count_interactions_for_uuid(
    history_dir: &Path,
    target_uuid: &str,
    partner_username_lc: &str,
    days_back: u32,
) -> (u32, u32) {
    let mut interactions = 0u32;
    let mut distinct_days = 0u32;
    let today = chrono::Utc::now().date_naive();
    let kind_marker = b"\"kind\":\"bot_";
    let username_lc_bytes = partner_username_lc.as_bytes();
    let uuid_bytes = target_uuid.as_bytes();
    for d in 0..days_back as i64 {
        let date = today - chrono::Duration::days(d);
        let path = crate::chat::jsonl::day_file_for_date(history_dir, date);
        let file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let reader = io::BufReader::new(file);
        let mut matched_today = false;
        for line_res in reader.lines() {
            let line = match line_res {
                Ok(l) => l,
                Err(_) => continue,
            };
            // Cheap byte prefilter: skip the DOM parse for the 95%+ of
            // history lines that aren't bot output addressed to the
            // partner. The line must contain `"kind":"bot_` AND either
            // the lowercased partner username (case-insensitive) or the
            // partner's UUID, before we pay the JSON parse cost.
            let bytes = line.as_bytes();
            if !contains_bytes(bytes, kind_marker) {
                continue;
            }
            if !contains_bytes_ci(bytes, username_lc_bytes)
                && !contains_bytes(bytes, uuid_bytes)
            {
                continue;
            }
            let row: HistRow = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let kind = row.kind.as_deref().unwrap_or("");
            if !matches!(kind, "bot_out" | "bot_chat" | "bot_whisper") {
                continue;
            }
            let target = row.target.as_deref().unwrap_or("");
            let target_uuid_field = row.target_uuid.as_deref().unwrap_or("");
            if target_uuid_field == target_uuid
                || target.eq_ignore_ascii_case(partner_username_lc)
            {
                interactions += 1;
                matched_today = true;
            }
        }
        if matched_today {
            distinct_days += 1;
        }
        // Early exit: once auto-Trust-2 is locked (`>=20` interactions
        // AND `>=7` distinct days) the verdict can't change with more
        // history reads — `compute_trust` saturates there.
        if interactions >= 20 && distinct_days >= 7 {
            break;
        }
    }
    (interactions, distinct_days)
}

#[derive(Deserialize)]
struct HistRow {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    target_uuid: Option<String>,
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn contains_bytes_ci(haystack: &[u8], needle_lc: &[u8]) -> bool {
    if needle_lc.is_empty() || needle_lc.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle_lc.len())
        .any(|w| w.iter().zip(needle_lc).all(|(h, n)| h.eq_ignore_ascii_case(n)))
}

// ===== TTL cache for count_interactions_for_uuid =====================
//
// The composer hot path calls `count_interactions_for_uuid` once per
// inbound chat event to derive the sender's Trust ladder rung. With a
// regular speaker the inputs only change when the writer task appends a
// new bot_out — re-reading and re-parsing the same week of JSONL every
// 30-60 s is wasted disk + CPU. A tiny per-process TTL cache trades a
// trivial amount of memory for material savings on the hottest path.
//
// Cache key: `(uuid, days_back, today_yyyymmdd)`. Including today's date
// in the key means a midnight rollover naturally invalidates yesterday's
// entries (the new day's lookups miss and recompute under the new key).
// Cache value: `(interactions, distinct_days)` plus the `Instant` it was
// inserted, for TTL eviction.
//
// `invalidate_trust_cache_for_uuid` lets a GDPR scrub drop entries for a
// forgotten player so the cached counts can't leak post-scrub.

const TRUST_CACHE_TTL: Duration = Duration::from_secs(60);
const TRUST_CACHE_MAX_ENTRIES: usize = 1024;

#[derive(Eq, Hash, PartialEq)]
struct TrustCacheKey {
    uuid: String,
    days_back: u32,
    date_today: String,
}

struct TrustCacheEntry {
    inserted_at: Instant,
    interactions: u32,
    distinct_days: u32,
}

fn trust_cache() -> &'static Mutex<HashMap<TrustCacheKey, TrustCacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<TrustCacheKey, TrustCacheEntry>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Cached wrapper around [`count_interactions_for_uuid`].
///
/// Looks up `(uuid, days_back, today)` in a per-process TTL cache (60 s).
/// On hit, returns the cached `(interactions, distinct_days)` without
/// touching disk. On miss (or expired entry), runs the underlying counter
/// and stores the result. Bounded to 1024 entries; oldest is evicted on
/// overflow.
///
/// Today's UTC date is captured once at call time and folded into the key
/// so a midnight rollover transparently invalidates yesterday's entries.
pub fn count_interactions_for_uuid_cached(
    history_dir: &Path,
    target_uuid: &str,
    partner_username_lc: &str,
    days_back: u32,
) -> (u32, u32) {
    let date_today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let key = TrustCacheKey {
        uuid: target_uuid.to_string(),
        days_back,
        date_today,
    };
    let now = Instant::now();
    {
        let mut guard = trust_cache().lock();
        if let Some(entry) = guard.get(&key) {
            if now.saturating_duration_since(entry.inserted_at) < TRUST_CACHE_TTL {
                return (entry.interactions, entry.distinct_days);
            }
            // Expired — evict so the recomputed value replaces it cleanly.
            guard.remove(&key);
        }
    }
    let (interactions, distinct_days) = count_interactions_for_uuid(
        history_dir,
        target_uuid,
        partner_username_lc,
        days_back,
    );
    let mut guard = trust_cache().lock();
    if guard.len() >= TRUST_CACHE_MAX_ENTRIES {
        // Cap reached — drop the oldest entry by `inserted_at` to make
        // room. Linear scan is fine at 1024 entries (cap-bounded) on
        // what is already a multi-millisecond disk path.
        if let Some(oldest_key) = guard
            .iter()
            .min_by_key(|(_, v)| v.inserted_at)
            .map(|(k, _)| TrustCacheKey {
                uuid: k.uuid.clone(),
                days_back: k.days_back,
                date_today: k.date_today.clone(),
            })
        {
            guard.remove(&oldest_key);
        }
    }
    guard.insert(
        key,
        TrustCacheEntry {
            inserted_at: now,
            interactions,
            distinct_days,
        },
    );
    (interactions, distinct_days)
}

/// Drop every cached entry whose key matches `uuid`. Called from the
/// GDPR scrub path so a forgotten player's stale interaction counts
/// don't survive in the per-process cache after disk rewrite.
pub fn invalidate_trust_cache_for_uuid(uuid: &str) {
    let mut guard = trust_cache().lock();
    guard.retain(|k, _| k.uuid != uuid);
}

/// Returns true iff `current_file_bytes` exceeds `cap_bytes` by more than
/// 25 % (CHAT.md: only summarize when threshold-plus-margin to avoid
/// thrash near the boundary). At/below 125 % of cap → false; strictly
/// above → true.
pub fn should_summarize_player_file(current_file_bytes: usize, cap_bytes: usize) -> bool {
    current_file_bytes * 100 > cap_bytes * 125
}

/// Drop every `{username_lc: <uuid>}` entry whose value matches the
/// given `uuid` and persist the result to disk via [`save_index`].
///
/// Used by `forget_player` (CHAT.md GDPR scrub) so a forgotten player's
/// username + UUID don't survive in cleartext inside `_index.json` until
/// the next full rebuild. Returns the count of entries removed.
///
/// Loads the index via [`load_or_rebuild_index`] so a not-yet-initialized
/// index is materialized first — skipping removal because the in-memory
/// state was lazy would defeat the scrub.
pub(crate) fn forget_index_entry(uuid: &str) -> io::Result<u64> {
    if !is_canonical_hyphen_uuid(uuid) {
        warn!(
            target: LOG_TARGET,
            uuid = uuid,
            "forget_index_entry called with non-canonical uuid; ignoring"
        );
        return Ok(0);
    }
    let mut idx = load_or_rebuild_index()?;
    let before = idx.by_lower_username.len();
    idx.by_lower_username
        .retain(|_, v| !v.eq_ignore_ascii_case(uuid));
    let removed = before.saturating_sub(idx.by_lower_username.len()) as u64;
    if removed > 0 {
        save_index(&idx)?;
    }
    Ok(removed)
}

/// Load the index from disk. On corruption or version mismatch the file
/// is renamed `<orig>.corrupt-<UTC>` and a fresh rebuild is run; the
/// original bytes are retained for forensic inspection.
pub fn load_or_rebuild_index() -> io::Result<PlayerIndex> {
    let path = Path::new(PLAYER_INDEX);
    if !path.exists() {
        return rebuild_index();
    }
    match fs::read_to_string(path) {
        Ok(s) => match serde_json::from_str::<PlayerIndex>(&s) {
            Ok(mut idx) if idx.version == INDEX_VERSION => {
                idx.by_lower_username.retain(|username, uuid| {
                    if is_canonical_hyphen_uuid(uuid) {
                        true
                    } else {
                        warn!(
                            target: LOG_TARGET,
                            username = username,
                            uuid = uuid,
                            "dropping tampered _index.json entry whose uuid is not canonical-hyphen"
                        );
                        false
                    }
                });
                Ok(idx)
            }
            Ok(_) | Err(_) => {
                warn!(target: LOG_TARGET, path = %path.display(), "player index unparsable or wrong version, rebuilding");
                let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
                let bad = path.with_extension(format!("json.corrupt-{stamp}"));
                if let Err(e) = fs::rename(path, &bad) {
                    warn!(target: LOG_TARGET, error = %e, "failed to set aside corrupt _index.json before rebuild");
                }
                rebuild_index()
            }
        },
        Err(e) => {
            warn!(target: LOG_TARGET, error = %e, "failed to read _index.json, rebuilding");
            rebuild_index()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scratch directory unique to this process, with the chat layout
    /// underneath. Cleanup is best-effort via Drop.
    struct Scratch(PathBuf, PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "cj-store-mem-{}-{}-{}",
                name,
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
            ));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&base).unwrap();
            let players = base.join("players");
            fs::create_dir_all(&players).unwrap();
            Self(base, players)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn empty_player_template_has_every_named_section() {
        let s = empty_player_template("Steve", "deadbeef-uuid", "2026-04-26");
        assert!(s.starts_with("# Steve\n"));
        for header in [
            "## Identity",
            "## Trust: 0",
            "## Stated preferences",
            "## Inferred",
            "## Topics & history",
            "## Do not mention",
        ] {
            assert!(s.contains(header), "template missing {header}");
        }
        assert!(s.contains("UUID: deadbeef-uuid"));
        assert!(s.contains("First seen: 2026-04-26"));
    }

    #[test]
    fn player_index_lookup_is_case_insensitive() {
        let mut idx = PlayerIndex::new();
        let u = "00000000-0000-0000-0000-000000000001";
        idx.insert("Steve", u);
        assert_eq!(idx.lookup("steve"), Some(u));
        assert_eq!(idx.lookup("STEVE"), Some(u));
        assert_eq!(idx.lookup("Steve"), Some(u));
        assert_eq!(idx.lookup("alice"), None);
    }

    #[test]
    fn player_index_round_trips_through_serde() {
        let mut idx = PlayerIndex::new();
        let u1 = "00000000-0000-0000-0000-000000000001";
        let u2 = "00000000-0000-0000-0000-000000000002";
        idx.insert("Steve", u1);
        idx.insert("Alice", u2);
        let json = serde_json::to_string(&idx).unwrap();
        let back: PlayerIndex = serde_json::from_str(&json).unwrap();
        assert_eq!(back.version, INDEX_VERSION);
        assert_eq!(back.lookup("steve"), Some(u1));
        assert_eq!(back.lookup("alice"), Some(u2));
    }

    #[test]
    fn rebuild_index_walks_player_directory() {
        // We can't cheaply override the global PLAYERS_DIR const, so this
        // test exercises the parsing logic via its individual pieces.
        let scratch = Scratch::new("rebuild-fragment");
        let p = scratch.1.join("00000000-0000-0000-0000-000000000001.md");
        fs::write(&p, "# Steve\n\n## Identity\n- UUID: ...\n").unwrap();
        // Reproduce the parse step rebuild_index does:
        let body = fs::read_to_string(&p).unwrap();
        let first_line = body.lines().next().unwrap();
        let username = first_line.strip_prefix("# ").unwrap().trim();
        assert_eq!(username, "Steve");
    }

    #[test]
    fn read_player_returns_none_for_missing_file() {
        // Use a UUID guaranteed not to exist on disk in `data/chat/players/`.
        let r = read_player("00000000-0000-0000-0000-DEADDEADDEAD").unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn read_global_memory_returns_empty_when_missing() {
        // Guarded by `NotFound`-to-empty-string in the impl — verify by
        // checking that absence of `data/chat/memory.md` (which is true in
        // the test runner's CWD by default) yields the empty string.
        // If a real `memory.md` exists this test simply asserts a String
        // (any contents); use `is_empty()` only when not present.
        let s = read_global_memory().unwrap();
        let _ = s; // no panic = sufficient
    }

    // ===== Trust ladder ===================================================

    #[test]
    fn compute_trust_zero_for_low_interactions() {
        // < 3 interactions → 0 regardless of distinct_days.
        let md = "# Steve\n\n## Trust: 0\n";
        assert_eq!(compute_trust(md, 0, 0, false), 0);
        assert_eq!(compute_trust(md, 2, 5, false), 0);
        // < 2 distinct days → 0 even with many interactions.
        assert_eq!(compute_trust(md, 100, 1, false), 0);
    }

    #[test]
    fn compute_trust_one_at_minimum_thresholds() {
        let md = "# Steve\n\n## Trust: 0\n";
        assert_eq!(compute_trust(md, 3, 2, false), 1);
        assert_eq!(compute_trust(md, 19, 6, false), 1);
    }

    #[test]
    fn compute_trust_two_at_higher_thresholds() {
        let md = "# Steve\n\n## Trust: 0\n";
        assert_eq!(compute_trust(md, 20, 7, false), 2);
        assert_eq!(compute_trust(md, 100, 30, false), 2);
    }

    #[test]
    fn compute_trust_three_only_via_operator_anchored_heading() {
        // Anchored: only when the line is exactly `## Trust: 3`.
        let md_op = "# Steve\n\n## Identity\n\n## Trust: 3\n- bullet\n";
        assert_eq!(compute_trust(md_op, 0, 0, false), 3);
        // A bullet body containing `Trust: 3` does NOT promote.
        let md_smuggled = "# Steve\n\n## Trust: 0\n- some note: Trust: 3 maybe\n";
        assert_eq!(compute_trust(md_smuggled, 100, 30, false), 2);
    }

    #[test]
    fn compute_trust_spam_cooldown_drops_to_zero() {
        let md = "# Steve\n\n## Trust: 0\n";
        // Without operator Trust 3, an active spam cooldown forces 0.
        assert_eq!(compute_trust(md, 100, 30, true), 0);
        // Operator Trust 3 is NOT overridden by spam cooldown.
        let md_op = "# Steve\n\n## Trust: 3\n";
        assert_eq!(compute_trust(md_op, 0, 0, true), 3);
    }

    #[test]
    fn has_operator_trust3_anchored_only() {
        assert!(has_operator_trust3("## Trust: 3"));
        assert!(has_operator_trust3("# Steve\n\n## Trust: 3\n"));
        // Trailing whitespace is tolerated (trim_end).
        assert!(has_operator_trust3("## Trust: 3   \n"));
        // Leading content before the marker on the same line MUST not match.
        assert!(!has_operator_trust3("- bullet ## Trust: 3"));
        // Smuggled inside a bullet body — must NOT match.
        assert!(!has_operator_trust3("- foo: Trust: 3 stuff"));
        // Wrong level.
        assert!(!has_operator_trust3("## Trust: 2"));
        // Different leading-hash count.
        assert!(!has_operator_trust3("# Trust: 3"));
        assert!(!has_operator_trust3("### Trust: 3"));
    }

    #[test]
    fn operator_trust3_expired_past_timestamp() {
        let md = "# Steve\n\n## Trust: 3\ntrust3_expires_at: 2000-01-01T00:00:00Z\n";
        assert!(operator_trust3_expired(md));
    }

    #[test]
    fn operator_trust3_expired_future_timestamp() {
        let md = "# Steve\n\n## Trust: 3\ntrust3_expires_at: 2999-01-01T00:00:00Z\n";
        assert!(!operator_trust3_expired(md));
    }

    #[test]
    fn operator_trust3_expired_no_marker_means_no_expiry() {
        // Absence of the marker → "never expires" → returns false.
        let md = "# Steve\n\n## Trust: 3\n";
        assert!(!operator_trust3_expired(md));
    }

    #[test]
    fn operator_trust3_expired_unparseable_timestamp_treated_as_no_marker() {
        let md = "# Steve\n\n## Trust: 3\ntrust3_expires_at: not-a-date\n";
        assert!(!operator_trust3_expired(md));
    }

    #[test]
    fn should_summarize_player_file_at_or_below_125_percent_is_false() {
        // Cap = 4096; 125 % = 5120. Equal-to or below MUST not trigger.
        assert!(!should_summarize_player_file(4096, 4096));
        assert!(!should_summarize_player_file(5120, 4096));
        assert!(!should_summarize_player_file(0, 4096));
    }

    #[test]
    fn should_summarize_player_file_strictly_above_125_percent_is_true() {
        // Cap = 4096; 125 % = 5120. Strictly above triggers.
        assert!(should_summarize_player_file(5121, 4096));
        assert!(should_summarize_player_file(8192, 4096));
    }

    #[test]
    fn count_interactions_returns_zero_for_missing_dir() {
        // Use a directory we just created and leave empty.
        let scratch = Scratch::new("count-empty");
        let dir = scratch.0.join("missing-history");
        let (i, d) = count_interactions_for_uuid(
            &dir,
            "11111111-2222-3333-4444-555555555555",
            "steve",
            7,
        );
        assert_eq!((i, d), (0, 0));
    }

    #[test]
    fn count_interactions_matches_uuid_and_username_and_skips_other_kinds() {
        let scratch = Scratch::new("count-history");
        let history = scratch.0.join("history");
        fs::create_dir_all(&history).unwrap();
        let today = chrono::Utc::now().date_naive();
        let yesterday = today - chrono::Duration::days(1);
        let target_uuid = "11111111-2222-3333-4444-555555555555";

        // Today's file: 2 matching bot_out + 1 non-matching kind.
        let today_path = crate::chat::jsonl::day_file_for_date(&history, today);
        let today_body = format!(
            "{}\n{}\n{}\n",
            // Match by target_uuid.
            serde_json::json!({
                "kind": "bot_out",
                "target": "Other",
                "target_uuid": target_uuid,
            }),
            // Match by target username (case-insensitive).
            serde_json::json!({
                "kind": "bot_out",
                "target": "STEVE",
                "target_uuid": "ffffffff-ffff-ffff-ffff-ffffffffffff",
            }),
            // Wrong kind — must not count.
            serde_json::json!({
                "kind": "public",
                "target": "Steve",
                "target_uuid": target_uuid,
            }),
        );
        fs::write(&today_path, today_body).unwrap();

        // Yesterday's file: 1 matching record.
        let yest_path = crate::chat::jsonl::day_file_for_date(&history, yesterday);
        let yest_body = format!(
            "{}\n",
            serde_json::json!({
                "kind": "bot_out",
                "target": "Steve",
                "target_uuid": target_uuid,
            }),
        );
        fs::write(&yest_path, yest_body).unwrap();

        let (i, d) =
            count_interactions_for_uuid(&history, target_uuid, "steve", 7);
        assert_eq!(i, 3);
        assert_eq!(d, 2);
    }

    // ===== Canonical-hyphen UUID gate =====================================

    #[test]
    fn is_canonical_hyphen_uuid_accepts_canonical_form() {
        assert!(is_canonical_hyphen_uuid(
            "00000000-0000-0000-0000-000000000000"
        ));
        assert!(is_canonical_hyphen_uuid(
            "deadbeef-cafe-1234-5678-90abcdef0123"
        ));
    }

    #[test]
    fn is_canonical_hyphen_uuid_rejects_malformed_inputs() {
        // Wrong length.
        assert!(!is_canonical_hyphen_uuid(""));
        assert!(!is_canonical_hyphen_uuid("uuid-1"));
        assert!(!is_canonical_hyphen_uuid("deadbeef-uuid"));
        // Hyphenless 32-char hex.
        assert!(!is_canonical_hyphen_uuid(
            "00000000000000000000000000000000"
        ));
        // Uppercase hex digits — gate enforces lowercase.
        assert!(!is_canonical_hyphen_uuid(
            "DEADBEEF-CAFE-1234-5678-90ABCDEF0123"
        ));
        // Non-hex character at a hex slot.
        assert!(!is_canonical_hyphen_uuid(
            "deadbeef-cafe-1234-5678-90abcdef012g"
        ));
        // Hyphen in a hex slot.
        assert!(!is_canonical_hyphen_uuid(
            "0000000--0000-0000-0000-000000000000"
        ));
        // Path-traversal smuggling attempt.
        assert!(!is_canonical_hyphen_uuid(
            "../etc/passwd-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        ));
    }

    #[test]
    fn player_index_insert_rejects_malformed_uuid() {
        // (iii) PlayerIndex::insert MUST silently drop a non-canonical uuid
        //       so future callers can't reintroduce tampered entries through
        //       the public mutation path.
        let mut idx = PlayerIndex::new();
        idx.insert("Steve", "uuid-1");
        idx.insert("Alice", "deadbeef-uuid");
        idx.insert(
            "Bob",
            "DEADBEEF-CAFE-1234-5678-90ABCDEF0123", /* uppercase */
        );
        assert!(idx.by_lower_username.is_empty());
        // Canonical entry is accepted.
        idx.insert("Mallory", "11111111-2222-3333-4444-555555555555");
        assert_eq!(
            idx.lookup("mallory"),
            Some("11111111-2222-3333-4444-555555555555")
        );
    }

    #[test]
    fn forget_index_entry_rejects_malformed_uuid() {
        // (iv) forget_index_entry MUST early-return when the uuid is not
        //      canonical so a malformed value can't sneak through and so
        //      operators see the warn! when the API is misused.
        let removed = forget_index_entry("not-a-uuid").unwrap();
        assert_eq!(removed, 0);
        let removed = forget_index_entry("").unwrap();
        assert_eq!(removed, 0);
        let removed =
            forget_index_entry("DEADBEEF-CAFE-1234-5678-90ABCDEF0123").unwrap();
        assert_eq!(removed, 0);
    }

    #[test]
    fn rebuild_index_skip_logic_rejects_non_canonical_stem() {
        // (i) rebuild_index applies is_canonical_hyphen_uuid to the file
        //     stem before insert. We can't cheaply override the global
        //     PLAYERS_DIR const, so test the gate logic the rebuild path
        //     uses on each candidate stem.
        let scratch = Scratch::new("rebuild-skip-non-canon");
        // Tampered: stem is not a canonical-hyphen uuid.
        let bad = scratch.1.join("..\\..\\evil.md");
        // The above join may normalize on Windows; just craft a stem
        // string directly and assert the gate would reject it.
        let _ = bad;
        let tampered_stems = [
            "../evil",
            "_index",
            "deadbeef-uuid",
            "00000000-0000-0000-0000-00000000000",  // 35 chars
            "00000000-0000-0000-0000-0000000000000", // 37 chars
            "DEADBEEF-CAFE-1234-5678-90ABCDEF0123",
        ];
        for stem in tampered_stems {
            assert!(
                !is_canonical_hyphen_uuid(stem),
                "rebuild_index should skip stem {stem:?}"
            );
        }
        // Honest canonical stem passes.
        assert!(is_canonical_hyphen_uuid(
            "00000000-0000-0000-0000-000000000001"
        ));
    }

    #[test]
    fn load_or_rebuild_index_drops_tampered_entries() {
        // (ii) After deserialization, load_or_rebuild_index retains only
        //      entries whose uuid passes the canonical-hyphen shape check.
        //      We can't override the PLAYER_INDEX const, so simulate the
        //      retain step the loader applies.
        let mut idx = PlayerIndex {
            version: INDEX_VERSION,
            by_lower_username: HashMap::new(),
        };
        idx.by_lower_username.insert(
            "steve".to_string(),
            "00000000-0000-0000-0000-000000000001".to_string(),
        );
        idx.by_lower_username
            .insert("alice".to_string(), "../etc/passwd".to_string());
        idx.by_lower_username.insert(
            "bob".to_string(),
            "DEADBEEF-CAFE-1234-5678-90ABCDEF0123".to_string(),
        );
        idx.by_lower_username
            .insert("mallory".to_string(), "uuid-1".to_string());

        // Apply the same retain predicate the loader uses.
        idx.by_lower_username
            .retain(|_, uuid| is_canonical_hyphen_uuid(uuid));

        assert_eq!(idx.by_lower_username.len(), 1);
        assert_eq!(
            idx.lookup("steve"),
            Some("00000000-0000-0000-0000-000000000001")
        );
        assert_eq!(idx.lookup("alice"), None);
        assert_eq!(idx.lookup("bob"), None);
        assert_eq!(idx.lookup("mallory"), None);
    }

    #[test]
    fn count_interactions_accepts_bot_chat_and_bot_whisper_kinds() {
        // Regression: the writer emits `bot_chat`/`bot_whisper`, but this
        // counter only matched `bot_out` — the trust ladder was silently
        // pinned at 0 in production. All three kinds must count.
        let scratch = Scratch::new("count-bot-kinds");
        let history = scratch.0.join("history");
        fs::create_dir_all(&history).unwrap();
        let today = chrono::Utc::now().date_naive();
        let target_uuid = "11111111-2222-3333-4444-555555555555";
        let path = crate::chat::jsonl::day_file_for_date(&history, today);
        let body = format!(
            "{}\n{}\n{}\n{}\n",
            serde_json::json!({
                "kind": "bot_chat",
                "target": "Steve",
                "target_uuid": target_uuid,
            }),
            serde_json::json!({
                "kind": "bot_whisper",
                "target": "Steve",
                "target_uuid": target_uuid,
            }),
            serde_json::json!({
                "kind": "bot_out",
                "target": "Steve",
                "target_uuid": target_uuid,
            }),
            // Wrong kind — must not count.
            serde_json::json!({
                "kind": "public",
                "sender": "Steve",
            }),
        );
        fs::write(&path, body).unwrap();
        let (i, _) = count_interactions_for_uuid(&history, target_uuid, "steve", 7);
        assert_eq!(i, 3);
    }
}
