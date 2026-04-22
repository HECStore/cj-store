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
    path::Path,
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
        let json = fs::read_to_string(path)?;
        // Corrupt JSON → empty journal. We keep the swallow-and-continue
        // behaviour (the journal is a diagnostic aid, not a hard dependency),
        // but surface a warning so operators don't silently lose in-flight
        // state on a malformed file. `unwrap_or_default()` on its own would
        // hide the corruption entirely.
        let entries: Vec<JournalEntry> = match serde_json::from_str(&json) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "[Journal] Failed to parse {:?} ({}); treating as empty - any in-flight operation record is lost",
                    path, e
                );
                Vec::new()
            }
        };
        let leftover = entries.into_iter().last();
        Ok((
            Self { entry: None, path: path.to_path_buf() },
            leftover,
        ))
    }

    /// Discard whatever was on disk at startup. Writes an empty journal file.
    pub fn clear_leftover(&mut self) -> io::Result<()> {
        self.entry = None;
        self.persist()
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
                "Journal: overwriting stale in-memory entry {:?} (previous op did not complete cleanly)",
                prev
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
        self.persist()?;
        Ok(operation_id)
    }

    /// Advance the active entry to a new state and persist.
    ///
    /// No-op (with a warning) if there is no active entry — callers should
    /// always `begin` before `advance`, so hitting this path means the call
    /// sites got out of sync.
    pub fn advance(&mut self, state: JournalState) -> io::Result<()> {
        let Some(entry) = self.entry.as_mut() else {
            tracing::warn!("Journal: advance({:?}) called with no active entry", state);
            return Ok(());
        };
        entry.state = state;
        self.persist()
    }

    /// Mark the active operation complete and clear the journal.
    pub fn complete(&mut self) -> io::Result<()> {
        self.entry = None;
        self.persist()
    }

    /// View the currently-active entry, if any.
    #[cfg(test)]
    pub fn current(&self) -> Option<&JournalEntry> {
        self.entry.as_ref()
    }

    fn persist(&self) -> io::Result<()> {
        let path = &self.path;
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }
        let entries: Vec<&JournalEntry> = self.entry.iter().collect();
        let json = serde_json::to_string_pretty(&entries)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
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
}
