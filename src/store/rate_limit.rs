//! # Rate Limiter
//!
//! Anti-spam system with exponential backoff for player message rate limiting.
//!
//! ## Behavior
//! - Base cooldown: 2 seconds between messages
//! - Each violation doubles the required wait time (2s -> 4s -> 8s -> 16s...)
//! - Maximum cooldown caps at 60 seconds
//! - After 30 seconds of no messages, violation count resets
//!
//! ## Usage
//! ```ignore
//! let mut limiter = RateLimiter::new();
//! match limiter.check("player_uuid") {
//!     Ok(()) => { /* process message */ }
//!     Err(wait_duration) => { /* reject, tell player to wait */ }
//! }
//! ```

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::constants::{
    RATE_LIMIT_BASE_COOLDOWN_MS,
    RATE_LIMIT_MAX_COOLDOWN_MS,
    RATE_LIMIT_RESET_AFTER_MS,
};

/// Tracks rate limit state for a single user
#[derive(Debug, Clone)]
struct UserRateLimit {
    /// When the user last sent a message
    last_message_time: Instant,
    /// Number of consecutive rate limit violations
    consecutive_violations: u32,
}

impl UserRateLimit {
    fn new() -> Self {
        Self {
            last_message_time: Instant::now(),
            consecutive_violations: 0,
        }
    }
}

/// Rate limiter for player messages with exponential backoff
#[derive(Debug)]
pub struct RateLimiter {
    /// Per-user rate limit tracking
    limits: HashMap<String, UserRateLimit>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Calculate the required cooldown based on violation count
fn calculate_cooldown(violations: u32) -> Duration {
    // Exponential backoff: base * 2^violations
    // Cap the exponent to prevent overflow (2^10 = 1024, which is plenty)
    let multiplier = 1u64 << violations.min(10);
    let cooldown_ms = RATE_LIMIT_BASE_COOLDOWN_MS.saturating_mul(multiplier);
    Duration::from_millis(cooldown_ms.min(RATE_LIMIT_MAX_COOLDOWN_MS))
}

impl RateLimiter {
    /// Create a new rate limiter
    pub fn new() -> Self {
        Self {
            limits: HashMap::new(),
        }
    }

    /// Check if a user can send a message
    ///
    /// # Arguments
    /// * `user_uuid` - The UUID of the user sending the message
    ///
    /// # Returns
    /// * `Ok(())` - User can proceed with their message
    /// * `Err(Duration)` - User must wait this long before sending another message
    pub fn check(&mut self, user_uuid: &str) -> Result<(), Duration> {
        let now = Instant::now();

        // Get or create user's rate limit entry
        let user_limit = self.limits.entry(user_uuid.to_string()).or_insert_with(|| {
            // New user - set last_message_time in the past so first message always succeeds
            let mut limit = UserRateLimit::new();
            limit.last_message_time = now - Duration::from_secs(60);
            limit
        });

        let elapsed = now.duration_since(user_limit.last_message_time);

        // Check if violation count should be reset (30s of no messages)
        if elapsed >= Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS) {
            user_limit.consecutive_violations = 0;
        }

        // Calculate required cooldown based on current violation count
        let required_cooldown = calculate_cooldown(user_limit.consecutive_violations);

        if elapsed >= required_cooldown {
            // User waited long enough - allow the message
            user_limit.last_message_time = now;
            // Don't reset violations here - they only reset after 30s idle
            Ok(())
        } else {
            // User is sending too fast - increment violations and reject
            user_limit.consecutive_violations = user_limit.consecutive_violations.saturating_add(1);
            let remaining = required_cooldown - elapsed;
            Err(remaining)
        }
    }

    /// Get the current violation count for a user (for debugging/logging)
    #[allow(dead_code)]
    pub fn get_violations(&self, user_uuid: &str) -> u32 {
        self.limits
            .get(user_uuid)
            .map(|l| l.consecutive_violations)
            .unwrap_or(0)
    }

    /// Clean up stale entries (users who haven't sent messages in a while)
    /// Call periodically to prevent memory growth
    #[allow(dead_code)]
    pub fn cleanup_stale(&mut self, max_age: Duration) {
        let now = Instant::now();
        self.limits.retain(|_, limit| {
            now.duration_since(limit.last_message_time) < max_age
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_message_allowed() {
        let mut limiter = RateLimiter::new();
        assert!(limiter.check("user1").is_ok());
    }

    #[test]
    fn test_rapid_messages_rejected() {
        let mut limiter = RateLimiter::new();
        assert!(limiter.check("user1").is_ok());
        // Immediate second message should be rejected
        let result = limiter.check("user1");
        assert!(result.is_err());
    }

    #[test]
    fn test_exponential_backoff() {
        // 0 violations: 2s
        assert_eq!(calculate_cooldown(0), Duration::from_millis(2000));
        // 1 violation: 4s
        assert_eq!(calculate_cooldown(1), Duration::from_millis(4000));
        // 2 violations: 8s
        assert_eq!(calculate_cooldown(2), Duration::from_millis(8000));
        // 3 violations: 16s
        assert_eq!(calculate_cooldown(3), Duration::from_millis(16000));
    }

    #[test]
    fn test_max_cooldown_cap() {
        // High violation count should cap at max cooldown
        assert_eq!(
            calculate_cooldown(20),
            Duration::from_millis(RATE_LIMIT_MAX_COOLDOWN_MS)
        );
    }
}
