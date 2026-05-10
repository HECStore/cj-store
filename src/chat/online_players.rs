//! In-memory roster of currently-online players.
//!
//! The bot starts with an empty roster on connect. Join broadcasts add a
//! name; leave/death broadcasts remove it; any inbound chat from a player
//! lazily re-adds them (a player who speaks is online, regardless of whether
//! we observed their join). On `BotDisconnected` the roster is cleared so
//! the next connect starts fresh.
//!
//! The roster feeds two prompt blocks:
//! - **Composer**: list of online players plus a 1-line "you know about them"
//!   excerpt pulled from each player's memory file. Lets the model decide
//!   whether to greet, address, or stay silent in a way that respects who
//!   is actually present.
//! - **Classifier**: just usernames. Cheap to inject; lets the gate refuse
//!   to reply to a player who has already left.
//!
//! No persistence — this is task-local state. A bot restart loses the
//! roster, which is fine: the next inbound event from each player rebuilds
//! it via `mark_seen`.

use std::collections::HashMap;
use std::fmt::Write;
use std::time::{Duration, Instant};

/// One entry in the online roster.
#[derive(Debug, Clone)]
pub struct OnlinePlayerInfo {
    /// Canonical username as last observed on the wire (case preserved
    /// from the most recent sighting; the map key is lowercase).
    pub username: String,
    /// When we first added this player to the roster on the current
    /// session. Reset on disconnect. Useful for operator diagnostics
    /// even when no internal caller currently reads it.
    #[allow(dead_code)]
    pub first_seen: Instant,
    /// Last time we observed this player (join, chat, etc.).
    pub last_seen: Instant,
}

/// Roster of online players, keyed by `username.to_lowercase()`.
#[derive(Debug, Default)]
pub struct OnlinePlayers {
    by_lc: HashMap<String, OnlinePlayerInfo>,
}

impl OnlinePlayers {
    pub fn new() -> Self {
        Self {
            by_lc: HashMap::new(),
        }
    }

    /// Mark a player as joined. Idempotent — re-joins refresh `last_seen`
    /// but preserve `first_seen`. Empty / non-Mojang-shaped names are
    /// rejected to keep system-sender shrapnel out of the roster.
    /// Returns `true` when the call admitted the name (whether or not
    /// it was a fresh insert); the `false` return is reserved for the
    /// validation-rejected case only, preserving the previous contract.
    pub fn mark_joined(&mut self, username: &str) -> bool {
        // `upsert` returns `Some(true)` for a fresh insert and
        // `Some(false)` for an existing-entry refresh; both are
        // legitimate joins from the caller's perspective. `None`
        // signals a validation reject (non-Mojang-shaped name).
        self.upsert(username).is_some()
    }

    /// Note that we just heard from this player (chat, whisper, etc.).
    /// If they weren't in the roster, add them — the only way to send
    /// chat is to be on the server, so a chat event proves liveness even
    /// when we missed the join broadcast (e.g. bot connected mid-session).
    pub fn mark_seen(&mut self, username: &str) {
        // Same upsert dance as `mark_joined` — we don't care whether the
        // entry was new or refreshed, just that the roster reflects the
        // sighting. Validation rejects (non-Mojang-shaped names) are
        // silently dropped, matching the prior contract.
        let _ = self.upsert(username);
    }

    /// Private upsert shared by `mark_joined` and `mark_seen` so the
    /// lc-buffer dance, the get_mut/insert branch, and the
    /// `OnlinePlayerInfo` construction live in exactly one place.
    /// Returns:
    /// - `Some(true)` when the username was newly inserted,
    /// - `Some(false)` when an existing entry was refreshed,
    /// - `None` when the username failed the Mojang-shape gate.
    ///
    /// Keeping the public API unchanged (`mark_joined -> bool`,
    /// `mark_seen -> ()`) means external callers see no semantic
    /// difference; only the internal duplication is removed.
    fn upsert(&mut self, username: &str) -> Option<bool> {
        let mut buf = [0u8; 16];
        let lc_str = ascii_lc_buf(username, &mut buf)?;
        let now = Instant::now();
        if let Some(e) = self.by_lc.get_mut(lc_str) {
            e.last_seen = now;
            if e.username != username {
                e.username = username.to_string();
            }
            Some(false)
        } else {
            self.by_lc.insert(
                String::from(lc_str),
                OnlinePlayerInfo {
                    username: username.to_string(),
                    first_seen: now,
                    last_seen: now,
                },
            );
            Some(true)
        }
    }

