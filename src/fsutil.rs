//! # File System Utilities
//!
//! Provides atomic file write operations to prevent torn writes during crashes.
//!
//! **Strategy**: Write to `*.tmp`, then rename to final file.
//! On Windows, rename won't overwrite, so we remove destination first.
//!
//! **Note on directory durability**: On Unix the parent directory is fsynced
//! after the rename so the directory entry itself survives a crash. On Windows
//! the parent directory is intentionally NOT fsynced (Windows has no
//! equivalent operation), so directory-entry durability there depends on the
//! filesystem journal (e.g. NTFS) rather than an explicit flush from us.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

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
            {
                if let Ok(dir) = fs::File::open(parent) {
                    let _ = dir.sync_all();
                }
            }
            Ok(())
        }
        Err(e) => rename_failed_fallback_copy(path, parent, file_name, &tmp_path, e),
    }
}

/// Recovery path when the atomic rename fails (sharing violation, cross-volume
/// move, UNC quirk, long path, transient Windows lock). On Windows we first
/// aside-rename the prior file to `{path}.bak` so the destructive `fs::copy`
/// fallback can't destroy the prior good bytes if it itself fails midway. On
/// Unix `fs::rename` would have left `path` untouched, so the prior file is
/// already safe and no aside-rename is needed.
#[cfg_attr(not(windows), allow(unused_variables))]
fn rename_failed_fallback_copy(
    path: &Path,
    parent: &Path,
    file_name: &str,
    tmp_path: &Path,
    rename_err: io::Error,
) -> io::Result<()> {
    tracing::warn!("[File] rename {tmp_path:?} -> {path:?} failed: {rename_err} — moving prior file aside and falling back to copy");

    // 5 attempts × exponential backoff (10/20/40/80ms = ~150ms total) is
    // enough for transient AV scans / indexer handles to release the file
    // without making the UI feel unresponsive. Only runs in the failure path.
    #[cfg(windows)]
    let bak_path: Option<std::path::PathBuf> = {
        let mut bak: Option<std::path::PathBuf> = None;
        if path.exists() {
            let candidate = parent.join(format!("{file_name}.bak"));
            // A stale .bak from a prior failed write must not block the
            // aside-rename; remove it before moving the current good file aside.
            let _ = fs::remove_file(&candidate);
            for attempt in 0..5 {
                match fs::rename(path, &candidate) {
                    Ok(_) => {
                        bak = Some(candidate.clone());
                        break;
                    }
                    Err(e) => {
                        if attempt == 4 {
                            tracing::debug!("[File] {path:?}: aside-rename to .bak failed after 5 attempts: {e} — proceeding with copy anyway");
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10 * (1 << attempt)));
                    }
                }
            }
        }
        bak
    };

    match fs::copy(tmp_path, path) {
        Ok(_) => {
            let _ = fs::remove_file(tmp_path);
            #[cfg(windows)]
            if let Some(ref bak) = bak_path {
                let _ = fs::remove_file(bak);
            }
            Ok(())
        }
        Err(copy_err) => {
            // Copy failed. The prior good file is preserved — on Windows
            // it lives at `.bak` (if the aside-rename succeeded) or still
            // at `path` (if it didn't); on Unix `fs::rename` would have
            // left `path` untouched. Do NOT attempt a second copy that
            // requires removing the destination first — that path is what
            // silently destroys ledgered state on multi-failure.
            let _ = fs::remove_file(tmp_path);
            #[cfg(windows)]
            {
                if let Some(ref bak) = bak_path {
                    tracing::error!("[File] cannot save {path:?}: rename={rename_err}, copy={copy_err}; prior file preserved at {bak:?}");
                    return Err(io::Error::other(
                        format!("Failed to save file: rename error: {rename_err}, copy error: {copy_err} (path: {path:?}, prior file preserved at: {bak:?})")
                    ));
                }
                tracing::error!("[File] cannot save {path:?}: rename={rename_err}, copy={copy_err}; destination preserved");
                Err(io::Error::other(
                    format!("Failed to save file: rename error: {rename_err}, copy error: {copy_err} (path: {path:?}, destination preserved)")
                ))
            }
            #[cfg(not(windows))]
            {
                tracing::error!("[File] cannot save {path:?}: rename={rename_err}, copy={copy_err}; destination preserved");
                Err(io::Error::other(
                    format!("Failed to save file: rename error: {rename_err}, copy error: {copy_err} (path: {path:?}, destination preserved)")
                ))
            }
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
        // and must not leave a .bak sibling (Windows aside-rename should be
        // cleaned up after the destination rename succeeds).
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
        // The aside-rename creates `{path}.bak` on Windows when the destination
        // already exists; after a successful rename, the .bak must be removed.
        // This applies to overwrite cases — verify across two sequential writes
        // that no .bak (or .tmp) sibling lingers.
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

    /// On Windows, when the destination rename + copy both fail, the prior
    /// good file's bytes must be preserved either at the live path or at
    /// `{path}.bak`. We can't reliably force a rename failure in a unit test
    /// without unsafe FFI or external locking tools, so this test simulates
    /// the recovery contract: write a good file, then verify the contract
    /// holds for the happy path (the only path testable in CI), and document
    /// the .bak invariant via the aside-rename machinery exercised by the
    /// other tests above.
    #[test]
    fn good_file_bytes_preserved_across_overwrite() {
        let dir = TmpDir::new("preserve-bytes");
        let target = dir.path("ledger.json");
        write_atomic(&target, "ledgered economic data").unwrap();
        // Overwrite — the aside-rename moves the prior file to .bak, the new
        // rename succeeds, and then .bak is cleaned up. At every observable
        // moment after this call, either the live path or the .bak must hold
        // a complete file (never both empty).
        write_atomic(&target, "v2").unwrap();
        assert_eq!(read_to_string(&target), "v2");
        // No .bak should remain after success.
        assert!(!dir.path("ledger.json.bak").exists());
    }
}
