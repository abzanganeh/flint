//! Orchestration layer: directional/depth/clarifying response threads,
//! pre-warm cache, and session lifecycle management.
//!
//! Reference: design doc §8 (System Architecture), `.cursor/rules` flint-core
//! §4 (parallel threads via tokio::spawn, never sequential).
//!
//! ## Concurrency contract
//!
//! All three response threads are spawned via `tokio::spawn` in a single
//! statement — there is NO `.await` between spawns. One thread failing never
//! affects the others.
//!
//! ## Silence debounce
//!
//! After a `DetectedQuestion` arrives the orchestrator waits 600ms (task 4.10).
//! If a new question arrives within that window the timer resets and the older
//! question is discarded. This prevents double-firing on split utterances.

pub mod clarifying;
pub mod depth;
pub mod directional;
pub mod prewarm;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tauri::{AppHandle, Runtime};
use tokio::sync::{mpsc, Mutex};
use tracing::{info, info_span, warn, Instrument};
use uuid::Uuid;

use crate::audio::pipeline::DetectedQuestion;
use crate::confidence::{compute_confidence, ConfidenceLevel, ConfidenceSignals};
use crate::digest::Digest;
use crate::events::{
    emit_confidence_score, emit_context_truncated, emit_cost_cap_status, emit_inference_suspended,
    emit_rag_chunks_update, emit_response_metadata, emit_thread_status, emit_token_usage_update,
    ConfidenceScorePayload, ContextTruncatedPayload, CostCapStatusPayload,
    InferenceSuspendedPayload, RagChunkPayload, RagChunksUpdatePayload, ResponseMetadataPayload,
    ThreadStatusPayload, TokenUsageUpdatePayload,
};
use crate::interfaces::vector::{ScoredChunk, VectorInterface};
use crate::llm::failover::FailoverManager;
use crate::llm::provider::LLMProvider;
use crate::orchestrator::prewarm::PreWarmCache;
use crate::rag::embedder::Embedder;
use crate::rag::retriever::retrieve;
use crate::session::memory::{ContextBudget, ConversationMemory, MemoryContext, Turn};
use crate::session::persistence::{Response, ResponseType, SessionPersistence};
use crate::state::TurnCancelFlag;

// ──────────────────────────────────────────────────────────────────────────────
// Silence debounce
// ──────────────────────────────────────────────────────────────────────────────

/// Wait this long after the last detected question before firing threads.
const SILENCE_DEBOUNCE: Duration = Duration::from_millis(600);

// ──────────────────────────────────────────────────────────────────────────────
// Shared context passed to each thread
// ──────────────────────────────────────────────────────────────────────────────

/// Everything a response thread needs to build its prompt and produce a response.
#[derive(Clone)]
pub struct OrchestrationContext {
    pub session_id: Uuid,
    pub question: String,
    pub rag_chunks: Vec<ScoredChunk>,
    pub digest: Arc<Digest>,
    pub memory_ctx: MemoryContext,
    /// True if this response was served from the pre-warm cache.
    pub from_cache: bool,
    /// Cached directional text when pre-warm cache hit (≥ 0.85 cosine).
    pub cached_directional: Option<String>,
    /// Cached depth text when pre-warm cache hit.
    pub cached_depth: Option<String>,
    /// Per-turn cancellation flag — set by `cancel_inference`.
    pub turn_cancel: TurnCancelFlag,
    /// 1-indexed turn number in the current session.
    pub turn_number: usize,
}

// ──────────────────────────────────────────────────────────────────────────────
// Prompt loader
// ──────────────────────────────────────────────────────────────────────────────

/// Load a prompt template from `/prompts/{category}/{provider}.txt`.
/// Falls back to `default.txt` if the provider-specific variant does not exist.
pub fn load_prompt(category: &str, provider: &str, prompts_dir: &Path) -> Result<String> {
    let provider_path = prompts_dir.join(category).join(format!("{provider}.txt"));
    if provider_path.exists() {
        return std::fs::read_to_string(&provider_path)
            .with_context(|| format!("cannot read prompt {}", provider_path.display()));
    }
    let default_path = prompts_dir.join(category).join("default.txt");
    std::fs::read_to_string(&default_path)
        .with_context(|| format!("cannot read default prompt {}", default_path.display()))
}

// ──────────────────────────────────────────────────────────────────────────────
// Mean RAG grounding score
// ──────────────────────────────────────────────────────────────────────────────

