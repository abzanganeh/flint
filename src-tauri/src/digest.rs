//! Digest extraction: parse session context into a structured `Digest` that
//! drives pre-warming and grounds all live-session responses.
//!
//! Reference: design doc §19 (Digest Extraction Prompt), §4 (Core Concept),
//! `.cursor/rules` §14 (memory / universal question bank fallback).
//!
//! # Prompt loading
//!
//! Prompts are **never** inlined as strings. Each call to [`extract_digest`]
//! resolves the prompt file at runtime:
//!
//! 1. Look for `{FLINT_PROMPTS_DIR}/digest/{provider_name}.txt`.
//! 2. Fall back to `{FLINT_PROMPTS_DIR}/digest/default.txt`.
//!
//! In development/test `FLINT_PROMPTS_DIR` defaults to
//! `{CARGO_MANIFEST_DIR}/../prompts` so the prompts directory at the
//! workspace root is used automatically. Set the environment variable in
//! production to point at the bundled resource directory.

#![allow(dead_code)]

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, warn};

use crate::llm::provider::LLMProvider;

// ──────────────────────────────────────────────────────────────────────────────
// Public types
// ──────────────────────────────────────────────────────────────────────────────

/// Structured session context extracted from the user's pasted text.
///
/// Drives pre-warming (via `likely_questions`) and grounds all live-session
/// responses (via `role`, `domain`, `key_skills`, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Digest {
    pub role: String,
    pub company: String,
    pub domain: String,
    pub key_skills: Vec<String>,
    pub seniority: String,
    /// Top 5 questions the interviewer is most likely to ask.
    /// Guaranteed non-empty — padded with universal question bank entries
    /// if the LLM returns fewer than required.
    pub likely_questions: Vec<String>,
    pub topics_to_avoid: Vec<String>,
}

// ──────────────────────────────────────────────────────────────────────────────
// Universal question bank (design doc rules §14)
// ──────────────────────────────────────────────────────────────────────────────

/// Pre-warm priority order when fewer than 5 domain-specific questions are
/// extracted from the digest (`.cursor/rules` flint-data §"Universal Question Bank").
const UNIVERSAL_QUESTION_BANK: &[&str] = &[
    "Tell me about yourself",
    "What are your greatest strengths?",
    "Why are you interested in this role?",
    "Tell me about a significant challenge you faced",
    "Why should we hire you?",
];

// ──────────────────────────────────────────────────────────────────────────────
// Prompt loading
// ──────────────────────────────────────────────────────────────────────────────

/// Base directory for prompt files. Override with `FLINT_PROMPTS_DIR` env var.
fn prompts_base_dir() -> PathBuf {
    std::env::var("FLINT_PROMPTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            // Compile-time anchor: workspace root / prompts
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("prompts")
        })
}

