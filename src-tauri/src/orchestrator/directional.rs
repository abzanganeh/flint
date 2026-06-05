//! Directional response thread (design doc §8, task 4.7).
//!
//! Fires on every `System`-source question. Target TTFT < 800ms (P95 < 900ms).
//! Prompt loaded from `/prompts/directional/{provider}.txt` or `default.txt`.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use futures::StreamExt;
use tauri::{AppHandle, Runtime};
use tracing::{info, warn};
use uuid::Uuid;

use crate::events::{
    emit_directional_token, emit_thread_status, DirectionalTokenPayload, ThreadStatusPayload,
};
use crate::llm::failover::FailoverManager;
use crate::llm::provider::CompletionConfig;

use super::{load_prompt, OrchestrationContext};

/// Execute the directional response thread.
///
/// Streams tokens to the React layer via `directional_token` events.
/// Returns the full assembled response text (for confidence scoring and
/// memory recording).
pub async fn run_directional<R: Runtime>(
    ctx: OrchestrationContext,
    failover: Arc<FailoverManager>,
    prompts_dir: &Path,
    app: AppHandle<R>,
) -> Result<String> {
    let ttft_start = Instant::now();
    let provider_name = failover.active_provider_name().to_string();

    // Pre-warm cache hit — serve cached response without an LLM round-trip.
    if let Some(cached) = ctx.cached_directional {
        let text = emit_cached_directional_tokens(&cached, &app, &ctx.turn_cancel);
        let ttft_ms = ttft_start.elapsed().as_millis() as u64;
        log_cache_served(ctx.session_id, ttft_ms, &provider_name);
        emit_thread_status(
            &app,
            ThreadStatusPayload {
                thread: "directional".to_string(),
                status: "ok".to_string(),
            },
        );
        return Ok(text);
    }

    let prompt = build_prompt(&ctx, failover.active_provider_name(), prompts_dir)?;

    let config = CompletionConfig {
        max_tokens: Some(200),
        temperature: 0.0,
        stream: true,
    };

    let estimated_tokens = 300_u32;
    let mut stream = failover
        .complete_stream(prompt, config, &app, estimated_tokens)
        .await
        .context("directional stream failed")?;

    let mut full_response = String::new();
    let mut first_token = true;

    while let Some(token_result) = stream.next().await {
        if ctx.turn_cancel.load(Ordering::Acquire) {
            break;
        }
        let token = token_result.context("directional token error")?;

        if first_token {
            let ttft_ms = ttft_start.elapsed().as_millis() as u64;
            log_first_token(ctx.session_id, ttft_ms, &provider_name);
            if ttft_ms > 900 {
                log_ttft_breach(ctx.session_id, ttft_ms);
            }
            first_token = false;
        }

        full_response.push_str(&token);
        emit_directional_token(&app, DirectionalTokenPayload { token });
    }

    let stream_ms = ttft_start.elapsed().as_millis() as u64;
    log_complete(ctx.session_id, stream_ms, &provider_name, ctx.from_cache);

    emit_thread_status(
        &app,
        ThreadStatusPayload {
            thread: "directional".to_string(),
            status: "ok".to_string(),
        },
    );

    Ok(full_response)
}

/// Emit cached text word-by-word as `directional_token` events for incremental UI rendering.
fn emit_cached_directional_tokens<R: Runtime>(
    text: &str,
    app: &AppHandle<R>,
    cancel: &Arc<std::sync::atomic::AtomicBool>,
) -> String {
    for word in text.split_inclusive(' ') {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        emit_directional_token(
            app,
            DirectionalTokenPayload {
                token: word.to_string(),
            },
        );
    }
    text.to_string()
}

/// Helpers extracted so tarpaulin attributes coverage to the call site —
/// inline tracing macro arguments are reported as uncovered even when hit.
fn log_cache_served(session_id: Uuid, ttft_ms: u64, provider: &str) {
    info!(
        session_id = %session_id,
        event = "directional_thread_complete",
        thread_type = "directional",
        ttft_ms,
        stream_complete_ms = ttft_ms,
        provider = %provider,
        cache_hit = true,
        model = %provider,
        "directional served from pre-warm cache"
    );
}

fn log_first_token(session_id: Uuid, ttft_ms: u64, provider: &str) {
    info!(
        session_id = %session_id,
        event = "directional_ttft",
        thread_type = "directional",
        ttft_ms,
        provider = %provider,
        model = %provider,
        "directional first token"
    );
}

fn log_ttft_breach(session_id: Uuid, ttft_ms: u64) {
    warn!(
        session_id = %session_id,
        ttft_ms,
        "directional TTFT > 900ms — NFR breach"
    );
}

fn log_complete(session_id: Uuid, stream_ms: u64, provider: &str, cache_hit: bool) {
    info!(
        session_id = %session_id,
        event = "directional_thread_complete",
        thread_type = "directional",
        stream_complete_ms = stream_ms,
        provider = %provider,
        model = %provider,
        cache_hit = cache_hit,
        "directional thread finished"
    );
}

fn build_prompt(
    ctx: &OrchestrationContext,
    provider_name: &str,
    prompts_dir: &Path,
) -> Result<String> {
    let template = load_prompt("directional", provider_name, prompts_dir)
        .context("failed to load directional prompt")?;

    let rag_text = ctx
        .rag_chunks
        .iter()
        .enumerate()
        .map(|(i, c)| format!("[{}] {}", i + 1, c.chunk.text))
        .collect::<Vec<_>>()
        .join("\n\n");

    let key_skills = ctx.digest.key_skills.join(", ");

    Ok(template
        .replace("{session_domain}", &ctx.digest.domain)
        .replace("{rag_chunks}", &rag_text)
        .replace(
            "{rolling_summary_if_compressed}",
            &ctx.memory_ctx.rolling_summary,
        )
        .replace("{last_n_turns}", &ctx.memory_ctx.recent_turns)
        .replace("{question}", &ctx.question)
        .replace("{role}", &ctx.digest.role)
        .replace("{key_skills}", &key_skills))
}
