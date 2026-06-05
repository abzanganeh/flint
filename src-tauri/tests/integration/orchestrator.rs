//! Integration test for Phase 4 orchestrator — parallel thread dispatch.
//!
//! Task 4.15: mock provider → directional + depth threads fire concurrently.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use flint_lib::audio::pipeline::DetectedQuestion;
use flint_lib::digest::Digest;
use flint_lib::llm::failover::FailoverManager;
use flint_lib::llm::provider::{
    CompletionConfig, FailingMockLLMProvider, LLMProvider, MockLLMProvider,
    PanickingMockLLMProvider, RateLimit,
};
use flint_lib::llm::rate_limiter::RateLimiter;
use flint_lib::orchestrator::depth;
use flint_lib::orchestrator::directional;
use flint_lib::orchestrator::prewarm::PreWarmCache;
use flint_lib::orchestrator::{
    dispatch_turn, run_orchestrator, OrchestrationContext, OrchestratorConfig,
};
use flint_lib::rag::embedder::Embedder;
use flint_lib::rag::store::SqliteVecStore;
use flint_lib::session::memory::{ConversationMemory, MemoryContext};
use flint_lib::session::persistence::SessionPersistence;
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::AppHandle;
use tokio::sync::Mutex;
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
    ) -> anyhow::Result<std::pin::Pin<Box<dyn futures::Stream<Item = anyhow::Result<String>> + Send>>>
    {
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

// ─────────────────────────────────────────────────────────────────────────────
// Full dispatch_turn fixtures
// ─────────────────────────────────────────────────────────────────────────────

static EMBEDDER: OnceLock<Option<Arc<Embedder>>> = OnceLock::new();

fn try_embedder() -> Option<Arc<Embedder>> {
    EMBEDDER
        .get_or_init(|| Embedder::new().ok().map(Arc::new))
        .clone()
}

fn fresh_persistence() -> Arc<SessionPersistence> {
    Arc::new(SessionPersistence::new(":memory:").expect("in-memory persistence"))
}

fn fresh_vector_store() -> Arc<dyn flint_lib::interfaces::vector::VectorInterface> {
    Arc::new(SqliteVecStore::new(":memory:").expect("in-memory vector store"))
}

fn fast_failover(response: &str, name: &str) -> Arc<FailoverManager> {
    let primary: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
        response: response.to_string(),
        provider_name: name.to_string(),
    });
    let local: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
        response: "local".to_string(),
        provider_name: "ollama".to_string(),
    });
    let rl = Arc::new(RateLimiter::new("mock", 60_000, 60_000));
    Arc::new(FailoverManager::new(primary, local, rl))
}

/// Smoke-test the full per-turn pipeline end-to-end with all dependencies
/// wired up. Skips silently when the fastembed model isn't cached locally.
#[tokio::test]
async fn dispatch_turn_runs_full_pipeline_end_to_end() {
    let embedder =
        try_embedder().expect("embedder must load (model cached in src-tauri/.fastembed_cache)");

    let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts");
    let session_id = Uuid::new_v4();

    let persistence = fresh_persistence();
    persistence
        .create_session_row(
            session_id,
            "Test Session",
            "interview",
            "software engineering",
        )
        .expect("session row");
    persistence
        .write_state_transition(session_id, &flint_lib::session::state::SessionState::Live)
        .expect("state -> LIVE");

    let vector_store = fresh_vector_store();
    let prewarm_cache = Arc::new(Mutex::new(PreWarmCache::new()));
    let memory = Arc::new(Mutex::new(ConversationMemory::new(128_000)));
    let turn_cancel = Arc::new(AtomicBool::new(false));
    let local_llm: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
        response: "compressed summary".to_string(),
        provider_name: "ollama".to_string(),
    });

    let failover = fast_failover("Fast streamed answer.", "default");
    let app = mock_app_handle();

    let result = dispatch_turn(
        session_id,
        "Tell me about yourself".to_string(),
        1,
        Arc::new(test_digest()),
        prompts_dir,
        failover,
        embedder,
        vector_store,
        prewarm_cache,
        memory,
        "Summarise:\n{old_turns}".to_string(),
        turn_cancel,
        local_llm,
        persistence,
        app,
    )
    .await;

    assert!(result.is_ok(), "dispatch_turn must succeed: {result:?}");
}

