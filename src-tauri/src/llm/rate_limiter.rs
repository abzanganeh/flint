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
        (amount - self.tokens) / self.refill_rate
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// RateLimiter
// ──────────────────────────────────────────────────────────────────────────────

/// Per-provider rate limiter.
///
/// Wrap in `Arc<RateLimiter>` and share across orchestrator threads.
pub struct RateLimiter {
    provider_name: String,
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
            request_bucket: Arc::new(Mutex::new(Bucket::new(rpm))),
            token_bucket: Arc::new(Mutex::new(Bucket::new(tpm))),
            retry_after: Arc::new(Mutex::new(None)),
        }
    }

    /// Acquire a request slot and `estimated_tokens` from the token bucket.
    ///
    /// Blocks (via `tokio::time::sleep`) until both are available. Does NOT
    /// switch to a fallback provider — that decision belongs to the
    /// `FailoverManager`.
    pub async fn acquire(&self, estimated_tokens: u32) {
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
        let deadline = Instant::now() + Duration::from_secs(secs);
        *self.retry_after.lock().await = Some(deadline);
        warn!(
            provider = %self.provider_name,
            retry_after_secs = secs,
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
    async fn acquire_completes_when_tokens_available() {
        let limiter = RateLimiter::new("test", 60, 60_000);
        // Should return almost immediately since both buckets start full.
        let start = Instant::now();
        limiter.acquire(100).await;
        assert!(start.elapsed() < Duration::from_millis(100));
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
}
