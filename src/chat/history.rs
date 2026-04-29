//! Daily chat history JSONL: append-only persistence of every observed chat
//! event.
//!
//! Files live at `data/chat/history/<UTC-date>.jsonl` and are append-only.
//! Each line is a single JSON object describing one observed event. The
//! writer task is the only writer to its files — single-process
//! atomicity is sufficient because each `write_all` is one syscall and both
//! Linux and NTFS commit it atomically at typical chat-line sizes.
//!
//! ## Field-level truncation
//!
//! We never truncate the serialized line itself — cutting mid-UTF-8 or
//! mid-string-escape would produce unparseable JSON and break every
//! downstream consumer. Instead we cap individual fields before
//! serialization. Per-field caps:
//!
//! - `content`: 4 KB. Chat lines are short anyway; a 4 KB cap defangs
//!   pathological floods without ever affecting a real player.
//! - If after field truncation the whole record still exceeds
//!   `history.max_line_bytes` (default 64 KB), the line is replaced by a
//!   single `{ts, kind:"dropped", reason:"oversize", original_kind:..., size:N}`
//!   sentinel. Phase 1 doesn't yet hit this because `content` is the only
//!   variable-length field, but the discipline lives here so later phases
//!   (tool_result inlining etc.) cannot regress.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::messages::{ChatEvent, ChatEventKind};

/// Root directory of the chat history JSONL files.
pub const HISTORY_DIR: &str = "data/chat/history";

/// Per-field truncation cap for `content`. Bytes, not chars — so we count
/// UTF-8 bytes consistently with Minecraft's 256-byte chat-line ceiling.
const CONTENT_MAX_BYTES: usize = 4096;

/// Whole-line ceiling. Above this, the record is replaced by a sentinel.
/// Aligned with CHAT.md's `history.max_line_bytes` default.
const LINE_MAX_BYTES: usize = 64 * 1024;

/// Serialized JSON shape of one history record. Kept private — external
/// readers should parse the JSONL directly using their own struct, so
/// renames here are caught at the JSON level rather than across module
/// boundaries.
#[derive(Debug, Serialize)]
struct HistoryRecord<'a> {
    /// UTC ISO-8601 timestamp.
    ts: String,
    /// "public" | "whisper" | "bot_chat" | "bot_whisper" | "dropped".
    kind: &'a str,
    sender: &'a str,
    content: &'a str,
    /// Recipient username for bot_out records (whisper target, or the
    /// player the bot is replying to in public chat). Read by trust
    /// derivation in `memory::count_interactions_for_uuid` and by the
    /// GDPR forget-player scrub. Omitted when not applicable so legacy
    /// inbound records stay shape-identical.
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<&'a str>,
    /// Recipient UUID for bot_out records when known. Same readers as
    /// `target`.
    #[serde(skip_serializing_if = "Option::is_none")]
    target_uuid: Option<&'a str>,
    /// Marker that this record was emitted by the bot itself.
    /// Lets later searches attribute lines to the bot even when they were
    /// observed during the pre-Init window where username comparison is
    /// unreliable.
    is_bot: bool,
    /// Set true only when at least one field was truncated by the per-field
    /// cap (see [`CONTENT_MAX_BYTES`]).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    truncated_content: bool,
}

#[derive(Debug, Serialize)]
struct DroppedRecord<'a> {
    ts: String,
    kind: &'a str,
    reason: &'a str,
    original_kind: &'a str,
    size: usize,
}

