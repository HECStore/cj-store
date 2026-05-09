//! # File System Utilities
//!
//! Provides atomic file write operations to prevent torn writes during crashes.
//!
//! **Strategy**: Write to `*.tmp`, then `fs::rename` it into place. Rust's
//! `fs::rename` uses `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` on Windows,
//! so the replace is atomic on both platforms — no manual remove-first dance.
//! On rename failure (sharing violation, cross-volume, transient AV/indexer
//! lock) we fall through to a recovery path that copies the new bytes to a
//! `*.recovery.tmp` sibling, aside-renames the live file to `{path}.bak`, and
//! then atomically renames the recovery-temp into place — so a mid-stream
//! `fs::copy` failure can never truncate the prior good bytes at `path`.
//!
//! **Note on directory durability**: On Unix the parent directory is fsynced
//! after the rename so the directory entry itself survives a crash. On Windows
//! the parent directory is intentionally NOT fsynced (Windows has no
//! equivalent operation), so directory-entry durability there depends on the
//! filesystem journal (e.g. NTFS) rather than an explicit flush from us.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Writes a file atomically using a temporary file + rename pattern.
///
/// **Process**:
/// 1. Write contents to `{filename}.tmp` and `sync_all`
/// 2. Rename temp file to the final name (atomically replaces an existing
///    destination on both Unix and Windows — Rust's `fs::rename` uses
///    `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` on Windows)
/// 3. On rename failure (sharing violation, cross-volume, etc.): aside-rename
///    the prior file to `{path}.bak` and fall back to a defensive copy, so
///    a partial-copy can never destroy the prior good bytes
///
/// **Atomicity**: This is "best-effort" — not crash-proof in all edge cases,
/// but prevents torn writes in normal operation. For true atomicity, consider
/// using platform-specific APIs (e.g., `CreateFile` with `FILE_FLAG_WRITE_THROUGH` on Windows).
///
/// **Used By**: All JSON persistence operations (users, pairs, orders, trades, nodes, queue, config).
/// This ensures state files are never left in a corrupted state.
pub fn write_atomic(path: impl AsRef<Path>, contents: &str) -> io::Result<()> {
    let path = path.as_ref();

    // `file_name()` returns None for `.` and `..`, and the inner `to_str()` can
    // reject non-UTF-8 names on Unix; both cases produce `InvalidInput` so the
    // caller sees the real reason rather than a confusing later error.
    let file_name = path.file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Invalid file path: {path:?}")
        ))?;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    // Stat-then-mkdir on the hot path: a single existence check is much
    // cheaper than the unconditional mkdir syscall the journal pays on every
    // shulker-state transition.
    if !parent.exists() {
        fs::create_dir_all(parent)?;
    }

    let tmp_name = format!("{file_name}.tmp");
    let tmp_path = parent.join(&tmp_name);

    // `sync_all` flushes file contents and metadata to disk so a power loss
    // between the rename and any subsequent operation cannot leave a
    // zero-length or partially-written destination after recovery — relevant
    // on every platform; the Unix branch below additionally fsyncs the parent
    // directory so the rename itself survives a crash.
    {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }

    // Happy path: a single rename that atomically replaces any existing
    // destination on both platforms. No aside-rename, no .bak bookkeeping —
    // those used to run on every successful overwrite (3-4 extra syscalls per
    // shulker-state transition) under the false premise that Windows rename
    // won't replace; Rust's `fs::rename` on Windows passes
    // `MOVEFILE_REPLACE_EXISTING`, so the replace is already atomic.
    match fs::rename(&tmp_path, path) {
        Ok(_) => {
            #[cfg(unix)]
            sync_parent_dir(parent);
            Ok(())
        }
        Err(e) => rename_failed_fallback_copy(path, parent, file_name, &tmp_path, e),
    }
}

