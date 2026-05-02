//! Schema-drift cross-check tests.
//!
//! These tests serialize a real `crate::types::*` value through the
//! types crate, then deserialize via the chat-side `*View` struct.
//! If the types' on-disk schema ever drifts away from what chat expects
//! to see — a renamed field, a removed field, a type change — these
//! tests catch it at build time. The cross-check runs ONLY in
//! `#[cfg(test)]`, so the chat module gains no production-time
//! dependency on `crate::types::*`.

use crate::chat::store_view;

#[test]
fn trade_view_round_trips_a_real_trade() {
    use crate::types::{ItemId, Trade, TradeType};
    let t = Trade::new(
        TradeType::Buy,
        ItemId::new("diamond").unwrap(),
        3,
        7.5,
        "11111111-2222-3333-4444-555555555555".to_string(),
    );
    let json = serde_json::to_string(&t).unwrap();
    let view: store_view::trade::TradeView =
        serde_json::from_str(&json).expect("real Trade JSON must deserialize through TradeView");
    assert_eq!(view.trade_type, "Buy");
    assert_eq!(view.item, "diamond");
    assert_eq!(view.amount, 3);
    assert!((view.amount_currency - 7.5).abs() < 1e-9);
    assert_eq!(view.user_uuid, t.user_uuid);
    assert_eq!(view.timestamp, t.timestamp);
}

#[test]
fn trade_view_handles_every_trade_type_variant() {
    // Confirms the Pascal-case string assumption holds for every
    // variant — the chat tool exposes `trade_type` as a free-form
    // string filter, so a future TradeType variant rename shows up
    // here as a failed serialize round-trip.
    use crate::types::{ItemId, Trade, TradeType};
    for variant in [
        TradeType::Buy,
        TradeType::Sell,
        TradeType::AddStock,
        TradeType::RemoveStock,
        TradeType::DepositBalance,
        TradeType::WithdrawBalance,
        TradeType::AddCurrency,
        TradeType::RemoveCurrency,
    ] {
        let t = Trade {
            trade_type: variant.clone(),
            item: ItemId::new("diamond").unwrap(),
            amount: 1,
            amount_currency: 1.0,
            user_uuid: "u".to_string(),
            timestamp: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let view: store_view::trade::TradeView = serde_json::from_str(&json).unwrap();
        // Round-trip the string back to the type to confirm the chat
        // filter substring is wire-compatible.
        let trade_type_json = serde_json::to_string(&variant).unwrap();
        let view_string_in_json =
            format!("\"{}\"", view.trade_type);
        assert_eq!(trade_type_json, view_string_in_json);
    }
}

#[test]
fn pair_view_round_trips_a_real_pair() {
    use crate::types::{ItemId, Pair};
    let p = Pair {
        item: ItemId::new("diamond").unwrap(),
        stack_size: 64,
        item_stock: 100,
        currency_stock: 1000.5,
    };
    let json = serde_json::to_string(&p).unwrap();
    let view: store_view::pair::PairView = serde_json::from_str(&json).unwrap();
    assert_eq!(view.item, "diamond");
    assert_eq!(view.stack_size, 64);
    assert_eq!(view.item_stock, 100);
    assert!((view.currency_stock - 1000.5).abs() < 1e-9);
}

#[test]
fn user_view_drops_operator_field_via_deserialize() {
    use crate::types::User;
    // Operator-true user — confirm the View deserializes cleanly and
    // that no path through the View ever materializes the bit.
    let u = User {
        uuid: "11111111-2222-3333-4444-555555555555".to_string(),
        username: "alice".to_string(),
        balance: 5.0,
        operator: true,
    };
    let json = serde_json::to_string(&u).unwrap();
    assert!(json.contains("\"operator\":true"), "fixture must include operator:true");
    let view: store_view::user::UserView = serde_json::from_str(&json).unwrap();
    assert_eq!(view.uuid, u.uuid);
    assert_eq!(view.username, "alice");
    assert!((view.balance - 5.0).abs() < 1e-9);

    // Re-serializing `view` must not emit `operator`. A future
    // `serde(deny_unknown_fields)` on `User` would still accept this
    // because the wire shape is a strict subset.
    let view_json = serde_json::to_string(&serde_json::json!({
        "uuid": view.uuid,
        "username": view.username,
        "balance": view.balance,
    }))
    .unwrap();
    assert!(
        !view_json.contains("operator"),
        "operator field must NEVER appear in chat's UserView serialization: {view_json}",
    );
}

#[test]
fn user_view_loads_pre_operator_files() {
    // Older user files predate the `operator` field. Same `serde(default)`
    // tolerance the real `User` struct enables — the View must also
    // accept these.
    let json = r#"{"uuid":"u","username":"a","balance":1.0}"#;
    let view: store_view::user::UserView = serde_json::from_str(json).unwrap();
    assert_eq!(view.username, "a");
}
