//! Classifier — fast "should I respond?" pre-filter.
//!
//! ## Two-stage design
//!
//! **Stage A — deterministic pre-classifier gate** ([`classifier_gate`]).
//! Runs before any Haiku call, on every event. Decides whether the event
//! is even worth the cheap classifier round-trip. The first review
//! estimated $180–600/mo for "classifier on every message" at realistic
//! traffic; this gate is the cost firewall.
//!
//! **Stage B — Haiku classifier call** (Phase 3, not implemented yet).
//! Lives behind the gate; produces a strict-JSON verdict
//! `{respond, confidence, reason, urgency, ai_callout: {...}}`.
//!
//! Phase 3 lands stage A and the *types* for stage B; the actual HTTP
//! call against Anthropic's API arrives once `client.rs` is built.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::config::ChatConfig;
use crate::messages::{ChatEvent, ChatEventKind};

/// Verdict from [`classifier_gate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateVerdict {
    /// Continue to the Haiku classifier.
    Classify,
    /// Skip the classifier entirely; the event silently expires.
    Skip(SkipReason),
}

/// Reason a classifier-gate skip was applied. Logged into the decision
/// JSONL alongside the event so an operator can see at a glance how
/// many events were filtered and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// Defensive self-echo guard — the event sender matches the live bot
    /// username. The pre-filter normally catches this; the gate keeps a
    /// duplicate check so a single regression upstream can't trigger a
    /// classifier call against the bot's own line.
    PreClassifierSkip,
    /// Per-call sample roll said skip on an undirected public message.
    /// Direct addresses, whispers, questions, and recent-speaker
    /// continuations bypass the sample roll.
    SampleRate,
    /// Per-sender per-minute classifier cap exhausted.
    PerSenderCap,
    /// Sender is currently spam-suppressed (closes the classifier-DoS hole).
    SpamSuppressed,
}

/// In-process classifier-call ledger keyed by sender, used by
/// [`PerSenderCounter::record_and_check`] to enforce
/// `chat.classifier_per_sender_per_minute`.
///
/// A bounded `HashMap<String, Vec<Instant>>` — entries older than 60 s
/// are pruned on every record. The map is small in practice; servers
/// with thousands of distinct speakers per minute would be unusual.
#[derive(Debug, Default)]
pub struct PerSenderCounter {
    by_sender: HashMap<String, Vec<Instant>>,
}

impl PerSenderCounter {
    pub fn new() -> Self {
        Self {
            by_sender: HashMap::new(),
        }
    }

    /// Record a classifier dispatch for `sender` and return whether the
    /// per-minute cap is now exhausted (i.e. the **next** classifier call
    /// for this sender within the same minute should be rejected).
    ///
    /// `cap` is the inclusive cap: `cap = 3` means the 3rd call within a
    /// minute is allowed but the 4th is not. Returns `true` if the call
    /// just recorded would exceed the cap and the caller should skip.
    pub fn record_and_check(&mut self, sender: &str, cap: u32, now: Instant) -> bool {
        let entries = self.by_sender.entry(sender.to_string()).or_default();
        // Drop entries older than 60 s.
        let cutoff = now - Duration::from_secs(60);
        entries.retain(|&t| t >= cutoff);
        if entries.len() >= cap as usize {
            return true;
        }
        entries.push(now);
        false
    }

    /// Number of senders tracked in the last 60 s. Operator-visible
    /// metric; surfaces in the `Chat: status` CLI command (Phase 8).
    pub fn active_senders(&self) -> usize {
        self.by_sender.values().filter(|v| !v.is_empty()).count()
    }

    /// Drop empty entries for senders that haven't shown up in 60 s.
    pub fn prune(&mut self, now: Instant) {
        let cutoff = now - Duration::from_secs(60);
        self.by_sender.retain(|_, v| {
            v.retain(|&t| t >= cutoff);
            !v.is_empty()
        });
    }
}

/// Detect a "question-shaped" message — used by the heuristic gate.
///
/// Includes the trailing `?` test AND a leading-keyword test
/// (who/what/where/when/why/how/is/are/do/does/can/will). Case-insensitive.
pub fn is_question_shaped(content: &str) -> bool {
    if content.contains('?') {
        return true;
    }
    let first = content.split_whitespace().next().unwrap_or("");
    let first_lc = first.to_lowercase();
    matches!(
        first_lc.as_str(),
        "who" | "what" | "where" | "when" | "why" | "how"
        | "is" | "are" | "do" | "does" | "can" | "will"
    )
}

