//! Depth response thread (design doc §8, task 4.8).
//!
//! Fully streamed in < 8s P95. Prompt loaded from
//! `/prompts/depth/{provider}.txt` or `default.txt`.

use std::path::Path;
use std::time::Instant;

use std::sync::Arc;

use anyhow::{Context, Result};
use futures::StreamExt;
use tauri::AppHandle;
use tracing::{info, warn};

use crate::events::{emit_depth_token, emit_thread_status, DepthTokenPayload, ThreadStatusPayload};
use crate::llm::failover::FailoverManager;
use crate::llm::provider::CompletionConfig;

use super::{load_prompt, OrchestrationContext};

/// Execute the depth response thread.
///
/// Streams tokens to the React layer via `depth_token` events.
/// Returns the full assembled response text.
pub async fn run_depth(
    ctx: OrchestrationContext,
    failover: Arc<FailoverManager>,
    prompts_dir: &Path,
    app: AppHandle,
) -> Result<String> {
    let start = Instant::now();

    let prompt = build_prompt(&ctx, failover.active_provider_name(), prompts_dir)?;

    let config = CompletionConfig {
        max_tokens: Some(400),
        temperature: 0.0,
        stream: true,
    };

    let estimated_tokens = 500_u32;
    let provider_name = failover.active_provider_name().to_string();
    let mut stream = failover
        .complete_stream(prompt, config, &app, estimated_tokens)
        .await
        .context("depth stream failed")?;

    let mut full_response = String::new();

    while let Some(token_result) = stream.next().await {
        let token = token_result.context("depth token error")?;
        full_response.push_str(&token);
        emit_depth_token(&app, DepthTokenPayload { token });
    }

    let stream_ms = start.elapsed().as_millis() as u64;
    if stream_ms > 8_000 {
        warn!(
            session_id = %ctx.session_id,
            stream_ms,
            "depth stream > 8s — NFR breach"
        );
    }

    info!(
        session_id = %ctx.session_id,
        event = "depth_thread_complete",
        stream_complete_ms = stream_ms,
        provider = %provider_name,
        cache_hit = ctx.from_cache,
        "depth thread finished"
    );

    emit_thread_status(
        &app,
        ThreadStatusPayload {
            thread: "depth".to_string(),
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
    let template = load_prompt("depth", provider_name, prompts_dir)
        .context("failed to load depth prompt")?;

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
        .replace("{interviewer_role}", &ctx.digest.role)
        .replace("{interviewer_priorities}", &key_skills))
}