/// Format a `SystemTime` as a UTC ISO-8601 string. Centralized so the
/// `ts` field shape is guaranteed identical across record kinds.
fn iso_utc(t: SystemTime) -> String {
    let dt: DateTime<Utc> = t.into();
    dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Compute the per-day filename for `t`.
fn file_for(t: SystemTime) -> PathBuf {
    let dt: DateTime<Utc> = t.into();
    let date = dt.format("%Y-%m-%d");
    PathBuf::from(HISTORY_DIR).join(format!("{date}.jsonl"))
}

/// Truncate a UTF-8 string to at most `max_bytes`, on a char boundary.
/// Returns `(possibly truncated slice, was_truncated)`.
fn truncate_utf8(s: &str, max_bytes: usize) -> (&str, bool) {
    if s.len() <= max_bytes {
        return (s, false);
    }
    // Find the largest char boundary ≤ max_bytes. `floor_char_boundary` is
    // unstable as of writing, so we scan back ourselves.
    let mut idx = max_bytes;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    (&s[..idx], true)
}

/// Encode a single chat event into one JSONL line (newline-terminated).
///
/// Public for the test in this module. Production callers should use the
/// async writer task; this function is the pure substrate it builds on.
fn encode_event(event: &ChatEvent, is_bot: bool) -> String {
    let kind = match event.kind {
        ChatEventKind::Public => "public",
        ChatEventKind::Whisper => "whisper",
    };
    encode_with_kind(event, kind, is_bot, None, None)
}

fn encode_with_kind(
    event: &ChatEvent,
    kind: &str,
    is_bot: bool,
    target: Option<&str>,
    target_uuid: Option<&str>,
) -> String {
    let (content, truncated) = truncate_utf8(&event.content, CONTENT_MAX_BYTES);
    let rec = HistoryRecord {
        ts: iso_utc(event.recv_at),
        kind,
        sender: &event.sender,
        content,
        target,
        target_uuid,
        is_bot,
        truncated_content: truncated,
    };
    let mut line = serde_json::to_string(&rec).unwrap_or_else(|e| {
        // serde_json on a struct of borrowed strs basically never fails;
        // log and emit a sentinel rather than panic.
        warn!(error = ?e, "history serialization failed, emitting sentinel");
        let drop = DroppedRecord {
            ts: iso_utc(event.recv_at),
            kind: "dropped",
            reason: "serialize_error",
            original_kind: kind,
            size: event.content.len(),
        };
        serde_json::to_string(&drop).unwrap_or_else(|_| "{\"ts\":\"\",\"kind\":\"dropped\"}".to_string())
    });

    // Whole-line ceiling — should never trigger in Phase 1 with only the
    // `content` field carrying variable bytes, but the guard keeps Phase 5
    // tool-result embedding safe by construction.
    if line.len() > LINE_MAX_BYTES {
        let drop = DroppedRecord {
            ts: iso_utc(event.recv_at),
            kind: "dropped",
            reason: "oversize",
            original_kind: kind,
            size: line.len(),
        };
        line = serde_json::to_string(&drop).unwrap_or_else(|_| "{\"ts\":\"\",\"kind\":\"dropped\"}".to_string());
    }

    line.push('\n');
    line
}

/// Synchronously append a "bot_out" record (CHAT.md `is_bot`
/// tagging). Called from the chat task whenever it sends chat or a
/// whisper. The kind is recorded as `bot_chat` / `bot_whisper`
/// distinctly so log readers can filter.
///
/// `target` and `target_uuid` identify the player the bot is replying
/// to (whisper recipient, or the addressee in public chat). They are
/// stored as structured fields so trust derivation
/// ([`crate::chat::memory::count_interactions_for_uuid`]) and the GDPR
/// scrub can attribute the record without parsing the content text.
pub fn append_bot_output(
    bot_username: &str,
    target: Option<&str>,
    content: &str,
    is_whisper: bool,
) {
    let event = ChatEvent {
        kind: if is_whisper { ChatEventKind::Whisper } else { ChatEventKind::Public },
        sender: bot_username.to_string(),
        content: content.to_string(),
        recv_at: SystemTime::now(),
    };
    let kind_label = if is_whisper { "bot_whisper" } else { "bot_chat" };
    let line = encode_with_kind(&event, kind_label, /* is_bot */ true, target, None);
    let path = file_for(event.recv_at);
    if let Err(e) = append_line(&path, &line) {
        error!(path = %path.display(), error = %e, "[ChatHistory] bot-output append failed");
    }
}

/// Append one line to the per-day JSONL file. Creates the directory and
/// file on demand. Errors are returned to the caller; the writer task logs
/// them and continues so a transient disk hiccup doesn't kill the task.
fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(line.as_bytes())?;
    f.flush()?;
    Ok(())
}

