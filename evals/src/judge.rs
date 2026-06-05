//! LLM-as-judge for relevance and grounding scores.
//!
//! Uses the existing `OllamaProvider` from `flint_lib` so eval runs are
//! free, local, and deterministic per model+temperature. The judge prompt
//! is loaded from `prompts/eval_judge/default.txt` (or the per-model
//! variant if present) so it is versioned and reviewable like every other
//! prompt.
//!
//! The judge returns a JSON object with two floats in `[0.0, 1.0]`:
//!
//! ```json
//! { "relevance": 0.82, "grounding": 0.71 }
//! ```
//!
//! Free-text after the JSON is ignored — the parser extracts the first
//! valid JSON object from the response.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};

use flint_lib::llm::provider::{CompletionConfig, LLMProvider};

use crate::error::EvalError;

const DEFAULT_JUDGE_PROMPT: &str = include_str!("../prompts/eval_judge/default.txt");

/// Numeric scores returned by the judge. Stored alongside rule-based
/// metrics in the per-row report.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct JudgeScores {
    pub relevance: f32,
    pub grounding: f32,
}

impl JudgeScores {
    pub fn passes_relevance_floor(self) -> bool {
        const RELEVANCE_FLOOR: f32 = 0.7;
        self.relevance >= RELEVANCE_FLOOR
    }
}

/// Strategy seam — production uses [`OllamaJudge`]; tests can substitute a
/// deterministic fake.
#[async_trait]
pub trait Judge: Send + Sync {
    async fn score(&self, request: JudgeRequest<'_>) -> Result<JudgeScores, EvalError>;
}

#[derive(Debug, Clone)]
pub struct JudgeRequest<'a> {
    pub question: &'a str,
    pub context: &'a [String],
    pub response: &'a str,
    pub reference_answer: Option<&'a str>,
}

/// Production judge: wraps an `LLMProvider` and a versioned prompt template.
pub struct OllamaJudge {
    provider: std::sync::Arc<dyn LLMProvider>,
    prompt_template: String,
}

impl OllamaJudge {
    pub fn new(
        provider: std::sync::Arc<dyn LLMProvider>,
        prompts_dir: Option<&Path>,
    ) -> Result<Self, EvalError> {
        let prompt_template = load_judge_prompt(prompts_dir)?;
        Ok(Self {
            provider,
            prompt_template,
        })
    }

    fn build_prompt(&self, request: &JudgeRequest<'_>) -> String {
        let context_block = if request.context.is_empty() {
            "(no context provided)".to_string()
        } else {
            request
                .context
                .iter()
                .enumerate()
                .map(|(i, c)| format!("[{}] {}", i + 1, c))
                .collect::<Vec<_>>()
                .join("\n\n")
        };
        let reference_block = request
            .reference_answer
            .map(|r| format!("Reference answer (north star):\n{r}"))
            .unwrap_or_else(|| "Reference answer: (none provided)".to_string());

        self.prompt_template
            .replace("{question}", request.question)
            .replace("{context}", &context_block)
            .replace("{reference}", &reference_block)
            .replace("{response}", request.response)
    }
}

#[async_trait]
impl Judge for OllamaJudge {
    async fn score(&self, request: JudgeRequest<'_>) -> Result<JudgeScores, EvalError> {
        let prompt = self.build_prompt(&request);
        let cfg = CompletionConfig {
            temperature: 0.0,
            max_tokens: Some(200),
            stream: true,
        };
        let mut stream = self
            .provider
            .complete_stream(prompt, cfg)
            .await
            .map_err(|e| EvalError::Judge(format!("provider call failed: {e}")))?;

        let mut buffer = String::new();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(text) => buffer.push_str(&text),
                Err(e) => return Err(EvalError::Judge(format!("stream error: {e}"))),
            }
        }
        parse_judge_response(&buffer).map_err(|e| EvalError::Judge(e.to_string()))
    }
}

fn load_judge_prompt(prompts_dir: Option<&Path>) -> Result<String, EvalError> {
    if let Some(dir) = prompts_dir {
        let candidate: PathBuf = dir.join("eval_judge").join("default.txt");
        if candidate.exists() {
            return std::fs::read_to_string(&candidate).map_err(|e| EvalError::BaselineRead {
                path: candidate.display().to_string(),
                source: e,
            });
        }
    }
    Ok(DEFAULT_JUDGE_PROMPT.to_string())
}

/// Pulls the first balanced `{...}` block from the judge output and parses
/// it into [`JudgeScores`]. Tolerates leading prose and trailing
/// commentary, which Ollama sometimes emits even with strict instructions.
fn parse_judge_response(raw: &str) -> Result<JudgeScores, anyhow::Error> {
    let Some(start) = raw.find('{') else {
        return Err(anyhow!("no JSON object found in judge output: {raw}"));
    };
    let mut depth = 0i32;
    let mut end = None;
    for (i, ch) in raw[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(start + i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end.context("unbalanced braces in judge output")?;
    let slice = &raw[start..end];
    let parsed: JudgeScores =
        serde_json::from_str(slice).with_context(|| format!("invalid judge JSON: {slice}"))?;

    if !(0.0..=1.0).contains(&parsed.relevance) || !(0.0..=1.0).contains(&parsed.grounding) {
        return Err(anyhow!(
            "judge produced out-of-range scores: relevance={}, grounding={}",
            parsed.relevance,
            parsed.grounding
        ));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_judge_json() {
        let raw = r#"{"relevance": 0.8, "grounding": 0.6}"#;
        let scores = parse_judge_response(raw).expect("parse");
        assert!((scores.relevance - 0.8).abs() < f32::EPSILON);
        assert!((scores.grounding - 0.6).abs() < f32::EPSILON);
    }

    #[test]
    fn extracts_json_from_prose_wrapper() {
        let raw = "Here is my assessment:\n{\"relevance\": 0.95, \"grounding\": 0.5}\nDone.";
        let scores = parse_judge_response(raw).expect("parse");
        assert!(scores.passes_relevance_floor());
    }

    #[test]
    fn rejects_scores_out_of_range() {
        let raw = r#"{"relevance": 1.5, "grounding": 0.1}"#;
        assert!(parse_judge_response(raw).is_err());
    }

    #[test]
    fn rejects_missing_json() {
        let raw = "I refuse to score this.";
        assert!(parse_judge_response(raw).is_err());
    }
}
