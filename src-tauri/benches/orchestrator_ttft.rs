//! Orchestrator TTFT P95 — NFR < 800ms (CI gate < 900ms).
//!
//! This bench isolates the Rust-side overhead of the streaming path
//! (FailoverManager + provider streaming + first-token plumbing) using
//! deterministic mock providers. Real-world TTFT is dominated by the LLM
//! provider's network/inference latency, which we cannot exercise in CI; what
//! we *can* gate is that our own orchestration overhead remains negligible
//! relative to the 800ms NFR budget.
//!
//! Two scenarios:
//! - `failover_immediate_first_token`: primary mock yields immediately —
//!   measures the streaming + rate-limit + emit overhead.
//! - `failover_with_simulated_latency`: primary mock yields after 50ms —
//!   exercises the same path with a realistic-ish provider response time
//!   so the bench surfaces orchestration-side regressions even when the
//!   provider is "fast".

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use futures::stream::{self, Stream, StreamExt};
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::AppHandle;

use flint_lib::llm::failover::FailoverManager;
use flint_lib::llm::provider::{CompletionConfig, LLMProvider, MockLLMProvider, RateLimit};
use flint_lib::llm::rate_limiter::RateLimiter;

/// Provider that streams a fixed response token-by-token after a configurable
/// initial delay. Used to simulate realistic provider response times without
/// going over the network.
struct DelayedMockProvider {
    name: String,
    initial_delay: Duration,
    response: String,
}

#[async_trait]
impl LLMProvider for DelayedMockProvider {
    async fn complete_stream(
        &self,
        _prompt: String,
        _config: CompletionConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        let mut tokens: Vec<String> = self
            .response
            .split_whitespace()
            .map(|t| format!("{t} "))
            .collect();
        // Empty responses are uninteresting and not realistic for the bench
        // — `MockLLMProvider` already covers that path.
        let first = tokens.remove(0);
        let delay = self.initial_delay;

        let first_with_delay = stream::once(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            Ok(first)
        });
        let remaining = stream::iter(tokens.into_iter().map(Ok));
        Ok(Box::pin(first_with_delay.chain(remaining)))
    }

    fn name(&self) -> &str {
        &self.name
    }
    fn is_available(&self) -> bool {
        true
    }
    fn context_window(&self) -> usize {
        128_000
    }
    fn rate_limit(&self) -> RateLimit {
        RateLimit {
            requests_per_minute: 600,
            tokens_per_minute: 60_000,
        }
    }
}

fn mock_app_handle() -> AppHandle<MockRuntime> {
    mock_builder()
        .build(mock_context(noop_assets()))
        .expect("mock app")
        .handle()
        .clone()
}

fn build_failover(initial_delay: Duration) -> FailoverManager {
    let primary: Arc<dyn LLMProvider> = Arc::new(DelayedMockProvider {
        name: "bench-primary".to_string(),
        initial_delay,
        response: "Brief directional answer about platform reliability.".to_string(),
    });
    let local: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
        response: "fallback".to_string(),
        provider_name: "bench-local".to_string(),
    });
    // Rate limits set high enough that the limiter never gates the bench —
    // we are measuring streaming overhead, not token-bucket wait time.
    let limiter = Arc::new(RateLimiter::new("bench-primary", u32::MAX, u32::MAX));
    FailoverManager::new(primary, None, local, limiter)
}

fn bench_ttft(c: &mut Criterion) {
    let app = mock_app_handle();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let mut group = c.benchmark_group("orchestrator_ttft");
    // Streaming benches need long enough measurement windows for criterion's
    // bootstrap to produce stable percentiles.
    group.sample_size(50);
    group.measurement_time(Duration::from_secs(15));

    for delay_ms in [0_u64, 50] {
        let label = BenchmarkId::new("primary_to_first_token", delay_ms);
        let failover = Arc::new(build_failover(Duration::from_millis(delay_ms)));
        group.bench_with_input(label, &delay_ms, |b, _| {
            let failover = failover.clone();
            let app = app.clone();
            b.iter(|| {
                rt.block_on(async {
                    let config = CompletionConfig {
                        temperature: 0.2,
                        max_tokens: Some(64),
                        stream: true,
                    };
                    let mut stream = failover
                        .complete_stream(black_box("Bench question".to_string()), config, &app, 16)
                        .await
                        .expect("complete_stream");
                    // Pull only the first token — that is what TTFT measures.
                    let first = stream.next().await.expect("first token").expect("ok");
                    black_box(first);
                });
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_ttft);
criterion_main!(benches);
