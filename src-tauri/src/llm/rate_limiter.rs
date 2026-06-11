//! Token-bucket rate limiter for LLM provider calls (design doc §29).
//!
//! Enforces 80% of documented free-tier limits per provider. Handles
//! `Retry-After` from 429 responses — the limiter parks the caller until the
//! backoff window expires without triggering a fallover.
//!
//! Two independent buckets per provider:
//!   - **Request bucket**: refills at `requests_per_minute / 60` tokens/second.
//!   - **Token bucket**: refills at `tokens_per_minute / 60` tokens/second.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tracing::{info, warn};

// ──────────────────────────────────────────────────────────────────────────────
// Bucket
// ──────────────────────────────────────────────────────────────────────────────

struct Bucket {
    capacity: f64,
    tokens: f64,
    /// Tokens added per second.
    refill_rate: f64,
    last_refill: Instant,
}

impl Bucket {
    fn new(capacity: u32) -> Self {
        let cap = capacity as f64;
        Self {
            capacity: cap,
            tokens: cap,
            refill_rate: cap / 60.0,
            last_refill: Instant::now(),
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity);
        self.last_refill = now;
    }

    /// Try to consume `amount` tokens. Returns `true` on success.
    fn try_consume(&mut self, amount: f64) -> bool {
        self.refill();
        if self.tokens >= amount {
            self.tokens -= amount;
            true
        } else {
            false
        }
    }

