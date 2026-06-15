//! Depth response thread (design doc §8, task 4.8).
//!
//! Fully streamed in < 8s P95. Prompt loaded from
//! `/prompts/depth/{provider}.txt` or `default.txt`.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Error, Result};
use futures::StreamExt;
use tauri::{AppHandle, Runtime};
use tokio::time::timeout;
use tracing::{info, warn};
use uuid::Uuid;

use crate::events::{emit_depth_token, emit_thread_status, DepthTokenPayload, ThreadStatusPayload};
use crate::llm::failover::FailoverManager;
use crate::llm::provider::CompletionConfig;

use super::{load_prompt, OrchestrationContext};

/// Execute the depth response thread.
///
/// Streams tokens to the React layer via `depth_token` events.
/// Returns the full assembled response text.
pub async fn run_depth<R: Runtime>(
    ctx: OrchestrationContext,
    failover: Arc<FailoverManager>,
    prompts_dir: &Path,
    app: AppHandle<R>,
) -> Result<String> {
    let start = Instant::now();
    let provider_name = failover.active_provider_name().to_string();

    // Pre-warm cache hit — serve cached depth; on turn ≥ 3 also run fresh LLM.
    if let Some(cached) = ctx.cached_depth.clone() {
        let mut full_response = emit_cached_depth_tokens(&cached, &app, &ctx.turn_cancel);

        if ctx.turn_number >= 3 {
            log_refresh_on_turn_three(ctx.session_id, ctx.turn_number);
            match run_fresh_depth(&ctx, Arc::clone(&failover), prompts_dir, &app).await {
                Ok(fresh) if !fresh.is_empty() => full_response = fresh,
                Ok(_) => {}
                Err(e) => {
                    log_fresh_refresh_failed(ctx.session_id, &e);
                }
            }
        }

        let stream_ms = start.elapsed().as_millis() as u64;
        log_depth_complete(ctx.session_id, stream_ms, &provider_name, true);
        emit_thread_status(
            &app,
            ThreadStatusPayload {
                thread: "depth".to_string(),
                status: "ok".to_string(),
            },
        );
        return Ok(full_response);
    }

    run_fresh_depth(&ctx, failover, prompts_dir, &app).await
}

async fn run_fresh_depth<R: Runtime>(
    ctx: &OrchestrationContext,
    failover: Arc<FailoverManager>,
    prompts_dir: &Path,
    app: &AppHandle<R>,
) -> Result<String> {
    let start = Instant::now();
    let provider_name = failover.active_provider_name().to_string();

    let prompt = build_prompt(ctx, failover.active_provider_name(), prompts_dir)?;

    let config = CompletionConfig {
        max_tokens: Some(400),
        temperature: 0.0,
        stream: true,
    };

    let estimated_tokens = 500_u32;
    let mut stream = failover
        .complete_stream(prompt, config, app, estimated_tokens)
        .await
        .context("depth stream failed")?;

    let mut full_response = String::new();
    let stream_deadline = Instant::now() + Duration::from_secs(60);

    while Instant::now() < stream_deadline {
        if ctx.turn_cancel.load(Ordering::Acquire) {
            break;
        }
        match timeout(Duration::from_secs(15), stream.next()).await {
            Ok(Some(Ok(token))) => {
                full_response.push_str(&token);
                emit_depth_token(app, DepthTokenPayload { token });
            }
            Ok(Some(Err(e))) => return Err(e).context("depth token error"),
            Ok(None) => break,
            Err(_) => {
                warn!(
                    session_id = %ctx.session_id,
                    "depth stream stalled — returning partial response"
                );
                break;
            }
        }
    }

    let stream_ms = start.elapsed().as_millis() as u64;
    if stream_ms > 8_000 {
        log_depth_nfr_breach(ctx.session_id, stream_ms);
    }

    log_depth_complete(ctx.session_id, stream_ms, &provider_name, ctx.from_cache);

    emit_thread_status(
        app,
        ThreadStatusPayload {
            thread: "depth".to_string(),
            status: "ok".to_string(),
        },
    );

    Ok(full_response)
}

/// Helpers extracted so tarpaulin attributes coverage to the call site —
/// inline tracing macro arguments are reported as uncovered even when hit.
fn log_refresh_on_turn_three(session_id: Uuid, turn: usize) {
    info!(
        session_id = %session_id,
        turn = turn,
        "cache hit turn ≥ 3 — running fresh depth in parallel"
    );
}

fn log_fresh_refresh_failed(session_id: Uuid, error: &Error) {
    warn!(
        session_id = %session_id,
        error = %error,
        "fresh depth after cache hit failed — keeping cached response"
    );
}

fn log_depth_nfr_breach(session_id: Uuid, stream_ms: u64) {
    warn!(
        session_id = %session_id,
        stream_ms,
        "depth stream > 8s — NFR breach"
    );
}

fn log_depth_complete(session_id: Uuid, stream_ms: u64, provider: &str, cache_hit: bool) {
    info!(
        session_id = %session_id,
        event = "depth_thread_complete",
        thread_type = "depth",
        stream_complete_ms = stream_ms,
        provider = %provider,
        model = %provider,
        cache_hit = cache_hit,
        "depth thread finished"
    );
}

fn emit_cached_depth_tokens<R: Runtime>(
    text: &str,
    app: &AppHandle<R>,
    cancel: &Arc<std::sync::atomic::AtomicBool>,
) -> String {
    for word in text.split_inclusive(' ') {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        emit_depth_token(
            app,
            DepthTokenPayload {
                token: word.to_string(),
            },
        );
    }
    text.to_string()
}

fn build_prompt(
    ctx: &OrchestrationContext,
    provider_name: &str,
    prompts_dir: &Path,
) -> Result<String> {
    let template =
        load_prompt("depth", provider_name, prompts_dir).context("failed to load depth prompt")?;

    let rag_text = ctx
        .rag_chunks
        .iter()
        .enumerate()
        .map(|(i, c)| format!("[{}] {}", i + 1, c.chunk.text))
        .collect::<Vec<_>>()
        .join("\n\n");

    let qa_section = if ctx.qa_chunks.is_empty() {
        String::new()
    } else {
        let qa_text = ctx
            .qa_chunks
            .iter()
            .map(|c| c.chunk.text.clone())
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");
        format!(
            "\n\n[How you answered a similar question earlier — use as a reference, not verbatim]\n{qa_text}"
        )
    };

    let key_skills = ctx.digest.key_skills.join(", ");

    Ok(template
        .replace("{session_domain}", &ctx.digest.domain)
        .replace("{rag_chunks}", &rag_text)
        .replace("{qa_chunks}", &qa_section)
        .replace(
            "{rolling_summary_if_compressed}",
            &ctx.memory_ctx.rolling_summary,
        )
        .replace("{last_n_turns}", &ctx.memory_ctx.recent_turns)
        .replace("{question}", &ctx.question)
        .replace("{interviewer_role}", &ctx.digest.role)
        .replace("{interviewer_priorities}", &key_skills))
}
