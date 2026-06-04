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
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tauri::AppHandle;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};
use uuid::Uuid;

use crate::audio::pipeline::DetectedQuestion;
use crate::confidence::{compute_confidence, ConfidenceSignals};
use crate::digest::Digest;
use crate::events::{
    emit_confidence_score, emit_thread_status, ConfidenceScorePayload, ThreadStatusPayload,
};
use crate::llm::failover::FailoverManager;
use crate::rag::embedder::Embedder;
use crate::rag::retriever::retrieve;
use crate::interfaces::vector::{ScoredChunk, VectorInterface};
use crate::session::memory::{ContextBudget, ConversationMemory, MemoryContext, Turn};
use crate::orchestrator::prewarm::PreWarmCache;

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
}

/// Receive detected questions and run the three parallel response threads.
///
/// Designed to be spawned as a `tokio::task` for the duration of the live
/// session. Exits when `question_rx` is closed (i.e. `stop_session` drops the
/// sender).
pub async fn run_orchestrator(
    mut question_rx: mpsc::Receiver<DetectedQuestion>,
    config: OrchestratorConfig,
    app: AppHandle,
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
        };

        tokio::spawn(async move {
            if let Err(e) = run_turn(cfg, app_clone).await {
                warn!(error = %e, "orchestrator turn failed");
            }
        });
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
}

async fn run_turn(cfg: OrchestratorTurnConfig, app: AppHandle) -> Result<()> {
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
        cache.lookup(&embedding).map(|e| (e.directional_response.clone(), e.depth_response.clone()))
    };

    let from_cache = cache_hit.is_some();
    if from_cache {
        info!(session_id = %cfg.session_id, turn = cfg.turn_number, "pre-warm cache hit");
    }

    let rag_chunks = retrieve_rag(&cfg, &embedding).await?;

    // ── 3. Build memory context ───────────────────────────────────────────
    let memory_ctx = {
        let mem = cfg.memory.lock().await;
        let budget = ContextBudget::from_window(
            if cfg.failover.is_using_local() {
                4_096
            } else {
                128_000
            },
        );
        mem.build_context(&budget, None, &cfg.compression_prompt, cfg.session_id)
            .await?
    };

    let rag_grounding = mean_rag_score(&rag_chunks);

    // ── 4. Build shared context ───────────────────────────────────────────
    let ctx = OrchestrationContext {
        session_id: cfg.session_id,
        question: cfg.question_text.clone(),
        rag_chunks,
        digest: Arc::clone(&cfg.digest),
        memory_ctx,
        from_cache,
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

    let directional_text = dir_result
        .unwrap_or_else(|e| {
            warn!(session_id = %cfg.session_id, "directional task panicked: {e}");
            emit_thread_status(
                &app,
                ThreadStatusPayload {
                    thread: "directional".to_string(),
                    status: "error".to_string(),
                },
            );
            Err(anyhow::anyhow!("directional panic"))
        })
        .unwrap_or_default();

    let depth_text = dep_result
        .unwrap_or_else(|e| {
            warn!(session_id = %cfg.session_id, "depth task panicked: {e}");
            emit_thread_status(
                &app,
                ThreadStatusPayload {
                    thread: "depth".to_string(),
                    status: "error".to_string(),
                },
            );
            Err(anyhow::anyhow!("depth panic"))
        })
        .unwrap_or_default();

    // ── 6. Confidence scoring ─────────────────────────────────────────────
    let signals = ConfidenceSignals {
        rag_grounding,
        response_text: directional_text.clone(),
        provider_name: cfg.failover.active_provider_name().to_string(),
        cache_stale: from_cache && cfg.failover.is_using_local(),
        local_fallback_active: cfg.failover.is_using_local(),
        turn_number: cfg.turn_number,
    };
    let (confidence_score, confidence_level) = compute_confidence(&signals);

    info!(
        session_id = %cfg.session_id,
        turn = cfg.turn_number,
        confidence = confidence_score,
        level = %confidence_level.as_str(),
        provider = %cfg.failover.active_provider_name(),
        "confidence computed"
    );

    emit_confidence_score(
        &app,
        ConfidenceScorePayload {
            level: confidence_level.as_str().to_string(),
        },
    );

    // ── 7. Update conversation memory ─────────────────────────────────────
    {
        let mut mem = cfg.memory.lock().await;
        mem.push_turn(Turn {
            question: cfg.question_text.clone(),
            directional_response: directional_text,
            depth_response: depth_text,
        });
    }

    Ok(())
}

async fn retrieve_rag(
    cfg: &OrchestratorTurnConfig,
    embedding: &[f32],
) -> Result<Vec<ScoredChunk>> {
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

        let chunks = vec![make_chunk(0.9), make_chunk(0.8), make_chunk(0.7), make_chunk(0.6)];
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