    /// Drop a player from the roster. Returns true if the player was
    /// present. Idempotent on a missing key.
    pub fn mark_left(&mut self, username: &str) -> bool {
        let mut buf = [0u8; 16];
        let Some(key) = ascii_lc_buf(username, &mut buf) else {
            return false;
        };
        self.by_lc.remove(key).is_some()
    }

    /// Wipe the roster. Called on `BotDisconnected` so the next session
    /// starts empty.
    pub fn clear(&mut self) {
        self.by_lc.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.by_lc.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_lc.len()
    }

    pub fn contains(&self, username: &str) -> bool {
        let mut buf = [0u8; 16];
        let Some(key) = ascii_lc_buf(username, &mut buf) else {
            return false;
        };
        self.by_lc.contains_key(key)
    }

    /// Iterate over (canonical_username, info) sorted by most-recently-seen
    /// first. Used by the proactive partner picker (when the window-based
    /// picker comes up empty).
    #[allow(dead_code)]
    pub fn iter_recent(&self) -> Vec<(&str, &OnlinePlayerInfo)> {
        let mut v: Vec<(&str, &OnlinePlayerInfo)> = self
            .by_lc
            .values()
            .map(|info| (info.username.as_str(), info))
            .collect();
        v.sort_by(|a, b| b.1.last_seen.cmp(&a.1.last_seen));
        v
    }

