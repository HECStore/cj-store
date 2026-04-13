//! Price calculation functions
//!
//! Implements constant product AMM pricing (x * y = k).
//! 
//! The price is NOT a simple ratio - it depends on trade size (slippage).
//! Larger trades move the price more, which is the key feature of AMMs.
//!
//! # Formula
//! - Buy cost: `cost = y * Δx / (x - Δx) * (1 + fee)`
//! - Sell payout: `payout = y * Δx / (x + Δx) * (1 - fee)`
//!
//! Where:
//! - x = item_stock
//! - y = currency_stock  
//! - Δx = amount being traded
//! - k = x * y (constant product, only increases due to fees)

use super::Store;
use crate::constants::{FEE_MIN, FEE_MAX, MIN_RESERVE_FOR_PRICE};

/// Validate that fee is within acceptable range.
/// 
/// # Arguments
/// * `fee` - Fee rate (0.0 to 1.0, e.g., 0.125 = 12.5%)
/// 
/// # Returns
/// * `true` if fee is valid (between FEE_MIN and FEE_MAX)
/// * `false` otherwise
pub fn validate_fee(fee: f64) -> bool {
    fee >= FEE_MIN && fee <= FEE_MAX && fee.is_finite()
}

/// Check if reserves are sufficient for reliable price calculation.
/// Very small reserves can lead to precision issues or extreme prices.
///
/// Rationale: with tiny `y`, the AMM price approaches zero; with tiny `x`,
/// a single trade can consume most of the pool and cause huge slippage or
/// floating-point precision loss near the `(x - dx)` denominator.
/// 
/// # Arguments
/// * `item_stock` - Number of items in reserve
/// * `currency_stock` - Amount of currency in reserve
/// 
/// # Returns
/// * `true` if reserves are sufficient
/// * `false` if reserves are too low
pub fn reserves_sufficient(item_stock: i32, currency_stock: f64) -> bool {
    item_stock > 0 && currency_stock > MIN_RESERVE_FOR_PRICE
}

/// Calculate total cost to buy a given amount of items using constant product formula.
/// 
/// # Formula
/// `cost = currency_stock * amount / (item_stock - amount) * (1 + fee)`
/// 
/// This implements the x * y = k invariant:
/// - Before: item_stock * currency_stock = k
/// - After: (item_stock - amount) * (currency_stock + cost) = k
///
/// Slippage emerges naturally: as `amount` approaches `item_stock`, the
/// denominator `(x - amount)` shrinks toward zero and cost grows without
/// bound. This is the self-balancing property of an AMM - it becomes
/// progressively more expensive to drain the pool, which protects against
/// total stock-out and manipulates incentives toward equilibrium.
///
/// The fee is applied on top of the base AMM cost (not added to the
/// reserves formula), so `k` grows slightly each trade - this is how fee
/// revenue accrues in the pool.
///
/// # Arguments
/// * `store` - The store state
/// * `item` - Item identifier
/// * `amount` - Number of items to buy
///
/// # Returns
/// * `Some(cost)` - Total cost in currency to buy the specified amount
/// * `None` - If reserves insufficient, amount exceeds stock, fee invalid, or calculation fails
///
/// # Example
/// With 1000 currency stock, 100 item stock, and 12.5% fee, buying 10 items:
/// - Base cost: 1000 * 10 / (100 - 10) = 1000 * 10 / 90 = 111.11
/// - With fee: 111.11 * 1.125 = 125.0
///
/// Note: Buying all 100 items would cost infinity (you can't drain the pool).
pub fn calculate_buy_cost(store: &Store, item: &str, amount: i32) -> Option<f64> {
    let pair = store.pairs.get(item)?;
    buy_cost_pure(pair.item_stock, pair.currency_stock, amount, store.config.fee)
}

/// Pure AMM buy-cost math — no `Store` dependency.
///
/// Same semantics as [`calculate_buy_cost`] but callable directly with
/// reserves and fee. This is the shape that property-based tests exercise.
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