fn mean_rag_score(chunks: &[ScoredChunk]) -> f32 {
    if chunks.is_empty() {
        return 0.0;
    }
    let top = chunks.iter().take(3);
    let sum: f32 = top.map(|c| c.score).sum();
    sum / chunks.len().min(3) as f32
}

/// Collapse the nested `JoinError`/thread `Result` into a plain text payload.
/// Emits a `thread_status` error event whenever the task panicked or the
/// thread itself returned an error. Extracted so tarpaulin attributes the
/// panic and error branches to the call site (inline closures + macros were
/// reported as uncovered even when hit).
fn persist_thread_response(
    persistence: &SessionPersistence,
    session_id: Uuid,
    response_type: ResponseType,
    text: &str,
) {
    if text.is_empty() {
        return;
    }
    let r = Response {
        id: Uuid::new_v4(),
        session_id,
        response_type,
        content: text.to_string(),
        confidence: 0.0,
    };
    if let Err(e) = persistence.write_response(&r) {
        warn!(
            session_id = %session_id,
            error = %e,
            "thread persist failed"
        );
    }
}

fn collect_thread_text<R: Runtime>(
    result: std::result::Result<Result<String>, tokio::task::JoinError>,
    session_id: Uuid,
    thread: &str,
    app: &AppHandle<R>,
) -> String {
    match result {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => {
            log_thread_failed(session_id, thread, &e);
            emit_thread_error(app, thread);
            String::new()
        }
        Err(join_err) => {
            log_thread_panicked(session_id, thread, &join_err);
            emit_thread_error(app, thread);
            String::new()
        }
    }
}

fn collect_clarifying(
    result: std::result::Result<Result<Option<String>>, tokio::task::JoinError>,
    session_id: Uuid,
) -> bool {
    match result {
        Ok(Ok(Some(_))) => true,
        Ok(Ok(None)) => false,
        Ok(Err(e)) => {
            log_thread_failed(session_id, "clarifying", &e);
            false
        }
        Err(join_err) => {
            log_thread_panicked(session_id, "clarifying", &join_err);
            false
        }
    }
}

fn emit_thread_error<R: Runtime>(app: &AppHandle<R>, thread: &str) {
    emit_thread_status(
        app,
        ThreadStatusPayload {
            thread: thread.to_string(),
            status: "error".to_string(),
        },
    );
}

fn log_thread_failed(session_id: Uuid, thread: &str, error: &anyhow::Error) {
    warn!(session_id = %session_id, thread, error = %error, "thread failed");
}

fn log_thread_panicked(session_id: Uuid, thread: &str, join_err: &tokio::task::JoinError) {
    warn!(session_id = %session_id, thread, "task panicked: {join_err}");
}

