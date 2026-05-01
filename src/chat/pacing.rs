//! Pacing — typing-delay computation, AI-tell stripping, post-sleep
//! recheck.
//!
//! Pure utilities. The chat task glues these together with an actual
//! `tokio::time::sleep` between [`compute_typing_delay`] and the
//! [`recheck_after_sleep`] gate, then sends via `BotInstruction::SendChat`
//! / `Whisper`.

/// Outcome of [`recheck_after_sleep`].
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

impl SendDecision {
    /// Stable snake_case identifier for the audit log. Exhaustive on
    /// purpose — adding a variant must be a compile error here so the
    /// reason code stays in sync with the enum.
    pub fn reason_code(&self) -> &'static str {
        match self {
            SendDecision::Send => "send",
            SendDecision::DropRateLimited => "drop_rate_limited",
            SendDecision::DropMinSilence => "drop_min_silence",
            SendDecision::DeferredCriticalSection => "deferred_critical_section",
        }
    }
}

/// Compute the typing delay for a reply, in milliseconds. The Gaussian
/// jitter is approximated by Box-Muller from a single uniform sample
/// supplied by the caller — letting tests inject a deterministic value.
///
/// `gaussian_sample` should be a `f32` from a `N(0, 1)` standard normal,
/// already drawn — caller multiplies by sigma_ms before this. Decoupling
/// the sample lets us test the clamp without an RNG dependency.
///
/// CHAT.md step 4:
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
///.
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

/// Built-in seed AI-tells. CHAT.md step 1.
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

/// XML-style reasoning-container tag names recognized by
/// [`strip_reasoning`]. Matched case-insensitively.
const REASONING_TAGS: &[&str] = &[
    "thinking",
    "think",
    "reasoning",
    "reason",
    "analysis",
    "scratchpad",
    "monologue",
];

/// Line-prefix markers (followed by `:`) that introduce a
/// reasoning/preamble line. Whole-line strip; matched case-insensitively
/// after trimming leading whitespace.
const REASONING_LINE_PREFIXES: &[&str] = &[
    "thinking:",
    "reasoning:",
    "analysis:",
    "internal:",
    "internal monologue:",
    "scratchpad:",
];

/// Strip chain-of-thought / reasoning markup from a model reply.
///
/// The composer system prompt tells the model that its output is sent
/// verbatim to chat. This is the defensive backstop: if a turn still
/// emits `<thinking>...</thinking>` or "Reasoning:" preamble, those
/// fragments are excised before the reply ever reaches a player.
///
/// Removed:
/// - `<thinking>...</thinking>` and the variants in [`REASONING_TAGS`]
///   (case-insensitive). The opening tag, closing tag, and everything
///   between them is dropped.
/// - An unclosed opening tag — content from the tag to end-of-input is
///   removed, on the assumption the model emitted reasoning and never
///   produced a real reply.
/// - Whole lines starting with a known reasoning prefix from
///   [`REASONING_LINE_PREFIXES`].
///
/// The result is trimmed; if everything was reasoning the caller's
/// "empty after stripping → silent" gate handles the empty string.
pub fn strip_reasoning(reply: &str) -> String {
    let mut out = reply.to_string();

    // Pass 1 — strip XML-style reasoning blocks. We maintain a lowered
    // view alongside `out` and edit both in lock-step; ASCII lowercasing
    // (`to_ascii_lowercase` — MUST stay this fold, NOT the locale-
    // dependent `to_lowercase` which is length-changing) is
    // byte-length preserving, so byte indices in `lower` apply to `out`
    // unchanged.
    //
    // **Code-fence skip.** A `<thinking>` token inside a backtick code
    // fence (single ` … ` or triple ``` … ``` ) is the bot legitimately
    // quoting XML markup back at a player who asked about it — not a
    // chain-of-thought leak. Each iteration finds the earliest
    // reasoning-tag occurrence outside any fence; if all matches fall
    // inside fences we're done.
    loop {
        let lower = out.to_ascii_lowercase();
        let fences = code_fence_ranges(&lower);

        // Find the earliest non-fenced opening tag across all reasoning
        // tag names. Track the close tag alongside so we can excise
        // open..close in one shot below.
        let mut earliest: Option<(usize, String, String)> = None;
        for tag in REASONING_TAGS {
            let open = format!("<{tag}>");
            let mut search_from = 0usize;
            while let Some(rel) = lower[search_from..].find(&open) {
                let pos = search_from + rel;
                if range_contains(&fences, pos) {
                    search_from = pos + open.len();
                    continue;
                }
                if earliest.as_ref().map_or(true, |(s, _, _)| pos < *s) {
                    earliest = Some((pos, open.clone(), format!("</{tag}>")));
                }
                break;
            }
        }

        let Some((start, open, close)) = earliest else { break };
        match lower[start + open.len()..].find(&close) {
            Some(rel) => {
                let end = start + open.len() + rel + close.len();
                out.replace_range(start..end, "");
                // `lower` is rebuilt at the top of the next iteration
                // from the freshly-edited `out`.
            }
            None => {
                // Unclosed reasoning block — model likely never produced
                // a real reply. Drop everything from the opening tag on.
                out.truncate(start);
                break;
            }
        }
    }

    // Pass 2 — drop whole lines that begin with a reasoning prefix.
    // Match against the line with whitespace squeezed around `:` so a
    // model emitting `Reasoning :` (space before colon) is also caught.
    let mut kept: Vec<&str> = Vec::new();
    for line in out.lines() {
        let lower = line.trim_start().to_ascii_lowercase();
        let normalized = squeeze_colon_whitespace(&lower);
        if REASONING_LINE_PREFIXES
            .iter()
            .any(|p| normalized.starts_with(p))
        {
            continue;
        }
        kept.push(line);
    }
    kept.join("\n").trim().to_string()
}