/// Pick a free `<dir>/<base>.<kind>-<unix_ms>-<seq>[-<bump>].json` path.
///
/// The caller passes a per-module `ARCHIVE_SEQ` so within-process collisions
/// are impossible. The bump suffix protects across-process restarts where
/// `unix_ms` falls back to 0 from a clock error AND the sibling counter is
/// reset — without it, `fs::rename` would silently overwrite the prior
/// archive, destroying exactly the crash evidence the helper preserves.
pub(crate) fn pick_archive_path(
    parent: Option<&Path>,
    base: &str,
    kind: &str,
    seq: &AtomicU64,
) -> PathBuf {
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let n = seq.fetch_add(1, Ordering::Relaxed);
    let primary = format!("{base}.{kind}-{unix_ms}-{n}.json");
    let mut candidate = match parent {
        Some(p) => p.join(&primary),
        None => PathBuf::from(&primary),
    };
    let mut bump: u32 = 0;
    while candidate.exists() && bump < 1000 {
        bump += 1;
        let alt = format!("{base}.{kind}-{unix_ms}-{n}-{bump}.json");
        candidate = match parent {
            Some(p) => p.join(&alt),
            None => PathBuf::from(&alt),
        };
    }
    candidate
}

/// `sync_all` an archived file (best-effort) and its parent directory on Unix.
/// Mirrors `write_atomic`'s durability dance for the cross-device fallback in
/// archive helpers — without this, a power loss between `fs::copy` and the OS
/// page-cache flush can leave the archive empty or partial.
pub(crate) fn durably_sync_archive(archived: &Path) {
    match fs::File::open(archived) {
        Ok(file) => {
            if let Err(e) = file.sync_all() {
                tracing::warn!(
                    "[Archive] sync_all on {archived:?} failed: {e} — continuing; archive may not be fully durable across crashes"
                );
            }
        }
        Err(e) => tracing::warn!(
            "[Archive] cannot reopen {archived:?} to fsync: {e} — continuing; archive may not be fully durable across crashes"
        ),
    }
    #[cfg(unix)]
    if let Some(parent) = archived.parent() {
        sync_parent_dir(parent);
    }
}

#[cfg(unix)]
fn sync_parent_dir(parent: &Path) {
    match fs::File::open(parent) {
        Ok(dir) => {
            if let Err(e) = dir.sync_all() {
                tracing::warn!(
                    "[File] sync_all on parent dir {parent:?} failed: {e} — rename's directory entry may not be fully durable across crashes"
                );
            }
        }
        Err(e) => tracing::warn!(
            "[File] cannot open parent dir {parent:?} to fsync: {e} — rename's directory entry may not be fully durable across crashes"
        ),
    }
}

