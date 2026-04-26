//! Classifier — fast "should I respond?" pre-filter.
//!
//! ## Two-stage design (PLAN §4.2)
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
    /// Heuristic: not directly addressed, no question marker, no recent
    /// interaction within `chat.recent_speaker_secs`.
    PreClassifierSkip,
    /// Heuristic gate passed but the per-call sample rate rolled "skip".
    SampleRate,
    /// Per-sender per-minute classifier cap exhausted (PLAN §4.2.1 S8).
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
/// counts as direct address. PLAN §4.4 calls out the dictionary downgrade
/// for bot names that are also common English words (Sky, Steve, Alex);
/// callers must filter that list before passing it in. Phase 1 callers
/// pass just the live `bot_username`; Phase 6 will broaden to nicknames
/// from `persona.md`.
pub fn is_direct_address(content: &str, bare_word_eligible_names: &[String]) -> bool {
    // `@<name>` anywhere in the first 16 chars.
    let head = if content.len() > 16 {
        &content[..16]
    } else {
        content
    };
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

/// Decide whether to dispatch the event to the classifier (PLAN §4.2.1).
///
/// Returns [`GateVerdict::Classify`] only when ALL of the following hold:
///
/// 1. Heuristic gate: directly addressed, OR question-shaped, OR sender
///    interacted with the bot in the last `recent_speaker_secs`.
/// 2. Per-sender per-minute cap not yet exceeded.
/// 3. Random sample roll succeeds (at the configured sample rate; direct
///    addresses bypass the sample roll).
///
/// `sample_roll` is a function that returns a uniform-random `f32` in
/// `[0.0, 1.0)` — caller-supplied so tests can use a deterministic value.
/// `sender_recent_speaker` is the caller's record of whether this sender
/// has interacted in the last `recent_speaker_secs`.
///
/// `spam_suppressed` short-circuits the gate when the sender is currently
/// in spam cooldown (PLAN §4.2.1).
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
    // (PLAN §4.1), but a defensive check here shaves a wasted classifier
    // call if the upstream filter ever regresses.
    if let Some(bot) = bot_username
        && event.sender.eq_ignore_ascii_case(bot)
    {
        return GateVerdict::Skip(SkipReason::PreClassifierSkip);
    }

    // Bot username goes into the bare-word match set unless it's
    // dictionary-conflicted; the caller is responsible for that filter
    // (passes the empty list when the name is in common-words.txt).
    let directly_addressed =
        is_direct_address(&event.content, bare_word_eligible_names);

    let question = is_question_shaped(&event.content);

    let heuristic_pass = directly_addressed || question || sender_recent_speaker;
    if !heuristic_pass {
        return GateVerdict::Skip(SkipReason::PreClassifierSkip);
    }

    // Per-sender cap. Direct addresses still count against it but the cap
    // is generous enough (default 3/min) that legitimate addressed-burst
    // chat is fine.
    if counter.record_and_check(&event.sender, config.classifier_per_sender_per_minute, now) {
        return GateVerdict::Skip(SkipReason::PerSenderCap);
    }

    // Sample rate — bypassed for direct addresses (PLAN §4.4: direct
    // addresses bypass dyad/silence guards; same spirit applies here so
    // we don't accidentally drop a "@bot, you online?").
    if !directly_addressed
        && (event.kind == ChatEventKind::Public)
        && sample_roll() >= config.classifier_sample_rate
    {
        return GateVerdict::Skip(SkipReason::SampleRate);
    }

    GateVerdict::Classify
}

// ===== Stage B — actual Haiku call =========================================

use serde::Deserialize;

/// Strict-shape verdict the classifier returns. PLAN §4.2.2.
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
    #[serde(default)]
    pub trigger: String,
}

/// System prompt the classifier sees. Wrapped together with the
/// adjustments block in the snapshot the caller passes in (PLAN §4.2.2:
/// "cache breakpoint here" on adjustments).
pub fn system_prompt(persona_summary: &str, adjustments_md: &str) -> String {
    format!(
        "You are a chat-classifier for a Minecraft bot. Decide whether the bot \
         should reply to the most recent message. Output STRICT JSON with this \
         shape and nothing else:\n\
         \n\
         {{\n  \"respond\": <true|false>,\n  \"confidence\": <0.0-1.0>,\n  \
         \"reason\": \"<short explanation>\",\n  \"urgency\": \"<low|med|high>\",\n  \
         \"ai_callout\": {{\"detected\": <true|false>, \"trigger\": \"<verbatim quote if true>\"}}\n}}\n\
         \n\
         Guidance:\n\
         - The bot stays in character (see persona summary). It is OK to stay \
           silent on most messages — humans don't reply to everything.\n\
         - Set ai_callout.detected = true ONLY when a player accuses the bot of \
           being AI / scripted / a robot. Quote the exact trigger.\n\
         - Confidence reflects how sure you are about responding/not — not how \
           important the topic is.\n\
         \n\
         === Persona summary ===\n{persona_summary}\n\
         \n\
         === Adjustments (style lessons learned) ===\n{adjustments_md}",
    )
}

/// Build the classifier request. `history_slice` is the trailing N
/// chat lines (PLAN: default 30); `event` is the new message under
/// consideration. The adjustments block carries a cache-control marker
/// so the prefix is cached across calls (PLAN §4.2.2 P2).
pub fn build_request(
    model: &str,
    persona_summary: &str,
    adjustments_md: &str,
    history_slice: &str,
    event: &crate::messages::ChatEvent,
    cache_ttl: crate::chat::client::CacheTtl,
) -> crate::chat::client::CreateMessageRequest {
    use crate::chat::client::{CacheControl, ContentBlock, Message, Role, SystemBlock};

    let system = vec![
        SystemBlock::Text {
            text: system_prompt(persona_summary, adjustments_md),
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
        temperature: Some(0.0),
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
/// PLAN §4.7. Errors are logged but never raised — pending writes are
/// best-effort and recoverable from history.
pub fn write_pending_adjustment(
    trigger: &str,
    sender: &str,
    sender_uuid: Option<&str>,
) {
    use std::fs::OpenOptions;
    use std::io::Write;
    let entry = crate::chat::reflection::PendingEntry {
        ts: chrono::Utc::now()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        trigger: trigger.to_string(),
        sender: sender.to_string(),
        sender_uuid: sender_uuid.map(str::to_string),
        observed_day_utc: chrono::Utc::now().format("%Y-%m-%d").to_string(),
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
        // Defensive duplicate of §4.1 self-echo filter.
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
    fn no_signal_message_skipped_at_pre_classifier() {
        // No question, no direct address, no recent interaction. Must skip.
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
        assert_eq!(v, GateVerdict::Skip(SkipReason::PreClassifierSkip));
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
    fn question_skipped_when_sample_says_skip() {
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
        assert_eq!(v, GateVerdict::Skip(SkipReason::SampleRate));
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
        assert_eq!(ac.trigger, "you sound like a bot");
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
        assert_eq!(req.system.len(), 2);
        // First system block (persona + adjustments) carries the
        // cache_control marker; second (history) does not.
        match &req.system[0] {
            crate::chat::client::SystemBlock::Text { cache_control, .. } => {
                assert!(cache_control.is_some(), "adjustments block must be cached");
            }
        }
        match &req.system[1] {
            crate::chat::client::SystemBlock::Text { cache_control, .. } => {
                assert!(cache_control.is_none(), "history slice must be uncached");
            }
        }
    }

    #[test]
    fn whisper_event_bypasses_sample_rate_gate() {
        // PLAN §4.2.1: sample rate applies to public-chat events; whispers
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