    /// Plain comma-separated username list, sorted by most-recently-seen.
    /// Empty roster returns `"(none)"` so prompt blocks always render
    /// non-empty text.
    pub fn format_usernames(&self) -> String {
        if self.by_lc.is_empty() {
            return "(none)".to_string();
        }
        let mut entries: Vec<&OnlinePlayerInfo> = self.by_lc.values().collect();
        entries.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        entries
            .iter()
            .map(|e| e.username.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Render the online-players block for the composer prompt. Each
    /// online player gets one line: `- <name> (seen <X>s ago)` plus an
    /// optional one-line memory excerpt supplied by the caller.
    ///
    /// `note_for` is invoked with the canonical username and may return
    /// `Some(snippet)` (a short string already trimmed and escaped) or
    /// `None` to omit the trailing note. The closure shape lets the
    /// caller pull from per-player memory files without this module
    /// taking a memory-layer dependency.
    pub fn format_for_composer(&self, note_for: impl Fn(&str) -> Option<String>) -> String {
        if self.by_lc.is_empty() {
            return "Online players: (none — server appears empty from your point of view)\n"
                .to_string();
        }
        let now = Instant::now();
        let mut entries: Vec<&OnlinePlayerInfo> = self.by_lc.values().collect();
        entries.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        let mut out = String::with_capacity(64 + entries.len() * 96);
        out.push_str("Online players (live roster — only these are currently on the server):\n");
        for e in entries {
            let secs_seen =
                now.saturating_duration_since(e.last_seen).as_secs();
            let _ = write!(out, "- {} (seen {}s ago", e.username, secs_seen);
            if let Some(note) = note_for(&e.username) {
                let trimmed = note.trim();
                if !trimmed.is_empty() {
                    out.push_str("; ");
                    out.push_str(trimmed);
                }
            }
            out.push_str(")\n");
        }
        out.push_str(
            "Do not address or assume presence of players not in this list — they have left.\n",
        );
        out
    }

    /// Drop entries we haven't observed in `max_age`. Belt-and-braces
    /// against missed leave broadcasts on flaky servers — without this,
    /// the roster could grow stale after an extended quiet period.
    pub fn prune_stale(&mut self, max_age: Duration) {
        let now = Instant::now();
        self.by_lc
            .retain(|_, info| now.saturating_duration_since(info.last_seen) <= max_age);
    }
}

/// ASCII-lowercase a Mojang-shaped name into a stack buffer and return it as
/// `&str`, or `None` if the input is not Mojang-shaped (length out of 3..=16
/// or any non-alphanumeric/underscore byte). Acts as both the gate (mirrors
/// `bot::parse_join_broadcast`'s acceptance rule so we don't admit system
/// shrapnel like `[Server]` or `+` into the roster) and the lowercase
/// projection used as the map key — letting `mark_*` and `contains` probe
/// the lowercase-keyed map without allocating a `String` per call.
fn ascii_lc_buf<'a>(s: &str, buf: &'a mut [u8; 16]) -> Option<&'a str> {
    let bytes = s.as_bytes();
    let len = bytes.len();
    if !(3..=16).contains(&len) {
        return None;
    }
    for (i, &b) in bytes.iter().enumerate() {
        if !b.is_ascii_alphanumeric() && b != b'_' {
            return None;
        }
        buf[i] = b.to_ascii_lowercase();
    }
    // SAFETY: every byte verified ASCII above; ASCII is valid UTF-8.
    std::str::from_utf8(&buf[..len]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roster_starts_empty() {
        let r = OnlinePlayers::new();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
        assert_eq!(r.format_usernames(), "(none)");
    }

    #[test]
    fn mark_joined_adds_player() {
        let mut r = OnlinePlayers::new();
        assert!(r.mark_joined("Steve"));
        assert!(r.contains("steve"));
        assert!(r.contains("STEVE"));
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn mark_joined_rejects_non_mojang_names() {
        let mut r = OnlinePlayers::new();
        assert!(!r.mark_joined("[Server]"));
        assert!(!r.mark_joined("Foo Bar"));
        assert!(!r.mark_joined("ab"));
        assert!(!r.mark_joined("a-very-long-name-that-overflows"));
        assert!(r.is_empty());
    }

    #[test]
    fn mark_left_removes_player() {
        let mut r = OnlinePlayers::new();
        r.mark_joined("Alice");
        r.mark_joined("Bob");
        assert!(r.mark_left("alice"));
        assert!(!r.contains("alice"));
        assert!(r.contains("bob"));
        assert!(!r.mark_left("alice"), "second remove is a no-op");
    }

    #[test]
    fn mark_seen_lazily_admits_unseen_player() {
        let mut r = OnlinePlayers::new();
        r.mark_seen("Charlie");
        assert!(r.contains("charlie"));
    }

    #[test]
    fn mark_joined_preserves_first_seen_on_rejoin() {
        let mut r = OnlinePlayers::new();
        r.mark_joined("Dave");
        let first = r.by_lc.get("dave").unwrap().first_seen;
        std::thread::sleep(Duration::from_millis(5));
        r.mark_joined("Dave");
        assert_eq!(r.by_lc.get("dave").unwrap().first_seen, first);
        assert!(r.by_lc.get("dave").unwrap().last_seen > first);
    }

    #[test]
    fn clear_wipes_roster() {
        let mut r = OnlinePlayers::new();
        r.mark_joined("Eve");
        r.mark_joined("Frank");
        r.clear();
        assert!(r.is_empty());
    }

    #[test]
    fn format_usernames_lists_recent_first() {
        let mut r = OnlinePlayers::new();
        r.mark_joined("Old");
        std::thread::sleep(Duration::from_millis(5));
        r.mark_joined("New");
        let s = r.format_usernames();
        assert!(s.starts_with("New"), "got: {s}");
    }

    #[test]
    fn format_for_composer_renders_block() {
        let mut r = OnlinePlayers::new();
        r.mark_joined("Steve");
        r.mark_joined("Alex");
        let block = r.format_for_composer(|name| {
            if name == "Steve" {
                Some("Trust 1; into redstone".to_string())
            } else {
                None
            }
        });
        assert!(block.contains("Steve"));
        assert!(block.contains("Alex"));
        assert!(block.contains("Trust 1; into redstone"));
        assert!(block.contains("Online players"));
    }

    #[test]
    fn format_for_composer_handles_empty_roster() {
        let r = OnlinePlayers::new();
        let block = r.format_for_composer(|_| None);
        assert!(block.contains("(none"));
    }

    #[test]
    fn prune_stale_drops_old_entries() {
        let mut r = OnlinePlayers::new();
        r.mark_joined("Stale");
        std::thread::sleep(Duration::from_millis(20));
        r.prune_stale(Duration::from_millis(10));
        assert!(r.is_empty());
    }

    #[test]
    fn iter_recent_orders_by_last_seen_descending() {
        let mut r = OnlinePlayers::new();
        r.mark_joined("First");
        std::thread::sleep(Duration::from_millis(5));
        r.mark_joined("Second");
        let v = r.iter_recent();
        assert_eq!(v[0].0, "Second");
        assert_eq!(v[1].0, "First");
    }
}
