//! # Rate Limiter
//!
//! Anti-spam system with exponential backoff for player message rate limiting.
//!
//! ## Behavior
//! - Base cooldown: 2 seconds between messages
//! - Each violation doubles the required wait time (2s -> 4s -> 8s -> 16s...)
//! - Maximum cooldown caps at 60 seconds
//! - After RATE_LIMIT_RESET_AFTER_MS of idleness, violation count resets
//!
//! ## Usage
//! ```ignore
//! let mut limiter = RateLimiter::new();
//! match limiter.check("player_uuid") {
//!     Ok(()) => { /* process message */ }
//!     Err(throttled) => { /* reject; whisper iff throttled.should_whisper */ }
//! }
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Initial HashMap capacity for `RateLimiter::limits`. Sized to a small
/// server's typical concurrent active-user count so the map doesn't rehash
/// repeatedly through its growth from 0 toward `MAX_RATE_LIMIT_ENTRIES`
/// during a spam burst. Steady-state idle bot allocates ~256 buckets — small
/// in absolute terms relative to `MAX_RATE_LIMIT_ENTRIES = 10_000`.
const INITIAL_LIMITS_CAPACITY: usize = 256;

use tracing::{debug, warn};

use crate::constants::{
    RATE_LIMIT_BASE_COOLDOWN_MS,
    RATE_LIMIT_MAX_COOLDOWN_MS,
    RATE_LIMIT_RESET_AFTER_MS,
};

/// Smallest violation count at which `calculate_cooldown` is already pinned at
/// `RATE_LIMIT_MAX_COOLDOWN_MS`. Any value beyond this point produces the same
/// cooldown, so for log-readability we cap the displayed `violations` field at
/// this threshold and surface a separate `saturated` boolean instead of letting
/// the displayed counter grow unboundedly.
///
/// Hand-picked: with `RATE_LIMIT_BASE_COOLDOWN_MS = 2_000` and
/// `RATE_LIMIT_MAX_COOLDOWN_MS = 60_000`, `2_000 << 5 = 64_000 >= 60_000`,
/// so 5 is the smallest shift that pins the cooldown at MAX. A `const`
/// assertion below ties this to the constants so a future tuning of either
/// value will fail the build rather than silently desync.
const SATURATION_THRESHOLD: u32 = 5;
const _: () = assert!(
    RATE_LIMIT_BASE_COOLDOWN_MS << SATURATION_THRESHOLD >= RATE_LIMIT_MAX_COOLDOWN_MS,
    "SATURATION_THRESHOLD must be large enough that the exponential cooldown is pinned at MAX"
);

/// Tracks rate limit state for a single user
#[derive(Debug, Clone)]
struct UserRateLimit {
    /// When the user last sent a message that was ACCEPTED. Used to gate the
    /// per-message cooldown.
    last_message_time: Instant,
    /// When the user last *attempted* a message (accepted OR rejected). Used
    /// to gate the violation-count reset window: the doc promises that the
    /// counter clears after RATE_LIMIT_RESET_AFTER_MS of idleness, where
    /// idleness means "no attempts at all", not "no accepted attempts".
    /// Without this, a continuous spammer would have their violations reset
    /// every ~90s while still spamming, because `last_message_time` stays
    /// frozen at the last accepted message.
    last_attempt_time: Instant,
    /// Number of consecutive rate limit violations
    consecutive_violations: u32,
    /// When we last emitted a `warn!` for this user. Gates the per-rejection
    /// log so a sustained DoS at e.g. 50 attempts/sec/user doesn't drown the
    /// operator-interesting transitions (first violation, saturation,
    /// recovery) in identical "Violation recorded" spam — the rate limiter
    /// rate-limits its own observability.
    last_warn_time: Option<Instant>,
    /// When we last emitted a player-facing whisper for this user. Tracked
    /// independently of `last_warn_time` so the operator log gate and the
    /// outbound chat gate don't entangle — `check()` updates `last_warn_time`
    /// on its way out and computes the whisper-gate decision from THIS
    /// field, so the warn-gate stamp must not be treated as "whisper just
    /// fired" or every rejection after the first would be silently
    /// suppressed.
    last_whisper_time: Option<Instant>,
}

impl UserRateLimit {
    /// Create a fresh tracking entry. `last_message_time` is seeded to "now",
    /// but `RateLimiter::check` overrides it on insert so a first-time user
    /// is not instantly rate limited. `last_attempt_time` is left at "now"
    /// so a brand-new user is not falsely treated as having been idle.
    fn new() -> Self {
        let now = Instant::now();
        Self {
            last_message_time: now,
            last_attempt_time: now,
            consecutive_violations: 0,
            last_warn_time: None,
            last_whisper_time: None,
        }
    }
}

/// Outcome of a rejected `RateLimiter::check`. Combines the suggested
/// wait duration with the whisper-throttling decision so callers
/// cannot accidentally drop one or skip the second-phase call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Throttled {
    /// Suggested wait before the next attempt.
    pub wait: Duration,
    /// Whether the caller should emit a player-facing whisper for
    /// this rejection. False when the per-user whisper budget for
    /// this cycle is exhausted (cap on chat amplification under
    /// sustained spam).
    pub should_whisper: bool,
}

