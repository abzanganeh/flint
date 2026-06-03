//! Directional response thread (design doc §8, task 4.7).
//!
//! Fires on every `System`-source question. Target TTFT < 800ms (P95 < 900ms).
//! Prompt loaded from `/prompts/directional/{provider}.txt` or `default.txt`.

use std::path::Path;
use std::time::Instant;

use std::sync::Arc;

use anyhow::{Context, Result};
use futures::StreamExt;
use tauri::AppHandle;
use tracing::{info, warn};

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
pub async fn run_directional(
    ctx: OrchestrationContext,
    failover: Arc<FailoverManager>,
    prompts_dir: &Path,
    app: AppHandle,
) -> Result<String> {
    let ttft_start = Instant::now();

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

    let provider_name = failover.active_provider_name().to_string();
    let mut full_response = String::new();
    let mut first_token = true;

    while let Some(token_result) = stream.next().await {
        let token = token_result.context("directional token error")?;

        if first_token {
            let ttft_ms = ttft_start.elapsed().as_millis() as u64;
            info!(
                session_id = %ctx.session_id,
                event = "directional_ttft",
                ttft_ms,
                provider = %provider_name,
                "directional first token"
            );
            if ttft_ms > 900 {
                warn!(
                    session_id = %ctx.session_id,
                    ttft_ms,
                    "directional TTFT > 900ms — NFR breach"
                );
            }
            first_token = false;
        }

        full_response.push_str(&token);
        emit_directional_token(&app, DirectionalTokenPayload { token });
    }

    let stream_ms = ttft_start.elapsed().as_millis() as u64;
    info!(
        session_id = %ctx.session_id,
        event = "directional_thread_complete",
        stream_complete_ms = stream_ms,
        provider = %provider_name,
        cache_hit = ctx.from_cache,
        "directional thread finished"
    );

    emit_thread_status(
        &app,
        ThreadStatusPayload {
            thread: "directional".to_string(),
            status: "ok".to_string(),
        },
    );

    Ok(full_response)
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
