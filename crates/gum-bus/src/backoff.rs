//! Exponential backoff with jitter, shared by every retrying component so
//! that "how long until the next attempt" has one definition.

use std::time::Duration;

use rand::Rng;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackoffPolicy {
    /// Delay after the first failure.
    pub base: Duration,
    /// Upper bound for the delay, before jitter.
    pub cap: Duration,
}

impl BackoffPolicy {
    pub const fn new(base: Duration, cap: Duration) -> Self {
        Self { base, cap }
    }

    /// Delay before attempt number `attempt + 1`, where `attempt` is how
    /// many attempts have failed so far (1 after the first failure).
    /// Equal jitter: half the exponential delay is guaranteed, the other
    /// half is uniformly random, so retries spread without ever being
    /// unreasonably early.
    pub fn delay(&self, attempt: u32) -> Duration {
        backoff_delay(self.base, self.cap, attempt, rand::rng().random::<f64>())
    }
}

/// The pure computation behind [`BackoffPolicy::delay`]; `unit` is a sample
/// in `[0, 1)` so tests can pin it.
pub fn backoff_delay(base: Duration, cap: Duration, attempt: u32, unit: f64) -> Duration {
    let exponent = attempt.saturating_sub(1).min(31);
    let scaled = base
        .checked_mul(1u32 << exponent)
        .unwrap_or(cap)
        .min(cap);
    let half = scaled / 2;
    half + Duration::from_secs_f64(half.as_secs_f64() * unit.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_doubles_and_caps() {
        let base = Duration::from_secs(2);
        let cap = Duration::from_secs(60);
        assert_eq!(backoff_delay(base, cap, 1, 0.0), Duration::from_secs(1));
        assert_eq!(backoff_delay(base, cap, 1, 1.0), Duration::from_secs(2));
        assert_eq!(backoff_delay(base, cap, 3, 1.0), Duration::from_secs(8));
        assert_eq!(backoff_delay(base, cap, 10, 1.0), cap);
        assert_eq!(backoff_delay(base, cap, 40, 0.0), cap / 2);
    }

    #[test]
    fn zero_attempts_is_the_base() {
        assert_eq!(
            backoff_delay(Duration::from_secs(4), Duration::from_secs(60), 0, 1.0),
            Duration::from_secs(4)
        );
    }
}