/// History writer task: drains [`ChatEvent`]s from the publisher-side mpsc
/// and persists them to `data/chat/history/<UTC-date>.jsonl`.
///
/// **Single writer.** Owns the `history_rx` exclusively — the bot's
/// `try_send` is the only producer. On `history_rx` close
/// the task drains any buffered events then exits.
///
/// **Failure handling.** A failed append logs an error and continues; we
/// never block forward progress on a single bad write. If disk space is
/// exhausted the failures will be loud and recurring — the operator
/// playbook covers this.
///
/// **Disabled-chat fast path.** When `chat_enabled` is false
/// the task drains the channel silently — no JSONL files are created on
/// disk for trade-only operators. Senders never see a `try_send` failure
/// (the receiver stays alive) but nothing reaches the filesystem.
pub async fn writer_task(mut history_rx: mpsc::Receiver<ChatEvent>, chat_enabled: bool) {
    if !chat_enabled {
        info!("[ChatHistory] chat disabled — writer task draining silently, no on-disk history");
        // Drain so producers' `try_send` calls keep succeeding; we just
        // discard every event. Exits when the sender is dropped.
        while history_rx.recv().await.is_some() {}
        info!("[ChatHistory] history channel closed (chat disabled), writer task exiting");
        return;
    }
    info!("[ChatHistory] writer task starting (path: {})", HISTORY_DIR);
    while let Some(event) = history_rx.recv().await {
        let line = encode_event(&event, /* is_bot */ false);
        let path = file_for(event.recv_at);
        if let Err(e) = append_line(&path, &line) {
            error!(
                path = %path.display(),
                error = %e,
                "[ChatHistory] append failed; event dropped"
            );
        } else {
            debug!(
                kind = ?event.kind,
                sender = %event.sender,
                "[ChatHistory] appended"
            );
        }
    }
    info!("[ChatHistory] history channel closed, writer task exiting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn fixed_event(content: &str) -> ChatEvent {
        ChatEvent {
            kind: ChatEventKind::Public,
            sender: "Steve".to_string(),
            content: content.to_string(),
            // 2024-01-15T10:30:00Z — fixed so encoded `ts` is deterministic.
            recv_at: UNIX_EPOCH + Duration::from_secs(1_705_314_600),
        }
    }

    #[test]
    fn encode_event_round_trips_through_serde() {
        let line = encode_event(&fixed_event("hi"), false);
        // Trailing newline ends the JSONL record.
        assert!(line.ends_with('\n'));
        let parsed: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(parsed["sender"], "Steve");
        assert_eq!(parsed["content"], "hi");
        assert_eq!(parsed["kind"], "public");
        assert_eq!(parsed["is_bot"], false);
        assert_eq!(parsed["ts"], "2024-01-15T10:30:00.000Z");
        // truncated_content is omitted when false.
        assert!(parsed.get("truncated_content").is_none());
    }

    #[test]
    fn whisper_kind_serializes_as_whisper() {
        let mut ev = fixed_event("hello");
        ev.kind = ChatEventKind::Whisper;
        let line = encode_event(&ev, false);
        let parsed: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(parsed["kind"], "whisper");
    }

    #[test]
    fn bot_out_event_tags_is_bot_true() {
        // Phase 4 will send `SendChat` lines through the writer with
        // `is_bot=true`; verify the flag rides through serialization.
        let line = encode_event(&fixed_event("hi"), true);
        let parsed: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(parsed["is_bot"], true);
    }

    #[test]
    fn long_content_is_truncated_at_4kb_with_flag() {
        let big = "x".repeat(8192);
        let line = encode_event(&fixed_event(&big), false);
        let parsed: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        let content = parsed["content"].as_str().unwrap();
        assert!(content.len() <= CONTENT_MAX_BYTES);
        assert_eq!(parsed["truncated_content"], true);
    }

    #[test]
    fn truncate_utf8_does_not_split_codepoints() {
        // 4-byte CJK chars at the boundary must be preserved or dropped
        // wholesale, never split.
        let big: String = std::iter::repeat_n('日', 2000).collect(); // 6000 bytes
        let (out, truncated) = truncate_utf8(&big, 4096);
        assert!(truncated);
        assert!(out.len() <= 4096);
        // out.len() must equal a multiple of 3 (each '日' is 3 bytes in UTF-8).
        assert_eq!(out.len() % 3, 0);
    }

    #[test]
    fn file_for_uses_utc_date() {
        // Pin the filename format so a renamed format directive is caught
        // by the test rather than at runtime.
        let t = UNIX_EPOCH + Duration::from_secs(1_705_314_600);
        let p = file_for(t);
        let s = p.to_string_lossy();
        assert!(s.ends_with("2024-01-15.jsonl"), "got: {s}");
        assert!(s.contains("history"));
    }

    #[tokio::test]
    async fn writer_task_drains_silently_when_disabled() {
        // CHAT.md: when chat is disabled the writer task must drain
        // events without creating any on-disk files. We can't easily
        // observe "no files created" against the real `data/chat/history`
        // path (other tests/processes may write there), so we observe the
        // task's externally-visible behavior instead: events are accepted
        // (try_send succeeds, the receiver stays live), and the task
        // returns promptly once the sender is dropped.
        let (tx, rx) = mpsc::channel::<ChatEvent>(8);
        let handle = tokio::spawn(writer_task(rx, /* chat_enabled */ false));

        // Push a handful of events. If the task were running the enabled
        // body, these would still succeed but each would also trigger a
        // disk write. With chat_enabled=false we simply drain and discard.
        for i in 0..5 {
            tx.try_send(fixed_event(&format!("evt-{i}")))
                .expect("try_send should succeed against the live drain task");
        }
        drop(tx);

        // Bound the wait so a regression that hangs the task surfaces as
        // a test timeout rather than a process-wide hang.
        let res = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        assert!(res.is_ok(), "writer_task should exit promptly after sender drop");
        res.unwrap().expect("writer_task should not panic");
    }

    #[tokio::test]
    async fn writer_task_round_trips_event_to_disk() {
        // End-to-end smoke: send one event, close the channel, verify the
        // file exists and contains the expected content. Use a temp dir so
        // the test is isolated from the real `data/chat/history`.
        let scratch = std::env::temp_dir().join(format!(
            "cj-store-history-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).unwrap();

        // We test `encode_event` + `append_line` directly rather than the
        // task itself, since the task hard-codes the `data/chat/history`
        // path. The two helpers compose to give the task's behavior.
        let line = encode_event(&fixed_event("hello world"), false);
        let target = scratch.join("2024-01-15.jsonl");
        append_line(&target, &line).unwrap();

        // Append a second event — the file must grow, not be overwritten.
        let line2 = encode_event(&fixed_event("again"), false);
        append_line(&target, &line2).unwrap();

        let on_disk = std::fs::read_to_string(&target).unwrap();
        let lines: Vec<&str> = on_disk.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("hello world"));
        assert!(lines[1].contains("again"));

        let _ = std::fs::remove_dir_all(&scratch);
    }
}