/// Recovery path when the atomic rename fails (sharing violation, cross-volume
/// move, UNC quirk, long path, transient Windows lock).
///
/// **Crash-safety:** the prior good bytes at `path` must survive any single
/// failure here. The earlier implementation used a bare `fs::copy(tmp, path)`,
/// which on Unix opens `path` with O_TRUNC (and on Windows uses CopyFileExW's
/// overwrite mode) — both truncate the destination at the moment of open, so
/// a mid-stream ENOSPC/EIO/sharing-violation left the destination empty or
/// partial. We now copy `tmp_path` to a sibling **recovery-temp** first,
/// `sync_all` it, optionally aside-rename the live `path` to `{path}.bak`
/// (so even a failure of the second rename leaves a recoverable copy at
/// `.bak`), then `fs::rename` the recovery-temp into `path`. The destination
/// is only ever flipped in one atomic step.
fn rename_failed_fallback_copy(
    path: &Path,
    parent: &Path,
    file_name: &str,
    tmp_path: &Path,
    rename_err: io::Error,
) -> io::Result<()> {
    tracing::warn!("[File] rename {tmp_path:?} -> {path:?} failed: {rename_err} — moving prior file aside and falling back to copy-then-rename");

    // 1. Copy tmp_path to a sibling recovery-temp and fsync. The recovery-temp
    //    is a distinct path so that `fs::copy` cannot truncate the live `path`
    //    even if it fails midway.
    let recovery_tmp = parent.join(format!("{file_name}.recovery.tmp"));
    // Stale recovery-temp from an earlier failed run must not block this attempt.
    let _ = fs::remove_file(&recovery_tmp);
    let prior_existed = path.exists();
    if let Err(copy_err) = fs::copy(tmp_path, &recovery_tmp) {
        let prior_state = if prior_existed {
            format!("prior file preserved at: {path:?}")
        } else {
            format!("no prior file existed at {path:?} — this was a first write")
        };
        tracing::error!("[File] cannot save {path:?}: rename={rename_err}, copy-to-recovery-temp={copy_err}; {prior_state}");
        return Err(io::Error::other(format!(
            "Failed to save file: rename error: {rename_err}, copy-to-recovery-temp error: {copy_err} (path: {path:?}, {prior_state})"
        )));
    }
    match fs::File::open(&recovery_tmp) {
        Ok(file) => {
            if let Err(sync_err) = file.sync_all() {
                tracing::warn!("[File] sync_all on recovery-temp {recovery_tmp:?} failed: {sync_err} — continuing; rename remains atomic");
            }
        }
        Err(open_err) => tracing::warn!(
            "[File] cannot reopen recovery-temp {recovery_tmp:?} to fsync: {open_err} — continuing; second rename may not be fully durable across crashes"
        ),
    }

    // 2. On both platforms, aside-rename the live `path` to `{path}.bak` so
    //    that the final atomic rename below has a known-empty target slot.
    //    Even if that final rename fails, the prior good bytes are at .bak
    //    for an operator to recover. Five attempts on Windows for transient
    //    AV/indexer locks; one attempt on Unix where rename is reliable.
    let bak_path: Option<std::path::PathBuf> = {
        let mut bak: Option<std::path::PathBuf> = None;
        if prior_existed {
            let candidate = parent.join(format!("{file_name}.bak"));
            // A stale .bak from a prior failed write must not block the
            // aside-rename; remove it before moving the current good file aside.
            let _ = fs::remove_file(&candidate);
            #[cfg(windows)]
            {
                for attempt in 0..5 {
                    match fs::rename(path, &candidate) {
                        Ok(_) => {
                            bak = Some(candidate.clone());
                            break;
                        }
                        Err(e) => {
                            if attempt == 4 {
                                tracing::debug!("[File] {path:?}: aside-rename to .bak failed after 5 attempts: {e} — final rename below will overwrite path directly");
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(10 * (1 << attempt)));
                        }
                    }
                }
            }
            #[cfg(not(windows))]
            {
                match fs::rename(path, &candidate) {
                    Ok(_) => {
                        bak = Some(candidate.clone());
                    }
                    Err(e) => {
                        tracing::debug!("[File] {path:?}: aside-rename to .bak failed: {e} — final rename below will overwrite path directly");
                    }
                }
            }
        }
        bak
    };

    // 3. Atomically rename the recovery-temp into the destination. This is
    //    a clean swap: either the new bytes are at `path`, or the old bytes
    //    are at `.bak`. No torn-state intermediate is observable.
    match fs::rename(&recovery_tmp, path) {
        Ok(_) => {
            #[cfg(unix)]
            sync_parent_dir(parent);
            // Live path is fresh; clean up the bookkeeping siblings.
            let _ = fs::remove_file(tmp_path);
            if let Some(ref bak) = bak_path {
                let _ = fs::remove_file(bak);
            }
            Ok(())
        }
        Err(final_err) => {
            // Final rename failed. The prior good bytes are at `.bak`
            // (if the aside-rename succeeded) or still at `path` (if it
            // didn't — Windows-only fallthrough). Leave the recovery-temp
            // and tmp_path on disk so an operator can recover by hand.
            let preserved = match bak_path.as_ref() {
                Some(p) => format!("prior file preserved at {p:?}"),
                None if prior_existed => format!("prior file preserved at {path:?}"),
                None => format!("no prior file existed at {path:?} — this was a first write"),
            };
            tracing::error!(
                "[File] cannot save {path:?}: rename={rename_err}, recovery-rename={final_err}; new bytes preserved at {recovery_tmp:?} for manual recovery; {preserved}"
            );
            Err(io::Error::other(format!(
                "Failed to save file: rename error: {rename_err}, recovery-rename error: {final_err} (path: {path:?}, new bytes at: {recovery_tmp:?}, {preserved})"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// Scratch directory under the system temp dir, uniquely named so tests
    /// can't clobber each other or stale runs. Cleaned up in each test's
    /// Drop via the returned guard.
    struct TmpDir(std::path::PathBuf);

    impl TmpDir {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "cj-store-fsutil-{}-{}",
                name,
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&base).unwrap();
            Self(base)
        }

        fn path(&self, name: &str) -> std::path::PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn read_to_string(p: &Path) -> String {
        let mut f = fs::File::open(p).unwrap();
        let mut s = String::new();
        f.read_to_string(&mut s).unwrap();
        s
    }

    #[test]
    fn creates_file_when_destination_missing() {
        let dir = TmpDir::new("create-missing");
        let target = dir.path("new.json");
        assert!(!target.exists());
        write_atomic(&target, "hello").unwrap();
        assert_eq!(read_to_string(&target), "hello");
        // .tmp must not survive a successful write.
        assert!(!dir.path("new.json.tmp").exists());
    }

    #[test]
    fn overwrites_existing_file() {
        let dir = TmpDir::new("overwrite");
        let target = dir.path("data.json");
        fs::write(&target, "old contents").unwrap();
        write_atomic(&target, "new contents").unwrap();
        assert_eq!(read_to_string(&target), "new contents");
        assert!(!dir.path("data.json.tmp").exists());
    }

    #[test]
    fn creates_parent_directory_if_missing() {
        let dir = TmpDir::new("mkdirs");
        let target = dir.path("nested/sub/file.json");
        write_atomic(&target, "x").unwrap();
        assert_eq!(read_to_string(&target), "x");
    }

    #[test]
    fn empty_string_is_valid_content() {
        let dir = TmpDir::new("empty");
        let target = dir.path("empty.json");
        write_atomic(&target, "").unwrap();
        assert_eq!(read_to_string(&target), "");
    }

    #[test]
    fn rejects_path_without_file_name() {
        // A path ending in "/" (or "\\") has no file_name component — the
        // validator should refuse with InvalidInput rather than producing a
        // confusing downstream error from the create/rename step.
        let dir = TmpDir::new("reject-dir");
        // Target the directory itself, which has a file_name, but use ".." which
        // does not. "." also has no file_name component on Unix.
        let bad: std::path::PathBuf = dir.path("..").components()
            .take_while(|c| !matches!(c, std::path::Component::ParentDir))
            .collect();
        // If the above didn't produce a bad path for this platform, fall back to
        // an empty path which is explicitly rejected.
        let bad = if bad.file_name().is_none() { bad } else { std::path::PathBuf::from("") };
        let err = write_atomic(&bad, "x").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn survives_stale_tmp_file() {
        // A `.tmp` sibling left behind by a prior crash must not poison the
        // next write: `write_atomic` overwrites the temp file in place via
        // `File::create`, so a fresh write should produce the new contents
        // and leave no `.tmp` sibling.
        let dir = TmpDir::new("stale-tmp");
        let target = dir.path("target.json");
        let stale_tmp = dir.path("target.json.tmp");
        fs::write(&stale_tmp, b"garbage from a prior crash").unwrap();
        assert!(stale_tmp.exists());

        write_atomic(&target, "fresh").unwrap();

        assert_eq!(read_to_string(&target), "fresh");
        assert!(!stale_tmp.exists(), "stale .tmp must be gone after successful write");
    }

    #[test]
    fn temp_file_cleaned_up_on_success() {
        // Regression guard: a successful write must not leave a .tmp sibling
        // and must not leave a .bak sibling. The happy path goes straight
        // through `fs::rename` (atomic replace on both platforms) and never
        // creates a `.bak` — that artefact is reserved for the rename-failure
        // recovery path in `rename_failed_fallback_copy`.
        let dir = TmpDir::new("cleanup");
        let target = dir.path("foo.json");
        write_atomic(&target, "data").unwrap();
        let mut found_tmp = false;
        let mut found_bak = false;
        for entry in fs::read_dir(&dir.0).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().to_string();
            if name.ends_with(".tmp") {
                found_tmp = true;
            }
            if name.ends_with(".bak") {
                found_bak = true;
            }
        }
        assert!(!found_tmp, "unexpected leftover .tmp file in {:?}", dir.0);
        assert!(!found_bak, "unexpected leftover .bak file in {:?}", dir.0);
    }

    #[test]
    fn two_sequential_writes_clean_up_bak() {
        // The recovery path (only) aside-renames the prior file to `{path}.bak`
        // on both platforms before its final atomic rename, and cleans it up on
        // success. The happy path does not touch `.bak` at all. Either way, two
        // sequential successful writes must leave no `.bak` (or `.tmp`) sibling.
        let dir = TmpDir::new("seq-bak-cleanup");
        let target = dir.path("ledger.json");
        write_atomic(&target, "v1").unwrap();
        write_atomic(&target, "v2").unwrap();
        assert_eq!(read_to_string(&target), "v2");
        for entry in fs::read_dir(&dir.0).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().to_string();
            assert!(!name.ends_with(".bak"), "unexpected leftover .bak: {name}");
            assert!(!name.ends_with(".tmp"), "unexpected leftover .tmp: {name}");
        }
    }

    #[test]
    fn stale_bak_does_not_block_overwrite() {
        // A `.bak` left behind by a prior failed write must not block the next
        // overwrite — the happy path goes straight through `fs::rename`
        // (atomic replace on both platforms) and never touches `.bak`. The
        // `.bak` stays where it was (recoverable forensic evidence of the
        // earlier failure); the new write still lands at the live path.
        let dir = TmpDir::new("stale-bak");
        let target = dir.path("ledger.json");
        let stale_bak = dir.path("ledger.json.bak");
        fs::write(&target, "good").unwrap();
        fs::write(&stale_bak, "ancient garbage").unwrap();

        write_atomic(&target, "fresh").unwrap();
        assert_eq!(read_to_string(&target), "fresh");
    }

    /// When the destination rename and the recovery rename both fail, the
    /// prior good file's bytes must be preserved either at the live path or
    /// at `{path}.bak`. We can't reliably force a rename failure in a unit
    /// test without unsafe FFI or external locking tools, so this test pins
    /// the contract on the happy path (the only path testable in CI) — the
    /// failure-path .bak invariant is documented in
    /// `rename_failed_fallback_copy`.
    #[test]
    fn stale_recovery_tmp_does_not_block_overwrite() {
        // A stale `.recovery.tmp` from a prior failed run must not block the
        // next successful write — the happy path goes straight through
        // `fs::rename`, which never touches `.recovery.tmp`. The stale file
        // is left in place for an operator to inspect (matches the
        // .bak-stale-after-failure semantics).
        let dir = TmpDir::new("stale-recovery-tmp");
        let target = dir.path("ledger.json");
        let stale_recovery = dir.path("ledger.json.recovery.tmp");
        fs::write(&target, "good").unwrap();
        fs::write(&stale_recovery, "ancient garbage").unwrap();

        write_atomic(&target, "fresh").unwrap();
        assert_eq!(read_to_string(&target), "fresh");
        // Happy-path overwrite: stale recovery-temp is not touched (it
        // would only be cleared if the recovery branch had run).
        assert!(stale_recovery.exists(), "stale .recovery.tmp left for operator review on happy path");
    }

    #[test]
    fn good_file_bytes_preserved_across_overwrite() {
        let dir = TmpDir::new("preserve-bytes");
        let target = dir.path("ledger.json");
        write_atomic(&target, "ledgered economic data").unwrap();
        // Overwrite — the happy path goes straight through `fs::rename`
        // (atomic replace on both platforms), so no `.bak` is ever created.
        // The test below asserts no `.bak` remains after success.
        write_atomic(&target, "v2").unwrap();
        assert_eq!(read_to_string(&target), "v2");
        // No .bak should remain after success.
        assert!(!dir.path("ledger.json.bak").exists());
    }
}
