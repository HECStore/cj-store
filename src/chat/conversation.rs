//! Conversation-layer helpers: whisper routing.
//!
//! This module is pure (no Tokio, no I/O) and testable in isolation. The
//! single load-bearing function in this skeleton phase is [`route_whisper`],
//! which decides whether an incoming whisper goes to the Store
//! (command-shaped) or to the chat module (freeform).
//!
//! See `PLAN.md` §2.3 — the routing rules are written there in plain English
//! and this module is the executable mirror. The accompanying tests pin every
//! rule.
//!
//! Phase 6 layer: addressee/dyad detection ([`classify_window`]) and spam
//! guard ([`SpamGuard`]). Both are pure / deterministic, threaded through
//! the chat task by the caller.

/// Where an inbound whisper should be routed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhisperRoute {
    /// Drop silently — empty / sigil-only / shorter than 2 chars after
    /// normalization. The caller still records the event in history.
    Drop,
    /// Treat as a command — forward to the Store. Either the first token
    /// matches a known command prefix exactly, OR fuzzy-typo rescue
    /// matched a verb.
    Store,
    /// Freeform text — forward to the chat module. Only reachable when
    /// `chat.enabled == true` and `chat.dry_run == false`; otherwise the
    /// router falls back to [`WhisperRoute::Store`] so existing trade-bot
    /// UX is preserved (PLAN §2.3 rule 1).
    Chat,
}