/// Detect a direct address — `@<name>` prefix or a bare-word match of
/// `bot_username`/nickname (caller passes the candidate names).
///
/// `bare_word_eligible_names` is the set of names where a bare-word match
/// counts as direct address. CHAT.md calls out the dictionary downgrade
/// for bot names that are also common English words (Sky, Steve, Alex);
/// callers must filter that list before passing it in. Phase 1 callers
/// pass just the live `bot_username`; Phase 6 will broaden to nicknames
/// from `persona.md`.
pub fn is_direct_address(content: &str, bare_word_eligible_names: &[String]) -> bool {
    // `@<name>` anywhere in the first `AT_HANDLE_HEAD_BYTES` bytes, rounded
    // down to a UTF-8 char boundary so a leading emoji
    // (e.g. "🎉🎉🎉🎉🎉hi @bot…") cannot panic the chat task with a
    // mid-codepoint slice. The cap is wide enough to admit a typical
    // leading greeting ("hi everyone, ") before the `@`-handle while still
    // bounded so the function does not match `@name` arbitrarily deep into
    // long messages — preserving the "prefix-style" addressing convention
    // mirrored in `is_reply_to_other_speaker` (conversation.rs).
    const AT_HANDLE_HEAD_BYTES: usize = 32;
    let mut head_end = content.len().min(AT_HANDLE_HEAD_BYTES);
    while head_end > 0 && !content.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let head = &content[..head_end];
    if head.contains('@') {
        for name in bare_word_eligible_names {
            // Form `@name` (case-insensitive) — name must follow `@`.
            let needle = format!("@{}", name.to_lowercase());
            if head.to_lowercase().contains(&needle) {
                return true;
            }
        }
        // Generic `@name` even without exact match: be conservative and
        // treat it as ambiguous — return false so the heuristic gate's
        // OTHER signals (question, recent interaction) are required to
        // promote past the gate.
    }
    // Bare-word match: any of `bare_word_eligible_names` appears as a
    // standalone word.
    let lc = content.to_lowercase();
    for name in bare_word_eligible_names {
        let n = name.to_lowercase();
        if lc.split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|tok| tok == n)
        {
            return true;
        }
    }
    false
}

/// Decide whether to dispatch the event to the classifier.
///
/// Returns [`GateVerdict::Classify`] when:
///
/// 1. Per-sender per-minute cap not yet exceeded.
/// 2. Whisper, direct address, question, or recent-speaker continuation —
///    these bypass the sample roll. Otherwise the configured sample rate
///    decides whether undirected public chat reaches the classifier.
///
/// Why no hard heuristic skip: the classifier (Haiku) is cheap, and the
/// persona/prompt guidance handles "should I respond?" better than any
/// keyword list. Dropping every non-keyword message at the gate made the
/// bot look broken on greetings and short bare-name calls. The
/// per-sender cap + sample rate still cap classifier load.
///
/// `sample_roll` is a function that returns a uniform-random `f32` in
/// `[0.0, 1.0)` — caller-supplied so tests can use a deterministic value.
/// `sender_recent_speaker` is the caller's record of whether this sender
/// has interacted in the last `recent_speaker_secs`.
///
/// `spam_suppressed` short-circuits the gate when the sender is currently
/// in spam cooldown.
#[allow(clippy::too_many_arguments)]
pub fn classifier_gate(
    event: &ChatEvent,
    bot_username: Option<&str>,
    bare_word_eligible_names: &[String],
    sender_recent_speaker: bool,
    spam_suppressed: bool,
    config: &ChatConfig,
    counter: &mut PerSenderCounter,
    now: Instant,
    sample_roll: impl FnOnce() -> f32,
) -> GateVerdict {
    if spam_suppressed {
        return GateVerdict::Skip(SkipReason::SpamSuppressed);
    }
    // Self-echo guard — the caller is expected to filter these earlier
    //, but a defensive check here shaves a wasted classifier
    // call if the upstream filter ever regresses.
    if let Some(bot) = bot_username
        && event.sender.eq_ignore_ascii_case(bot)
    {
        return GateVerdict::Skip(SkipReason::PreClassifierSkip);
    }

    // Bot username goes into the bare-word match set unless it's
    // dictionary-conflicted; the caller is responsible for that filter
    // (passes the empty list when the name is in common-words.txt).
    // Whispers count as direct address regardless of content — a player
    // who DM'd the bot is by definition reaching out to it, even if the
    // message text is just "hi".
    let directly_addressed = event.kind == ChatEventKind::Whisper
        || is_direct_address(&event.content, bare_word_eligible_names);

    let question = is_question_shaped(&event.content);

    // Strong-signal events bypass the sample roll: direct address,
    // questions, whispers, and continuing a conversation the sender was
    // already part of. Everything else goes through the sample-rate cap
    // so undirected public chat doesn't burn the per-day classifier
    // budget on every line.
    let strong_signal = directly_addressed || question || sender_recent_speaker;

    // Per-sender cap. Direct addresses still count against it but the cap
    // is generous enough (default 3/min) that legitimate addressed-burst
    // chat is fine.
    if counter.record_and_check(&event.sender, config.classifier_per_sender_per_minute, now) {
        return GateVerdict::Skip(SkipReason::PerSenderCap);
    }

    // Sample roll — only applied to undirected public chat. The
    // classifier (cheap Haiku) gets the final "respond?" call via prompt
    // guidance.
    if !strong_signal
        && (event.kind == ChatEventKind::Public)
        && sample_roll() >= config.classifier_sample_rate
    {
        return GateVerdict::Skip(SkipReason::SampleRate);
    }

    GateVerdict::Classify
}