    /// Seconds until `amount` tokens are available.
    fn wait_secs(&mut self, amount: f64) -> f64 {
        self.refill();
        if self.tokens >= amount {
            return 0.0;
        }
        // Zero-capacity bucket (e.g. stub provider rpm=0) must not divide by zero.
        if self.refill_rate <= f64::EPSILON {
            return 0.0;
        }
        (amount - self.tokens) / self.refill_rate
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// RateLimiter
// ──────────────────────────────────────────────────────────────────────────────

/// Groq free-tier Retry-After can exceed 10 minutes; never block a turn longer than this.
pub const MAX_RETRY_AFTER_HONOR_SECS: u64 = 45;

/// Per-provider rate limiter.
///
/// Wrap in `Arc<RateLimiter>` and share across orchestrator threads.
pub struct RateLimiter {
    provider_name: String,
    /// When true, `acquire` is a no-op (stub provider with rpm/tpm = 0).
    disabled: bool,
    request_bucket: Arc<Mutex<Bucket>>,
    token_bucket: Arc<Mutex<Bucket>>,
    /// Hard-stop override: when `Some(deadline)`, all calls sleep until
    /// the deadline passes (set on `Retry-After` header receipt).
    retry_after: Arc<Mutex<Option<Instant>>>,
}

impl RateLimiter {
    pub fn new(provider_name: impl Into<String>, rpm: u32, tpm: u32) -> Self {
        Self {
            provider_name: provider_name.into(),
            disabled: rpm == 0 && tpm == 0,
            request_bucket: Arc::new(Mutex::new(Bucket::new(rpm.max(1)))),
            token_bucket: Arc::new(Mutex::new(Bucket::new(tpm.max(1)))),
            retry_after: Arc::new(Mutex::new(None)),
        }
    }

    /// Acquire a request slot and `estimated_tokens` from the token bucket.
    ///
    /// Blocks (via `tokio::time::sleep`) until both are available. Does NOT
    /// switch to a fallback provider — that decision belongs to the
    /// `FailoverManager`.
    pub async fn acquire(&self, estimated_tokens: u32) {
        if self.disabled {
            return;
        }

        // 1. Honour any active Retry-After window first.
        let deadline_opt = *self.retry_after.lock().await;
        if let Some(deadline) = deadline_opt {
            let now = Instant::now();
            if deadline > now {
                let remaining = deadline - now;
                warn!(
                    provider = %self.provider_name,
                    wait_secs = remaining.as_secs_f64(),
                    "honouring Retry-After before next request"
                );
                tokio::time::sleep(remaining).await;
                *self.retry_after.lock().await = None;
            }
        }

        let token_amount = estimated_tokens as f64;

        // 2. Wait for request slot.
        loop {
            let wait = self.request_bucket.lock().await.wait_secs(1.0);
            if wait <= 0.0 {
                self.request_bucket.lock().await.try_consume(1.0);
                break;
            }
            tokio::time::sleep(Duration::from_secs_f64(wait)).await;
        }

        // 3. Wait for token budget.
        loop {
            let wait = self.token_bucket.lock().await.wait_secs(token_amount);
            if wait <= 0.0 {
                self.token_bucket.lock().await.try_consume(token_amount);
                break;
            }
            tokio::time::sleep(Duration::from_secs_f64(wait)).await;
        }

        info!(
            provider = %self.provider_name,
            estimated_tokens,
            "rate-limit slot acquired"
        );
    }

    /// Record a `Retry-After` value received from a 429 response.
    ///
    /// All subsequent `acquire()` calls will sleep until the deadline.
    pub async fn set_retry_after(&self, secs: u64) {
        let capped = secs.min(MAX_RETRY_AFTER_HONOR_SECS);
        let deadline = Instant::now() + Duration::from_secs(capped);
        *self.retry_after.lock().await = Some(deadline);
        warn!(
            provider = %self.provider_name,
            retry_after_secs = secs,
            honored_secs = capped,
            "rate limit (429) — Retry-After set"
        );
    }

    /// Return true when there is no active `Retry-After` backoff in progress.
    pub async fn is_ready(&self) -> bool {
        let guard = self.retry_after.lock().await;
        match *guard {
            None => true,
            Some(deadline) => Instant::now() >= deadline,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_starts_full() {
        let b = Bucket::new(60);
        assert!((b.tokens - 60.0).abs() < 0.01);
    }

    #[test]
    fn bucket_consume_decrements() {
        let mut b = Bucket::new(60);
        assert!(b.try_consume(30.0));
        assert!((b.tokens - 30.0).abs() < 0.01);
    }

    #[test]
    fn bucket_consume_fails_when_empty() {
        let mut b = Bucket::new(10);
        assert!(b.try_consume(10.0));
        assert!(!b.try_consume(1.0));
    }

    #[tokio::test]
    async fn acquire_noop_when_disabled() {
        let limiter = RateLimiter::new("stub", 0, 0);
        let start = Instant::now();
        limiter.acquire(5000).await;
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "disabled limiter should not block"
        );
    }

    #[tokio::test]
    async fn acquire_completes_when_tokens_available() {
        let limiter = RateLimiter::new("test", 60, 60_000);
        // Both buckets start full so acquire must not trigger any
        // `tokio::time::sleep`. The 500 ms bound is generous enough to absorb
        // jitter from coverage-instrumented or loaded CI runners while still
        // catching a regression that introduces a real wait.
        let start = Instant::now();
        limiter.acquire(100).await;
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "acquire should not sleep when buckets are full (took {:?})",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn is_ready_true_before_retry_after() {
        let limiter = RateLimiter::new("test", 60, 60_000);
        assert!(limiter.is_ready().await);
    }

    #[tokio::test]
    async fn retry_after_marks_not_ready() {
        let limiter = RateLimiter::new("test", 60, 60_000);
        limiter.set_retry_after(60).await;
        assert!(!limiter.is_ready().await);
    }

    /// Exercises `Bucket::wait_secs` when tokens are insufficient — covers
    /// the divisor branch that returns the projected wait.
    #[test]
    fn bucket_wait_secs_returns_projected_wait_when_short() {
        // 60 tokens/minute = 1 token/sec. Empty the bucket then ask for 30 tokens.
        let mut b = Bucket::new(60);
        assert!(b.try_consume(60.0));
        let wait = b.wait_secs(30.0);
        // Roughly 30 seconds — refill is 1 tok/sec from empty.
        assert!(wait > 29.0 && wait < 31.0, "got wait_secs = {wait}");
    }

    /// Exercises the Retry-After branch in `acquire`: sets a 2 s deadline,
    /// verifies the limiter parks under paused-time then clears the deadline.
    #[tokio::test(start_paused = true)]
    async fn acquire_honours_retry_after_window_then_clears_it() {
        let limiter = RateLimiter::new("test", 60, 60_000);
        limiter.set_retry_after(2).await;
        assert!(!limiter.is_ready().await);

        // Under paused time the sleep inside `acquire` advances virtual time
        // automatically. After the call completes the retry_after window
        // must be cleared.
        limiter.acquire(100).await;
        assert!(limiter.is_ready().await);
    }

    /// Drains the request bucket so `acquire` must sleep waiting for refill.
    /// Hits the `tokio::time::sleep` line inside the request-slot loop.
    ///
    /// `Bucket` uses `std::time::Instant` (not `tokio::time::Instant`), so
    /// paused tokio time does NOT speed up refill — the test runs in real
    /// time. We pick a high `rpm` so the wait is on the order of 100 ms.
    #[tokio::test]
    async fn acquire_waits_when_request_bucket_empty() {
        // 6000 rpm = 100 req/sec; capacity 6000.
        let limiter = RateLimiter::new("test", 6000, 60_000);
        // Drain the bucket.
        for _ in 0..6000 {
            limiter.acquire(0).await;
        }
        // Next acquire enters the wait loop and sleeps until ~10 ms refill
        // produces 1 token (effectively ~10 ms real time).
        limiter.acquire(0).await;
    }

    /// Drains the token bucket so `acquire` waits inside the token-budget loop.
    ///
    /// Same caveat as the request-bucket test: refill is real-time. We size
    /// `tpm` so the wait is ~100 ms.
    #[tokio::test]
    async fn acquire_waits_when_token_bucket_empty() {
        // 6000 tpm = 100 tok/sec; capacity 6000.
        let limiter = RateLimiter::new("test", 60_000, 6000);
        // Drain.
        limiter.acquire(6000).await;
        // Needs 10 more tokens → ~100 ms real-time wait.
        limiter.acquire(10).await;
    }
}
