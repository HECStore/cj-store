//! Utility functions for the Store

use tokio::sync::oneshot;
use tracing::debug;

use crate::messages::BotInstruction;
use crate::types::User;
use super::Store;

/// Ensure user exists in store, creating if missing.
///
/// UUIDs are the canonical identity key (usernames can change), so we look up
/// by UUID and only update the stored username when it has drifted. Marks the
/// store dirty on any mutation so the change is persisted on the next flush.
pub fn ensure_user_exists(store: &mut Store, username: &str, uuid: &str) {
    if !store.users.contains_key(uuid) {
        store.users.insert(
            uuid.to_string(),
            User {
                uuid: uuid.to_string(),
                username: username.to_string(),
                balance: 0.0,
                operator: false,
            },
        );
        store.dirty = true;
        debug!(uuid = uuid, username = username, "Created new user record");
    } else if let Some(user) = store.users.get_mut(uuid)
        && user.username != username {
            let old = std::mem::replace(&mut user.username, username.to_string());
            store.dirty = true;
            debug!(
                uuid = uuid,
                old_username = %old,
                new_username = username,
                "Updated user's changed username"
            );
        }
}

/// Returns true iff the user with `user_uuid` exists and has the operator flag set.
pub fn is_operator(store: &Store, user_uuid: &str) -> bool {
    store.users.get(user_uuid).is_some_and(|u| u.operator)
}

/// Get node position for a given chest_id.
///
/// Each node holds `CHESTS_PER_NODE` chests, so the node id is
/// `chest_id / CHESTS_PER_NODE`. If the node isn't materialized in
/// `storage.nodes` yet, we deterministically recompute its position from the
/// storage origin so callers always get a valid location.
pub fn get_node_position(store: &Store, chest_id: i32) -> crate::types::Position {
    let node_id = chest_id / crate::constants::CHESTS_PER_NODE as i32;
    store.storage.nodes.iter()
        .find(|n| n.id == node_id)
        .map(|n| n.position)
        .unwrap_or_else(|| {
            crate::types::Node::calc_position(node_id, &store.storage.position)
        })
}

/// Send a message to a player via bot whisper.
///
/// Uses a oneshot channel so we can await the bot's acknowledgement and
/// surface send failures (bot disconnected, channel closed) back to the caller
/// instead of silently dropping the message.
///
/// Returns a typed `StoreError` so callers can match on the failure kind
/// (e.g. retry on `BotDisconnected`, escalate on other variants).
pub async fn send_message_to_player(
    store: &Store,
    player_name: &str,
    message: &str,
) -> Result<(), crate::error::StoreError> {
    debug!(player = player_name, message = message, "Whispering to player");
    let (tx, rx) = oneshot::channel();
    store
        .bot_tx
        .send(BotInstruction::Whisper {
            target: player_name.to_string(),
            message: message.to_string(),
            respond_to: tx,
        })
        .await
        .map_err(|_| crate::error::StoreError::BotDisconnected)?;

    rx.await
        .map_err(|_| crate::error::StoreError::BotDisconnected)?
        .map_err(crate::error::StoreError::BotError)
}

/// Helper to format transfer summaries (excludes coordinates for security).
///
/// Player-facing output must NEVER leak chest coordinates: exposing them would
/// let customers (or griefers) locate and bypass the storage system directly.
/// Only item + amount pairs are included; long lists are truncated with a
/// "(+N more)" suffix to keep whispers within Minecraft's chat limits.
pub fn summarize_transfers(transfers: &[crate::types::storage::ChestTransfer], max: usize) -> String {
    if transfers.is_empty() {
        return "none".to_string();
    }

    let mut parts: Vec<String> = Vec::new();
    for t in transfers.iter().take(max) {
        // Intentionally omit t.position — coordinates must not leak to players.
        parts.push(format!("{}x {}", t.amount, t.item));
    }

    if transfers.len() > max {
        parts.push(format!("(+{} more)", transfers.len() - max));
    }

    parts.join("; ")
}

