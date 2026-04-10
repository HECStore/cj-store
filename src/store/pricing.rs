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
    
    // Validate fee
    if !validate_fee(store.config.fee) {
        tracing::warn!("Invalid fee rate: {}", store.config.fee);
        return None;
    }
    
    // Check reserves
    if !reserves_sufficient(pair.item_stock, pair.currency_stock) {
        return None;
    }
    
    // Validate amount
    if amount <= 0 {
        return None;
    }
    
    // Cannot buy more than or equal to entire stock (would divide by zero or negative).
    // At amount == item_stock the denominator (x - dx) is 0 => infinite cost;
    // beyond that it flips sign and would produce a nonsensical "negative" cost.
    if amount >= pair.item_stock {
        return None;
    }
    
    let x = pair.item_stock as f64;
    let y = pair.currency_stock;
    let dx = amount as f64;

    // cost = y * dx / (x - dx)
    // Derived from keeping k constant: x*y = (x-dx)*(y+cost) => cost = y*dx/(x-dx)
    let base_cost = y * dx / (x - dx);
    let cost = base_cost * (1.0 + store.config.fee);

    // Guard against NaN/Infinity sneaking out (e.g. from extreme reserve values
    // or a denominator that underflowed despite the earlier amount < stock check).
    if cost.is_finite() && cost > 0.0 {
        Some(cost)
    } else {
        None
    }
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
    
    // Validate fee
    if !validate_fee(store.config.fee) {
        tracing::warn!("Invalid fee rate: {}", store.config.fee);
        return None;
    }
    
    // Check reserves
    if !reserves_sufficient(pair.item_stock, pair.currency_stock) {
        return None;
    }
    
    // Validate amount
    if amount <= 0 {
        return None;
    }
    
    let x = pair.item_stock as f64;
    let y = pair.currency_stock;
    let dx = amount as f64;

    // payout = y * dx / (x + dx)
    // Derived from keeping k constant: x*y = (x+dx)*(y-payout) => payout = y*dx/(x+dx)
    // As dx -> infinity, payout asymptotically approaches y but never reaches it,
    // so the pool's currency reserve can never be fully drained by selling.
    let base_payout = y * dx / (x + dx);
    let payout = base_payout * (1.0 - store.config.fee);

    // A zero payout can occur for tiny `dx` against a large `x` due to
    // floating-point rounding; treat that as a failed trade rather than
    // silently accepting a free item transfer.
    if payout.is_finite() && payout > 0.0 {
        Some(payout)
    } else {
        None
    }
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
}