/// Helper extracted so tarpaulin attributes coverage to the call site.
/// Inline `info!` arguments are otherwise reported as uncovered even when hit.
#[allow(clippy::too_many_arguments)]
fn log_confidence_computed(
    session_id: Uuid,
    turn: usize,
    confidence_score: f32,
    confidence_level: ConfidenceLevel,
    provider: &str,
    cache_hit: bool,
    rag_latency_ms: u64,
    failover_triggered: bool,
) {
    info!(
        session_id = %session_id,
        turn = turn,
        event = "directional_thread_complete",
        thread_type = "directional",
        confidence = confidence_score,
        level = %confidence_level.as_str(),
        provider = %provider,
        cache_hit = cache_hit,
        rag_latency_ms,
        failover_triggered = failover_triggered,
        "confidence computed"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Main orchestrator loop
// ──────────────────────────────────────────────────────────────────────────────

/// Configuration passed once when starting the orchestrator task.
pub struct OrchestratorConfig {
    pub session_id: Uuid,
    pub digest: Arc<Digest>,
    pub prompts_dir: PathBuf,
    pub failover: Arc<FailoverManager>,
    pub embedder: Arc<Embedder>,
    pub vector_store: Arc<dyn VectorInterface>,
    pub prewarm_cache: Arc<Mutex<PreWarmCache>>,
    pub memory: Arc<Mutex<ConversationMemory>>,
    pub compression_prompt: String,
    /// Local Ollama provider — used for history compression during failover.
    pub local_llm: Arc<dyn LLMProvider>,
    /// Shared with `LiveTaskHandles` — replaced on each turn dispatch.
    pub turn_cancel_slot: Arc<Mutex<Option<TurnCancelFlag>>>,
    /// Write-through SQLite persistence for crash recovery.
    pub persistence: Arc<SessionPersistence>,
    /// Phase 7.4 — cumulative cost / token accounting. Checked pre-dispatch
    /// to enforce the cap and updated post-dispatch with the turn's usage.
    pub cost_tracker: Arc<crate::cost::CostTracker>,
}

/// Receive detected questions and run the three parallel response threads.
///
/// Designed to be spawned as a `tokio::task` for the duration of the live
/// session. Exits when `question_rx` is closed (i.e. `stop_session` drops the
/// sender).
pub async fn run_orchestrator<R: Runtime>(
    mut question_rx: mpsc::Receiver<DetectedQuestion>,
    config: OrchestratorConfig,
    app: AppHandle<R>,
) {
    info!(session_id = %config.session_id, "orchestrator started");

    let mut turn_number: usize = 0;

    while let Some(first) = question_rx.recv().await {
        // ── Silence debounce ─────────────────────────────────────────────────
        // Drain additional questions that arrive within the debounce window,
        // keeping only the last one (most complete utterance).
        let question = debounce(&mut question_rx, first, SILENCE_DEBOUNCE).await;

        turn_number += 1;
        let turn = turn_number;

        info!(
            session_id = %config.session_id,
            turn = turn,
            question = %question.text,
            "orchestrator dispatching turn"
        );

        let turn_cancel = Arc::new(AtomicBool::new(false));
        {
            let mut slot = config.turn_cancel_slot.lock().await;
            *slot = Some(Arc::clone(&turn_cancel));
        }

        // Dispatch in its own task so the loop can accept the next question
        // while this turn is still processing.
        let app_clone = app.clone();
        let cfg = OrchestratorTurnConfig {
            session_id: config.session_id,
            question_text: question.text.clone(),
            digest: Arc::clone(&config.digest),
            prompts_dir: config.prompts_dir.clone(),
            failover: Arc::clone(&config.failover),
            embedder: Arc::clone(&config.embedder),
            vector_store: Arc::clone(&config.vector_store),
            prewarm_cache: Arc::clone(&config.prewarm_cache),
            memory: Arc::clone(&config.memory),
            compression_prompt: config.compression_prompt.clone(),
            turn_number: turn,
            turn_cancel,
            local_llm: Arc::clone(&config.local_llm),
            persistence: Arc::clone(&config.persistence),
            cost_tracker: Arc::clone(&config.cost_tracker),
        };

        let span = info_span!(
            "orchestrator_turn",
            session_id = %cfg.session_id,
            turn = cfg.turn_number,
        );
        tokio::spawn(
            async move {
                if let Err(e) = run_turn(cfg, app_clone).await {
                    warn!(error = %e, "orchestrator turn failed");
                }
            }
            .instrument(span),
        );
    }

    info!(session_id = %config.session_id, "orchestrator stopped");
}

/// Drain the channel for `window` duration, returning the last question seen.
/// This implements the 600ms silence debounce — if the speaker adds more words
/// within the window the stale partial question is discarded.
async fn debounce(
    rx: &mut mpsc::Receiver<DetectedQuestion>,
    first: DetectedQuestion,
    window: Duration,
) -> DetectedQuestion {
    let mut latest = first;
    while let Ok(Some(newer)) = tokio::time::timeout(window, rx.recv()).await {
        latest = newer;
    }
    latest
}

// ──────────────────────────────────────────────────────────────────────────────
// Per-turn execution
// ──────────────────────────────────────────────────────────────────────────────

struct OrchestratorTurnConfig {
    session_id: Uuid,
    question_text: String,
    digest: Arc<Digest>,
    prompts_dir: PathBuf,
    failover: Arc<FailoverManager>,
    embedder: Arc<Embedder>,
    vector_store: Arc<dyn VectorInterface>,
    prewarm_cache: Arc<Mutex<PreWarmCache>>,
    memory: Arc<Mutex<ConversationMemory>>,
    compression_prompt: String,
    turn_number: usize,
    turn_cancel: TurnCancelFlag,
    local_llm: Arc<dyn LLMProvider>,
    persistence: Arc<SessionPersistence>,
    cost_tracker: Arc<crate::cost::CostTracker>,
}

async fn run_turn<R: Runtime>(cfg: OrchestratorTurnConfig, app: AppHandle<R>) -> Result<()> {
    if cfg.turn_cancel.load(Ordering::Acquire) {
        return Ok(());
    }

    // ── Phase 7.4 — cap pre-check ────────────────────────────────────────
    // Reject the turn before any LLM call when the tracker is suspended.
    // The frontend already received an `inference_suspended` event on the
    // transition; this gate just guarantees no further spend can accrue
    // until the user explicitly lifts the suspension.
    if cfg.cost_tracker.is_suspended() {
        let snap = cfg.cost_tracker.snapshot();
        emit_inference_suspended(
            &app,
            InferenceSuspendedPayload {
                reason: "cost_cap_reached",
                total_tokens: snap.usage.total_tokens,
                cost_estimate_usd: snap.usage.cost_estimate_usd,
            },
        );
        return Ok(());
    }

    let rag_start = std::time::Instant::now();
    // ── 1. Embed the question ─────────────────────────────────────────────
    let embedder = Arc::clone(&cfg.embedder);
    let question_clone = cfg.question_text.clone();
    let embedding = tokio::task::spawn_blocking(move || {
        embedder
            .embed_one(&question_clone)
            .context("question embedding failed")
    })
    .await
    .context("embed task panicked")?
    .context("embed failed")?;

    // ── 2. Pre-warm cache lookup ──────────────────────────────────────────
    // Clone strings out of the cache entry before releasing the lock so the
    // MutexGuard is dropped before the first `.await` in `retrieve_rag`.
    let cache_hit = {
        let cache = cfg.prewarm_cache.lock().await;
        cache
            .lookup(&embedding)
            .map(|e| (e.directional_response.clone(), e.depth_response.clone()))
    };

    let from_cache = cache_hit.is_some();
    let (cached_directional, cached_depth) = match cache_hit {
        Some((dir, dep)) => (Some(dir), Some(dep)),
        None => (None, None),
    };
    if from_cache {
        info!(
            session_id = %cfg.session_id,
            turn = cfg.turn_number,
            event = "prewarm_cache_hit",
            "pre-warm cache hit"
        );
    }

    let rag_chunks = retrieve_rag(&cfg, &embedding).await?;
    let rag_latency_ms = rag_start.elapsed().as_millis() as u64;

    emit_rag_chunks_update(
        &app,
        RagChunksUpdatePayload {
            chunks: rag_chunks
                .iter()
                .take(10)
                .map(|c| RagChunkPayload {
                    text: c.chunk.text.clone(),
                    score: c.score,
                })
                .collect(),
        },
    );

    if from_cache {
        emit_response_metadata(&app, ResponseMetadataPayload { pre_prepared: true });
    }

    // ── 3. Build memory context ───────────────────────────────────────────
    let using_local = cfg.failover.is_using_local();
    let compression_llm: Option<Arc<dyn LLMProvider>> = if using_local {
        Some(Arc::clone(&cfg.local_llm))
    } else {
        None
    };

    let memory_ctx = {
        let mem = cfg.memory.lock().await;
        let budget = ContextBudget::from_window(if using_local { 4_096 } else { 128_000 });
        mem.build_context(
            &budget,
            compression_llm.as_ref(),
            &cfg.compression_prompt,
            cfg.session_id,
        )
        .await?
    };

    if memory_ctx.truncated {
        emit_context_truncated(
            &app,
            ContextTruncatedPayload {
                session_id: cfg.session_id.to_string(),
            },
        );
    }

    let rag_grounding = mean_rag_score(&rag_chunks);

    // ── 4. Build shared context ───────────────────────────────────────────
    let ctx = OrchestrationContext {
        session_id: cfg.session_id,
        question: cfg.question_text.clone(),
        rag_chunks: rag_chunks.clone(),
        digest: Arc::clone(&cfg.digest),
        memory_ctx,
        from_cache,
        cached_directional,
        cached_depth,
        turn_cancel: Arc::clone(&cfg.turn_cancel),
        turn_number: cfg.turn_number,
    };

    // ── 5. Spawn three threads concurrently ───────────────────────────────
    // RULE: no .await between spawns — all three are dispatched simultaneously.
    let dir_ctx = ctx.clone();
    let dep_ctx = ctx.clone();
    let cla_ctx = ctx.clone();

    let dir_app = app.clone();
    let dep_app = app.clone();
    let cla_app = app.clone();

    let dir_failover = Arc::clone(&cfg.failover);
    let dep_failover = Arc::clone(&cfg.failover);
    let cla_failover = Arc::clone(&cfg.failover);

    let dir_prompts = cfg.prompts_dir.clone();
    let dep_prompts = cfg.prompts_dir.clone();
    let cla_prompts = cfg.prompts_dir.clone();

    let dir_task = tokio::spawn(async move {
        directional::run_directional(dir_ctx, dir_failover, &dir_prompts, dir_app).await
    });

    let dep_task = tokio::spawn(async move {
        depth::run_depth(dep_ctx, dep_failover, &dep_prompts, dep_app).await
    });

    let cla_task = tokio::spawn(async move {
        clarifying::run_clarifying(cla_ctx, cla_failover, &cla_prompts, cla_app).await
    });

    // Collect results — one thread failing never crashes the others.
    let (dir_result, dep_result, _cla_result) = tokio::join!(dir_task, dep_task, cla_task);

    let directional_text = collect_thread_text(dir_result, cfg.session_id, "directional", &app);
    let depth_text = collect_thread_text(dep_result, cfg.session_id, "depth", &app);
    let clarifying_emitted = collect_clarifying(_cla_result, cfg.session_id);

    // ── 6. Confidence scoring ─────────────────────────────────────────────
    if clarifying_emitted {
        emit_confidence_score(
            &app,
            ConfidenceScorePayload {
                level: ConfidenceLevel::Grey.as_str().to_string(),
            },
        );
    } else {
        let rag_texts: Vec<String> = rag_chunks
            .iter()
            .take(3)
            .map(|c| c.chunk.text.clone())
            .collect();

        let signals = ConfidenceSignals {
            rag_grounding,
            response_text: directional_text.clone(),
            rag_texts,
            provider_name: cfg.failover.active_provider_name().to_string(),
            cache_stale: from_cache && cfg.turn_number > 3,
            local_fallback_active: cfg.failover.is_using_local(),
            turn_number: cfg.turn_number,
        };
        let (confidence_score, confidence_level) = compute_confidence(&signals);

        log_confidence_computed(
            cfg.session_id,
            cfg.turn_number,
            confidence_score,
            confidence_level,
            cfg.failover.active_provider_name(),
            from_cache,
            rag_latency_ms,
            cfg.failover.is_using_local(),
        );

        emit_confidence_score(
            &app,
            ConfidenceScorePayload {
                level: confidence_level.as_str().to_string(),
            },
        );
    }

    // ── 7. Persist responses — crash-recovery insurance ───────────────────
    persist_thread_response(
        &cfg.persistence,
        cfg.session_id,
        ResponseType::Directional,
        &directional_text,
    );
    persist_thread_response(
        &cfg.persistence,
        cfg.session_id,
        ResponseType::Depth,
        &depth_text,
    );

    // ── 8. Update conversation memory ─────────────────────────────────────
    {
        let mut mem = cfg.memory.lock().await;
        mem.push_turn(Turn {
            question: cfg.question_text.clone(),
            directional_response: directional_text.clone(),
            depth_response: depth_text.clone(),
        });
    }

    // ── 8. Token usage estimate (4 chars ≈ 1 token) ───────────────────────
    // The +500 fudge accounts for the system prompt + RAG chunks injected
    // into every call. It overestimates slightly, which is the safer side
    // to err on for a hard cost cap.
    let turn_input = (cfg.question_text.len() as u64 + 500) / 4;
    let turn_output = (directional_text.len() as u64 + depth_text.len() as u64 + 500) / 4;
    let total = turn_input + turn_output;
    let cost_estimate = total as f64 * 0.0000002;
    emit_token_usage_update(
        &app,
        TokenUsageUpdatePayload {
            input: turn_input,
            output: turn_output,
            total,
            cost_estimate,
        },
    );

    // ── 9. Phase 7.4 — record against the cap and emit transitions ───────
    let (snap, is_transition) =
        cfg.cost_tracker
            .record_turn_with_transition(turn_input, turn_output, cost_estimate);
    if is_transition {
        let status_str: &'static str = match snap.status {
            crate::cost::CostCapStatus::Ok => "ok",
            crate::cost::CostCapStatus::Warning80 => "warning_80",
            crate::cost::CostCapStatus::Reached => "reached",
        };
        emit_cost_cap_status(
            &app,
            CostCapStatusPayload {
                status: status_str,
                suspended: snap.suspended,
                input_tokens: snap.usage.input_tokens,
                output_tokens: snap.usage.output_tokens,
                total_tokens: snap.usage.total_tokens,
                cost_estimate_usd: snap.usage.cost_estimate_usd,
                max_total_tokens: snap.cap.max_total_tokens,
                max_cost_estimate_usd: snap.cap.max_cost_estimate_usd,
                fraction_used: snap.fraction_used,
            },
        );
        if matches!(snap.status, crate::cost::CostCapStatus::Reached) {
            emit_inference_suspended(
                &app,
                InferenceSuspendedPayload {
                    reason: "cost_cap_reached",
                    total_tokens: snap.usage.total_tokens,
                    cost_estimate_usd: snap.usage.cost_estimate_usd,
                },
            );
        }
    }

    Ok(())
}

/// Run a single orchestrator turn (rehearsal or direct dispatch).
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_turn<R: Runtime>(
    session_id: Uuid,
    question_text: String,
    turn_number: usize,
    digest: Arc<Digest>,
    prompts_dir: PathBuf,
    failover: Arc<FailoverManager>,
    embedder: Arc<Embedder>,
    vector_store: Arc<dyn VectorInterface>,
    prewarm_cache: Arc<Mutex<PreWarmCache>>,
    memory: Arc<Mutex<ConversationMemory>>,
    compression_prompt: String,
    turn_cancel: TurnCancelFlag,
    local_llm: Arc<dyn LLMProvider>,
    persistence: Arc<SessionPersistence>,
    cost_tracker: Arc<crate::cost::CostTracker>,
    app: AppHandle<R>,
) -> Result<()> {
    run_turn(
        OrchestratorTurnConfig {
            session_id,
            question_text,
            digest,
            prompts_dir,
            failover,
            embedder,
            vector_store,
            prewarm_cache,
            memory,
            compression_prompt,
            turn_number,
            turn_cancel,
            local_llm,
            persistence,
            cost_tracker,
        },
        app,
    )
    .await
}

async fn retrieve_rag(cfg: &OrchestratorTurnConfig, embedding: &[f32]) -> Result<Vec<ScoredChunk>> {
    let chunks = retrieve(
        cfg.vector_store.as_ref(),
        cfg.session_id,
        embedding,
        10,
        0.7,
    )
    .await
    .context("RAG retrieval failed")?;
    Ok(chunks)
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_prompt_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let category = dir.path().join("directional");
        std::fs::create_dir_all(&category).unwrap();
        std::fs::write(category.join("default.txt"), "default template").unwrap();

        let result = load_prompt("directional", "nonexistent_provider", dir.path());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "default template");
    }

    #[test]
    fn load_prompt_uses_provider_specific_file() {
        let dir = tempfile::tempdir().unwrap();
        let category = dir.path().join("directional");
        std::fs::create_dir_all(&category).unwrap();
        std::fs::write(category.join("default.txt"), "default").unwrap();
        std::fs::write(category.join("groq.txt"), "groq variant").unwrap();

        let result = load_prompt("directional", "groq", dir.path());
        assert_eq!(result.unwrap(), "groq variant");
    }

    #[test]
    fn load_prompt_errors_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let category = dir.path().join("directional");
        std::fs::create_dir_all(&category).unwrap();

        assert!(load_prompt("directional", "any", dir.path()).is_err());
    }

    #[test]
    fn mean_rag_score_averages_top_three() {
        use crate::interfaces::vector::{Chunk, ScoredChunk};
        use uuid::Uuid;

        let make_chunk = |score: f32| ScoredChunk {
            chunk: Chunk {
                id: Uuid::new_v4(),
                session_id: Uuid::new_v4(),
                text: "text".to_string(),
                embedding: vec![],
            },
            score,
        };

        let chunks = vec![
            make_chunk(0.9),
            make_chunk(0.8),
            make_chunk(0.7),
            make_chunk(0.6),
        ];
        let score = mean_rag_score(&chunks);
        // top 3: (0.9 + 0.8 + 0.7) / 3 = 0.8
        assert!((score - 0.8).abs() < 0.01, "score={score}");
    }

    #[test]
    fn mean_rag_score_empty_returns_zero() {
        assert_eq!(mean_rag_score(&[]), 0.0);
    }

    #[tokio::test]
    async fn debounce_returns_latest_question() {
        let (tx, mut rx) = mpsc::channel(8);
        let first = DetectedQuestion {
            text: "first".to_string(),
            session_id: Uuid::new_v4(),
            detected_at: std::time::Instant::now(),
        };
        let second = DetectedQuestion {
            text: "second".to_string(),
            session_id: Uuid::new_v4(),
            detected_at: std::time::Instant::now(),
        };
        // Send the second question immediately before the debounce timer fires.
        tx.send(second).await.unwrap();
        drop(tx); // close channel so debounce loop exits

        let result = debounce(&mut rx, first, Duration::from_millis(50)).await;
        assert_eq!(result.text, "second");
    }
}