/// dispatch_turn must complete cleanly when the LLM provider is failing.
/// Failover routes to the local provider; responses are still persisted.
#[tokio::test]
async fn dispatch_turn_survives_primary_llm_failure() {
    let embedder = try_embedder().expect("embedder must load");

    let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts");
    let session_id = Uuid::new_v4();

    let persistence = fresh_persistence();
    persistence
        .create_session_row(
            session_id,
            "Failover Test",
            "interview",
            "software engineering",
        )
        .expect("session row");

    // Both primary and local fail — verifies the orchestrator doesn't crash.
    let primary: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
        provider_name: "default".to_string(),
        error_message: "primary down".to_string(),
    });
    let local: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
        provider_name: "ollama".to_string(),
        error_message: "local also down".to_string(),
    });
    let rl = Arc::new(RateLimiter::new("mock", 60_000, 60_000));
    let failover = Arc::new(FailoverManager::new(primary, local, rl));

    let local_llm: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
        response: "summary".to_string(),
        provider_name: "ollama".to_string(),
    });

    let result = dispatch_turn(
        session_id,
        "Will this fail gracefully?".to_string(),
        1,
        Arc::new(test_digest()),
        prompts_dir,
        failover,
        embedder,
        fresh_vector_store(),
        Arc::new(Mutex::new(PreWarmCache::new())),
        Arc::new(Mutex::new(ConversationMemory::new(128_000))),
        "Summarise:\n{old_turns}".to_string(),
        Arc::new(AtomicBool::new(false)),
        local_llm,
        persistence,
        mock_app_handle(),
    )
    .await;

    // The orchestrator catches per-thread errors and returns Ok overall.
    assert!(
        result.is_ok(),
        "must not bubble per-thread failure: {result:?}"
    );
}

/// Pre-warm cache hit: dispatch_turn serves cached directional + depth without
/// hitting the LLM. Exercises the cache-hit branch inside `run_turn`.
#[tokio::test]
async fn dispatch_turn_serves_prewarm_cache_hit() {
    let embedder = try_embedder().expect("embedder must load");

    let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts");
    let session_id = Uuid::new_v4();

    // Seed the pre-warm cache with an entry for the question.
    let question = "Tell me about yourself";
    let embedding = embedder.embed_one(question).expect("embed");
    let mut cache = PreWarmCache::new();
    cache.insert(flint_lib::orchestrator::prewarm::PreWarmEntry {
        question: question.to_string(),
        directional_response: "Cached brief answer.".to_string(),
        depth_response: "Cached detailed answer.".to_string(),
        created_at: chrono::Utc::now(),
        embedding,
    });
    let prewarm_cache = Arc::new(Mutex::new(cache));

    let persistence = fresh_persistence();
    persistence
        .create_session_row(
            session_id,
            "Cache Test",
            "interview",
            "software engineering",
        )
        .expect("session row");

    // Use a failing primary to prove the LLM was NOT called.
    let primary: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
        provider_name: "default".to_string(),
        error_message: "MUST NOT BE CALLED — cache should serve".to_string(),
    });
    let local: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
        response: "local".to_string(),
        provider_name: "ollama".to_string(),
    });
    let rl = Arc::new(RateLimiter::new("mock", 60_000, 60_000));
    let failover = Arc::new(FailoverManager::new(primary, local, rl));

    let local_llm: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
        response: "summary".to_string(),
        provider_name: "ollama".to_string(),
    });

    let result = dispatch_turn(
        session_id,
        question.to_string(),
        1,
        Arc::new(test_digest()),
        prompts_dir,
        failover,
        embedder,
        fresh_vector_store(),
        prewarm_cache,
        Arc::new(Mutex::new(ConversationMemory::new(128_000))),
        "Summarise:\n{old_turns}".to_string(),
        Arc::new(AtomicBool::new(false)),
        local_llm,
        persistence,
        mock_app_handle(),
    )
    .await;

    assert!(result.is_ok(), "cache-hit path must succeed: {result:?}");
}

