//! Mock interview coach — analyzes the user's answer and returns structured
//! `CoachFeedback` JSON.
//!
//! Runs as a one-shot `tokio::spawn` per turn.  The result is persisted to
//! `mock_turns.coach_json` and emitted as a `mock_coach_feedback` event.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};
use tokio::time::timeout;
use tracing::{info, warn};
use uuid::Uuid;

use crate::events::{emit_mock_coach_feedback, MockCoachFeedbackPayload};
use crate::interfaces::vector::ScoredChunk;
use crate::llm::failover::FailoverManager;
use crate::llm::provider::CompletionConfig;
use crate::orchestrator::load_prompt;

// ── Domain types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrammarIssue {
    pub original: String,
    pub fix: String,
    pub why: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToneAssessment {
    pub assessment: String,
    pub suggestion: String,
}

/// Structured coaching output that the frontend renders in the Coach panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoachFeedback {
    pub grammar_issues: Vec<GrammarIssue>,
    pub tone: ToneAssessment,
    pub context_gaps: Vec<String>,
    pub corrected_answer: String,
    pub score: u8,
}

impl Default for CoachFeedback {
    fn default() -> Self {
        Self {
            grammar_issues: vec![],
            tone: ToneAssessment {
                assessment: "good".to_string(),
                suggestion: String::new(),
            },
            context_gaps: vec![],
            corrected_answer: String::new(),
            score: 0,
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Build and fire the coach LLM call for a single mock turn.
/// Emits `mock_coach_feedback` on completion.  Returns the serialised JSON.
#[allow(clippy::too_many_arguments)]
pub async fn run_coach<R: Runtime>(
    app: AppHandle<R>,
    session_id: Uuid,
    turn_n: u32,
    question: String,
    user_answer: String,
    rag_chunks: Vec<ScoredChunk>,
    failover: Arc<FailoverManager>,
    prompts_dir: &Path,
) -> Result<(String, u8)> {
    let prompt = build_coach_prompt(
        &question,
        &user_answer,
        &rag_chunks,
        failover.active_provider_name(),
        prompts_dir,
    )?;

    let config = CompletionConfig {
        max_tokens: Some(600),
        temperature: 0.0,
        stream: true,
    };

    let mut stream = failover
        .complete_stream(prompt, config, &app, 600)
        .await
        .context("coach stream failed")?;

    let mut raw = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(90);

    while std::time::Instant::now() < deadline {
        match timeout(Duration::from_secs(20), stream.next()).await {
            Ok(Some(Ok(token))) => raw.push_str(&token),
            Ok(Some(Err(e))) => return Err(e).context("coach token error"),
            Ok(None) => break,
            Err(_) => {
                warn!(session_id = %session_id, "coach stream stalled");
                break;
            }
        }
    }

    let (feedback, json) = parse_coach_json(&raw);
    let score = feedback.score;

    info!(
        session_id = %session_id,
        turn_n,
        score,
        "coach feedback ready"
    );

    emit_mock_coach_feedback(
        &app,
        MockCoachFeedbackPayload {
            turn_n,
            coach_json: json.clone(),
            score,
        },
    );

    Ok((json, score))
}

// ── Prompt builder ────────────────────────────────────────────────────────────

fn build_coach_prompt(
    question: &str,
    user_answer: &str,
    rag_chunks: &[ScoredChunk],
    provider: &str,
    prompts_dir: &Path,
) -> Result<String> {
    let template = load_prompt("mock_coach", provider, prompts_dir)?;
    let rag_text = rag_chunks
        .iter()
        .take(5)
        .map(|c| c.chunk.text.as_str())
        .collect::<Vec<_>>()
        .join("\n---\n");

    let prompt = template
        .replace("{question}", question)
        .replace("{user_answer}", user_answer)
        .replace("{rag_chunks}", &rag_text);
    Ok(prompt)
}

// ── JSON parser ───────────────────────────────────────────────────────────────

/// Extract the JSON object from the LLM output (model may add preamble/suffix).
fn parse_coach_json(raw: &str) -> (CoachFeedback, String) {
    // Find the first '{' and last '}'.
    if let (Some(start), Some(end)) = (raw.find('{'), raw.rfind('}')) {
        let json_slice = &raw[start..=end];
        if let Ok(feedback) = serde_json::from_str::<CoachFeedback>(json_slice) {
            return (feedback, json_slice.to_owned());
        }
    }
    // Fallback: return a default stub so the UI doesn't crash.
    let fallback = CoachFeedback {
        context_gaps: vec!["Coach feedback could not be parsed.".to_string()],
        ..Default::default()
    };
    let json = serde_json::to_string(&fallback).unwrap_or_default();
    (fallback, json)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_coach_json() {
        let raw = r#"
        Some preamble the model forgot to omit.
        {
          "grammar_issues": [],
          "tone": { "assessment": "good", "suggestion": "" },
          "context_gaps": [],
          "corrected_answer": "I led the migration to microservices.",
          "score": 82
        }
        trailing text
        "#;
        let (fb, json) = parse_coach_json(raw);
        assert_eq!(fb.score, 82);
        assert_eq!(fb.corrected_answer, "I led the migration to microservices.");
        assert!(json.starts_with('{'));
    }

    #[test]
    fn parse_broken_json_returns_fallback() {
        let raw = "sorry, I cannot help with that.";
        let (fb, _) = parse_coach_json(raw);
        assert_eq!(fb.score, 0);
        assert!(!fb.context_gaps.is_empty());
    }
}
