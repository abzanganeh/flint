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
use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

use super::provider::{CompletionConfig, LLMProvider, RateLimit};
use super::sse_lines::buffered_lines;

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

#[derive(Debug, Deserialize)]
struct NonStreamMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NonStreamChoice {
    message: NonStreamMessage,
}

#[derive(Debug, Deserialize)]
struct NonStreamCompletion {
    choices: Vec<NonStreamChoice>,
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

    /// Wraps a raw byte stream as a token stream: reassembles complete SSE
    /// lines across chunk boundaries via `buffered_lines`, then applies the
    /// existing per-line parser unchanged. Extracted from `complete_stream`
    /// so the chunk-boundary regression test can drive it with a synthetic
    /// byte stream that deliberately splits a `data: {...}` line mid-JSON.
    fn token_stream_from_bytes(
        byte_stream: impl Stream<Item = Result<Bytes>> + Send + Unpin + 'static,
    ) -> Pin<Box<dyn Stream<Item = Result<String>> + Send>> {
        let line_stream = buffered_lines(byte_stream).filter_map(|line_result| async move {
            match line_result {
                Err(e) => Some(Err(e)),
                Ok(line) => {
                    #[cfg(debug_assertions)]
                    debug!(line = %line, "groq sse line");

                    Self::parse_sse_line(&line).map(Ok)
                }
            }
        });
        Box::pin(line_stream)
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

        // Digest extraction and other one-shot callers use stream=false, which
        // returns a plain JSON body — not SSE `data:` lines.
        if !config.stream {
            let completion: NonStreamCompletion = response
                .json()
                .await
                .context("Groq non-streaming response decode failed")?;
            let content = completion
                .choices
                .into_iter()
                .next()
                .and_then(|c| c.message.content)
                .unwrap_or_default();
            let stream = futures::stream::once(async move { Ok(content) });
            return Ok(Box::pin(stream));
        }

        let byte_stream = response
            .bytes_stream()
            .map(|chunk| chunk.context("Groq stream read error"));
        Ok(Self::token_stream_from_bytes(byte_stream))
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
    fn parse_non_stream_completion_json() {
        let body =
            r#"{"choices":[{"message":{"role":"assistant","content":"{\"role\":\"Engineer\"}"}}]}"#;
        let completion: NonStreamCompletion = serde_json::from_str(body).unwrap();
        let content = completion
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_default();
        assert!(content.contains("Engineer"));
    }

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

    /// Regression test for the corrupted-Mermaid-diagram bug: a `data: {...}`
    /// SSE line split mid-JSON across two network chunks must reassemble
    /// into one complete token, not be silently dropped by both halves
    /// failing to parse independently.
    #[tokio::test]
    async fn token_stream_reassembles_sse_line_split_mid_json_across_chunks() {
        let first_chunk = Bytes::from_static(b"data: {\"id\":\"1\",\"choices\":[{\"delta\":");
        let second_chunk = Bytes::from_static(b"{\"content\":\"Twilio\"}}]}\n");
        let byte_stream = futures::stream::iter(vec![Ok(first_chunk), Ok(second_chunk)]);

        let mut token_stream = GroqProvider::token_stream_from_bytes(byte_stream);
        let mut tokens = Vec::new();
        while let Some(result) = token_stream.next().await {
            tokens.push(result.expect("token stream should not error"));
        }

        assert_eq!(tokens, vec!["Twilio".to_string()]);
    }
}
