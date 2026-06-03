//! Clarifying question thread (design doc §8, task 4.9).
//!
//! Fires when the incoming utterance is potentially ambiguous.
//! The result is a single short clarifying question the user can ask.
//! Result emitted via `clarifying_question` event.

use std::path::Path;
use std::time::Instant;

use std::sync::Arc;

use anyhow::{Context, Result};
use tauri::AppHandle;
use tracing::info;

use crate::events::{emit_clarifying_question, ClarifyingQuestionPayload};
use crate::llm::failover::FailoverManager;
use crate::llm::provider::CompletionConfig;

use super::{load_prompt, OrchestrationContext};

/// Execute the clarifying question thread.
///
/// Returns the generated clarifying question, or `None` if the response was
/// empty or generation failed.
pub async fn run_clarifying(
    ctx: OrchestrationContext,
    failover: Arc<FailoverManager>,
    prompts_dir: &Path,
    app: AppHandle,
) -> Result<Option<String>> {
    let start = Instant::now();

    let template = load_prompt("clarifying", failover.active_provider_name(), prompts_dir)
        .context("failed to load clarifying prompt")?;

    let prompt = template
        .replace("{session_domain}", &ctx.digest.domain)
        .replace("{utterance}", &ctx.question);

    let config = CompletionConfig {
        max_tokens: Some(50),
        temperature: 0.2,
        stream: false,
    };

    let provider_name = failover.active_provider_name().to_string();
    match failover.complete_stream(prompt, config, &app, 80).await {
        Ok(mut stream) => {
            use futures::StreamExt;
            let mut text = String::new();
            while let Some(tok) = stream.next().await {
                if let Ok(t) = tok {
                    text.push_str(&t);
                }
            }
            let question = text.trim().to_string();
            if question.is_empty() {
                return Ok(None);
            }
            info!(
                session_id = %ctx.session_id,
                event = "clarifying_thread_complete",
                elapsed_ms = start.elapsed().as_millis(),
                provider = %provider_name,
                "clarifying thread finished"
            );
            emit_clarifying_question(
                &app,
                ClarifyingQuestionPayload {
                    question: question.clone(),
                    rank: 1,
                },
            );
            Ok(Some(question))
        }
        Err(e) => {
            // Clarifying thread failure is non-fatal — log and return None.
            tracing::warn!(
                session_id = %ctx.session_id,
                error = %e,
                "clarifying thread failed"
            );
            Ok(None)
        }
    }
}