/// Calculate total payout for selling a given amount of items using constant product formula.
/// 
/// # Formula
/// `payout = currency_stock * amount / (item_stock + amount) * (1 - fee)`
/// 
/// This implements the x * y = k invariant:
/// - Before: item_stock * currency_stock = k
/// - After: (item_stock + amount) * (currency_stock - payout) = k
///
/// Note that unlike `calculate_buy_cost`, there is no hard cap on `amount`:
/// the seller can dump arbitrarily many items into the pool. The `(x + dx)`
/// denominator only grows, so payout is bounded above by `y` and naturally
/// exhibits diminishing returns (slippage against the seller). The fee is
/// subtracted from the payout, meaning the pool keeps a little extra `y`
/// and `k` grows.
///
/// # Arguments
/// * `store` - The store state
/// * `item` - Item identifier
/// * `amount` - Number of items to sell
/// 
/// # Returns
/// * `Some(payout)` - Total payout in currency for selling the specified amount
/// * `None` - If reserves insufficient, fee invalid, or calculation fails
/// 
/// # Example
/// With 1000 currency stock, 100 item stock, and 12.5% fee, selling 10 items:
/// - Base payout: 1000 * 10 / (100 + 10) = 1000 * 10 / 110 = 90.91
/// - After fee: 90.91 * 0.875 = 79.55
pub fn calculate_sell_payout(store: &Store, item: &str, amount: i32) -> Option<f64> {
    let pair = store.pairs.get(item)?;
    sell_payout_pure(pair.item_stock, pair.currency_stock, amount, store.config.fee)
}

