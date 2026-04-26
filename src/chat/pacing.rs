//! Pacing — typing-delay computation, AI-tell stripping, post-sleep
//! recheck (PLAN §4.8).
//!
//! Pure utilities. The chat task glues these together with an actual
//! `tokio::time::sleep` between [`compute_typing_delay`] and the
//! [`recheck_after_sleep`] gate, then sends via `BotInstruction::SendChat`
//! / `Whisper`.

use std::time::Instant;

/// Outcome of [`recheck_after_sleep`] (PLAN §4.8 step 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendDecision {
    /// All gates pass — send the reply.
    Send,
    /// `max_replies_per_minute` would be exceeded if we sent now.
    DropRateLimited,
    /// Public chat: `min_silence_secs` floor not yet elapsed since the
    /// last bot send; reply is stale, drop. (Direct addresses bypass
    /// this gate per CON5; the caller is responsible for setting
    /// `direct_address` true in that case.)
    DropMinSilence,
    /// In-critical-section gate: a trade started during the typing
    /// delay; public-chat replies are dropped, whispers are deferred
    /// (caller's job — this returns the verdict only).
    DeferredCriticalSection,
}

/// Compute the typing delay for a reply, in milliseconds. The Gaussian
/// jitter is approximated by Box-Muller from a single uniform sample
/// supplied by the caller — letting tests inject a deterministic value.
///
/// `gaussian_sample` should be a `f32` from a `N(0, 1)` standard normal,
/// already drawn — caller multiplies by sigma_ms before this. Decoupling
/// the sample lets us test the clamp without an RNG dependency.
///
/// PLAN §4.8 step 4:
/// `delay = clamp(base + per_char * len + jitter, floor, max)`.
pub fn compute_typing_delay(
    reply_chars: usize,
    base_ms: u32,
    per_char_ms: u32,
    jitter_ms: i32,
    floor_ms: u32,
    max_ms: u32,
) -> u32 {
    let raw = (base_ms as i64) + (per_char_ms as i64) * (reply_chars as i64) + (jitter_ms as i64);
    let clamped = raw.max(floor_ms as i64).min(max_ms as i64);
    clamped as u32
}

/// Decide whether to send a reply after the typing-delay sleep
/// (PLAN §4.8 step 6).
///
/// `direct_address` indicates whether the reply is to a directly-
/// addressed event; CON5 exempts these from the `min_silence_secs` gate
/// so a directly-addressed reply isn't silently dropped because an
/// unrelated bot message went out during the typing delay.
///
/// `recent_replies_within_window` is the count of replies the bot has
/// sent in the trailing 60-s window (caller maintains this).
///
/// `secs_since_last_bot_send` is `None` if the bot hasn't sent anything
/// yet this session.
pub fn recheck_after_sleep(
    direct_address: bool,
    in_critical_section: bool,
    is_public_chat: bool,
    recent_replies_within_window: u32,
    max_replies_per_minute: u32,
    secs_since_last_bot_send: Option<u64>,
    min_silence_secs: u32,
) -> SendDecision {
    // Always-applies gates first.
    if recent_replies_within_window >= max_replies_per_minute {
        return SendDecision::DropRateLimited;
    }
    if in_critical_section {
        return SendDecision::DeferredCriticalSection;
    }
    // CON5 — min_silence exemption for directly-addressed replies.
    if !direct_address && is_public_chat {
        if let Some(s) = secs_since_last_bot_send
            && (s as u32) < min_silence_secs
        {
            return SendDecision::DropMinSilence;
        }
    }
    SendDecision::Send
}

/// Built-in seed AI-tells. PLAN §4.8 step 1.
///
/// Operator-managed `data/chat/strip_patterns.txt` extends this list at
/// runtime (added in Phase 8). The seed below is the always-on baseline.
pub const BUILT_IN_AI_TELLS: &[&str] = &[
    "As an AI",
    "as an AI",
    "I cannot",
    "I'm Claude",
    "I am Claude",
    "language model",
];