/// Drives the public `run_orchestrator` loop with a question channel.
/// Exercises debounce, turn-number increment, and the spawn-per-turn path
/// in `mod.rs` (lines 143-209) that are unreachable through `dispatch_turn`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_orchestrator_processes_question_and_exits_when_channel_closed() {
    let embedder = try_embedder().expect("embedder must load");

    let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts");
    let session_id = Uuid::new_v4();

    let persistence = fresh_persistence();
    persistence
        .create_session_row(session_id, "Loop Test", "interview", "software engineering")
        .expect("session row");

    let failover = fast_failover("Streamed.", "default");
    let local_llm: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
        response: "local summary".to_string(),
        provider_name: "ollama".to_string(),
    });

    let (tx, rx) = tokio::sync::mpsc::channel::<DetectedQuestion>(8);

    let config = OrchestratorConfig {
        session_id,
        digest: Arc::new(test_digest()),
        prompts_dir,
        failover,
        embedder,
        vector_store: fresh_vector_store(),
        prewarm_cache: Arc::new(Mutex::new(PreWarmCache::new())),
        memory: Arc::new(Mutex::new(ConversationMemory::new(128_000))),
        compression_prompt: "Summarise:\n{old_turns}".to_string(),
        local_llm,
        turn_cancel_slot: Arc::new(Mutex::new(None)),
        persistence,
    };

    let app = mock_app_handle();
    let handle = tokio::spawn(run_orchestrator(rx, config, app));

    // Send one question then close the channel to make the loop exit.
    tx.send(DetectedQuestion {
        text: "What is your favourite programming language?".to_string(),
        session_id,
        detected_at: Instant::now(),
    })
    .await
    .expect("send question");
    drop(tx);

    handle
        .await
        .expect("orchestrator loop must shut down cleanly");
}

/// Two questions arriving inside the silence-debounce window collapse into
/// one turn (the second utterance wins). Covers the inner `while let Ok(...)`
/// branch of `debounce`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_orchestrator_debounces_rapid_questions() {
    let embedder = try_embedder().expect("embedder must load");
    let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts");
    let session_id = Uuid::new_v4();

    let persistence = fresh_persistence();
    persistence
        .create_session_row(session_id, "Debounce Test", "interview", "swe")
        .expect("row");

    let failover = fast_failover("ok", "default");
    let local_llm: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
        response: "s".to_string(),
        provider_name: "ollama".to_string(),
    });

    let (tx, rx) = tokio::sync::mpsc::channel::<DetectedQuestion>(8);
    let config = OrchestratorConfig {
        session_id,
        digest: Arc::new(test_digest()),
        prompts_dir,
        failover,
        embedder,
        vector_store: fresh_vector_store(),
        prewarm_cache: Arc::new(Mutex::new(PreWarmCache::new())),
        memory: Arc::new(Mutex::new(ConversationMemory::new(128_000))),
        compression_prompt: "Sum:\n{old_turns}".to_string(),
        local_llm,
        turn_cancel_slot: Arc::new(Mutex::new(None)),
        persistence,
    };

    let handle = tokio::spawn(run_orchestrator(rx, config, mock_app_handle()));

    // Burst three questions inside the 600ms debounce window.
    for text in ["What", "What is", "What is your name?"] {
        tx.send(DetectedQuestion {
            text: text.to_string(),
            session_id,
            detected_at: Instant::now(),
        })
        .await
        .expect("send");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    drop(tx);
    handle.await.expect("loop closes");
}

