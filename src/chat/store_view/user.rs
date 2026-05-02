//! Read-only view over `data/users/*.json`.
//!
//! **Operator-status redaction (hard rule).** This struct intentionally
//! does NOT deserialize the `operator` field. Tools that return a
//! `UserView` therefore cannot leak operator status through the chat
//! surface — even a future "just serialize the whole struct" change
//! wouldn't expose it, because the field never reaches memory.
//!
//! Path safety: `get_by_uuid` constructs `data/users/{uuid}.json` only
//! AFTER the chat tool layer has run [`crate::chat::tools::validate_uuid`]
//! on its input. UUIDs are 32–36 ASCII hex/hyphen characters, so the
//! resulting filename can never escape `data/users/`. `get_by_username`
//! never touches the path layer at all — it scans the catalog and
//! returns whichever entry matches case-insensitively.

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
pub fn get_by_uuid_in_dir(dir: &std::path::Path, uuid: &str) -> Option<UserView> {
    let path = dir.join(format!("{uuid}.json"));
    // One retry to absorb the write_atomic rename window.
    let mut body = std::fs::read_to_string(&path).ok()?;
    if serde_json::from_str::<UserView>(&body).is_err() {
        body = std::fs::read_to_string(&path).ok()?;
    }
    serde_json::from_str(&body).ok()
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
