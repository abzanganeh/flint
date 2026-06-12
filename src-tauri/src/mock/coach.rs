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
    let trimmed = strip_markdown_fence(raw.trim());

    if let Some(slice) = extract_balanced_json(trimmed) {
        if let Ok(feedback) = serde_json::from_str::<CoachFeedback>(&slice) {
            return (feedback, slice);
        }
        if let Some(feedback) = salvage_coach_fields(&slice) {
            let json = serde_json::to_string(&feedback).unwrap_or(slice.clone());
            warn!("coach JSON required field salvage");
            return (feedback, json);
        }
    }

    if let Some(feedback) = salvage_coach_fields(trimmed) {
        let json = serde_json::to_string(&feedback).unwrap_or_default();
        warn!("coach JSON unparseable — salvaged partial fields");
        return (feedback, json);
    }

    coach_parse_failure("Coach feedback could not be parsed.")
}

/// Build a fallback payload and emit it when the coach LLM call fails entirely.
pub fn coach_failure_payload(message: &str) -> (String, u8) {
    let (_, json) = coach_parse_failure(message);
    (json, 0)
}

fn coach_parse_failure(message: &str) -> (CoachFeedback, String) {
    let fallback = CoachFeedback {
        context_gaps: vec![message.to_string()],
        ..Default::default()
    };
    let json = serde_json::to_string(&fallback).unwrap_or_default();
    (fallback, json)
}

fn strip_markdown_fence(raw: &str) -> &str {
    let stripped = raw
        .strip_prefix("```json")
        .or_else(|| raw.strip_prefix("```"))
        .unwrap_or(raw);
    stripped.strip_suffix("```").unwrap_or(stripped).trim()
}

/// Return the first `{ … }` slice with string-aware brace matching.
fn extract_balanced_json(raw: &str) -> Option<String> {
    let start = raw.find('{')?;
    let bytes = raw.as_bytes();
    let mut depth = 0u32;
    let mut in_string = false;
    let mut escape = false;

    for i in start..bytes.len() {
        let b = bytes[i];
        if escape {
            escape = false;
            continue;
        }
        if in_string {
            if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(raw[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Best-effort field extraction when the model returns almost-valid JSON.
fn salvage_coach_fields(raw: &str) -> Option<CoachFeedback> {
    let score = extract_json_u8_field(raw, "score")?;
    let mut feedback = CoachFeedback {
        score,
        ..Default::default()
    };

    if let Some(assessment) = extract_json_string_field(raw, "assessment") {
        feedback.tone.assessment = assessment;
    }
    if let Some(suggestion) = extract_json_string_field(raw, "suggestion") {
        feedback.tone.suggestion = suggestion;
    }
    if let Some(answer) = extract_json_string_field(raw, "corrected_answer") {
        feedback.corrected_answer = answer;
    }

    feedback.context_gaps = extract_json_string_array(raw, "context_gaps");

    Some(feedback)
}

fn extract_json_u8_field(raw: &str, key: &str) -> Option<u8> {
    let needle = format!("\"{key}\"");
    let pos = raw.find(&needle)?;
    let mut rest = raw[pos + needle.len()..].trim_start();
    if !rest.starts_with(':') {
        return None;
    }
    rest = rest[1..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    rest[..end].parse::<u8>().ok().map(|n| n.min(100))
}

fn extract_json_string_field(raw: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let pos = raw.find(&needle)?;
    let mut rest = raw[pos + needle.len()..].trim_start();
    if !rest.starts_with(':') {
        return None;
    }
    rest = rest[1..].trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    rest = &rest[1..];

    let mut out = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            },
            '"' => return Some(out),
            other => out.push(other),
        }
    }
    None
}

fn extract_json_string_array(raw: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{key}\"");
    let Some(pos) = raw.find(&needle) else {
        return vec![];
    };
    let mut rest = raw[pos + needle.len()..].trim_start();
    if !rest.starts_with(':') {
        return vec![];
    }
    rest = rest[1..].trim_start();
    let Some(start) = rest.find('[') else {
        return vec![];
    };
    let mut depth = 0u32;
    let mut in_string = false;
    let mut escape = false;
    let bytes = rest.as_bytes();
    let mut end_idx = None;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if escape {
            escape = false;
            continue;
        }
        if in_string {
            if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' => depth += 1,
            b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    end_idx = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(end) = end_idx else {
        return vec![];
    };
    let slice = &rest[start..=end];
    serde_json::from_str::<Vec<String>>(slice).unwrap_or_default()
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

    #[test]
    fn salvage_score_from_malformed_json() {
        let raw = r#"{
          "grammar_issues": [],
          "tone": { "assessment": "good", "suggestion": "Be more specific" },
          "context_gaps": ["missing metrics"],
          "corrected_answer": "I built an agentic system in production.\",
          "score": 72
        }"#;
        let (fb, _) = parse_coach_json(raw);
        assert_eq!(fb.score, 72);
        assert_eq!(fb.tone.assessment, "good");
        assert!(!fb.corrected_answer.is_empty());
    }
}
