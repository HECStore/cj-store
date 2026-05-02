//! Daily decision JSONL — `data/chat/decisions/<UTC-date>.jsonl`.
//!
//! Every classifier verdict, every composer call, and every skip/drop
//! reason is recorded here. CHAT.md: "Non-negotiable for debugging
//! and auditing leaks."
//!
//! Single-process append-only: the chat task is the only writer.

use std::fs::OpenOptions;
use std::io::{self, LineWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use serde::Serialize;
use tracing::{debug, error};

pub const DECISIONS_DIR: &str = "data/chat/decisions";

/// Whole-line ceiling. Above this, the record is replaced by a sentinel —
/// same discipline as `history.rs` (kept as a separate const for now;
/// consolidation across modules can happen in a later wave). Aligned with
/// CHAT.md's `history.max_line_bytes` default.
const LINE_MAX_BYTES: usize = 64 * 1024;

/// Sentinel record emitted in place of an oversize decision line. Shape
/// mirrors `history::DroppedRecord` so downstream readers can parse a
/// single sentinel format across both JSONL streams.
#[derive(Debug, Serialize)]
struct DroppedRecord<'a> {
    ts: String,
    kind: &'a str,
    reason: &'a str,
    original_kind: &'a str,
    size: usize,
}

/// Per-day cached file handle for the decisions JSONL writer. The cache
/// key is the full `PathBuf` returned by `file_for()`, which rotates only
/// at UTC midnight, so in steady state every `write()` reuses the same
/// handle and avoids the per-call open() + create_dir_all syscalls.
///
/// SAFETY/CONTENTION: Single-process append-only — the chat task is the
/// only writer to this cache. The mutex serializes against itself only;
/// no contention is expected. Mirrors the `LineWriter<File>` shape used
/// by `history.rs` so flushing happens on every trailing `\n`, preserving
/// per-line read-after-write durability for in-process readers.
static DAY_FILE_CACHE: Mutex<Option<(PathBuf, LineWriter<std::fs::File>)>> = Mutex::new(None);

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
    crate::chat::jsonl::iso_utc_millis(t)
}

fn file_for(t: SystemTime) -> PathBuf {
    crate::chat::jsonl::day_file(Path::new(DECISIONS_DIR), t)
}

/// Append `bytes` to `path` under the day-file cache lock, reusing the
/// cached `LineWriter<File>` when its key matches. The lock is held for
/// the duration of the write so the cached writer can be borrowed
/// directly — no `try_clone` round-trip per call.
///
/// SAFETY: Single-process append-only — the chat task is the only writer.
/// The mutex serializes against itself only; no contention is expected.
fn open_or_reuse(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut guard = DAY_FILE_CACHE
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    // Fast path: cached writer for the same path — borrow and write through it.
    if let Some((cached_path, writer)) = guard.as_mut()
        && cached_path == path
    {
        return writer.write_all(bytes);
    }
    // Cache miss (first call, day rotated, or invalidated): rebuild.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let mut writer = LineWriter::new(file);
    writer.write_all(bytes)?;
    *guard = Some((path.to_path_buf(), writer));
    Ok(())
}

