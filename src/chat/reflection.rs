//! Reflection pass — distills `pending_adjustments.jsonl` into bullets
//! for `adjustments.md`. PLAN §4.7.
//!
//! ## Two-stage poisoning defense
//!
//! 1. The classifier writes call-out signals to
//!    `data/chat/pending_adjustments.jsonl` (one JSON line per detection).
//! 2. A separate, lower-frequency reflection pass — running on Haiku per
//!    PLAN §4.7 P9 — reads the pending file, paraphrases the lessons,
//!    and writes them to `adjustments.md`. Each candidate lesson is run
//!    through [`MultiAxisValidator`] (PLAN §4.7 ADV2 + ADV12) before
//!    being admitted.
//!
//! Phase 6 lands the file format (serde) and the validator. The actual
//! Haiku call lives in the chat task and is wired up alongside the
//! composer (Phase 4) — the validator is tested independently because
//! it's the security-load-bearing piece.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// One pending entry. Written by the classifier when `ai_callout.detected`,
/// consumed by the reflection pass.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingEntry {
    /// UTC ISO-8601 timestamp.
    pub ts: String,
    /// Verbatim quote from the player. Wrapped in nonce-tagged
    /// `<untrusted_chat_*>` markers before being shown to the
    /// reflection model (PLAN §4.7 S2).
    pub trigger: String,
    /// Sender's username at observation time.
    pub sender: String,
    /// Sender's UUID at observation time. Optional because UUID
    /// resolution is lazy (PLAN §3.1).
    #[serde(default)]
    pub sender_uuid: Option<String>,
    /// UTC date the entry was observed (YYYY-MM-DD). Used by the
    /// distinct-days check below.
    pub observed_day_utc: String,
}

/// Pre-validation summary of one Haiku-paraphrased lesson candidate.
/// The reflection model produces these; we run them through
/// [`MultiAxisValidator::check`] before writing them to
/// `adjustments.md`.
pub struct LessonCandidate<'a> {
    /// The paraphrased lesson the reflection model produced.
    pub lesson: &'a str,
    /// The pending entries the reflection model claims to have
    /// abstracted from. Indices into the pending file consumed for
    /// this batch.
    pub source_entries: &'a [&'a PendingEntry],
    /// Per-sender Trust score, computed by [`crate::chat::tools`]
    /// helpers from history. Trust < 1 triggers the §ADV12 quality
    /// failure.
    pub trust_for_sender: &'a (dyn Fn(&str) -> u8 + Sync),
}

/// Result of [`MultiAxisValidator::check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LessonVerdict {
    Admit,
    /// Substring overlap ≥ 40 % — naive copy of trigger content.
    SubstringOverlap,
    /// Fewer than `min_distinct_triggers` distinct triggers cited.
    NotEnoughDistinctTriggers,
    /// Fewer than `min_distinct_senders` distinct senders cited.
    NotEnoughDistinctSenders,
    /// At least one contributing sender has Trust < 1 (PLAN §ADV12).
    LowTrustSender { sender: String },
}

/// Multi-axis lesson validator (PLAN §4.7 ADV2 + ADV12).
pub struct MultiAxisValidator {
    pub min_distinct_triggers: usize,
    pub min_distinct_senders: usize,
    /// Substring-overlap percentage threshold (0.0–1.0). PLAN: 0.40.
    pub substring_overlap_threshold: f64,
}

impl Default for MultiAxisValidator {
    fn default() -> Self {
        Self {
            min_distinct_triggers: 3,
            min_distinct_senders: 3,
            substring_overlap_threshold: 0.40,
        }
    }
}

