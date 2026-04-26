//! Price calculation functions.
//!
//! Constant-product AMM pricing: `x * y = k`, where `x = item_stock` and
//! `y = currency_stock`. Price is not a ratio — it depends on trade size
//! (slippage).
//!
//! Formulas (`Δx = amount`):
//! - Buy cost:    `y * Δx / (x - Δx) * (1 + fee)`
//! - Sell payout: `y * Δx / (x + Δx) * (1 - fee)`
//!
//! `k` is conserved by the base AMM identity and strictly increases with
//! every fee-bearing trade — that is how fee revenue accrues in the pool.

use super::Store;
use crate::constants::{FEE_MIN, FEE_MAX, MIN_RESERVE_FOR_PRICE};

/// Returns `true` iff `fee` is finite and within `[FEE_MIN, FEE_MAX]`.
pub fn validate_fee(fee: f64) -> bool {
    (FEE_MIN..=FEE_MAX).contains(&fee) && fee.is_finite()
}

/// Returns `true` iff reserves are large enough to quote a reliable price.
///
/// Requires `item_stock > 0` and `currency_stock > MIN_RESERVE_FOR_PRICE`.
/// With a tiny `y` the AMM price collapses toward zero; with a tiny `x` a
/// single trade can consume the pool and pushes the `(x - dx)` denominator
/// into floating-point precision loss. Below the threshold the bot refuses
/// to quote rather than emit a garbage price.
pub fn reserves_sufficient(item_stock: i32, currency_stock: f64) -> bool {
    item_stock > 0 && currency_stock > MIN_RESERVE_FOR_PRICE
}

/// Cost in currency to buy `amount` items from `item`'s pair.
///
/// Returns `None` if the item is unknown, reserves are insufficient, fee
/// is invalid, `amount` is non-positive, or `amount >= item_stock`.
/// See [`buy_cost_pure`] for the math.
pub fn calculate_buy_cost(store: &Store, item: &str, amount: i32) -> Option<f64> {
    let pair = store.pairs.get(item)?;
    buy_cost_pure(pair.item_stock, pair.currency_stock, amount, store.config.fee)
}

/// Pure AMM buy-cost math: `y * dx / (x - dx) * (1 + fee)`.
///
/// As `amount` approaches `item_stock`, the denominator shrinks toward zero
/// and cost grows without bound — you cannot drain the pool. The fee is
/// applied on top of the base AMM cost (not added to the reserves formula),
/// so `k` grows slightly on every trade.
///
/// Returns `None` on invalid fee, insufficient reserves, non-positive
/// amount, amount that would drain or exceed the pool, or a non-finite /
/// non-positive result.
///
/// # Example
/// With 1000 currency stock, 100 item stock, and 12.5 % fee, buying 10:
/// base = `1000 * 10 / (100 - 10)` = 111.11; with fee = `111.11 * 1.125` = 125.0.
pub fn buy_cost_pure(item_stock: i32, currency_stock: f64, amount: i32, fee: f64) -> Option<f64> {
    if !validate_fee(fee) {
        tracing::warn!("Invalid fee rate: {}", fee);
        return None;
    }
    if !reserves_sufficient(item_stock, currency_stock) {
        return None;
    }
    if amount <= 0 || amount >= item_stock {
        return None;
    }

    let x = item_stock as f64;
    let y = currency_stock;
    let dx = amount as f64;

    let base_cost = y * dx / (x - dx);
    let cost = base_cost * (1.0 + fee);

    if cost.is_finite() && cost > 0.0 { Some(cost) } else { None }
}

/// Payout in currency for selling `amount` items into `item`'s pair.
///
/// Returns `None` if the item is unknown, reserves are insufficient, fee
/// is invalid, `amount` is non-positive, or the result is not a positive
/// finite number. See [`sell_payout_pure`] for the math.
pub fn calculate_sell_payout(store: &Store, item: &str, amount: i32) -> Option<f64> {
    let pair = store.pairs.get(item)?;
    sell_payout_pure(pair.item_stock, pair.currency_stock, amount, store.config.fee)
}

