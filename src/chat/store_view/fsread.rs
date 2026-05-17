//! Atomic-rename-safe JSON read helper for chat-side store views.
//!
//! Why this exists: the trade bot writes every JSON file under `data/`
//! via `write_atomic` (tmp-file + rename). A chat-side read can race
//! with that rename window in two ways:
//!   - the read lands after the destination is removed but before the
//!     rename completes, returning `NotFound`;
//!   - the read returns successfully but with bytes from the
//!     half-replaced file, so `serde_json::from_str` fails.
//!
//! Both shapes are absorbed by a single sleepless re-read: by the
//! second attempt the rename has settled on any non-pathological disk.
//!
//! Centralizing this here means the retry/log policy lives in exactly
//! one place — adding (say) a small backoff, a structured event, or a
//! third attempt only has to happen here.
//!
//! `NotFound` is treated as a clean miss and returns `None` silently
//! (it's the normal "no such user / no such pair" path). Other I/O
//! errors get a `warn!` log; parse errors after the retry also `warn!`.

use std::path::Path;

use serde::de::DeserializeOwned;

/// Read `path`, deserialize as `T`, with one rename-window retry.
///
/// Returns `None` for missing files (silent) and for I/O or parse
/// errors after the retry (logged at `warn`).
pub(crate) fn read_json_with_atomic_retry<T: DeserializeOwned>(path: &Path) -> Option<T> {
    for attempt in 0..2 {
        match std::fs::read_to_string(path) {
            Ok(body) => match serde_json::from_str::<T>(&body) {
                Ok(v) => return Some(v),
                Err(_) if attempt == 0 => continue,
                Err(e) => {
                    tracing::warn!(
                        "[chat/fsread] parse error after retry on {}: {e}",
                        path.display()
                    );
                    return None;
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(_) if attempt == 0 => continue,
            Err(e) => {
                tracing::warn!("[chat/fsread] io error on {}: {e}", path.display());
                return None;
            }
        }
    }
    None
}