impl MultiAxisValidator {
    pub fn check(&self, candidate: &LessonCandidate<'_>) -> LessonVerdict {
        // 1. Distinct triggers (case-insensitive on the trigger string).
        let mut distinct_triggers: HashSet<String> = HashSet::new();
        for e in candidate.source_entries {
            distinct_triggers.insert(e.trigger.to_lowercase());
        }
        if distinct_triggers.len() < self.min_distinct_triggers {
            return LessonVerdict::NotEnoughDistinctTriggers;
        }

        // 2. Distinct senders (lowercased usernames).
        let mut distinct_senders: HashSet<String> = HashSet::new();
        for e in candidate.source_entries {
            distinct_senders.insert(e.sender.to_lowercase());
        }
        if distinct_senders.len() < self.min_distinct_senders {
            return LessonVerdict::NotEnoughDistinctSenders;
        }

        // 3. Per-sender Trust ≥ 1 floor.
        for sender in &distinct_senders {
            if (candidate.trust_for_sender)(sender) < 1 {
                return LessonVerdict::LowTrustSender {
                    sender: sender.clone(),
                };
            }
        }

        // 4. Substring overlap. Each trigger contributes its
        //    longest-common-substring length to the overlap-byte budget;
        //    final overlap is `total_overlap / lesson.len()`.
        let lesson_lc = candidate.lesson.to_lowercase();
        let lesson_len = lesson_lc.len();
        if lesson_len == 0 {
            return LessonVerdict::SubstringOverlap; // empty lesson is suspicious
        }
        let mut max_run = 0usize;
        for e in candidate.source_entries {
            let trig_lc = e.trigger.to_lowercase();
            let run = longest_common_substring_len(&lesson_lc, &trig_lc);
            if run > max_run {
                max_run = run;
            }
        }
        let overlap = max_run as f64 / lesson_len as f64;
        if overlap >= self.substring_overlap_threshold {
            return LessonVerdict::SubstringOverlap;
        }

        LessonVerdict::Admit
    }
}

/// Length of the longest common substring between two byte slices.
/// O(n*m) DP — fine for the small inputs reflection deals with
/// (lessons ≤ 280 chars; triggers ≤ chat-line cap of 240).
fn longest_common_substring_len(a: &str, b: &str) -> usize {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.is_empty() || bb.is_empty() {
        return 0;
    }
    let n = ab.len();
    let m = bb.len();
    let mut prev = vec![0usize; m + 1];
    let mut cur = vec![0usize; m + 1];
    let mut best = 0usize;
    for i in 1..=n {
        for j in 1..=m {
            if ab[i - 1] == bb[j - 1] {
                cur[j] = prev[j - 1] + 1;
                if cur[j] > best {
                    best = cur[j];
                }
            } else {
                cur[j] = 0;
            }
        }
        std::mem::swap(&mut prev, &mut cur);
        for v in cur.iter_mut() {
            *v = 0;
        }
    }
    best
}

// ===== Trigger gates (PLAN §4.7) ===========================================

/// True if it has been at least `min_interval_secs` since the last
/// reflection pass. `last_reflection_at` is ISO-UTC (RFC3339); `None`
/// means "never run" and is treated as "interval elapsed".
///
/// Unparseable timestamps are also treated as "elapsed" so a corrupt
/// state file can't permanently lock the reflection pass off.
pub fn min_interval_elapsed(
    last_reflection_at: Option<&str>,
    min_interval_secs: u32,
) -> bool {
    let Some(s) = last_reflection_at else {
        return true;
    };
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) else {
        return true;
    };
    let then = t.with_timezone(&chrono::Utc);
    let now = chrono::Utc::now();
    (now - then).num_seconds() >= min_interval_secs as i64
}

/// True if pending count >= `max_pending` AND distinct senders in
/// pending >= `min_distinct_senders`. PLAN §4.7 size-cap auto-trigger:
/// once the pending file fills up, fire a reflection pass even if
/// `min_interval_secs` hasn't elapsed — but only when we have at
/// least the minimum diversity of senders (a single griefer flooding
/// pending shouldn't trip the trigger alone).
pub fn should_trigger_size_cap(
    pending: &[PendingEntry],
    max_pending: u32,
    min_distinct_senders: u32,
) -> bool {
    if (pending.len() as u32) < max_pending {
        return false;
    }
    let mut senders: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for p in pending {
        senders.insert(p.sender.as_str());
    }
    senders.len() as u32 >= min_distinct_senders
}

