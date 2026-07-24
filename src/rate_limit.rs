use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use thiserror::Error;

use crate::{errors::AppError, AppState};

pub const DEFAULT_REQUESTS_PER_SECOND: u32 = 10;
pub const DEFAULT_BURST: u32 = 20;
pub const MAX_REQUESTS_PER_SECOND: u32 = 1_000_000;
pub const MAX_BURST: u32 = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RateLimitPolicy {
    requests_per_second: u32,
    burst: u32,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum RateLimitPolicyError {
    #[error("requests per second must be between 1 and {MAX_REQUESTS_PER_SECOND}")]
    InvalidRequestsPerSecond,
    #[error("burst must be between 1 and {MAX_BURST}")]
    InvalidBurst,
}

impl RateLimitPolicy {
    /// Creates a validated token-bucket policy.
    ///
    /// Both values must be nonzero and within the documented runtime bounds.
    /// The rate controls continuous refill and the burst controls maximum
    /// accumulated capacity.
    pub fn new(requests_per_second: u32, burst: u32) -> Result<Self, RateLimitPolicyError> {
        if !(1..=MAX_REQUESTS_PER_SECOND).contains(&requests_per_second) {
            return Err(RateLimitPolicyError::InvalidRequestsPerSecond);
        }
        if !(1..=MAX_BURST).contains(&burst) {
            return Err(RateLimitPolicyError::InvalidBurst);
        }
        Ok(Self {
            requests_per_second,
            burst,
        })
    }

    /// Returns the configured continuous refill rate in requests per second.
    pub fn requests_per_second(self) -> u32 {
        self.requests_per_second
    }

    /// Returns the maximum number of tokens the bucket may accumulate.
    pub fn burst(self) -> u32 {
        self.burst
    }
}

impl Default for RateLimitPolicy {
    /// Returns the documented default policy of 10 requests/second with burst 20.
    fn default() -> Self {
        Self {
            requests_per_second: DEFAULT_REQUESTS_PER_SECOND,
            burst: DEFAULT_BURST,
        }
    }
}

#[derive(Debug)]
struct BucketState {
    tokens: f64,
    last_refill: Duration,
}

#[derive(Debug)]
pub struct RateLimiter {
    policy: RateLimitPolicy,
    started_at: Instant,
    bucket: Mutex<BucketState>,
}

impl RateLimiter {
    /// Creates a full token bucket for the supplied validated policy.
    ///
    /// Each limiter owns independent state. Share one limiter through `Arc`
    /// when router clones must consume the same process-wide budget.
    pub fn new(policy: RateLimitPolicy) -> Self {
        Self {
            policy,
            started_at: Instant::now(),
            bucket: Mutex::new(BucketState {
                tokens: f64::from(policy.burst),
                last_refill: Duration::ZERO,
            }),
        }
    }

    /// Returns the immutable policy governing this limiter.
    pub fn policy(&self) -> RateLimitPolicy {
        self.policy
    }

    /// Charges one request token or returns the whole-second retry delay.
    ///
    /// Refill is continuous and capped at burst capacity. A rejected request
    /// does not consume a token. The returned delay is rounded up and is always
    /// at least one second for a rejection.
    pub fn check(&self) -> Result<(), u64> {
        self.check_at(self.started_at.elapsed())
    }

    /// Applies one admission decision at a caller-supplied monotonic offset.
    ///
    /// This is separated from `check` so refill and retry behavior can be
    /// tested deterministically without sleeping.
    fn check_at(&self, now: Duration) -> Result<(), u64> {
        let mut bucket = self
            .bucket
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let elapsed = now.saturating_sub(bucket.last_refill);
        if !elapsed.is_zero() {
            let refilled = elapsed.as_secs_f64() * f64::from(self.policy.requests_per_second);
            bucket.tokens = (bucket.tokens + refilled).min(f64::from(self.policy.burst));
            bucket.last_refill = now;
        }

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            return Ok(());
        }

        let missing_tokens = 1.0 - bucket.tokens;
        let retry_seconds =
            (missing_tokens / f64::from(self.policy.requests_per_second)).ceil() as u64;
        Err(retry_seconds.max(1))
    }
}

/// Enforces the single shared HTTP request budget before auth and handlers.
///
/// Every routed or unmatched request costs one token. Rejections return the
/// standard HTTP error shape even on `/mcp`, and do not call downstream
/// authentication, JSON-RPC, or provider code.
pub async fn rate_limit_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    match state.rate_limiter.check() {
        Ok(()) => next.run(request).await,
        Err(retry_after_seconds) => {
            AppError::too_many_requests(retry_after_seconds).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// Creates a validated limiter for deterministic offset-based tests.
    fn limiter(rate: u32, burst: u32) -> RateLimiter {
        RateLimiter::new(RateLimitPolicy::new(rate, burst).expect("valid test policy"))
    }

    /// Verifies initial capacity and the first rejection boundary.
    #[test]
    fn starts_full_and_exhausts_at_burst_capacity() {
        let limiter = limiter(10, 3);

        assert_eq!(limiter.check_at(Duration::ZERO), Ok(()));
        assert_eq!(limiter.check_at(Duration::ZERO), Ok(()));
        assert_eq!(limiter.check_at(Duration::ZERO), Ok(()));
        assert_eq!(limiter.check_at(Duration::ZERO), Err(1));
    }

    /// Verifies fractional refill and rounded retry timing.
    #[test]
    fn refills_continuously_and_calculates_retry_without_sleeping() {
        let limiter = limiter(2, 1);

        assert_eq!(limiter.check_at(Duration::ZERO), Ok(()));
        assert_eq!(limiter.check_at(Duration::from_millis(250)), Err(1));
        assert_eq!(limiter.check_at(Duration::from_millis(500)), Ok(()));
    }

    /// Verifies idle time cannot accumulate more than the burst.
    #[test]
    fn refill_never_exceeds_burst_capacity() {
        let limiter = limiter(100, 2);

        assert_eq!(limiter.check_at(Duration::from_secs(60)), Ok(()));
        assert_eq!(limiter.check_at(Duration::from_secs(60)), Ok(()));
        assert_eq!(limiter.check_at(Duration::from_secs(60)), Err(1));
    }

    /// Verifies shared limiter handles charge the same bucket.
    #[test]
    fn arc_clones_share_bucket_state() {
        let first = Arc::new(limiter(1, 1));
        let second = Arc::clone(&first);

        assert_eq!(first.check_at(Duration::ZERO), Ok(()));
        assert_eq!(second.check_at(Duration::ZERO), Err(1));
    }
}