/// Compute the byte ranges occupied by code fences in `s`. Triple-tick
/// fences (```` ``` ```` … ```` ``` ````) are recognized first so the
/// inner backticks of a triple fence don't open spurious single fences.
/// Single-tick fences (`` ` `` … `` ` ``) cover the remaining ranges.
/// Both forms include the surrounding ticks so the test
/// `range_contains(start)` excludes the open marker itself.
fn code_fence_ranges(s: &str) -> Vec<(usize, usize)> {
    let bytes = s.as_bytes();
    // Pass A — triple-tick fences. Collected separately so the
    // single-tick pass can consult them without aliasing the final
    // result vector.
    let mut triples: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i + 3 <= bytes.len() {
        if &bytes[i..i + 3] == b"```" {
            let open_start = i;
            let mut j = i + 3;
            let mut found = false;
            while j + 3 <= bytes.len() {
                if &bytes[j..j + 3] == b"```" {
                    triples.push((open_start, j + 3));
                    i = j + 3;
                    found = true;
                    break;
                }
                j += 1;
            }
            if !found {
                // Unclosed triple fence — treat from the opener to EOS
                // as fenced so a `<thinking>` after a stray ``` cannot
                // be silently stripped.
                triples.push((open_start, bytes.len()));
                break;
            }
        } else {
            i += 1;
        }
    }
    // Pass B — single-tick fences outside any triple region.
    let triple_covered =
        |pos: usize| triples.iter().any(|&(a, b)| a <= pos && pos < b);
    let mut singles: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'`' && !triple_covered(i) {
            let open_start = i;
            let mut j = i + 1;
            let mut found = false;
            while j < bytes.len() {
                if bytes[j] == b'`' && !triple_covered(j) {
                    singles.push((open_start, j + 1));
                    i = j + 1;
                    found = true;
                    break;
                }
                j += 1;
            }
            if !found {
                // Unclosed single backtick — same defensive choice as
                // the triple-fence case.
                singles.push((open_start, bytes.len()));
                break;
            }
        } else {
            i += 1;
        }
    }
    triples.extend(singles);
    triples
}