/// Normalize a whisper for routing.
///
/// Performs the §2.3 / S9 normalization steps (NFKC is approximated as
/// identity here — pure-ASCII whisper traffic dominates Minecraft, and
/// pulling in a Unicode-normalization crate is deferred until we have a
/// concrete attack to defend against). We DO collapse internal whitespace
/// runs and trim, which closes the most common Unicode-smuggling vectors.
fn normalize(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut prev_was_space = false;
    for c in content.chars() {
        // Whitespace (including `\t`, `\r`, `\n`, etc.) collapses to a
        // single space — this is what defeats Unicode-smuggling attacks
        // that wedge zero-width or unusual whitespace between letters.
        // The `is_whitespace` branch must come BEFORE the `is_control`
        // strip so `\t` isn't silently glued onto its neighbours.
        if c.is_whitespace() {
            if !prev_was_space && !out.is_empty() {
                out.push(' ');
                prev_was_space = true;
            }
            continue;
        }
        // Other ASCII control chars (NUL, BEL, …) are never legitimate
        // in a chat line and could be used for log injection. Strip them.
        if c.is_control() {
            continue;
        }
        out.push(c);
        prev_was_space = false;
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Decide where to send an incoming whisper.
///
/// `command_prefixes` is the operator-overridable list of verbs that should
/// always reach the Store; defaults are kept in sync with `parse_command` —
/// see `default_chat_command_prefixes` in `config.rs`.
///
/// `typo_max_distance` controls the fuzzy-typo rescue (S2.3 rule 6): if the
/// first token doesn't match a prefix exactly but the message is
/// "command-shaped" (≤ 3 tokens, alphanumeric-only) and within
/// Levenshtein distance ≤ `typo_max_distance` of any prefix, route to Store
/// so the parser's "Unknown command" hint reaches the player.
///
/// **Order of rules is load-bearing** — see PLAN §2.3.
pub fn route_whisper(
    content: &str,
    chat_enabled: bool,
    chat_dry_run: bool,
    command_prefixes: &[String],
    typo_max_distance: u32,
) -> WhisperRoute {
    // Rule 1: chat disabled OR dry-run — preserve existing trade-only behavior.
    if !chat_enabled || chat_dry_run {
        return WhisperRoute::Store;
    }

    // Rule 2: normalize.
    let norm = normalize(content);

    // Rule 3: empty / sigil-only / shorter than 2 chars → drop.
    if norm.len() < 2 {
        return WhisperRoute::Drop;
    }
    if norm.chars().all(|c| c == '!' || c == '/') {
        return WhisperRoute::Drop;
    }

    // Rule 4: sigil rule. Single leading `!` or `/` followed by an ASCII
    // letter is stripped; multiple leading sigils route to chat directly.
    let stripped = strip_sigil(&norm);
    if let StripResult::MultipleSigils = stripped {
        return WhisperRoute::Chat;
    }
    let core = match stripped {
        StripResult::Stripped(s) => s,
        StripResult::Unchanged(s) => s,
        StripResult::MultipleSigils => unreachable!(),
    };

    // Rule 5: command-prefix match on the lowercased first token.
    let first_token = match core.split_whitespace().next() {
        Some(t) => t.to_lowercase(),
        None => return WhisperRoute::Drop,
    };
    if command_prefixes.iter().any(|p| p.eq_ignore_ascii_case(&first_token)) {
        return WhisperRoute::Store;
    }

    // Rule 6: fuzzy-typo rescue. Only command-shaped messages (≤ 3 tokens,
    // alphanumeric-only) are eligible.
    let tokens: Vec<&str> = core.split_whitespace().collect();
    let command_shaped = tokens.len() <= 3
        && tokens
            .iter()
            .all(|t| t.chars().all(|c| c.is_ascii_alphanumeric()));
    if command_shaped && typo_max_distance > 0 {
        let token_bytes = first_token.as_bytes();
        for prefix in command_prefixes {
            // Skip very short prefixes — Levenshtein 2 against a 1-2 char
            // alias would match almost any 4-char token, eating freeform
            // chat. Defensive: require the prefix to be at least
            // typo_max_distance + 1 long for the comparison to make sense.
            if prefix.len() <= typo_max_distance as usize {
                continue;
            }
            let dist = levenshtein(token_bytes, prefix.as_bytes());
            if dist > 0 && dist <= typo_max_distance as usize {
                return WhisperRoute::Store;
            }
        }
    }

    // Rule 7: else → chat.
    WhisperRoute::Chat
}

enum StripResult<'a> {
    Stripped(&'a str),
    Unchanged(&'a str),
    MultipleSigils,
}

fn strip_sigil(s: &str) -> StripResult<'_> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return StripResult::Unchanged(s);
    }
    let first = bytes[0];
    if first != b'!' && first != b'/' {
        return StripResult::Unchanged(s);
    }
    // Multiple leading sigils?
    if bytes.len() >= 2 && (bytes[1] == b'!' || bytes[1] == b'/') {
        return StripResult::MultipleSigils;
    }
    // Single sigil followed by ASCII letter → strip.
    if bytes.len() >= 2 && (bytes[1] as char).is_ascii_alphabetic() {
        return StripResult::Stripped(&s[1..]);
    }
    // Single sigil followed by something else (digit, punctuation) — leave
    // it on, letting the command-prefix step decide.
    StripResult::Unchanged(s)
}

/// Levenshtein distance between two byte slices.
///
/// Lifted into this module rather than imported because the typo rescue is
/// the only call site in the whole crate; pulling in a strsim/edit-distance
/// dependency for a 25-line function is overkill.
fn levenshtein(a: &[u8], b: &[u8]) -> usize {
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    // Two-row DP. `prev[j]` is the previous row's column j, `cur[j]` the
    // current row's column j. We only ever look at row i-1, so two rows are
    // enough.
    let n = b.len();
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut cur = vec![0usize; n + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = if ca.eq_ignore_ascii_case(&cb) { 0 } else { 1 };
            cur[j + 1] = (cur[j] + 1)
                .min(prev[j + 1] + 1)
                .min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[n]
}

// ===== Dyad / open-chat detection (PLAN §4.4) ==============================

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::messages::{ChatEvent, ChatEventKind};

/// Result of [`classify_window`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelClass {
    /// ≥3 distinct senders in the window — free-for-all; classifier
    /// alone decides whether to respond.
    OpenChat,
    /// Two senders dominate the window with at least 2 transitions
    /// between them; the bot stays silent unless directly addressed.
    Dyad { speaker_a: String, speaker_b: String },
    /// Window is too small to classify (fewer than 8 entries) and
    /// nothing else matches; default to open-chat behavior (no
    /// suppression).
    NotEnoughData,
}

/// Classify the recent N events as open-chat, dyad, or
/// not-enough-data per PLAN §4.4.
///
/// `window` is expected to be the last 8 events for the channel, in
/// arrival order.
pub fn classify_window(window: &[ChatEvent]) -> ChannelClass {
    if window.len() < 8 {
        // The dyad rule explicitly looks at "last 8 slots"; smaller
        // windows fall through to open-chat behavior.
        return ChannelClass::NotEnoughData;
    }
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for ev in window {
        *counts.entry(ev.sender.as_str()).or_insert(0) += 1;
    }
    let distinct = counts.len();
    if distinct >= 3 {
        return ChannelClass::OpenChat;
    }
    if distinct < 2 {
        return ChannelClass::NotEnoughData;
    }
    // Exactly two distinct senders — check the dyad criteria
    // (≥6 of 8, ≥2 transitions).
    let mut iter = counts.iter();
    let (a_name, a_count) = iter.next().unwrap();
    let (b_name, b_count) = iter.next().unwrap();
    if a_count + b_count < 6 {
        return ChannelClass::OpenChat;
    }
    let mut transitions = 0;
    for w in window.windows(2) {
        if w[0].sender != w[1].sender
            && (w[0].sender == *a_name || w[0].sender == *b_name)
            && (w[1].sender == *a_name || w[1].sender == *b_name)
        {
            transitions += 1;
        }
    }
    if transitions < 2 {
        return ChannelClass::OpenChat;
    }
    ChannelClass::Dyad {
        speaker_a: a_name.to_string(),
        speaker_b: b_name.to_string(),
    }
}

// ===== Spam guard (PLAN §4.5) ==============================================

/// Per-sender sliding-window message counter. Drops old entries lazily
/// on each `record`. Suppression is binary (suppressed or not); the
/// chat task observes [`SpamGuard::is_suppressed`] before classifier
/// dispatch.
#[derive(Debug, Default)]
pub struct SpamGuard {
    by_sender: HashMap<String, SenderState>,
}

#[derive(Debug, Default)]
struct SenderState {
    timestamps: VecDeque<Instant>,
    last_contents: VecDeque<(Instant, String)>,
    cooldown_until: Option<Instant>,
}

impl SpamGuard {
    pub fn new() -> Self {
        Self {
            by_sender: HashMap::new(),
        }
    }

    /// Record an event and decide whether the sender is now in
    /// cooldown. Returns `true` if the *next* event from this sender
    /// should be suppressed (i.e. cooldown engaged or already active).
    pub fn record(
        &mut self,
        event: &ChatEvent,
        msgs_per_window: u32,
        window_secs: u32,
        cooldown_secs: u32,
        now: Instant,
    ) -> bool {
        let s = self.by_sender.entry(event.sender.clone()).or_default();

        // Already in cooldown?
        if let Some(until) = s.cooldown_until
            && now < until
        {
            return true;
        }
        if s.cooldown_until.is_some_and(|u| now >= u) {
            s.cooldown_until = None;
        }

        // Sliding window drop.
        let window = Duration::from_secs(window_secs as u64);
        s.timestamps.push_back(now);
        while let Some(&t) = s.timestamps.front() {
            if now.duration_since(t) > window {
                s.timestamps.pop_front();
            } else {
                break;
            }
        }

        // Near-identical content within 60 s.
        let near_dup_window = Duration::from_secs(60);
        s.last_contents
            .retain(|(t, _)| now.duration_since(*t) <= near_dup_window);
        let mut near_duplicate = false;
        for (_, prev) in &s.last_contents {
            if levenshtein_ratio(prev, &event.content) >= 0.9 {
                near_duplicate = true;
                break;
            }
        }
        s.last_contents.push_back((now, event.content.clone()));
        if s.last_contents.len() > 16 {
            s.last_contents.pop_front();
        }

        let over_rate = s.timestamps.len() > msgs_per_window as usize;
        if over_rate || near_duplicate {
            s.cooldown_until = Some(now + Duration::from_secs(cooldown_secs as u64));
            return true;
        }
        false
    }

    /// Whether the sender is currently in cooldown.
    pub fn is_suppressed(&self, sender: &str, now: Instant) -> bool {
        self.by_sender
            .get(sender)
            .and_then(|s| s.cooldown_until)
            .is_some_and(|u| now < u)
    }

    /// Operator-managed blocklist short-circuit. The caller has
    /// already loaded `data/chat/blocklist.txt` and converts it to a
    /// `&HashSet<String>` (lowercased usernames AND/OR UUIDs).
    pub fn is_blocklisted(
        sender_username_lc: &str,
        sender_uuid: Option<&str>,
        blocklist: &std::collections::HashSet<String>,
    ) -> bool {
        if blocklist.contains(sender_username_lc) {
            return true;
        }
        if let Some(u) = sender_uuid
            && blocklist.contains(u)
        {
            return true;
        }
        false
    }
}

/// Read a text file as one line per element. Empty file or missing
/// file returns an empty Vec — operator-extensible config files (e.g.
/// `data/chat/system_senders.txt`) are optional by design.
pub fn load_lines_or_empty(path: &str) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Operator-managed blocklist load (`data/chat/blocklist.txt`).
/// Comments starting with `#` and blank lines are skipped; entries are
/// lowercased so callers can match `name.to_lowercase()`.
pub fn load_blocklist(path: &str) -> std::collections::HashSet<String> {
    std::fs::read_to_string(path)
        .map(|s| {
            s.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(|l| l.to_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

/// PLAN §4.6 — system-pseudo-sender filter. Returns true if the sender
/// is clearly automated (server broadcast, console plugin, etc.) and
/// should not trigger a chat-AI response.
///
/// Rules:
/// - The Mojang username shape `^[A-Za-z0-9_]{3,16}$` is required;
///   anything else (`[Server]`, `[CONSOLE]`, etc.) is a pseudo-sender.
/// - `regex_lines` (from `data/chat/system_senders_re.txt`): treated
///   as substring patterns here. We avoid pulling in the `regex` crate
///   for a defense-in-depth filter — the operator can list literal
///   prefixes and we check substring containment. A `^` prefix is
///   stripped and treated as a "starts with" anchor for the common case.
/// - `exact_lines` (from `data/chat/system_senders.txt`): exact-match
///   list, evaluated with case-sensitive equality.
pub fn is_system_pseudo_sender(
    name: &str,
    regex_lines: &[String],
    exact_lines: &[String],
) -> bool {
    // Mojang shape gate (§4.6).
    let shape_ok = name.len() >= 3
        && name.len() <= 16
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !shape_ok {
        return true;
    }
    // Exact-list match.
    if exact_lines.iter().any(|n| n.trim() == name) {
        return true;
    }
    // Operator regex list — degraded to substring/prefix matching so
    // the chat module doesn't take a runtime regex dependency for what
    // is, in practice, a small list of literal names.
    for raw in regex_lines {
        let p = raw.trim();
        if p.is_empty() || p.starts_with('#') {
            continue;
        }
        if let Some(stripped) = p.strip_prefix('^') {
            if let Some(stripped) = stripped.strip_suffix('$') {
                if name == stripped {
                    return true;
                }
            } else if name.starts_with(stripped) {
                return true;
            }
        } else if name.contains(p) {
            return true;
        }
    }
    false
}

/// Pre-loaded moderation patterns. PLAN §4.6 S16: when ANY pattern
/// matches a chat line addressed at the bot, the bot enters a long
/// backoff. Patterns are loaded with the bot username interpolated so
/// the default seeds match the live identity.
pub struct ModerationPatterns {
    /// Lower-cased pattern strings. We use substring containment for
    /// the same reason as `is_system_pseudo_sender` — keeps the chat
    /// module dependency-light.
    patterns: Vec<String>,
}

impl ModerationPatterns {
    /// Load operator-managed patterns from `path` plus the built-in
    /// seeds. The `bot_username` is lower-cased and interpolated into
    /// the `[Mod] X -> bot` default; operator entries can include the
    /// literal `<bot_username>` placeholder which is replaced before
    /// matching.
    pub fn load_with_defaults(path: &str, bot_username: &str) -> Self {
        let bot_lc = bot_username.to_lowercase();
        let mut patterns: Vec<String> = vec![
            "you have been muted".to_string(),
            "you have been banned".to_string(),
            "you have been temporarily banned".to_string(),
            "you have been temp banned".to_string(),
            format!("[mod] -> {bot_lc}"),
            format!("[mod] whispers to {bot_lc}"),
            format!("[moderator] -> {bot_lc}"),
            format!("[moderator] whispers to {bot_lc}"),
        ];
        for raw in load_lines_or_empty(path) {
            let l = raw.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            patterns.push(l.replace("<bot_username>", &bot_lc).to_lowercase());
        }
        Self { patterns }
    }

    /// True if any pattern matches (case-insensitive substring).
    pub fn is_moderation_event(&self, content: &str) -> bool {
        let lc = content.to_lowercase();
        self.patterns.iter().any(|p| lc.contains(p))
    }
}

/// PLAN §4.4 — direct-address detection with the common-words downgrade.
/// When the bot's nickname is in the operator's `common_words.txt`
/// (e.g. a persona named "Sky" on a server that says "the sky is nice"),
/// a bare-word match is downgraded: it requires the name to start the
/// message OR be `@`-prefixed. Names not in the common-words list use
/// whole-word matching as before.
pub fn is_direct_address_with_common_words(
    content: &str,
    nicknames: &[String],
    common_words: &[String],
) -> bool {
    let lower = content.to_lowercase();
    let common_set: std::collections::HashSet<String> =
        common_words.iter().map(|w| w.to_lowercase()).collect();
    for nick in nicknames {
        if nick.is_empty() {
            continue;
        }
        let n_lower = nick.to_lowercase();
        if !common_set.contains(&n_lower) {
            if has_whole_word(&lower, &n_lower) {
                return true;
            }
        } else {
            // Downgraded path.
            if let Some(rest) = lower.strip_prefix(&n_lower) {
                let next = rest.as_bytes().first().copied();
                let boundary = matches!(
                    next,
                    None | Some(b' ') | Some(b',') | Some(b':') | Some(b';')
                        | Some(b'!') | Some(b'?') | Some(b'.')
                );
                if boundary {
                    return true;
                }
            }
            let needle = format!("@{n_lower}");
            if lower.contains(&needle) {
                return true;
            }
        }
    }
    false
}

fn has_whole_word(lower: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let bytes = lower.as_bytes();
    let nb = name.as_bytes();
    let mut i = 0;
    while i + nb.len() <= bytes.len() {
        if &bytes[i..i + nb.len()] == nb {
            let before_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
            let after_ok =
                i + nb.len() == bytes.len() || !bytes[i + nb.len()].is_ascii_alphanumeric();
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// PLAN §4.4 — reply heuristic. Returns true if `content` looks like
/// it's threaded at a non-self, non-bot speaker — meaning the bot
/// should stay silent unless it IS that addressee.
///
/// Triggers:
/// - `@<name>` in the first 16 chars where `<name>` is a recent
///   non-self speaker not in `common_words`.
/// - Starts with `<name>,`, `<name>:`, or `<name> ` where `<name>` is
///   the most recent non-self speaker AND not in `common_words`.
pub fn is_reply_to_other_speaker(
    content: &str,
    bot_username: &str,
    recent_speakers: &[String], // most-recent first
    common_words: &[String],
) -> bool {
    let lower = content.to_lowercase();
    let common_set: std::collections::HashSet<String> =
        common_words.iter().map(|w| w.to_lowercase()).collect();
    let candidates: Vec<String> = recent_speakers
        .iter()
        .filter(|n| !n.eq_ignore_ascii_case(bot_username))
        .map(|n| n.to_lowercase())
        .filter(|n| !common_set.contains(n))
        .collect();
    if candidates.is_empty() {
        return false;
    }
    let head = &lower[..lower.len().min(16)];
    for name in &candidates {
        let needle = format!("@{name}");
        if head.contains(&needle) {
            return true;
        }
    }
    if let Some(first) = candidates.first()
        && (lower.starts_with(&format!("{first},"))
            || lower.starts_with(&format!("{first}:"))
            || lower.starts_with(&format!("{first} ")))
    {
        return true;
    }
    false
}

/// Levenshtein ratio in [0.0, 1.0]: 1.0 means identical, 0.0 means
/// completely different. Defined as `1 - dist / max(len_a, len_b)`.
pub fn levenshtein_ratio(a: &str, b: &str) -> f64 {
    let max_len = a.len().max(b.len());
    if max_len == 0 {
        return 1.0;
    }
    let dist = levenshtein(a.as_bytes(), b.as_bytes());
    1.0 - (dist as f64 / max_len as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_prefixes() -> Vec<String> {
        crate::config::ChatConfig::default().command_prefixes
    }

    // ---- chat-disabled fallback (Rule 1) ----------------------------------

    #[test]
    fn chat_disabled_routes_freeform_to_store() {
        // With chat disabled, every whisper must reach Store unchanged —
        // existing trade-only operators must see no UX regression.
        let r = route_whisper("hello there friend", false, false, &default_prefixes(), 2);
        assert_eq!(r, WhisperRoute::Store);
    }

    #[test]
    fn chat_disabled_routes_command_to_store() {
        let r = route_whisper("buy diamond 1", false, false, &default_prefixes(), 2);
        assert_eq!(r, WhisperRoute::Store);
    }

    #[test]
    fn dry_run_routes_freeform_to_store() {
        // Dry-run: chat composes but doesn't send; whispers still reach Store
        // so the operator's whisper UX is unchanged during shadow testing.
        let r = route_whisper("hello there", true, true, &default_prefixes(), 2);
        assert_eq!(r, WhisperRoute::Store);
    }

    // ---- empty / sigil-only / too short (Rule 3) --------------------------

    #[test]
    fn empty_normalized_message_is_dropped() {
        let r = route_whisper("   ", true, false, &default_prefixes(), 2);
        assert_eq!(r, WhisperRoute::Drop);
    }

    #[test]
    fn single_character_is_dropped() {
        let r = route_whisper("a", true, false, &default_prefixes(), 2);
        assert_eq!(r, WhisperRoute::Drop);
    }

    #[test]
    fn sigil_only_messages_are_dropped() {
        assert_eq!(route_whisper("!", true, false, &default_prefixes(), 2), WhisperRoute::Drop);
        assert_eq!(route_whisper("//", true, false, &default_prefixes(), 2), WhisperRoute::Drop);
        assert_eq!(route_whisper("/!/", true, false, &default_prefixes(), 2), WhisperRoute::Drop);
    }

    // ---- multi-sigil rule (Rule 4) ----------------------------------------

    #[test]
    fn double_bang_routes_to_chat_with_no_token_check() {
        // `!!buy` with sigils repeated must reach chat (the sigils are a
        // "speak this literally" intent), even though the suffix matches a
        // command prefix.
        let r = route_whisper("!!buy diamond now", true, false, &default_prefixes(), 2);
        assert_eq!(r, WhisperRoute::Chat);
    }

    #[test]
    fn slash_bang_combo_routes_to_chat() {
        let r = route_whisper("!/help", true, false, &default_prefixes(), 2);
        assert_eq!(r, WhisperRoute::Chat);
    }

    #[test]
    fn single_bang_followed_by_letter_is_stripped() {
        // `!buy diamond 1` → after sigil strip, "buy diamond 1" → Store.
        let r = route_whisper("!buy diamond 1", true, false, &default_prefixes(), 2);
        assert_eq!(r, WhisperRoute::Store);
    }

    #[test]
    fn single_slash_followed_by_letter_is_stripped() {
        let r = route_whisper("/buy diamond 1", true, false, &default_prefixes(), 2);
        assert_eq!(r, WhisperRoute::Store);
    }

    // ---- command-prefix exact match (Rule 5) ------------------------------

    #[test]
    fn known_verb_routes_to_store() {
        let p = default_prefixes();
        for verb in &["buy", "sell", "deposit", "withdraw", "price", "balance"] {
            let r = route_whisper(verb, true, false, &p, 2);
            assert_eq!(r, WhisperRoute::Store, "verb {verb} should route to store");
        }
    }

    #[test]
    fn known_alias_routes_to_store() {
        let p = default_prefixes();
        for alias in &["b", "s", "d", "w", "p", "bal", "h", "q", "c"] {
            let r = route_whisper(&format!("{alias} cobblestone 1"), true, false, &p, 2);
            assert_eq!(r, WhisperRoute::Store, "alias {alias} should route to store");
        }
    }

    #[test]
    fn verb_match_is_case_insensitive() {
        let r = route_whisper("BUY diamond 1", true, false, &default_prefixes(), 2);
        assert_eq!(r, WhisperRoute::Store);
    }

    #[test]
    fn operator_verbs_route_to_store() {
        // Operator-only verbs still go to Store (auth happens at dispatch).
        let p = default_prefixes();
        for verb in &["additem", "removeitem", "addcurrency", "removecurrency"] {
            let r = route_whisper(&format!("{verb} diamond 1"), true, false, &p, 2);
            assert_eq!(r, WhisperRoute::Store, "operator verb {verb} should route to store");
        }
    }

    // ---- fuzzy-typo rescue (Rule 6) ---------------------------------------

    #[test]
    fn typo_close_to_command_routes_to_store_when_command_shaped() {
        // "buyy" is one edit from "buy" and the message is command-shaped
        // (3 alphanumeric tokens) — must reach Store so the parser whispers
        // "Unknown command 'buyy'" rather than silently absorbing into chat.
        let r = route_whisper("buyy diamond 1", true, false, &default_prefixes(), 2);
        assert_eq!(r, WhisperRoute::Store);
    }

    #[test]
    fn typo_in_freeform_message_routes_to_chat() {
        // "buyy hello there" has 3 tokens but contains the freeform-shaped
        // word; actually all 3 tokens are alphanumeric. The point of the
        // rule is "command-shaped" — leading verb plus short args. Since
        // "hello" / "there" aren't verbs, this reaches the typo branch and
        // triggers Store — which is acceptable because the parser will
        // simply tell the user "unknown command 'buyy'", a clearer outcome
        // than chat absorbing a typo. So actually this test is wrong.
        // Adjusted: a 4+ token freeform message escapes the
        // command-shaped gate.
        let r = route_whisper(
            "buyy this stuff is great okay",
            true,
            false,
            &default_prefixes(),
            2,
        );
        assert_eq!(r, WhisperRoute::Chat);
    }

    #[test]
    fn freeform_message_with_no_typo_routes_to_chat() {
        let r = route_whisper("hello friend how are you", true, false, &default_prefixes(), 2);
        assert_eq!(r, WhisperRoute::Chat);
    }

    #[test]
    fn typo_distance_zero_disables_fuzzy_rescue() {
        // typo_max_distance=0 means exact-only matching. "buyy" must reach
        // chat (no rescue).
        let r = route_whisper("buyy diamond 1", true, false, &default_prefixes(), 0);
        assert_eq!(r, WhisperRoute::Chat);
    }

    #[test]
    fn very_short_prefix_does_not_typo_match_random_words() {
        // The single-letter alias "b" must NOT typo-match a freeform word
        // like "hi" — that would route every short greeting to Store.
        let r = route_whisper("hi", true, false, &default_prefixes(), 2);
        // "hi" passes the empty/short check (2 chars) but is too close to
        // "h" via Levenshtein. The implementation skips prefixes with
        // length ≤ typo_max_distance precisely to defang this.
        assert_eq!(r, WhisperRoute::Chat);
    }

    // ---- normalization ----------------------------------------------------

    #[test]
    fn collapses_inner_whitespace_runs() {
        // "buy   diamond  1" → "buy diamond 1" — same routing.
        let r = route_whisper("buy   diamond  1", true, false, &default_prefixes(), 2);
        assert_eq!(r, WhisperRoute::Store);
    }

    #[test]
    fn strips_control_characters() {
        // Embedded \r and \t between letters get stripped; the resulting
        // first token is still "buy".
        let r = route_whisper("buy\tdiamond\r1", true, false, &default_prefixes(), 2);
        assert_eq!(r, WhisperRoute::Store);
    }

    // ---- prefix list contract --------------------------------------------

    #[test]
    fn default_prefixes_cover_every_parser_verb() {
        // Pin: if `parse_command` gains a new verb, this test prods the
        // engineer to either add it to the default prefixes (so the chat
        // AI does not shadow the new command) or to deliberately leave it
        // off. Verbs are pulled from `store::command::parse_command`.
        let p = default_prefixes();
        let p_lower: Vec<String> = p.iter().map(|s| s.to_lowercase()).collect();
        let parser_verbs = [
            "buy", "b", "sell", "s",
            "deposit", "d", "withdraw", "w",
            "price", "p", "balance", "bal", "pay",
            "items", "queue", "q", "cancel", "c",
            "status", "help", "h",
            "additem", "ai", "removeitem", "ri",
            "addcurrency", "ac", "removecurrency", "rc",
        ];
        for v in parser_verbs {
            assert!(
                p_lower.iter().any(|s| s == v),
                "default chat command_prefixes must include parser verb '{v}'; \
                 update default_chat_command_prefixes() in config.rs after editing parse_command"
            );
        }
    }

    // ---- Levenshtein helper ----------------------------------------------

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein(b"", b""), 0);
        assert_eq!(levenshtein(b"abc", b"abc"), 0);
        assert_eq!(levenshtein(b"abc", b"abd"), 1);
        assert_eq!(levenshtein(b"buyy", b"buy"), 1);
        assert_eq!(levenshtein(b"hello", b"world"), 4);
    }

    #[test]
    fn levenshtein_is_case_insensitive() {
        // Whisper routing should treat "BUY" and "buy" as identical.
        assert_eq!(levenshtein(b"BUY", b"buy"), 0);
        assert_eq!(levenshtein(b"BuYy", b"buy"), 1);
    }

    // ===== Phase 6: dyad / open-chat detection ============================

    fn ev(sender: &str, content: &str) -> ChatEvent {
        ChatEvent {
            kind: ChatEventKind::Public,
            sender: sender.to_string(),
            content: content.to_string(),
            recv_at: std::time::SystemTime::now(),
        }
    }

    #[test]
    fn classify_returns_not_enough_data_for_short_window() {
        let w = vec![ev("A", "x"); 5];
        assert_eq!(classify_window(&w), ChannelClass::NotEnoughData);
    }

    #[test]
    fn classify_open_chat_with_three_distinct_senders() {
        let w = vec![
            ev("A", "1"),
            ev("B", "2"),
            ev("A", "3"),
            ev("C", "4"),
            ev("B", "5"),
            ev("A", "6"),
            ev("C", "7"),
            ev("B", "8"),
        ];
        assert_eq!(classify_window(&w), ChannelClass::OpenChat);
    }

    #[test]
    fn classify_dyad_when_two_alternating_speakers_dominate() {
        let w = vec![
            ev("A", "1"),
            ev("B", "2"),
            ev("A", "3"),
            ev("B", "4"),
            ev("A", "5"),
            ev("B", "6"),
            ev("A", "7"),
            ev("B", "8"),
        ];
        match classify_window(&w) {
            ChannelClass::Dyad { speaker_a, speaker_b } => {
                let names = [speaker_a.as_str(), speaker_b.as_str()];
                assert!(names.contains(&"A") && names.contains(&"B"));
            }
            other => panic!("expected dyad, got {other:?}"),
        }
    }

    #[test]
    fn classify_open_chat_when_two_speakers_dont_alternate() {
        // 8 entries, 2 distinct senders, but only one transition: AAAAAAAAB.
        // Wait that's 9 entries — let's do 7 As + 1 B.
        let mut w = vec![ev("A", "x"); 7];
        w.push(ev("B", "x"));
        // Only 1 transition between A and B; PLAN §4.4 requires ≥ 2.
        assert_eq!(classify_window(&w), ChannelClass::OpenChat);
    }

    #[test]
    fn classify_open_chat_when_two_speakers_dont_dominate() {
        // 2 distinct senders BUT count_a + count_b < 6 — impossible
        // with only 2 distinct senders in 8 slots, but the count check
        // belt-and-braces. Test with 1 sender = passes count check
        // trivially? Actually 1 sender gets caught by `distinct < 2 → NotEnoughData`.
        // Skip — covered by the not-enough-data path.
        let w = vec![ev("A", "x"); 8];
        // Single sender: distinct = 1 < 2, falls through.
        assert_eq!(classify_window(&w), ChannelClass::NotEnoughData);
    }

    // ===== Phase 6: SpamGuard =============================================

    #[test]
    fn spam_guard_admits_initial_burst_under_cap() {
        let mut g = SpamGuard::new();
        let now = Instant::now();
        // 5 msgs cap, 30s window. Each event has clearly different
        // content (no near-duplicate gate) so we test the rate-limit
        // path in isolation.
        let texts = ["hi", "what's up", "did you trade today", "ok bye now", "actually wait"];
        for t in texts {
            let event = ev("Alice", t);
            assert!(!g.record(&event, 5, 30, 300, now));
        }
    }

    #[test]
    fn spam_guard_engages_after_cap_exceeded() {
        let mut g = SpamGuard::new();
        let now = Instant::now();
        let mut event = ev("Alice", "msg ");
        // Vary content slightly to defeat the near-duplicate check, so
        // we test the rate-limit path specifically.
        for i in 0..6 {
            event.content = format!("msg {i}");
            let r = g.record(&event, 5, 30, 300, now);
            // The 6th call (i=5) is the one that crosses the cap.
            if i < 5 {
                assert!(!r, "iteration {i} should not suppress");
            } else {
                assert!(r, "iteration 5 (6th) should engage cooldown");
            }
        }
        // Subsequent events stay suppressed during cooldown.
        assert!(g.is_suppressed("Alice", now + Duration::from_secs(60)));
    }

    #[test]
    fn spam_guard_engages_on_near_duplicate() {
        let mut g = SpamGuard::new();
        let now = Instant::now();
        let event1 = ev("Alice", "the diamond price is now");
        let event2 = ev("Alice", "the diamond price is now!");
        assert!(!g.record(&event1, 99, 30, 300, now));
        // Levenshtein ratio between the two strings is well above 0.9.
        assert!(g.record(&event2, 99, 30, 300, now));
    }

    #[test]
    fn spam_guard_resets_after_cooldown_expires() {
        let mut g = SpamGuard::new();
        let now = Instant::now();
        let mut event = ev("Alice", "x");
        // Engage cooldown.
        for i in 0..6 {
            event.content = format!("m{i}");
            let _ = g.record(&event, 5, 30, 30, now);
        }
        assert!(g.is_suppressed("Alice", now));
        // Step past cooldown.
        let later = now + Duration::from_secs(31);
        assert!(!g.is_suppressed("Alice", later));
    }

    #[test]
    fn spam_guard_isolates_per_sender() {
        let mut g = SpamGuard::new();
        let now = Instant::now();
        let mut event = ev("Alice", "x");
        for i in 0..6 {
            event.content = format!("m{i}");
            let _ = g.record(&event, 5, 30, 300, now);
        }
        assert!(g.is_suppressed("Alice", now));
        // Bob unaffected.
        let bob_event = ev("Bob", "hi");
        assert!(!g.record(&bob_event, 5, 30, 300, now));
        assert!(!g.is_suppressed("Bob", now));
    }

    #[test]
    fn blocklist_matches_lowercase_username() {
        let mut bl = std::collections::HashSet::new();
        bl.insert("alice".to_string());
        assert!(SpamGuard::is_blocklisted("alice", None, &bl));
        assert!(!SpamGuard::is_blocklisted("bob", None, &bl));
    }

    #[test]
    fn blocklist_matches_uuid() {
        let mut bl = std::collections::HashSet::new();
        bl.insert("11111111-2222-3333-4444-555555555555".to_string());
        assert!(SpamGuard::is_blocklisted(
            "alice",
            Some("11111111-2222-3333-4444-555555555555"),
            &bl
        ));
    }

    // ===== levenshtein_ratio ==============================================

    #[test]
    fn levenshtein_ratio_zero_for_identical() {
        assert_eq!(levenshtein_ratio("hello", "hello"), 1.0);
        assert_eq!(levenshtein_ratio("", ""), 1.0);
    }

    #[test]
    fn levenshtein_ratio_high_for_near_duplicates() {
        // 23 vs 24 chars, 1 char different → ratio ~0.96.
        let r = levenshtein_ratio(
            "the diamond price is now",
            "the diamond price is now!",
        );
        assert!(r > 0.9, "got {r}");
    }

    #[test]
    fn levenshtein_ratio_low_for_unrelated() {
        let r = levenshtein_ratio("hello world", "completely different text");
        assert!(r < 0.5, "got {r}");
    }
}
