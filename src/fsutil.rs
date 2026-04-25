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
/// 1. Write contents to `{filename}.tmp`
/// 2. Remove destination file if it exists (Windows requirement)
/// 3. Rename temp file to final name
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
    fs::create_dir_all(parent)?;

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

    // On Windows, rename won't overwrite an existing file, so we must remove it first.
    // 5 attempts with exponential backoff gives ~150ms total wait (10+20+40+80),
    // which is enough for transient AV scans / indexer handles to release the file
    // without making the UI feel unresponsive.
    // On Unix, `fs::rename` already atomically replaces an existing file, so
    // pre-removing the destination would needlessly create a window where a
    // concurrent reader sees `NotFound` instead of the old-or-new contents.
    #[cfg(windows)]
    if path.exists() {
        for attempt in 0..5 {
            match fs::remove_file(path) {
                Ok(_) => break,
                Err(e) => {
                    if attempt == 4 {
                        // All remove attempts failed — fall through to rename,
                        // which may still succeed (handle could have just closed),
                        // and if it doesn't the copy fallback produces a better error.
                        tracing::debug!("[File] {path:?}: remove failed after 5 attempts: {e} — trying rename anyway");
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10 * (1 << attempt)));
                }
            }
        }
    }

    match fs::rename(&tmp_path, path) {
        Ok(_) => {
            #[cfg(unix)]
            {
                if let Ok(dir) = fs::File::open(parent) {
                    let _ = dir.sync_all();
                }
            }
            Ok(())
        },
        Err(e) => {
            // Rename can fail for cross-volume moves, UNC quirks, long paths,
            // or Windows locks we couldn't clear. Copy loses atomicity but
            // prevents losing the write entirely.
            tracing::warn!("[File] rename {tmp_path:?} -> {path:?} failed: {e} — falling back to copy");
            match fs::copy(&tmp_path, path) {
                Ok(_) => {
                    let _ = fs::remove_file(&tmp_path);
                    Ok(())
                }
                Err(copy_err) => {
                    if path.exists() {
                        if let Err(remove_err) = fs::remove_file(path) {
                            let _ = fs::remove_file(&tmp_path);
                            tracing::error!("[File] cannot save {path:?}: rename={e}, copy={copy_err}, remove={remove_err}");
                            return Err(io::Error::other(
                                format!("Failed to save file: rename error: {e}, copy error: {copy_err}, remove error: {remove_err} (path: {path:?})")
                            ));
                        }
                        match fs::copy(&tmp_path, path) {
                            Ok(_) => {
                                let _ = fs::remove_file(&tmp_path);
                                Ok(())
                            }
                            Err(final_copy_err) => {
                                let _ = fs::remove_file(&tmp_path);
                                tracing::error!("[File] cannot save {path:?}: rename={e}, copy={copy_err}, final_copy={final_copy_err}");
                                Err(io::Error::other(
                                    format!("Failed to save file after removing existing: rename error: {e}, copy error: {copy_err}, final copy error: {final_copy_err} (path: {path:?}, tmp_path: {tmp_path:?})")
                                ))
                            }
                        }
                    } else {
                        let _ = fs::remove_file(&tmp_path);
                        tracing::error!("[File] cannot save {path:?} (no existing dest): rename={e}, copy={copy_err}");
                        Err(io::Error::other(
                            format!("Failed to save file (destination doesn't exist): rename error: {e}, copy error: {copy_err} (path: {path:?}, tmp_path: {tmp_path:?})")
                        ))
                    }
                }
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
        // Regression guard: a successful write must not leave a .tmp sibling.
        // Before the rename the .tmp exists; after, it must be gone (either
        // via rename consuming it, or via explicit cleanup in the copy fallback).
        let dir = TmpDir::new("cleanup");
        let target = dir.path("foo.json");
        write_atomic(&target, "data").unwrap();
        let mut found_tmp = false;
        for entry in fs::read_dir(&dir.0).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().to_string();
            if name.ends_with(".tmp") {
                found_tmp = true;
            }
        }
        assert!(!found_tmp, "unexpected leftover .tmp file in {:?}", dir.0);
    }
}
