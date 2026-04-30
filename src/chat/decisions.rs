//! Daily decision JSONL — `data/chat/decisions/<UTC-date>.jsonl`.
//!
//! Every classifier verdict, every composer call, and every skip/drop
//! reason is recorded here. CHAT.md: "Non-negotiable for debugging
//! and auditing leaks."
//!
//! Single-process append-only: the chat task is the only writer.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tracing::{debug, error};

pub const DECISIONS_DIR: &str = "data/chat/decisions";

/// Per-day cached file handle for the decisions JSONL writer. The cache
/// key is the full `PathBuf` returned by `file_for()`, which rotates only
/// at UTC midnight, so in steady state every `write()` reuses the same
/// handle and avoids the per-call open() + create_dir_all syscalls.
static DAY_FILE_CACHE: Mutex<Option<(PathBuf, std::fs::File)>> = Mutex::new(None);

/// One JSONL record. Open-shape — callers stuff arbitrary metadata into
/// `extra`, which is flattened into the top-level object. Required
/// fields (ts, kind) are always present.
#[derive(Debug, Serialize)]
pub struct DecisionRecord {
    pub ts: String,
    /// Short tag — `classifier`, `classifier_skip`, `composer`,
    /// `composer_skip`, `pacing_drop`, `cap_tripped`, `tool_call`,
    /// `tool_error`, etc.
    pub kind: &'static str,
    /// Sender of the originating event, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
    /// Originating event timestamp (links back to history JSONL).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_ts: Option<String>,
    /// Free-form reason / message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Latency of the operation in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    /// Token spend for the call, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// USD estimate for this call only (not cumulative).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usd: Option<f64>,
    /// Tokens added to the cache by this call (Anthropic charges 25 %
    /// extra for these; useful for cache-hit-rate triage).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    /// Tokens served from the cache (90 % cheaper than full input).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    /// Open-ended map for kind-specific fields.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
    /// Single source of truth for the instant this record was created.
    /// Used to derive both the rfc3339 `ts` string and the day-file path
    /// in [`write`], so a midnight-boundary tick can never split them.
    /// Skipped during serialization — wire format is unchanged.
    #[serde(skip)]
    created: SystemTime,
}

impl DecisionRecord {
    pub fn new(kind: &'static str) -> Self {
        let now = SystemTime::now();
        Self {
            ts: iso_utc(now),
            kind,
            sender: None,
            event_ts: None,
            reason: None,
            latency_ms: None,
            input_tokens: None,
            output_tokens: None,
            usd: None,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            extra: serde_json::Map::new(),
            created: now,
        }
    }

    pub fn with_sender(mut self, sender: impl Into<String>) -> Self {
        self.sender = Some(sender.into());
        self
    }