/// Hard cap on entries in `RateLimiter::limits` between cleanup sweeps. The
/// constants comment names `attacker_rate * STALE_AFTER_SECS` as the bound
/// — that's an unbounded multiplication on attacker_rate. This cap turns the
/// best-effort O(n) memory bound into a real upper bound and protects
/// long-running instances against a memory-pressure DoS.
const MAX_RATE_LIMIT_ENTRIES: usize = 10_000;

/// Rate limiter for player messages with exponential backoff
#[derive(Debug)]
pub struct RateLimiter {
    /// Per-user rate limit tracking
    limits: HashMap<String, UserRateLimit>,
    /// One-shot guard for the "cleanup_stale threshold below floor" warning.
    /// Per-instance so a misconfiguration in one limiter doesn't silence the
    /// warning forever in every other limiter sharing the process (the
    /// previous implementation used a `static AtomicBool`, which made the
    /// warning effectively single-fire across all instances and tests).
    clamp_warned: AtomicBool,
    /// Last time the cap-overflow path emitted a player-facing whisper.
    /// Without this gate, the cap-refusal branch had no per-user entry to
    /// throttle against and would whisper on every fresh-name attempt at
    /// cap — turning inbound junk into outbound chat amplification, the
    /// exact failure the per-user `last_whisper_time` was designed to
    /// prevent. Throttled to one whisper per `RATE_LIMIT_RESET_AFTER_MS`.
    cap_refusal_last_whisper: Option<Instant>,
    /// Last time the cap-overflow path actually ran an inline `cleanup_stale`.
    /// At cap, a Sybil-style new-key flood would otherwise re-run the full
    /// O(MAX_RATE_LIMIT_ENTRIES) `HashMap::retain` on every check — a CPU
    /// amplification primitive on the hot message path. Throttled to once
    /// per `RATE_LIMIT_BASE_COOLDOWN_MS`; the periodic `cleanup_stale`
    /// (300s loop) still preserves correctness.
    last_inline_sweep: Option<Instant>,
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
            limits: HashMap::with_capacity(INITIAL_LIMITS_CAPACITY),
            clamp_warned: AtomicBool::new(false),
            cap_refusal_last_whisper: None,
            last_inline_sweep: None,
        }
    }

    /// Clamp a `cleanup_stale` threshold to the `RATE_LIMIT_RESET_AFTER_MS`
    /// floor. The floor is the violation-reset window, not the cooldown cap:
    /// an entry must survive long enough for `consecutive_violations` to
    /// clear naturally, otherwise eviction would silently strip the violation
    /// count from a user who is past their cooldown but still inside the
    /// reset window. If the input is already `>= RESET_AFTER_MS`, returned
    /// unchanged. Otherwise raised to the floor, and a per-instance one-shot
    /// `warn!` is emitted so misconfiguration surfaces without per-call spam.
    fn clamp_stale_threshold(&self, stale_threshold: Duration) -> Duration {
        let floor = Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS);
        let clamped = stale_threshold.max(floor);
        if stale_threshold < clamped
            && !self.clamp_warned.swap(true, Ordering::Relaxed)
        {
            warn!(
                original_threshold_ms = stale_threshold.as_millis() as u64,
                clamped_threshold_ms = clamped.as_millis() as u64,
                "[RateLimit] cleanup_stale threshold below RATE_LIMIT_RESET_AFTER_MS; clamping to floor to preserve violation counts inside the reset window"
            );
        }
        clamped
    }

    /// Check if a user can send a message.
    ///
    /// # Arguments
    /// * `user_uuid` - The UUID of the user sending the message
    ///
    /// # Returns
    /// * `Ok(())` - User can proceed with their message
    /// * `Err(Throttled)` - User must wait `Throttled::wait` before sending
    ///   another message; the caller should emit a player-facing whisper
    ///   only if `Throttled::should_whisper` is true. The whisper decision
    ///   is folded into the same return value so callers cannot
    ///   accidentally drop it (a missed gate would silently flood chat
    ///   under sustained spam).
    ///
    /// # Key namespacing
    /// Callers may use a `prefix:` namespacing convention on keys (e.g. `n:`
    /// for usernames, `u:` for UUIDs) to keep semantically distinct gates
    /// from sharing a slot in the limiter's map. The limiter itself treats
    /// the key as an opaque string.
    pub fn check(&mut self, user_uuid: &str) -> Result<(), Throttled> {
        let now = Instant::now();

        // Cold-path cap: an attacker cycling shape-valid usernames creates
        // one fresh entry per name. On overflow, run an inline
        // cleanup_stale at the RATE_LIMIT_RESET_AFTER_MS floor —
        // actively-throttled users (still inside the reset window) are
        // guaranteed to survive. If the sweep frees no slots, refuse the
        // new attempt with the base cooldown so still-throttled users are
        // never silently reset by cap pressure.
        //
        // Sweep itself is throttled to once per RATE_LIMIT_BASE_COOLDOWN_MS:
        // without that, a sustained Sybil flood would force a 10k-element
        // `HashMap::retain` on every check (CPU amplification). The periodic
        // `cleanup_stale` (300s loop) still preserves correctness; the
        // throttle just bounds adversarial worst-case inline work.
        if !self.limits.contains_key(user_uuid) && self.limits.len() >= MAX_RATE_LIMIT_ENTRIES {
            let sweep_due = self
                .last_inline_sweep
                .is_none_or(|t| now.duration_since(t) >= Duration::from_millis(RATE_LIMIT_BASE_COOLDOWN_MS));
            if sweep_due {
                self.cleanup_stale(Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS));
                self.last_inline_sweep = Some(now);
            }
            if self.limits.len() >= MAX_RATE_LIMIT_ENTRIES {
                // No tracked entry to gate the whisper against — gate
                // globally instead so a Sybil-style flood at cap cannot
                // turn each inbound junk message into an outbound whisper.
                // One whisper per RATE_LIMIT_RESET_AFTER_MS window.
                let should_whisper = self
                    .cap_refusal_last_whisper
                    .is_none_or(|t| now.duration_since(t) >= Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS));
                if should_whisper {
                    self.cap_refusal_last_whisper = Some(now);
                }
                return Err(Throttled {
                    wait: calculate_cooldown(0),
                    should_whisper,
                });
            }
        }

        // Warm path: a single `get_mut` hash lookup, no `to_owned()`. The
        // cold-path `entry()` only fires when the user has no entry yet,
        // so the per-call `to_string()` allocation paid by the prior
        // `entry(user_uuid.to_string())` is now scoped to first-attempt
        // inserts. Keeping `entry()` (rather than a plain `insert`) on
        // the cold path preserves the "fail-fast on duplicate insert"
        // guarantee and the existing closure body unchanged.
        let user_limit = match self.limits.get_mut(user_uuid) {
            Some(e) => e,
            None => self
                .limits
                .entry(user_uuid.to_owned())
                .or_insert_with(|| {
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
                    // `last_attempt_time` is initialized to `now` by
                    // `UserRateLimit::new`. The post-insert block below
                    // (`user_limit.last_attempt_time = now;`) is the canonical
                    // site that stamps every call's attempt timestamp; do not
                    // re-assign here. Avoiding the double-write makes it
                    // unambiguous which write a future test would pin.
                    limit
                }),
        };

        let elapsed = now.duration_since(user_limit.last_message_time);

        // Compute idleness from `last_attempt_time` (BEFORE we update it
        // below) so a continuous spammer cannot self-forgive: every rejected
        // attempt also bumps `last_attempt_time`, keeping the idle window
        // from ever being satisfied while spam is ongoing.
        let attempt_elapsed = now.duration_since(user_limit.last_attempt_time);
        if attempt_elapsed >= Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS) {
            user_limit.consecutive_violations = 0;
            // Clear the warn/whisper gates so "first rejection in a new cycle"
            // takes the explicit `None` arm regardless of how RESET_AFTER and
            // the warn/whisper windows are tuned. Today they are all the same
            // constant and the gates would reopen anyway, but coupling three
            // independent windows by coincidence is brittle — making the
            // invariant local prevents a future tuning change from silently
            // suppressing the first warn/whisper after a natural reset.
            user_limit.last_warn_time = None;
            user_limit.last_whisper_time = None;
        }
        // Stamp the attempt now, AFTER reading the prior value, so this call
        // counts as "an attempt just happened" for the next call's idle check.
        user_limit.last_attempt_time = now;

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
            // Compute the wait against the POST-increment cooldown so the
            // number we hand back is "honest": a player told to wait W and who
            // actually waits W will be accepted on their next try. Using the
            // pre-increment cooldown would tell them to wait the old (shorter)
            // window, then reject them again because their cooldown has
            // already escalated. `saturating_sub` is correct here because at
            // the cap the new and old cooldowns are equal, so `elapsed` may
            // exceed `new_required` by a hair.
            let new_required = calculate_cooldown(user_limit.consecutive_violations);
            let remaining = new_required.saturating_sub(elapsed);
            // Cap the DISPLAYED violations counter at `SATURATION_THRESHOLD`
            // (the point at which `calculate_cooldown` is already pinned at
            // MAX). The stored counter is left unchanged — tests rely on it
            // accumulating — but readers of the log get a meaningful number
            // plus a `saturated` flag indicating "the cooldown is pinned at
            // MAX from this rejection onward", which is the operationally
            // interesting signal.
            //
            // Gate the warn so a sustained DoS doesn't drown operators in
            // identical lines: emit on the FIRST violation, on the
            // SATURATION transition, and at most once per RESET_AFTER
            // window thereafter. Per-rejection observability stays at
            // `debug!`.
            let saturation_transition = user_limit.consecutive_violations == SATURATION_THRESHOLD;
            let warn_window = Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS);
            let should_warn = match user_limit.last_warn_time {
                None => true,
                Some(_) if saturation_transition => true,
                Some(prev) => now.duration_since(prev) >= warn_window,
            };
            if should_warn {
                warn!(
                    gate_key = %user_uuid,
                    violations = user_limit.consecutive_violations.min(SATURATION_THRESHOLD),
                    saturated = user_limit.consecutive_violations >= SATURATION_THRESHOLD,
                    wait_ms = remaining.as_millis() as u64,
                    "[RateLimit] Violation recorded"
                );
                user_limit.last_warn_time = Some(now);
            } else {
                debug!(
                    gate_key = %user_uuid,
                    violations = user_limit.consecutive_violations.min(SATURATION_THRESHOLD),
                    wait_ms = remaining.as_millis() as u64,
                    "[RateLimit] Violation (suppressed warn)"
                );
            }
            // Whisper-throttling, folded inline so callers can't skip it
            // and silently flood chat. Returns true on the FIRST rejection
            // in the current cycle, on the SATURATION transition, and at
            // most once per RATE_LIMIT_RESET_AFTER_MS window thereafter.
            // The whisper window happens to equal `warn_window` today, but
            // the field is tracked independently of `last_warn_time` so a
            // future tuning of either window doesn't entangle the two.
            // `saturation_transition` is reused from the warn-gate logic
            // above — both are post-increment by the time we get here.
            let whisper_window = Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS);
            let should_whisper = match user_limit.last_whisper_time {
                None => true,
                Some(_) if saturation_transition => true,
                Some(prev) => now.duration_since(prev) >= whisper_window,
            };
            if should_whisper {
                user_limit.last_whisper_time = Some(now);
            }
            Err(Throttled {
                wait: remaining,
                should_whisper,
            })
        }
    }

    /// Drop entries for users idle longer than `stale_threshold`. Call
    /// periodically to prevent unbounded memory growth. The threshold must
    /// be **at least** `RATE_LIMIT_RESET_AFTER_MS` — equality is permitted
    /// because the retain predicate is strict-`<`, so an entry whose
    /// elapsed equals the threshold is dropped, but at that boundary the
    /// natural reset would have cleared violations on the next check
    /// anyway. A threshold strictly less than the floor would let an
    /// actively-throttled spammer get a free reset by being evicted.
    ///
    /// Idleness is measured against `last_attempt_time` (any attempt counts,
    /// accepted or rejected) rather than `last_message_time`, so a
    /// continuously-rejected spammer is never evicted while still attempting.
    pub fn cleanup_stale(&mut self, stale_threshold: Duration) {
        // Enforce the "threshold must exceed the violation-reset window" contract:
        // if violated, an actively-throttled spammer could be evicted and get a
        // free reset before the natural reset window elapses.
        debug_assert!(
            stale_threshold >= Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS),
            "cleanup_stale threshold ({:?}) must be >= RATE_LIMIT_RESET_AFTER_MS ({}ms) \
             to avoid evicting users whose violation counts have not yet reset",
            stale_threshold,
            RATE_LIMIT_RESET_AFTER_MS,
        );
        self.cleanup_stale_clamped(stale_threshold);
    }

    /// Release-mode core of [`Self::cleanup_stale`]: clamp the threshold to
    /// the `RATE_LIMIT_RESET_AFTER_MS` floor (defense-in-depth against a
    /// misconfigured caller that bypasses the `debug_assert!`), then evict
    /// every entry whose `last_attempt_time` is older than the (clamped)
    /// threshold. Factored out so a direct unit test can confirm the
    /// release-mode safety net actually preserves actively-throttled users
    /// when handed a sub-floor threshold, without relying on `cfg`-gated
    /// test-build forks.
    fn cleanup_stale_clamped(&mut self, stale_threshold: Duration) {
        let stale_threshold = self.clamp_stale_threshold(stale_threshold);
        let now = Instant::now();
        let before = self.limits.len();
        // Key idleness off `last_attempt_time` (bumped on every check, accepted
        // or rejected) rather than `last_message_time` (bumped only on accepts).
        // A continuously-rejected spammer otherwise has `last_message_time`
        // frozen at their last accept and could be evicted at the boundary
        // even while still actively throttled.
        self.limits.retain(|_, limit| {
            now.duration_since(limit.last_attempt_time) < stale_threshold
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

    /// Test-only: backdate a user's `last_message_time`, `last_attempt_time`,
    /// and (when present) `last_warn_time` / `last_whisper_time` by `by` so
    /// time-dependent paths (reset window, staleness, warn/whisper gates) can
    /// be exercised without real sleeps. All four fields shift in lockstep so
    /// the synthetic "time travel" matches what would happen in real
    /// wall-clock time — otherwise tests would be probing internal accounting
    /// rather than externally observable behavior. The warn/whisper fields
    /// are only shifted when `Some`, so a fresh entry whose gates have never
    /// fired stays in the `None` state.
    /// Returns false if the user has no entry yet.
    #[cfg(test)]
    fn backdate(&mut self, user_uuid: &str, by: Duration) -> bool {
        if let Some(limit) = self.limits.get_mut(user_uuid) {
            limit.last_message_time -= by;
            limit.last_attempt_time -= by;
            if let Some(t) = limit.last_warn_time.as_mut() {
                *t -= by;
            }
            if let Some(t) = limit.last_whisper_time.as_mut() {
                *t -= by;
            }
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
        // 90s reset window.
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
    fn continuous_spam_does_not_self_forgive_after_reset_window() {
        // Regression: previously the violation-reset check used elapsed time
        // since the last ACCEPTED message. A user spamming continuously left
        // `last_message_time` frozen at their last accepted message, and
        // after RATE_LIMIT_RESET_AFTER_MS of nonstop rejection their
        // violations would silently reset to 0 — with the next attempt
        // accepted under the BASE 2s cooldown, then re-accumulating from 0.
        //
        // The fix tracks `last_attempt_time` (bumped on every check, accepted
        // or rejected) and uses THAT for the idle-window check, so violations
        // never reset while attempts keep coming.
        //
        // Note: even with the fix, a spammer DOES get one accept per
        // MAX_COOLDOWN window (because `elapsed` against `last_message_time`
        // crosses MAX_COOLDOWN), but each such accept preserves the
        // violation count, so the next attempt still hits the escalated
        // cooldown. The bug-regression signal is that violations do NOT
        // silently reset to 0 mid-spam.
        let mut limiter = RateLimiter::new();
        let _ = limiter.check("spammer"); // accepted, violations stays 0
        let _ = limiter.check("spammer"); // rejected, violations -> 1
        assert!(
            limiter.violations_for("spammer").unwrap() >= 1,
            "precondition: at least one rejection recorded"
        );

        // Simulate spam at 1s intervals across more than RATE_LIMIT_RESET_AFTER_MS
        // of synthetic time. Pre-fix: violations would silently reset to 0 once
        // accumulated `elapsed` against last_message_time crossed RESET_AFTER.
        // Post-fix: violations accumulate without reset.
        let step = Duration::from_secs(1);
        let total_window =
            Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS) + Duration::from_secs(10);
        let mut elapsed = Duration::ZERO;
        while elapsed < total_window {
            assert!(limiter.backdate("spammer", step));
            let _ = limiter.check("spammer");
            elapsed += step;
        }

        // Pre-fix: violations would have reset at the ~90s mark and only
        // re-accumulated for the trailing ~10s, leaving violations near
        // single digits. Post-fix: violations accumulate across the entire
        // ~100-iteration loop minus a small number of MAX_COOLDOWN-window
        // accepts (~2), staying well above the pre-fix ceiling.
        let violations = limiter.violations_for("spammer").unwrap();
        assert!(
            violations > 30,
            "continuous spam over the reset window must NOT silently reset \
             violations; got {violations}. Pre-fix bug would leave this in \
             single digits."
        );
    }

    #[test]
    fn rejected_wait_does_not_exceed_current_cooldown() {
        let mut limiter = RateLimiter::new();
        let _ = limiter.check("user1");
        // The rejection branch now reports the wait against the POST-increment
        // cooldown (1 violation -> 4s) so the value is "honest": waiting that
        // long actually clears the throttle. The bound therefore reflects the
        // escalated cooldown rather than the base cooldown.
        let escalated = calculate_cooldown(1);
        match limiter.check("user1") {
            Err(Throttled { wait, .. }) => assert!(
                wait <= escalated,
                "wait {wait:?} exceeds escalated cooldown {escalated:?}"
            ),
            Ok(()) => panic!("expected rejection"),
        }
    }

    #[test]
    fn waiting_the_returned_duration_unblocks_next_check() {
        // "Honest wait" invariant: after a rejection that returned wait W,
        // backdating `last_message_time` so it is W farther in the past must
        // make the next `check` succeed. Otherwise the player is told to wait
        // a duration that, when obeyed, still gets them rejected.
        let mut limiter = RateLimiter::new();
        let _ = limiter.check("user1");
        let wait = match limiter.check("user1") {
            Err(Throttled { wait, .. }) => wait,
            Ok(()) => panic!("expected rejection"),
        };
        assert!(limiter.backdate("user1", wait));
        assert!(
            limiter.check("user1").is_ok(),
            "next check after honoring returned wait must succeed"
        );
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
        limiter.cleanup_stale(Duration::from_secs(crate::constants::RATE_LIMIT_STALE_AFTER_SECS));
        assert_eq!(limiter.len(), 1);
        assert_eq!(limiter.violations_for("old_user"), None);
        assert_eq!(limiter.violations_for("recent_user"), Some(0));
    }

    #[test]
    fn cleanup_stale_is_noop_when_all_entries_are_fresh() {
        let mut limiter = RateLimiter::new();
        let _ = limiter.check("user1");
        let _ = limiter.check("user2");
        limiter.cleanup_stale(Duration::from_secs(crate::constants::RATE_LIMIT_STALE_AFTER_SECS));
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
        // A threshold smaller than RESET_AFTER would let cleanup evict users
        // whose violations have not yet naturally reset. The debug_assert
        // catches misuse.
        let mut limiter = RateLimiter::new();
        let _ = limiter.check("user1");
        limiter.cleanup_stale(Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS - 1));
    }

    #[test]
    fn cleanup_stale_evicts_at_exact_floor_boundary() {
        // The retain predicate is strict-`<`: an entry whose elapsed equals
        // the threshold IS dropped. This test pins that contract so a future
        // maintainer who flips it to `<=` (a plausible "preserve at boundary"
        // tweak) gets a test failure instead of silently changing eviction
        // semantics. Eviction at exactly the floor is harmless in production
        // because the floor is the natural-reset window — violations would
        // have reset on the next check anyway — but the predicate still has
        // to be the one we ship.
        let mut limiter = RateLimiter::new();
        let _ = limiter.check("spammer");
        for _ in 0..20 {
            let _ = limiter.check("spammer");
        }
        let v_before = limiter.violations_for("spammer").unwrap();
        assert!(v_before >= 20, "precondition: saturation reached");

        let floor = Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS);
        // `backdate` shifts both fields uniformly; the retain predicate is
        // keyed on `last_attempt_time`, so this places the entry exactly at
        // the boundary and exercises the strict-`<` check.
        assert!(limiter.backdate("spammer", floor));
        limiter.cleanup_stale(floor);
        assert_eq!(
            limiter.violations_for("spammer"),
            None,
            "exact-floor entries must be evicted under strict-`<` retain",
        );
    }

    #[test]
    fn cleanup_stale_preserves_user_just_inside_floor() {
        // Companion to the boundary test: an entry whose elapsed is one
        // millisecond LESS than the threshold survives. The two tests
        // together pin both sides of the strict-`<` predicate.
        let mut limiter = RateLimiter::new();
        let _ = limiter.check("spammer2");
        for _ in 0..20 {
            let _ = limiter.check("spammer2");
        }
        let v_before = limiter.violations_for("spammer2").unwrap();
        let floor = Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS);
        assert!(limiter.backdate("spammer2", floor - Duration::from_millis(1)));
        limiter.cleanup_stale(floor);
        assert_eq!(
            limiter.violations_for("spammer2"),
            Some(v_before),
            "user just inside the floor must keep their violation count",
        );
    }

    #[test]
    fn default_produces_empty_limiter() {
        let limiter = RateLimiter::default();
        assert_eq!(limiter.len(), 0);
    }

    #[test]
    fn clamp_stale_threshold_returns_input_when_at_or_above_floor() {
        // At-floor input: returned unchanged. The floor is RESET_AFTER_MS,
        // not MAX_COOLDOWN_MS, so an entry must survive long enough for
        // `consecutive_violations` to reset naturally.
        let limiter = RateLimiter::new();
        let at_floor = Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS);
        assert_eq!(limiter.clamp_stale_threshold(at_floor), at_floor);

        // Above-floor input: returned unchanged. Use a realistic production
        // threshold so the assertion ties to actual deployment configuration.
        let above_floor = Duration::from_secs(crate::constants::RATE_LIMIT_STALE_AFTER_SECS);
        assert!(above_floor >= at_floor, "test precondition");
        assert_eq!(limiter.clamp_stale_threshold(above_floor), above_floor);

        // Far-above-floor input: still unchanged.
        let way_above = Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS * 100);
        assert_eq!(limiter.clamp_stale_threshold(way_above), way_above);
    }

    #[test]
    fn clamp_stale_threshold_raises_below_floor_inputs_to_floor() {
        // Below-floor inputs must be raised to AT LEAST RESET_AFTER. We
        // assert `>= floor` rather than `== floor` so this test stays
        // resilient if the helper later switches to (e.g.) `RESET_AFTER * 2`
        // as the safety floor.
        let limiter = RateLimiter::new();
        let floor = Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS);

        let zero = Duration::from_millis(0);
        assert!(limiter.clamp_stale_threshold(zero) >= floor);

        // The old MAX_COOLDOWN-based floor used to admit this value
        // unchanged; the tightened floor now raises it.
        let at_old_floor = Duration::from_millis(RATE_LIMIT_MAX_COOLDOWN_MS);
        assert!(limiter.clamp_stale_threshold(at_old_floor) >= floor);

        let just_under = Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS - 1);
        assert!(limiter.clamp_stale_threshold(just_under) >= floor);

        let way_under = Duration::from_millis(1);
        assert!(limiter.clamp_stale_threshold(way_under) >= floor);
    }

    #[test]
    fn cleanup_stale_clamped_preserves_actively_throttled_user_under_subfloor_input() {
        // Direct release-mode test of the safety net: even when handed a
        // sub-floor threshold (which would `debug_assert!` in the public
        // wrapper), the inner `cleanup_stale_clamped` raises it to the floor
        // and an actively-throttled user inside the reset window survives.
        let mut limiter = RateLimiter::new();
        // Trigger a violation so the user has consecutive_violations > 0.
        assert!(limiter.check("alice").is_ok());
        assert!(limiter.check("alice").is_err());
        let pre = limiter.violations_for("alice").unwrap();
        assert!(pre > 0, "test precondition: alice has violations");

        // Hand a deliberately-sub-floor threshold; clamp must rescue.
        let too_small = Duration::from_millis(1);
        limiter.cleanup_stale_clamped(too_small);

        assert!(
            limiter.violations_for("alice").is_some(),
            "actively-throttled user inside reset window must survive sub-floor cleanup"
        );
    }

    /// Helper for cap-overflow tests: pre-fill the limiter to
    /// `MAX_RATE_LIMIT_ENTRIES` with synthetic distinct keys. Each entry is
    /// inserted via the public `check()` path so the per-user accounting is
    /// produced by real code, not hand-rolled struct literals — keeps the
    /// test honest if `UserRateLimit::new` semantics change.
    #[cfg(test)]
    fn fill_to_cap(limiter: &mut RateLimiter) {
        for i in 0..MAX_RATE_LIMIT_ENTRIES {
            let key = format!("filler-{i}");
            let _ = limiter.check(&key);
        }
        assert_eq!(limiter.len(), MAX_RATE_LIMIT_ENTRIES);
    }

    #[test]
    fn cap_overflow_evicts_only_entries_past_reset_window() {
        // When the map is at cap and a new key arrives, the inline
        // cleanup_stale sweep must drop entries whose last_attempt_time is
        // past RATE_LIMIT_RESET_AFTER_MS — and ONLY those entries. Entries
        // still inside the reset window survive.
        let mut limiter = RateLimiter::new();
        fill_to_cap(&mut limiter);

        // Backdate exactly one filler past the reset window so cleanup_stale
        // has at least one eligible victim.
        let stale_key = "filler-0";
        assert!(limiter.backdate(
            stale_key,
            Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS + 1_000),
        ));

        // A representative still-fresh entry; it must survive the sweep.
        let fresh_key = "filler-1";
        let fresh_violations_before = limiter.violations_for(fresh_key);

        assert!(limiter.check("newcomer").is_ok());

        assert_eq!(
            limiter.violations_for(stale_key),
            None,
            "entry past the reset window must be evicted by the inline sweep"
        );
        assert_eq!(
            limiter.violations_for(fresh_key),
            fresh_violations_before,
            "entries inside the reset window must survive the inline sweep"
        );
    }

    #[test]
    fn cap_overflow_refuses_new_user_when_no_eligible_victim() {
        // If every entry is still inside the reset window, the inline
        // cleanup_stale cannot free any slots. The contract is to refuse
        // the new attempt with a base cooldown rather than silently evict
        // an actively-throttled or legitimate-but-idle user.
        let mut limiter = RateLimiter::new();
        fill_to_cap(&mut limiter);
        let len_before = limiter.len();

        match limiter.check("newcomer") {
            Ok(()) => panic!("expected refusal when no entry is past the reset window"),
            Err(Throttled { wait, .. }) => assert_eq!(
                wait,
                Duration::from_millis(RATE_LIMIT_BASE_COOLDOWN_MS),
                "refusal must use the base cooldown"
            ),
        }
        assert_eq!(
            limiter.len(),
            len_before,
            "new key must NOT be inserted when the cap cannot be relieved"
        );
        assert!(
            limiter.violations_for("newcomer").is_none(),
            "new key must NOT have an entry after refusal"
        );
    }

    #[test]
    fn cap_overflow_preserves_actively_throttled_user() {
        // Anchor the property the previous LRU policy violated: an
        // actively-throttled user (saturated violations, recent
        // last_attempt_time) must keep their violation count when the cap
        // is hit by a new attacker name. The new policy refuses the
        // newcomer rather than evicting any actively-tracked user.
        let mut limiter = RateLimiter::new();

        // Seed the throttled user FIRST so they get a deterministic key,
        // then saturate their violations.
        let throttled = "throttled_user";
        let _ = limiter.check(throttled);
        for _ in 0..(SATURATION_THRESHOLD + 2) {
            let _ = limiter.check(throttled);
        }
        let throttled_violations = limiter
            .violations_for(throttled)
            .expect("throttled user has an entry");
        assert!(
            throttled_violations >= SATURATION_THRESHOLD,
            "precondition: violations saturated"
        );

        // Fill the rest of the map. `fill_to_cap` would push us past cap
        // because the throttled user already occupies a slot, so insert
        // one fewer filler to land exactly at cap.
        for i in 0..(MAX_RATE_LIMIT_ENTRIES - limiter.len()) {
            let key = format!("filler-{i}");
            let _ = limiter.check(&key);
        }
        assert_eq!(limiter.len(), MAX_RATE_LIMIT_ENTRIES);

        // Newcomer arrival under no eligible-victim conditions: should
        // refuse, leaving the throttled user untouched.
        let _ = limiter.check("attacker_new_name");
        assert_eq!(
            limiter.violations_for(throttled),
            Some(throttled_violations),
            "actively-throttled user's violation count must be unchanged by cap pressure"
        );
    }

    #[test]
    fn throttled_first_rejection_returns_should_whisper_true() {
        // The first rejection in a fresh cycle must surface
        // `should_whisper: true` so the caller emits the player-facing
        // notice. Anchors the missing-entry / first-cycle behavior the
        // prior `should_whisper_rejection_returns_true_when_no_entry`
        // and `..._emits_then_suppresses_within_window` covered.
        let mut limiter = RateLimiter::new();
        assert!(limiter.check("user1").is_ok());
        match limiter.check("user1") {
            Err(Throttled { should_whisper, .. }) => assert!(
                should_whisper,
                "first whisper after a rejection must fire"
            ),
            Ok(()) => panic!("precondition: second check must produce Err"),
        }
    }

    #[test]
    fn throttled_repeated_rejection_within_window_suppresses_whisper() {
        // A second rejection inside the suppression window without any
        // natural reset must report `should_whisper: false` so the bot
        // doesn't spam whispers on every rejected attempt.
        let mut limiter = RateLimiter::new();
        assert!(limiter.check("user1").is_ok());
        match limiter.check("user1") {
            Err(Throttled { should_whisper, .. }) => {
                assert!(should_whisper, "first whisper must fire")
            }
            Ok(()) => panic!("precondition: second check must produce Err"),
        }
        match limiter.check("user1") {
            Err(Throttled { should_whisper, .. }) => assert!(
                !should_whisper,
                "second whisper inside the suppression window must be suppressed"
            ),
            Ok(()) => panic!("third check must also be rejected"),
        }
    }

    #[test]
    fn throttled_re_emits_should_whisper_after_reset_window() {
        // After RATE_LIMIT_RESET_AFTER_MS of synthetic time has elapsed
        // since the last whisper, the gate must reopen so a returning
        // spammer (or a legitimate lapsed user) gets a fresh whisper
        // rather than being silently throttled forever by stale state.
        let mut limiter = RateLimiter::new();
        assert!(limiter.check("user1").is_ok());
        match limiter.check("user1") {
            Err(Throttled { should_whisper, .. }) => {
                assert!(should_whisper, "first whisper must fire")
            }
            Ok(()) => panic!("precondition: second check must produce Err"),
        }

        // Push last_whisper_time past the suppression window. `backdate`
        // shifts last_whisper_time alongside the message/attempt
        // timestamps so this scenario is actually modelable.
        let beyond_window =
            Duration::from_millis(RATE_LIMIT_RESET_AFTER_MS) + Duration::from_secs(1);
        assert!(limiter.backdate("user1", beyond_window));

        // Note: the backdate also crosses the violation-reset window, so
        // the next check sees `attempt_elapsed >= RESET_AFTER_MS` and
        // clears violations — and clears `last_whisper_time` to None —
        // making this look like the start of a fresh cycle. The "honest
        // wait" invariant means that next check ALSO crosses the
        // cooldown and is accepted, so we do one accept then trigger a
        // new rejection to observe the whisper gate.
        assert!(
            limiter.check("user1").is_ok(),
            "post-reset-window check must accept (full idle resets state)"
        );
        match limiter.check("user1") {
            Err(Throttled { should_whisper, .. }) => assert!(
                should_whisper,
                "whisper must re-emit on the first rejection after the suppression window elapses"
            ),
            Ok(()) => panic!("expected rejection within fresh cooldown"),
        }
    }

    #[test]
    fn throttled_emits_should_whisper_on_saturation_transition() {
        // The saturation transition (consecutive_violations crossing
        // SATURATION_THRESHOLD) is operationally interesting and the
        // folded whisper gate explicitly bypasses the suppression window
        // for that crossing. After the first rejection (which fires the
        // whisper), accumulate further rejections inside the suppression
        // window until violations hit SATURATION_THRESHOLD; on that
        // crossing the whisper must fire again.
        let mut limiter = RateLimiter::new();
        assert!(limiter.check("user1").is_ok());
        match limiter.check("user1") {
            Err(Throttled { should_whisper, .. }) => assert!(
                should_whisper,
                "initial whisper after first rejection must fire"
            ),
            Ok(()) => panic!("precondition: second check must produce Err"),
        }

        let mut saw_saturation_whisper = false;
        // Generous upper bound — SATURATION_THRESHOLD is small (5) and
        // the saturation transition fires once consecutive_violations
        // equals that constant (post-increment, inside check).
        for _ in 0..50 {
            let throttled = match limiter.check("user1") {
                Err(t) => t,
                Ok(()) => panic!("still within cooldown — every check must be rejected"),
            };
            let v = limiter.violations_for("user1").unwrap();
            if v == SATURATION_THRESHOLD {
                assert!(
                    throttled.should_whisper,
                    "saturation-transition rejection must fire a whisper \
                     even within the suppression window"
                );
                saw_saturation_whisper = true;
                break;
            } else {
                assert!(
                    !throttled.should_whisper,
                    "non-transition rejection inside the suppression window \
                     must NOT fire a whisper (violations={v})"
                );
            }
        }
        assert!(
            saw_saturation_whisper,
            "loop must reach the SATURATION_THRESHOLD crossing"
        );
    }

    #[test]
    fn throttled_warn_gate_does_not_silently_suppress_whisper_gate() {
        // Load-bearing invariant per the field doc-comments: `check()`
        // updates `last_warn_time` on its way out, and the folded
        // whisper-gate decision must NOT see that update treated as
        // "whisper just fired". Otherwise every rejection after the
        // first would be silently suppressed because the warn-gate stamp
        // would gate the whisper too. The two gates share their window
        // by coincidence today, but their stamps are tracked
        // independently and the first rejection must always whisper.
        let mut limiter = RateLimiter::new();
        assert!(limiter.check("user1").is_ok());
        // First rejection — `check()` stamps last_warn_time AND fires
        // the whisper gate. The whisper gate must still observe its own
        // None state at decision time, not the freshly-stamped warn
        // gate.
        match limiter.check("user1") {
            Err(Throttled { should_whisper, .. }) => assert!(
                should_whisper,
                "first whisper after a rejection must fire even though \
                 check() also stamps last_warn_time — the gates must \
                 evolve independently"
            ),
            Ok(()) => panic!("precondition: second check must produce Err"),
        }
    }
}