/// Pure AMM sell-payout math — no `Store` dependency.
///
/// Same semantics as [`calculate_sell_payout`] but callable directly with
/// reserves and fee. Used by property-based tests.
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

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_validate_fee() {
        // Valid fees
        assert!(validate_fee(0.0));
        assert!(validate_fee(0.125));
        assert!(validate_fee(0.5));
        assert!(validate_fee(1.0));
        
        // Invalid fees
        assert!(!validate_fee(-0.1));
        assert!(!validate_fee(1.1));
        assert!(!validate_fee(f64::NAN));
        assert!(!validate_fee(f64::INFINITY));
    }
    
    #[test]
    fn test_reserves_sufficient() {
        // Valid reserves
        assert!(reserves_sufficient(100, 1000.0));
        assert!(reserves_sufficient(1, 0.01));
        
        // Invalid reserves
        assert!(!reserves_sufficient(0, 1000.0));
        assert!(!reserves_sufficient(100, 0.0));
        assert!(!reserves_sufficient(100, 0.0001)); // Too small
        assert!(!reserves_sufficient(-1, 1000.0));
    }
    
    #[test]
    fn test_constant_product_invariant() {
        // Test that the formulas maintain x * y = k (before fees)
        // With 1000 currency, 100 items, k = 100,000
        let x: f64 = 100.0;
        let y: f64 = 1000.0;
        let k = x * y; // 100,000
        
        // Buying 10 items
        let buy_amount: f64 = 10.0;
        let buy_cost = y * buy_amount / (x - buy_amount); // 1000 * 10 / 90 = 111.11
        let new_x_after_buy = x - buy_amount; // 90
        let new_y_after_buy = y + buy_cost; // 1111.11
        let new_k_buy = new_x_after_buy * new_y_after_buy;
        assert!((new_k_buy - k).abs() < 0.01, "Buy should maintain k: {} vs {}", new_k_buy, k);
        
        // Selling 10 items
        let sell_amount: f64 = 10.0;
        let sell_payout = y * sell_amount / (x + sell_amount); // 1000 * 10 / 110 = 90.91
        let new_x_after_sell = x + sell_amount; // 110
        let new_y_after_sell = y - sell_payout; // 909.09
        let new_k_sell = new_x_after_sell * new_y_after_sell;
        assert!((new_k_sell - k).abs() < 0.01, "Sell should maintain k: {} vs {}", new_k_sell, k);
    }
    
    #[test]
    fn test_slippage_increases_with_trade_size() {
        // Larger trades should have worse effective prices (slippage)
        let x: f64 = 100.0;
        let y: f64 = 1000.0;
        
        // Buying 1 item
        let cost_1 = y * 1.0 / (x - 1.0);
        let price_per_1 = cost_1 / 1.0;
        
        // Buying 10 items
        let cost_10 = y * 10.0 / (x - 10.0);
        let price_per_10 = cost_10 / 10.0;
        
        // Buying 50 items
        let cost_50 = y * 50.0 / (x - 50.0);
        let price_per_50 = cost_50 / 50.0;
        
        // Price per item should increase with larger trades (worse for buyer)
        assert!(price_per_10 > price_per_1, "Larger buy should have higher price per item");
        assert!(price_per_50 > price_per_10, "Even larger buy should have even higher price per item");
        
        // Selling 1 item
        let payout_1 = y * 1.0 / (x + 1.0);
        let price_per_sell_1 = payout_1 / 1.0;
        
        // Selling 10 items
        let payout_10 = y * 10.0 / (x + 10.0);
        let price_per_sell_10 = payout_10 / 10.0;
        
        // Selling 50 items
        let payout_50 = y * 50.0 / (x + 50.0);
        let price_per_sell_50 = payout_50 / 50.0;
        
        // Payout per item should decrease with larger trades (worse for seller)
        assert!(price_per_sell_10 < price_per_sell_1, "Larger sell should have lower price per item");
        assert!(price_per_sell_50 < price_per_sell_10, "Even larger sell should have even lower price per item");
    }

    // ========================================================================
    // Property-based tests
    //
    // These assert AMM invariants across the full input space instead of a
    // handful of hand-picked cases. The specific properties checked are the
    // load-bearing ones that users rely on:
    //   - `k` never decreases (fee revenue accrues, pool never leaks value)
    //   - spread is always positive (round-trip buy+sell is strictly lossy)
    //   - per-item price strictly increases with trade size (slippage)
    // ========================================================================

    use proptest::prelude::*;

    const TEST_FEE: f64 = 0.125;

    proptest! {
        /// After a buy, `k` must be non-decreasing (fees only grow the pool).
        /// The base AMM identity holds `k` exactly; the fee markup adds a
        /// strictly positive delta to `y`, so `k_new >= k_old`.
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
            // Small slack for floating-point rounding on large reserves.
            prop_assert!(k_new + 1e-6 >= k_old, "k decreased: {} -> {}", k_old, k_new);
        }

        /// After a sell, `k` must be non-decreasing. Same reasoning as
        /// `buy_never_decreases_k`: the fee is subtracted from the payout,
        /// leaving extra `y` in the pool.
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

        /// For the same reserves and quantity, the buy cost must exceed the
        /// sell payout. This is the fee spread — it guarantees round-trip
        /// trades are strictly lossy and prevents arbitrage.
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

        /// Per-item buy cost increases with trade size (slippage).
        /// Buying `n+1` items must cost strictly more per item than buying
        /// `n`. This encodes the self-balancing property of the AMM.
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

        /// Per-item sell payout decreases with trade size (slippage against
        /// the seller). Dual of `buy_price_per_item_increases`.
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

        /// Sell payout is strictly bounded by the currency reserve — you
        /// can never drain the pool by selling, no matter how much you dump.
        #[test]
        fn sell_payout_bounded_by_currency(
            stock in 1i32..10_000,
            currency in 1.0f64..100_000.0,
            qty in 1i32..1_000_000,
        ) {
            let Some(payout) = sell_payout_pure(stock, currency, qty, TEST_FEE) else { return Ok(()); };
            prop_assert!(payout < currency, "payout {} >= currency {}", payout, currency);
        }
    }
}