/// True if `pending` is non-empty AND chat has been idle (no composer
/// call) for at least `idle_trigger_secs` AND distinct senders meet
/// the floor. PLAN §4.7 idle-window auto-trigger.
///
/// `last_composer_at` is RFC3339-UTC; `None` (never called) is treated
/// as "idle".
pub fn should_trigger_idle(
    pending: &[PendingEntry],
    last_composer_at: Option<&str>,
    idle_trigger_secs: u32,
    min_distinct_senders: u32,
) -> bool {
    if pending.is_empty() {
        return false;
    }
    let mut senders: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for p in pending {
        senders.insert(p.sender.as_str());
    }
    if (senders.len() as u32) < min_distinct_senders {
        return false;
    }
    match last_composer_at {
        None => true,
        Some(s) => match chrono::DateTime::parse_from_rfc3339(s) {
            Ok(t) => {
                let then = t.with_timezone(&chrono::Utc);
                let now = chrono::Utc::now();
                (now - then).num_seconds() >= idle_trigger_secs as i64
            }
            Err(_) => true,
        },
    }
}

// ===== Pass orchestration ==================================================

pub const PENDING_FILE: &str = "data/chat/pending_adjustments.jsonl";
pub const ADJUSTMENTS_FILE: &str = "data/chat/adjustments.md";

/// Outcome of [`run_pass`]: bullets that were admitted to
/// `adjustments.md`, plus the count of candidates rejected per reason.
#[derive(Debug, Default)]
pub struct ReflectionOutcome {
    pub admitted: Vec<String>,
    pub rejected_substring: usize,
    pub rejected_distinct_triggers: usize,
    pub rejected_distinct_senders: usize,
    pub rejected_low_trust: usize,
    pub haiku_input_tokens: u64,
    pub haiku_output_tokens: u64,
}

/// Read the pending-adjustments file. Returns `Ok(vec![])` if missing.
pub fn read_pending() -> std::io::Result<Vec<PendingEntry>> {
    let p = std::path::Path::new(PENDING_FILE);
    if !p.exists() {
        return Ok(Vec::new());
    }
    let body = std::fs::read_to_string(p)?;
    let mut out = Vec::new();
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<PendingEntry>(line) {
            Ok(e) => out.push(e),
            Err(e) => tracing::warn!(error = %e, line = %line, "[Chat] skipping malformed pending entry"),
        }
    }
    Ok(out)
}

/// After a successful pass, atomically rotate the pending file out of
/// the way so a fresh batch starts empty (PLAN §4.7 C17 crash-recovery
/// shape: rename to `pending_adjustments.<UTC>.jsonl`).
pub fn rotate_pending() -> std::io::Result<()> {
    let p = std::path::Path::new(PENDING_FILE);
    if !p.exists() {
        return Ok(());
    }
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let dest = std::path::Path::new("data/chat")
        .join(format!("pending_adjustments.{stamp}.jsonl"));
    std::fs::rename(p, dest)
}

