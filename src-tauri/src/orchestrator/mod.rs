//! Orchestration layer: response threads (answer/visual, replacing the
//! retired directional/depth/clarifying threads as of `lpav` slice 28),
//! pre-warm cache, and session lifecycle management.
//!
//! Reference: design doc §8 (System Architecture), `.cursor/rules` flint-core
//! §4 (parallel threads via tokio::spawn, never sequential).
//!
//! ## Concurrency contract
//!
//! The answer and visual response threads are spawned via `tokio::spawn` in
//! a single statement — there is NO `.await` between spawns. One thread
//! failing never affects the other.
//!
//! ## Silence debounce
//!
//! After a `DetectedQuestion` arrives the orchestrator waits 1500ms (task 4.10).
//! If a new question arrives within that window the timer resets and the older
//! question is discarded. This prevents double-firing on split utterances.

pub mod answer;
pub mod prewarm;
pub mod visual;
pub mod visual_classifier;

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
    emit_turn_started, ConfidenceScorePayload, ContextTruncatedPayload, CostCapStatusPayload,
    InferenceSuspendedPayload, RagChunkPayload, RagChunksUpdatePayload, ResponseMetadataPayload,
    ThreadStatusPayload, TokenUsageUpdatePayload, TurnStartedPayload,
};
use crate::interfaces::vector::{
    PromptChunks, ScoredChunk, VectorInterface, QA_EMBED_CONFIDENCE_THRESHOLD,
};
use crate::llm::failover::FailoverManager;
use crate::llm::provider::LLMProvider;
use crate::orchestrator::prewarm::{is_plausible_cached_response, PreWarmCache};
use crate::rag::embedder::Embedder;
use crate::rag::retriever::retrieve_for_prompt;
use crate::session::memory::{ContextBudget, ConversationMemory, MemoryContext, Turn};
use crate::session::persistence::{Response, ResponseType, SessionPersistence};
use crate::state::TurnCancelFlag;

// ──────────────────────────────────────────────────────────────────────────────
// Silence debounce
// ──────────────────────────────────────────────────────────────────────────────

/// Wait this long after the last detected question before firing threads.
const SILENCE_DEBOUNCE: Duration = Duration::from_millis(1500);

// ──────────────────────────────────────────────────────────────────────────────
// Shared context passed to each thread
// ──────────────────────────────────────────────────────────────────────────────