/// Pure AMM sell-payout math: `y * dx / (x + dx) * (1 - fee)`.
///
/// Unlike [`buy_cost_pure`] there is no cap on `amount`: the seller can
/// dump arbitrarily many items. Payout is bounded above by `y` and
/// exhibits diminishing returns as trade size grows (slippage against the
/// seller). The fee is subtracted from the payout, so the pool retains
/// extra `y` and `k` grows.
pub fn sell_payout_pure(item_stock: i32, currency_stock: f64, amount: i32, fee: f64) -> Option<f64> {
    if !validate_fee(fee) {
        tracing::warn!("Invalid fee rate: {}", fee);
        return None;
    }
    if !reserves_sufficient(item_stock, currency_stock) {
        return None;
    }
    if amount <= 0 {
        return None;
    }

    let x = item_stock as f64;
    let y = currency_stock;
    let dx = amount as f64;

    let base_payout = y * dx / (x + dx);
    let payout = base_payout * (1.0 - fee);

    if payout.is_finite() && payout > 0.0 { Some(payout) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- validate_fee ---------------------------------------------------------

    #[test]
    fn validate_fee_accepts_bounds_and_interior() {
        assert!(validate_fee(FEE_MIN));
        assert!(validate_fee(FEE_MAX));
        assert!(validate_fee(0.125));
        assert!(validate_fee(0.5));
    }

    #[test]
    fn validate_fee_rejects_out_of_range() {
        assert!(!validate_fee(-f64::MIN_POSITIVE));
        assert!(!validate_fee(-0.1));
        assert!(!validate_fee(FEE_MAX + f64::EPSILON));
        assert!(!validate_fee(1.1));
    }

    #[test]
    fn validate_fee_rejects_non_finite() {
        assert!(!validate_fee(f64::NAN));
        assert!(!validate_fee(f64::INFINITY));
        assert!(!validate_fee(f64::NEG_INFINITY));
    }

    // -- reserves_sufficient --------------------------------------------------

    #[test]
    fn reserves_sufficient_accepts_values_above_threshold() {
        assert!(reserves_sufficient(100, 1000.0));
        assert!(reserves_sufficient(1, MIN_RESERVE_FOR_PRICE + 1e-9));
    }

    #[test]
    fn reserves_sufficient_boundary_is_strictly_greater_than() {
        // The threshold itself is not sufficient — the check is strictly >.
        assert!(!reserves_sufficient(1, MIN_RESERVE_FOR_PRICE));
        assert!(reserves_sufficient(1, MIN_RESERVE_FOR_PRICE * 1.000_001));
    }

    #[test]
    fn reserves_sufficient_rejects_zero_or_negative_stock() {
        assert!(!reserves_sufficient(0, 1000.0));
        assert!(!reserves_sufficient(-1, 1000.0));
    }

    #[test]
    fn reserves_sufficient_rejects_tiny_or_zero_currency() {
        assert!(!reserves_sufficient(100, 0.0));
        assert!(!reserves_sufficient(100, MIN_RESERVE_FOR_PRICE / 2.0));
    }

    // -- buy_cost_pure: boundaries -------------------------------------------

    #[test]
    fn buy_cost_pure_returns_none_when_amount_equals_stock() {
        // Cannot drain the entire pool.
        assert_eq!(buy_cost_pure(100, 1000.0, 100, 0.0), None);
    }

    #[test]
    fn buy_cost_pure_returns_none_when_amount_exceeds_stock() {
        assert_eq!(buy_cost_pure(100, 1000.0, 101, 0.0), None);
    }

    #[test]
    fn buy_cost_pure_returns_none_on_non_positive_amount() {
        assert_eq!(buy_cost_pure(100, 1000.0, 0, 0.125), None);
        assert_eq!(buy_cost_pure(100, 1000.0, -1, 0.125), None);
    }

    #[test]
    fn buy_cost_pure_returns_none_on_zero_reserves() {
        // Zero reserves must yield None, not panic or produce NaN.
        assert_eq!(buy_cost_pure(0, 1000.0, 1, 0.125), None);
        assert_eq!(buy_cost_pure(100, 0.0, 1, 0.125), None);
    }

    #[test]
    fn buy_cost_pure_returns_none_just_under_reserve_threshold() {
        // Currency stock just below MIN_RESERVE_FOR_PRICE → no quote.
        assert_eq!(
            buy_cost_pure(100, MIN_RESERVE_FOR_PRICE, 1, 0.0),
            None,
            "currency == threshold is not sufficient (strict >)",
        );
        assert_eq!(
            buy_cost_pure(100, MIN_RESERVE_FOR_PRICE / 2.0, 1, 0.0),
            None,
        );
    }

    #[test]
    fn buy_cost_pure_returns_some_just_over_reserve_threshold() {
        // Just above the threshold → a quote is produced, finite and positive.
        let cost = buy_cost_pure(100, MIN_RESERVE_FOR_PRICE * 1.01, 1, 0.0)
            .expect("cost should be quoted just above threshold");
        assert!(cost > 0.0 && cost.is_finite());
    }

    #[test]
    fn buy_cost_pure_returns_none_on_invalid_fee() {
        assert_eq!(buy_cost_pure(100, 1000.0, 1, -0.1), None);
        assert_eq!(buy_cost_pure(100, 1000.0, 1, 1.1), None);
        assert_eq!(buy_cost_pure(100, 1000.0, 1, f64::NAN), None);
    }

    // -- buy_cost_pure / sell_payout_pure: exact fee boundaries --------------

    #[test]
    fn buy_cost_pure_at_fee_zero_matches_base_amm() {
        // At fee == 0, cost is exactly the base AMM formula.
        let cost = buy_cost_pure(100, 1000.0, 10, 0.0).expect("cost");
        let expected = 1000.0 * 10.0 / (100.0 - 10.0);
        assert!((cost - expected).abs() < 1e-9);
    }

    #[test]
    fn buy_cost_pure_at_fee_one_doubles_base_cost() {
        // (1 + fee) == 2 → cost == 2 × base.
        let cost = buy_cost_pure(100, 1000.0, 10, 1.0).expect("cost");
        let expected = 2.0 * (1000.0 * 10.0 / (100.0 - 10.0));
        assert!((cost - expected).abs() < 1e-9);
    }

    #[test]
    fn sell_payout_pure_at_fee_zero_matches_base_amm() {
        let payout = sell_payout_pure(100, 1000.0, 10, 0.0).expect("payout");
        let expected = 1000.0 * 10.0 / (100.0 + 10.0);
        assert!((payout - expected).abs() < 1e-9);
    }

    #[test]
    fn sell_payout_pure_at_fee_one_returns_none() {
        // (1 - fee) == 0 → payout == 0 → rejected (must be strictly positive).
        assert_eq!(sell_payout_pure(100, 1000.0, 10, 1.0), None);
    }

    #[test]
    fn buy_and_sell_converge_to_spot_ratio_for_small_dx() {
        // For dx << x and equal reserves, both buy cost and sell payout
        // approach the spot ratio y/x = 1.0. This pins down the
        // zero-fee small-trade limit of the AMM.
        let dx = 1;
        let buy = buy_cost_pure(10_000, 10_000.0, dx, 0.0).expect("buy");
        let sell = sell_payout_pure(10_000, 10_000.0, dx, 0.0).expect("sell");
        assert!((buy - 1.0).abs() < 1e-3, "buy {} not near 1.0", buy);
        assert!((sell - 1.0).abs() < 1e-3, "sell {} not near 1.0", sell);
    }

    #[test]
    fn buy_exceeds_sell_at_zero_fee_due_to_curve() {
        // Even at fee == 0 the AMM curve produces a spread: the buy
        // denominator (x - dx) is smaller than the sell denominator (x + dx),
        // so buy is always marginally more expensive than sell. This is a
        // pure curve effect, not a fee effect — verifies the spread
        // survives fee == 0.
        let dx = 1;
        let buy = buy_cost_pure(10_000, 10_000.0, dx, 0.0).expect("buy");
        let sell = sell_payout_pure(10_000, 10_000.0, dx, 0.0).expect("sell");
        assert!(buy > sell, "expected buy {} > sell {}", buy, sell);
    }

    // -- the core invariants, exercised via the actual functions --------------

    #[test]
    fn buy_preserves_constant_product_at_zero_fee() {
        // k must be preserved exactly (within rounding) when fee == 0.
        let x = 100i32;
        let y = 1000.0;
        let dx = 10i32;
        let cost = buy_cost_pure(x, y, dx, 0.0).expect("cost");
        let k_old = x as f64 * y;
        let k_new = (x - dx) as f64 * (y + cost);
        assert!((k_new - k_old).abs() < 1e-9, "k shifted: {} -> {}", k_old, k_new);
    }

    #[test]
    fn sell_preserves_constant_product_at_zero_fee() {
        let x = 100i32;
        let y = 1000.0;
        let dx = 10i32;
        let payout = sell_payout_pure(x, y, dx, 0.0).expect("payout");
        let k_old = x as f64 * y;
        let k_new = (x + dx) as f64 * (y - payout);
        assert!((k_new - k_old).abs() < 1e-9, "k shifted: {} -> {}", k_old, k_new);
    }

    #[test]
    fn buy_slippage_strictly_increases_per_item_price() {
        // Larger buys pay more per item, via the actual function.
        let (x, y) = (100, 1000.0);
        let p1 = buy_cost_pure(x, y, 1, 0.0).unwrap() / 1.0;
        let p10 = buy_cost_pure(x, y, 10, 0.0).unwrap() / 10.0;
        let p50 = buy_cost_pure(x, y, 50, 0.0).unwrap() / 50.0;
        assert!(p10 > p1);
        assert!(p50 > p10);
    }

    #[test]
    fn sell_slippage_strictly_decreases_per_item_payout() {
        let (x, y) = (100, 1000.0);
        let p1 = sell_payout_pure(x, y, 1, 0.0).unwrap() / 1.0;
        let p10 = sell_payout_pure(x, y, 10, 0.0).unwrap() / 10.0;
        let p50 = sell_payout_pure(x, y, 50, 0.0).unwrap() / 50.0;
        assert!(p10 < p1);
        assert!(p50 < p10);
    }

    // -- Store-based wrappers -------------------------------------------------

    #[test]
    fn calculate_buy_cost_returns_none_for_unknown_item() {
        let store = build_store_with(vec![]);
        assert_eq!(calculate_buy_cost(&store, "nonexistent", 1), None);
    }

    #[test]
    fn calculate_sell_payout_returns_none_for_unknown_item() {
        let store = build_store_with(vec![]);
        assert_eq!(calculate_sell_payout(&store, "nonexistent", 1), None);
    }

    #[test]
    fn calculate_buy_cost_delegates_to_pure_with_store_fee() {
        let store = build_store_with(vec![("cobblestone", 100, 1000.0)]);
        let expected = buy_cost_pure(100, 1000.0, 10, store.config.fee);
        let got = calculate_buy_cost(&store, "cobblestone", 10);
        assert_eq!(got, expected);
    }

    #[test]
    fn calculate_sell_payout_delegates_to_pure_with_store_fee() {
        let store = build_store_with(vec![("cobblestone", 100, 1000.0)]);
        let expected = sell_payout_pure(100, 1000.0, 10, store.config.fee);
        let got = calculate_sell_payout(&store, "cobblestone", 10);
        assert_eq!(got, expected);
    }

    /// Minimal `Store` for pricing wrapper tests — the pricing code reads
    /// only `store.pairs` and `store.config.fee`, so the mock bot channel
    /// and empty storage/users are fine.
    fn build_store_with(pairs_spec: Vec<(&str, i32, f64)>) -> Store {
        use crate::config::Config;
        use crate::types::{Pair, Position, Storage};
        use crate::types::item_id::ItemId;
        use std::collections::HashMap;
        use tokio::sync::mpsc;

        let origin = Position { x: 0, y: 64, z: 0 };
        let storage = Storage::new(&origin);
        let (tx, _rx) = mpsc::channel(1);

        let config = Config {
            position: origin,
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

        let mut pairs = HashMap::new();
        for (name, item_stock, currency_stock) in pairs_spec {
            pairs.insert(
                name.to_string(),
                Pair {
                    item: ItemId::from_normalized(name.to_string()),
                    stack_size: 64,
                    item_stock,
                    currency_stock,
                },
            );
        }

        Store::new_for_test(tx, config, pairs, HashMap::new(), storage)
    }

    // -- Property-based tests -------------------------------------------------
    //
    // These assert AMM invariants across the full input space instead of
    // hand-picked cases. Load-bearing properties:
    //   - `k` never decreases (fees only grow the pool)
    //   - spread is positive (round-trip is strictly lossy)
    //   - per-item price increases with trade size (slippage)

    use proptest::prelude::*;

    const TEST_FEE: f64 = 0.125;

    proptest! {
        /// After a buy, `k` must be non-decreasing: the base identity holds
        /// `k` exactly and the fee adds a strictly positive delta to `y`.
        #[test]
        fn buy_never_decreases_k(
            stock in 2i32..10_000,
            currency in 1.0f64..100_000.0,
            qty in 1i32..9_999,
        ) {
            prop_assume!(qty < stock);
            let Some(cost) = buy_cost_pure(stock, currency, qty, TEST_FEE) else { return Ok(()); };
            let k_old = stock as f64 * currency;
            let k_new = (stock - qty) as f64 * (currency + cost);
            // Slack for floating-point rounding on large reserves.
            prop_assert!(k_new + 1e-6 >= k_old, "k decreased: {} -> {}", k_old, k_new);
        }

        /// Dual of `buy_never_decreases_k`: the fee is subtracted from the
        /// payout, leaving extra `y` in the pool.
        #[test]
        fn sell_never_decreases_k(
            stock in 1i32..10_000,
            currency in 1.0f64..100_000.0,
            qty in 1i32..10_000,
        ) {
            let Some(payout) = sell_payout_pure(stock, currency, qty, TEST_FEE) else { return Ok(()); };
            let k_old = stock as f64 * currency;
            let k_new = (stock + qty) as f64 * (currency - payout);
            prop_assert!(k_new + 1e-6 >= k_old, "k decreased: {} -> {}", k_old, k_new);
        }

        /// For the same reserves and quantity, buy cost exceeds sell payout:
        /// the fee spread, which makes round-trip trades strictly lossy.
        #[test]
        fn buy_cost_exceeds_sell_payout(
            stock in 2i32..10_000,
            currency in 1.0f64..100_000.0,
            qty in 1i32..9_999,
        ) {
            prop_assume!(qty < stock);
            let (Some(cost), Some(payout)) = (
                buy_cost_pure(stock, currency, qty, TEST_FEE),
                sell_payout_pure(stock, currency, qty, TEST_FEE),
            ) else { return Ok(()); };
            prop_assert!(cost > payout, "spread not positive: cost {} <= payout {}", cost, payout);
        }

        /// Buying `n+1` items costs strictly more per item than buying `n`.
        #[test]
        fn buy_price_per_item_increases(
            stock in 10i32..10_000,
            currency in 1.0f64..100_000.0,
            n in 1i32..1_000,
        ) {
            prop_assume!(n + 1 < stock);
            let (Some(c1), Some(c2)) = (
                buy_cost_pure(stock, currency, n, TEST_FEE),
                buy_cost_pure(stock, currency, n + 1, TEST_FEE),
            ) else { return Ok(()); };
            let p1 = c1 / n as f64;
            let p2 = c2 / (n + 1) as f64;
            prop_assert!(p2 > p1, "price per item did not increase: {} -> {}", p1, p2);
        }

        /// Dual of `buy_price_per_item_increases`.
        #[test]
        fn sell_price_per_item_decreases(
            stock in 10i32..10_000,
            currency in 1.0f64..100_000.0,
            n in 1i32..1_000,
        ) {
            let (Some(p1), Some(p2)) = (
                sell_payout_pure(stock, currency, n, TEST_FEE),
                sell_payout_pure(stock, currency, n + 1, TEST_FEE),
            ) else { return Ok(()); };
            let pp1 = p1 / n as f64;
            let pp2 = p2 / (n + 1) as f64;
            prop_assert!(pp2 < pp1, "payout per item did not decrease: {} -> {}", pp1, pp2);
        }

        /// Sell payout is strictly bounded by the currency reserve — the
        /// pool can never be drained by selling.
        #[test]
        fn sell_payout_bounded_by_currency(
            stock in 1i32..10_000,
            currency in 1.0f64..100_000.0,
            qty in 1i32..1_000_000,
        ) {
            let Some(payout) = sell_payout_pure(stock, currency, qty, TEST_FEE) else { return Ok(()); };
            prop_assert!(payout < currency, "payout {} >= currency {}", payout, currency);
        }

        /// After a buy, both new reserves remain strictly positive and
        /// finite — no NaN/inf territory, no drain.
        #[test]
        fn buy_leaves_reserves_positive_finite(
            stock in 2i32..10_000,
            currency in 1.0f64..100_000.0,
            qty in 1i32..9_999,
        ) {
            prop_assume!(qty < stock);
            let Some(cost) = buy_cost_pure(stock, currency, qty, TEST_FEE) else { return Ok(()); };
            let new_stock = stock - qty;
            let new_currency = currency + cost;
            prop_assert!(new_stock > 0, "new item_stock not positive: {}", new_stock);
            prop_assert!(new_currency.is_finite() && new_currency > currency,
                "new currency_stock invalid: {}", new_currency);
        }

        /// Sequential buy-then-sell is strictly lossy at the reserves
        /// produced by the buy — the spread property over sequential ops.
        #[test]
        fn buy_then_sell_loses_value(
            stock in 4i32..10_000,
            currency in 10.0f64..100_000.0,
            qty in 1i32..5_000,
        ) {
            prop_assume!(qty < stock / 2);
            let Some(cost) = buy_cost_pure(stock, currency, qty, TEST_FEE) else { return Ok(()); };
            let mid_stock = stock - qty;
            let mid_currency = currency + cost;
            let Some(payout) = sell_payout_pure(mid_stock, mid_currency, qty, TEST_FEE) else { return Ok(()); };
            prop_assert!(payout < cost,
                "round-trip not lossy: paid {} got back {}", cost, payout);
        }

        /// Both pricing functions reject non-positive quantities — no "free
        /// trade" escape hatch at qty == 0 or negative.
        #[test]
        fn non_positive_qty_returns_none(
            stock in 1i32..10_000,
            currency in 1.0f64..100_000.0,
            qty in -100i32..=0,
        ) {
            prop_assert!(buy_cost_pure(stock, currency, qty, TEST_FEE).is_none());
            prop_assert!(sell_payout_pure(stock, currency, qty, TEST_FEE).is_none());
        }

        /// With fee == 0, the base AMM identity `x*y = k` is preserved
        /// exactly (within relative FP tolerance). Isolates the fee as the
        /// sole source of `k` growth.
        #[test]
        fn fee_zero_preserves_k(
            stock in 2i32..10_000,
            currency in 1.0f64..100_000.0,
            qty in 1i32..9_999,
        ) {
            prop_assume!(qty < stock);
            let Some(cost) = buy_cost_pure(stock, currency, qty, 0.0) else { return Ok(()); };
            let k_old = stock as f64 * currency;
            let k_new = (stock - qty) as f64 * (currency + cost);
            let tol = k_old * 1e-9 + 1e-6;
            prop_assert!((k_new - k_old).abs() <= tol,
                "k drifted with fee=0: {} -> {} (tol {})", k_old, k_new, tol);
        }

        /// Higher fee produces a higher buy cost and a lower sell payout
        /// for identical reserves and quantity.
        #[test]
        fn fee_monotonic(
            stock in 4i32..10_000,
            currency in 10.0f64..100_000.0,
            qty in 1i32..5_000,
            f_low in 0.0f64..0.4,
            delta in 0.01f64..0.5,
        ) {
            prop_assume!(qty < stock);
            let f_high = f_low + delta;
            prop_assume!(f_high <= 1.0);
            let (Some(c_lo), Some(c_hi)) = (
                buy_cost_pure(stock, currency, qty, f_low),
                buy_cost_pure(stock, currency, qty, f_high),
            ) else { return Ok(()); };
            let (Some(p_lo), Some(p_hi)) = (
                sell_payout_pure(stock, currency, qty, f_low),
                sell_payout_pure(stock, currency, qty, f_high),
            ) else { return Ok(()); };
            prop_assert!(c_hi > c_lo, "buy cost not monotonic in fee: {} -> {}", c_lo, c_hi);
            prop_assert!(p_hi < p_lo, "sell payout not monotonic in fee: {} -> {}", p_lo, p_hi);
        }
    }
}