/// Load the digest prompt for `provider_name`.
///
/// Tries `digest/{provider_name}.txt` first; falls back to `digest/default.txt`.
fn load_digest_prompt(provider_name: &str) -> Result<String> {
    let base = prompts_base_dir();

    let specific = base.join("digest").join(format!("{provider_name}.txt"));
    let default = base.join("digest").join("default.txt");

    if specific.exists() {
        debug!(path = %specific.display(), "loading provider-specific digest prompt");
        std::fs::read_to_string(&specific)
            .with_context(|| format!("failed to read digest prompt {}", specific.display()))
    } else {
        debug!(path = %default.display(), "loading default digest prompt");
        std::fs::read_to_string(&default)
            .with_context(|| format!("failed to read default digest prompt {}", default.display()))
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Extraction
// ──────────────────────────────────────────────────────────────────────────────

/// Extract a [`Digest`] from `context_text` using `llm`.
///
/// # Behaviour
///
/// 1. Loads the digest prompt from `prompts/digest/{provider}.txt` (or
///    `default.txt`).
/// 2. Substitutes `{pasted_context}` with `context_text`.
/// 3. Calls the LLM once (non-streaming collection).
/// 4. Parses the response as JSON into a [`Digest`].
/// 5. If `likely_questions` is empty after parsing, pads it with entries from
///    the universal question bank so pre-warming always has questions to fire.
///
/// # Errors
///
/// Returns `Err` if:
/// - The prompt file cannot be read.
/// - The LLM call fails.
/// - The response cannot be parsed as valid JSON matching the `Digest` schema.
///   The raw response is logged at ERROR before returning.
pub async fn extract_digest(context_text: &str, llm: &dyn LLMProvider) -> Result<Digest> {
    let template = load_digest_prompt(llm.name())
        .context("digest prompt unavailable — ensure prompts/digest/default.txt exists")?;

    let prompt = template.replace("{pasted_context}", context_text);

    debug!(provider = llm.name(), "firing digest extraction LLM call");

    let raw = llm
        .complete(
            prompt,
            crate::llm::provider::CompletionConfig {
                max_tokens: Some(1024),
                temperature: 0.0,
                stream: false,
            },
        )
        .await
        .context("LLM call failed during digest extraction")?;

    // Strip potential markdown fencing before parsing.
    let json_str = strip_markdown_fences(raw.trim());

    let mut digest: Digest = serde_json::from_str(json_str).map_err(|parse_err| {
        // raw_response is logged at DEBUG only — it contains LLM-paraphrased
        // pasted context which must not appear in release-build logs
        // (flint-security.mdc §"Hard Constraints").
        error!(
            provider = llm.name(),
            response_len = raw.len(),
            parse_error = %parse_err,
            "digest extraction failed — LLM did not return valid JSON",
        );
        #[cfg(debug_assertions)]
        debug!(raw_response = %raw, "digest LLM raw response (debug builds only)");
        anyhow!("Digest extraction failed — try rephrasing your context")
    })?;

    // Pad likely_questions with universal bank entries if the LLM returned
    // fewer than needed for pre-warming (design doc §4, rules §14).
    if digest.likely_questions.is_empty() {
        warn!(
            provider = llm.name(),
            "LLM returned no likely_questions — using universal question bank"
        );
    }
    pad_likely_questions(&mut digest.likely_questions);

    debug!(
        role = %digest.role,
        domain = %digest.domain,
        questions = digest.likely_questions.len(),
        "digest extraction complete",
    );

    Ok(digest)
}

/// Remove ` ```json … ``` ` or ` ``` … ``` ` wrapping that some LLMs add
/// despite being instructed not to.
fn strip_markdown_fences(s: &str) -> &str {
    let s = s.trim();
    // Remove opening fence
    let s = if s.starts_with("```json") {
        s.trim_start_matches("```json")
    } else if s.starts_with("```") {
        s.trim_start_matches("```")
    } else {
        s
    };
    // Remove closing fence
    let s = if s.ends_with("```") {
        s.trim_end_matches("```")
    } else {
        s
    };
    s.trim()
}

/// Ensure `likely_questions` contains at least 5 entries by appending
/// universal question bank items that are not already present.
fn pad_likely_questions(questions: &mut Vec<String>) {
    for &fallback in UNIVERSAL_QUESTION_BANK {
        if questions.len() >= 5 {
            break;
        }
        let already_present = questions
            .iter()
            .any(|q| q.to_lowercase() == fallback.to_lowercase());
        if !already_present {
            questions.push(fallback.to_string());
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::MockLLMProvider;

    fn valid_digest_json() -> String {
        serde_json::json!({
            "role": "Senior Software Engineer",
            "company": "Acme Corp",
            "domain": "software engineering",
            "key_skills": ["Rust", "distributed systems", "PostgreSQL"],
            "seniority": "senior",
            "likely_questions": [
                "Tell me about your experience with distributed systems",
                "How do you approach system design?",
                "Describe a challenging debugging session",
                "What is your experience with Rust?",
                "How do you handle on-call incidents?"
            ],
            "topics_to_avoid": ["previous salary"]
        })
        .to_string()
    }

    fn mock_llm(response: impl Into<String>) -> MockLLMProvider {
        MockLLMProvider {
            response: response.into(),
            provider_name: "default".to_string(),
        }
    }

    #[tokio::test]
    async fn test_extract_digest_parses_valid_json() {
        let llm = mock_llm(valid_digest_json());
        let digest = extract_digest("some context text", &llm)
            .await
            .expect("extract_digest should succeed");

        assert_eq!(digest.role, "Senior Software Engineer");
        assert_eq!(digest.company, "Acme Corp");
        assert_eq!(digest.domain, "software engineering");
        assert_eq!(digest.seniority, "senior");
        assert_eq!(digest.key_skills.len(), 3);
        assert_eq!(digest.likely_questions.len(), 5);
    }

    #[tokio::test]
    async fn test_extract_digest_pads_empty_likely_questions() {
        let json = serde_json::json!({
            "role": "Engineer",
            "company": "",
            "domain": "engineering",
            "key_skills": [],
            "seniority": "mid",
            "likely_questions": [],
            "topics_to_avoid": []
        })
        .to_string();

        let llm = mock_llm(json);
        let digest = extract_digest("some context", &llm)
            .await
            .expect("should succeed");

        assert_eq!(
            digest.likely_questions.len(),
            5,
            "must be padded to 5 with universal question bank"
        );
        assert!(
            digest.likely_questions[0].contains("Tell me about yourself"),
            "first fallback must be the universal bank's first entry"
        );
    }

    #[tokio::test]
    async fn test_extract_digest_pads_partial_likely_questions() {
        let json = serde_json::json!({
            "role": "PM",
            "company": "Corp",
            "domain": "product management",
            "key_skills": ["roadmapping"],
            "seniority": "senior",
            "likely_questions": ["Why product management?", "Tell me about a launch"],
            "topics_to_avoid": []
        })
        .to_string();

        let llm = mock_llm(json);
        let digest = extract_digest("some context", &llm).await.unwrap();

        assert_eq!(digest.likely_questions.len(), 5);
        // Original 2 kept, 3 padded from universal bank
        assert_eq!(digest.likely_questions[0], "Why product management?");
        assert_eq!(digest.likely_questions[1], "Tell me about a launch");
    }

    #[tokio::test]
    async fn test_extract_digest_strips_markdown_fences() {
        let fenced = format!("```json\n{}\n```", valid_digest_json());
        let llm = mock_llm(fenced);
        let digest = extract_digest("context", &llm).await.unwrap();
        assert_eq!(digest.role, "Senior Software Engineer");
    }

    #[tokio::test]
    async fn test_extract_digest_returns_error_on_invalid_json() {
        let llm = mock_llm("this is not json at all");
        let result = extract_digest("context", &llm).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Digest extraction failed"),
            "error must contain user-facing message, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_extract_digest_returns_error_on_wrong_schema() {
        let llm = mock_llm(r#"{"foo": "bar", "baz": 123}"#);
        let result = extract_digest("context", &llm).await;
        // serde_json will fail because required fields are missing
        assert!(result.is_err());
    }

    #[test]
    fn test_pad_likely_questions_fills_to_five() {
        let mut qs = vec![];
        pad_likely_questions(&mut qs);
        assert_eq!(qs.len(), 5);
        assert_eq!(qs[0], "Tell me about yourself");
    }

    #[test]
    fn test_pad_likely_questions_does_not_exceed_five() {
        let mut qs: Vec<String> = (0..6).map(|i| format!("Question {i}")).collect();
        pad_likely_questions(&mut qs);
        assert_eq!(qs.len(), 6); // already above 5, pad is a no-op
    }

    #[test]
    fn test_pad_likely_questions_skips_duplicates() {
        let mut qs = vec!["Tell me about yourself".to_string()];
        pad_likely_questions(&mut qs);
        // The duplicate should not be added again
        assert_eq!(
            qs.iter()
                .filter(|q| q.as_str() == "Tell me about yourself")
                .count(),
            1
        );
        assert_eq!(qs.len(), 5);
    }

    #[test]
    fn test_strip_markdown_fences_json_prefix() {
        let s = "```json\n{\"key\": \"val\"}\n```";
        assert_eq!(strip_markdown_fences(s), "{\"key\": \"val\"}");
    }

    #[test]
    fn test_strip_markdown_fences_plain_prefix() {
        let s = "```\n{\"key\": \"val\"}\n```";
        assert_eq!(strip_markdown_fences(s), "{\"key\": \"val\"}");
    }

    #[test]
    fn test_strip_markdown_fences_no_fences() {
        let s = "{\"key\": \"val\"}";
        assert_eq!(strip_markdown_fences(s), s);
    }

    #[test]
    fn test_load_digest_prompt_default_exists() {
        // The default prompt file must be present at the expected path.
        let result = load_digest_prompt("nonexistent_provider");
        assert!(
            result.is_ok(),
            "default.txt must exist; got: {:?}",
            result.err()
        );
        let content = result.unwrap();
        assert!(
            content.contains("{pasted_context}"),
            "prompt must contain the {{pasted_context}} placeholder"
        );
    }
}
