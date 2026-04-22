//! # File System Utilities
//!
//! Provides atomic file write operations to prevent torn writes during crashes.
//!
//! **Strategy**: Write to `*.tmp`, then rename to final file.
//! On Windows, rename won't overwrite, so we remove destination first.

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
/// **Atomicity**: This is "best-effort" - not crash-proof in all edge cases,
/// but prevents torn writes in normal operation. For true atomicity, consider
/// using platform-specific APIs (e.g., `CreateFile` with `FILE_FLAG_WRITE_THROUGH` on Windows).
///
/// **Used By**: All JSON persistence operations (users, pairs, orders, trades, nodes, queue, config).
/// This ensures state files are never left in a corrupted state.
pub fn write_atomic(path: impl AsRef<Path>, contents: &str) -> io::Result<()> {
    let path = path.as_ref();
    tracing::debug!("[File] write_atomic: Starting write operation for {:?}", path);
    
    // Ensure we have a valid file name.
    // `file_name()` returns None for `.` and `..`, and the inner `to_str()` can
    // reject non-UTF-8 names on Unix; both cases produce `InvalidInput` so the
    // caller sees the real reason rather than a confusing later error.
    let file_name = path.file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Invalid file path: {:?}", path)
        ))?;
    
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    tracing::debug!("[File] write_atomic: Creating parent directory if needed: {:?}", parent);
    fs::create_dir_all(parent)?;
    tracing::debug!("[File] write_atomic: Parent directory ready");

    let tmp_name = format!("{}.tmp", file_name);
    let tmp_path = parent.join(&tmp_name);
    tracing::debug!("[File] write_atomic: Temp file path: {:?}", tmp_path);
    
    // Validate paths are valid (not empty, no invalid characters on Windows)
    let path_str = path.to_string_lossy();
    let tmp_path_str = tmp_path.to_string_lossy();
    if path_str.is_empty() || tmp_path_str.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Empty path: path={:?}, tmp_path={:?}", path, tmp_path)
        ));
    }

    // Write to temp file with explicit sync to ensure it's fully written
    // This is especially important on Windows to avoid file handle issues
    tracing::debug!("[File] write_atomic: Writing {} bytes to temp file", contents.len());
    {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(contents.as_bytes())?;
        tracing::debug!("[File] write_atomic: Data written, syncing to disk");
        file.sync_all()?;
        tracing::debug!("[File] write_atomic: File synced, closing handle");
        // File handle is closed here when it goes out of scope
    }
    tracing::debug!("[File] write_atomic: Temp file handle closed");
    
    // Verify temp file was created successfully
    if !tmp_path.exists() {
        return Err(io::Error::other(
            format!("Temp file was not created: {:?}", tmp_path)
        ));
    }
    tracing::debug!("[File] write_atomic: Temp file verified to exist");

    // On Windows, rename won't overwrite an existing file, so we must remove it first.
    // If the file is locked (e.g., by antivirus or another process), we retry a few times.
    let dest_exists = path.exists();
    tracing::debug!("[File] write_atomic: Destination file exists: {}", dest_exists);
    if dest_exists {
        tracing::debug!("[File] write_atomic: Attempting to remove existing destination file");
        // Try to remove the existing file, with retries for Windows file locking issues.
        // 5 attempts with exponential backoff gives ~150ms total wait (10+20+40+80),
        // which is enough for transient AV scans / indexer handles to release the file
        // without making the UI feel unresponsive.
        for attempt in 0..5 {
            match fs::remove_file(path) {
                Ok(_) => {
                    tracing::debug!("[File] write_atomic: Existing file removed successfully (attempt {})", attempt + 1);
                    break;
                }
                Err(e) => {
                    tracing::debug!("[File] write_atomic: Failed to remove existing file (attempt {}): {}", attempt + 1, e);
                    if attempt == 4 {
                        // Last attempt failed, try rename anyway (might work if file was just closed).
                        // We don't return an error here because the rename/copy fallback below
                        // may still succeed, and if not, it produces a more informative error.
                        tracing::debug!("[File] write_atomic: All remove attempts failed, will try rename anyway");
                        break;
                    }
                    // Wait a bit before retrying (exponential backoff: 10ms, 20ms, 40ms, 80ms)
                    std::thread::sleep(std::time::Duration::from_millis(10 * (1 << attempt)));
                }
            }
        }
    }

    // Attempt rename - on Windows this will fail if the destination still exists
    // In that case, try one more remove and rename
    tracing::debug!("[File] write_atomic: Attempting to rename temp file to destination");
    match fs::rename(&tmp_path, path) {
        Ok(_) => {
            tracing::debug!("[File] write_atomic: Rename successful");
            Ok(())
        },
        Err(e) => {
            tracing::warn!("[File] write_atomic: Rename failed: {} (path: {:?}, tmp_path: {:?})", e, path, tmp_path);
            // If rename failed, try fallback: copy + remove.
            // This handles cases where rename fails due to Windows path issues
            // (e.g., cross-volume moves, or quirks with UNC / long paths).
            // Copy loses the atomicity guarantee, but it's preferable to losing the write entirely.
            tracing::debug!("[File] write_atomic: Trying copy fallback");
            match fs::copy(&tmp_path, path) {
                Ok(_) => {
                    tracing::debug!("[File] write_atomic: Copy succeeded, removing temp file");
                    // Copy succeeded, remove temp file
                    let _ = fs::remove_file(&tmp_path);
                    tracing::debug!("[File] write_atomic: Copy fallback successful");
                    Ok(())
                }
                Err(copy_err) => {
                    tracing::warn!("[File] write_atomic: Copy also failed: {} (path: {:?})", copy_err, path);
                    // Copy also failed - try one more time to remove existing file and copy
                    if path.exists() {
                        tracing::debug!("[File] write_atomic: Destination still exists, trying to remove before copy");
                        // Try to remove the existing file
                        if let Err(remove_err) = fs::remove_file(path) {
                            tracing::error!("[File] write_atomic: Failed to remove existing file: {}", remove_err);
                            // If remove failed, clean up temp file and return error
                            let _ = fs::remove_file(&tmp_path);
                            return Err(io::Error::other(
                                format!("Failed to save file: rename error: {}, copy error: {}, remove error: {} (path: {:?})", e, copy_err, remove_err, path)
                            ));
                        }
                        tracing::debug!("[File] write_atomic: Existing file removed, trying copy again");
                        // Try copy again after removal
                        match fs::copy(&tmp_path, path) {
                            Ok(_) => {
                                tracing::debug!("[File] write_atomic: Copy after removal succeeded");
                                let _ = fs::remove_file(&tmp_path);
                                Ok(())
                            }
                            Err(final_copy_err) => {
                                tracing::error!("[File] write_atomic: Final copy attempt failed: {}", final_copy_err);
                                // Clean up temp file on final failure
                                let _ = fs::remove_file(&tmp_path);
                                Err(io::Error::other(
                                    format!("Failed to save file after removing existing: rename error: {}, copy error: {}, final copy error: {} (path: {:?}, tmp_path: {:?})", e, copy_err, final_copy_err, path, tmp_path)
                                ))
                            }
                        }
                    } else {
                        tracing::error!("[File] write_atomic: Destination doesn't exist but both rename and copy failed");
                        // Path doesn't exist, but both rename and copy failed
                        let _ = fs::remove_file(&tmp_path);
                        Err(io::Error::other(
                            format!("Failed to save file (destination doesn't exist): rename error: {}, copy error: {} (path: {:?}, tmp_path: {:?})", e, copy_err, path, tmp_path)
                        ))
                    }
                }
            }
        }
    }
}

// ============================================================================
// TESTS
// ============================================================================

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
