//! # Operation journal for crash recovery
//!
//! Shulker-level chest operations are multi-step: the bot takes a shulker out
//! of a chest, places it on a station, transfers items, picks the shulker back
//! up, and replaces it. If the bot crashes mid-sequence (lost connection,
//! process killed, server restart) a shulker can be left on the station or
//! stranded in the bot's inventory, and recovery previously required manual
//! operator intervention.
//!
//! The journal is a persistent record of the *current* in-flight shulker
//! operation. Exactly one entry can be active at a time (chest I/O is
//! serialized through the store task). On startup the bot reads any surviving
//! entry and logs a warning so an operator can reconcile state — this is
//! **detection**, not automatic resume. Full resume would need to replay the
//! correct step against live world state, which is out of scope here.
//!
//! ## File format
//!
//! `data/journal.json` holds a single optional `JournalEntry` serialized as a
//! `Vec<JournalEntry>` (empty vec = no in-flight operation). Using a vec keeps
//! the format forward-compatible if we ever want to track concurrent
//! operations.
//!
//! ## Lifecycle states
//!
//! - `ShulkerTaken` — picked up from the chest slot into the cursor
//! - `ShulkerOnStation` — placed as a block on the shulker station
//! - `ItemsTransferred` — contents moved to/from the bot inventory
//! - `ShulkerPickedUp` — broken and picked back into the bot inventory
//! - `ShulkerReplaced` — put back into its original chest slot (complete)
//!
//! A `ShulkerReplaced` entry is immediately `complete`d, so on disk only
//! truly in-flight entries remain.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use parking_lot::Mutex;

use serde::{Deserialize, Serialize};

use crate::fsutil::write_atomic;

const JOURNAL_FILE: &str = "data/journal.json";

/// Broad category of the operation being journaled.
///
/// Serialized variant names are part of the on-disk format. Adding variants
/// is backwards compatible; renaming them is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JournalOp {
    WithdrawFromChest,
    DepositToChest,
}

/// Where in the shulker lifecycle the bot was when the entry was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JournalState {
    ShulkerTaken,
    ShulkerOnStation,
    ItemsTransferred,
    ShulkerPickedUp,
    ShulkerReplaced,
}

/// One in-flight shulker operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub operation_id: u64,
    pub operation_type: JournalOp,
    pub chest_id: i32,
    pub slot_index: usize,
    pub state: JournalState,
}

/// Persistent journal of in-flight chest operations.
///
/// A `Journal` holds at most one entry at a time because chest I/O is
/// serialized through the store task. [`begin`](Self::begin) writes a new
/// entry, [`advance`](Self::advance) moves the state forward, and
/// [`complete`](Self::complete) clears it. All mutations write the file
/// atomically before returning so a crash between steps leaves a consistent
/// snapshot on disk.
#[derive(Debug)]
pub struct Journal {
    entry: Option<JournalEntry>,
    path: std::path::PathBuf,
}

impl Default for Journal {
    fn default() -> Self {
        Self {
            entry: None,
            path: std::path::PathBuf::from(JOURNAL_FILE),
        }
    }
}

/// Monotonic counter for synthesizing operation IDs during one run.
///
/// The on-disk format uses `u64` IDs which are not compared across restarts,
/// so a process-local counter seeded at 1 is enough — we only need IDs to be
/// unique within a run for logging/correlation.
static NEXT_OPERATION_ID: AtomicU64 = AtomicU64::new(1);

/// Per-process disambiguator for archived crash-evidence filenames.
///
/// Two archive operations colliding on the same `unix_ms` (or both falling
/// back to `unwrap_or(0)` from a clock error) would otherwise produce the
/// same path and `fs::rename` would silently overwrite the older artifact —
/// destroying exactly the crash evidence these helpers exist to preserve.
/// A monotonically-bumped suffix keeps each archive distinct within one run.
static ARCHIVE_SEQ: AtomicU64 = AtomicU64::new(0);

