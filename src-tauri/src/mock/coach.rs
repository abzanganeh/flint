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
use crate::mock::conductor::MockMode;
use crate::mock::echo::{collect_shared_vocab_terms, should_cap_echo_score};
use crate::mock::transcript::text_has_profanity;
use crate::orchestrator::load_prompt;

/// Score cap when the user reads the suggested answer in practice mode.
const ECHO_SCORE_CAP: u8 = 45;
/// Minimum spoken words for a scored answer in practice mode.
const MIN_ANSWER_WORDS: usize = 5;
/// avg_logprob below this threshold indicates STT confidence was too low to
/// score delivery fairly.  Whisper avg_logprob is typically in [-0.3, -0.6]
/// for clear speech and < -0.7 for garbled / low-confidence output.
const STT_LOW_CONFIDENCE_THRESHOLD: f32 = -0.7;

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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CoachAxes {
    pub content: u8,
    pub specificity: u8,
    pub company_alignment: u8,
    pub delivery: u8,
}

/// Structured coaching output that the frontend renders in the Coach panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoachFeedback {
    pub grammar_issues: Vec<GrammarIssue>,
    pub tone: ToneAssessment,
    pub context_gaps: Vec<String>,
    pub corrected_answer: String,
    pub score: u8,
    /// Multi-axis rubric — optional for backward compat with persisted turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axes: Option<CoachAxes>,
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
            axes: None,
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
    suggested_answer: String,
    rag_chunks: Vec<ScoredChunk>,
    company_context: &str,
    speaking_style: &str,
    // Mean avg_logprob across all Whisper chunks for this turn. `None` when
    // the transcript came from persistence (regrade) rather than live capture.
    transcript_confidence: Option<f32>,
    mode: MockMode,
    failover: Arc<FailoverManager>,
    prompts_dir: &Path,
) -> Result<(String, u8)> {
    let prompt = build_coach_prompt(
        &question,
        &user_answer,
        &suggested_answer,
        &rag_chunks,
        company_context,
        speaking_style,
        transcript_confidence,
        mode,
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

    let (mut feedback, mut json) = parse_coach_json(&raw);
    normalize_feedback(&mut feedback);
    apply_coach_guardrails(
        &mut feedback,
        &user_answer,
        &suggested_answer,
        company_context,
        transcript_confidence,
        mode,
    );
    let score = feedback.score;
    json = serde_json::to_string(&feedback).unwrap_or(json);

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

#[allow(clippy::too_many_arguments)]
fn build_coach_prompt(
    question: &str,
    user_answer: &str,
    suggested_answer: &str,
    rag_chunks: &[ScoredChunk],
    company_context: &str,
    speaking_style: &str,
    transcript_confidence: Option<f32>,
    mode: MockMode,
    provider: &str,
    prompts_dir: &Path,
) -> Result<String> {
    let template_name = match mode {
        MockMode::Study => "mock_coach/study",
        MockMode::Practice => "mock_coach",
    };
    let template = load_prompt(template_name, provider, prompts_dir)?;
    let rag_text = rag_chunks
        .iter()
        .take(5)
        .map(|c| c.chunk.text.as_str())
        .collect::<Vec<_>>()
        .join("\n---\n");

    let confidence_note = build_confidence_note(transcript_confidence);

    let prompt = template
        .replace("{question}", question)
        .replace("{user_answer}", user_answer)
        .replace("{suggested_answer}", suggested_answer)
        .replace("{rag_chunks}", &rag_text)
        .replace("{company_context}", company_context)
        .replace("{speaking_style}", speaking_style)
        .replace("{transcript_confidence_note}", &confidence_note);
    Ok(prompt)
}

/// Build a human-readable note about STT reliability for the coach prompt.
fn build_confidence_note(transcript_confidence: Option<f32>) -> String {
    let mut parts = vec![
        "Speech-to-text is machine-generated and often garbles words. Whisper \
         frequently hallucinates profanity where none was spoken (e.g. 'specifically' \
         misheard as 'specific' + an expletive). NEVER coach the candidate on swearing \
         unless the prepared script also contains that language. Do NOT quote expletives \
         in tone.suggestion or grammar_issues."
            .to_string(),
    ];

    if let Some(lp) = transcript_confidence.filter(|lp| *lp < STT_LOW_CONFIDENCE_THRESHOLD) {
        parts.push(format!(
            "WARNING: speech-to-text confidence was low this turn \
             (avg_logprob={lp:.2}). The transcript may contain additional recognition \
             errors. Do NOT score delivery lower than 55 and do NOT flag too_hesitant \
             unless there is clear evidence of hesitation beyond STT artefacts. Omit \
             grammar_issues that look like transcription noise (single garbled words, \
             nonsense fragments)."
        ));
    }

    parts.join("\n")
}

/// Backfill rubric axes for legacy coach JSON and clamp values.
fn normalize_feedback(feedback: &mut CoachFeedback) {
    if feedback.axes.is_none() {
        feedback.axes = Some(CoachAxes {
            content: feedback.score,
            specificity: feedback.score,
            company_alignment: feedback.score,
            delivery: feedback.score,
        });
    }
    if let Some(axes) = &mut feedback.axes {
        axes.content = axes.content.min(100);
        axes.specificity = axes.specificity.min(100);
        axes.company_alignment = axes.company_alignment.min(100);
        axes.delivery = axes.delivery.min(100);
    }
    feedback.score = feedback.score.min(100);
}

/// Enforce deterministic scoring rules the LLM may ignore.
fn apply_coach_guardrails(
    feedback: &mut CoachFeedback,
    user_answer: &str,
    suggested_answer: &str,
    company_context: &str,
    transcript_confidence: Option<f32>,
    mode: MockMode,
) {
    let word_count = count_words(user_answer);

    if word_count < MIN_ANSWER_WORDS {
        feedback.score = 0;
        if let Some(axes) = &mut feedback.axes {
            *axes = CoachAxes::default();
        }
        if !feedback
            .context_gaps
            .iter()
            .any(|g| g.contains("No answer recorded"))
        {
            feedback
                .context_gaps
                .insert(0, "No answer recorded".to_string());
        }
        return;
    }

    // If STT confidence was low, floor delivery at 55 and clear too_hesitant.
    let stt_low = matches!(transcript_confidence, Some(lp) if lp < STT_LOW_CONFIDENCE_THRESHOLD);
    if stt_low {
        if let Some(axes) = &mut feedback.axes {
            if axes.delivery < 55 {
                axes.delivery = 55;
            }
        }
        if feedback.score < 55 {
            feedback.score = 55;
        }
        if feedback.tone.assessment == "too_hesitant" {
            feedback.tone.assessment = "good".to_string();
            feedback.tone.suggestion = String::new();
        }
        let note = "Note: transcription confidence was low — delivery score may not fully reflect actual delivery quality.";
        if !feedback
            .context_gaps
            .iter()
            .any(|g| g.contains("transcription confidence"))
        {
            feedback.context_gaps.push(note.to_string());
        }
    }

    if mode == MockMode::Practice && !suggested_answer.trim().is_empty() {
        let exclude = collect_shared_vocab_terms(company_context, suggested_answer);
        if should_cap_echo_score(user_answer, suggested_answer, &exclude) {
            feedback.score = feedback.score.min(ECHO_SCORE_CAP);
            if let Some(axes) = &mut feedback.axes {
                axes.delivery = axes.delivery.min(ECHO_SCORE_CAP);
            }
            let msg = "You read the suggested answer — practice in your own words.";
            if !feedback.context_gaps.iter().any(|g| g == msg) {
                feedback.context_gaps.insert(0, msg.to_string());
            }
        }
    }

    scrub_false_profanity_coaching(feedback, user_answer, suggested_answer);
}

fn count_words(text: &str) -> usize {
    text.split_whitespace().filter(|w| !w.is_empty()).count()
}

/// Remove coach feedback that cites profanity when neither transcript nor script contains it.
fn scrub_false_profanity_coaching(
    feedback: &mut CoachFeedback,
    user_answer: &str,
    suggested_answer: &str,
) {
    if text_has_profanity(user_answer) || text_has_profanity(suggested_answer) {
        return;
    }

    if feedback_cites_profanity(&feedback.tone.suggestion) {
        feedback.tone.suggestion = if feedback.tone.assessment == "good" {
            String::new()
        } else {
            "Slow down slightly for clearer, confident delivery.".to_string()
        };
    }

    feedback.grammar_issues.retain(|issue| {
        !feedback_cites_profanity(&issue.original)
            && !feedback_cites_profanity(&issue.fix)
            && !feedback_cites_profanity(&issue.why)
    });
}

fn feedback_cites_profanity(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("fuck")
        || lower.contains("f*ck")
        || lower.contains("expletive")
        || lower.contains("profan")
        || lower.contains("swear")
        || lower.contains("curse")
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
    fn parse_valid_coach_json_with_axes() {
        let raw = r#"{
          "grammar_issues": [],
          "tone": { "assessment": "good", "suggestion": "" },
          "context_gaps": [],
          "corrected_answer": "I led the migration.",
          "score": 82,
          "axes": { "content": 85, "specificity": 80, "company_alignment": 75, "delivery": 78 }
        }"#;
        let (fb, _) = parse_coach_json(raw);
        assert_eq!(fb.score, 82);
        let axes = fb.axes.expect("axes");
        assert_eq!(axes.content, 85);
        assert_eq!(axes.company_alignment, 75);
    }

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

    #[test]
    fn guardrails_zero_score_for_empty_answer() {
        let mut fb = CoachFeedback {
            score: 80,
            ..Default::default()
        };
        apply_coach_guardrails(
            &mut fb,
            "um",
            "full suggested script here",
            "",
            None,
            MockMode::Practice,
        );
        assert_eq!(fb.score, 0);
    }

    #[test]
    fn guardrails_caps_score_when_reading_suggested_script() {
        let script = "I built an agentic AI system with hybrid RAG and evaluation gates achieving high accuracy.";
        let mut fb = CoachFeedback {
            score: 85,
            ..Default::default()
        };
        apply_coach_guardrails(&mut fb, script, script, "", None, MockMode::Practice);
        assert_eq!(fb.score, ECHO_SCORE_CAP);
        assert!(fb
            .context_gaps
            .iter()
            .any(|g| g.contains("read the suggested answer")));
    }

    #[test]
    fn guardrails_does_not_cap_echo_in_study_mode() {
        let script = "I built an agentic AI system with hybrid RAG and evaluation gates achieving high accuracy.";
        let mut fb = CoachFeedback {
            score: 85,
            ..Default::default()
        };
        apply_coach_guardrails(&mut fb, script, script, "", None, MockMode::Study);
        assert_eq!(fb.score, 85);
    }

    #[test]
    fn guardrails_floors_delivery_on_low_stt_confidence() {
        let answer = "Fisher takes fiduciary responsibility seriously fee only client interests.";
        let mut fb = CoachFeedback {
            score: 40,
            tone: ToneAssessment {
                assessment: "too_hesitant".to_string(),
                suggestion: "Slow down.".to_string(),
            },
            axes: Some(CoachAxes {
                content: 40,
                specificity: 40,
                company_alignment: 40,
                delivery: 30,
            }),
            ..Default::default()
        };
        apply_coach_guardrails(&mut fb, answer, "", "", Some(-0.85), MockMode::Practice);
        assert!(
            fb.score >= 55,
            "score should be floored at 55, got {}",
            fb.score
        );
        assert_ne!(fb.tone.assessment, "too_hesitant");
        let axes = fb.axes.as_ref().unwrap();
        assert!(axes.delivery >= 55);
        assert!(fb
            .context_gaps
            .iter()
            .any(|g| g.contains("transcription confidence")));
    }

    #[test]
    fn guardrails_scrubs_false_profanity_coaching() {
        let mut fb = CoachFeedback {
            tone: ToneAssessment {
                assessment: "good".to_string(),
                suggestion: "Avoid filler words like 'F*CK!'".to_string(),
            },
            ..Default::default()
        };
        let answer = "A few things stood out about Fisher specifically.";
        apply_coach_guardrails(&mut fb, answer, answer, "", None, MockMode::Practice);
        assert!(!fb.tone.suggestion.to_lowercase().contains("fuck"));
    }
}
