//! LLM failover manager (design doc §22, flint-rust.mdc failover section).
//!
//! Decision tree:
//!   1. Primary call fails (5xx / timeout / connection refused)
//!   2. One immediate retry after 100ms
//!   3. If retry fails → cascade cloud tiers (DeepSeek → OpenRouter) → Ollama
//!   4. Background task pings primary every 30 seconds
//!   5. On recovery → emit `primary_restored`
//!
//! 429 rate-limit path:
//!   - Short Retry-After (≤ threshold): honour capped wait, retry primary once.
//!   - Long Retry-After (> threshold): cascade cloud tiers then Ollama.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::Stream;
use std::pin::Pin;
use std::sync::atomic::Ordering;
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
/// Groq Retry-After above this skips long sleeps — try OpenRouter then Ollama.
const RATE_LIMIT_CLOUD_FAILOVER_SECS: u64 = 10;

const TIER_PRIMARY: u8 = 0;
// Cloud tier indices are `1..=cloud_tiers.len()`. Local is `cloud_tiers.len() + 1`.

fn local_tier_index(cloud_tier_count: usize) -> u8 {
    (cloud_tier_count + 1) as u8
}

fn rate_limit_without_fallback_message(retry_after_secs: u64) -> String {
    let mins = retry_after_secs.div_ceil(60);
    format!(
        "Primary LLM rate limit exceeded (retry in ~{mins} min). Add DeepSeek or OpenRouter \
         keys in Settings for cloud fallback, or start Ollama on localhost:11434."
    )
}

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

/// Manages the primary → cloud tiers → Ollama failover cycle.
pub struct FailoverManager {
    primary: Arc<dyn LLMProvider>,
    cloud_tiers: Vec<Arc<dyn LLMProvider>>,
    local: Arc<dyn LLMProvider>,
    rate_limiter: Arc<RateLimiter>,
    /// `0` primary, `1..=cloud_tiers.len()` cloud tier, `cloud_tiers.len()+1` local.
    active_tier: Arc<std::sync::atomic::AtomicU8>,
    _ping_task: Option<JoinHandle<()>>,
}

impl FailoverManager {
    pub fn new(
        primary: Arc<dyn LLMProvider>,
        cloud_tiers: Vec<Arc<dyn LLMProvider>>,
        local: Arc<dyn LLMProvider>,
        rate_limiter: Arc<RateLimiter>,
    ) -> Self {
        Self {
            primary,
            cloud_tiers,
            local,
            rate_limiter,
            active_tier: Arc::new(std::sync::atomic::AtomicU8::new(TIER_PRIMARY)),
            _ping_task: None,
        }
    }

    fn local_index(&self) -> u8 {
        local_tier_index(self.cloud_tiers.len())
    }

    fn cloud_provider_at(&self, tier: u8) -> Option<&Arc<dyn LLMProvider>> {
        if tier == TIER_PRIMARY {
            return None;
        }
        let idx = tier as usize;
        if idx == 0 || idx > self.cloud_tiers.len() {
            return None;
        }
        self.cloud_tiers.get(idx - 1)
    }