    pub fn with_event_ts(mut self, ts: SystemTime) -> Self {
        self.event_ts = Some(iso_utc(ts));
        self
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    pub fn with_latency(mut self, ms: u64) -> Self {
        self.latency_ms = Some(ms);
        self
    }

    pub fn with_tokens(mut self, input: u64, output: u64, usd: f64) -> Self {
        self.input_tokens = Some(input);
        self.output_tokens = Some(output);
        self.usd = Some(usd);
        self
    }

    /// Attach cache-side token counts to the record. Both fields are
    /// emitted only when nonzero so quiet entries stay terse in the
    /// decisions log.
    pub fn with_cache_tokens(mut self, creation: u64, read: u64) -> Self {
        if creation > 0 {
            self.cache_creation_input_tokens = Some(creation);
        }
        if read > 0 {
            self.cache_read_input_tokens = Some(read);
        }
        self
    }

    pub fn extra(mut self, key: &str, value: serde_json::Value) -> Self {
        self.extra.insert(key.to_string(), value);
        self
    }
}

fn iso_utc(t: SystemTime) -> String {
    let dt: DateTime<Utc> = t.into();
    dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn file_for(t: SystemTime) -> PathBuf {
    let dt: DateTime<Utc> = t.into();
    let date = dt.format("%Y-%m-%d");
    PathBuf::from(DECISIONS_DIR).join(format!("{date}.jsonl"))
}

/// Return an owned handle for `path`, reusing the cached day-file handle
/// when its key matches. On any open() / try_clone() / dir-create failure
/// we fall back to a fresh per-call open so a transient filesystem error
/// can't poison the cache or wedge the writer.
fn open_or_reuse(path: &Path) -> io::Result<std::fs::File> {
    // Lock is held only across the open()/clone(); dropped before the
    // caller's actual write_all to keep contention minimal.
    let mut guard = DAY_FILE_CACHE
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some((cached_path, cached_file)) = guard.as_ref()
        && cached_path == path
        && let Ok(clone) = cached_file.try_clone()
    {
        return Ok(clone);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    match file.try_clone() {
        Ok(clone) => {
            *guard = Some((path.to_path_buf(), file));
            Ok(clone)
        }
        Err(_) => {
            // try_clone failed — don't poison the cache; just return the
            // fresh handle so the caller can still write.
            *guard = None;
            Ok(file)
        }
    }
}

/// Append a record to the per-day decisions JSONL. Errors are logged
/// at error level but never propagated — a disk hiccup must not stop
/// the chat task.
pub fn write(record: &DecisionRecord) {
    let path = file_for(record.created);
    let line = match serde_json::to_string(record) {
        Ok(s) => s,
        Err(e) => {
            error!(error = %e, kind = record.kind, "decision serialize failed");
            return;
        }
    };
    match open_or_reuse(&path) {
        Ok(mut f) => {
            let line = format!("{line}\n");
            if let Err(e) = f.write_all(line.as_bytes()) {
                error!(path = %path.display(), error = %e, "decision append failed");
            } else {
                debug!(kind = record.kind, "decision logged");
            }
        }
        Err(e) => error!(path = %path.display(), error = %e, "decision open failed"),
    }
    let _ = path; // keep variable used by future callers
}

#[allow(dead_code)]
pub(crate) fn file_for_test(t: SystemTime) -> PathBuf {
    file_for(t)
}

#[allow(dead_code)]
pub(crate) fn dir() -> &'static Path {
    Path::new(DECISIONS_DIR)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_serializes_required_fields() {
        let r = DecisionRecord::new("classifier_skip")
            .with_sender("Alice")
            .with_reason("pre_classifier_skip");
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["kind"], "classifier_skip");
        assert_eq!(json["sender"], "Alice");
        assert_eq!(json["reason"], "pre_classifier_skip");
        assert!(json.get("ts").is_some());
        // Optional fields are omitted when None.
        assert!(json.get("input_tokens").is_none());
    }

    #[test]
    fn record_with_tokens_emits_three_fields() {
        let r = DecisionRecord::new("classifier").with_tokens(500, 30, 0.0123);
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["input_tokens"], 500);
        assert_eq!(json["output_tokens"], 30);
        assert_eq!(json["usd"], 0.0123);
    }

    #[test]
    fn extra_fields_flatten_to_top_level() {
        let r = DecisionRecord::new("composer")
            .extra("confidence", serde_json::Value::from(0.82))
            .extra("urgency", serde_json::Value::from("med"));
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["confidence"], 0.82);
        assert_eq!(json["urgency"], "med");
    }

    #[test]
    fn file_for_uses_date_under_decisions_dir() {
        use std::time::Duration;
        let t = std::time::UNIX_EPOCH + Duration::from_secs(1_705_314_600);
        let p = file_for(t);
        let s = p.to_string_lossy();
        assert!(s.ends_with("2024-01-15.jsonl"), "got {s}");
        assert!(s.contains("decisions"));
    }

    #[test]
    fn ts_is_iso_with_millis() {
        let r = DecisionRecord::new("test");
        // Ends with ".NNNZ" or ".NNN+NN:NN" — the chrono millis format
        // we asked for. Easiest cheap check: it parses back via chrono.
        let _: DateTime<Utc> = r.ts.parse().expect("ts must parse as RFC3339");
    }
}