/// Helper to format issue lists.
///
/// Produces a compact "prefix: a; b; c (+N more)" string for operator reports
/// and whispers. The `max` cap prevents one pathological operation from
/// flooding chat when many issues accumulate.
pub fn fmt_issues(prefix: &str, issues: &[String], max: usize) -> String {
    if issues.is_empty() {
        return prefix.to_string();
    }
    let mut out = String::new();
    out.push_str(prefix);
    out.push_str(": ");
    for (i, s) in issues.iter().take(max).enumerate() {
        if i > 0 {
            out.push_str("; ");
        }
        out.push_str(s);
    }
    if issues.len() > max {
        out.push_str(&format!(" (+{} more)", issues.len() - max));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    #[test]
    fn fmt_issues_empty_returns_bare_prefix() {
        assert_eq!(fmt_issues("Errors", &[], 5), "Errors");
    }

    #[test]
    fn fmt_issues_single_item_separates_with_colon() {
        assert_eq!(
            fmt_issues("Errors", &["Issue 1".to_string()], 5),
            "Errors: Issue 1"
        );
    }

    #[test]
    fn fmt_issues_joins_multiple_items_with_semicolons() {
        assert_eq!(
            fmt_issues("Errors", &["A".to_string(), "B".to_string()], 5),
            "Errors: A; B"
        );
    }

    #[test]
    fn fmt_issues_at_max_does_not_append_more_suffix() {
        // Exactly `max` issues: no truncation suffix
        let issues: Vec<String> = (1..=3).map(|i| format!("I{}", i)).collect();
        assert_eq!(fmt_issues("E", &issues, 3), "E: I1; I2; I3");
    }

    #[test]
    fn fmt_issues_truncates_with_more_suffix_above_max() {
        let issues: Vec<String> = (1..=10).map(|i| format!("Issue {}", i)).collect();
        let result = fmt_issues("Errors", &issues, 3);
        assert_eq!(result, "Errors: Issue 1; Issue 2; Issue 3 (+7 more)");
    }

    #[test]
    fn summarize_transfers_empty_returns_none_literal() {
        assert_eq!(summarize_transfers(&[], 5), "none");
    }

    #[test]
    fn summarize_transfers_formats_single_entry_without_coords() {
        use crate::types::storage::ChestTransfer;
        use crate::types::Position;

        let transfers = vec![ChestTransfer {
            chest_id: 0,
            item: crate::types::ItemId::new("diamond").unwrap(),
            amount: 64,
            position: Position { x: 123, y: 64, z: -456 },
        }];
        let s = summarize_transfers(&transfers, 5);
        assert_eq!(s, "64x diamond");
        // Security invariant: coordinates must never appear in the summary
        assert!(!s.contains("123"));
        assert!(!s.contains("-456"));
    }

    #[test]
    fn summarize_transfers_joins_multiple_with_semicolons() {
        use crate::types::storage::ChestTransfer;
        use crate::types::Position;

        let transfers = vec![
            ChestTransfer {
                chest_id: 0,
                item: crate::types::ItemId::new("diamond").unwrap(),
                amount: 64,
                position: Position::default(),
            },
            ChestTransfer {
                chest_id: 1,
                item: crate::types::ItemId::new("iron_ingot").unwrap(),
                amount: 128,
                position: Position::default(),
            },
        ];
        assert_eq!(
            summarize_transfers(&transfers, 5),
            "64x diamond; 128x iron_ingot"
        );
    }

    #[test]
    fn summarize_transfers_truncates_above_max_with_more_suffix() {
        use crate::types::storage::ChestTransfer;
        use crate::types::Position;

        let transfers: Vec<ChestTransfer> = (0..5)
            .map(|i| ChestTransfer {
                chest_id: i,
                item: crate::types::ItemId::new("stone").unwrap(),
                amount: 1,
                position: Position::default(),
            })
            .collect();
        let s = summarize_transfers(&transfers, 2);
        assert_eq!(s, "1x stone; 1x stone; (+3 more)");
    }

    // ------------------------------------------------------------------------
    // ensure_user_exists / is_operator / get_node_position each need a Store.
    // Use `Store::new_for_test` to bypass disk I/O.
    // ------------------------------------------------------------------------
    fn test_store() -> Store {
        let (tx, _rx) = mpsc::channel::<BotInstruction>(1);
        let config = crate::config::Config {
            position: crate::types::Position { x: 0, y: 64, z: 0 },
            fee: 0.125,
            account_email: String::new(),
            server_address: "test".to_string(),
            buffer_chest_position: None,
            trade_timeout_ms: 5_000,
            pathfinding_timeout_ms: 5_000,
            max_orders: 1000,
            max_trades_in_memory: 1000,
            autosave_interval_secs: 10,
            chat: crate::config::ChatConfig::default(),
        };
        Store::new_for_test(tx, config, HashMap::new(), HashMap::new(), crate::types::Storage::default())
    }

    #[test]
    fn ensure_user_exists_creates_new_user_and_marks_dirty() {
        let mut store = test_store();
        ensure_user_exists(&mut store, "Alice", "uuid-a");
        let u = store.users.get("uuid-a").expect("user inserted");
        assert_eq!(u.username, "Alice");
        assert_eq!(u.balance, 0.0);
        assert!(!u.operator);
        assert!(store.dirty);
    }

    #[test]
    fn ensure_user_exists_updates_username_on_drift_and_marks_dirty() {
        let mut store = test_store();
        ensure_user_exists(&mut store, "Alice", "uuid-a");
        store.dirty = false;

        ensure_user_exists(&mut store, "AliceRenamed", "uuid-a");
        assert_eq!(store.users.get("uuid-a").unwrap().username, "AliceRenamed");
        assert!(store.dirty);
    }

    #[test]
    fn ensure_user_exists_is_noop_when_username_matches() {
        let mut store = test_store();
        ensure_user_exists(&mut store, "Alice", "uuid-a");
        store.dirty = false;

        ensure_user_exists(&mut store, "Alice", "uuid-a");
        assert!(!store.dirty, "no change should not mark dirty");
    }

    #[test]
    fn is_operator_returns_false_for_unknown_uuid() {
        let store = test_store();
        assert!(!is_operator(&store, "missing"));
    }

    #[test]
    fn is_operator_returns_false_for_regular_user() {
        let mut store = test_store();
        ensure_user_exists(&mut store, "Alice", "uuid-a");
        assert!(!is_operator(&store, "uuid-a"));
    }

    #[test]
    fn is_operator_returns_true_when_operator_flag_set() {
        let mut store = test_store();
        ensure_user_exists(&mut store, "Alice", "uuid-a");
        store.users.get_mut("uuid-a").unwrap().operator = true;
        assert!(is_operator(&store, "uuid-a"));
    }

    #[test]
    fn get_node_position_falls_back_to_calc_when_node_absent() {
        let store = test_store();
        // Storage has no nodes; fallback must equal Node::calc_position for
        // the derived node_id (chest_id / CHESTS_PER_NODE).
        let chest_id = 5; // node_id = 5 / 4 = 1
        let pos = get_node_position(&store, chest_id);
        let expected = crate::types::Node::calc_position(
            chest_id / crate::constants::CHESTS_PER_NODE as i32,
            &store.storage.position,
        );
        assert_eq!(pos, expected);
    }

    #[test]
    fn get_node_position_uses_materialized_node_when_present() {
        use crate::types::Position;
        let mut store = test_store();
        let explicit = Position { x: 999, y: 64, z: -999 };
        store.storage.nodes.push(crate::types::Node {
            id: 1,
            position: explicit,
            chests: Vec::new(),
        });
        // chest_id 5 -> node_id 1 -> should pick up the materialized node,
        // not the calc_position fallback.
        assert_eq!(get_node_position(&store, 5), explicit);
    }
}