/// True if `pos` lies inside any half-open `[start, end)` range.
fn range_contains(ranges: &[(usize, usize)], pos: usize) -> bool {
    ranges.iter().any(|&(a, b)| a <= pos && pos < b)
}

/// Collapse whitespace immediately before a `:` so that
/// `"reasoning :"` and `"reasoning  :"` both compare equal to
/// `"reasoning:"`. Caller-supplied lowered string; ASCII-only logic.
fn squeeze_colon_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut iter = s.chars().peekable();
    while let Some(c) = iter.next() {
        if c == ' ' || c == '\t' {
            // Look ahead: only skip whitespace runs that terminate at `:`.
            let mut peek = iter.clone();
            while let Some(&next) = peek.peek() {
                if next == ' ' || next == '\t' {
                    peek.next();
                    continue;
                }
                break;
            }
            if peek.peek() == Some(&':') {
                // Drop this whitespace; advance the real iterator past it.
                while let Some(&next) = iter.peek() {
                    if next == ' ' || next == '\t' {
                        iter.next();
                        continue;
                    }
                    break;
                }
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// Strip AI tells, smart quotes, and em-dashes.
///
/// This is a literal-substring strip — nothing fancy. Operators who want
/// regex matching extend `strip_patterns.txt` (Phase 8).
pub fn strip_ai_tells(reply: &str) -> String {
    // Unicode-normalize FIRST so that smart-apostrophe variants like
    // "I\u{2019}m Claude" collapse to the literal ASCII forms that
    // BUILT_IN_AI_TELLS matches against. None of the seed tells contain
    // smart quotes or em-dashes, so this reordering can't mask any
    // existing matches.
    let mut out = reply
        .replace('\u{201c}', "\"")
        .replace('\u{201d}', "\"")
        .replace('\u{2018}', "'")
        .replace('\u{2019}', "'")
        .replace('\u{2014}', " - ")
        .replace('\u{2013}', "-");
    for tell in BUILT_IN_AI_TELLS {
        // Two-pass with case variants would be tighter but produces the
        // same effect since BUILT_IN_AI_TELLS already includes both cases.
        out = out.replace(tell, "");
    }
    // Cleanup pass — collapse the punctuation/whitespace debris left
    // behind when a tell is excised mid-sentence. The rules below are
    // fixed points (a second pass is a no-op) so idempotence is
    // preserved.
    // 1. Collapse runs of 2+ ASCII spaces into a single space. We avoid
    //    `char::is_whitespace` here so newlines survive untouched.
    while out.contains("  ") {
        out = out.replace("  ", " ");
    }
    // 2. Collapse "<space><sentence-punct>" into the bare punctuation.
    for pair in &[" ,", " .", " ;", " :", " !", " ?"] {
        let bare = &pair[1..];
        while out.contains(pair) {
            out = out.replace(pair, bare);
        }
    }
    // 3. Trim leading runs of `,;:` plus leading ASCII whitespace.
    //    Leading `.` is preserved so an intentional "..." opener
    //    survives.
    let trimmed_start = out
        .find(|c: char| !matches!(c, ',' | ';' | ':' | ' ' | '\t' | '\r' | '\n'))
        .unwrap_or(out.len());
    if trimmed_start > 0 {
        out.drain(..trimmed_start);
    }
    out
}

/// Truncate a reply to the Minecraft chat limit.
/// `max_chars` defaults to 240 (256 server cap with margin for the
/// username prefix).
pub fn truncate_to_chat_limit(reply: &str, max_chars: usize) -> String {
    if reply.chars().count() <= max_chars {
        return reply.to_string();
    }
    reply.chars().take(max_chars).collect()
}

/// Apply persona-driven lowercase-first-character rule (CHAT.md
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

/// CHAT.md — active-hours gate. Returns true if the current UTC hour
/// is within the configured active-hours window. `None` = always on.
/// The matching helper `crate::config::within_active_hours_utc` lives
/// in config.rs so the validator can reuse it.
pub fn within_active_hours_now(active_hours_utc: Option<(u32, u32)>) -> bool {
    use chrono::Timelike;
    let hour = chrono::Utc::now().hour();
    crate::config::within_active_hours_utc(active_hours_utc, hour)
}

/// CHAT.md — Gaussian-jittered typing delay. Box-Muller transform
/// from two uniform draws on [0, 1). `u1` is clamped away from 0 to
/// keep `ln(u1)` finite. Returns a milliseconds offset (signed) — the
/// caller adds this to the deterministic base+per_char delay before
/// clamping to the floor/max. Uniform RNG is supplied by the caller so
/// tests can pin determinism.
pub fn gaussian_jitter_ms(
    mean_ms: i32,
    sigma_ms: u32,
    rng_unit: &mut impl FnMut() -> f32,
) -> i32 {
    let u1 = (rng_unit)().clamp(1e-6, 1.0);
    let u2 = (rng_unit)().clamp(0.0, 1.0);
    let z = (-2.0_f32 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos();
    mean_ms + (z * sigma_ms as f32) as i32
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

    #[test]
    fn send_decision_reason_code_maps_each_variant() {
        // Pinned snake_case audit codes — changing one is a breaking
        // log-format change and must be intentional.
        assert_eq!(SendDecision::Send.reason_code(), "send");
        assert_eq!(
            SendDecision::DropRateLimited.reason_code(),
            "drop_rate_limited"
        );
        assert_eq!(
            SendDecision::DropMinSilence.reason_code(),
            "drop_min_silence"
        );
        assert_eq!(
            SendDecision::DeferredCriticalSection.reason_code(),
            "deferred_critical_section"
        );
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

    #[test]
    fn strip_trims_leading_comma_orphan_after_tell_removal() {
        // After "As an AI" is excised the remaining ", I think..." would
        // start with a comma — the normalization pass must trim that so
        // the reply opens with "I".
        let s = strip_ai_tells("As an AI, I think this is fine.");
        assert!(
            s.starts_with('I'),
            "expected reply to start with 'I', got {s:?}"
        );
    }

    #[test]
    fn strip_collapses_mid_sentence_orphan_comma() {
        // After stripping "As an AI" the input becomes
        // "Hello. , I will help.". The " ," collapse rule pulls the
        // comma flush against the preceding period.
        let s = strip_ai_tells("Hello. As an AI, I will help.");
        assert!(
            !s.contains(" , "),
            "reply still contains ' , ' debris: {s:?}"
        );
        assert_eq!(s, "Hello., I will help.");
    }

    #[test]
    fn strip_catches_smart_apostrophe_identity_tells() {
        // Claude virtually always emits U+2019 for the apostrophe; the
        // normalization pass must run before the substring loop so that
        // "I\u{2019}m Claude" is collapsed to "I'm Claude" and stripped.
        let s = strip_ai_tells("I\u{2019}m Claude, here.");
        assert!(!s.contains("I'm Claude"));
        assert!(!s.contains("Claude"));
    }

    // ---- strip_reasoning -----------------------------------------------

    #[test]
    fn strip_reasoning_removes_thinking_block() {
        let s = strip_reasoning("<thinking>let me consider</thinking>hello");
        assert_eq!(s, "hello");
    }

    #[test]
    fn strip_reasoning_handles_mixed_case_tags() {
        let s = strip_reasoning("<Thinking>blah</Thinking>hi there");
        assert_eq!(s, "hi there");
    }

    #[test]
    fn strip_reasoning_removes_multiline_block() {
        let s = strip_reasoning("<thinking>step 1\nstep 2\nstep 3</thinking>\nactual reply");
        assert_eq!(s, "actual reply");
    }

    #[test]
    fn strip_reasoning_unclosed_tag_truncates_to_open_tag() {
        // Model produced reasoning and was cut off (max_tokens, etc.) —
        // there is no real reply to send.
        let s = strip_reasoning("preamble <thinking>started reasoning never finished");
        assert_eq!(s, "preamble");
    }

    #[test]
    fn strip_reasoning_handles_multiple_blocks() {
        let s = strip_reasoning("<thinking>a</thinking>foo<reasoning>b</reasoning>bar");
        assert_eq!(s, "foobar");
    }

    #[test]
    fn strip_reasoning_drops_reasoning_prefix_lines() {
        let input = "Reasoning: I should be friendly.\nhey what's up";
        assert_eq!(strip_reasoning(input), "hey what's up");
    }

    #[test]
    fn strip_reasoning_drops_thinking_prefix_lines_case_insensitive() {
        let input = "thinking: ok let me see\nReply text here";
        assert_eq!(strip_reasoning(input), "Reply text here");
    }

    #[test]
    fn strip_reasoning_preserves_pure_reply() {
        let input = "just a normal reply, no reasoning";
        assert_eq!(strip_reasoning(input), input);
    }

    #[test]
    fn strip_reasoning_returns_empty_when_only_reasoning() {
        let s = strip_reasoning("<thinking>only reasoning, no reply</thinking>");
        assert!(s.is_empty());
    }

    #[test]
    fn strip_reasoning_handles_think_short_tag() {
        // Some models prefer <think> over <thinking>.
        let s = strip_reasoning("<think>hmm</think>yo");
        assert_eq!(s, "yo");
    }

    #[test]
    fn strip_reasoning_is_idempotent() {
        let once = strip_reasoning("<thinking>x</thinking>Reasoning: y\nhello");
        let twice = strip_reasoning(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn strip_reasoning_preserves_thinking_inside_single_backticks() {
        // Player asks about XML; bot quotes their tag back. The
        // backtick-fenced `<thinking>...</thinking>` is content the bot
        // is INTENTIONALLY echoing — strip must leave it intact.
        let s = strip_reasoning("the `<thinking>123</thinking>` block is for cot");
        assert_eq!(s, "the `<thinking>123</thinking>` block is for cot");
    }

    #[test]
    fn strip_reasoning_preserves_thinking_inside_triple_backticks() {
        let input = "here's the syntax:\n```\n<thinking>do_thing()</thinking>\n```\nthat's it";
        assert_eq!(strip_reasoning(input), input);
    }

    #[test]
    fn strip_reasoning_strips_outside_fence_keeps_inside() {
        // Mixed: a real leak before the fence and a quoted example
        // inside. Strip the leak, keep the fenced quote.
        let input = "<thinking>scratch</thinking>here's how: `<thinking>x</thinking>`";
        assert_eq!(
            strip_reasoning(input),
            "here's how: `<thinking>x</thinking>`"
        );
    }

    #[test]
    fn strip_reasoning_handles_nested_same_tag() {
        // `<thinking>a<thinking>b</thinking>c</thinking>`. The lazy
        // close-tag match pairs the first `</thinking>` with the
        // outermost open, leaving `c</thinking>` as garbage. We
        // accept that — nested same-tag CoT is exotic — but pin
        // the behavior so future refactors notice if it changes.
        let s = strip_reasoning("hi <thinking>a<thinking>b</thinking>c</thinking> there");
        // After first removal: "hi c</thinking> there".
        // Then `</thinking>` has no matching open, so loop ends.
        // No prefix line drop applies. Final: "hi c</thinking> there".
        assert_eq!(s, "hi c</thinking> there");
    }

    #[test]
    fn strip_reasoning_drops_prefix_with_spaced_colon() {
        // Models commonly emit "Reasoning :" with a space before the colon;
        // the squeeze pass should normalize that before matching.
        let s = strip_reasoning("Reasoning : let me think\nactual reply");
        assert_eq!(s, "actual reply");
        let s2 = strip_reasoning("thinking  :   stuff\nreply");
        assert_eq!(s2, "reply");
    }

    #[test]
    fn strip_reasoning_does_not_eat_legitimate_thinking_word() {
        // Lowercase "thinking maybe..." mid-sentence is not a
        // reasoning marker — only the `Thinking:` colon-prefixed line
        // form is.
        let s = strip_reasoning("i was thinking maybe we trade");
        assert_eq!(s, "i was thinking maybe we trade");
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