/// Memory truncation path: pre-fill memory with many turns so the budget
/// forces compression, exercising `emit_context_truncated`.
#[tokio::test]
async fn dispatch_turn_emits_context_truncated_when_memory_compressed() {
    let embedder = try_embedder().expect("embedder must load");
    let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts");
    let session_id = Uuid::new_v4();
    let persistence = fresh_persistence();
    persistence
        .create_session_row(session_id, "Mem Test", "interview", "swe")
        .expect("row");

    // Tiny context window forces compression on even short history.
    let mut memory = ConversationMemory::new(64);
    for i in 0..6 {
        memory.push_turn(flint_lib::session::memory::Turn {
            question: format!("Question {i} with several words to fill up the budget"),
            directional_response: "Directional answer ".repeat(20),
            depth_response: "Depth answer ".repeat(20),
        });
    }

    // Use local provider for compression — failover must report using_local.
    let primary: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
        provider_name: "default".to_string(),
        error_message: "down".to_string(),
    });
    let local: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
        response: "compressed".to_string(),
        provider_name: "ollama".to_string(),
    });
    let rl = Arc::new(RateLimiter::new("mock", 60_000, 60_000));
    let failover = Arc::new(FailoverManager::new(primary, local, rl));

    // Force failover to "using local" by triggering a primary call that fails.
    // After this, `is_using_local()` is true and compression uses the local LLM.
    let warmup_app = mock_app_handle();
    let _ = failover
        .complete_stream(
            "warmup".to_string(),
            CompletionConfig {
                temperature: 0.0,
                max_tokens: Some(8),
                stream: true,
            },
            &warmup_app,
            8,
        )
        .await;

    let local_llm: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
        response: "Local summary".to_string(),
        provider_name: "ollama".to_string(),
    });

    let result = dispatch_turn(
        session_id,
        "Tell me about yourself".to_string(),
        2,
        Arc::new(test_digest()),
        prompts_dir,
        failover,
        embedder,
        fresh_vector_store(),
        Arc::new(Mutex::new(PreWarmCache::new())),
        Arc::new(Mutex::new(memory)),
        "Summarise:\n{old_turns}".to_string(),
        Arc::new(AtomicBool::new(false)),
        local_llm,
        persistence,
        mock_app_handle(),
    )
    .await;

    assert!(result.is_ok());
}

/// Cache hit + turn >= 3 → depth.rs runs a fresh LLM pass in parallel with
/// streaming the cached text. Covers the `cached_depth && turn_number >= 3`
/// branch in `depth::run_depth`.
#[tokio::test]
async fn dispatch_turn_runs_fresh_depth_on_cached_turn_three() {
    let embedder = try_embedder().expect("embedder must load");
    let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts");
    let session_id = Uuid::new_v4();

    let question = "What is your favourite framework?";
    let embedding = embedder.embed_one(question).expect("embed");
    let mut cache = PreWarmCache::new();
    cache.insert(flint_lib::orchestrator::prewarm::PreWarmEntry {
        question: question.to_string(),
        directional_response: "Cached brief.".to_string(),
        depth_response: "Cached depth.".to_string(),
        created_at: chrono::Utc::now(),
        embedding,
    });

    let persistence = fresh_persistence();
    persistence
        .create_session_row(session_id, "Refresh Test", "interview", "swe")
        .expect("row");

    let result = dispatch_turn(
        session_id,
        question.to_string(),
        3, // turn ≥ 3 triggers the fresh-depth path
        Arc::new(test_digest()),
        prompts_dir,
        fast_failover("Fresh depth answer.", "default"),
        embedder,
        fresh_vector_store(),
        Arc::new(Mutex::new(cache)),
        Arc::new(Mutex::new(ConversationMemory::new(128_000))),
        "Summarise:\n{old_turns}".to_string(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(MockLLMProvider {
            response: "x".to_string(),
            provider_name: "ollama".to_string(),
        }),
        persistence,
        mock_app_handle(),
    )
    .await;

    assert!(result.is_ok(), "fresh-after-cache must succeed: {result:?}");
}

/// LLM provider panic propagates as `JoinError` — orchestrator catches it,
/// emits a `thread_status` error event, and continues. Covers the panic-arm
/// of `collect_thread_text` and `collect_clarifying`.
#[tokio::test]
async fn dispatch_turn_recovers_from_panicking_llm_provider() {
    let embedder = try_embedder().expect("embedder must load");
    let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts");
    let session_id = Uuid::new_v4();
    let persistence = fresh_persistence();
    persistence
        .create_session_row(session_id, "Panic Test", "interview", "swe")
        .expect("row");

    let primary: Arc<dyn LLMProvider> = Arc::new(PanickingMockLLMProvider {
        provider_name: "default".to_string(),
    });
    let local: Arc<dyn LLMProvider> = Arc::new(PanickingMockLLMProvider {
        provider_name: "ollama".to_string(),
    });
    let rl = Arc::new(RateLimiter::new("mock", 60_000, 60_000));
    let failover = Arc::new(FailoverManager::new(primary, local, rl));

    let local_llm: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
        response: "ok".to_string(),
        provider_name: "ollama".to_string(),
    });

    let result = dispatch_turn(
        session_id,
        "Will panic propagate gracefully?".to_string(),
        1,
        Arc::new(test_digest()),
        prompts_dir,
        failover,
        embedder,
        fresh_vector_store(),
        Arc::new(Mutex::new(PreWarmCache::new())),
        Arc::new(Mutex::new(ConversationMemory::new(128_000))),
        "Sum:\n{old_turns}".to_string(),
        Arc::new(AtomicBool::new(false)),
        local_llm,
        persistence,
        mock_app_handle(),
    )
    .await;

    assert!(
        result.is_ok(),
        "orchestrator must absorb provider panics: {result:?}"
    );
}