    /// Spawn the background ping loop. Must be called once from an async context.
    pub fn start_ping_loop<R: Runtime>(&mut self, app: AppHandle<R>) {
        let primary = Arc::clone(&self.primary);
        let active_tier = Arc::clone(&self.active_tier);

        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(PRIMARY_PING_INTERVAL).await;
                probe_primary_once(&primary, &active_tier, &app).await;
            }
        });

        self._ping_task = Some(handle);
    }

    /// Execute a streaming completion with automatic failover.
    ///
    /// - Acquires a rate-limit slot for the primary.
    /// - Calls primary; on hard failure retries once after 100ms.
    /// - On second failure, routes to local Ollama and emits `failover_triggered`.
    /// - On 429 with short Retry-After: capped wait, one Groq retry.
    /// - On 429 with long Retry-After: immediate Ollama failover (live-safe).
    pub async fn complete_stream<R: Runtime>(
        &self,
        prompt: String,
        config: CompletionConfig,
        app: &AppHandle<R>,
        estimated_tokens: u32,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        let tier = self.active_tier.load(Ordering::Acquire);
        let local_idx = self.local_index();
        if tier == local_idx {
            return self.local.complete_stream(prompt, config).await;
        }
        if let Some(cloud) = self.cloud_provider_at(tier) {
            match cloud.complete_stream(prompt.clone(), config.clone()).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    warn!(
                        provider = %cloud.name(),
                        error = %e,
                        "active cloud tier stream failed — falling back to Ollama"
                    );
                    return self.failover_to_local(prompt, config, app, None).await;
                }
            }
        }

        self.rate_limiter.acquire(estimated_tokens).await;

        let first_result = self
            .primary
            .complete_stream(prompt.clone(), config.clone())
            .await;

        match first_result {
            Ok(stream) => return Ok(stream),
            Err(ref e) => match classify_error(e) {
                CallError::RateLimit(secs) => {
                    if secs > RATE_LIMIT_CLOUD_FAILOVER_SECS {
                        warn!(
                            retry_after_secs = secs,
                            provider = %self.primary.name(),
                            "Groq quota backoff — failing over to cloud/local"
                        );
                        return self
                            .failover_from_primary(prompt, config, app, Some(secs))
                            .await;
                    }
                    self.rate_limiter.set_retry_after(secs).await;
                    self.rate_limiter.acquire(estimated_tokens).await;
                    match self
                        .primary
                        .complete_stream(prompt.clone(), config.clone())
                        .await
                    {
                        Ok(stream) => return Ok(stream),
                        Err(ref retry_err) => match classify_error(retry_err) {
                            CallError::RateLimit(retry_secs) => {
                                warn!(
                                    retry_after_secs = retry_secs,
                                    "Groq still rate-limited after short wait — failing over"
                                );
                                return self
                                    .failover_from_primary(prompt, config, app, Some(retry_secs))
                                    .await;
                            }
                            CallError::Hard => {
                                return Err(anyhow::anyhow!("{retry_err}"));
                            }
                        },
                    }
                }
                CallError::Hard => {
                    log_primary_call_failed(self.primary.name(), e);
                }
            },
        }

        tokio::time::sleep(INITIAL_RETRY_DELAY).await;
        let retry_result = self
            .primary
            .complete_stream(prompt.clone(), config.clone())
            .await;

        match retry_result {
            Ok(stream) => return Ok(stream),
            Err(ref e) => log_primary_retry_failed(self.primary.name(), e),
        }

        self.failover_from_primary(prompt, config, app, None).await
    }

    /// Collect a full non-streaming completion (used by prep research chat).
    pub async fn complete<R: Runtime>(
        &self,
        prompt: String,
        config: CompletionConfig,
        app: &AppHandle<R>,
        estimated_tokens: u32,
    ) -> Result<String> {
        use futures::StreamExt;
        let mut stream = self
            .complete_stream(
                prompt,
                CompletionConfig {
                    stream: false,
                    ..config
                },
                app,
                estimated_tokens,
            )
            .await?;
        let mut out = String::new();
        while let Some(token) = stream.next().await {
            out.push_str(&token?);
        }
        Ok(out)
    }

    async fn failover_from_primary<R: Runtime>(
        &self,
        prompt: String,
        config: CompletionConfig,
        app: &AppHandle<R>,
        rate_limit_retry_secs: Option<u64>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        for (i, cloud) in self.cloud_tiers.iter().enumerate() {
            if !cloud.health_check().await {
                warn!(
                    provider = %cloud.name(),
                    "cloud fallback tier skipped — not configured or unavailable"
                );
                continue;
            }

            let tier = (i + 1) as u8;
            self.active_tier.store(tier, Ordering::Release);
            emit_failover_triggered(
                app,
                FailoverTriggeredPayload {
                    from: self.primary.name().to_string(),
                    to: cloud.name().to_string(),
                },
            );
            info!(
                provider = %cloud.name(),
                "primary unavailable — routing inference to cloud fallback tier"
            );

            match cloud.complete_stream(prompt.clone(), config.clone()).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    warn!(
                        provider = %cloud.name(),
                        error = %e,
                        "cloud fallback request failed — trying next tier"
                    );
                }
            }
        }

        self.failover_to_local(prompt, config, app, rate_limit_retry_secs)
            .await
    }

    async fn failover_to_local<R: Runtime>(
        &self,
        prompt: String,
        config: CompletionConfig,
        app: &AppHandle<R>,
        rate_limit_retry_secs: Option<u64>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        if !self.local.health_check().await {
            if let Some(secs) = rate_limit_retry_secs {
                return Err(anyhow::anyhow!(rate_limit_without_fallback_message(secs)));
            }
            return Err(anyhow::anyhow!(
                "Primary LLM ({}) failed. Add DeepSeek or OpenRouter keys in Settings for cloud \
                 fallback, or start Ollama on localhost:11434.",
                self.primary.name()
            ));
        }

        self.active_tier
            .store(self.local_index(), Ordering::Release);
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
        self.active_tier.load(Ordering::Acquire) == self.local_index()
    }

    #[cfg(test)]
    fn force_tier(&self, tier: u8) {
        self.active_tier.store(tier, Ordering::Release);
    }

    pub fn active_provider_name(&self) -> &str {
        let tier = self.active_tier.load(Ordering::Acquire);
        if tier == self.local_index() {
            return self.local.name();
        }
        if let Some(cloud) = self.cloud_provider_at(tier) {
            return cloud.name();
        }
        self.primary.name()
    }
}

