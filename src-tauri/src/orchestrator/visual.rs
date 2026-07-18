//! Visual response thread (design doc §8, task 4.8; `lpav-s19-visual-thread`).
//!
//! Renamed from `depth.rs` — repurposes the same streaming/failover/cache
//! plumbing, but the output contract is different: `/prompts/visual/`
//! (slice 17) instructs the model to emit exactly one fenced Mermaid diagram
//! or code block. A half-streamed diagram can't be rendered, so unlike the
//! Answer thread this module buffers tokens internally and only emits once
//! a complete fenced block has arrived (or the stream ends, as a raw
//! fallback) — see [`extract_complete_fence`].
//!
//! Fully streamed in < 8s P95. Prompt loaded from
//! `/prompts/visual/{provider}.txt` or `default.txt`.

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

use crate::events::{emit_thread_status, emit_visual_token, ThreadStatusPayload, VisualTokenPayload};
use crate::llm::failover::FailoverManager;
use crate::llm::provider::CompletionConfig;

use super::{load_prompt, OrchestrationContext};

/// Execute the Visual response thread.
///
/// Emits the complete fenced block to the React layer via a single
/// `visual_token` event once the closing fence has arrived — never a
/// token-by-token stream, since a partially-fenced Mermaid block cannot be
/// rendered.
/// Returns the full assembled response text.
pub async fn run_visual<R: Runtime>(
    ctx: OrchestrationContext,
    failover: Arc<FailoverManager>,
    prompts_dir: &Path,
    app: AppHandle<R>,
) -> Result<String> {
    let start = Instant::now();
    let provider_name = failover.active_provider_name().to_string();

    // Pre-warm / preferred hit — serve cached visual block; on turn ≥ 3 also run fresh LLM
    // unless this is a user-saved preferred script.
    if let Some(cached) = ctx.cached_visual.clone() {
        let mut full_response = emit_cached_visual_block(&cached, &app, &ctx.turn_cancel);

        if ctx.turn_number >= 3 && !ctx.from_preferred {
            log_refresh_on_turn_three(ctx.session_id, ctx.turn_number);
            match run_fresh_visual(&ctx, Arc::clone(&failover), prompts_dir, &app).await {
                Ok(fresh) if !fresh.is_empty() => full_response = fresh,
                Ok(_) => {}
                Err(e) => {
                    log_fresh_refresh_failed(ctx.session_id, &e);
                }
            }
        }

        let stream_ms = start.elapsed().as_millis() as u64;
        log_visual_complete(ctx.session_id, stream_ms, &provider_name, true);
        emit_thread_status(
            &app,
            ThreadStatusPayload {
                thread: "visual".to_string(),
                status: "ok".to_string(),
            },
        );
        return Ok(full_response);
    }

    run_fresh_visual(&ctx, failover, prompts_dir, &app).await
}

async fn run_fresh_visual<R: Runtime>(
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
        .context("visual stream failed")?;

    let mut full_response = String::new();
    let mut flushed = false;
    let stream_deadline = Instant::now() + Duration::from_secs(60);

    while Instant::now() < stream_deadline {
        if ctx.turn_cancel.load(Ordering::Acquire) {
            break;
        }
        match timeout(Duration::from_secs(15), stream.next()).await {
            Ok(Some(Ok(token))) => {
                full_response.push_str(&token);
                if !flushed {
                    if let Some(block) = extract_complete_fence(&full_response) {
                        emit_visual_token(
                            app,
                            VisualTokenPayload {
                                token: block.to_string(),
                            },
                        );
                        flushed = true;
                    }
                }
            }
            Ok(Some(Err(e))) => return Err(e).context("visual token error"),
            Ok(None) => break,
            Err(_) => {
                warn!(
                    session_id = %ctx.session_id,
                    "visual stream stalled — returning partial response"
                );
                break;
            }
        }
    }

    // Malformed or truncated stream — no closing fence ever arrived. Flush
    // whatever we have so the panel can at least show the raw fallback
    // (VisualPanel Tier 1 renders raw text when Mermaid parsing fails).
    if !flushed && !full_response.trim().is_empty() {
        emit_visual_token(
            app,
            VisualTokenPayload {
                token: full_response.clone(),
            },
        );
    }

    let stream_ms = start.elapsed().as_millis() as u64;
    if stream_ms > 8_000 {
        log_visual_nfr_breach(ctx.session_id, stream_ms);
    }

    log_visual_complete(ctx.session_id, stream_ms, &provider_name, ctx.from_cache);

    emit_thread_status(
        app,
        ThreadStatusPayload {
            thread: "visual".to_string(),
            status: "ok".to_string(),
        },
    );

    Ok(full_response)
}