// ===== Stage B — actual Haiku call =========================================

use serde::Deserialize;

/// Strict-shape verdict the classifier returns. CHAT.md
#[derive(Debug, Clone, Deserialize)]
pub struct Verdict {
    pub respond: bool,
    pub confidence: f32,
    pub reason: String,
    #[serde(default)]
    pub urgency: String,
    #[serde(default)]
    pub ai_callout: Option<AiCallout>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AiCallout {
    pub detected: bool,
    // Haiku emits `null` here when `detected` is false. `#[serde(default)]`
    // alone covers a missing field but not an explicit null, so the field
    // is wrapped in `Option` to accept both shapes.
    #[serde(default)]
    pub trigger: Option<String>,
}

/// Two-block system prompt for the classifier. Returns
/// `(persona_block, adjustments_block)`. The CALLER places `cache_control:
/// ephemeral` on the *adjustments* block (block 2); persona is uncached.
///
/// Splitting the prompt this way isolates the rarely-changing persona
/// summary from the more-frequently-mutated adjustments.md, so a
/// reflection-pass write to adjustments.md doesn't invalidate the
/// persona-side prefix cache.
pub fn system_prompt_blocks(persona_summary: &str, adjustments_md: &str) -> (String, String) {
    let header = "You are a chat-classifier for a Minecraft bot. Decide whether the bot \
         should reply to the most recent message. Output STRICT JSON with this \
         shape and nothing else:\n\
         \n\
         {\n  \"respond\": <true|false>,\n  \"confidence\": <0.0-1.0>,\n  \
         \"reason\": \"<short explanation>\",\n  \"urgency\": \"<low|med|high>\",\n  \
         \"ai_callout\": {\"detected\": <true|false>, \"trigger\": \"<verbatim quote if true>\"}\n}\n\
         \n\
         Guidance:\n\
         - The bot stays in character (see persona summary). Humans don't \
           reply to everything — silence is fine when there's nothing to add.\n\
         - If the bot was itself a recent speaker in the history slice, the bot \
           is part of the conversation. Treat continuations naturally — respond \
           when the new message is a reply to or relevant to the bot's own line.\n\
         - If two OTHER players are mid-1-on-1 (the bot is not one of the two \
           recent speakers), default to staying out of it. BUT chime in when the \
           bot has something genuinely worth adding: a useful fact, a callback, \
           a relevant joke, correction, or experience. Don't interrupt for the \
           sake of interrupting — chime in only when the contribution is more \
           interesting than the silence.\n\
         - When a message contains something genuinely interesting (an unusual \
           claim, a fun topic, something the persona has an opinion on), lean \
           toward responding even without direct address.\n\
         - Greetings (\"hi\", \"hey\", \"hello\", \"yo\", \"sup\") deserve a reply \
           more often than not — even a one-word acknowledgment counts. People \
           feel ignored when they're not greeted back, and the persona is \
           someone who's *in* the chat, not a silent NPC.\n\
         - Join broadcasts arrive as messages where the content is exactly \
           \"*just joined the server*\" (literal asterisks). The sender field \
           is the joining player's name. Decide naturally — a casual \"welcome\" \
           or \"hey <name>\" is often appropriate, especially if the persona \
           knows them or the channel has been quiet, but don't greet every \
           single join if the server is bursty.\n\
         - Set ai_callout.detected = true ONLY when a player accuses the bot of \
           being AI / scripted / a robot. Quote the exact trigger.\n\
         - Confidence reflects how sure you are about responding/not — not how \
           important the topic is.\n";
    let persona = format!(
        "{header}\n=== Persona summary ===\n{persona_summary}\n",
    );
    let adjustments = format!(
        "Behavioral adjustments learned from past interactions (trusted):\n\n\
         === Adjustments (style lessons learned) ===\n{adjustments_md}\n",
    );
    (persona, adjustments)
}

/// Build the classifier request. `history_slice` is the trailing N
/// chat lines; `event` is the new message under
/// consideration. The adjustments block carries a cache-control marker
/// so the prefix is cached across calls.
///
/// The system prompt is laid out as THREE blocks:
///
/// 1. Persona summary — uncached (block boundary, no cache_control).
/// 2. Adjustments — `cache_control: ephemeral`. CHAT.md places the
///    cache breakpoint here so reflection-pass writes to adjustments.md
///    invalidate only the adjustments-onward prefix (the persona block
///    is implicitly cached by being before the breakpoint).
/// 3. Recent history slice — uncached, varies per call.
pub fn build_request(
    model: &str,
    persona_summary: &str,
    adjustments_md: &str,
    history_slice: &str,
    event: &crate::messages::ChatEvent,
    cache_ttl: crate::chat::client::CacheTtl,
) -> crate::chat::client::CreateMessageRequest {
    use crate::chat::client::{CacheControl, ContentBlock, Message, Role, SystemBlock};

    let (persona_block, adjustments_block) =
        system_prompt_blocks(persona_summary, adjustments_md);

    let system = vec![
        SystemBlock::Text {
            text: persona_block,
            cache_control: None,
        },
        SystemBlock::Text {
            text: adjustments_block,
            cache_control: Some(CacheControl::ephemeral(cache_ttl)),
        },
        SystemBlock::Text {
            text: format!("=== Recent history ===\n{history_slice}"),
            cache_control: None,
        },
    ];

    let user_text = format!(
        "Event:\nfrom: {}\ncontent: {}",
        event.sender, event.content,
    );

    crate::chat::client::CreateMessageRequest {
        model: model.to_string(),
        max_tokens: 256,
        system,
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: user_text,
                cache_control: None,
            }],
        }],
        temperature: None,
        tools: vec![],
    }
}