/// Drive a complete reflection pass: read pending, ask Haiku to
/// produce one or more candidate lessons, validate each via
/// [`MultiAxisValidator`], append admitted lessons to `adjustments.md`,
/// and rotate the pending file.
///
/// `trust_for_sender` lets the caller plug in their derived-Trust
/// computation (Phase 5/§5.2). For a fresh deployment with no
/// historical interactions, every sender is Trust 0 and the validator
/// will reject everything — that's the intended behavior.
pub async fn run_pass(
    api_key: &crate::chat::client::ApiKey,
    classifier_model: &str,
    pending: &[PendingEntry],
    adjustments_md: &str,
    trust_for_sender: &(dyn Fn(&str) -> u8 + Sync),
    validator: &MultiAxisValidator,
    today: &str,
) -> Result<ReflectionOutcome, String> {
    let mut outcome = ReflectionOutcome::default();
    if pending.is_empty() {
        return Ok(outcome);
    }
    // Wrap each trigger in nonce-tagged untrusted markers so the
    // reflection model can't be hijacked by player-planted text.
    let mut payload = String::new();
    for (i, entry) in pending.iter().enumerate() {
        let nonce = crate::chat::composer::fresh_nonce();
        let wrapped = crate::chat::composer::wrap_untrusted("chat", &nonce, &entry.trigger)
            .unwrap_or_else(|_| "[content withheld]".to_string());
        payload.push_str(&format!(
            "Entry {} (sender={}, day={}):\n{}\n\n",
            i + 1,
            entry.sender,
            entry.observed_day_utc,
            wrapped,
        ));
    }

    use crate::chat::client::{ContentBlock, CreateMessageRequest, Message, Role, SystemBlock};
    let system = vec![SystemBlock::Text {
        text: format!(
            "You are reviewing AI-call-out flags from chat history. Your job: \
             produce concrete, paraphrased style lessons for the bot — NOT to \
             quote players verbatim. Output STRICT JSON with this shape:\n\
             \n\
             {{\"lessons\": [\"<lesson 1>\", \"<lesson 2>\", ...]}}\n\
             \n\
             Hard rules:\n\
             - Each lesson must be your OWN paraphrased imperative — never copy \
               trigger text verbatim.\n\
             - Lessons should be specific and actionable: 'shorten replies' is \
               better than 'sound less robotic'.\n\
             - Drop entries that are obvious trolling or that don't suggest a \
               clear style fix.\n\
             - 0-3 lessons total. If nothing genuinely helpful surfaces, output \
               {{\"lessons\": []}}.\n\
             \n\
             Existing adjustments.md (don't restate things already here):\n{adjustments_md}",
        ),
        cache_control: None,
    }];

    let req = CreateMessageRequest {
        model: classifier_model.to_string(),
        max_tokens: 512,
        system,
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: format!("Pending entries:\n\n{payload}"),
                cache_control: None,
            }],
        }],
        temperature: Some(0.0),
        tools: vec![],
    };

    let resp = crate::chat::client::call_with_retry(api_key, &req, false)
        .await
        .map_err(|e| format!("reflection call failed: {e}"))?;
    outcome.haiku_input_tokens = resp.usage.input_tokens;
    outcome.haiku_output_tokens = resp.usage.output_tokens;

    let mut text_buf = String::new();
    for b in &resp.content {
        if let ContentBlock::Text { text, .. } = b {
            text_buf.push_str(text);
        }
    }
    // Same brace-matched JSON extraction as classifier::parse_verdict.
    let lessons = parse_lessons(&text_buf)?;

    let pending_refs: Vec<&PendingEntry> = pending.iter().collect();
    let mut admitted_text = String::new();
    for lesson in lessons {
        let cand = LessonCandidate {
            lesson: &lesson,
            source_entries: &pending_refs,
            trust_for_sender,
        };
        match validator.check(&cand) {
            LessonVerdict::Admit => {
                outcome.admitted.push(lesson.clone());
                admitted_text.push_str(&format!(
                    "- {today} | trigger: <see pending log> | lesson: {lesson}\n"
                ));
            }
            LessonVerdict::SubstringOverlap => outcome.rejected_substring += 1,
            LessonVerdict::NotEnoughDistinctTriggers => {
                outcome.rejected_distinct_triggers += 1
            }
            LessonVerdict::NotEnoughDistinctSenders => {
                outcome.rejected_distinct_senders += 1
            }
            LessonVerdict::LowTrustSender { .. } => outcome.rejected_low_trust += 1,
        }
    }

    if !admitted_text.is_empty() {
        let mut new_body = adjustments_md.to_string();
        if !new_body.is_empty() && !new_body.ends_with('\n') {
            new_body.push('\n');
        }
        new_body.push_str(&admitted_text);
        crate::fsutil::write_atomic(ADJUSTMENTS_FILE, &new_body)
            .map_err(|e| format!("write adjustments.md: {e}"))?;
    }

    rotate_pending().map_err(|e| format!("rotate pending: {e}"))?;
    Ok(outcome)
}

