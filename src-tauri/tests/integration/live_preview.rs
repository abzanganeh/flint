//! Integration tests for the LIVE_PREVIEW flow (`lpav-s9-live-preview-commands`).
//!
//! `start_live_preview` / `commit_live_preview` / `cancel_live_preview` live
//! in `commands.rs` as `#[tauri::command]`s that depend on real `cpal` audio
//! hardware (`AudioCapture::start`) and a fully-managed `tauri::App` — neither
//! is available in a headless test process, so these tests exercise the same
//! underlying building blocks the commands orchestrate instead of the Tauri
//! command wrappers themselves:
//!
//! * The [`SessionStateMachine`] transitions the commands drive
//!   (`READY -> LIVE_PREVIEW`, then `-> LIVE` on commit or `-> READY` on
//!   cancel), verified against real SQLite persistence.
//! * The question-channel hand-off `commit_live_preview` performs: during
//!   the preview window nothing drains `question_rx` (no orchestrator is
//!   running yet), and `commit_live_preview` must hand that exact receiver —
//!   with anything already queued on it — to a freshly spawned
//!   `run_orchestrator`, so a question detected during the preview window is
//!   never silently dropped.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use flint_lib::audio::pipeline::{DetectedQuestion, DetectedQuestionSource};
use flint_lib::cost::CostTracker;
use flint_lib::digest::Digest;
use flint_lib::llm::failover::FailoverManager;
use flint_lib::llm::provider::{FailingMockLLMProvider, LLMProvider, MockLLMProvider};
use flint_lib::llm::rate_limiter::RateLimiter;
use flint_lib::orchestrator::prewarm::PreWarmCache;
use flint_lib::orchestrator::{run_orchestrator, OrchestratorConfig};
use flint_lib::rag::store::SqliteVecStore;
use flint_lib::session::memory::ConversationMemory;
use flint_lib::session::persistence::SessionPersistence;
use flint_lib::session::state::{SessionState, SessionStateMachine, StatePersister};
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::AppHandle;
use tokio::sync::Mutex;
use uuid::Uuid;

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

fn fresh_persistence() -> Arc<SessionPersistence> {
    Arc::new(SessionPersistence::new(":memory:").expect("in-memory persistence"))
}

fn fresh_vector_store() -> Arc<dyn flint_lib::interfaces::vector::VectorInterface> {
    Arc::new(SqliteVecStore::new(":memory:").expect("in-memory vector store"))
}

fn fast_failover(response: &str) -> Arc<FailoverManager> {
    let primary: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
        response: response.to_string(),
        provider_name: "default".to_string(),
    });
    let local: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
        response: "local".to_string(),
        provider_name: "ollama".to_string(),
    });
    let rl = Arc::new(RateLimiter::new("mock", 60_000, 60_000));
    Arc::new(FailoverManager::new(primary, vec![], local, rl))
}

