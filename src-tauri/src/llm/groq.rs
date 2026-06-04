//! Groq streaming provider (design doc §27).
//!
//! Uses the OpenAI-compatible chat completions API at
//! `https://api.groq.com/openai/v1/chat/completions` with SSE streaming.
//!
//! Rate limits at 80% of free-tier documented values (design doc §29):
//!   - 30 RPM free → 24 RPM enforced
//!   - 30,000 TPM free → 24,000 TPM enforced

use std::pin::Pin;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

use super::provider::{CompletionConfig, LLMProvider, RateLimit};

// ──────────────────────────────────────────────────────────────────────────────
// Constants
// ──────────────────────────────────────────────────────────────────────────────

const GROQ_BASE_URL: &str = "https://api.groq.com/openai/v1/chat/completions";
const DEFAULT_MODEL: &str = "llama-3.3-70b-versatile";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// 128K tokens — documented Groq context window for llama-3.3-70b-versatile.
const CONTEXT_WINDOW: usize = 128_000;
/// 80% of 30 RPM free-tier.
const RATE_LIMIT_RPM: u32 = 24;
/// 80% of 30,000 TPM free-tier.
const RATE_LIMIT_TPM: u32 = 24_000;

// ──────────────────────────────────────────────────────────────────────────────
// Request / response types
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct CompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<usize>,
    temperature: f32,
}

#[derive(Debug, Deserialize)]
struct StreamDelta {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
}

#[derive(Debug, Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
}

// ──────────────────────────────────────────────────────────────────────────────
// Provider
// ──────────────────────────────────────────────────────────────────────────────

pub struct GroqProvider {
    api_key: SecretString,
    model: String,
    client: reqwest::Client,
}

impl GroqProvider {
    pub fn new(api_key: SecretString) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("Failed to build HTTP client")?;

        Ok(Self {
            api_key,
            model: DEFAULT_MODEL.to_string(),
            client,
        })
    }

    /// Override the model (useful in tests or for user-configurable tier).
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Build a completion request body from a prompt string.
    ///
    /// The prompt is injected as a single `user` message so that the system
    /// prompt embedding (handled by prompt templates) is preserved verbatim.
    fn build_request(&self, prompt: &str, config: &CompletionConfig) -> CompletionRequest {
        CompletionRequest {
            model: self.model.clone(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
            stream: config.stream,
            max_tokens: config.max_tokens,
            temperature: config.temperature,
        }
    }

    /// Parse a single `data: {...}` SSE line and extract the token text.
    /// Returns `None` for `data: [DONE]` or malformed lines.
    fn parse_sse_line(line: &str) -> Option<String> {
        let data = line.strip_prefix("data: ")?;
        if data.trim() == "[DONE]" {
            return None;
        }
        let chunk: StreamChunk = serde_json::from_str(data).ok()?;
        chunk
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.delta.content)
    }

    /// Check whether the JSON error body contains a 429-style rate limit.
    fn is_rate_limit_error(body: &Value) -> bool {
        body.get("error")
            .and_then(|e| e.get("type"))
            .and_then(|t| t.as_str())
            .map(|t| t.contains("rate_limit"))
            .unwrap_or(false)
    }
}

#[async_trait]
impl LLMProvider for GroqProvider {
    async fn complete_stream(
        &self,
        prompt: String,
        config: CompletionConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>> {
        let body = self.build_request(&prompt, &config);

        let response = self
            .client
            .post(GROQ_BASE_URL)
            .header(
                AUTHORIZATION,
                format!("Bearer {}", self.api_key.expose_secret()),
            )
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
            .context("Groq API request failed")?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(10);

            warn!(
                retry_after_secs = retry_after,
                "Groq rate limit (429) — caller should honour Retry-After"
            );

            bail!("rate_limit:{retry_after}");
        }

        if !status.is_success() {
            let err_body: Value = response
                .json()
                .await
                .unwrap_or_else(|_| serde_json::json!({"error": {"message": "unknown"}}));

            if Self::is_rate_limit_error(&err_body) {
                bail!("rate_limit:10");
            }

            bail!(
                "Groq API error {status}: {}",
                err_body
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown")
            );
        }

        // Wrap the byte stream as an SSE line stream.
        let byte_stream = response.bytes_stream();
        let line_stream = byte_stream
            .map(|chunk| chunk.context("Groq stream read error"))
            .flat_map(|chunk_result| {
                let lines: Vec<Result<String>> = match chunk_result {
                    Ok(bytes) => String::from_utf8_lossy(&bytes)
                        .lines()
                        .filter(|l| !l.is_empty())
                        .map(|l| Ok(l.to_string()))
                        .collect(),
                    Err(e) => vec![Err(e)],
                };
                futures::stream::iter(lines)
            })
            .filter_map(|line_result| async move {
                match line_result {
                    Err(e) => Some(Err(e)),
                    Ok(line) => {
                        #[cfg(debug_assertions)]
                        debug!(line = %line, "groq sse line");

                        Self::parse_sse_line(&line).map(Ok)
                    }
                }
            });

        Ok(Box::pin(line_stream))
    }

    fn name(&self) -> &str {
        "groq"
    }

    fn is_available(&self) -> bool {
        !self.api_key.expose_secret().is_empty()
    }

    fn context_window(&self) -> usize {
        CONTEXT_WINDOW
    }

    fn rate_limit(&self) -> RateLimit {
        RateLimit {
            requests_per_minute: RATE_LIMIT_RPM,
            tokens_per_minute: RATE_LIMIT_TPM,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sse_line_extracts_token() {
        let line = r#"data: {"id":"1","choices":[{"delta":{"content":"Hello"}}]}"#;
        assert_eq!(
            GroqProvider::parse_sse_line(line),
            Some("Hello".to_string())
        );
    }

    #[test]
    fn parse_sse_line_done_returns_none() {
        assert_eq!(GroqProvider::parse_sse_line("data: [DONE]"), None);
    }

    #[test]
    fn parse_sse_line_non_data_returns_none() {
        assert_eq!(GroqProvider::parse_sse_line("event: message"), None);
    }

    #[test]
    fn parse_sse_line_null_content_returns_none() {
        let line = r#"data: {"choices":[{"delta":{"content":null}}]}"#;
        assert_eq!(GroqProvider::parse_sse_line(line), None);
    }

    #[test]
    fn is_available_false_for_empty_key() {
        let provider = GroqProvider::new(SecretString::new("".into())).unwrap();
        assert!(!provider.is_available());
    }

    #[test]
    fn is_available_true_for_nonempty_key() {
        let provider = GroqProvider::new(SecretString::new("test-key".into())).unwrap();
        assert!(provider.is_available());
    }

    #[test]
    fn rate_limit_at_80_percent() {
        let provider = GroqProvider::new(SecretString::new("key".into())).unwrap();
        let rl = provider.rate_limit();
        assert_eq!(rl.requests_per_minute, 24);
        assert_eq!(rl.tokens_per_minute, 24_000);
    }
}