/// Extract `{"lessons": [...]}` from a Haiku response. Same brace-
/// match strategy as `classifier::parse_verdict`.
fn parse_lessons(text: &str) -> Result<Vec<String>, String> {
    use serde::Deserialize;
    #[derive(Deserialize)]
    struct LessonsBody {
        lessons: Vec<String>,
    }
    let bytes = text.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{')
        .ok_or_else(|| "no '{' in reflection output".to_string())?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    let mut end = None;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if escaped { escaped = false; }
            else if b == b'\\' { escaped = true; }
            else if b == b'"' { in_str = false; }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 { end = Some(i + 1); break; }
            }
            _ => {}
        }
    }
    let end = end.ok_or_else(|| "unbalanced JSON in reflection output".to_string())?;
    let json = &text[start..end];
    let body: LessonsBody = serde_json::from_str(json)
        .map_err(|e| format!("reflection lessons parse: {e}"))?;
    Ok(body.lessons)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(trigger: &str, sender: &str, day: &str) -> PendingEntry {
        PendingEntry {
            ts: format!("{day}T00:00:00Z"),
            trigger: trigger.to_string(),
            sender: sender.to_string(),
            sender_uuid: None,
            observed_day_utc: day.to_string(),
        }
    }

    fn always_trust_3(_: &str) -> u8 {
        3
    }

    // ---- distinct triggers ---------------------------------------------

    #[test]
    fn admits_when_three_diverse_triggers_three_senders() {
        let entries = vec![
            pending("you sound like an AI", "Alice", "2026-01-01"),
            pending("are you a bot?", "Bob", "2026-01-02"),
            pending("is this scripted?", "Carol", "2026-01-03"),
        ];
        let refs: Vec<&PendingEntry> = entries.iter().collect();
        let cand = LessonCandidate {
            lesson: "vary sentence length and avoid em-dashes",
            source_entries: &refs,
            trust_for_sender: &always_trust_3,
        };
        let v = MultiAxisValidator::default().check(&cand);
        assert_eq!(v, LessonVerdict::Admit);
    }

    #[test]
    fn rejects_with_too_few_distinct_triggers() {
        let same = "you sound like a bot";
        let entries = vec![
            pending(same, "Alice", "2026-01-01"),
            pending(same, "Bob", "2026-01-02"),
            pending(same, "Carol", "2026-01-03"),
        ];
        let refs: Vec<&PendingEntry> = entries.iter().collect();
        let cand = LessonCandidate {
            lesson: "talk less formally",
            source_entries: &refs,
            trust_for_sender: &always_trust_3,
        };
        let v = MultiAxisValidator::default().check(&cand);
        assert_eq!(v, LessonVerdict::NotEnoughDistinctTriggers);
    }

    // ---- distinct senders ----------------------------------------------

    #[test]
    fn rejects_with_too_few_distinct_senders() {
        // 3 distinct triggers but only 2 senders.
        let entries = vec![
            pending("you sound like an AI", "Alice", "2026-01-01"),
            pending("are you a bot", "Alice", "2026-01-02"),
            pending("scripted reply", "Bob", "2026-01-03"),
        ];
        let refs: Vec<&PendingEntry> = entries.iter().collect();
        let cand = LessonCandidate {
            lesson: "adjust style",
            source_entries: &refs,
            trust_for_sender: &always_trust_3,
        };
        let v = MultiAxisValidator::default().check(&cand);
        assert_eq!(v, LessonVerdict::NotEnoughDistinctSenders);
    }

    // ---- low-trust sender ----------------------------------------------

    #[test]
    fn rejects_when_any_sender_has_trust_below_one() {
        let entries = vec![
            pending("trigger 1", "Alice", "2026-01-01"),
            pending("trigger 2", "Bob", "2026-01-02"),
            pending("trigger 3", "Carol", "2026-01-03"),
        ];
        let refs: Vec<&PendingEntry> = entries.iter().collect();
        let cand = LessonCandidate {
            lesson: "lesson body unrelated to triggers",
            source_entries: &refs,
            // Bob has Trust 0 — fresh / suspicious, should fail.
            trust_for_sender: &|s| if s == "bob" { 0 } else { 3 },
        };
        let v = MultiAxisValidator::default().check(&cand);
        assert!(matches!(v, LessonVerdict::LowTrustSender { .. }));
    }

    // ---- substring overlap ---------------------------------------------

    #[test]
    fn rejects_when_lesson_copies_trigger_content() {
        // Lesson is mostly trigger text — naive copy. PLAN §ADV2.
        let entries = vec![
            pending("don't use em-dashes ever again", "Alice", "2026-01-01"),
            pending("sound like a bot", "Bob", "2026-01-02"),
            pending("scripted", "Carol", "2026-01-03"),
        ];
        let refs: Vec<&PendingEntry> = entries.iter().collect();
        let cand = LessonCandidate {
            lesson: "don't use em-dashes ever again",
            source_entries: &refs,
            trust_for_sender: &always_trust_3,
        };
        let v = MultiAxisValidator::default().check(&cand);
        assert_eq!(v, LessonVerdict::SubstringOverlap);
    }

    #[test]
    fn admits_paraphrased_lesson_under_overlap_threshold() {
        let entries = vec![
            pending("don't use em-dashes ever again", "Alice", "2026-01-01"),
            pending("sound like a bot to me", "Bob", "2026-01-02"),
            pending("you are scripted aren't you", "Carol", "2026-01-03"),
        ];
        let refs: Vec<&PendingEntry> = entries.iter().collect();
        let cand = LessonCandidate {
            // No shared substring runs > ~6 chars; well under 40 %.
            lesson: "vary punctuation; keep replies casual and brief",
            source_entries: &refs,
            trust_for_sender: &always_trust_3,
        };
        let v = MultiAxisValidator::default().check(&cand);
        assert_eq!(v, LessonVerdict::Admit);
    }

    #[test]
    fn rejects_empty_lesson() {
        let entries = vec![
            pending("a", "Alice", "2026-01-01"),
            pending("b", "Bob", "2026-01-02"),
            pending("c", "Carol", "2026-01-03"),
        ];
        let refs: Vec<&PendingEntry> = entries.iter().collect();
        let cand = LessonCandidate {
            lesson: "",
            source_entries: &refs,
            trust_for_sender: &always_trust_3,
        };
        let v = MultiAxisValidator::default().check(&cand);
        assert_eq!(v, LessonVerdict::SubstringOverlap);
    }

    // ---- pending entry serde -------------------------------------------

    #[test]
    fn pending_entry_round_trips_through_jsonl() {
        let e = PendingEntry {
            ts: "2026-04-26T10:00:00.000Z".to_string(),
            trigger: "you sound like a bot".to_string(),
            sender: "Alice".to_string(),
            sender_uuid: Some("11111111-2222-3333-4444-555555555555".to_string()),
            observed_day_utc: "2026-04-26".to_string(),
        };
        let line = serde_json::to_string(&e).unwrap();
        let back: PendingEntry = serde_json::from_str(&line).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn pending_entry_accepts_missing_uuid() {
        let raw = r#"{
            "ts": "2026-04-26T10:00:00.000Z",
            "trigger": "you sound like a bot",
            "sender": "Alice",
            "observed_day_utc": "2026-04-26"
        }"#;
        let back: PendingEntry = serde_json::from_str(raw).unwrap();
        assert!(back.sender_uuid.is_none());
    }

    // ---- helper --------------------------------------------------------

    // ---- parse_lessons --------------------------------------------------

    #[test]
    fn parse_lessons_extracts_the_array() {
        let raw = r#"Sure: {"lessons": ["keep replies short", "vary punctuation"]}"#;
        let v = parse_lessons(raw).unwrap();
        assert_eq!(v, vec!["keep replies short", "vary punctuation"]);
    }

    #[test]
    fn parse_lessons_empty_array_is_ok() {
        let raw = r#"{"lessons": []}"#;
        let v = parse_lessons(raw).unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn parse_lessons_rejects_no_json() {
        assert!(parse_lessons("nothing here").is_err());
    }

    // ---- read_pending / rotate_pending ----------------------------------

    #[test]
    fn read_pending_returns_empty_for_missing_file() {
        // We don't override PENDING_FILE for this test (it points at a
        // process-shared path). The function returns Ok in both branches
        // and must not panic.
        let _ = read_pending().unwrap();
    }

    // ---- trigger gates --------------------------------------------------

    #[test]
    fn min_interval_elapsed_none_returns_true() {
        // No prior reflection — always allowed.
        assert!(min_interval_elapsed(None, 3600));
    }

    #[test]
    fn min_interval_elapsed_unparseable_returns_true() {
        // Corrupt state — fail-open so the pass can recover.
        assert!(min_interval_elapsed(Some("not-a-timestamp"), 3600));
    }

    #[test]
    fn min_interval_elapsed_recent_returns_false() {
        let now = chrono::Utc::now()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        assert!(!min_interval_elapsed(Some(&now), 3600));
    }

    #[test]
    fn min_interval_elapsed_old_returns_true() {
        let old = (chrono::Utc::now() - chrono::Duration::seconds(7200))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        assert!(min_interval_elapsed(Some(&old), 3600));
    }

    #[test]
    fn should_trigger_size_cap_below_cap_is_false() {
        let entries = vec![
            pending("a", "Alice", "2026-01-01"),
            pending("b", "Bob", "2026-01-02"),
        ];
        assert!(!should_trigger_size_cap(&entries, 5, 2));
    }

    #[test]
    fn should_trigger_size_cap_at_cap_with_distinct_senders_is_true() {
        let entries = vec![
            pending("a", "Alice", "2026-01-01"),
            pending("b", "Bob", "2026-01-02"),
            pending("c", "Carol", "2026-01-03"),
        ];
        assert!(should_trigger_size_cap(&entries, 3, 3));
    }

    #[test]
    fn should_trigger_size_cap_at_cap_without_diversity_is_false() {
        // 3 entries, all from one sender — capped but no diversity.
        let entries = vec![
            pending("a", "Alice", "2026-01-01"),
            pending("b", "Alice", "2026-01-02"),
            pending("c", "Alice", "2026-01-03"),
        ];
        assert!(!should_trigger_size_cap(&entries, 3, 2));
    }

    #[test]
    fn should_trigger_idle_empty_pending_is_false() {
        // Nothing to reflect on.
        assert!(!should_trigger_idle(&[], None, 600, 1));
    }

    #[test]
    fn should_trigger_idle_diversity_floor_blocks() {
        let entries = vec![pending("a", "Alice", "2026-01-01")];
        // min_distinct_senders=2 unmet → no trigger even though chat is idle.
        assert!(!should_trigger_idle(&entries, None, 600, 2));
    }

    #[test]
    fn should_trigger_idle_no_composer_call_means_idle() {
        let entries = vec![
            pending("a", "Alice", "2026-01-01"),
            pending("b", "Bob", "2026-01-02"),
        ];
        assert!(should_trigger_idle(&entries, None, 600, 2));
    }

    #[test]
    fn should_trigger_idle_recent_composer_blocks() {
        let entries = vec![
            pending("a", "Alice", "2026-01-01"),
            pending("b", "Bob", "2026-01-02"),
        ];
        let now = chrono::Utc::now()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        assert!(!should_trigger_idle(&entries, Some(&now), 600, 2));
    }

    #[test]
    fn should_trigger_idle_old_composer_admits() {
        let entries = vec![
            pending("a", "Alice", "2026-01-01"),
            pending("b", "Bob", "2026-01-02"),
        ];
        let old = (chrono::Utc::now() - chrono::Duration::seconds(1200))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        assert!(should_trigger_idle(&entries, Some(&old), 600, 2));
    }

    #[test]
    fn longest_common_substring_basic() {
        assert_eq!(longest_common_substring_len("hello world", "world peace"), 5);
        assert_eq!(longest_common_substring_len("abc", "xyz"), 0);
        assert_eq!(longest_common_substring_len("", "abc"), 0);
        assert_eq!(longest_common_substring_len("identical", "identical"), 9);
    }
}
