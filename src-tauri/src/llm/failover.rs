//! LLM failover manager (design doc §22, flint-rust.mdc failover section).
//!
//! Decision tree:
//!   1. Primary call fails (5xx / timeout / connection refused)
//!   2. One immediate retry after 100ms
//!   3. If retry fails → silent failover to Ollama → emit `failover_triggered`
//!   4. Background task pings primary every 30 seconds
//!   5. On recovery → emit `primary_restored`
//!
//! 429 rate-limit path is separate: honour `Retry-After`, queue under the
//! rate-limiter, do NOT failover on the first 429.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::Stream;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Runtime};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::events::{
    emit_failover_triggered, emit_primary_restored, FailoverTriggeredPayload,
    PrimaryRestoredPayload,
};
use crate::llm::provider::{CompletionConfig, LLMProvider};
use crate::llm::rate_limiter::RateLimiter;

// ──────────────────────────────────────────────────────────────────────────────
// Constants
// ──────────────────────────────────────────────────────────────────────────────

const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(100);
const PRIMARY_PING_INTERVAL: Duration = Duration::from_secs(30);

// ──────────────────────────────────────────────────────────────────────────────
// Error classification
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum CallError {
    /// 429 with `Retry-After` seconds.
    RateLimit(u64),
    /// 5xx, timeout, connection refused — eligible for failover.
    Hard,
}

fn classify_error(err: &anyhow::Error) -> CallError {
    let msg = err.to_string();
    if let Some(stripped) = msg.strip_prefix("rate_limit:") {
        let secs = stripped.parse::<u64>().unwrap_or(10);
        return CallError::RateLimit(secs);
    }
    CallError::Hard
}

// ──────────────────────────────────────────────────────────────────────────────
// FailoverManager
// ──────────────────────────────────────────────────────────────────────────────

/// Manages the primary→Ollama failover cycle for a single provider.
///
/// Wrap in `Arc` and share across orchestrator threads.
pub struct FailoverManager {
    primary: Arc<dyn LLMProvider>,
    local: Arc<dyn LLMProvider>,
    rate_limiter: Arc<RateLimiter>,
    /// `true` when the local Ollama fallback is currently active.
    using_local: Arc<AtomicBool>,
    /// Background task handle — kept alive for the session duration.
    _ping_task: Option<JoinHandle<()>>,
}

impl FailoverManager {
    /// Create a new manager. Call `start_ping_loop` after construction to
    /// begin monitoring the primary.
    pub fn new(
        primary: Arc<dyn LLMProvider>,
        local: Arc<dyn LLMProvider>,
        rate_limiter: Arc<RateLimiter>,
    ) -> Self {
        Self {
            primary,
            local,
            rate_limiter,
            using_local: Arc::new(AtomicBool::new(false)),
            _ping_task: None,
        }
    }