/// Everything a response thread needs to build its prompt and produce a response.
#[derive(Clone)]
pub struct OrchestrationContext {
    pub session_id: Uuid,
    pub question: String,
    /// Context store chunks (JD, resume, notes). Always filled; 1–6 items.
    pub rag_chunks: Vec<ScoredChunk>,
    /// Q&A store chunks (past answers, score ≥ 0.80). May be empty.
    /// When present, prompt templates inject a labelled section.
    pub qa_chunks: Vec<ScoredChunk>,
    pub digest: Arc<Digest>,
    pub memory_ctx: MemoryContext,
    /// True if this response was served from the pre-warm cache or a saved preferred answer.
    pub from_cache: bool,
    /// True when the turn used a user-saved preferred answer (not pre-warm).
    pub from_preferred: bool,
    /// Saved preferred answer text when present (used for prompt hints during generation).
    pub preferred_answer: String,
    /// Cached answer text when pre-warm cache hit (≥ 0.85 cosine).
    pub cached_answer: Option<String>,
    /// Cached visual text when pre-warm cache hit.
    pub cached_visual: Option<String>,
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

/// Gate for the fire-and-forget Q&A embedding (step 7b). Only the Answer
/// thread's output feeds this decision — Visual output is never embedded
/// for Q&A recall, since it is diagram/code shaped rather than a reusable
/// spoken answer.
fn should_embed_qa_pair(confidence_score: f32, answer_text: &str) -> bool {
    confidence_score >= QA_EMBED_CONFIDENCE_THRESHOLD && !answer_text.trim().is_empty()
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
    confidence: f32,
) {
    if text.is_empty() {
        return;
    }
    let r = Response {
        id: Uuid::new_v4(),
        session_id,
        response_type,
        content: text.to_string(),
        confidence,
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
) -> (String, Option<String>) {
    match result {
        Ok(Ok(text)) => (text, None),
        Ok(Err(e)) => {
            let detail = format!("{e:#}");
            log_thread_failed(session_id, thread, &e);
            emit_thread_error(app, thread);
            (String::new(), Some(detail))
        }
        Err(join_err) => {
            let detail = format!("task panicked: {join_err}");
            log_thread_panicked(session_id, thread, &join_err);
            emit_thread_error(app, thread);
            (String::new(), Some(detail))
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
        event = "answer_thread_complete",
        thread_type = "answer",
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
        // M13 S5 — defensive check: only the System loopback channel, the
        // phone-mode manual confirmation, and the React-side trigger button
        // may produce a question for the orchestrator. Mic-tagged questions
        // mean a regression in the audio pipeline routing; drop the turn and
        // log loudly rather than silently dispatching responses to the user's
        // own utterance.
        if !is_valid_question_source(first.source) {
            tracing::error!(
                session_id = %config.session_id,
                source = ?first.source,
                "orchestrator received question with invalid source — turn dropped"
            );
            continue;
        }

        // ── Silence debounce ─────────────────────────────────────────────────
        // Drain additional questions that arrive within the debounce window,
        // keeping only the last one (most complete utterance).
        let question = debounce(&mut question_rx, first, SILENCE_DEBOUNCE).await;

        turn_number += 1;
        let turn = turn_number;

        // Question text is session content and must not appear at INFO or
        // above in release builds (flint-security.mdc §"Hard Constraints").
        // We log a length proxy for operability while gating the literal
        // text behind a debug-only sibling event.
        info!(
            session_id = %config.session_id,
            turn = turn,
            question_len = question.text.len(),
            "orchestrator dispatching turn"
        );
        #[cfg(debug_assertions)]
        tracing::debug!(
            session_id = %config.session_id,
            turn = turn,
            question = %question.text,
            "orchestrator dispatching turn (debug-only content)",
        );

        let turn_cancel = Arc::new(AtomicBool::new(false));
        {
            let mut slot = config.turn_cancel_slot.lock().await;
            if let Some(prev) = slot.take() {
                prev.store(true, Ordering::Release);
            }
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
            usage_category: "live_turn".to_string(),
            force_visual: matches!(
                question.source,
                crate::audio::pipeline::DetectedQuestionSource::VisualManual
            ),
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

/// True for every [`DetectedQuestionSource`] variant the orchestrator is
/// allowed to consume. Microphone is reserved as a sentinel that should never
/// reach the orchestrator (M13 S5).
fn is_valid_question_source(source: crate::audio::pipeline::DetectedQuestionSource) -> bool {
    use crate::audio::pipeline::DetectedQuestionSource;
    matches!(
        source,
        DetectedQuestionSource::System
            | DetectedQuestionSource::PhoneManual
            | DetectedQuestionSource::UserTriggered
            | DetectedQuestionSource::VisualManual
    )
}

/// Drain the channel for `window` duration, returning the last question seen.
/// If the speaker adds more words within the window the stale partial question
/// is discarded in favour of the merged follow-up.
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
    /// Phase 5.5.7 — activity category for the usage widget.
    usage_category: String,
    /// Bypasses `visual_classifier` when true — set for Live
    /// `DetectedQuestionSource::VisualManual` and for rehearsal turns
    /// that pass `force_visual` through [`dispatch_turn`].
    force_visual: bool,
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
        anyhow::bail!(
            "Inference blocked — usage limit reached ({} tokens, ${:.4}). \
             Open Settings → Usage limits → Reset counters or raise the cap.",
            snap.usage.total_tokens,
            snap.usage.cost_estimate_usd
        );
    }

    // Turn boundary for the frontend: previous answer cards are archived and
    // a fresh card headed by this question begins streaming. Emitted after
    // the cap check so a refused turn does not wipe the previous answer.
    emit_turn_started(
        &app,
        TurnStartedPayload {
            question: cfg.question_text.clone(),
            turn: cfg.turn_number,
        },
    );

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

    // ── 2. Preferred answer lookup (exact key, then cosine ≥ 0.85) ────────
    let preferred_answer = cfg
        .persistence
        .resolve_preferred_answer(cfg.session_id, &cfg.question_text, Some(&embedding))
        .unwrap_or_default();
    let from_preferred = !preferred_answer.trim().is_empty();

    // Clone strings out of the cache entry before releasing the lock so the
    // MutexGuard is dropped before the first `.await` in `retrieve_rag`.
    let cache_hit = if from_preferred {
        Some((preferred_answer.clone(), preferred_answer.clone()))
    } else {
        let cache = cfg.prewarm_cache.lock().await;
        cache.lookup(&embedding).and_then(|e| {
            let ans = if is_plausible_cached_response(&e.answer_response) {
                e.answer_response.clone()
            } else {
                String::new()
            };
            let vis = if is_plausible_cached_response(&e.visual_response) {
                e.visual_response.clone()
            } else {
                String::new()
            };
            if ans.is_empty() && vis.is_empty() {
                None
            } else {
                Some((ans, vis))
            }
        })
    };

    let from_cache = cache_hit.is_some();
    let (cached_answer, cached_visual) = match cache_hit {
        Some((ans, vis)) => (Some(ans), Some(vis)),
        None => (None, None),
    };
    if from_preferred {
        info!(
            session_id = %cfg.session_id,
            turn = cfg.turn_number,
            event = "preferred_answer_hit",
            "serving user preferred answer"
        );
    } else if from_cache {
        info!(
            session_id = %cfg.session_id,
            turn = cfg.turn_number,
            event = "prewarm_cache_hit",
            "pre-warm cache hit"
        );
    }

    let prompt_chunks = retrieve_rag(&cfg, &embedding).await?;
    let rag_latency_ms = rag_start.elapsed().as_millis() as u64;

    emit_rag_chunks_update(
        &app,
        RagChunksUpdatePayload {
            chunks: prompt_chunks
                .context
                .iter()
                .chain(prompt_chunks.qa.iter())
                .take(10)
                .map(|c| RagChunkPayload {
                    text: c.chunk.text.clone(),
                    score: c.score,
                })
                .collect(),
        },
    );
    let rag_chunks = prompt_chunks.context.clone();
    let qa_chunks = prompt_chunks.qa.clone();

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
        qa_chunks: qa_chunks.clone(),
        digest: Arc::clone(&cfg.digest),
        memory_ctx,
        from_cache,
        from_preferred,
        preferred_answer: preferred_answer.clone(),
        cached_answer,
        cached_visual,
        turn_cancel: Arc::clone(&cfg.turn_cancel),
        turn_number: cfg.turn_number,
    };

    // ── 5. Spawn Answer + Visual concurrently ─────────────────────────────
    // RULE: no .await between spawns — both are dispatched simultaneously
    // when Visual fires. The retired Clarifying thread is no longer spawned
    // here (its job is absorbed into the Answer prompt, slice 18); the
    // module itself was deleted end-to-end in slice 28.
    let dir_ctx = ctx.clone();
    let dep_ctx = ctx.clone();

    let dir_app = app.clone();
    let dep_app = app.clone();

    let dir_failover = Arc::clone(&cfg.failover);
    let dep_failover = Arc::clone(&cfg.failover);

    let dir_prompts = cfg.prompts_dir.clone();
    let dep_prompts = cfg.prompts_dir.clone();

    let dir_task = tokio::spawn(async move {
        answer::run_answer(dir_ctx, dir_failover, &dir_prompts, dir_app).await
    });

    // Visual only fires when the cheap classifier judges the question likely
    // to benefit from a diagram, or the user forced it via
    // `trigger_visual_response` (DetectedQuestionSource::VisualManual).
    let needs_visual = cfg.force_visual || visual_classifier::needs_visual(&cfg.question_text);
    let dep_task = needs_visual.then(|| {
        tokio::spawn(async move {
            visual::run_visual(dep_ctx, dep_failover, &dep_prompts, dep_app).await
        })
    });

    // Collect results — one thread failing never crashes the other.
    let (dir_result, dep_result) = match dep_task {
        Some(dep_task) => {
            let (d, v) = tokio::join!(dir_task, dep_task);
            (d, Some(v))
        }
        None => (dir_task.await, None),
    };

    let (answer_text, dir_err) = collect_thread_text(dir_result, cfg.session_id, "answer", &app);
    let (visual_text, dep_err) = match dep_result {
        Some(result) => collect_thread_text(result, cfg.session_id, "visual", &app),
        None => {
            emit_thread_status(
                &app,
                ThreadStatusPayload {
                    thread: "visual".to_string(),
                    status: "idle".to_string(),
                },
            );
            (String::new(), None)
        }
    };

    if answer_text.trim().is_empty() {
        let detail = dir_err
            .or(dep_err)
            .unwrap_or_else(|| "unknown inference failure".to_string());
        anyhow::bail!(
            "No answer was generated. Groq may be rate-limited (free tier: ~3 \
             parallel calls per question). Add an OpenRouter key in Settings → API Keys for cloud \
             fallback, or run `ollama serve` and `ollama pull llama3.1:8b`. Detail: {detail}"
        );
    }

    // ── 6. Confidence scoring ─────────────────────────────────────────────
    // Confidence is computed once and reused by both the UI event (step 6)
    // and the Q&A embedding gate (step 7b). Previously short-circuited to a
    // fixed Grey level when the (now-retired) Clarifying thread produced a
    // question; the Answer thread's prompt now handles ambiguity inline
    // (slice 18), so confidence is always computed from the answer text.
    let (confidence_score, confidence_level) = {
        let rag_texts: Vec<String> = rag_chunks
            .iter()
            .take(3)
            .map(|c| c.chunk.text.clone())
            .collect();

        let signals = ConfidenceSignals {
            rag_grounding,
            response_text: answer_text.clone(),
            rag_texts,
            provider_name: cfg.failover.active_provider_name().to_string(),
            cache_stale: from_cache && cfg.turn_number > 3,
            local_fallback_active: cfg.failover.is_using_local(),
            turn_number: cfg.turn_number,
        };
        let (score, level) = compute_confidence(&signals);

        log_confidence_computed(
            cfg.session_id,
            cfg.turn_number,
            score,
            level,
            cfg.failover.active_provider_name(),
            from_cache,
            rag_latency_ms,
            cfg.failover.is_using_local(),
        );

        emit_confidence_score(
            &app,
            ConfidenceScorePayload {
                level: level.as_str().to_string(),
            },
        );
        (score, level)
    };

    // ── 6b. Rehearsal question attempt tracking ───────────────────────────
    if cfg.usage_category == "rehearsal_turn" {
        let canonical =
            crate::session::question_attempts::strip_rephrase_prefix(&cfg.question_text);
        let satisfied = crate::session::question_attempts::rehearsal_attempt_satisfied(
            confidence_score,
            confidence_level,
        );
        if let Err(e) = cfg.persistence.upsert_question_attempt(
            cfg.session_id,
            canonical,
            "rehearsal",
            confidence_score,
            0,
            satisfied,
        ) {
            warn!(
                session_id = %cfg.session_id,
                error = %e,
                "failed to record rehearsal question attempt"
            );
        }
    }

    // ── 7. Persist responses — crash-recovery insurance ───────────────────
    // Confidence is a property of the turn (driven by the directional answer's
    // grounding); both threads are persisted with the same turn confidence so
    // the post-session summary can build a real distribution.
    persist_thread_response(
        &cfg.persistence,
        cfg.session_id,
        ResponseType::Answer,
        &answer_text,
        confidence_score,
    );
    persist_thread_response(
        &cfg.persistence,
        cfg.session_id,
        ResponseType::Visual,
        &visual_text,
        confidence_score,
    );

    // ── 7b. Quality-gated Q&A embedding ──────────────────────────────────
    // If the Answer thread's output reached confidence ≥ QA_EMBED_CONFIDENCE_THRESHOLD
    // (green or blue), embed the Q&A pair into the session's Q&A vector store
    // so it can be retrieved as a supplemental slot in future turns.
    //
    // This is intentionally fire-and-forget: a failure here must never block
    // the turn. Low-confidence answers (amber/grey/red) are skipped to
    // prevent contaminating future retrievals.
    {
        if should_embed_qa_pair(confidence_score, &answer_text) {
            let qa_text = format!("Q: {}\nA: {}", cfg.question_text.trim(), answer_text.trim());
            let embedder = Arc::clone(&cfg.embedder);
            let store = Arc::clone(&cfg.vector_store);
            let session_id = cfg.session_id;
            // Spawn detached — failure logged but never propagated.
            tokio::spawn(async move {
                let result: anyhow::Result<()> = async {
                    let text_clone = qa_text.clone();
                    let embedding = tokio::task::spawn_blocking(move || {
                        embedder
                            .embed_one(&text_clone)
                            .context("Q&A pair embedding failed")
                    })
                    .await
                    .context("embed task panicked")?
                    .context("embed failed")?;

                    let chunk = crate::interfaces::vector::Chunk {
                        id: uuid::Uuid::new_v4(),
                        text: qa_text,
                        embedding,
                        session_id,
                    };
                    store
                        .ingest_qa(session_id, vec![chunk])
                        .await
                        .context("ingest_qa failed")?;
                    Ok(())
                }
                .await;

                if let Err(e) = result {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %e,
                        "Q&A pair embedding skipped (non-fatal)"
                    );
                }
            });
        }
    }

    // ── 8. Update conversation memory ─────────────────────────────────────
    {
        let mut mem = cfg.memory.lock().await;
        mem.push_turn(Turn::new(
            cfg.question_text.clone(),
            answer_text.clone(),
            visual_text.clone(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        ));
    }

    // ── 8. Token usage estimate (4 chars ≈ 1 token) ───────────────────────
    // The +500 fudge accounts for the system prompt + RAG chunks injected
    // into every call. It overestimates slightly, which is the safer side
    // to err on for a hard cost cap.
    let turn_input = (cfg.question_text.len() as u64 + 500) / 4;
    let turn_output = (answer_text.len() as u64 + visual_text.len() as u64 + 500) / 4;
    let total = turn_input + turn_output;
    let cost_estimate = total as f64 * 0.0000002;
    emit_token_usage_update(
        &app,
        TokenUsageUpdatePayload {
            input: turn_input,
            output: turn_output,
            total,
            cost_estimate,
            usage_category: cfg.usage_category.clone(),
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
///
/// When `force_visual` is true, the Visual thread spawns regardless of
/// `visual_classifier::needs_visual` — used by Rehearsal's manual
/// "Generate diagram" path.
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
    force_visual: bool,
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
            usage_category: "rehearsal_turn".to_string(),
            force_visual,
        },
        app,
    )
    .await
}

async fn retrieve_rag(cfg: &OrchestratorTurnConfig, embedding: &[f32]) -> Result<PromptChunks> {
    let chunks = retrieve_for_prompt(cfg.vector_store.as_ref(), cfg.session_id, embedding)
        .await
        .context("slot-allocated RAG retrieval failed")?;
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
        let category = dir.path().join("answer");
        std::fs::create_dir_all(&category).unwrap();
        std::fs::write(category.join("default.txt"), "default template").unwrap();

        let result = load_prompt("answer", "nonexistent_provider", dir.path());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "default template");
    }

    #[test]
    fn load_prompt_uses_provider_specific_file() {
        let dir = tempfile::tempdir().unwrap();
        let category = dir.path().join("answer");
        std::fs::create_dir_all(&category).unwrap();
        std::fs::write(category.join("default.txt"), "default").unwrap();
        std::fs::write(category.join("groq.txt"), "groq variant").unwrap();

        let result = load_prompt("answer", "groq", dir.path());
        assert_eq!(result.unwrap(), "groq variant");
    }

    #[test]
    fn load_prompt_errors_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let category = dir.path().join("answer");
        std::fs::create_dir_all(&category).unwrap();

        assert!(load_prompt("answer", "any", dir.path()).is_err());
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

    #[test]
    fn should_embed_qa_pair_fires_on_high_confidence_answer_text() {
        assert!(should_embed_qa_pair(
            QA_EMBED_CONFIDENCE_THRESHOLD,
            "A grounded answer."
        ));
        assert!(should_embed_qa_pair(0.9, "A grounded answer."));
    }

    #[test]
    fn should_embed_qa_pair_skips_below_threshold() {
        let just_under = QA_EMBED_CONFIDENCE_THRESHOLD - 0.01;
        assert!(!should_embed_qa_pair(just_under, "A grounded answer."));
    }

    #[test]
    fn should_embed_qa_pair_skips_empty_answer_text() {
        assert!(!should_embed_qa_pair(0.95, ""));
        assert!(!should_embed_qa_pair(0.95, "   "));
    }

    /// Slice 17 (`lpav-s17-prompts-answer-visual`) — the Answer + Visual
    /// prompt artifacts must exist on disk before either thread can load
    /// them; the gpt/claude/llama variants are the task's minimum bar.
    #[test]
    fn answer_and_visual_prompts_exist_on_disk() {
        let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts");
        for category in ["answer", "visual"] {
            for provider in ["default", "gpt", "claude", "llama"] {
                let path = prompts_dir.join(category).join(format!("{provider}.txt"));
                assert!(path.exists(), "missing prompt file: {}", path.display());
            }
        }
    }

    #[test]
    fn load_prompt_answer_loads_from_real_prompts_dir() {
        let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts");
        let template =
            load_prompt("answer", "groq", &prompts_dir).expect("answer prompt must load");
        assert!(template.contains("{question}"));
        assert!(template.contains("Follow-up"));
    }

    #[test]
    fn load_prompt_visual_loads_from_real_prompts_dir() {
        let prompts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts");
        let template =
            load_prompt("visual", "groq", &prompts_dir).expect("visual prompt must load");
        assert!(template.contains("{question}"));
        assert!(template.to_lowercase().contains("fenced"));
    }

    #[tokio::test]
    async fn debounce_returns_latest_question() {
        use crate::audio::pipeline::DetectedQuestionSource;
        let (tx, mut rx) = mpsc::channel(8);
        let first = DetectedQuestion {
            text: "first".to_string(),
            session_id: Uuid::new_v4(),
            detected_at: std::time::Instant::now(),
            source: DetectedQuestionSource::System,
        };
        let second = DetectedQuestion {
            text: "second".to_string(),
            session_id: Uuid::new_v4(),
            detected_at: std::time::Instant::now(),
            source: DetectedQuestionSource::System,
        };
        // Send the second question immediately before the debounce timer fires.
        tx.send(second).await.unwrap();
        drop(tx); // close channel so debounce loop exits

        let result = debounce(&mut rx, first, Duration::from_millis(50)).await;
        assert_eq!(result.text, "second");
    }

    #[test]
    fn orchestrator_accepts_system_phone_and_user_sources() {
        use crate::audio::pipeline::DetectedQuestionSource;
        assert!(is_valid_question_source(DetectedQuestionSource::System));
        assert!(is_valid_question_source(
            DetectedQuestionSource::PhoneManual
        ));
        assert!(is_valid_question_source(
            DetectedQuestionSource::UserTriggered
        ));
        assert!(is_valid_question_source(
            DetectedQuestionSource::VisualManual
        ));
    }

    #[test]
    fn orchestrator_rejects_microphone_source() {
        use crate::audio::pipeline::DetectedQuestionSource;
        assert!(!is_valid_question_source(
            DetectedQuestionSource::Microphone
        ));
    }
}