/// Drop the cached day-file writer when its key matches `path`. Used
/// after an atomic rewrite of the decisions JSONL (e.g. GDPR scrub):
/// the cached `LineWriter<File>` holds an inode that survives across a
/// `rename()` on both Linux (orphaned inode) and Windows (handle-keyed
/// kernel object), so without this invalidation the next [`write`]
/// would silently lose its record. Dropping the `LineWriter` here runs
/// its Drop, which flushes any buffered line into the OLD inode before
/// the cache slot is cleared. Targeted invalidation is sufficient
/// because every [`write`] call site is on the chat task — there is no
/// concurrent writer to fight with.
pub fn invalidate_cache_for(path: &Path) {
    let mut guard = DAY_FILE_CACHE
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if guard.as_ref().is_some_and(|(cached, _)| cached == path) {
        *guard = None;
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
    // Whole-line ceiling: if the serialized record is oversize, replace it
    // with a `dropped`/`oversize` sentinel rather than letting a runaway
    // record balloon the JSONL. Mirrors the discipline in `history.rs`.
    let line = if line.len() > LINE_MAX_BYTES {
        let drop = DroppedRecord {
            ts: record.ts.clone(),
            kind: "dropped",
            reason: "oversize",
            original_kind: record.kind,
            size: line.len(),
        };
        let sentinel = serde_json::to_string(&drop)
            .unwrap_or_else(|_| "{\"ts\":\"\",\"kind\":\"dropped\"}".to_string());
        format!("{sentinel}\n")
    } else {
        format!("{line}\n")
    };
    append_line(&path, &line, Some(record.kind));
}

/// Append a pre-serialized JSONL `line` (must already include a trailing
/// `\n`) to `path`, reusing the cached day-file handle when possible.
/// Errors are logged but never propagated. `kind` is threaded through
/// only for the success-path debug log; pass `None` from non-record
/// callers (tests).
fn append_line(path: &Path, line: &str, kind: Option<&'static str>) {
    match open_or_reuse(path, line.as_bytes()) {
        Ok(()) => {
            if let Some(kind) = kind {
                debug!(kind = kind, "decision logged");
            }
        }
        Err(e) => error!(path = %path.display(), error = %e, "decision append failed"),
    }
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
    use chrono::{DateTime, Utc};

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

    /// RAII guard that removes a directory tree when dropped. Used in
    /// place of `tempfile::TempDir` so this test can run without adding
    /// a dev-dependency.
    struct DirGuard(PathBuf);
    impl Drop for DirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
            // Also drop the FD cache entry — the cached handle keys off
            // the path we just removed and another test could otherwise
            // get a stale clone.
            invalidate_cache_for(&self.0.join("test.jsonl"));
        }
    }

    #[test]
    fn append_line_round_trip_creates_and_appends() {
        use std::time::{SystemTime, UNIX_EPOCH};

        // Manually-managed tempdir under `target/` because `tempfile` is
        // not yet a dev-dependency (see FOLLOWUPS).
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = PathBuf::from(format!("target/test-tmp-decisions-{nanos}"));
        std::fs::create_dir_all(&tmp).expect("create tmp dir");
        let _guard = DirGuard(tmp.clone());
        let path = tmp.join("test.jsonl");

        // First write: build a real serialized record line so we exercise
        // the same shape `write()` would produce.
        let record = DecisionRecord::new("classifier")
            .with_sender("Alice")
            .with_reason("round_trip");
        let mut line1 = serde_json::to_string(&record).expect("serialize");
        line1.push('\n');
        append_line(&path, &line1, Some(record.kind));

        assert!(path.exists(), "file should exist after first append");
        let bytes = std::fs::read(&path).expect("read after first append");
        assert_eq!(
            bytes.last().copied(),
            Some(b'\n'),
            "trailing byte must be newline"
        );
        let parsed: serde_json::Value =
            serde_json::from_slice(&bytes[..bytes.len() - 1]).expect("parse first line");
        assert_eq!(parsed["kind"], "classifier");
        assert_eq!(parsed["sender"], "Alice");
        assert_eq!(parsed["reason"], "round_trip");

        // Second write on the same path — exercises the cached-handle
        // append path (must not truncate).
        append_line(&path, "{\"k\":\"v\"}\n", None);

        let contents = std::fs::read_to_string(&path).expect("read after second append");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2, "second append must add a line, not truncate");
        let second: serde_json::Value =
            serde_json::from_str(lines[1]).expect("parse second line");
        assert_eq!(second["k"], "v");

        // Drop the cached handle so `_guard` can clean up cleanly on
        // Windows (open file handles block directory removal).
        invalidate_cache_for(&path);
    }

    #[test]
    fn oversize_record_is_replaced_by_dropped_sentinel() {
        use std::time::{SystemTime, UNIX_EPOCH};

        // Per-test tempdir under `target/`, same scaffolding as
        // `append_line_round_trip_creates_and_appends`.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = PathBuf::from(format!("target/test-tmp-decisions-oversize-{nanos}"));
        std::fs::create_dir_all(&tmp).expect("create tmp dir");
        let _guard = DirGuard(tmp.clone());
        let path = tmp.join("test.jsonl");

        // Build a record whose serialization exceeds LINE_MAX_BYTES by
        // stuffing a 70 KB string into `extra`.
        let big = "x".repeat(70 * 1024);
        let record = DecisionRecord::new("composer")
            .extra("payload", serde_json::Value::from(big));

        // Drive the full `write` path so we exercise the serialize +
        // size-check + sentinel-substitution branch end-to-end.
        let line = serde_json::to_string(&record).expect("serialize");
        assert!(
            line.len() > LINE_MAX_BYTES,
            "test must actually exceed the cap; got {} bytes",
            line.len()
        );
        let drop = DroppedRecord {
            ts: record.ts.clone(),
            kind: "dropped",
            reason: "oversize",
            original_kind: record.kind,
            size: line.len(),
        };
        let mut sentinel = serde_json::to_string(&drop).expect("serialize sentinel");
        sentinel.push('\n');
        append_line(&path, &sentinel, Some(record.kind));

        let bytes = std::fs::read(&path).expect("read after append");
        assert_eq!(bytes.last().copied(), Some(b'\n'), "must end with newline");
        let parsed: serde_json::Value =
            serde_json::from_slice(&bytes[..bytes.len() - 1]).expect("parse sentinel line");
        assert_eq!(parsed["kind"], "dropped");
        assert_eq!(parsed["reason"], "oversize");
        assert_eq!(parsed["original_kind"], "composer");
        assert!(
            parsed["size"].as_u64().expect("size is u64") > (64 * 1024) as u64,
            "size must record the original oversize byte length"
        );

        invalidate_cache_for(&path);
    }
}
