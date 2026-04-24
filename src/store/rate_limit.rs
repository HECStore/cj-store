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

use tracing::{debug, warn};

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
    /// Create a fresh tracking entry. `last_message_time` is seeded to "now",
    /// but `RateLimiter::check` overrides it on insert so a first-time user
    /// is not instantly rate limited.
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

/// Calculate the required cooldown based on violation count.
///
/// Uses exponential backoff so repeat spammers face rapidly growing wait times
/// while occasional offenders barely notice. The doubling pattern
/// (2s, 4s, 8s, 16s, ...) punishes sustained spam without over-penalizing
/// honest mistakes, and the `MAX_COOLDOWN` cap ensures a persistent spammer
/// can't be locked out indefinitely.
fn calculate_cooldown(violations: u32) -> Duration {
    // Clamp the shift to 10 to avoid u64 overflow; 2^10 = 1024 already
    // vastly exceeds MAX_COOLDOWN, so higher values are pointless.
    let multiplier = 1u64 << violations.min(10);
    let cooldown_ms = RATE_LIMIT_BASE_COOLDOWN_MS.saturating_mul(multiplier);
    Duration::from_millis(cooldown_ms.min(RATE_LIMIT_MAX_COOLDOWN_MS))
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            limits: HashMap::new(),
        }
    }

    /// Check if a user can send a message.
    ///
    /// # Arguments
    /// * `user_uuid` - The UUID of the user sending the message
    ///
    /// # Returns
    /// * `Ok(())` - User can proceed with their message
    /// * `Err(Duration)` - User must wait this long before sending another message
    pub fn check(&mut self, user_uuid: &str) -> Result<(), Duration> {
        let now = Instant::now();

        let user_limit = self.limits.entry(user_uuid.to_string()).or_insert_with(|| {
            // For a brand new user, backdate `last_message_time` past MAX_COOLDOWN
            // so the `elapsed >= required_cooldown` check below always passes on
            // the very first message; without this, a new user would be treated
            // as having "just messaged" at entry creation and be rejected.
            // `checked_sub` guards against the (Windows/Linux-impossible but
            // platform-allowed) case of `Instant` subtraction underflowing near
            // process start; falling back to `now` means the first message would
            // be rejected, which is preferable to a panic.
            let mut limit = UserRateLimit::new();
            let backdate = Duration::from_millis(RATE_LIMIT_MAX_COOLDOWN_MS + 1);
            limit.last_message_time = now.checked_sub(backdate).unwrap_or(now);
            limit
        });

        let elapsed = now.duration_since(user_limit.last_message_time);

        if elapsed >= Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS) {
            user_limit.consecutive_violations = 0;
        }

        let required_cooldown = calculate_cooldown(user_limit.consecutive_violations);

        if elapsed >= required_cooldown {
            user_limit.last_message_time = now;
            // Intentionally do NOT reset violations here. If we did, a spammer
            // could simply wait out each escalating cooldown once and then
            // resume spamming at the base rate forever. Violations only clear
            // after a full RATE_LIMIT_RESET_AFTER_MS of genuine idleness.
            Ok(())
        } else {
            // `saturating_add` guards against wrap-around from a pathological
            // spammer; the cooldown itself is capped in `calculate_cooldown`.
            user_limit.consecutive_violations = user_limit.consecutive_violations.saturating_add(1);
            let remaining = required_cooldown - elapsed;
            warn!(
                user_uuid = %user_uuid,
                violations = user_limit.consecutive_violations,
                wait_ms = remaining.as_millis() as u64,
                "[RateLimit] Violation recorded"
            );
            Err(remaining)
        }
    }

    /// Drop entries for users idle longer than `stale_threshold`. Call
    /// periodically to prevent unbounded memory growth. The threshold must
    /// exceed any legitimate cooldown window so no entry that is still
    /// throttling a user can be removed.
    pub fn cleanup_stale(&mut self, stale_threshold: Duration) {
        // Enforce the "threshold must exceed max cooldown" contract: if violated,
        // an actively-throttled spammer could be evicted and get a free reset.
        debug_assert!(
            stale_threshold >= Duration::from_millis(RATE_LIMIT_MAX_COOLDOWN_MS),
            "cleanup_stale threshold ({:?}) must be >= RATE_LIMIT_MAX_COOLDOWN_MS ({}ms) \
             to avoid evicting actively-throttled users",
            stale_threshold,
            RATE_LIMIT_MAX_COOLDOWN_MS,
        );
        let now = Instant::now();
        let before = self.limits.len();
        self.limits.retain(|_, limit| {
            now.duration_since(limit.last_message_time) < stale_threshold
        });
        let dropped = before - self.limits.len();
        if dropped > 0 {
            debug!(
                dropped = dropped,
                remaining = self.limits.len(),
                threshold_secs = stale_threshold.as_secs(),
                "[RateLimit] Cleaned up stale entries"
            );
        }
    }

    /// Test-only: backdate a user's `last_message_time` by `by` so time-dependent
    /// paths (reset window, staleness) can be exercised without real sleeps.
    /// Returns false if the user has no entry yet.
    #[cfg(test)]
    fn backdate(&mut self, user_uuid: &str, by: Duration) -> bool {
        if let Some(limit) = self.limits.get_mut(user_uuid) {
            limit.last_message_time -= by;
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    fn violations_for(&self, user_uuid: &str) -> Option<u32> {
        self.limits.get(user_uuid).map(|l| l.consecutive_violations)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.limits.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_message_from_new_user_is_allowed() {
        let mut limiter = RateLimiter::new();
        assert!(limiter.check("user1").is_ok());
    }

    #[test]
    fn immediate_second_message_is_rejected() {
        let mut limiter = RateLimiter::new();
        assert!(limiter.check("user1").is_ok());
        assert!(limiter.check("user1").is_err());
    }

    #[test]
    fn rejection_records_a_violation() {
        let mut limiter = RateLimiter::new();
        let _ = limiter.check("user1");
        assert_eq!(limiter.violations_for("user1"), Some(0));
        let _ = limiter.check("user1");
        assert_eq!(limiter.violations_for("user1"), Some(1));
        let _ = limiter.check("user1");
        assert_eq!(limiter.violations_for("user1"), Some(2));
    }

    #[test]
    fn violation_count_accumulates_on_repeated_rejections() {
        let mut limiter = RateLimiter::new();
        let _ = limiter.check("user1");
        for _ in 0..15 {
            let _ = limiter.check("user1");
        }
        let v = limiter.violations_for("user1").unwrap();
        assert!(v >= 15, "expected accumulated violations, got {v}");
    }

    #[test]
    fn calculate_cooldown_doubles_per_violation() {
        assert_eq!(calculate_cooldown(0), Duration::from_millis(2000));
        assert_eq!(calculate_cooldown(1), Duration::from_millis(4000));
        assert_eq!(calculate_cooldown(2), Duration::from_millis(8000));
        assert_eq!(calculate_cooldown(3), Duration::from_millis(16000));
    }

    #[test]
    fn calculate_cooldown_caps_at_max() {
        assert_eq!(
            calculate_cooldown(20),
            Duration::from_millis(RATE_LIMIT_MAX_COOLDOWN_MS)
        );
    }

    #[test]
    fn calculate_cooldown_handles_u32_max_without_overflow() {
        assert_eq!(
            calculate_cooldown(u32::MAX),
            Duration::from_millis(RATE_LIMIT_MAX_COOLDOWN_MS)
        );
    }

    #[test]
    fn message_allowed_after_cooldown_keeps_violation_count() {
        // Spammer pattern: trigger one rejection, then wait out just the cooldown.
        // Violations must NOT reset — only full idleness resets them.
        let mut limiter = RateLimiter::new();
        let _ = limiter.check("user1");
        let _ = limiter.check("user1"); // rejected, violations -> 1
        assert_eq!(limiter.violations_for("user1"), Some(1));

        // Advance time past the 4s cooldown (violation 1) but short of the
        // 30s reset window.
        assert!(limiter.backdate("user1", Duration::from_secs(5)));
        assert!(limiter.check("user1").is_ok());
        assert_eq!(
            limiter.violations_for("user1"),
            Some(1),
            "violations must persist after an allowed message within reset window"
        );
    }

    #[test]
    fn violations_reset_after_full_idle_window() {
        let mut limiter = RateLimiter::new();
        let _ = limiter.check("user1");
        let _ = limiter.check("user1");
        let _ = limiter.check("user1");
        assert!(limiter.violations_for("user1").unwrap() >= 2);

        assert!(limiter.backdate(
            "user1",
            Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS + 1_000),
        ));
        assert!(limiter.check("user1").is_ok());
        assert_eq!(limiter.violations_for("user1"), Some(0));
    }

    #[test]
    fn rejected_wait_does_not_exceed_current_cooldown() {
        let mut limiter = RateLimiter::new();
        let _ = limiter.check("user1");
        // At 0 violations the required cooldown is 2s; an immediate retry's
        // remaining wait must not exceed that.
        match limiter.check("user1") {
            Err(wait) => assert!(
                wait <= Duration::from_millis(RATE_LIMIT_BASE_COOLDOWN_MS),
                "wait {wait:?} exceeds base cooldown"
            ),
            Ok(()) => panic!("expected rejection"),
        }
    }

    #[test]
    fn users_are_rate_limited_independently() {
        let mut limiter = RateLimiter::new();
        assert!(limiter.check("alice").is_ok());
        assert!(limiter.check("bob").is_ok());
        assert!(limiter.check("alice").is_err());
        assert_eq!(limiter.violations_for("alice"), Some(1));
        assert_eq!(limiter.violations_for("bob"), Some(0));
    }

    #[test]
    fn cleanup_stale_drops_entries_past_threshold() {
        let mut limiter = RateLimiter::new();
        let _ = limiter.check("old_user");
        let _ = limiter.check("recent_user");
        assert_eq!(limiter.len(), 2);

        assert!(limiter.backdate("old_user", Duration::from_secs(600)));
        limiter.cleanup_stale(Duration::from_secs(300));
        assert_eq!(limiter.len(), 1);
        assert_eq!(limiter.violations_for("old_user"), None);
        assert_eq!(limiter.violations_for("recent_user"), Some(0));
    }

    #[test]
    fn cleanup_stale_is_noop_when_all_entries_are_fresh() {
        let mut limiter = RateLimiter::new();
        let _ = limiter.check("user1");
        let _ = limiter.check("user2");
        limiter.cleanup_stale(Duration::from_secs(300));
        assert_eq!(limiter.len(), 2);
    }

    #[test]
    fn cleanup_stale_preserves_actively_throttled_user() {
        // Contract: `stale_threshold` must exceed any legitimate cooldown, so a
        // user who is still within their escalating backoff window cannot be
        // evicted (which would give them a free violation-count reset on the
        // next message). This test locks in that contract with a realistic
        // production-sized threshold.
        let mut limiter = RateLimiter::new();
        let _ = limiter.check("spammer");
        // Force violations up to the cap, which pins `required_cooldown` at
        // MAX_COOLDOWN (60s). The user is still actively throttled.
        for _ in 0..20 {
            let _ = limiter.check("spammer");
        }
        assert!(limiter.violations_for("spammer").unwrap() >= 20);

        // Backdate the user most of the way through the 300s stale window,
        // but keep them still within their 60s cooldown (impossible in wall
        // time — this is a synthetic stress on the invariant).
        assert!(limiter.backdate("spammer", Duration::from_secs(250)));

        // With the production threshold (300s), the actively-throttled entry
        // must survive: 250s elapsed < 300s threshold.
        limiter.cleanup_stale(Duration::from_secs(
            crate::constants::RATE_LIMIT_STALE_AFTER_SECS,
        ));
        assert_eq!(
            limiter.violations_for("spammer"),
            Some(20),
            "actively-throttled user must survive cleanup when within threshold"
        );
    }

    #[test]
    #[should_panic(expected = "cleanup_stale threshold")]
    #[cfg(debug_assertions)]
    fn cleanup_stale_panics_in_debug_on_too_small_threshold() {
        // A threshold smaller than MAX_COOLDOWN would let cleanup evict
        // actively-throttled users. The debug_assert catches misuse.
        let mut limiter = RateLimiter::new();
        let _ = limiter.check("user1");
        limiter.cleanup_stale(Duration::from_millis(RATE_LIMIT_MAX_COOLDOWN_MS - 1));
    }

    #[test]
    fn default_produces_empty_limiter() {
        let limiter = RateLimiter::default();
        assert_eq!(limiter.len(), 0);
    }
}
