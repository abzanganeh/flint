//! Clarifying question thread (design doc §8, task 4.9).
//!
//! Fires when the incoming utterance is potentially ambiguous.
//! The result is a single short clarifying question the user can ask.
//! Result emitted via `clarifying_question` event.

use std::path::Path;
use std::time::Instant;

use std::sync::Arc;

use anyhow::{Context, Result};
use tauri::{AppHandle, Runtime};
use tracing::info;

use crate::events::{emit_clarifying_question, ClarifyingQuestionPayload};
use crate::llm::failover::FailoverManager;
use crate::llm::provider::CompletionConfig;

use super::{load_prompt, OrchestrationContext};

// ──────────────────────────────────────────────────────────────────────────────
// Tracing helper
// ──────────────────────────────────────────────────────────────────────────────

fn log_clarifying_failed(session_id: uuid::Uuid, err: &anyhow::Error) {
    tracing::warn!(
        session_id = %session_id,
        error = %err,
        "clarifying thread failed"
    );
}

/// Execute the clarifying question thread.
///
/// Returns the generated clarifying question, or `None` if the response was
/// empty or generation failed.
pub async fn run_clarifying<R: Runtime>(
    ctx: OrchestrationContext,
    failover: Arc<FailoverManager>,
    prompts_dir: &Path,
    app: AppHandle<R>,
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
            log_clarifying_failed(ctx.session_id, &e);
            Ok(None)
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::{FailingMockLLMProvider, LLMProvider, MockLLMProvider};
    use crate::llm::rate_limiter::RateLimiter;
    use crate::session::memory::MemoryContext;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
    use uuid::Uuid;

    fn mock_app_handle() -> tauri::AppHandle<MockRuntime> {
        mock_builder()
            .build(mock_context(noop_assets()))
            .expect("mock app")
            .handle()
            .clone()
    }

    fn prompts_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts")
    }

    fn test_context(question: &str) -> OrchestrationContext {
        OrchestrationContext {
            session_id: Uuid::new_v4(),
            question: question.to_string(),
            rag_chunks: vec![],
            qa_chunks: vec![],
            digest: Arc::new(crate::digest::Digest {
                role: "Engineer".to_string(),
                company: "Acme".to_string(),
                domain: "software engineering".to_string(),
                key_skills: vec!["Rust".to_string()],
                seniority: "senior".to_string(),
                likely_questions: vec![],
                topics_to_avoid: vec![],
            }),
            memory_ctx: MemoryContext {
                rolling_summary: String::new(),
                recent_turns: String::new(),
                truncated: false,
            },
            from_cache: false,
            from_preferred: false,
            preferred_answer: String::new(),
            cached_directional: None,
            cached_depth: None,
            turn_cancel: Arc::new(AtomicBool::new(false)),
            turn_number: 1,
        }
    }

    fn make_failover(primary: Arc<dyn LLMProvider>) -> Arc<FailoverManager> {
        let local: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "ollama".to_string(),
            provider_name: "ollama".to_string(),
        });
        let rl = Arc::new(RateLimiter::new("mock", 60, 60_000));
        Arc::new(FailoverManager::new(primary, vec![], local, rl))
    }

    #[tokio::test]
    async fn run_clarifying_emits_event_on_success() {
        let primary: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "Could you specify which system you mean?".to_string(),
            provider_name: "default".to_string(),
        });
        let failover = make_failover(primary);
        let app = mock_app_handle();

        let ctx = test_context("It was complicated.");
        let result = run_clarifying(ctx, failover, &prompts_dir(), app)
            .await
            .expect("clarifying must return Ok");

        assert_eq!(
            result.as_deref(),
            Some("Could you specify which system you mean?")
        );
    }

    #[tokio::test]
    async fn run_clarifying_returns_none_when_response_is_whitespace() {
        let primary: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "   \n   ".to_string(),
            provider_name: "default".to_string(),
        });
        let failover = make_failover(primary);
        let app = mock_app_handle();

        let ctx = test_context("Anything you want to add?");
        let result = run_clarifying(ctx, failover, &prompts_dir(), app)
            .await
            .expect("whitespace response must not error");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn run_clarifying_logs_and_returns_none_on_provider_failure() {
        // Local provider also fails so the failover path errors all the way.
        let primary: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
            provider_name: "default".to_string(),
            error_message: "network down".to_string(),
        });
        let local: Arc<dyn LLMProvider> = Arc::new(FailingMockLLMProvider {
            provider_name: "ollama".to_string(),
            error_message: "local also down".to_string(),
        });
        let rl = Arc::new(RateLimiter::new("mock", 60, 60_000));
        let failover = Arc::new(FailoverManager::new(primary, vec![], local, rl));
        let app = mock_app_handle();

        let ctx = test_context("Could you clarify?");
        let result = run_clarifying(ctx, failover, &prompts_dir(), app)
            .await
            .expect("clarifying failure must be Ok(None), not Err");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn run_clarifying_errors_when_prompt_directory_missing() {
        let primary: Arc<dyn LLMProvider> = Arc::new(MockLLMProvider {
            response: "ok".to_string(),
            provider_name: "default".to_string(),
        });
        let failover = make_failover(primary);
        let app = mock_app_handle();

        let bogus = std::env::temp_dir().join("flint-no-such-prompts");
        let ctx = test_context("clarify please");
        let result = run_clarifying(ctx, failover, &bogus, app).await;
        assert!(result.is_err(), "missing prompts dir must surface an error");
    }

    #[test]
    fn log_clarifying_failed_does_not_panic() {
        log_clarifying_failed(Uuid::new_v4(), &anyhow::anyhow!("boom"));
    }
}