    /// Spawn the background ping loop. Must be called once from an async context.
    pub fn start_ping_loop<R: Runtime>(&mut self, app: AppHandle<R>) {
        let primary = Arc::clone(&self.primary);
        let using_local = Arc::clone(&self.using_local);

        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(PRIMARY_PING_INTERVAL).await;

                if !using_local.load(Ordering::Relaxed) {
                    continue;
                }

                // Probe the primary with a minimal completion.
                let probe_cfg = CompletionConfig {
                    max_tokens: Some(1),
                    temperature: 0.0,
                    stream: false,
                };
                match primary.complete("ping".to_string(), probe_cfg).await {
                    Ok(_) => {
                        using_local.store(false, Ordering::Release);
                        info!(provider = %primary.name(), "primary LLM restored");
                        emit_primary_restored(
                            &app,
                            PrimaryRestoredPayload {
                                provider: primary.name().to_string(),
                            },
                        );
                    }
                    Err(e) => {
                        #[cfg(debug_assertions)]
                        tracing::debug!(error = %e, "primary still unavailable");
                        let _ = e;
                    }
                }
            }
        });

        self._ping_task = Some(handle);
    }

    /// Execute a streaming completion with automatic failover.
    ///
    /// - Acquires a rate-limit slot for the primary.
    /// - Calls primary; on hard failure retries once after 100ms.
    /// - On second failure, routes to local Ollama and emits `failover_triggered`.
    /// - On 429: parks at the rate-limiter backoff without routing to Ollama.
    pub async fn complete_stream<R: Runtime>(
        &self,
        prompt: String,
        config: CompletionConfig,
        app: &AppHandle<R>,
        estimated_tokens: u32,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        if self.using_local.load(Ordering::Acquire) {
            return self.local.complete_stream(prompt, config).await;
        }

        // Acquire rate-limit slot before calling primary.
        self.rate_limiter.acquire(estimated_tokens).await;

        let first_result = self
            .primary
            .complete_stream(prompt.clone(), config.clone())
            .await;

        match first_result {
            Ok(stream) => return Ok(stream),
            Err(ref e) => match classify_error(e) {
                CallError::RateLimit(secs) => {
                    self.rate_limiter.set_retry_after(secs).await;
                    // Re-acquire with the updated Retry-After then retry once.
                    self.rate_limiter.acquire(estimated_tokens).await;
                    return self.primary.complete_stream(prompt, config).await;
                }
                CallError::Hard => {
                    warn!(
                        provider = %self.primary.name(),
                        error = %e,
                        "primary LLM call failed — retrying after 100ms"
                    );
                }
            },
        }

        // One immediate retry.
        tokio::time::sleep(INITIAL_RETRY_DELAY).await;
        let retry_result = self
            .primary
            .complete_stream(prompt.clone(), config.clone())
            .await;

        match retry_result {
            Ok(stream) => return Ok(stream),
            Err(ref e) => {
                warn!(
                    provider = %self.primary.name(),
                    error = %e,
                    "primary LLM retry failed — falling over to Ollama"
                );
            }
        }

        // Failover to local Ollama.
        self.using_local.store(true, Ordering::Release);
        emit_failover_triggered(
            app,
            FailoverTriggeredPayload {
                from: self.primary.name().to_string(),
                to: self.local.name().to_string(),
            },
        );

        self.local.complete_stream(prompt, config).await
    }

    /// Whether the local Ollama fallback is currently active.
    pub fn is_using_local(&self) -> bool {
        self.using_local.load(Ordering::Acquire)
    }

    /// Provider name string — primary when healthy, "ollama" during fallback.
    pub fn active_provider_name(&self) -> &str {
        if self.is_using_local() {
            self.local.name()
        } else {
            self.primary.name()
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::{FailingMockLLMProvider, MockLLMProvider};
    use futures::StreamExt;
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};

    fn mock_app_handle() -> tauri::AppHandle<MockRuntime> {
        mock_builder()
            .build(mock_context(noop_assets()))
            .expect("mock app")
            .handle()
            .clone()
    }

    fn make_failover(
        primary: Arc<dyn LLMProvider>,
        local: Arc<dyn LLMProvider>,
    ) -> FailoverManager {
        let rl = Arc::new(RateLimiter::new("mock", 60, 60_000));
        FailoverManager::new(primary, local, rl)
    }

    #[test]
    fn active_provider_name_before_failover() {
        let primary: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "hello".to_string(),
            provider_name: "mock".to_string(),
        });
        let local: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "local".to_string(),
            provider_name: "ollama".to_string(),
        });
        let manager = make_failover(primary, local);
        assert_eq!(manager.active_provider_name(), "mock");
        assert!(!manager.is_using_local());
    }

    #[test]
    fn classify_rate_limit_error() {
        let err = anyhow::anyhow!("rate_limit:30");
        assert_eq!(classify_error(&err), CallError::RateLimit(30));
    }

    #[test]
    fn classify_hard_error() {
        let err = anyhow::anyhow!("connection refused");
        assert_eq!(classify_error(&err), CallError::Hard);
    }

    #[test]
    fn classify_missing_retry_after_defaults_to_ten() {
        let err = anyhow::anyhow!("rate_limit:not_a_number");
        assert_eq!(classify_error(&err), CallError::RateLimit(10));
    }

    #[tokio::test]
    async fn hard_failure_fails_over_to_local() {
        let primary: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
            provider_name: "groq".to_string(),
            error_message: "connection refused".to_string(),
        });
        let local: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "fallback answer".to_string(),
            provider_name: "ollama".to_string(),
        });
        let manager = make_failover(primary, Arc::clone(&local));

        // No real AppHandle — failover event emission is best-effort (let _ = emit).
        let app = mock_app_handle();

        let config = CompletionConfig {
            max_tokens: Some(50),
            temperature: 0.0,
            stream: true,
        };
        let mut stream = manager
            .complete_stream("test prompt".to_string(), config, &app, 100)
            .await
            .expect("failover should route to local");

        assert!(manager.is_using_local());
        let token = stream.next().await.unwrap().unwrap();
        assert_eq!(token, "fallback answer");
    }

    #[tokio::test]
    async fn rate_limit_retries_primary_without_failover() {
        use std::sync::atomic::{AtomicU32, Ordering};

        struct RateLimitThenOk {
            calls: AtomicU32,
            provider_name: String,
        }

        #[async_trait::async_trait]
        impl LLMProvider for RateLimitThenOk {
            async fn complete_stream(
                &self,
                _prompt: String,
                _config: CompletionConfig,
            ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    return Err(anyhow::anyhow!("rate_limit:0"));
                }
                let stream = futures::stream::once(async move { Ok("ok".to_string()) });
                Ok(Box::pin(stream))
            }
            fn name(&self) -> &str {
                &self.provider_name
            }
            fn is_available(&self) -> bool {
                true
            }
            fn context_window(&self) -> usize {
                128_000
            }
            fn rate_limit(&self) -> crate::llm::provider::RateLimit {
                crate::llm::provider::RateLimit {
                    requests_per_minute: 60,
                    tokens_per_minute: 6_000,
                }
            }
        }

        let primary: Arc<dyn LLMProvider> = Arc::new(RateLimitThenOk {
            calls: AtomicU32::new(0),
            provider_name: "groq".to_string(),
        });
        let local: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
            provider_name: "ollama".to_string(),
            error_message: "should not be called".to_string(),
        });
        let manager = make_failover(primary, local);

        let app = mock_app_handle();

        let config = CompletionConfig {
            max_tokens: Some(50),
            temperature: 0.0,
            stream: true,
        };
        let mut stream = manager
            .complete_stream("test".to_string(), config, &app, 100)
            .await
            .expect("429 retry should succeed on primary");

        assert!(!manager.is_using_local());
        let token = stream.next().await.unwrap().unwrap();
        assert_eq!(token, "ok");
    }
}