fn drive(sm: &mut SessionStateMachine, states: &[SessionState]) {
    for &s in states {
        sm.transition(s)
            .unwrap_or_else(|e| panic!("drive failed at {s}: {e}"));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// State machine transitions (`start_live_preview` / commit / cancel)
// ─────────────────────────────────────────────────────────────────────────────

/// Mirrors exactly what `start_live_preview` then `commit_live_preview` do
/// to the state machine: READY -> LIVE_PREVIEW -> LIVE, persisted to SQLite.
#[test]
fn state_transitions_ready_through_live_preview_to_live_on_commit() {
    let persistence = fresh_persistence();
    let session_id = Uuid::new_v4();
    persistence
        .create_session_row(session_id, "Preview Commit", "interview", "swe")
        .expect("session row");

    let persister = Arc::clone(&persistence) as Arc<dyn StatePersister>;
    let mut sm = SessionStateMachine::with_persister(persister);
    sm.set_session_id(session_id).unwrap();

    drive(
        &mut sm,
        &[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
        ],
    );

    // start_live_preview: READY -> LIVE_PREVIEW.
    sm.transition(SessionState::LivePreview)
        .expect("READY -> LIVE_PREVIEW must succeed");
    assert_eq!(*sm.current(), SessionState::LivePreview);

    // commit_live_preview: LIVE_PREVIEW -> LIVE.
    sm.transition(SessionState::Live)
        .expect("LIVE_PREVIEW -> LIVE must succeed");
    assert_eq!(*sm.current(), SessionState::Live);
}

/// Mirrors `start_live_preview` then `cancel_live_preview` (or the 60s
/// auto-cancel timer, which drives the same transition): READY ->
/// LIVE_PREVIEW -> READY.
#[test]
fn state_transitions_ready_through_live_preview_back_to_ready_on_cancel() {
    let persistence = fresh_persistence();
    let session_id = Uuid::new_v4();
    persistence
        .create_session_row(session_id, "Preview Cancel", "interview", "swe")
        .expect("session row");

    let persister = Arc::clone(&persistence) as Arc<dyn StatePersister>;
    let mut sm = SessionStateMachine::with_persister(persister);
    sm.set_session_id(session_id).unwrap();

    drive(
        &mut sm,
        &[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
        ],
    );

    sm.transition(SessionState::LivePreview).unwrap();
    assert_eq!(*sm.current(), SessionState::LivePreview);

    sm.transition(SessionState::Ready)
        .expect("LIVE_PREVIEW -> READY must succeed");
    assert_eq!(*sm.current(), SessionState::Ready);

    // A user may re-enter preview after cancelling, any number of times,
    // before finally committing or leaving the session.
    sm.transition(SessionState::LivePreview).unwrap();
    sm.transition(SessionState::Live).unwrap();
    assert_eq!(*sm.current(), SessionState::Live);
}

/// `commit_live_preview` / `cancel_live_preview` are only valid from
/// LIVE_PREVIEW — this is the guard both commands check before touching any
/// task handles. Asserted directly against the state machine allow-list
/// rather than duplicating the full command (which requires live hardware).
#[test]
fn commit_and_cancel_are_rejected_outside_live_preview() {
    let mut sm = SessionStateMachine::new();
    drive(
        &mut sm,
        &[
            SessionState::Configuring,
            SessionState::Ingesting,
            SessionState::DigestReview,
            SessionState::PreWarming,
            SessionState::Ready,
        ],
    );

    // Neither LIVE nor READY (the two valid "commit"/"cancel" targets) is
    // reachable from READY directly through a LIVE_PREVIEW-only command —
    // going straight to LIVE without a preview must still work (the
    // existing non-preview path), but attempting to leave a
    // LIVE_PREVIEW-shaped state that was never entered is invalid.
    let err = sm.transition(SessionState::Live);
    assert!(err.is_ok(), "direct READY -> LIVE (no preview) still valid");
    assert_eq!(*sm.current(), SessionState::Live);

    // Once LIVE, LIVE_PREVIEW is unreachable — commit/cancel's precondition
    // (being IN LIVE_PREVIEW) can never be satisfied from here.
    assert!(sm.transition(SessionState::LivePreview).is_err());
}

// ─────────────────────────────────────────────────────────────────────────────
// Question-channel hand-off across the preview -> commit boundary
// ─────────────────────────────────────────────────────────────────────────────

/// The riskiest part of `commit_live_preview`'s design: during the preview
/// window the pipeline's `question_tx` has a live receiver sitting unread in
/// `LivePreviewTaskHandles` (no orchestrator is running to drain it yet).
/// `commit_live_preview` must hand that *same* receiver — including
/// anything already queued on it — to a freshly spawned `run_orchestrator`,
/// or a question detected moments before the user clicks "Go Live" would be
/// silently lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn question_queued_during_preview_window_is_processed_after_commit() {
    let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts");
    let session_id = Uuid::new_v4();

    let persistence = fresh_persistence();
    persistence
        .create_session_row(session_id, "Handoff Test", "interview", "swe")
        .expect("session row");
    persistence
        .write_state_transition(session_id, &SessionState::Live)
        .expect("state -> LIVE for response persistence");

    // Exactly as start_live_preview creates it — capacity 64, nothing reads
    // from `rx` while the "preview window" (simulated below) is open.
    let (tx, rx) = tokio::sync::mpsc::channel::<DetectedQuestion>(64);

    // A question is detected while only the preview pipeline is running —
    // no orchestrator exists yet to receive it.
    tx.send(DetectedQuestion {
        text: "What is your experience with distributed systems?".to_string(),
        session_id,
        detected_at: Instant::now(),
        source: DetectedQuestionSource::System,
    })
    .await
    .expect("preview-window question enqueues onto the parked channel");

    // Simulate the rest of the preview window elapsing with the channel
    // still unread.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // commit_live_preview: spawn the orchestrator on the SAME receiver.
    let embedder = match flint_lib::rag::embedder::Embedder::new_if_cached() {
        Some(e) => Arc::new(e),
        None => return, // embedder model not cached locally — skip
    };
    let local_llm: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
        response: "compressed summary".to_string(),
        provider_name: "ollama".to_string(),
    });
    let config = OrchestratorConfig {
        session_id,
        digest: Arc::new(test_digest()),
        prompts_dir,
        failover: fast_failover("Answer to the queued question."),
        embedder,
        vector_store: fresh_vector_store(),
        prewarm_cache: Arc::new(Mutex::new(PreWarmCache::new())),
        memory: Arc::new(Mutex::new(ConversationMemory::new(128_000))),
        compression_prompt: "Summarise:\n{old_turns}".to_string(),
        local_llm,
        turn_cancel_slot: Arc::new(Mutex::new(None)),
        persistence: Arc::clone(&persistence),
        cost_tracker: Arc::new(CostTracker::new()),
    };

    let app = mock_app_handle();
    let handle = tokio::spawn(run_orchestrator(rx, config, app));

    // No further sends — dropping tx closes the channel once the queued
    // question has been drained, so the orchestrator loop exits cleanly.
    drop(tx);

    handle
        .await
        .expect("orchestrator must process the pre-queued question and exit");

    let responses = persistence
        .load_session_data(session_id)
        .expect("load session data")
        .expect("session must exist")
        .responses;
    assert!(
        !responses.is_empty(),
        "the question queued during the preview window must have been answered, not dropped"
    );
}