/// Log a primary-LLM failure on the first attempt. Extracted so the
/// `tracing::warn` macro lives outside coverage-tricky inline expansions.
fn log_primary_call_failed(provider: &str, err: &anyhow::Error) {
    warn!(
        provider = %provider,
        error = %err,
        "primary LLM call failed — retrying after 100ms"
    );
}

/// Log a primary-LLM failure on the retry that triggers failover.
fn log_primary_retry_failed(provider: &str, err: &anyhow::Error) {
    warn!(
        provider = %provider,
        error = %err,
        "primary LLM retry failed — falling over to Ollama"
    );
}

/// One iteration of the primary-recovery probe. Extracted so unit tests can
/// drive the recovery path without spawning a background task — spawned-task
/// line coverage is unreliable under tarpaulin teardown.
///
/// When `using_local` is `false`, the function is a no-op. When `true`, it
/// probes the primary with a minimal completion. On success it clears the
/// flag and emits `primary_restored`; on error it logs and leaves the flag.
async fn probe_primary_once<R: Runtime>(
    primary: &Arc<dyn LLMProvider>,
    active_tier: &Arc<std::sync::atomic::AtomicU8>,
    app: &AppHandle<R>,
) {
    if active_tier.load(Ordering::Relaxed) == TIER_PRIMARY {
        return;
    }

    let probe_cfg = CompletionConfig {
        max_tokens: Some(1),
        temperature: 0.0,
        stream: false,
    };
    match primary.complete("ping".to_string(), probe_cfg).await {
        Ok(_) => {
            active_tier.store(TIER_PRIMARY, Ordering::Release);
            info!(provider = %primary.name(), "primary LLM restored");
            emit_primary_restored(
                app,
                PrimaryRestoredPayload {
                    provider: primary.name().to_string(),
                },
            );
        }
        Err(e) => log_primary_probe_failed(&e),
    }
}