/// Strip AI tells, smart quotes, and em-dashes (PLAN §4.8 step 1).
///
/// This is a literal-substring strip — nothing fancy. Operators who want
/// regex matching extend `strip_patterns.txt` (Phase 8).
pub fn strip_ai_tells(reply: &str) -> String {
    let mut out = reply.to_string();
    for tell in BUILT_IN_AI_TELLS {
        // Two-pass with case variants would be tighter but produces the
        // same effect since BUILT_IN_AI_TELLS already includes both cases.
        out = out.replace(tell, "");
    }
    // Smart quotes → straight, em-dash → " - "
    out = out
        .replace('\u{201c}', "\"")
        .replace('\u{201d}', "\"")
        .replace('\u{2018}', "'")
        .replace('\u{2019}', "'")
        .replace('\u{2014}', " - ")
        .replace('\u{2013}', "-");
    out
}

/// Truncate a reply to the Minecraft chat limit (PLAN §4.8 step 2).
/// `max_chars` defaults to 240 (256 server cap with margin for the
/// username prefix).
pub fn truncate_to_chat_limit(reply: &str, max_chars: usize) -> String {
    if reply.chars().count() <= max_chars {
        return reply.to_string();
    }
    reply.chars().take(max_chars).collect()
}

/// Apply persona-driven lowercase-first-character rule (PLAN §4.8
/// step 1, last bullet).
pub fn lowercase_first_per_sentence(reply: &str) -> String {
    let mut out = String::with_capacity(reply.len());
    let mut at_sentence_start = true;
    for c in reply.chars() {
        if at_sentence_start && c.is_alphabetic() {
            for lower in c.to_lowercase() {
                out.push(lower);
            }
            at_sentence_start = false;
        } else {
            out.push(c);
            if matches!(c, '.' | '!' | '?') {
                at_sentence_start = true;
            } else if !c.is_whitespace() {
                at_sentence_start = false;
            }
        }
    }
    out
}