/// Parse the classifier's text response into a [`Verdict`]. The model
/// may emit text outside the JSON block (despite instruction); we
/// extract the first balanced `{...}` block we see and parse that.
pub fn parse_verdict(text: &str) -> Result<Verdict, String> {
    // Find first '{', then scan to the matching '}' tracking string
    // literals. Robust enough for Haiku's typical output.
    let bytes = text.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{')
        .ok_or_else(|| "no '{' in classifier output".to_string())?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    let mut end = None;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end.ok_or_else(|| "unbalanced JSON in classifier output".to_string())?;
    let json = &text[start..end];
    serde_json::from_str::<Verdict>(json)
        .map_err(|e| format!("classifier verdict parse failed: {e}"))
}

/// Append a pending-adjustment entry to `data/chat/pending_adjustments.jsonl`.
/// CHAT.mdrrors are logged but never raised — pending writes are
/// best-effort and recoverable from history.
pub fn write_pending_adjustment(
    trigger: &str,
    sender: &str,
    sender_uuid: Option<&str>,
) {
    use std::fs::OpenOptions;
    use std::io::Write;
    // Single `now()` so `ts` and `observed_day_utc` cannot disagree
    // across a midnight-UTC tick.
    let now = chrono::Utc::now();
    let entry = crate::chat::reflection::PendingEntry {
        ts: now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        trigger: trigger.to_string(),
        sender: sender.to_string(),
        sender_uuid: sender_uuid.map(str::to_string),
        observed_day_utc: now.format("%Y-%m-%d").to_string(),
    };
    let line = match serde_json::to_string(&entry) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "pending adjustment serialize failed");
            return;
        }
    };
    let path = std::path::Path::new("data/chat/pending_adjustments.jsonl");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(mut f) => {
            if let Err(e) = writeln!(f, "{line}") {
                tracing::error!(error = %e, "pending adjustment append failed");
            }
        }
        Err(e) => tracing::error!(error = %e, "pending adjustment open failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn ev(content: &str, sender: &str) -> ChatEvent {
        ChatEvent {
            kind: ChatEventKind::Public,
            sender: sender.to_string(),
            content: content.to_string(),
            recv_at: SystemTime::now(),
        }
    }

    fn cfg() -> ChatConfig {
        ChatConfig {
            enabled: true,
            ..ChatConfig::default()
        }
    }

    fn always_pass() -> f32 {
        0.0
    }
    fn always_skip() -> f32 {
        0.99
    }

    // ---- is_question_shaped ----------------------------------------------

    #[test]
    fn question_mark_anywhere_counts_as_question() {
        assert!(is_question_shaped("anyone home?"));
        assert!(is_question_shaped("hey?how's it"));
    }

    #[test]
    fn leading_question_word_counts_as_question() {
        for w in [
            "who", "what", "where", "when", "why", "how", "is", "are", "do",
            "does", "can", "will",
        ] {
            assert!(is_question_shaped(&format!("{w} something")), "{w}");
        }
    }

    #[test]
    fn case_insensitive_question_word() {
        assert!(is_question_shaped("WHO is there"));
        assert!(is_question_shaped("Where to next"));
    }

    #[test]
    fn declarative_message_is_not_question_shaped() {
        assert!(!is_question_shaped("the diamonds are great"));
        assert!(!is_question_shaped("just chilling"));
    }

    // ---- is_direct_address ------------------------------------------------

    #[test]
    fn at_prefix_with_bot_name_counts_as_direct() {
        let names = vec!["TradeBot".to_string()];
        assert!(is_direct_address("@TradeBot you online", &names));
        assert!(is_direct_address("@tradebot you online", &names));
    }

    #[test]
    fn bare_word_with_bot_name_counts_as_direct() {
        let names = vec!["TradeBot".to_string()];
        assert!(is_direct_address("Hey TradeBot, how are you", &names));
        assert!(is_direct_address("hey tradebot how are you", &names));
    }

    #[test]
    fn bot_name_inside_a_word_does_not_count() {
        // Substring match across word boundaries is a regression we want
        // to never have — `TradeBot` must not match inside `"tradeboth"`.
        let names = vec!["TradeBot".to_string()];
        assert!(!is_direct_address("tradeboth options please", &names));
    }

    #[test]
    fn multibyte_prefix_does_not_panic_at_byte_16() {
        // Regression: head was sliced as `&content[..16]`. With a leading
        // emoji a codepoint can straddle byte 16 and panic the chat task.
        // 5 × "🎉" (4 bytes each = 20 bytes) followed by an at-handle —
        // byte index 16 falls mid-codepoint of the 5th emoji.
        let names = vec!["TradeBot".to_string()];
        let _ = is_direct_address("🎉🎉🎉🎉🎉hello @TradeBot", &names);
        // Also exercise a case where the @-handle lives inside the first
        // 16 bytes once a multi-byte char is present.
        assert!(is_direct_address("é @TradeBot hi there", &names));
    }

    #[test]
    fn at_handle_within_head_bound_matches() {
        // Regression: a leading greeting like "hi everyone, " pushes the
        // `@`-handle past byte 16 but still within the widened
        // AT_HANDLE_HEAD_BYTES cap (32). The cap encodes a "prefix-style
        // addressing" intent — the @-handle should still register as a
        // direct address when it follows a normal-length greeting.
        let names = vec!["TradeBot".to_string()];
        assert!(is_direct_address(
            "hi everyone, are you online @TradeBot please?",
            &names,
        ));
    }

    #[test]
    fn empty_name_list_means_no_direct_address() {
        // Caller passes an empty list when the bot name is dictionary-
        // conflicted; no message can register as direct address.
        assert!(!is_direct_address("Hey Sky, look", &[]));
        assert!(!is_direct_address("@Sky look", &[]));
    }

    // ---- PerSenderCounter -------------------------------------------------

    #[test]
    fn per_sender_counter_caps_at_configured_rate() {
        let mut c = PerSenderCounter::new();
        let now = Instant::now();
        // Cap = 2: 1st & 2nd calls allowed, 3rd skipped.
        assert!(!c.record_and_check("Alice", 2, now));
        assert!(!c.record_and_check("Alice", 2, now));
        assert!(c.record_and_check("Alice", 2, now));
    }

    #[test]
    fn per_sender_counter_isolates_per_sender() {
        // Alice exhausting her quota does not bleed into Bob.
        let mut c = PerSenderCounter::new();
        let now = Instant::now();
        for _ in 0..5 {
            let _ = c.record_and_check("Alice", 2, now);
        }
        assert!(!c.record_and_check("Bob", 2, now));
    }

    #[test]
    fn per_sender_counter_window_slides_after_60s() {
        // Entries older than 60 s are pruned on each record. Past cap
        // resets once we step beyond the window.
        let mut c = PerSenderCounter::new();
        let t0 = Instant::now();
        let _ = c.record_and_check("Alice", 1, t0);
        // Immediately at-cap.
        assert!(c.record_and_check("Alice", 1, t0));
        // Step past the 60-s window.
        let t_later = t0 + Duration::from_secs(61);
        // Counter sees the prior call as expired and admits the new one.
        assert!(!c.record_and_check("Alice", 1, t_later));
    }

    // ---- classifier_gate --------------------------------------------------

    #[test]
    fn spam_suppressed_short_circuits_to_skip() {
        let event = ev("hi there", "Alice");
        let mut counter = PerSenderCounter::new();
        let v = classifier_gate(
            &event,
            Some("TradeBot"),
            &[],
            false,
            true, // spam suppressed
            &cfg(),
            &mut counter,
            Instant::now(),
            always_pass,
        );
        assert_eq!(v, GateVerdict::Skip(SkipReason::SpamSuppressed));
    }

    #[test]
    fn self_echo_short_circuits_to_skip() {
        // Defensive duplicate of self-echo filter.
        let event = ev("any message", "TradeBot");
        let mut counter = PerSenderCounter::new();
        let v = classifier_gate(
            &event,
            Some("TradeBot"),
            &[],
            false,
            false,
            &cfg(),
            &mut counter,
            Instant::now(),
            always_pass,
        );
        assert_eq!(v, GateVerdict::Skip(SkipReason::PreClassifierSkip));
    }

    #[test]
    fn no_signal_message_classifies_when_sample_roll_passes() {
        // No question, no direct address, no recent interaction. Under the
        // softer gate the sample roll is what gates undirected public chat
        // — when it passes, the message reaches Haiku so the
        // persona-driven prompt can decide whether to greet / chime in.
        let event = ev("the weather is nice today", "Alice");
        let mut counter = PerSenderCounter::new();
        let v = classifier_gate(
            &event,
            Some("TradeBot"),
            &[], // no eligible names
            false, // not a recent speaker
            false,
            &cfg(),
            &mut counter,
            Instant::now(),
            always_pass,
        );
        assert_eq!(v, GateVerdict::Classify);
    }

    #[test]
    fn no_signal_message_skipped_when_sample_says_skip() {
        // The sample roll is now the *only* cost firewall for undirected
        // public chat. When it fails, the gate must still skip with the
        // sample-rate reason (not the old PreClassifierSkip).
        let event = ev("the weather is nice today", "Alice");
        let mut counter = PerSenderCounter::new();
        let v = classifier_gate(
            &event,
            Some("TradeBot"),
            &[],
            false,
            false,
            &cfg(),
            &mut counter,
            Instant::now(),
            always_skip,
        );
        assert_eq!(v, GateVerdict::Skip(SkipReason::SampleRate));
    }

    #[test]
    fn bare_greeting_reaches_classifier_when_sample_passes() {
        // Regression: a bare "hi" from a non-recent speaker used to be
        // dropped at the heuristic gate. Now it must reach Haiku so the
        // persona can decide whether to greet back.
        let event = ev("hi", "Alice");
        let mut counter = PerSenderCounter::new();
        let v = classifier_gate(
            &event,
            Some("TradeBot"),
            &[],
            false,
            false,
            &cfg(),
            &mut counter,
            Instant::now(),
            always_pass,
        );
        assert_eq!(v, GateVerdict::Classify);
    }

    #[test]
    fn directly_addressed_passes_gate_even_under_sample_skip() {
        let event = ev("@TradeBot you there?", "Alice");
        let mut counter = PerSenderCounter::new();
        let v = classifier_gate(
            &event,
            Some("TradeBot"),
            &["TradeBot".to_string()],
            false,
            false,
            &cfg(),
            &mut counter,
            Instant::now(),
            always_skip,
        );
        // Sample roll is "skip" but direct address bypasses the sample.
        assert_eq!(v, GateVerdict::Classify);
    }

    #[test]
    fn question_message_passes_gate_when_sample_passes() {
        let event = ev("anyone selling diamonds?", "Alice");
        let mut counter = PerSenderCounter::new();
        let v = classifier_gate(
            &event,
            Some("TradeBot"),
            &["TradeBot".to_string()],
            false,
            false,
            &cfg(),
            &mut counter,
            Instant::now(),
            always_pass,
        );
        assert_eq!(v, GateVerdict::Classify);
    }

    #[test]
    fn question_bypasses_sample_roll() {
        // Questions are strong-signal and reach the classifier even when
        // the sample roll says skip. This is the engagement-tuning fix:
        // dropping a clear "anyone selling diamonds?" because of a random
        // sample miss made the bot look broken on perfectly reasonable
        // chat.
        let event = ev("anyone selling diamonds?", "Alice");
        let mut counter = PerSenderCounter::new();
        let v = classifier_gate(
            &event,
            Some("TradeBot"),
            &["TradeBot".to_string()],
            false,
            false,
            &cfg(),
            &mut counter,
            Instant::now(),
            always_skip,
        );
        assert_eq!(v, GateVerdict::Classify);
    }

    #[test]
    fn recent_speaker_passes_heuristic() {
        let event = ev("yeah whatever", "Alice");
        let mut counter = PerSenderCounter::new();
        let v = classifier_gate(
            &event,
            Some("TradeBot"),
            &["TradeBot".to_string()],
            true, // recent speaker
            false,
            &cfg(),
            &mut counter,
            Instant::now(),
            always_pass,
        );
        assert_eq!(v, GateVerdict::Classify);
    }

    #[test]
    fn per_sender_cap_skip_takes_precedence_over_sample() {
        // Pre-load Alice past her cap.
        let mut counter = PerSenderCounter::new();
        let now = Instant::now();
        let cfg_low = ChatConfig {
            enabled: true,
            classifier_per_sender_per_minute: 1,
            ..ChatConfig::default()
        };
        // First call recorded.
        let event = ev("@TradeBot still online?", "Alice");
        let _ = classifier_gate(
            &event,
            Some("TradeBot"),
            &["TradeBot".to_string()],
            false,
            false,
            &cfg_low,
            &mut counter,
            now,
            always_pass,
        );
        // Second call — over cap.
        let v = classifier_gate(
            &event,
            Some("TradeBot"),
            &["TradeBot".to_string()],
            false,
            false,
            &cfg_low,
            &mut counter,
            now,
            always_pass,
        );
        assert_eq!(v, GateVerdict::Skip(SkipReason::PerSenderCap));
    }

    // ---- parse_verdict --------------------------------------------------

    #[test]
    fn parse_verdict_parses_strict_json() {
        let raw = r#"{"respond": true, "confidence": 0.82, "reason": "directly addressed", "urgency": "med", "ai_callout": {"detected": false, "trigger": ""}}"#;
        let v = parse_verdict(raw).unwrap();
        assert!(v.respond);
        assert!((v.confidence - 0.82).abs() < 1e-6);
        assert_eq!(v.urgency, "med");
        let ac = v.ai_callout.unwrap();
        assert!(!ac.detected);
    }

    #[test]
    fn parse_verdict_handles_text_around_json() {
        // Haiku sometimes emits a leading sentence even when told not to.
        let raw = "Sure: {\"respond\": false, \"confidence\": 0.4, \"reason\": \"low value\"} done";
        let v = parse_verdict(raw).unwrap();
        assert!(!v.respond);
        assert!(v.ai_callout.is_none());
    }

    #[test]
    fn parse_verdict_handles_braces_inside_strings() {
        // String content may contain literal `}` which must not close
        // the outer brace prematurely.
        let raw = r#"{"respond": false, "confidence": 0.1, "reason": "user said {hi}"}"#;
        let v = parse_verdict(raw).unwrap();
        assert!(!v.respond);
        assert_eq!(v.reason, "user said {hi}");
    }

    #[test]
    fn parse_verdict_ai_callout_carries_trigger_when_detected() {
        let raw = r#"{"respond": true, "confidence": 0.9, "reason": "callout", "urgency": "high", "ai_callout": {"detected": true, "trigger": "you sound like a bot"}}"#;
        let v = parse_verdict(raw).unwrap();
        let ac = v.ai_callout.unwrap();
        assert!(ac.detected);
        assert_eq!(ac.trigger.as_deref(), Some("you sound like a bot"));
    }

    #[test]
    fn parse_verdict_accepts_null_trigger_when_not_detected() {
        // Haiku emits `"trigger": null` instead of `""` when detected=false.
        let raw = r#"{"respond": false, "confidence": 0.5, "reason": "n/a", "urgency": "low", "ai_callout": {"detected": false, "trigger": null}}"#;
        let v = parse_verdict(raw).unwrap();
        let ac = v.ai_callout.unwrap();
        assert!(!ac.detected);
        assert!(ac.trigger.is_none());
    }

    #[test]
    fn parse_verdict_rejects_no_json() {
        let r = parse_verdict("just plain text");
        assert!(r.is_err());
    }

    // ---- build_request --------------------------------------------------

    #[test]
    fn build_request_caches_adjustments_block() {
        let event = ev("hi", "Alice");
        let req = build_request(
            "claude-haiku-4-5-20251001",
            "persona summary text",
            "no adjustments yet",
            "recent: hi from Alice",
            &event,
            crate::chat::client::CacheTtl::Ephemeral1Hour,
        );
        // Three blocks: persona (uncached), adjustments (cached),
        // history (uncached).
        assert_eq!(req.system.len(), 3);
        match &req.system[0] {
            crate::chat::client::SystemBlock::Text { cache_control, .. } => {
                assert!(cache_control.is_none(), "persona block must be uncached");
            }
        }
        match &req.system[1] {
            crate::chat::client::SystemBlock::Text { cache_control, .. } => {
                assert!(cache_control.is_some(), "adjustments block must be cached");
            }
        }
        match &req.system[2] {
            crate::chat::client::SystemBlock::Text { cache_control, .. } => {
                assert!(cache_control.is_none(), "history slice must be uncached");
            }
        }
    }

    #[test]
    fn classifier_two_blocks_have_cache_control_only_on_adjustments() {
        // CHAT.md: persona and adjustments are TWO distinct system
        // blocks, with the cache breakpoint placed on the adjustments
        // block alone.
        let event = ev("hi", "Alice");
        let req = build_request(
            "claude-haiku-4-5-20251001",
            "PERSONA_SUMMARY_MARKER",
            "ADJUSTMENTS_MARKER",
            "history",
            &event,
            crate::chat::client::CacheTtl::Ephemeral5Min,
        );
        // Verify the first two blocks carry the persona and adjustments
        // texts respectively, and that ONLY block 2 is cached.
        match &req.system[0] {
            crate::chat::client::SystemBlock::Text { text, cache_control } => {
                assert!(text.contains("PERSONA_SUMMARY_MARKER"));
                assert!(!text.contains("ADJUSTMENTS_MARKER"));
                assert!(cache_control.is_none());
            }
        }
        match &req.system[1] {
            crate::chat::client::SystemBlock::Text { text, cache_control } => {
                assert!(text.contains("ADJUSTMENTS_MARKER"));
                assert!(!text.contains("PERSONA_SUMMARY_MARKER"));
                assert!(cache_control.is_some());
            }
        }
    }

    #[test]
    fn whisper_event_bypasses_sample_rate_gate() {
        // CHAT.md: sample rate applies to public-chat events; whispers
        // are always classifier-evaluated.
        let mut event = ev("hi friend", "Alice");
        event.kind = ChatEventKind::Whisper;
        let mut counter = PerSenderCounter::new();
        // Whispers always go to chat (router handles routing to chat),
        // and the gate's sample roll should be bypassed for whispers
        // even with spam-pass-rate `always_skip`. We mark sender as
        // recent so the heuristic passes.
        let v = classifier_gate(
            &event,
            Some("TradeBot"),
            &["TradeBot".to_string()],
            true,
            false,
            &cfg(),
            &mut counter,
            Instant::now(),
            always_skip,
        );
        assert_eq!(v, GateVerdict::Classify);
    }
}