impl Journal {
    /// Load the journal from disk, returning `(journal, leftover)`.
    ///
    /// `leftover` is the entry present on disk at startup, if any. Callers
    /// should log it and then call [`clear_leftover`](Self::clear_leftover)
    /// to zero the file — we intentionally don't auto-clear inside `load`
    /// so callers can decide whether to abort or warn-and-continue.
    pub fn load() -> io::Result<(Self, Option<JournalEntry>)> {
        Self::load_from(Path::new(JOURNAL_FILE))
    }

    fn load_from(path: &Path) -> io::Result<(Self, Option<JournalEntry>)> {
        if !path.exists() {
            return Ok((
                Self { entry: None, path: path.to_path_buf() },
                None,
            ));
        }
        // Distinguish "file present but unreadable" from "missing": a transient
        // IO error (Windows AV scanner hold, lost handle, permission flap) on a
        // journal containing an in-flight entry is exactly the case where
        // preserving the record matters. If we returned the error here the
        // caller would fall back to `Journal::default()` pointing at the same
        // path, and the next `begin()` would silently overwrite the unreadable
        // file. Move it aside first so the path is clear for a fresh journal
        // while the artifact is preserved for operator review.
        let json = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(read_err) => {
                tracing::warn!(
                    "[Journal] failed to read journal file {:?}: {read_err} - attempting to quarantine before falling back to empty journal",
                    path
                );
                match Self::quarantine_unreadable(path) {
                    Ok(archived) => {
                        tracing::error!(
                            "[Journal] quarantined unreadable journal to {:?} - preserve for operator review",
                            archived
                        );
                        return Ok((
                            Self { entry: None, path: path.to_path_buf() },
                            None,
                        ));
                    }
                    Err(rename_err) => {
                        tracing::error!(
                            "[Journal] could not quarantine unreadable journal {:?}: {rename_err} - returning original read error so caller is aware",
                            path
                        );
                        return Err(read_err);
                    }
                }
            }
        };
        // Corrupt JSON → empty journal. We keep the swallow-and-continue
        // behaviour (the journal is a diagnostic aid, not a hard dependency),
        // but surface a warning so operators don't silently lose in-flight
        // state on a malformed file. `unwrap_or_default()` on its own would
        // hide the corruption entirely.
        let entries: Vec<JournalEntry> = match serde_json::from_str(&json) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "[Journal] corrupt journal file {:?}: {e} - treating as empty, any in-flight shulker operation record is lost",
                    path
                );
                Vec::new()
            }
        };
        let leftover = entries.into_iter().last();
        if let Some(entry) = &leftover {
            tracing::info!(
                "[Journal] loaded leftover entry: op_id={} type={:?} chest_id={} slot={} state={:?}",
                entry.operation_id, entry.operation_type, entry.chest_id, entry.slot_index, entry.state
            );
        }
        Ok((
            Self { entry: None, path: path.to_path_buf() },
            leftover,
        ))
    }

    /// Discard whatever was on disk at startup. Writes an empty journal file.
    pub fn clear_leftover(&mut self) -> io::Result<()> {
        self.entry = None;
        let res = self.persist();
        match &res {
            Ok(()) => tracing::info!("[Journal] cleared leftover entry from {:?}", self.path),
            Err(e) => tracing::error!(
                "[Journal] failed to clear leftover entry at {:?}: {e}",
                self.path
            ),
        }
        res
    }

    /// Quarantine the on-disk journal by renaming it to a timestamped sibling.
    ///
    /// Preferred over [`clear_leftover`](Self::clear_leftover) at startup: a
    /// leftover entry is forensic evidence of a stranded shulker, and silently
    /// zeroing the file means a second crash before an operator notices wipes
    /// that evidence. Renaming preserves the artifact while still freeing the
    /// active path so the bot can boot.
    ///
    /// Falls back to copy+remove if the file lives on a different device than
    /// the destination (rare on a single-disk deploy, but rename on Windows
    /// can also fail if another process holds a handle).
    pub fn archive_leftover(&self) -> io::Result<std::path::PathBuf> {
        let unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let seq = ARCHIVE_SEQ.fetch_add(1, Ordering::Relaxed);
        let archived = match self.path.parent() {
            Some(parent) => parent.join(format!("journal.leftover-{unix_ms}-{seq}.json")),
            None => std::path::PathBuf::from(format!("journal.leftover-{unix_ms}-{seq}.json")),
        };
        match fs::rename(&self.path, &archived) {
            Ok(()) => Ok(archived),
            Err(_) => {
                fs::copy(&self.path, &archived)?;
                fs::remove_file(&self.path)?;
                Ok(archived)
            }
        }
    }

    /// Move an unreadable journal file aside to a timestamped sibling so the
    /// active path is free for a fresh journal without clobbering the original.
    ///
    /// Used by [`load_from`](Self::load_from) when `read_to_string` fails on an
    /// existing file (transient IO: AV scanner hold, lost handle, permission
    /// flap). Mirrors [`archive_leftover`](Self::archive_leftover)'s rename →
    /// copy+remove fallback for cross-device or held-handle cases. Returns the
    /// archived path on success so callers can log it.
    fn quarantine_unreadable(path: &Path) -> io::Result<PathBuf> {
        let unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let seq = ARCHIVE_SEQ.fetch_add(1, Ordering::Relaxed);
        let archived = match path.parent() {
            Some(parent) => parent.join(format!("journal.unreadable-{unix_ms}-{seq}.json")),
            None => PathBuf::from(format!("journal.unreadable-{unix_ms}-{seq}.json")),
        };
        match fs::rename(path, &archived) {
            Ok(()) => Ok(archived),
            Err(_) => {
                fs::copy(path, &archived)?;
                fs::remove_file(path)?;
                Ok(archived)
            }
        }
    }

    /// Start tracking a new shulker operation.
    ///
    /// Returns the newly-assigned operation ID so call sites can correlate
    /// log lines. Overwrites any previously active entry — since chest I/O
    /// is serialized, a pre-existing entry indicates either a bug or a
    /// previous crash; we log and move on.
    pub fn begin(
        &mut self,
        operation_type: JournalOp,
        chest_id: i32,
        slot_index: usize,
    ) -> io::Result<u64> {
        if let Some(prev) = &self.entry {
            tracing::warn!(
                "[Journal] overwriting stale in-memory entry op_id={} type={:?} chest_id={} slot={} state={:?} - previous op did not complete cleanly",
                prev.operation_id, prev.operation_type, prev.chest_id, prev.slot_index, prev.state
            );
        }
        let operation_id = NEXT_OPERATION_ID.fetch_add(1, Ordering::Relaxed);
        self.entry = Some(JournalEntry {
            operation_id,
            operation_type,
            chest_id,
            slot_index,
            state: JournalState::ShulkerTaken,
        });
        self.persist().inspect_err(|e| {
            tracing::error!(
                "[Journal] failed to persist begin: op_id={operation_id} type={operation_type:?} chest_id={chest_id} slot={slot_index}: {e}"
            );
        })?;
        Ok(operation_id)
    }

    /// Advance the active entry to a new state and persist.
    ///
    /// No-op (with a warning) if there is no active entry — callers should
    /// always `begin` before `advance`, so hitting this path means the call
    /// sites got out of sync.
    pub fn advance(&mut self, state: JournalState) -> io::Result<()> {
        let Some(entry) = self.entry.as_mut() else {
            tracing::warn!(
                "[Journal] advance to {state:?} called with no active entry - begin/advance call sites out of sync"
            );
            return Ok(());
        };
        let op_id = entry.operation_id;
        let chest_id = entry.chest_id;
        let slot_index = entry.slot_index;
        entry.state = state;
        self.persist().inspect_err(|e| {
            tracing::error!(
                "[Journal] failed to persist advance to {state:?}: op_id={op_id} chest_id={chest_id} slot={slot_index}: {e}"
            );
        })
    }

    /// Mark the active operation complete and clear the journal.
    pub fn complete(&mut self) -> io::Result<()> {
        let op_id = self.entry.as_ref().map(|e| e.operation_id);
        self.entry = None;
        self.persist().inspect_err(|e| {
            tracing::error!(
                "[Journal] failed to persist complete: op_id={op_id:?}: {e}"
            );
        })
    }

    /// View the currently-active entry, if any.
    #[cfg(test)]
    pub fn current(&self) -> Option<&JournalEntry> {
        self.entry.as_ref()
    }

    fn persist(&self) -> io::Result<()> {
        let path = &self.path;
        if let Some(parent) = path.parent()
            && !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        let entries: Vec<&JournalEntry> = self.entry.iter().collect();
        let json = serde_json::to_string_pretty(&entries)
            .map_err(io::Error::other)?;
        write_atomic(path, &json)?;
        Ok(())
    }
}