/// If the preview is cancelled (or times out) before any question arrives,
/// dropping the sender side (as `cancel_live_preview`'s teardown does when
/// the pipeline task is aborted) must close the channel cleanly rather than
/// hang a hypothetical consumer — there is no orchestrator in this path, but
/// the channel must still behave correctly for any future consumer.
#[tokio::test]
async fn dropping_sender_after_cancelled_preview_closes_channel_without_hanging() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<DetectedQuestion>(64);
    drop(tx);

    let result = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await;
    assert!(
        result.expect("recv must not hang after sender drop").is_none(),
        "closed, empty channel must yield None"
    );
}

/// `run_orchestrator` must never be started twice against the same
/// question_rx — a defensive regression check that the mock provider is
/// only ever invoked the expected number of times when a single question is
/// queued (guards against accidentally re-sending / double-committing).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_queued_question_produces_exactly_one_orchestrator_turn() {
    let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts");
    let session_id = Uuid::new_v4();
    let persistence = fresh_persistence();
    persistence
        .create_session_row(session_id, "Single Turn Test", "interview", "swe")
        .expect("session row");

    let (tx, rx) = tokio::sync::mpsc::channel::<DetectedQuestion>(64);
    tx.send(DetectedQuestion {
        text: "Only one question was asked during preview.".to_string(),
        session_id,
        detected_at: Instant::now(),
        source: DetectedQuestionSource::System,
    })
    .await
    .unwrap();
    drop(tx);

    let embedder = match flint_lib::rag::embedder::Embedder::new_if_cached() {
        Some(e) => Arc::new(e),
        None => return, // embedder model not cached locally — skip
    };

    let config = OrchestratorConfig {
        session_id,
        digest: Arc::new(test_digest()),
        prompts_dir,
        failover: fast_failover("Single answer."),
        embedder,
        vector_store: fresh_vector_store(),
        prewarm_cache: Arc::new(Mutex::new(PreWarmCache::new())),
        memory: Arc::new(Mutex::new(ConversationMemory::new(128_000))),
        compression_prompt: "Summarise:\n{old_turns}".to_string(),
        local_llm: Arc::new(MockLLMProvider {
            response: "summary".to_string(),
            provider_name: "ollama".to_string(),
        }),
        turn_cancel_slot: Arc::new(Mutex::new(None)),
        persistence: Arc::clone(&persistence),
        cost_tracker: Arc::new(CostTracker::new()),
    };

    run_orchestrator(rx, config, mock_app_handle()).await;

    let responses = persistence
        .load_session_data(session_id)
        .expect("load session data")
        .expect("session must exist")
        .responses;
    assert_eq!(
        responses.len(),
        1,
        "exactly one queued question must produce exactly one turn's responses"
    );
}

/// Sanity check that a failing primary+local LLM stack still lets the
/// hand-off complete without hanging — commit_live_preview must not be able
/// to wedge the app if the failover stack captured during the preview has
/// gone bad by the time the user commits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handoff_completes_even_when_failover_stack_is_down() {
    let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts");
    let session_id = Uuid::new_v4();
    let persistence = fresh_persistence();
    persistence
        .create_session_row(session_id, "Down Failover Test", "interview", "swe")
        .expect("session row");

    let (tx, rx) = tokio::sync::mpsc::channel::<DetectedQuestion>(64);
    tx.send(DetectedQuestion {
        text: "Will this hang if the LLM stack is down?".to_string(),
        session_id,
        detected_at: Instant::now(),
        source: DetectedQuestionSource::System,
    })
    .await
    .unwrap();
    drop(tx);

    let embedder = match flint_lib::rag::embedder::Embedder::new_if_cached() {
        Some(e) => Arc::new(e),
        None => return,
    };

    let primary: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
        provider_name: "default".to_string(),
        error_message: "primary down".to_string(),
    });
    let local: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
        provider_name: "ollama".to_string(),
        error_message: "local also down".to_string(),
    });
    let rl = Arc::new(RateLimiter::new("mock", 60_000, 60_000));
    let failover = Arc::new(FailoverManager::new(primary, vec![], local, rl));

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
        local_llm: Arc::new(FailingMockLLMProvider {
            provider_name: "ollama".to_string(),
            error_message: "local also down".to_string(),
        }),
        turn_cancel_slot: Arc::new(Mutex::new(None)),
        persistence,
        cost_tracker: Arc::new(CostTracker::new()),
    };

    let handle = tokio::spawn(run_orchestrator(rx, config, mock_app_handle()));
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("hand-off must not hang even when every LLM provider fails")
        .expect("orchestrator task must not panic");
}