/// Returns the first complete fenced block in `buffer` — both an opening and
/// a matching closing ` ``` ` have arrived — or `None` while the stream is
/// still mid-block. Extracted standalone so fence detection can be unit
/// tested without spinning up a mock LLM stream.
fn extract_complete_fence(buffer: &str) -> Option<&str> {
    const FENCE: &str = "```";
    let start = buffer.find(FENCE)?;
    let after_open = start + FENCE.len();
    let close_offset = buffer[after_open..].find(FENCE)?;
    let end = after_open + close_offset + FENCE.len();
    Some(&buffer[start..end])
}

/// Helpers extracted so tarpaulin attributes coverage to the call site —
/// inline tracing macro arguments are reported as uncovered even when hit.
fn log_refresh_on_turn_three(session_id: Uuid, turn: usize) {
    info!(
        session_id = %session_id,
        turn = turn,
        "cache hit turn ≥ 3 — running fresh visual in parallel"
    );
}

fn log_fresh_refresh_failed(session_id: Uuid, error: &Error) {
    warn!(
        session_id = %session_id,
        error = %error,
        "fresh visual after cache hit failed — keeping cached response"
    );
}

fn log_visual_nfr_breach(session_id: Uuid, stream_ms: u64) {
    warn!(
        session_id = %session_id,
        stream_ms,
        "visual stream > 8s — NFR breach"
    );
}

fn log_visual_complete(session_id: Uuid, stream_ms: u64, provider: &str, cache_hit: bool) {
    info!(
        session_id = %session_id,
        event = "visual_thread_complete",
        thread_type = "visual",
        stream_complete_ms = stream_ms,
        provider = %provider,
        model = %provider,
        cache_hit = cache_hit,
        "visual thread finished"
    );
}

/// Emit a cached fenced block as a single `visual_token` event — cached text
/// is already a complete block, so there's no fence to wait for.
fn emit_cached_visual_block<R: Runtime>(
    text: &str,
    app: &AppHandle<R>,
    cancel: &Arc<std::sync::atomic::AtomicBool>,
) -> String {
    if !cancel.load(Ordering::Acquire) {
        emit_visual_token(
            app,
            VisualTokenPayload {
                token: text.to_string(),
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
        load_prompt("visual", provider_name, prompts_dir).context("failed to load visual prompt")?;

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
    let style = crate::session::question_attempts::answer_style_instructions(&ctx.question, true);
    let preferred_hint = if ctx.preferred_answer.trim().is_empty() || ctx.from_preferred {
        String::new()
    } else {
        format!(
            "\n\n[Your saved preferred answer — expand on this script, do not contradict it]\n{}",
            ctx.preferred_answer.trim()
        )
    };

    Ok(template
        .replace("{session_domain}", &ctx.digest.domain)
        .replace("{rag_chunks}", &rag_text)
        .replace("{qa_chunks}", &qa_section)
        .replace("{answer_style_instructions}", &style)
        .replace("{preferred_answer_hint}", &preferred_hint)
        .replace(
            "{rolling_summary_if_compressed}",
            &ctx.memory_ctx.rolling_summary,
        )
        .replace("{last_n_turns}", &ctx.memory_ctx.recent_turns)
        .replace("{question}", &ctx.question)
        .replace("{interviewer_role}", &ctx.digest.role)
        .replace("{interviewer_priorities}", &key_skills))
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_complete_fence_returns_none_when_no_fence_opened() {
        assert!(extract_complete_fence("just some prose, no fence yet").is_none());
    }

    #[test]
    fn extract_complete_fence_returns_none_when_only_opening_fence_arrived() {
        assert!(extract_complete_fence("```mermaid\nflowchart TD\nA-->B").is_none());
    }

    #[test]
    fn extract_complete_fence_returns_block_once_closing_fence_arrives() {
        let buffer = "```mermaid\nflowchart TD\nA-->B\n```";
        let block = extract_complete_fence(buffer).expect("fence must be detected");
        assert_eq!(block, buffer);
    }

    #[test]
    fn extract_complete_fence_ignores_trailing_content_after_close() {
        let buffer = "```mermaid\nflowchart TD\nA-->B\n```\nSome trailing prose the model added.";
        let block = extract_complete_fence(buffer).expect("fence must be detected");
        assert_eq!(block, "```mermaid\nflowchart TD\nA-->B\n```");
    }

    #[test]
    fn extract_complete_fence_handles_fence_with_no_language_tag() {
        let buffer = "```\nsequenceDiagram\nA->>B: hi\n```";
        let block = extract_complete_fence(buffer).expect("fence must be detected");
        assert_eq!(block, buffer);
    }
}