/// Return `Instant::now()`. Test seam — most callers thread their own
/// `Instant` through but the chat task uses this directly.
pub fn now() -> Instant {
    Instant::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- compute_typing_delay -------------------------------------------

    #[test]
    fn typing_delay_scales_linearly_with_reply_length() {
        let d10 = compute_typing_delay(10, 800, 60, 0, 400, 12_000);
        let d20 = compute_typing_delay(20, 800, 60, 0, 400, 12_000);
        // d20 - d10 should equal 10 * per_char_ms = 600.
        assert_eq!(d20 - d10, 600);
    }

    #[test]
    fn typing_delay_clamps_to_floor_with_negative_jitter() {
        // Negative jitter that would drive the result below floor must
        // clamp at floor. Jitter -1_000 with base 800 + 0 chars = -200
        // raw → clamp to 400.
        let d = compute_typing_delay(0, 800, 60, -1_000, 400, 12_000);
        assert_eq!(d, 400);
    }

    #[test]
    fn typing_delay_clamps_to_max_with_long_reply() {
        // 240 chars * 60 ms = 14 400 ms, +800 base = 15 200, well above
        // 12 000 cap.
        let d = compute_typing_delay(240, 800, 60, 0, 400, 12_000);
        assert_eq!(d, 12_000);
    }

    #[test]
    fn typing_delay_returns_floor_for_empty_reply_with_zero_base() {
        let d = compute_typing_delay(0, 0, 60, 0, 400, 12_000);
        assert_eq!(d, 400);
    }

    // ---- recheck_after_sleep --------------------------------------------

    #[test]
    fn recheck_drops_when_rate_limit_exhausted() {
        let v = recheck_after_sleep(
            false, false, true, /* recent */ 4, /* max */ 4, Some(60), 6,
        );
        assert_eq!(v, SendDecision::DropRateLimited);
    }

    #[test]
    fn recheck_defers_when_in_critical_section() {
        let v = recheck_after_sleep(true, true, true, 0, 4, Some(60), 6);
        assert_eq!(v, SendDecision::DeferredCriticalSection);
    }

    #[test]
    fn recheck_drops_min_silence_for_undirected_public_chat() {
        // 3 s since last bot send, 6 s floor, public chat, not addressed.
        let v = recheck_after_sleep(false, false, true, 0, 4, Some(3), 6);
        assert_eq!(v, SendDecision::DropMinSilence);
    }

    #[test]
    fn recheck_min_silence_does_not_apply_to_direct_address() {
        // CON5: direct addresses bypass min_silence.
        let v = recheck_after_sleep(true, false, true, 0, 4, Some(3), 6);
        assert_eq!(v, SendDecision::Send);
    }

    #[test]
    fn recheck_min_silence_does_not_apply_to_whispers() {
        // Whisper replies (`is_public_chat = false`) skip the min-silence
        // floor — that gate is for public-chat noise, not DMs.
        let v = recheck_after_sleep(false, false, false, 0, 4, Some(3), 6);
        assert_eq!(v, SendDecision::Send);
    }

    #[test]
    fn recheck_sends_when_all_gates_pass() {
        let v = recheck_after_sleep(false, false, true, 1, 4, Some(60), 6);
        assert_eq!(v, SendDecision::Send);
    }

    #[test]
    fn recheck_sends_when_no_prior_bot_send() {
        // First send of the session — None must be treated as "long
        // enough since last send" for the min-silence gate.
        let v = recheck_after_sleep(false, false, true, 0, 4, None, 6);
        assert_eq!(v, SendDecision::Send);
    }

    // ---- strip_ai_tells -------------------------------------------------

    #[test]
    fn strip_removes_as_an_ai_phrasing() {
        let s = strip_ai_tells("As an AI, I think you should sell.");
        assert!(!s.contains("As an AI"));
        // The rest of the sentence is preserved (modulo tells).
        assert!(s.contains("sell"));
    }

    #[test]
    fn strip_replaces_smart_quotes_and_em_dashes() {
        let s = strip_ai_tells("the trade \u{2014} a good one \u{2014} and \u{201c}fair\u{201d}");
        assert!(!s.contains('\u{2014}'));
        assert!(!s.contains('\u{201c}'));
        assert!(!s.contains('\u{201d}'));
        assert!(s.contains(" - "));
        assert!(s.contains("\"fair\""));
    }

    #[test]
    fn strip_is_idempotent() {
        let once = strip_ai_tells("As an AI I cannot help.");
        let twice = strip_ai_tells(&once);
        assert_eq!(once, twice);
    }

    // ---- truncate_to_chat_limit ----------------------------------------

    #[test]
    fn truncate_passes_short_replies_through() {
        assert_eq!(truncate_to_chat_limit("hello", 240), "hello");
    }

    #[test]
    fn truncate_caps_long_replies() {
        let big = "a".repeat(300);
        let s = truncate_to_chat_limit(&big, 240);
        assert_eq!(s.chars().count(), 240);
    }

    #[test]
    fn truncate_does_not_split_codepoints() {
        let big: String = std::iter::repeat_n('日', 300).collect();
        let s = truncate_to_chat_limit(&big, 100);
        assert_eq!(s.chars().count(), 100);
        // Each '日' is 3 bytes in UTF-8 — the byte count is a multiple
        // of 3, never a torn codepoint.
        assert_eq!(s.len() % 3, 0);
    }

    // ---- lowercase_first_per_sentence ----------------------------------

    #[test]
    fn lowercases_first_alpha_of_first_sentence() {
        let s = lowercase_first_per_sentence("Hello world");
        assert_eq!(s, "hello world");
    }

    #[test]
    fn lowercases_first_alpha_after_period() {
        let s = lowercase_first_per_sentence("Hello world. How are you?");
        assert_eq!(s, "hello world. how are you?");
    }

    #[test]
    fn leaves_mid_sentence_capitals_alone() {
        // Proper nouns mid-sentence are preserved; only leading char
        // per sentence is lowered.
        let s = lowercase_first_per_sentence("I went to Steve's place.");
        assert_eq!(s, "i went to Steve's place.");
    }
}