/// Thread-safe handle shared with the bot task.
///
/// Uses `parking_lot::Mutex` rather than `std::sync::Mutex` or
/// `tokio::sync::Mutex`: mutations are short (in-memory update + one atomic
/// file write). parking_lot gives us a non-poisoning lock with no `Result`
/// return on `lock()`, eliminating the panic path where a prior panic inside
/// the critical section would otherwise poison the mutex and kill every
/// subsequent bot operation. Callers still must not hold the guard across
/// `.await` points.
pub type SharedJournal = std::sync::Arc<Mutex<Journal>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_journal(suffix: &str) -> (Journal, std::path::PathBuf) {
        let dir = std::env::temp_dir()
            .join(format!("cj-store-journal-{}-{}", suffix, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("journal.json");
        let j = Journal { entry: None, path: path.clone() };
        (j, dir)
    }

    #[test]
    fn begin_assigns_unique_ids() {
        let (mut j, dir) = temp_journal("ids");

        let a = j.begin(JournalOp::WithdrawFromChest, 1, 2).unwrap();
        j.complete().unwrap();
        let b = j.begin(JournalOp::DepositToChest, 3, 4).unwrap();
        assert_ne!(a, b);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn advance_transitions_state() {
        let (mut j, dir) = temp_journal("advance");

        j.begin(JournalOp::WithdrawFromChest, 7, 12).unwrap();
        assert_eq!(j.current().unwrap().state, JournalState::ShulkerTaken);
        j.advance(JournalState::ShulkerOnStation).unwrap();
        assert_eq!(j.current().unwrap().state, JournalState::ShulkerOnStation);
        j.advance(JournalState::ItemsTransferred).unwrap();
        j.advance(JournalState::ShulkerPickedUp).unwrap();
        j.advance(JournalState::ShulkerReplaced).unwrap();
        j.complete().unwrap();
        assert!(j.current().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_surfaces_leftover() {
        let (mut j, dir) = temp_journal("load");
        let path = j.path.clone();

        j.begin(JournalOp::DepositToChest, 5, 9).unwrap();
        j.advance(JournalState::ItemsTransferred).unwrap();

        let (mut loaded, leftover) = Journal::load_from(&path).unwrap();
        let leftover = leftover.expect("leftover entry should be present");
        assert_eq!(leftover.chest_id, 5);
        assert_eq!(leftover.slot_index, 9);
        assert_eq!(leftover.state, JournalState::ItemsTransferred);
        assert_eq!(leftover.operation_type, JournalOp::DepositToChest);

        loaded.clear_leftover().unwrap();
        let (_again, leftover) = Journal::load_from(&path).unwrap();
        assert!(leftover.is_none(), "clear_leftover should empty the file");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_from_returns_none_when_file_missing() {
        let dir = std::env::temp_dir().join(format!(
            "cj-store-journal-missing-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("journal.json");
        assert!(!path.exists());

        let (j, leftover) = Journal::load_from(&path).unwrap();
        assert!(leftover.is_none());
        assert!(j.current().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_from_treats_corrupt_json_as_empty() {
        let (j, dir) = temp_journal("corrupt");
        let path = j.path.clone();
        drop(j);
        std::fs::write(&path, "{ this is not valid json ][").unwrap();

        let (_loaded, leftover) = Journal::load_from(&path).unwrap();
        assert!(
            leftover.is_none(),
            "corrupt JSON must surface as empty leftover, not an error"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn begin_overwrites_stale_entry() {
        let (mut j, dir) = temp_journal("stale");

        let first = j.begin(JournalOp::WithdrawFromChest, 11, 3).unwrap();
        j.advance(JournalState::ShulkerOnStation).unwrap();
        // Simulate a previous op that never reached `complete` (e.g. crash).
        let second = j.begin(JournalOp::DepositToChest, 22, 7).unwrap();
        assert_ne!(first, second);
        let cur = j.current().expect("entry must exist after begin");
        assert_eq!(cur.operation_id, second);
        assert_eq!(cur.chest_id, 22);
        assert_eq!(cur.slot_index, 7);
        assert_eq!(cur.state, JournalState::ShulkerTaken);
        assert_eq!(cur.operation_type, JournalOp::DepositToChest);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn advance_without_active_entry_is_noop_not_error() {
        let (mut j, dir) = temp_journal("advance-noop");

        j.advance(JournalState::ShulkerOnStation).unwrap();
        assert!(j.current().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn complete_without_active_entry_is_noop_not_error() {
        let (mut j, dir) = temp_journal("complete-noop");

        j.complete().unwrap();
        assert!(j.current().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archive_leftover_disambiguates_rapid_successive_calls() {
        // Two archive operations colliding on the same unix_ms (or both falling
        // back to unwrap_or(0) from a clock error) must not clobber each other:
        // the per-process SEQ counter is what guarantees distinct paths.
        let dir = std::env::temp_dir().join(format!(
            "cj-store-journal-archive-seq-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("journal.json");

        // First archive: distinct payload "alpha".
        std::fs::write(&path, "alpha").unwrap();
        let j1 = Journal { entry: None, path: path.clone() };
        let archived1 = j1.archive_leftover().expect("first archive");

        // Second archive: distinct payload "beta", in rapid succession.
        std::fs::write(&path, "beta").unwrap();
        let j2 = Journal { entry: None, path: path.clone() };
        let archived2 = j2.archive_leftover().expect("second archive");

        assert_ne!(archived1, archived2, "archive paths must differ");
        assert!(archived1.exists(), "first archive must still exist");
        assert!(archived2.exists(), "second archive must exist");
        assert_eq!(std::fs::read_to_string(&archived1).unwrap(), "alpha");
        assert_eq!(std::fs::read_to_string(&archived2).unwrap(), "beta");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quarantine_unreadable_disambiguates_rapid_successive_calls() {
        // Same disambiguator contract for the quarantine path: two unreadable
        // events in the same millisecond must each produce their own archive.
        let dir = std::env::temp_dir().join(format!(
            "cj-store-journal-quarantine-seq-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("journal.json");

        std::fs::write(&path, "first-unreadable").unwrap();
        let archived1 = Journal::quarantine_unreadable(&path).expect("first quarantine");

        std::fs::write(&path, "second-unreadable").unwrap();
        let archived2 = Journal::quarantine_unreadable(&path).expect("second quarantine");

        assert_ne!(archived1, archived2, "quarantine paths must differ");
        assert!(archived1.exists(), "first quarantine must still exist");
        assert!(archived2.exists(), "second quarantine must exist");
        assert_eq!(std::fs::read_to_string(&archived1).unwrap(), "first-unreadable");
        assert_eq!(std::fs::read_to_string(&archived2).unwrap(), "second-unreadable");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn persist_survives_round_trip_through_disk() {
        let (mut j, dir) = temp_journal("roundtrip");
        let path = j.path.clone();

        let op_id = j.begin(JournalOp::WithdrawFromChest, 101, 17).unwrap();
        j.advance(JournalState::ShulkerPickedUp).unwrap();

        let (_loaded, leftover) = Journal::load_from(&path).unwrap();
        let entry = leftover.expect("persisted entry must be readable");
        assert_eq!(entry.operation_id, op_id);
        assert_eq!(entry.chest_id, 101);
        assert_eq!(entry.slot_index, 17);
        assert_eq!(entry.state, JournalState::ShulkerPickedUp);
        assert_eq!(entry.operation_type, JournalOp::WithdrawFromChest);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