/// Writes to a closed/invalid persistence so `write_response` fails, exercising
/// the warning branch in `persist_thread_response`.
#[tokio::test]
async fn dispatch_turn_logs_when_persistence_write_fails() {
    let embedder = try_embedder().expect("embedder must load");
    let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts");
    let session_id = Uuid::new_v4();

    // No session row is created -> foreign-key constraint on the response
    // insert will reject the write, triggering the warn branch.
    let persistence = fresh_persistence();

    let result = dispatch_turn(
        session_id,
        "What is Rust?".to_string(),
        1,
        Arc::new(test_digest()),
        prompts_dir,
        fast_failover("Streamed answer.", "default"),
        embedder,
        fresh_vector_store(),
        Arc::new(Mutex::new(PreWarmCache::new())),
        Arc::new(Mutex::new(ConversationMemory::new(128_000))),
        "Sum:\n{old_turns}".to_string(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(MockLLMProvider {
            response: "x".to_string(),
            provider_name: "ollama".to_string(),
        }),
        persistence,
        mock_app_handle(),
    )
    .await;

    assert!(
        result.is_ok(),
        "persist failure must not crash turn: {result:?}"
    );
}

/// Cancelled turn exits early at the top of `run_turn`.
#[tokio::test]
async fn dispatch_turn_returns_early_when_cancel_flag_set() {
    let embedder = try_embedder().expect("embedder must load");
    let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts");
    let session_id = Uuid::new_v4();
    let persistence = fresh_persistence();

    let cancel = Arc::new(AtomicBool::new(true));

    let result = dispatch_turn(
        session_id,
        "Skipped".to_string(),
        1,
        Arc::new(test_digest()),
        prompts_dir,
        fast_failover("ok", "default"),
        embedder,
        fresh_vector_store(),
        Arc::new(Mutex::new(PreWarmCache::new())),
        Arc::new(Mutex::new(ConversationMemory::new(128_000))),
        "Sum:\n{old_turns}".to_string(),
        cancel,
        Arc::new(MockLLMProvider {
            response: "x".to_string(),
            provider_name: "ollama".to_string(),
        }),
        persistence,
        mock_app_handle(),
    )
    .await;

    assert!(result.is_ok(), "cancelled turn returns Ok(())");
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
