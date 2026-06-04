//! Integration test for Phase 4 orchestrator — parallel thread dispatch.
//!
//! Task 4.15: mock provider → directional + depth threads fire concurrently.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use flint_lib::digest::Digest;
use flint_lib::llm::failover::FailoverManager;
use flint_lib::llm::provider::{CompletionConfig, LLMProvider, MockLLMProvider, RateLimit};
use flint_lib::llm::rate_limiter::RateLimiter;
use flint_lib::orchestrator::depth;
use flint_lib::orchestrator::directional;
use flint_lib::orchestrator::OrchestrationContext;
use flint_lib::session::memory::MemoryContext;
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::AppHandle;
use uuid::Uuid;

const MOCK_DELAY_MS: u64 = 150;

struct DelayedMockLLMProvider {
    response: String,
    provider_name: String,
    delay: Duration,
}

#[async_trait::async_trait]
impl LLMProvider for DelayedMockLLMProvider {
    async fn complete_stream(
        &self,
        _prompt: String,
        _config: CompletionConfig,
    ) -> anyhow::Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = anyhow::Result<String>> + Send>>,
    > {
        tokio::time::sleep(self.delay).await;
        let response = self.response.clone();
        let stream = futures::stream::once(async move { Ok(response) });
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
    fn rate_limit(&self) -> RateLimit {
        RateLimit {
            requests_per_minute: 60,
            tokens_per_minute: 6_000,
        }
    }
}

fn mock_app_handle() -> AppHandle<MockRuntime> {
    mock_builder()
        .build(mock_context(noop_assets()))
        .expect("mock tauri app")
        .handle()
        .clone()
}

fn test_digest() -> Digest {
    Digest {
        role: "Engineer".to_string(),
        company: "Acme".to_string(),
        domain: "software engineering".to_string(),
        key_skills: vec!["Rust".to_string()],
        seniority: "senior".to_string(),
        likely_questions: vec!["Tell me about yourself".to_string()],
        topics_to_avoid: vec![],
    }
}

fn test_context(question: &str) -> OrchestrationContext {
    OrchestrationContext {
        session_id: Uuid::new_v4(),
        question: question.to_string(),
        rag_chunks: vec![],
        digest: Arc::new(test_digest()),
        memory_ctx: MemoryContext {
            rolling_summary: String::new(),
            recent_turns: String::new(),
            truncated: false,
        },
        from_cache: false,
        cached_directional: None,
        cached_depth: None,
        turn_cancel: Arc::new(AtomicBool::new(false)),
        turn_number: 1,
    }
}

fn make_failover(response: &str) -> Arc<FailoverManager> {
    let primary: Arc<dyn LLMProvider> = Arc::new(DelayedMockLLMProvider {
        response: response.to_string(),
        provider_name: "mock".to_string(),
        delay: Duration::from_millis(MOCK_DELAY_MS),
    });
    let local: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
        response: "local".to_string(),
        provider_name: "ollama".to_string(),
    });
    let rl = Arc::new(RateLimiter::new("mock", 60, 60_000));
    Arc::new(FailoverManager::new(primary, local, rl))
}

#[tokio::test]
async fn directional_and_depth_threads_run_concurrently() {
    let app = mock_app_handle();
    let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts");

    let dir_ctx = test_context("What is your experience with Rust?");
    let dep_ctx = test_context("What is your experience with Rust?");

    let dir_failover = make_failover("directional answer");
    let dep_failover = make_failover("depth answer");

    let dir_app = app.clone();
    let dep_app = app.clone();
    let dir_prompts = prompts_dir.clone();
    let dep_prompts = prompts_dir;

    let start = Instant::now();
    let dir_task = tokio::spawn(async move {
        directional::run_directional(dir_ctx, dir_failover, &dir_prompts, dir_app).await
    });
    let dep_task = tokio::spawn(async move {
        depth::run_depth(dep_ctx, dep_failover, &dep_prompts, dep_app).await
    });

    let (dir_result, dep_result) = tokio::join!(dir_task, dep_task);
    let elapsed = start.elapsed();

    assert!(dir_result.unwrap().is_ok());
    assert!(dep_result.unwrap().is_ok());

    let sequential_floor = Duration::from_millis(MOCK_DELAY_MS * 2);
    assert!(
        elapsed < sequential_floor,
        "threads ran sequentially: elapsed={elapsed:?} expected < {sequential_floor:?}"
    );
}

#[tokio::test]
async fn cache_hit_serves_directional_without_llm_call() {
    let app = mock_app_handle();
    let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts");

    let mut ctx = test_context("Tell me about yourself");
    ctx.cached_directional = Some("Cached directional response.".to_string());
    ctx.from_cache = true;

    let failover = make_failover("should not be called");

    let start = Instant::now();
    let result = directional::run_directional(ctx, failover, &prompts_dir, app)
        .await
        .expect("cache serve should succeed");
    let elapsed = start.elapsed();

    assert_eq!(result, "Cached directional response.");
    assert!(
        elapsed < Duration::from_millis(MOCK_DELAY_MS),
        "cache path should not wait for LLM delay"
    );
}