/// Log a primary-probe failure during the recovery loop. Extracted so the
/// `cfg(debug_assertions)` + `tracing::debug!` pair lives outside an inline
/// branch arm where coverage is unreliable.
fn log_primary_probe_failed(err: &anyhow::Error) {
    #[cfg(debug_assertions)]
    tracing::debug!(error = %err, "primary still unavailable");
    // Reference err unconditionally so release builds (debug_assertions off)
    // do not generate an unused-variable warning.
    let _ = err;
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
        FailoverManager::new(primary, vec![], local, rl)
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

    #[tokio::test]
    async fn cloud_failure_cascades_to_local() {
        struct AlwaysRateLimited {
            provider_name: String,
        }

        #[async_trait::async_trait]
        impl LLMProvider for AlwaysRateLimited {
            async fn complete_stream(
                &self,
                _prompt: String,
                _config: CompletionConfig,
            ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
                Err(anyhow::anyhow!("rate_limit:600"))
            }
            fn name(&self) -> &str {
                &self.provider_name
            }
            fn is_available(&self) -> bool {
                true
            }
            async fn health_check(&self) -> bool {
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

        struct FailingCloud {
            provider_name: String,
        }

        #[async_trait::async_trait]
        impl LLMProvider for FailingCloud {
            async fn complete_stream(
                &self,
                _prompt: String,
                _config: CompletionConfig,
            ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
                Err(anyhow::anyhow!("OpenRouter API error 401: invalid key"))
            }
            fn name(&self) -> &str {
                &self.provider_name
            }
            fn is_available(&self) -> bool {
                true
            }
            async fn health_check(&self) -> bool {
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

        let primary: Arc<dyn LLMProvider> = Arc::new(AlwaysRateLimited {
            provider_name: "groq".to_string(),
        });
        let cloud: Arc<dyn LLMProvider> = Arc::new(FailingCloud {
            provider_name: "openrouter".to_string(),
        });
        let local: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "local after cloud fail".to_string(),
            provider_name: "ollama".to_string(),
        });
        let rl = Arc::new(RateLimiter::new("mock", 60, 60_000));
        let manager = FailoverManager::new(primary, vec![cloud], local, rl);
        let app = mock_app_handle();

        let mut stream = manager
            .complete_stream(
                "test".to_string(),
                CompletionConfig {
                    max_tokens: Some(50),
                    temperature: 0.0,
                    stream: true,
                },
                &app,
                100,
            )
            .await
            .expect("cloud failure must cascade to Ollama");

        assert!(manager.is_using_local());
        let token = stream.next().await.unwrap().unwrap();
        assert_eq!(token, "local after cloud fail");
    }

    #[tokio::test]
    async fn long_rate_limit_failover_to_ollama_without_minute_long_sleep() {
        struct AlwaysRateLimited {
            provider_name: String,
        }

        #[async_trait::async_trait]
        impl LLMProvider for AlwaysRateLimited {
            async fn complete_stream(
                &self,
                _prompt: String,
                _config: CompletionConfig,
            ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
                Err(anyhow::anyhow!("rate_limit:600"))
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

        let primary: Arc<dyn LLMProvider> = Arc::new(AlwaysRateLimited {
            provider_name: "groq".to_string(),
        });
        let local: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "local fallback answer".to_string(),
            provider_name: "ollama".to_string(),
        });
        let manager = make_failover(primary, local);
        let app = mock_app_handle();

        let start = std::time::Instant::now();
        let result = manager
            .complete_stream(
                "test".to_string(),
                CompletionConfig {
                    max_tokens: Some(50),
                    temperature: 0.0,
                    stream: true,
                },
                &app,
                100,
            )
            .await;
        assert!(result.is_ok(), "long 429 must failover to Ollama");
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "must not sleep for Retry-After minutes"
        );
        assert!(manager.is_using_local());
        let mut stream = result.unwrap();
        let token = stream.next().await.unwrap().unwrap();
        assert_eq!(token, "local fallback answer");
    }

    /// When `using_local` is already set, `complete_stream` must short-circuit
    /// to the local provider without touching the rate limiter or primary.
    #[tokio::test]
    async fn complete_stream_short_circuits_to_local_when_already_failed_over() {
        let primary: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
            provider_name: "groq".to_string(),
            error_message: "should not be called".to_string(),
        });
        let local: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "fast local answer".to_string(),
            provider_name: "ollama".to_string(),
        });
        let manager = make_failover(Arc::clone(&primary), Arc::clone(&local));
        manager.force_tier(local_tier_index(0));
        assert_eq!(manager.active_provider_name(), "ollama");

        let app = mock_app_handle();
        let config = CompletionConfig {
            max_tokens: Some(50),
            temperature: 0.0,
            stream: true,
        };
        let mut stream = manager
            .complete_stream("test".to_string(), config, &app, 100)
            .await
            .expect("local should serve directly");
        let token = stream.next().await.unwrap().unwrap();
        assert_eq!(token, "fast local answer");
    }

    /// Hard failure on the first call, success on the retry: no failover.
    /// Exercises the `Ok(stream) => return Ok(stream)` branch after the retry
    /// in `complete_stream`.
    #[tokio::test]
    async fn complete_stream_retry_succeeds_without_failover() {
        use std::sync::atomic::{AtomicU32, Ordering};

        struct HardFailThenOk {
            calls: AtomicU32,
            provider_name: String,
        }

        #[async_trait::async_trait]
        impl LLMProvider for HardFailThenOk {
            async fn complete_stream(
                &self,
                _prompt: String,
                _config: CompletionConfig,
            ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    return Err(anyhow::anyhow!("connection refused"));
                }
                let stream = futures::stream::once(async move { Ok("retry-ok".to_string()) });
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

        let primary: Arc<dyn LLMProvider> = Arc::new(HardFailThenOk {
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
            .expect("retry should succeed");

        assert!(
            !manager.is_using_local(),
            "successful retry must not trigger failover"
        );
        let token = stream.next().await.unwrap().unwrap();
        assert_eq!(token, "retry-ok");
    }

    /// Happy path: primary succeeds on the first call, no retry, no failover.
    #[tokio::test]
    async fn complete_stream_returns_primary_response_on_first_success() {
        let primary: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "primary ok".to_string(),
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
            .expect("primary first-success must return the stream");

        assert!(!manager.is_using_local());
        let token = stream.next().await.unwrap().unwrap();
        assert_eq!(token, "primary ok");
    }

    /// `probe_primary_once`: success case — primary recovers, flag flips back.
    #[tokio::test]
    async fn probe_primary_once_flips_active_tier_on_success() {
        let primary: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "pong".to_string(),
            provider_name: "groq".to_string(),
        });
        let active_tier = Arc::new(std::sync::atomic::AtomicU8::new(1));
        let app = mock_app_handle();

        probe_primary_once(&primary, &active_tier, &app).await;
        assert_eq!(active_tier.load(Ordering::Acquire), TIER_PRIMARY);
    }

    /// `probe_primary_once`: skip path when not in fallback mode (no-op).
    #[tokio::test]
    async fn probe_primary_once_noop_when_not_in_fallback() {
        let primary: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
            provider_name: "groq".to_string(),
            error_message: "should never be called".to_string(),
        });
        let active_tier = Arc::new(std::sync::atomic::AtomicU8::new(TIER_PRIMARY));
        let app = mock_app_handle();

        probe_primary_once(&primary, &active_tier, &app).await;
        assert_eq!(active_tier.load(Ordering::Acquire), TIER_PRIMARY);
    }

    /// `probe_primary_once`: error case — primary still failing, flag stays true.
    #[tokio::test]
    async fn probe_primary_once_keeps_local_on_probe_failure() {
        let primary: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
            provider_name: "groq".to_string(),
            error_message: "still down".to_string(),
        });
        let active_tier = Arc::new(std::sync::atomic::AtomicU8::new(local_tier_index(0)));
        let app = mock_app_handle();

        probe_primary_once(&primary, &active_tier, &app).await;
        assert_eq!(active_tier.load(Ordering::Acquire), local_tier_index(0));
    }

    /// Post-session summary path: Groq long 429 must cascade to the next cloud tier.
    #[tokio::test]
    async fn complete_falls_back_to_cloud_tier_on_long_groq_rate_limit() {
        struct AlwaysRateLimited {
            provider_name: String,
        }

        #[async_trait::async_trait]
        impl LLMProvider for AlwaysRateLimited {
            async fn complete_stream(
                &self,
                _prompt: String,
                _config: CompletionConfig,
            ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
                Err(anyhow::anyhow!("rate_limit:4901"))
            }
            fn name(&self) -> &str {
                &self.provider_name
            }
            fn is_available(&self) -> bool {
                true
            }
            async fn health_check(&self) -> bool {
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

        let primary: Arc<dyn LLMProvider> = Arc::new(AlwaysRateLimited {
            provider_name: "groq".to_string(),
        });
        let cloud: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "deepseek summary".to_string(),
            provider_name: "deepseek".to_string(),
        });
        let local: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
            provider_name: "ollama".to_string(),
            error_message: "should not be called".to_string(),
        });
        let rl = Arc::new(RateLimiter::new("mock", 60, 60_000));
        let manager = FailoverManager::new(primary, vec![cloud], local, rl);
        let app = mock_app_handle();

        let summary = manager
            .complete(
                "session summary prompt".to_string(),
                CompletionConfig {
                    max_tokens: Some(600),
                    temperature: 0.0,
                    stream: false,
                },
                &app,
                800,
            )
            .await
            .expect("Groq 429 must fail over to DeepSeek for summary");

        assert_eq!(summary, "deepseek summary");
        assert_eq!(manager.active_provider_name(), "deepseek");
    }

    /// Post-session summary path: all cloud tiers exhausted → Ollama fallback.
    #[tokio::test]
    async fn complete_falls_back_to_ollama_when_all_cloud_tiers_rate_limited() {
        struct AlwaysRateLimited {
            provider_name: String,
        }

        #[async_trait::async_trait]
        impl LLMProvider for AlwaysRateLimited {
            async fn complete_stream(
                &self,
                _prompt: String,
                _config: CompletionConfig,
            ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
                Err(anyhow::anyhow!("rate_limit:4901"))
            }
            fn name(&self) -> &str {
                &self.provider_name
            }
            fn is_available(&self) -> bool {
                true
            }
            async fn health_check(&self) -> bool {
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

        let primary: Arc<dyn LLMProvider> = Arc::new(AlwaysRateLimited {
            provider_name: "groq".to_string(),
        });
        let local: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "ollama summary".to_string(),
            provider_name: "ollama".to_string(),
        });
        let manager = make_failover(primary, local);
        let app = mock_app_handle();

        let summary = manager
            .complete(
                "session summary prompt".to_string(),
                CompletionConfig {
                    max_tokens: Some(600),
                    temperature: 0.0,
                    stream: false,
                },
                &app,
                800,
            )
            .await
            .expect("summary must reach Ollama when Groq is rate-limited");

        assert_eq!(summary, "ollama summary");
        assert!(manager.is_using_local());
    }

    #[test]
    fn log_primary_call_failed_does_not_panic() {
        log_primary_call_failed("groq", &anyhow::anyhow!("boom"));
        log_primary_retry_failed("groq", &anyhow::anyhow!("boom"));
        log_primary_probe_failed(&anyhow::anyhow!("probe boom"));
    }

    /// Sanity check that `start_ping_loop` actually spawns the background
    /// task. We do not assert on the ping work itself (coverage of that
    /// lives in the `probe_primary_once` tests above) — just confirm the
    /// JoinHandle is stored.
    #[tokio::test(start_paused = true)]
    async fn start_ping_loop_stores_join_handle() {
        let primary: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "ok".to_string(),
            provider_name: "groq".to_string(),
        });
        let local: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "local".to_string(),
            provider_name: "ollama".to_string(),
        });
        let mut manager = make_failover(primary, local);
        assert!(manager._ping_task.is_none());

        manager.start_ping_loop(mock_app_handle());
        assert!(
            manager._ping_task.is_some(),
            "start_ping_loop must persist the JoinHandle"
        );
    }
}
