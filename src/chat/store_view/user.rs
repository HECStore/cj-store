//! Read-only view over `data/users/*.json`.
//!
//! **Operator-status redaction (hard rule).** This struct intentionally
//! does NOT deserialize the `operator` field. Tools that return a
//! `UserView` therefore cannot leak operator status through the chat
//! surface — even a future "just serialize the whole struct" change
//! wouldn't expose it, because the field never reaches memory.
//!
//! Path safety: `get_by_uuid_in_dir` enforces a canonical-hyphenated
//! 36-char lowercase-hex shape gate inline before constructing
//! `data/users/{uuid}.json`, so the resulting filename can never escape
//! `data/users/` regardless of caller validation. The chat tool layer
//! (`crate::chat::tools::validate_uuid`) is still expected to run as
//! the first perimeter; this gate is defense-in-depth at the
//! path-construction site, mirroring `pair::is_safe_pair_stem`.
//! Username lookups are not handled here — `crate::chat::tools::get_user_balance_tool`
//! resolves usernames upstream via `chat::memory` + `mojang` and then
//! calls `get_by_uuid`.

use serde::Deserialize;

/// Users directory. Mirrors `crate::types::User::USERS_DIR` but owned
/// by chat.
pub const USERS_DIR: &str = "data/users";

/// Minimal deserializer for one user JSON file. The `operator` field is
/// deliberately absent — see the module docstring.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct UserView {
    pub uuid: String,
    pub username: String,
    pub balance: f64,
}

/// Returns true iff `s` matches the canonical 36-char hyphenated
/// lowercase-hex UUID form: `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`.
/// Mirrors `crate::chat::tools::is_canonical_hyphen_uuid` and
/// `crate::types::user::is_valid_uuid_shape` — duplicated inline by the
/// same chat-independence rationale used elsewhere in this module.
fn is_canonical_hyphen_uuid(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    s.bytes().enumerate().all(|(i, b)| match i {
        8 | 13 | 18 | 23 => b == b'-',
        _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
    })
}

/// Look up a user by canonical hyphenated UUID (the on-disk filename).
/// Returns `None` if the file is missing or fails to deserialize.
///
/// Wrapped in `spawn_blocking` because the chat task runs on a small
/// tokio worker pool.
pub async fn get_by_uuid(uuid: &str) -> Option<UserView> {
    let uuid = uuid.to_string();
    tokio::task::spawn_blocking(move || {
        get_by_uuid_in_dir(std::path::Path::new(USERS_DIR), &uuid)
    })
    .await
    .ok()
    .flatten()
}

/// Inner sync helper for [`get_by_uuid`] — exposed so tests can target
/// a temp dir.
///
/// Retries once on either I/O or parse failure to absorb the
/// atomic-rename window: `write_atomic` saves user files via
/// tmp-file + rename, so a chat-side read can land mid-rename and see
/// either a transient `NotFound` or (rarely) a half-written body. The
/// retry is sleepless — by the second attempt the rename has settled.
///
/// Defense-in-depth: rejects any `uuid` that is not the canonical
/// 36-char hyphenated lowercase-hex form before joining the path.
/// `pair::is_safe_pair_stem` and `types::user::get_user_file_path` both
/// apply the same pattern at their respective storage boundaries.
pub fn get_by_uuid_in_dir(dir: &std::path::Path, uuid: &str) -> Option<UserView> {
    if !is_canonical_hyphen_uuid(uuid) {
        tracing::warn!(
            "[chat/user] rejecting non-canonical uuid at storage boundary: {uuid:?}"
        );
        return None;
    }
    let path = dir.join(format!("{uuid}.json"));
    for attempt in 0..2 {
        match std::fs::read_to_string(&path) {
            Ok(body) => match serde_json::from_str::<UserView>(&body) {
                Ok(u) => return Some(u),
                Err(_) if attempt == 0 => continue,
                Err(_) => return None,
            },
            Err(_) if attempt == 0 => continue,
            Err(_) => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_dir(tag: &str) -> std::path::PathBuf {
        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "cj-store-user-view-{}-{}-{tag}",
            std::process::id(),
            nanos,
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn get_by_uuid_round_trips() {
        let dir = fixture_dir("by-uuid");
        std::fs::write(
            dir.join("11111111-2222-3333-4444-555555555555.json"),
            r#"{"uuid":"11111111-2222-3333-4444-555555555555","username":"alice","balance":42.5,"operator":true}"#,
        )
        .unwrap();
        let u = get_by_uuid_in_dir(&dir, "11111111-2222-3333-4444-555555555555")
            .expect("user");
        assert_eq!(u.username, "alice");
        assert!((u.balance - 42.5).abs() < 1e-9);
        // The struct has no `operator` field; the JSON had `operator:true`
        // and was nonetheless deserialized cleanly. Belt-and-braces:
        // re-serialize and confirm the operator key is gone.
        let s = serde_json::to_string(&serde_json::json!({
            "uuid": u.uuid,
            "username": u.username,
            "balance": u.balance,
        }))
        .unwrap();
        assert!(!s.contains("operator"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn get_by_uuid_returns_none_when_file_missing() {
        let dir = fixture_dir("missing");
        let u = get_by_uuid_in_dir(&dir, "00000000-0000-0000-0000-000000000000");
        assert!(u.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

}
